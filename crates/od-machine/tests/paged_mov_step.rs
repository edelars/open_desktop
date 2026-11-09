use od_isa::execute::RunExit;
use od_isa::memory::{
    AccessContext, AccessKind, AccessOrigin, InstructionMode, MemoryError, PrivilegeLevel,
};
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_machine::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, GuestPhysicalAddress, GuestPhysicalRange,
    IoSpace, MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding,
    PagedMovStepReport, RamLimits, TrackedAccessError,
};
use std::sync::{Arc, Mutex};

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const ROOT: u64 = 0x1000;
const CODE_LINEAR: u64 = 0x0040_0000;
const DATA_LINEAR: u64 = 0x0080_0000;
const CODE_PHYSICAL: u64 = 0x7000;
const NEXT_CODE_PHYSICAL: u64 = 0xb000;
const NEXT_DATA_PHYSICAL: u64 = 0xd000;
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

#[derive(Clone, Debug, Eq, PartialEq)]
enum JournalEvent {
    CommittedWrite(GuestPhysicalRange),
    MmioRead(DeviceAccess),
    MmioWrite(DeviceAccess, u64),
}

struct CountingMmio(Arc<Mutex<Vec<JournalEvent>>>, bool);

impl BusDevice for CountingMmio {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        self.0.lock().unwrap().push(JournalEvent::MmioRead(access));
        if self.1 {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        Ok(0xfeed_face_cafe_beef)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        self.0
            .lock()
            .unwrap()
            .push(JournalEvent::MmioWrite(access, value));
        if self.1 {
            Err(DeviceError::UnsupportedConfiguration)
        } else {
            Ok(())
        }
    }
}

fn make_bus_with_mmio(journal: Arc<Mutex<Vec<JournalEvent>>>) -> MachineBus {
    make_bus_with_mmio_behavior(journal, false)
}

fn make_bus_with_mmio_behavior(journal: Arc<Mutex<Vec<JournalEvent>>>, fail: bool) -> MachineBus {
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
        vec![Box::new(CountingMmio(journal, fail))],
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

fn put_byte(bus: &mut MachineBus, address: u64, value: u8) {
    bus.write_physical(
        GuestPhysicalAddress::new(address),
        AccessWidth::Byte,
        u64::from(value),
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

fn step_dword(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    privilege: PrivilegeLevel,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    od_machine::step_paged_mov_dword_read(
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

fn step_word(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    cr3: u64,
    privilege: PrivilegeLevel,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    od_machine::step_paged_mov_word_read(
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

fn step_word_fetch(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    step_word_fetch_with_efer(state, bus, privilege, EFER, writes)
}

fn step_word_fetch_with_efer(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
    efer: u64,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    od_machine::step_paged_mov_word_read_cross_page_fetch(
        state,
        bus,
        ROOT,
        efer,
        context(privilege),
        INTEGER_FLAGS & !(1 << 7),
        |range| writes.push(range),
    )
    .unwrap()
}

fn make_user_bus() -> MachineBus {
    let mut bus = make_bus();
    for entry in [0x1000, 0x2000, 0x3010, 0x3020, 0x4000, 0x5000] {
        let value = qword(&mut bus, entry);
        put(&mut bus, entry, value | 4);
    }
    bus
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
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xffc;
    cpu.gpr[1] = 1;
    writes.clear();
    let crossing = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        crossing.memory_error,
        Some(TrackedAccessError::CrossesPage { .. })
    ));
    assert_eq!(crossing.cpu.instructions_executed, 0);
    assert_eq!(
        writes,
        [range(0x1000), range(0x2000), range(0x3010), range(0x4000)]
    );

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
fn paged_mov_mmio_read_dispatches_once_and_commits_destination() {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[0] = 0x1111_2222_3333_4444;
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS,
        |range| {
            writes.push(range);
            journal
                .lock()
                .unwrap()
                .push(JournalEvent::CommittedWrite(range));
        },
    )
    .unwrap();
    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(cpu.gpr[0], 0xfeed_face_cafe_beef);
    assert_eq!(report.cpu.instructions_executed, 1);
    let access = DeviceAccess {
        space: IoSpace::Mmio,
        offset: 0,
        width: AccessWidth::Qword,
    };
    assert_eq!(
        *journal.lock().unwrap(),
        [
            JournalEvent::CommittedWrite(range(0x1000)),
            JournalEvent::CommittedWrite(range(0x2000)),
            JournalEvent::CommittedWrite(range(0x3010)),
            JournalEvent::CommittedWrite(range(0x4000)),
            JournalEvent::CommittedWrite(range(0x3020)),
            JournalEvent::CommittedWrite(range(0x5000)),
            JournalEvent::MmioRead(access),
        ]
    );
}

#[test]
fn mmio_device_failure_is_backend_error_with_no_retirement_after_dirty_commit() {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio_behavior(Arc::clone(&journal), true);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 0x1234;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS,
        |range| {
            writes.push(range);
            journal
                .lock()
                .unwrap()
                .push(JournalEvent::CommittedWrite(range));
        },
    )
    .unwrap();
    assert!(matches!(
        report.cpu.exit,
        RunExit::MemoryError(MemoryError::Backend(_))
    ));
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::MmioBackend { .. })
    ));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_eq!(
        *journal.lock().unwrap(),
        [
            JournalEvent::CommittedWrite(range(0x1000)),
            JournalEvent::CommittedWrite(range(0x2000)),
            JournalEvent::CommittedWrite(range(0x3010)),
            JournalEvent::CommittedWrite(range(0x4000)),
            JournalEvent::CommittedWrite(range(0x3020)),
            JournalEvent::CommittedWrite(range(0x5000)),
            JournalEvent::CommittedWrite(range(0x5000)),
            JournalEvent::MmioWrite(
                DeviceAccess {
                    space: IoSpace::Mmio,
                    offset: 0,
                    width: AccessWidth::Qword
                },
                0x1234
            ),
        ]
    );
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5000)
        ]
    );
    assert_eq!(qword(&mut bus, 0x5000), RAM_LEN | 0x67);
}

#[test]
fn paged_mov_mmio_write_dispatches_once_after_a_and_d_updates() {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 0x1234_5678_9abc_def0;
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS,
        |range| {
            writes.push(range);
            journal
                .lock()
                .unwrap()
                .push(JournalEvent::CommittedWrite(range));
        },
    )
    .unwrap();
    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    let access = DeviceAccess {
        space: IoSpace::Mmio,
        offset: 0,
        width: AccessWidth::Qword,
    };
    assert_eq!(
        *journal.lock().unwrap(),
        [
            JournalEvent::CommittedWrite(range(0x1000)),
            JournalEvent::CommittedWrite(range(0x2000)),
            JournalEvent::CommittedWrite(range(0x3010)),
            JournalEvent::CommittedWrite(range(0x4000)),
            JournalEvent::CommittedWrite(range(0x3020)),
            JournalEvent::CommittedWrite(range(0x5000)),
            JournalEvent::CommittedWrite(range(0x5000)),
            JournalEvent::MmioWrite(access, 0x1234_5678_9abc_def0),
        ]
    );
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5000)
        ]
    );
    assert_eq!(qword(&mut bus, 0x5000), RAM_LEN | 0x67);
}

#[test]
fn mmio_permission_fault_never_calls_device() {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    put(&mut bus, 0x5000, RAM_LEN | 5); // read-only for supervisor with WP=1
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[1] = 9;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS,
        |range| {
            writes.push(range);
            journal
                .lock()
                .unwrap()
                .push(JournalEvent::CommittedWrite(range));
        },
    )
    .unwrap();
    assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_eq!(
        *journal.lock().unwrap(),
        [
            JournalEvent::CommittedWrite(range(0x1000)),
            JournalEvent::CommittedWrite(range(0x2000)),
            JournalEvent::CommittedWrite(range(0x3010)),
            JournalEvent::CommittedWrite(range(0x4000)),
            JournalEvent::CommittedWrite(range(0x3020)),
            JournalEvent::CommittedWrite(range(0x5000)),
        ]
    );
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000)
        ]
    );
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

#[test]
fn cross_page_qword_load_covers_all_offsets_with_two_noncontiguous_ram_pages() {
    let expected = [0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88];
    for offset in 0xff9..=0xfff {
        let mut bus = make_bus();
        let second_physical = 0xb000;
        put(&mut bus, 0x5008, second_physical | 7);
        let first_len = (0x1000 - offset) as usize;
        for (index, byte) in expected[..first_len].iter().enumerate() {
            put_byte(&mut bus, DATA_PHYSICAL + offset + index as u64, *byte);
        }
        for (index, byte) in expected[first_len..].iter().enumerate() {
            put_byte(&mut bus, second_physical + index as u64, *byte);
        }

        let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
        cpu.gpr[3] = DATA_LINEAR + offset;
        cpu.gpr[0] = 0xdead_beef_cafe_babe;
        let mut writes = Vec::new();
        let report = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

        assert_eq!(report.cpu.exit, RunExit::StopAddress, "offset {offset:#x}");
        assert_eq!(report.cpu.instructions_executed, 1, "offset {offset:#x}");
        assert_eq!(cpu.rip, CODE_LINEAR + 3, "offset {offset:#x}");
        assert_eq!(
            cpu.gpr[0],
            u64::from_le_bytes(expected),
            "offset {offset:#x}"
        );
        assert_eq!(
            writes,
            [
                range(0x1000),
                range(0x2000),
                range(0x3010),
                range(0x4000),
                range(0x3020),
                range(0x5000),
                range(0x5008),
            ],
            "offset {offset:#x}"
        );
        assert_eq!(qword(&mut bus, 0x5000), DATA_PHYSICAL | 0x27);
        assert_eq!(qword(&mut bus, 0x5008), second_physical | 0x27);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x5008) & (1 << 6), 0);
    }
}

#[test]
fn s4_11c_dword_load_covers_same_page_and_all_three_crossing_offsets() {
    let expected = [0x11, 0x22, 0x33, 0x44];
    for offset in [0x100, 0xffd, 0xffe, 0xfff] {
        let mut bus = make_bus();
        let second_physical = 0xb000;
        put(&mut bus, 0x5008, second_physical | 7);
        let first_len = (0x1000 - offset) as usize;
        for (index, byte) in expected[..first_len.min(4)].iter().enumerate() {
            put_byte(&mut bus, DATA_PHYSICAL + offset + index as u64, *byte);
        }
        if offset >= 0xffd {
            for (index, byte) in expected[first_len..].iter().enumerate() {
                put_byte(&mut bus, second_physical + index as u64, *byte);
            }
        }

        let mut cpu = state(CODE_LINEAR, &[0x8b, 0x03], &mut bus);
        cpu.gpr[3] = DATA_LINEAR + offset;
        cpu.gpr[0] = 0xdead_beef_cafe_babe;
        let mut writes = Vec::new();
        let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

        assert_eq!(report.cpu.exit, RunExit::StopAddress, "offset {offset:#x}");
        assert_eq!(report.cpu.instructions_executed, 1, "offset {offset:#x}");
        assert_eq!(cpu.rip, CODE_LINEAR + 2, "offset {offset:#x}");
        assert_eq!(cpu.gpr[0], 0x4433_2211, "offset {offset:#x}");
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x5008) & (1 << 6), 0);
        if offset >= 0xffd {
            assert_eq!(qword(&mut bus, 0x5008), second_physical | 0x27);
        }
    }
}

#[test]
fn s4_11c_second_page_fault_is_precise_and_preserves_full_cpu_state() {
    let mut bus = make_bus();
    let mut cpu = state(CODE_LINEAR, &[0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    cpu.gpr[0] = 0xdead_beef_cafe_babe;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

    assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
    let fault = report.page_fault.as_ref().expect("second-page #PF");
    assert_eq!(fault.fault().vector, 14);
    assert_eq!(fault.fault().error_code, Some(0));
    assert_eq!(fault.fault().linear_address, Some(DATA_LINEAR + 0x1000));
    assert_eq!(fault.saved_rip(), CODE_LINEAR);
    assert_eq!(fault.access(), AccessKind::Read);
    assert_eq!(fault.context(), context(PrivilegeLevel::Ring0));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert_ne!(qword(&mut bus, 0x5000) & (1 << 5), 0);
    assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
}

#[test]
fn s4_11c_cross_page_alias_samples_after_both_walks_commit_accessed_bits() {
    let mut bus = make_bus();
    put(&mut bus, 0x5000, 0x5000 | 7);
    put(&mut bus, 0x5008, 0x5000 | 7);
    let mut cpu = state(CODE_LINEAR, &[0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    let mut writes = Vec::new();
    let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert_eq!(cpu.gpr[0], 0x0050_2700);
    assert_eq!(qword(&mut bus, 0x5000), 0x5027);
    assert_eq!(qword(&mut bus, 0x5008), 0x5027);
}

#[test]
fn s4_11c_cross_page_mmio_is_rejected_before_device_dispatch() {
    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    put(&mut bus, 0x5008, RAM_LEN | 7);
    let mut cpu = state(CODE_LINEAR, &[0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert!(journal.lock().unwrap().is_empty());
}

#[test]
fn s4_11c_profile_rejects_prefix_other_width_and_other_address_form() {
    for bytes in [&[0x48, 0x8b, 0x03][..], &[0x66, 0x8b, 0x03], &[0x8b, 0x01]] {
        let mut bus = make_bus();
        let mut cpu = state(CODE_LINEAR, bytes, &mut bus);
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
        assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
    }
}

#[test]
fn s4_11d_exact_dword_instruction_fetch_crosses_to_noncontiguous_ram() {
    let mut bus = make_bus();
    put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    put_byte(&mut bus, NEXT_CODE_PHYSICAL, 0x03);
    put(&mut bus, DATA_PHYSICAL, 0x8877_6655_4433_2211);
    let rip = CODE_LINEAR + 0xfff;
    let mut cpu = LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.gpr[3] = DATA_LINEAR;
    cpu.gpr[0] = 0xdead_beef_cafe_babe;
    cpu.rflags = 0x8d7;
    let old_gprs = cpu.gpr;
    let old_flags = cpu.rflags;
    let mut writes = Vec::new();

    let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert!(report.page_fault.is_none());
    assert!(report.memory_error.is_none());
    assert_eq!(cpu.rip, rip + 2);
    assert_eq!(cpu.gpr[0], 0x4433_2211);
    for index in 1..cpu.gpr.len() {
        assert_eq!(cpu.gpr[index], old_gprs[index]);
    }
    assert_eq!(cpu.gpr[3], old_gprs[3]);
    assert_eq!(cpu.rflags, old_flags);
    assert_eq!(
        writes,
        [0x1000, 0x2000, 0x3010, 0x4000, 0x4008, 0x3020, 0x5000].map(range)
    );
    for pte in [0x4000, 0x4008, 0x5000] {
        assert_ne!(qword(&mut bus, pte) & (1 << 5), 0, "PTE {pte:#x}");
        assert_eq!(qword(&mut bus, pte) & (1 << 6), 0, "PTE {pte:#x}");
    }
}

#[test]
fn s4_11d_second_fetch_fault_is_precise_and_prevents_operand_walk() {
    for (second_pte, expected_pfec, expected_writes, second_pte_accessed) in [
        (0, 0x10, [0x1000, 0x2000, 0x3010, 0x4000].as_slice(), false),
        (
            NEXT_CODE_PHYSICAL | 7 | (1 << 63),
            0x11,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x4008].as_slice(),
            true,
        ),
    ] {
        let mut bus = make_bus();
        put(&mut bus, 0x4008, second_pte);
        put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
        let rip = CODE_LINEAR + 0xfff;
        let mut cpu = LongModeState {
            rip,
            cr0: Some(CR0),
            cr4: Some(CR4),
            ..LongModeState::default()
        };
        cpu.gpr[3] = DATA_LINEAR;
        cpu.gpr[0] = 0xdead_beef_cafe_babe;
        let before = cpu.clone();
        let mut writes = Vec::new();

        let report = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

        assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
        let fault = report.page_fault.as_ref().expect("second-byte fetch #PF");
        assert_eq!(fault.fault().vector, 14);
        assert_eq!(fault.fault().error_code, Some(expected_pfec));
        assert_eq!(fault.fault().linear_address, Some(CODE_LINEAR + 0x1000));
        assert_eq!(fault.saved_rip(), rip);
        assert_eq!(fault.access(), AccessKind::Fetch);
        assert_eq!(fault.context(), context(PrivilegeLevel::Ring0));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(qword(&mut bus, 0x4000) & (1 << 5), 1 << 5);
        assert_eq!(qword(&mut bus, 0x4000) & (1 << 6), 0);
        assert_eq!(
            qword(&mut bus, 0x4008) & (1 << 5) != 0,
            second_pte_accessed,
            "second code PTE A bit for PFEC {expected_pfec:#x}"
        );
        assert_eq!(qword(&mut bus, 0x4008) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 5), 0);
        assert!(!writes.contains(&range(0x5000)));
        assert_eq!(
            writes,
            expected_writes
                .iter()
                .copied()
                .map(range)
                .collect::<Vec<_>>(),
            "page-table write journal for PFEC {expected_pfec:#x}"
        );
    }
}

#[test]
fn s4_11d_second_fetch_hole_and_mmio_keep_backend_classification() {
    let mut bus = make_bus();
    put(&mut bus, 0x4008, 0x30000 | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    let rip = CODE_LINEAR + 0xfff;
    let mut cpu = LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    let before = cpu.clone();
    let mut writes = Vec::new();
    let hole = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(hole.cpu.exit, RunExit::MemoryError(_)));
    assert!(matches!(
        hole.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert!(hole.page_fault.is_none());
    assert_eq!(hole.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);

    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    put(&mut bus, 0x4008, RAM_LEN | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    let mut cpu = LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    let before = cpu.clone();
    let mmio = step_dword(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(mmio.cpu.exit, RunExit::MemoryError(_)));
    assert!(matches!(
        mmio.memory_error,
        Some(TrackedAccessError::MmioBackend { .. })
    ));
    assert!(mmio.page_fault.is_none());
    assert_eq!(mmio.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert!(journal.lock().unwrap().is_empty());
}

#[test]
fn s4_11e_word_read_same_page_and_cross_page_preserve_upper_rax() {
    let mut bus = make_bus();
    let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
    let old_rax = 0x1234_5678_9abc_ffff;
    let old_flags = cpu.rflags;
    cpu.gpr[0] = old_rax;
    cpu.gpr[3] = DATA_LINEAR + 0x100;
    put_byte(&mut bus, DATA_PHYSICAL + 0x100, 0x34);
    put_byte(&mut bus, DATA_PHYSICAL + 0x101, 0x12);
    let mut writes = Vec::new();
    let same_page = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(same_page.cpu.exit, RunExit::StopAddress);
    assert_eq!(same_page.cpu.instructions_executed, 1);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
    assert_eq!(cpu.gpr[0], 0x1234_5678_9abc_1234);
    assert_eq!(cpu.gpr[0] & !0xffff, old_rax & !0xffff);
    assert_eq!(cpu.rflags, old_flags);

    let mut bus = make_user_bus();
    put(&mut bus, 0x5008, NEXT_DATA_PHYSICAL | 7 | 4);
    let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
    cpu.gpr[0] = old_rax;
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    put_byte(&mut bus, DATA_PHYSICAL + 0xfff, 0x34);
    put_byte(&mut bus, NEXT_DATA_PHYSICAL, 0x12);
    let user = step_word(
        &mut cpu,
        &mut bus,
        ROOT,
        PrivilegeLevel::Ring3,
        &mut Vec::new(),
    );
    assert_eq!(user.cpu.exit, RunExit::StopAddress);
    assert_eq!(user.cpu.instructions_executed, 1);
    assert_eq!(cpu.gpr[0], 0x1234_5678_9abc_1234);

    let mut bus = make_bus();
    put(&mut bus, 0x5008, NEXT_DATA_PHYSICAL | 7);
    let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
    cpu.gpr[0] = old_rax;
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    put_byte(&mut bus, DATA_PHYSICAL + 0xfff, 0xef);
    put_byte(&mut bus, NEXT_DATA_PHYSICAL, 0xbe);
    let mut writes = Vec::new();
    let crossing = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(crossing.cpu.exit, RunExit::StopAddress);
    assert_eq!(crossing.cpu.instructions_executed, 1);
    assert_eq!(cpu.rip, CODE_LINEAR + 3);
    assert_eq!(cpu.gpr[0], 0x1234_5678_9abc_beef);
    assert_eq!(cpu.gpr[0] & !0xffff, old_rax & !0xffff);
    assert_eq!(
        writes,
        [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000, 0x5008].map(range)
    );
    for pte in [0x4000, 0x5000, 0x5008] {
        assert_ne!(qword(&mut bus, pte) & (1 << 5), 0, "PTE {pte:#x}");
        assert_eq!(qword(&mut bus, pte) & (1 << 6), 0, "PTE {pte:#x}");
    }
}

#[test]
fn s4_11e_second_page_faults_are_precise_for_cpl0_and_cpl3() {
    for (privilege, second_pte, pfec, expected_writes) in [
        (
            PrivilegeLevel::Ring0,
            0,
            0,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000].as_slice(),
        ),
        (
            PrivilegeLevel::Ring3,
            NEXT_DATA_PHYSICAL | 3,
            5,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000, 0x5008].as_slice(),
        ),
    ] {
        let mut bus = if privilege == PrivilegeLevel::Ring3 {
            make_user_bus()
        } else {
            make_bus()
        };
        put(&mut bus, 0x5008, second_pte);
        let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
        let rip = cpu.rip;
        let old_rax = 0xdead_beef_cafe_babe;
        cpu.gpr[0] = old_rax;
        cpu.gpr[3] = DATA_LINEAR + 0xfff;
        let before = cpu.clone();
        let mut writes = Vec::new();

        let report = step_word(&mut cpu, &mut bus, ROOT, privilege, &mut writes);

        assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
        let fault = report.page_fault.as_ref().expect("second-page #PF");
        assert_eq!(fault.fault().vector, 14);
        assert_eq!(fault.fault().error_code, Some(pfec));
        assert_eq!(fault.fault().linear_address, Some(DATA_LINEAR + 0x1000));
        assert_eq!(fault.saved_rip(), rip);
        assert_eq!(fault.access(), AccessKind::Read);
        assert_eq!(fault.context(), context(privilege));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 5), 1 << 5);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x5008) & (1 << 6), 0);
        assert_eq!(
            writes,
            expected_writes
                .iter()
                .copied()
                .map(range)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn s4_11e_ram_hole_mmio_alias_and_profile_boundaries() {
    let mut bus = make_bus();
    put(&mut bus, 0x5008, 0x30000 | 7);
    let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let hole = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(hole.cpu.exit, RunExit::MemoryError(_)));
    assert!(matches!(
        hole.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(cpu, before);
    assert_eq!(hole.cpu.instructions_executed, 0);

    for start in [DATA_LINEAR + 0x100, DATA_LINEAR + 0xfff] {
        let journal = Arc::new(Mutex::new(Vec::new()));
        let mut bus = make_bus_with_mmio(Arc::clone(&journal));
        if start == DATA_LINEAR + 0xfff {
            put(&mut bus, 0x5000, DATA_PHYSICAL | 7);
            put(&mut bus, 0x5008, RAM_LEN | 7);
        } else {
            put(&mut bus, 0x5000, RAM_LEN | 7);
        }
        let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
        cpu.gpr[3] = start;
        let before = cpu.clone();
        let report = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
        assert!(matches!(report.cpu.exit, RunExit::MemoryError(_)));
        assert!(report.memory_error.is_some());
        assert_eq!(cpu, before);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert!(journal.lock().unwrap().is_empty());
    }

    let mut bus = make_bus();
    put(&mut bus, 0x5000, 0x5000 | 7);
    put(&mut bus, 0x5008, 0x5000 | 7);
    let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    let mut writes = Vec::new();
    let alias = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert_eq!(alias.cpu.exit, RunExit::StopAddress);
    assert_eq!(cpu.gpr[0] & 0xffff, 0x2700);
    assert_eq!(qword(&mut bus, 0x5000), 0x5027);
    assert_eq!(qword(&mut bus, 0x5008), 0x5027);

    for bytes in [
        &[0x8b, 0x03][..],
        &[0x66, 0x48, 0x8b, 0x03],
        &[0xf3, 0x66, 0x8b, 0x03],
        &[0x66, 0x8b, 0x43, 0x00],
        &[0x66, 0x8b, 0x0b],
    ] {
        let mut bus = make_bus();
        let mut cpu = state(CODE_LINEAR, bytes, &mut bus);
        let before = cpu.clone();
        let report = step_word(
            &mut cpu,
            &mut bus,
            ROOT,
            PrivilegeLevel::Ring0,
            &mut Vec::new(),
        );
        assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
    }

    for privilege in [PrivilegeLevel::Ring0, PrivilegeLevel::Ring3] {
        let mut bus = make_bus();
        let mut cpu = state(CODE_LINEAR, &[0x66, 0x8b, 0x03], &mut bus);
        cpu.rflags |= 1 << 18;
        let before = cpu.clone();
        let mut writes = Vec::new();
        let ac = step_word(&mut cpu, &mut bus, ROOT, privilege, &mut writes);
        assert!(matches!(ac.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(cpu, before);
        assert!(writes.is_empty());
    }

    let mut bus = make_bus();
    put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xffe, 0x66);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    put_byte(&mut bus, NEXT_CODE_PHYSICAL, 0x03);
    let mut cpu = LongModeState {
        cr3: None,
        rip: CODE_LINEAR + 0xffe,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.gpr[3] = DATA_LINEAR;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let crossing_fetch = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        crossing_fetch.cpu.exit,
        RunExit::Unsupported { .. }
    ));
    assert_eq!(crossing_fetch.cpu.instructions_executed, 0);
    assert_eq!(cpu, before);
    assert!(writes.is_empty());
    assert_eq!(qword(&mut bus, 0x4000) & (1 << 5), 0);
}

#[test]
fn second_cross_page_walk_failures_preserve_cpu_and_keep_prior_a_updates() {
    for (second_pte, privilege, expected_pfec, expected_events) in [
        (
            0,
            PrivilegeLevel::Ring0,
            0,
            vec![0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000],
        ),
        // A CPL3 explicit read of a present supervisor-only PTE sets P|U/S;
        // CR0.WP is enabled in this fixture, but does not affect read access.
        (
            0xa003,
            PrivilegeLevel::Ring3,
            5,
            vec![0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000, 0x5008],
        ),
    ] {
        let mut bus = make_bus();
        put(&mut bus, 0x5008, second_pte);
        let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
        cpu.gpr[3] = DATA_LINEAR + 0xff9;
        cpu.gpr[0] = 0xdead_beef_cafe_babe;
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = step(&mut cpu, &mut bus, ROOT, privilege, &mut writes);

        assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
        let page_fault = report
            .page_fault
            .as_ref()
            .expect("precise second-page fault");
        assert_eq!(page_fault.fault().vector, 14);
        assert_eq!(page_fault.fault().error_code, Some(expected_pfec));
        assert_eq!(
            page_fault.fault().linear_address,
            Some(DATA_LINEAR + 0x1000)
        );
        assert_eq!(page_fault.saved_rip(), CODE_LINEAR);
        assert_eq!(page_fault.access(), AccessKind::Read);
        assert_eq!(page_fault.context(), context(privilege));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(
            writes,
            expected_events.into_iter().map(range).collect::<Vec<_>>()
        );
        assert_ne!(qword(&mut bus, 0x5000) & (1 << 5), 0);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
        if second_pte != 0 {
            assert_ne!(qword(&mut bus, 0x5008) & (1 << 5), 0);
            assert_eq!(qword(&mut bus, 0x5008) & (1 << 6), 0);
        }
    }
}

#[test]
fn cross_page_load_rejects_ram_hole_and_mmio_without_device_dispatch() {
    let mut bus = make_bus();
    put(&mut bus, 0x5008, 0x30_000 | 7);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xff9;
    cpu.gpr[0] = 0xdead_beef_cafe_babe;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let hole = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        hole.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(hole.cpu.instructions_executed, 0);
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
            range(0x5008)
        ]
    );

    let journal = Arc::new(Mutex::new(Vec::new()));
    let mut bus = make_bus_with_mmio(Arc::clone(&journal));
    put(&mut bus, 0x5000, DATA_PHYSICAL | 7);
    put(&mut bus, 0x5008, RAM_LEN | 7);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xff9;
    cpu.gpr[0] = 0xdead_beef_cafe_babe;
    let before = cpu.clone();
    writes.clear();
    let mmio = od_machine::step_paged_mov(
        &mut cpu,
        &mut bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS,
        |range| {
            writes.push(range);
            journal
                .lock()
                .unwrap()
                .push(JournalEvent::CommittedWrite(range));
        },
    )
    .unwrap();
    assert!(matches!(
        mmio.memory_error,
        Some(TrackedAccessError::OperandPhysical { .. })
    ));
    assert_eq!(mmio.cpu.instructions_executed, 0);
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
            range(0x5008)
        ]
    );
    assert_eq!(
        *journal.lock().unwrap(),
        writes
            .iter()
            .copied()
            .map(JournalEvent::CommittedWrite)
            .collect::<Vec<_>>()
    );
}

#[test]
fn cross_page_alias_reads_operand_after_both_walks_commit_accessed_bits() {
    let mut bus = make_bus();
    put(&mut bus, 0x5000, 0x5000 | 7);
    put(&mut bus, 0x5008, 0x5000 | 7);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xff9;
    let mut writes = Vec::new();
    let report = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);

    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert_eq!(cpu.gpr[0], 0x2700_0000_0000_0000);
    assert_eq!(
        writes,
        [
            range(0x1000),
            range(0x2000),
            range(0x3010),
            range(0x4000),
            range(0x3020),
            range(0x5000),
            range(0x5008),
        ]
    );
    assert_eq!(qword(&mut bus, 0x5000), 0x5027);
    assert_eq!(qword(&mut bus, 0x5008), 0x5027);
}

#[test]
fn s4_11f_word_read_fetch_crosses_both_code_page_offsets() {
    for offset in [0xffe, 0xfff] {
        let mut bus = make_bus();
        put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
        let bytes = [0x66, 0x8b, 0x03];
        for (index, byte) in bytes.iter().copied().enumerate() {
            let instruction_offset = offset + index as u64;
            let physical = if instruction_offset <= 0xfff {
                CODE_PHYSICAL + instruction_offset
            } else {
                NEXT_CODE_PHYSICAL + instruction_offset - 0x1000
            };
            put_byte(&mut bus, physical, byte);
        }
        put_byte(&mut bus, DATA_PHYSICAL + 0x100, 0xef);
        put_byte(&mut bus, DATA_PHYSICAL + 0x101, 0xbe);
        let rip = CODE_LINEAR + offset;
        let mut cpu = LongModeState {
            rip,
            cr0: Some(CR0),
            cr4: Some(CR4),
            rflags: 0x8d7,
            ..LongModeState::default()
        };
        for (index, register) in cpu.gpr.iter_mut().enumerate() {
            *register = 0x1111_2222_3333_4400 + index as u64;
        }
        cpu.gpr[0] = 0x1234_5678_9abc_ffff;
        cpu.gpr[3] = DATA_LINEAR + 0x100;
        let before = cpu.clone();
        let mut writes = Vec::new();

        let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);

        assert_eq!(report.cpu.exit, RunExit::StopAddress, "offset {offset:#x}");
        assert_eq!(report.cpu.instructions_executed, 1);
        assert_eq!(report.cpu.rflags_defined, INTEGER_FLAGS & !(1 << 7));
        assert!(report.page_fault.is_none());
        assert!(report.memory_error.is_none());
        assert_eq!(cpu.rip, rip + 3);
        assert_eq!(cpu.gpr[0], (before.gpr[0] & !0xffff) | 0xbeef);
        for index in 1..cpu.gpr.len() {
            assert_eq!(cpu.gpr[index], before.gpr[index], "GPR {index}");
        }
        assert_eq!(cpu.rflags, before.rflags);
        assert_eq!(
            writes,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x4008, 0x3020, 0x5000].map(range)
        );
        for pte in [0x4000, 0x4008, 0x5000] {
            assert_ne!(qword(&mut bus, pte) & (1 << 5), 0, "PTE {pte:#x}");
            assert_eq!(qword(&mut bus, pte) & (1 << 6), 0, "PTE {pte:#x}");
        }
    }
}

#[test]
fn s4_11f_same_page_offset_ffd_needs_no_next_code_page() {
    let mut bus = make_bus();
    for (index, byte) in [0x66, 0x8b, 0x03].into_iter().enumerate() {
        put_byte(&mut bus, CODE_PHYSICAL + 0xffd + index as u64, byte);
    }
    put_byte(&mut bus, DATA_PHYSICAL + 0x100, 0x34);
    put_byte(&mut bus, DATA_PHYSICAL + 0x101, 0x12);
    let rip = CODE_LINEAR + 0xffd;
    let mut cpu = LongModeState {
        rip,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.gpr[0] = 0xfeed_face_cafe_babe;
    cpu.gpr[3] = DATA_LINEAR + 0x100;
    let mut writes = Vec::new();

    let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);

    assert_eq!(report.cpu.exit, RunExit::StopAddress);
    assert_eq!(report.cpu.instructions_executed, 1);
    assert_eq!(cpu.rip, rip + 3);
    assert_eq!(cpu.gpr[0], 0xfeed_face_cafe_1234);
    assert_eq!(qword(&mut bus, 0x4008), 0);
    assert_eq!(
        writes,
        [0x1000, 0x2000, 0x3010, 0x4000, 0x3020, 0x5000].map(range)
    );
}

#[test]
fn s4_11f_second_code_page_fault_precedes_operand_walk() {
    for (offset, second_pte, pfec, expected_writes, second_a) in [
        (
            0xffe,
            0,
            0x10,
            [0x1000, 0x2000, 0x3010, 0x4000].as_slice(),
            false,
        ),
        (
            0xffe,
            NEXT_CODE_PHYSICAL | 7 | (1 << 63),
            0x11,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x4008].as_slice(),
            true,
        ),
        (
            0xfff,
            0,
            0x10,
            [0x1000, 0x2000, 0x3010, 0x4000].as_slice(),
            false,
        ),
        (
            0xfff,
            NEXT_CODE_PHYSICAL | 7 | (1 << 63),
            0x11,
            [0x1000, 0x2000, 0x3010, 0x4000, 0x4008].as_slice(),
            true,
        ),
    ] {
        let mut bus = make_bus();
        put(&mut bus, 0x4008, second_pte);
        put(&mut bus, 0x5000, 0); // Operand page is also inaccessible.
        if offset == 0xffe {
            put_byte(&mut bus, CODE_PHYSICAL + 0xffe, 0x66);
            put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
        } else {
            put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x66);
        }
        let rip = CODE_LINEAR + offset;
        let mut cpu = LongModeState {
            rip,
            cr0: Some(CR0),
            cr4: Some(CR4),
            ..LongModeState::default()
        };
        cpu.gpr[0] = 0xdead_beef_cafe_babe;
        cpu.gpr[3] = DATA_LINEAR;
        let before = cpu.clone();
        let mut writes = Vec::new();

        let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);

        assert!(matches!(report.cpu.exit, RunExit::Fault(_)));
        let fault = report.page_fault.as_ref().expect("next-code-page #PF");
        assert_eq!(fault.fault().vector, 14);
        assert_eq!(fault.fault().error_code, Some(pfec));
        assert_eq!(fault.fault().linear_address, Some(CODE_LINEAR + 0x1000));
        assert_eq!(fault.saved_rip(), rip);
        assert_eq!(fault.access(), AccessKind::Fetch);
        assert_eq!(fault.context(), context(PrivilegeLevel::Ring0));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(qword(&mut bus, 0x4000) & (1 << 5), 1 << 5);
        assert_eq!(qword(&mut bus, 0x4000) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x4008) & (1 << 5) != 0, second_a);
        assert_eq!(qword(&mut bus, 0x4008) & (1 << 6), 0);
        assert_eq!(qword(&mut bus, 0x3020) & (1 << 5), 0);
        assert_eq!(qword(&mut bus, 0x5000) & (1 << 5), 0);
        assert_eq!(
            writes,
            expected_writes
                .iter()
                .copied()
                .map(range)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn s4_11f_code_page_hole_and_mmio_never_dispatch_devices() {
    for (physical, expected_mmio) in [(0x30_000, false), (RAM_LEN, true)] {
        let journal = Arc::new(Mutex::new(Vec::new()));
        let mut bus = make_bus_with_mmio(Arc::clone(&journal));
        put(&mut bus, 0x4008, physical | 7);
        put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x66);
        let rip = CODE_LINEAR + 0xfff;
        let mut cpu = LongModeState {
            rip,
            cr0: Some(CR0),
            cr4: Some(CR4),
            ..LongModeState::default()
        };
        let before = cpu.clone();
        let mut writes = Vec::new();

        let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);

        assert!(matches!(report.cpu.exit, RunExit::MemoryError(_)));
        assert!(report.page_fault.is_none());
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert_eq!(
            matches!(
                report.memory_error,
                Some(TrackedAccessError::MmioBackend { .. })
            ),
            expected_mmio
        );
        if !expected_mmio {
            assert!(matches!(
                report.memory_error,
                Some(TrackedAccessError::OperandPhysical { .. })
            ));
        }
        assert!(journal.lock().unwrap().is_empty());
        assert!(!writes.contains(&range(0x3020)));
        assert_eq!(qword(&mut bus, 0x3020) & (1 << 5), 0);
    }
}

#[test]
fn s4_11f_negative_admission_and_s4_11e_fetch_boundary() {
    for (offset, privilege, ac) in [
        (0xffc, PrivilegeLevel::Ring0, false),
        (0xffe, PrivilegeLevel::Ring1, false),
        (0xffe, PrivilegeLevel::Ring2, false),
        (0xffe, PrivilegeLevel::Ring3, false),
        (0xffe, PrivilegeLevel::Ring0, true),
    ] {
        let mut bus = make_bus();
        put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
        let mut cpu = LongModeState {
            cr3: None,
            rip: CODE_LINEAR + offset,
            cr0: Some(CR0),
            cr4: Some(CR4),
            rflags: u64::from(ac) << 18,
            ..LongModeState::default()
        };
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = step_word_fetch(&mut cpu, &mut bus, privilege, &mut writes);
        assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert!(writes.is_empty());
    }

    for (offset, instruction) in [(0xffd, [0x66, 0x8b, 0x0b]), (0xffd, [0x66, 0x8b, 0x1b])] {
        let mut bus = make_bus();
        for (index, byte) in instruction.into_iter().enumerate() {
            put_byte(&mut bus, CODE_PHYSICAL + offset + index as u64, byte);
        }
        let mut cpu = LongModeState {
            cr3: None,
            rip: CODE_LINEAR + offset,
            cr0: Some(CR0),
            cr4: Some(CR4),
            ..LongModeState::default()
        };
        cpu.gpr[3] = DATA_LINEAR;
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);
        assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert!(!writes.contains(&range(0x3020)));
        assert!(!writes.contains(&range(0x5000)));
    }

    for offset in [0xffe, 0xfff] {
        let mut bus = make_bus();
        put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
        let bytes = [0x66, 0x8b, 0x03];
        for (index, byte) in bytes.iter().copied().enumerate() {
            let instruction_offset = offset + index as u64;
            let physical = if instruction_offset <= 0xfff {
                CODE_PHYSICAL + instruction_offset
            } else {
                NEXT_CODE_PHYSICAL + instruction_offset - 0x1000
            };
            put_byte(&mut bus, physical, byte);
        }
        let mut cpu = LongModeState {
            cr3: None,
            rip: CODE_LINEAR + offset,
            cr0: Some(CR0),
            cr4: Some(CR4),
            ..LongModeState::default()
        };
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = step_word(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
        assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        assert!(writes.is_empty());
    }
}

#[test]
fn s4_11f_rejects_nxe_disabled_before_any_code_fetch() {
    let mut bus = make_bus();
    put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xffe, 0x66);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    put_byte(&mut bus, NEXT_CODE_PHYSICAL, 0x03);
    let mut cpu = LongModeState {
        cr3: None,
        rip: CODE_LINEAR + 0xffe,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.gpr[3] = DATA_LINEAR;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = step_word_fetch_with_efer(
        &mut cpu,
        &mut bus,
        PrivilegeLevel::Ring0,
        EFER & !(1 << 11),
        &mut writes,
    );

    assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert!(report.page_fault.is_none());
    assert!(report.memory_error.is_none());
    assert_eq!(cpu, before);
    assert!(writes.is_empty());
    for entry in [0x1000, 0x2000, 0x3010, 0x4000, 0x4008, 0x3020, 0x5000] {
        assert_eq!(qword(&mut bus, entry) & (1 << 5), 0, "entry {entry:#x}");
    }
}

#[test]
fn s4_11f_cross_page_operand_is_rejected_after_fetch_before_data_walk() {
    let offset = 0xffe;
    let mut bus = make_bus();
    put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
    put(&mut bus, 0x5008, NEXT_DATA_PHYSICAL | 7);
    put_byte(&mut bus, CODE_PHYSICAL + 0xffe, 0x66);
    put_byte(&mut bus, CODE_PHYSICAL + 0xfff, 0x8b);
    put_byte(&mut bus, NEXT_CODE_PHYSICAL, 0x03);
    put_byte(&mut bus, DATA_PHYSICAL + 0xfff, 0xef);
    put_byte(&mut bus, NEXT_DATA_PHYSICAL, 0xbe);

    let mut cpu = LongModeState {
        cr3: None,
        rip: CODE_LINEAR + offset,
        cr0: Some(CR0),
        cr4: Some(CR4),
        ..LongModeState::default()
    };
    cpu.gpr[3] = DATA_LINEAR + 0xfff;
    let before = cpu.clone();
    let mut writes = Vec::new();
    let report = step_word_fetch(&mut cpu, &mut bus, PrivilegeLevel::Ring0, &mut writes);

    assert!(matches!(report.cpu.exit, RunExit::Unsupported { .. }));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert!(report.page_fault.is_none());
    assert!(report.memory_error.is_none());
    assert_eq!(cpu, before);
    assert_eq!(writes, [0x1000, 0x2000, 0x3010, 0x4000, 0x4008].map(range));
    assert!(!writes.contains(&range(0x3020)));
    assert!(!writes.contains(&range(0x5000)));
    assert!(!writes.contains(&range(0x5008)));
    assert_eq!(qword(&mut bus, 0x5000) & (1 << 6), 0);
    assert_eq!(qword(&mut bus, 0x5008) & (1 << 6), 0);
}

fn range(address: u64) -> GuestPhysicalRange {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(address), 8).unwrap()
}

fn step_2m(
    state: &mut LongModeState,
    bus: &mut MachineBus,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    od_machine::step_paged_mov_2m_ram(
        state,
        bus,
        ROOT,
        EFER,
        context(PrivilegeLevel::Ring0),
        INTEGER_FLAGS & !(1 << 7),
        |range| writes.push(range),
    )
    .unwrap()
}

#[test]
fn two_mib_ram_profile_loads_stores_sets_pde_ad_and_faults_precisely() {
    let mut bus = make_bus();
    // PD index 4 (data) becomes a 2 MiB leaf at physical 0: P|RW|PS (+U).
    put(&mut bus, 0x3020, 0x87);
    put(&mut bus, 0xf000, 0x8877_6655_4433_2211);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xf000;
    cpu.gpr[1] = 0x1234_5678_9abc_def0;
    let mut writes = Vec::new();

    let load = step_2m(&mut cpu, &mut bus, &mut writes);
    assert_eq!(load.cpu.instructions_executed, 1, "{:?}", load.cpu.exit);
    assert_eq!(cpu.gpr[0], 0x8877_6655_4433_2211);
    assert_eq!(qword(&mut bus, 0x3020) & 0x60, 0x20, "load sets A only");

    let store = step_2m(&mut cpu, &mut bus, &mut writes);
    assert_eq!(store.cpu.instructions_executed, 1, "{:?}", store.cpu.exit);
    assert_eq!(qword(&mut bus, 0xf000), 0x1234_5678_9abc_def0);
    assert_eq!(qword(&mut bus, 0x3020) & 0x60, 0x60, "store sets A and D");

    // Non-present PDE: precise #PF with CR2 = operand address, state preserved.
    put(&mut bus, 0x3020, 0);
    cpu.rip = CODE_LINEAR;
    cpu.gpr[0] = 0x123;
    let failed = step_2m(&mut cpu, &mut bus, &mut writes);
    let fault = failed.page_fault.expect("page fault");
    assert_eq!(fault.saved_rip(), CODE_LINEAR);
    assert_eq!(fault.cr2_value(), DATA_LINEAR + 0xf000);
    assert_eq!(fault.fault().error_code, Some(0));
    assert_eq!(cpu.rip, CODE_LINEAR);
    assert_eq!(cpu.gpr[0], 0x123);
}

#[test]
fn two_mib_ram_profile_keeps_legacy_profile_rejecting_large_pages() {
    let mut bus = make_bus();
    put(&mut bus, 0x3020, 0x87);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xf000;
    let mut writes = Vec::new();
    let legacy = step(&mut cpu, &mut bus, ROOT, PrivilegeLevel::Ring0, &mut writes);
    assert!(matches!(
        legacy.memory_error,
        Some(TrackedAccessError::UnsupportedPageSize { .. })
    ));
    assert_eq!(legacy.cpu.instructions_executed, 0);
}

#[test]
fn two_mib_ram_profile_rejects_qword_crossing_4k_boundary() {
    let mut bus = make_bus();
    put(&mut bus, 0x3020, 0x87);
    let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
    cpu.gpr[3] = DATA_LINEAR + 0xffc;
    let mut writes = Vec::new();
    let report = step_2m(&mut cpu, &mut bus, &mut writes);
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::CrossesPage { .. })
    ));
    assert_eq!(report.cpu.instructions_executed, 0);
    assert_eq!(cpu.rip, CODE_LINEAR);
}

#[test]
fn two_mib_ram_profile_rejects_decoder_lookahead_before_next_code_page_walk() {
    let mut bus = make_bus();
    // `48 8b 83 <disp32>`: the displacement lies on the next code page.
    let mut cpu = state(CODE_LINEAR + 0xffd, &[], &mut bus);
    for (offset, byte) in [0x48, 0x8b, 0x83].into_iter().enumerate() {
        put_byte(&mut bus, CODE_PHYSICAL + 0xffd + offset as u64, byte);
    }
    put(&mut bus, 0x4008, NEXT_CODE_PHYSICAL | 7);
    let mut writes = Vec::new();
    let report = step_2m(&mut cpu, &mut bus, &mut writes);
    assert!(matches!(report.cpu.exit, RunExit::MemoryError(_)));
    assert!(matches!(
        report.memory_error,
        Some(TrackedAccessError::CrossesPage { width: 4, .. })
    ));
    assert_eq!(cpu.rip, CODE_LINEAR + 0xffd);
    // The next-page PTE was never walked, so its accessed bit stays clear.
    assert_eq!(qword(&mut bus, 0x4008) & 0x20, 0);
}

/// S4-10c: PDPTE.PS=1 is a guest reserved-bit #PF (P|RSVD) in both profiles,
/// replacing the S4-10a host error `UnsupportedPageSize` for PDPT PS.
fn run_pdpte_ps(
    two_mib_profile: bool,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    privilege: PrivilegeLevel,
    writes: &mut Vec<GuestPhysicalRange>,
) -> PagedMovStepReport {
    if two_mib_profile {
        od_machine::step_paged_mov_2m_ram(
            state,
            bus,
            ROOT,
            EFER,
            context(privilege),
            INTEGER_FLAGS & !(1 << 7),
            |range| writes.push(range),
        )
        .unwrap()
    } else {
        step(state, bus, ROOT, privilege, writes)
    }
}

#[test]
fn s4_10c_pdpte_ps_on_fetch_is_guest_rsvd_pf_with_a_only_on_pml4e() {
    for two_mib_profile in [false, true] {
        let mut bus = make_bus();
        put(&mut bus, 0x2000, 0x3000 | 0x87); // PDPT[0] with PS=1 (code linear page)
        let mut cpu = state(CODE_LINEAR, &[0x48, 0x8b, 0x03], &mut bus);
        let before = cpu.clone();
        let mut writes = Vec::new();
        let report = run_pdpte_ps(
            two_mib_profile,
            &mut cpu,
            &mut bus,
            PrivilegeLevel::Ring0,
            &mut writes,
        );
        // Literal PFEC: P|RSVD|I/D (NXE=1) = 0x19.
        assert_eq!(
            report.cpu.exit,
            RunExit::Fault(od_isa::memory::ArchitecturalFault {
                vector: 14,
                error_code: Some(0x19),
                linear_address: Some(CODE_LINEAR),
            }),
            "2m={two_mib_profile}"
        );
        assert!(
            report.memory_error.is_none(),
            "no host error for PDPTE.PS, 2m={two_mib_profile}: {:?}",
            report.memory_error
        );
        let fault = report.page_fault.as_ref().expect("guest #PF record");
        assert_eq!(fault.saved_rip(), CODE_LINEAR);
        assert_eq!(fault.cr2_value(), CODE_LINEAR);
        assert_eq!(report.cpu.instructions_executed, 0);
        assert_eq!(cpu, before);
        // A on the successful PML4E only; none on the PDPTE; no D anywhere.
        assert_eq!(writes, [range(0x1000)], "2m={two_mib_profile}");
        assert_eq!(qword(&mut bus, 0x1000), 0x2027);
        assert_eq!(qword(&mut bus, 0x2000), 0x3000 | 0x87);
        assert_eq!(qword(&mut bus, 0x3010), 0x4007);
    }
}

#[test]
fn s4_10c_pdpte_ps_on_data_access_is_guest_rsvd_pf_without_retirement_or_store() {
    // PDPT[1] covers linear 0x4000_0000..0x8000_0000; code stays under PDPT[0].
    const GIB_LINEAR: u64 = 0x4000_0000 + 0xabc;
    for two_mib_profile in [false, true] {
        for (privilege, bytes, expected_pfec) in [
            (PrivilegeLevel::Ring0, [0x48u8, 0x8b, 0x03], 0x09u32),
            (PrivilegeLevel::Ring0, [0x48, 0x89, 0x0b], 0x0b),
            (PrivilegeLevel::Ring3, [0x48, 0x8b, 0x03], 0x0d),
            (PrivilegeLevel::Ring3, [0x48, 0x89, 0x0b], 0x0f),
        ] {
            let mut bus = make_bus();
            put(&mut bus, 0x2008, 0x5000 | 0x87); // PDPT[1] with PS=1
            put_byte(&mut bus, 0x5abc, 0x5a);
            let mut cpu = state(CODE_LINEAR, &bytes, &mut bus);
            cpu.gpr[3] = GIB_LINEAR;
            cpu.gpr[0] = 0xdead_beef;
            cpu.gpr[1] = 0x1122_3344_5566_7788;
            let before = cpu.clone();
            let mut writes = Vec::new();
            let report = run_pdpte_ps(two_mib_profile, &mut cpu, &mut bus, privilege, &mut writes);
            let label = format!("2m={two_mib_profile} {privilege:?} {bytes:02x?}");
            assert_eq!(
                report.cpu.exit,
                RunExit::Fault(od_isa::memory::ArchitecturalFault {
                    vector: 14,
                    error_code: Some(expected_pfec),
                    linear_address: Some(GIB_LINEAR),
                }),
                "{label}"
            );
            assert!(report.memory_error.is_none(), "{label}");
            let fault = report.page_fault.as_ref().expect("guest #PF record");
            assert_eq!(fault.saved_rip(), CODE_LINEAR, "{label}");
            assert_eq!(fault.cr2_value(), GIB_LINEAR, "{label}");
            assert_eq!(report.cpu.instructions_executed, 0, "{label}");
            assert_eq!(cpu, before, "{label}");
            // Only the code walk committed A; the faulting PDPTE got no A/D.
            assert_eq!(
                writes,
                [range(0x1000), range(0x2000), range(0x3010), range(0x4000)],
                "{label}"
            );
            assert_eq!(qword(&mut bus, 0x2008), 0x5000 | 0x87, "{label}");
            // Operand memory untouched: the store never happened.
            assert_eq!(qword(&mut bus, 0x5ab8) >> 32 & 0xff, 0x5a, "{label}");
        }
    }
}
