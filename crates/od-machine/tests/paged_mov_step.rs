use od_isa::execute::RunExit;
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, MemoryError, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, GuestPhysicalRange,
    IoSpace, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    PagedMovStepReport, RamLimits, TrackedAccessError,
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const DATA_LINEAR: u64 = 0x0080_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const DATA_PHYSICAL: u64 = 0x9000;
const RAM_LEN: u64 = 0x20_000;

fn context(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
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
    put(&mut bus, 0x3010, 0x4007); // PD index 2: code linear page
    put(&mut bus, 0x3020, 0x5007); // PD index 4: data linear page
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7);
    put(&mut bus, 0x5000, DATA_PHYSICAL | 7);
    bus
}

struct CountingMmio(Arc<AtomicUsize>);

impl BusDevice for CountingMmio {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        assert_eq!(access.space, IoSpace::Mmio);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    fn write(&mut self, access: DeviceAccess, _value: u64) -> Result<(), DeviceError> {
        assert_eq!(access.space, IoSpace::Mmio);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn make_bus_with_mmio(counter: Arc<AtomicUsize>) -> MachineBus {
    let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_LEN).unwrap();
    let mmio = GuestPhysicalRange::new(GuestPhysicalAddress::new(RAM_LEN), 0x1000).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
            MemoryRegionSpec::new(mmio, MemoryRegionKind::Mmio),
        ],
    )
    .unwrap();
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(RAM_LEN),
        vec![Box::new(CountingMmio(counter))],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(RAM_LEN),
            device_index: 0,
        }],
        vec![],
    )
    .unwrap();
    put(&mut bus, 0x1000, 0x2007);
    put(&mut bus, 0x2000, 0x3007);
    put(&mut bus, 0x3010, 0x4007);
    put(&mut bus, 0x3020, 0x5007);
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7);
    put(&mut bus, 0x5000, RAM_LEN | 7);
    bus
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

fn state(rip: u64, instruction: &[u8], bus: &mut MachineBus) -> LongModeState {
    for (offset, byte) in instruction.iter().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(CODE_PHYSICAL + offset as u64),
            AccessWidth::Byte,
            u64::from(*byte),
            |_| {},
        )
        .unwrap();
    }
    LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    }
}

fn step(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    privilege: PrivilegeLevel,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    od_machine::step_paged_mov(
        state,
        bus,
        cr3,
        EFER,
        context(privilege),
        INTEGER_FLAGS & !(1 << 7),
        |range| writes.push(range),
    )
    .unwrap()
}

fn qword(bus: &mut MachineBus, address: u64) -> u64 {
    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
        .unwrap()
}

#[test]
fn nonidentity_fetch_load_store_uses_literal_translations_and_commits_ad_before_operand() {
    let mut bus = make_bus();
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    put(&mut bus, DATA_PHYSICAL, 0x8877_6655_4433_2211);
    let mut writes = Vec::new();

    let load = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(load.cpu.exit, RunExit::StopAddress);
    assert_eq!(load.cpu.instructions_executed, 1);
    assert_eq!(cpu.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
        ]
    );
    assert_eq!(qword(&mut bus, 0x4000), CODE_PHYSICAL | 0x27);
    assert_eq!(qword(&mut bus, 0x5000), DATA_PHYSICAL | 0x27);

    cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 0x0123_4567_89ab_cdef;
    writes.clear();
    let store = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(store.cpu.exit, RunExit::StopAddress);
    assert_eq!(store.cpu.instructions_executed, 1);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
    assert_eq!(qword(&mut bus, DATA_PHYSICAL), 0x0123_4567_89ab_cdef);
    assert_eq!(writes, [range(0x5000), range(DATA_PHYSICAL)]);
    assert_eq!(qword(&mut bus, 0x5000), DATA_PHYSICAL | 0x67);
}

#[test]
fn page_faults_keep_linear_address_pfec_saved_rip_and_cpu_state() {
    let mut bus = make_bus();
    put(&mut bus, 0x5000, 0); // nonpresent data PTE
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[0] = 0xdead_beef;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let fault = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(
        fault.cpu.exit,
        RunExit::Fault(od_isa::memory::ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(DATA_LINEAR),
        })
    );
    assert_eq!(fault.cpu.instructions_executed, 0);
    assert_eq!(fault.page_fault.as_ref().unwrap().saved_rip(), CODE_LINEAR);
    assert_eq!(fault.page_fault.as_ref().unwrap().cr2_value(), DATA_LINEAR);
    assert_eq!(cpu, before);

    let mut bus = make_bus();
    put(&mut bus, 0x5000, DATA_PHYSICAL | 5); // supervisor write protected by CR0.WP
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 0xaabb_ccdd_eeff_0011;
    let before = cpu.clone();
    writes.clear();
    let fault = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(
        fault.cpu.exit,
        RunExit::Fault(od_isa::memory::ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(DATA_LINEAR),
        })
    );
    assert_eq!(fault.cpu.instructions_executed, 0);
    assert_eq!(fault.page_fault.as_ref().unwrap().saved_rip(), CODE_LINEAR);
    assert_eq!(cpu, before);
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
        ]
    );
    assert_eq!(qword(&mut bus, 0x5000), DATA_PHYSICAL | 0x25);
}

#[test]
fn nx_fetch_is_a_precise_fault_and_preserves_rip() {
    let mut bus = make_bus();
    put(&mut bus, 0x4000, CODE_PHYSICAL | 7 | (1 << 63));
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(
        report.cpu.exit,
        RunExit::Fault(od_isa::memory::ArchitecturalFault {
            vector: 14,
            error_code: Some(0x11),
            linear_address: Some(CODE_LINEAR),
        })
    );
    assert_eq!(report.page_fault.as_ref().unwrap().saved_rip(), CODE_LINEAR);
    assert_eq!(cpu, before);
}

#[test]
fn holes_cross_page_huge_page_and_unsupported_instruction_remain_distinct() {
    let mut bus = make_bus();
    put(&mut bus, 0x1000, 0x30000 | 7); // unmapped page-table page
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    let mut writes = Vec::new();
    let hole = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        hole.cpu.exit,
        RunExit::MemoryError(MemoryError::Backend(_))
    ));
    assert!(matches!(
        hole.memory_error,
        Some(TrackedAccessError::TableRead { .. })
    ));
    assert_eq!(hole.cpu.instructions_executed, 0);

    let mut bus = make_bus();
    put(&mut bus, 0x3010, 0x4000 | 0x87); // unsupported 2 MiB page
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    let huge = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        huge.memory_error,
        Some(TrackedAccessError::UnsupportedPageSize { .. })
    ));
    assert_eq!(huge.cpu.instructions_executed, 0);

    let mut bus = make_bus();
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xffc;
    let crossing = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        crossing.memory_error,
        Some(TrackedAccessError::CrossesPage { .. })
    ));
    assert_eq!(crossing.cpu.instructions_executed, 0);

    let mut bus = make_bus();
    let mut cpu = state(CODE_LINEAR, &[0x90, 0x90, 0x90], &mut bus);
    let unsupported = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(unsupported.cpu.exit, RunExit::Unsupported { .. }));
    assert!(unsupported.memory_error.is_none());
}

#[test]
fn different_cr3_roots_select_distinct_literal_mappings() {
    let mut bus = make_bus();
    put(&mut bus, 0xa000, 0xb007);
    put(&mut bus, 0xb000, 0xc007);
    put(&mut bus, 0xc010, 0xd007);
    put(&mut bus, 0xc020, 0xe007);
    put(&mut bus, 0xd000, CODE_PHYSICAL | 7);
    put(&mut bus, 0xe000, 0x12000 | 7);
    put(&mut bus, 0x12000, 0xfeed_face_cafe_beef);

    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    let mut writes = Vec::new();
    let report = step(
        &mut cpu,
        &mut bus,
        0xa000,
        PrivilegeLevel::Ring0,
        &mut writes,
    );
    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(cpu.gpr[0], 0xfeed_face_cafe_beef);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
}

#[test]
fn tracked_ram_path_rejects_mmio_without_device_side_effects() {
    let calls = Arc::new(AtomicUsize::new(0));
    let mut bus = make_bus_with_mmio(Arc::clone(&calls));
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    let mut writes = Vec::new();
    let report = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        report.cpu.exit,
        RunExit::MemoryError(MemoryError::Backend(_))
    ));
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

#[test]
fn explicit_origin_preserves_cpl_and_does_not_bypass_supervisor_permissions() {
    let mut bus = make_bus();
    put(&mut bus, 0x5000, DATA_PHYSICAL | 3); // supervisor-only data page
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring3,
            origin: AccessOrigin::DescriptorTable,
        },
        INTEGER_FLAGS,
        |range| writes.push(range),
    )
    .unwrap();
    assert_eq!(
        report.cpu.exit,
        RunExit::Fault(od_isa::memory::ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(DATA_LINEAR),
        })
    );
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu.rip, CODE_LINEAR);
}

#[test]
fn alias_operand_and_page_table_bytes_keep_duplicate_callback_order() {
    let mut bus = make_bus();
    put(&mut bus, 0x5000, 0x5000 | 7); // data linear page aliases its own PTE
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 0x0123_4567_89ab_cdef;
    let mut writes = Vec::new();
    let report = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5000),
            range(0x5000),
        ]
    );
    assert_eq!(qword(&mut bus, 0x5000), 0x0123_4567_89ab_cdef);
}

fn range(address: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
}
