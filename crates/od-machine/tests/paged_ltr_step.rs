use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::Gdtr;
use od_isa::{
    CpuActivity, INTEGER_FLAGS, LongModeState, PagedProbeExit, PagedProbeRejection, StiShadow,
    TaskRegisterCache,
};
use od_machine::{
    AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PagedLtrRejection, RamLimits,
};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const GDT_LINEAR: u64 = 0x0080_0000;
const GDT_PHYSICAL: u64 = 0x9000;
const GDT_PTE: u64 = 0x5000; // PT entry 0 maps GDT_LINEAR
const SELECTOR: u16 = 8;
const RAM_LEN: u64 = 0x20_000;

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

/// Available 64-bit TSS, base 0xffff_8000_1234_5678, limit 0x14567, G=1 AVL=1.
fn tss_descriptor() -> [u8; 16] {
    [
        0x67, 0x45, 0x78, 0x56, 0x34, 0x89, 0x91, 0x12, 0x00, 0x80, 0xff, 0xff, 0, 0, 0, 0,
    ]
}

fn make_bus() -> MachineBus {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    put(&mut bus, 0x1000, 0x2007);
    put(&mut bus, 0x2000, 0x3007);
    put(&mut bus, 0x3010, 0x4007); // PD index 2: code page table
    put(&mut bus, 0x3020, 0x5007); // PD index 4: GDT page table
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7);
    put(&mut bus, GDT_PTE, GDT_PHYSICAL | 7);
    put_bytes(&mut bus, GDT_PHYSICAL + 8, &tss_descriptor());
    put_bytes(&mut bus, CODE_PHYSICAL, &[0x0f, 0x00, 0xd8]);
    bus
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
        ..LongModeState::default()
    };
    state.gpr[0] = 0xdead_0000 | u64::from(SELECTOR);
    state.gpr[3] = 0x1122_3344;
    state
}

fn run(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
) -> (od_machine::PagedLtrReport, Vec<(u64, u64)>) {
    let mut log = Vec::new();
    let report = od_machine::step_paged_ltr_ax(
        state,
        bus,
        ROOT,
        EFER,
        context(privilege),
        INTEGER_FLAGS & !(1 << 7),
        |range| log.push((range.start().get(), range.len())),
    )
    .unwrap();
    (report, log)
}

#[test]
fn ltr_ax_loads_tr_marks_descriptor_busy_and_dirties_leaf_in_order() {
    let mut bus = make_bus();
    let mut cpu = state();
    let before = cpu.clone();
    let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert!(report.page_fault.is_none() && report.rejection.is_none());

    let mut expected = before;
    expected.rip = CODE_LINEAR + 3;
    expected.tr = Some(TaskRegisterCache {
        selector: SELECTOR,
        base: 0xffff_8000_1234_5678,
        limit: 0x14567fff,
        access: 0x8b,
        flags: 9,
    });
    assert_eq!(cpu, expected, "only TR and RIP change");

    let mut busy = tss_descriptor();
    busy[5] = 0x8b;
    assert_eq!(get(&mut bus, GDT_PHYSICAL + 8, 16), busy);
    // Leaf PTE got A and D; the D write precedes the descriptor write.
    let leaf = bus
        .read_physical(GuestPhysicalAddress::new(GDT_PTE), AccessWidth::Qword)
        .unwrap();
    assert_eq!(leaf & 0x60, 0x60);
    let tail = &log[log.len() - 2..];
    assert_eq!(tail, [(GDT_PTE, 8), (GDT_PHYSICAL + 8, 16)]);
}

#[test]
fn ltr_ax_with_dirty_leaf_writes_only_the_descriptor() {
    let mut bus = make_bus();
    put(&mut bus, GDT_PTE, GDT_PHYSICAL | 0x67);
    let mut cpu = state();
    let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
    assert_eq!(log.last(), Some(&(GDT_PHYSICAL + 8, 16)));
    assert!(!log.contains(&(GDT_PTE, 8)), "leaf was already dirty");
}

#[test]
fn ltr_ax_descriptor_faults_preserve_cpu_and_memory() {
    let cases: [(
        &str,
        Box<dyn Fn(&mut LongModeState, &mut [u8; 16])>,
        u8,
        u32,
    ); 7] = [
        ("null selector", Box::new(|s, _| s.gpr[0] = 0), 13, 0),
        (
            "selector beyond GDT limit",
            Box::new(|s, _| s.gpr[0] = 0x18),
            13,
            0x18,
        ),
        ("LDT selector", Box::new(|s, _| s.gpr[0] = 0xc), 13, 0xc),
        (
            "busy TSS",
            Box::new(|_, d| d[5] = 0x8b),
            13,
            u32::from(SELECTOR),
        ),
        (
            "not a TSS",
            Box::new(|_, d| d[5] = 0x92),
            13,
            u32::from(SELECTOR),
        ),
        (
            "not present",
            Box::new(|_, d| d[5] = 0x09),
            11,
            u32::from(SELECTOR),
        ),
        ("non-canonical base", Box::new(|_, d| d[9] = 0x01), 13, 0),
    ];
    for (name, mutate, vector, code) in cases {
        let mut bus = make_bus();
        let mut cpu = state();
        let mut descriptor = tss_descriptor();
        mutate(&mut cpu, &mut descriptor);
        put_bytes(&mut bus, GDT_PHYSICAL + 8, &descriptor);
        let before = cpu.clone();
        let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
        let PagedProbeExit::Fault(fault) = report.cpu.exit else {
            panic!("{name}: expected fault, got {:?}", report.cpu.exit)
        };
        assert_eq!(
            (fault.vector, fault.error_code),
            (vector, Some(code)),
            "{name}"
        );
        assert_eq!(report.cpu.instructions_executed, 0, "{name}");
        assert_eq!(cpu, before, "{name}");
        assert_eq!(get(&mut bus, GDT_PHYSICAL + 8, 16), descriptor, "{name}");
        let leaf = bus
            .read_physical(GuestPhysicalAddress::new(GDT_PTE), AccessWidth::Qword)
            .unwrap();
        assert_eq!(leaf & 0x40, 0, "{name}: leaf must not be dirtied");
    }
}

#[test]
fn ltr_ax_requires_cpl0_and_rejects_lock_and_other_encodings() {
    let mut bus = make_bus();
    let mut cpu = state();
    let before = cpu.clone();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring3);
    let PagedProbeExit::Fault(fault) = report.cpu.exit else {
        panic!("{:?}", report.cpu.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (13, Some(0)));
    assert_eq!(cpu, before);

    let mut bus = make_bus();
    put_bytes(&mut bus, CODE_PHYSICAL, &[0xf0, 0x0f, 0x00, 0xd8]);
    let mut cpu = state();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    let PagedProbeExit::Fault(fault) = report.cpu.exit else {
        panic!("{:?}", report.cpu.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (6, None));

    for bytes in [[0x0f, 0x00, 0xd0], [0x0f, 0x01, 0xd8], [0x90, 0x00, 0xd8]] {
        let mut bus = make_bus();
        put_bytes(&mut bus, CODE_PHYSICAL, &bytes);
        let mut cpu = state();
        let before = cpu.clone();
        let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(
            matches!(report.cpu.exit, PagedProbeExit::Unsupported { .. }),
            "{bytes:x?}: {:?}",
            report.cpu.exit
        );
        assert_eq!(cpu, before);
        assert!(!log.contains(&(GDT_PHYSICAL + 8, 16)));
    }
}

#[test]
fn ltr_ax_profile_state_is_rejected_before_fetch() {
    let mut bus = make_bus();
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
        (|s| s.activity = None, PagedProbeRejection::ActivityMissing),
    ] {
        let mut cpu = state();
        mutate(&mut cpu);
        let before = cpu.clone();
        let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert_eq!(report.cpu.exit, PagedProbeExit::Rejected(expected));
        assert_eq!(cpu, before);
        assert!(
            log.is_empty(),
            "no table walk happens before profile checks"
        );
    }
}

#[test]
fn ltr_ax_gdt_page_faults_are_precise_and_do_not_load_tr() {
    // Read-only GDT page with CR0.WP=1: validation passes, the write walk faults.
    let mut bus = make_bus();
    put(&mut bus, GDT_PTE, GDT_PHYSICAL | 5);
    let mut cpu = state();
    let before = cpu.clone();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    let record = report.page_fault.expect("write protection fault");
    assert_eq!(record.fault().error_code, Some(3));
    assert_eq!(record.cr2_value(), GDT_LINEAR + 8);
    assert_eq!(record.saved_rip(), CODE_LINEAR);
    assert!(matches!(report.cpu.exit, PagedProbeExit::Fault(_)));
    assert_eq!(cpu, before);
    assert_eq!(get(&mut bus, GDT_PHYSICAL + 8, 16), tss_descriptor());

    // Not-present GDT page: read fault, error code 0, supervisor.
    let mut bus = make_bus();
    put(&mut bus, GDT_PTE, 0);
    let mut cpu = state();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    let record = report.page_fault.expect("not present");
    assert_eq!(record.fault().error_code, Some(0));
    assert_eq!(record.cr2_value(), GDT_LINEAR + 8);
    assert!(cpu.tr.is_none());
}

#[test]
fn ltr_ax_rejects_descriptor_aliasing_a_page_table_page() {
    let mut bus = make_bus();
    // Map the GDT linear page onto the page table page that translates it.
    put(&mut bus, GDT_PTE, GDT_PTE | 7);
    let before_page = get(&mut bus, GDT_PTE, 64);
    let mut cpu = state();
    let before = cpu.clone();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.rejection, Some(PagedLtrRejection::TableAlias));
    assert!(matches!(
        report.cpu.exit,
        PagedProbeExit::Unsupported { opcode: 0x0f }
    ));
    assert_eq!(cpu, before);
    assert_eq!(report.cpu.instructions_executed, 0);
    // Only the accessed bit of the aliased leaf may have changed; no D/busy write.
    let after_page = get(&mut bus, GDT_PTE, 64);
    assert_eq!(after_page[0] & 0x40, 0);
    assert_eq!(after_page[8..], before_page[8..]);
}

#[test]
fn ltr_ax_descriptor_crossing_a_page_is_unsupported_without_access() {
    let mut bus = make_bus();
    let mut cpu = state();
    cpu.gdtr = Some(Gdtr {
        base: GDT_LINEAR + 0xff0,
        limit: 0x1f,
    });
    let before = cpu.clone();
    let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert!(matches!(
        report.cpu.exit,
        PagedProbeExit::Unsupported { opcode: 0x0f }
    ));
    assert_eq!(cpu, before);
    assert!(report.memory_error.is_none() && report.page_fault.is_none());
    assert!(!log.iter().any(|(address, _)| *address == GDT_PTE));
}

#[test]
fn ltr_ax_alias_accessed_write_may_change_descriptor_bytes_but_nothing_else() {
    let mut bus = make_bus();
    // GDT linear page index 1 maps onto the page-table page itself, so the
    // descriptor at +8 overlaps the leaf PTE whose A bit the walk sets.
    put(&mut bus, GDT_PTE + 8, GDT_PTE | 7);
    let mut cpu = state();
    cpu.gdtr = Some(Gdtr {
        base: GDT_LINEAR + 0x1000,
        limit: 0x1f,
    });
    let before = cpu.clone();
    let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.rejection, Some(PagedLtrRejection::TableAlias));
    assert_eq!(cpu, before);
    // The A bit landed in the aliased descriptor bytes; no D or busy write followed.
    assert_eq!(get(&mut bus, GDT_PTE + 8, 1), [0x27]);
    assert!(log.contains(&(GDT_PTE + 8, 8)));
    assert!(!log.contains(&(GDT_PTE + 8, 16)));
}

#[test]
fn ltr_ax_selector_rpl_is_kept_in_tr_and_masked_for_the_descriptor_address() {
    let mut bus = make_bus();
    let mut cpu = state();
    cpu.gpr[0] = 0x0b; // index 1, TI=0, RPL=3
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
    assert_eq!(cpu.tr.unwrap().selector, 0x0b);
}

#[test]
fn ltr_ax_byte_granular_tss_keeps_the_raw_limit() {
    let mut bus = make_bus();
    let mut descriptor = tss_descriptor();
    descriptor[6] = 0x12; // G=0, AVL=1, limit[19:16]=2
    put_bytes(&mut bus, GDT_PHYSICAL + 8, &descriptor);
    let mut cpu = state();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
    let tr = cpu.tr.unwrap();
    assert_eq!((tr.limit, tr.flags), (0x24567, 1));
}

#[test]
fn ltr_ax_not_present_precedes_bad_upper_type_and_reserved_upper_bits_are_ignored() {
    let mut bus = make_bus();
    let mut descriptor = tss_descriptor();
    descriptor[5] = 0x09; // not present
    descriptor[13] = 1; // non-zero upper type field
    put_bytes(&mut bus, GDT_PHYSICAL + 8, &descriptor);
    let mut cpu = state();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    let PagedProbeExit::Fault(fault) = report.cpu.exit else {
        panic!("{:?}", report.cpu.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (11, Some(8)));

    let mut bus = make_bus();
    let mut descriptor = tss_descriptor();
    descriptor[12] = 0xff;
    descriptor[13] = 0xe0;
    put_bytes(&mut bus, GDT_PHYSICAL + 8, &descriptor);
    let mut cpu = state();
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
}

#[test]
fn ltr_ax_fetch_stays_in_the_code_page_and_rip_is_canonical() {
    // 3-byte instruction does not fit after offset 0xffd: no fetch, no walk.
    for offset in [0xffe, 0xfff] {
        let mut bus = make_bus();
        let mut cpu = state();
        cpu.rip = CODE_LINEAR + offset;
        let before = cpu.clone();
        let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
        assert!(matches!(
            report.cpu.exit,
            PagedProbeExit::Unsupported { .. }
        ));
        assert_eq!(cpu, before);
        assert!(log.is_empty(), "no walk before the page-boundary rejection");
    }
    // Exactly at 0xffd the instruction fits.
    let mut bus = make_bus();
    put_bytes(&mut bus, CODE_PHYSICAL + 0xffd, &[0x0f, 0x00, 0xd8]);
    let mut cpu = state();
    cpu.rip = CODE_LINEAR + 0xffd;
    let (report, _) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    assert_eq!(report.cpu.exit, PagedProbeExit::Retired);
    assert_eq!(cpu.rip, CODE_LINEAR + 0x1000);
    // A non-canonical RIP faults before any fetch.
    let mut bus = make_bus();
    let mut cpu = state();
    cpu.rip = 0x0000_8000_0000_0000;
    let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
    let PagedProbeExit::Fault(fault) = report.cpu.exit else {
        panic!("{:?}", report.cpu.exit)
    };
    assert_eq!((fault.vector, fault.error_code), (13, Some(0)));
    assert!(log.is_empty());
}

#[test]
fn ltr_ax_noncanonical_or_wrapping_descriptor_address_is_gp0() {
    for base in [0x0000_7fff_ffff_fff0u64, 0xffff_ffff_ffff_fff0] {
        let mut bus = make_bus();
        let mut cpu = state();
        cpu.gdtr = Some(Gdtr { base, limit: 0x1f });
        let before = cpu.clone();
        let (report, log) = run(&mut cpu, &mut bus, PrivilegeLevel::Ring0);
        let PagedProbeExit::Fault(fault) = report.cpu.exit else {
            panic!("{base:#x}: {:?}", report.cpu.exit)
        };
        assert_eq!((fault.vector, fault.error_code), (13, Some(0)), "{base:#x}");
        assert_eq!(cpu, before);
        assert!(!log.iter().any(|(address, _)| *address == GDT_PTE));
    }
}
