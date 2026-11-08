use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{DataSegmentDescriptorCache, DsDescriptorCache, FsDescriptorCache, Gdtr};
use od_isa::{
    CpuActivity, INTEGER_FLAGS, LongModeState, PagedProbeExit, PagedProbeRejection, StiShadow,
};
use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, GuestPhysicalRange,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    PagedSegmentMovReport, RamLimits, TrackedAccessError,
};
use std::sync::{Arc, Mutex};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const GDT_LINEAR: u64 = 0x0080_0000;
const GDT_PHYSICAL: u64 = 0x9000;
const GDT_PTE: u64 = 0x5000;
const SELECTOR: u16 = 8;
const RAM_LEN: u64 = 0x20_000;

#[derive(Clone, Copy, Debug)]
enum Seg {
    Ds,
    Es,
    Fs,
    Gs,
}

const ALL: [Seg; 4] = [Seg::Ds, Seg::Es, Seg::Fs, Seg::Gs];

impl Seg {
    fn modrm(self) -> u8 {
        match self {
            Seg::Ds => 0xd8,
            Seg::Es => 0xc0,
            Seg::Fs => 0xe0,
            Seg::Gs => 0xe8,
        }
    }
}

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn put(bus: &mut MachineBus, address: u64, value: u64) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Qword,
        value,
        |_| {},
    )
    .unwrap();
}

fn put_bytes(bus: &mut MachineBus, address: u64, bytes: &[u8]) {
    for (offset, byte) in bytes.iter().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(address + offset as u64),
            AccessWidth::Byte,
            u64::from(*byte),
            |_| {},
        )
        .unwrap();
    }
}

fn get(bus: &mut MachineBus, address: u64, len: usize) -> Vec<u8> {
    (0..len)
        .map(|offset| {
            bus.read_physical(
                GuestPhysicalAddress::new(address + offset as u64),
                AccessWidth::Byte,
            )
            .unwrap() as u8
        })
        .collect()
}

/// Present, DPL0, writable data segment, A=0; base 0x12345678, G=1 D/B=1,
/// limit 0xfffff.
fn data_descriptor() -> [u8; 8] {
    [0xff, 0xff, 0x78, 0x56, 0x34, 0x92, 0xcf, 0x12]
}

fn make_bus(descriptor: [u8; 8]) -> MachineBus {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    put(&mut bus, 0x1000, 0x2007);
    put(&mut bus, 0x2000, 0x3007);
    put(&mut bus, 0x3010, 0x4007);
    put(&mut bus, 0x3020, 0x5007);
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7);
    put(&mut bus, GDT_PTE, GDT_PHYSICAL | 7);
    put_bytes(&mut bus, GDT_PHYSICAL + 8, &descriptor);
    bus
}

fn install_code(bus: &mut MachineBus, seg: Seg) {
    put_bytes(bus, CODE_PHYSICAL, &[0x8e, seg.modrm()]);
}

fn state() -> LongModeState {
    let mut state = LongModeState {
        rip: CODE_LINEAR,
        cr0: Some(CR0),
        cr4: Some(CR4),
        cr3: Some(ROOT),
        gdtr: Some(Gdtr {
            base: GDT_LINEAR,
            limit: 0x1f,
        }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_cache: Some(DsDescriptorCache {
            base: 0,
            limit: 0,
            access: 0x93,
            flags: 0,
        }),
        es_cache: Some(DataSegmentDescriptorCache {
            base: 0,
            limit: 0,
            access: 0x93,
            flags: 0,
        }),
        fs_cache: Some(FsDescriptorCache {
            limit: 0,
            access: 0x93,
            flags: 0,
        }),
        gs_cache: Some(FsDescriptorCache {
            limit: 0,
            access: 0x93,
            flags: 0,
        }),
        ..LongModeState::default()
    };
    state.gpr[0] = 0xdead_0000 | u64::from(SELECTOR);
    state.gpr[3] = 0x1122_3344;
    state
}

fn run(
    seg: Seg,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
) -> (PagedSegmentMovReport, Vec<(u64, u64)>) {
    let mut log = Vec::new();
    let mut commit = |range: GuestPhysicalRange| log.push((range.start().get(), range.len()));
    let (ctx, flags) = (context(privilege), INTEGER_FLAGS & !(1 << 7));
    let report = match seg {
        Seg::Ds => od_machine::step_paged_mov_ds(state, bus, ROOT, EFER, ctx, flags, &mut commit),
        Seg::Es => od_machine::step_paged_mov_es(state, bus, ROOT, EFER, ctx, flags, &mut commit),
        Seg::Fs => od_machine::step_paged_mov_fs(state, bus, ROOT, EFER, ctx, flags, &mut commit),
        Seg::Gs => od_machine::step_paged_mov_gs(state, bus, ROOT, EFER, ctx, flags, &mut commit),
    }
    .unwrap();
    (report, log)
}

fn expect_fault(report: &PagedSegmentMovReport, vector: u8, code: Option<u32>, what: &str) {
    let PagedProbeExit::Fault(fault) = &report.cpu.exit else {
        panic!("{what}: expected fault, got {:?}", report.cpu.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (vector, code), "{what}");
    assert_eq!(report.cpu.instructions_executed, 0, "{what}");
}

fn leaf(bus: &mut MachineBus) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(GDT_PTE), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn mov_sreg_loads_cache_marks_accessed_and_commits_last() {
    for seg in ALL {
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{seg:?}");
        assert_eq!(report.cpu.instructions_executed, 1);

        let mut expected = before;
        expected.rip = CODE_LINEAR + 2;
        match seg {
            Seg::Ds => {
                expected.ds_selector = SELECTOR;
                expected.ds_cache = Some(DsDescriptorCache {
                    base: 0x1234_5678,
                    limit: 0xffff_ffff,
                    access: 0x93,
                    flags: 0xc,
                });
            }
            Seg::Es => {
                expected.es_selector = SELECTOR;
                expected.es_cache = Some(DataSegmentDescriptorCache {
                    base: 0x1234_5678,
                    limit: 0xffff_ffff,
                    access: 0x93,
                    flags: 0xc,
                });
            }
            Seg::Fs => {
                expected.fs_selector = SELECTOR;
                expected.fs_base = 0x1234_5678;
                expected.fs_cache = Some(FsDescriptorCache {
                    limit: 0xffff_ffff,
                    access: 0x93,
                    flags: 0xc,
                });
            }
            Seg::Gs => {
                expected.gs_selector = SELECTOR;
                expected.gs_base = 0x1234_5678;
                expected.gs_cache = Some(FsDescriptorCache {
                    limit: 0xffff_ffff,
                    access: 0x93,
                    flags: 0xc,
                });
            }
        }
        assert_eq!(
            cpu, expected,
            "{seg:?}: only the loaded segment and RIP change"
        );
        // The accessed-bit write is a one-byte store; the leaf gets A and D first.
        assert_eq!(get(&mut bus, GDT_PHYSICAL + 8, 8)[5], 0x93);
        assert_eq!(leaf(&mut bus) & 0x60, 0x60);
        assert_eq!(
            log[log.len() - 2..],
            [(GDT_PTE, 8), (GDT_PHYSICAL + 8 + 5, 1)],
            "{seg:?}"
        );
    }
}

#[test]
fn mov_sreg_with_accessed_descriptor_writes_nothing() {
    for seg in ALL {
        let mut descriptor = data_descriptor();
        descriptor[5] = 0x93;
        let mut bus = make_bus(descriptor);
        install_code(&mut bus, seg);
        let mut cpu = state();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{seg:?}");
        assert!(
            !log.iter().any(|(_, len)| *len == 1),
            "{seg:?}: no byte write for an already accessed descriptor"
        );
        assert_eq!(leaf(&mut bus) & 0x40, 0, "{seg:?}: leaf stays clean");
    }
}

#[test]
fn mov_sreg_descriptor_faults_preserve_cpu_and_memory() {
    type Mutate = Box<dyn Fn(&mut LongModeState, &mut [u8; 8])>;
    let cases: Vec<(&str, Mutate, u8, u32)> = vec![
        (
            "beyond GDT limit",
            Box::new(|s, _| s.gpr[0] = 0x20),
            13,
            0x20,
        ),
        ("not present", Box::new(|_, d| d[5] = 0x12), 11, 8),
        ("system descriptor", Box::new(|_, d| d[5] = 0x82), 13, 8),
        ("execute-only code", Box::new(|_, d| d[5] = 0x98), 13, 8),
        ("RPL above DPL", Box::new(|s, _| s.gpr[0] = 0x0b), 13, 8),
    ];
    for (name, mutate, vector, code) in cases {
        for seg in ALL {
            let mut descriptor = data_descriptor();
            let mut cpu = state();
            mutate(&mut cpu, &mut descriptor);
            let mut bus = make_bus(descriptor);
            install_code(&mut bus, seg);
            let before = cpu.clone();
            let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            expect_fault(&report, vector, Some(code), &format!("{name} {seg:?}"));
            assert_eq!(cpu, before, "{name} {seg:?}");
            assert_eq!(
                get(&mut bus, GDT_PHYSICAL + 8, 8),
                descriptor,
                "{name} {seg:?}"
            );
            assert_eq!(leaf(&mut bus) & 0x40, 0, "{name} {seg:?}: no D bit");
        }
    }
}

#[test]
fn mov_sreg_null_ldt_and_unsupported_forms_are_rejected_without_state_change() {
    for seg in ALL {
        for selector in [0u64, 3, 0xc] {
            let mut bus = make_bus(data_descriptor());
            install_code(&mut bus, seg);
            let mut cpu = state();
            cpu.gpr[0] = selector;
            let before = cpu.clone();
            let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            assert!(
                matches!(
                    report.cpu.exit,
                    PagedProbeExit::Unsupported { opcode: 0x8e }
                ),
                "{seg:?} selector {selector:#x}: {:?}",
                report.cpu.exit
            );
            assert_eq!(cpu, before);
        }
        // Wrong second byte (another segment register / non-AX source).
        let mut bus = make_bus(data_descriptor());
        put_bytes(&mut bus, CODE_PHYSICAL, &[0x8e, 0xd9]);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { opcode: 0xd9 }
        ));
        assert_eq!(cpu, before);
        assert!(!log.iter().any(|(address, _)| *address == GDT_PTE));
        // Missing hidden cache for the requested segment.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        match seg {
            Seg::Ds => cpu.ds_cache = None,
            Seg::Es => cpu.es_cache = None,
            Seg::Fs => cpu.fs_cache = None,
            Seg::Gs => cpu.gs_cache = None,
        }
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { opcode: 0x8e }
        ));
        assert_eq!(cpu, before);
    }
}

#[test]
fn mov_sreg_requires_cpl0_and_exact_profile_before_any_walk() {
    for seg in ALL {
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring3);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { opcode: 0x8e }
        ));
        assert_eq!(cpu, before);
        assert!(log.is_empty(), "{seg:?}: CPL3 never fetches");

        for (mutate, expected) in [
            (
                (|s: &mut LongModeState| s.cr0 = Some(CR0 & !(1 << 16))) as fn(&mut LongModeState),
                PagedProbeRejection::ControlRegistersOutsideProfile,
            ),
            (
                |s| s.gdtr = None,
                PagedProbeRejection::ControlRegistersMissing,
            ),
            (
                |s| s.sti_shadow = Some(StiShadow::Blocking),
                PagedProbeRejection::StiShadowActive,
            ),
        ] {
            let mut bus = make_bus(data_descriptor());
            install_code(&mut bus, seg);
            let mut cpu = state();
            mutate(&mut cpu);
            let before = cpu.clone();
            let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            assert_eq!(
                report.cpu.exit,
                PagedProbeExit::Rejected(expected),
                "{seg:?}"
            );
            assert_eq!(cpu, before);
            assert!(log.is_empty());
        }

        // Two instruction bytes never straddle the code page.
        let mut bus = make_bus(data_descriptor());
        let mut cpu = state();
        cpu.rip = CODE_LINEAR + 0xfff;
        let before = cpu.clone();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { opcode: 0x8e }
        ));
        assert_eq!(cpu, before);
        assert!(log.is_empty());
    }
}

#[test]
fn mov_sreg_gdt_page_faults_are_precise() {
    for seg in ALL {
        // Read-only GDT page, CR0.WP=1, A=0: the accessed-bit store faults.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        put(&mut bus, GDT_PTE, GDT_PHYSICAL | 5);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        let record = report.page_fault.as_ref().expect("write protection fault");
        assert_eq!(record.fault().error_code, Some(3), "{seg:?}");
        assert_eq!(record.cr2_value(), GDT_LINEAR + 8 + 5, "{seg:?}");
        assert_eq!(record.saved_rip(), CODE_LINEAR);
        assert!(matches!(report.cpu.exit, PagedProbeExit::Fault(_)));
        assert_eq!(cpu, before);
        assert_eq!(get(&mut bus, GDT_PHYSICAL + 8, 8), data_descriptor());

        // Read-only page with an already accessed descriptor needs no write.
        let mut descriptor = data_descriptor();
        descriptor[5] = 0x93;
        let mut bus = make_bus(descriptor);
        install_code(&mut bus, seg);
        put(&mut bus, GDT_PTE, GDT_PHYSICAL | 5);
        let mut cpu = state();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{seg:?}");

        // Not-present GDT page: read fault, error code 0.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        put(&mut bus, GDT_PTE, 0);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        let record = report.page_fault.as_ref().expect("not present");
        assert_eq!(record.fault().error_code, Some(0));
        assert_eq!(record.cr2_value(), GDT_LINEAR + 8);
        assert_eq!(cpu, before);
    }
}

#[test]
fn mov_sreg_descriptor_crossing_a_page_or_outside_ram_is_not_a_guest_exception() {
    for seg in ALL {
        // 8-byte descriptor crossing a 4 KiB boundary: Unsupported.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        cpu.gdtr = Some(Gdtr {
            base: GDT_LINEAR + 0xff4,
            limit: 0x1f,
        });
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { opcode: 0x8e }
        ));
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::CrossesPage { .. })
        ));
        assert_eq!(cpu, before);

        // GDT page mapped outside RAM: backend error, CPU unchanged.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        put(&mut bus, GDT_PTE, 0x30_000 | 7);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(
            matches!(report.cpu.exit, PagedProbeExit::Backend(_)),
            "{seg:?}"
        );
        assert!(report.memory_error.is_some());
        assert!(report.page_fault.is_none());
        assert_eq!(cpu, before);
    }
}

fn gdtr_for_address(address: u64) -> Gdtr {
    // The selector is 8, so the descriptor lives at base + 8.
    Gdtr {
        base: address.wrapping_sub(8),
        limit: 0x1f,
    }
}

#[test]
fn mov_sreg_descriptor_offsets_at_the_page_end_are_exact() {
    for seg in ALL {
        // Last admissible offset: the 8 descriptor bytes end exactly at the page end.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        put_bytes(&mut bus, GDT_PHYSICAL + 0xff8, &data_descriptor());
        let mut cpu = state();
        cpu.gdtr = Some(gdtr_for_address(GDT_LINEAR + 0xff8));
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{seg:?} at 0xff8");
        assert_eq!(get(&mut bus, GDT_PHYSICAL + 0xff8, 8)[5], 0x93);

        // One byte further the qword read crosses into the next page.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        cpu.gdtr = Some(gdtr_for_address(GDT_LINEAR + 0xff9));
        let before = cpu.clone();
        let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(
            matches!(
                report.cpu.exit,
                PagedProbeExit::Unsupported { opcode: 0x8e }
            ),
            "{seg:?} at 0xff9: {:?}",
            report.cpu.exit
        );
        assert!(matches!(
            report.memory_error,
            Some(TrackedAccessError::CrossesPage { .. })
        ));
        assert_eq!(cpu, before);
    }
}

#[test]
fn mov_sreg_prefixed_forms_are_unsupported_before_any_gdt_access() {
    for seg in ALL {
        for prefix in [0x66u8, 0x67, 0xf0, 0xf3, 0x2e, 0x48] {
            let mut bus = make_bus(data_descriptor());
            put_bytes(&mut bus, CODE_PHYSICAL, &[prefix, 0x8e, seg.modrm()]);
            let mut cpu = state();
            let before = cpu.clone();
            let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            assert!(
                matches!(report.cpu.exit, PagedProbeExit::Unsupported { opcode } if opcode == prefix),
                "{seg:?} prefix {prefix:#x}: {:?}",
                report.cpu.exit
            );
            assert_eq!(cpu, before);
            assert!(!log.iter().any(|(address, _)| *address == GDT_PTE));
        }
    }
}

#[test]
fn mov_sreg_noncanonical_or_wrapping_descriptor_address_is_gp_selector() {
    for seg in ALL {
        for base in [
            0x0000_7fff_ffff_fff8u64, // base + 8 is non-canonical
            0x0000_7fff_ffff_fff4,    // address canonical, address + 7 is not
            u64::MAX - 3,             // base + 8 overflows
        ] {
            let mut bus = make_bus(data_descriptor());
            install_code(&mut bus, seg);
            let mut cpu = state();
            cpu.gdtr = Some(Gdtr { base, limit: 0x1f });
            let before = cpu.clone();
            let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            expect_fault(&report, 13, Some(8), &format!("{seg:?} base {base:#x}"));
            assert_eq!(cpu, before);
            assert!(!log.iter().any(|(address, _)| *address == GDT_PTE));
        }
    }
}

struct CountingMmio(Arc<Mutex<usize>>);

impl BusDevice for CountingMmio {
    fn read(&mut self, _access: DeviceAccess) -> Result<u64, DeviceError> {
        *self.0.lock().unwrap() += 1;
        Ok(0xfeed_face_cafe_beef)
    }

    fn write(&mut self, _access: DeviceAccess, _value: u64) -> Result<(), DeviceError> {
        *self.0.lock().unwrap() += 1;
        Ok(())
    }
}

fn bus_with_descriptor_region(case: u8) -> (MachineBus, Arc<Mutex<usize>>) {
    // case 0: ROM, 1: MMIO without a bound device, 2: RAM ending inside the descriptor.
    let ram_len = if case == 2 {
        GDT_PHYSICAL + 0xd
    } else {
        RAM_LEN
    };
    let mut regions = vec![MemoryRegionSpec::new(
        GuestPhysicalRange::new(GuestPhysicalAddress::new(0), ram_len).unwrap(),
        MemoryRegionKind::Ram,
    )];
    let special = match case {
        0 => Some(MemoryRegionKind::Rom),
        1 => Some(MemoryRegionKind::Mmio),
        _ => None,
    };
    if let Some(kind) = special {
        regions.push(MemoryRegionSpec::new(
            GuestPhysicalRange::new(GuestPhysicalAddress::new(0x30_000), 0x1000).unwrap(),
            kind,
        ));
    }
    let map = MachineMemoryMap::new(36, regions).unwrap();
    let dispatches = Arc::new(Mutex::new(0));
    let (devices, bindings): (Vec<Box<dyn BusDevice>>, Vec<MmioBinding>) = if case == 1 {
        (
            vec![Box::new(CountingMmio(dispatches.clone()))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(0x30_000),
                device_index: 0,
            }],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut bus = MachineBus::new(map, RamLimits::new(ram_len), devices, bindings, vec![]).unwrap();
    put(&mut bus, 0x1000, 0x2007);
    put(&mut bus, 0x2000, 0x3007);
    put(&mut bus, 0x3010, 0x4007);
    put(&mut bus, 0x3020, 0x5007);
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7);
    let gdt_physical = if special.is_some() {
        0x30_000
    } else {
        GDT_PHYSICAL
    };
    put(&mut bus, GDT_PTE, gdt_physical | 7);
    (bus, dispatches)
}

#[test]
fn mov_sreg_rom_mmio_and_partial_ram_descriptors_are_backend_errors() {
    for seg in ALL {
        for case in 0..3u8 {
            let (mut bus, dispatches) = bus_with_descriptor_region(case);
            install_code(&mut bus, seg);
            let mut cpu = state();
            let before = cpu.clone();
            let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            assert!(
                matches!(report.cpu.exit, PagedProbeExit::Backend(_)),
                "{seg:?} case {case}: {:?}",
                report.cpu.exit
            );
            assert!(report.memory_error.is_some() && report.page_fault.is_none());
            assert_eq!(cpu, before, "{seg:?} case {case}");
            // Only walk A-bit updates happened; no descriptor byte was written.
            assert!(!log.iter().any(|(_, len)| *len == 1), "{seg:?} case {case}");
            assert_eq!(
                *dispatches.lock().unwrap(),
                0,
                "no device dispatch, case {case}"
            );
        }
    }
}

#[test]
fn mov_sreg_accepts_readable_code_conforming_and_dpl3_with_matching_rpl() {
    type Setup = fn(&mut LongModeState, &mut [u8; 8]);
    let cases: [(&str, Setup, u8, u16); 4] = [
        ("readable code", |_, d| d[5] = 0x9a, 0x9b, 8),
        // Conforming readable code skips the privilege comparison, even with RPL 3.
        (
            "conforming readable code, RPL 3",
            |s, d| {
                d[5] = 0x9e;
                s.gpr[0] = 0x0b;
            },
            0x9f,
            0x0b,
        ),
        (
            "DPL3 data with RPL 3",
            |s, d| {
                d[5] = 0xf2;
                s.gpr[0] = 0x0b;
            },
            0xf3,
            0x0b,
        ),
        // Already accessed descriptors are loaded unchanged.
        ("accessed data", |_, d| d[5] = 0x93, 0x93, 8),
    ];
    for (name, setup, final_access, final_selector) in cases {
        for seg in ALL {
            let mut descriptor = data_descriptor();
            let mut cpu = state();
            setup(&mut cpu, &mut descriptor);
            let mut bus = make_bus(descriptor);
            install_code(&mut bus, seg);
            let (report, _) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
            assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{name} {seg:?}");
            assert_eq!(
                get(&mut bus, GDT_PHYSICAL + 8, 8)[5],
                final_access,
                "{name} {seg:?}"
            );
            let loaded = match seg {
                Seg::Ds => (cpu.ds_selector, cpu.ds_cache.unwrap().access),
                Seg::Es => (cpu.es_selector, cpu.es_cache.unwrap().access),
                Seg::Fs => (cpu.fs_selector, cpu.fs_cache.unwrap().access),
                Seg::Gs => (cpu.gs_selector, cpu.gs_cache.unwrap().access),
            };
            assert_eq!(loaded, (final_selector, final_access), "{name} {seg:?}");
        }
    }
}

#[test]
fn mov_sreg_halted_cpu_does_not_fetch() {
    for seg in ALL {
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        let mut cpu = state();
        cpu.activity = Some(CpuActivity::Halted);
        let before = cpu.clone();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Halted);
        assert_eq!(cpu, before);
        assert!(log.is_empty());
    }
}

#[test]
fn mov_sreg_aliased_descriptor_walks_keep_a_then_d_then_accessed_byte_order() {
    for seg in ALL {
        // The GDT linear page maps onto the page-table page that holds its own leaf
        // entry, so the descriptor (PT index 1) shares a physical page with the PT.
        let mut bus = make_bus(data_descriptor());
        install_code(&mut bus, seg);
        put(&mut bus, GDT_PTE, GDT_PTE | 7);
        put_bytes(&mut bus, GDT_PTE + 8, &data_descriptor());
        let mut cpu = state();
        let (report, log) = run(seg, &mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Retired, "{seg:?}");
        // Leaf PTE: A set by the read walk, D by the accessed-byte store walk.
        assert_eq!(leaf(&mut bus), GDT_PTE | 0x67, "{seg:?}");
        assert_eq!(get(&mut bus, GDT_PTE + 8, 8)[5], 0x93);
        assert_eq!(
            log[log.len() - 2..],
            [(GDT_PTE, 8), (GDT_PTE + 8 + 5, 1)],
            "{seg:?}: D write precedes the accessed-bit byte"
        );
    }
}
