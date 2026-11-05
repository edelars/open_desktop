use od_isa::execute::RdtscControls;
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_machine::{
    AccessWidth, DiagnosticExit, DiagnosticTerminalExit, DiagnosticVcpuControlError,
    DiagnosticVcpuSession, DiagnosticVcpuState, GuestPhysicalAddress, GuestPhysicalRange,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, RamLimits, VirtualClockError,
    WorkLimitReason,
};

const RAM: u64 = 0x1000;
const RAM_LEN: u64 = 0x1000;

fn session(
    code: &[u8],
    privilege: PrivilegeLevel,
    defined: u64,
    tsc_hz: u64,
    tsd: bool,
) -> DiagnosticVcpuSession {
    session_at(code, RAM, privilege, defined, tsc_hz, tsd)
}

fn session_at(
    code: &[u8],
    address: u64,
    privilege: PrivilegeLevel,
    defined: u64,
    tsc_hz: u64,
    tsd: bool,
) -> DiagnosticVcpuSession {
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(RAM), RAM_LEN).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    for (offset, byte) in code.iter().enumerate() {
        bus.write_physical(
            GuestPhysicalAddress::new(address + offset as u64),
            AccessWidth::Byte,
            u64::from(*byte),
            |_| {},
        )
        .unwrap();
    }
    let state = LongModeState {
        rip: address,
        ..LongModeState::default()
    };
    DiagnosticVcpuSession::new_with_tsc(
        state,
        bus,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        },
        defined,
        tsc_hz,
        RdtscControls { cr4_tsd: tsd },
    )
    .unwrap()
}

fn run(cpu: &mut DiagnosticVcpuSession, budget: u64) -> od_machine::DiagnosticRunResult {
    cpu.run(budget, || false).unwrap()
}

fn accepts_all_terminal_causes(value: DiagnosticTerminalExit) {
    match value {
        DiagnosticTerminalExit::Halted => {}
        DiagnosticTerminalExit::Fault(_) => {}
        DiagnosticTerminalExit::Unsupported { .. } => {}
        DiagnosticTerminalExit::BackendError(_) => {}
        DiagnosticTerminalExit::StopAddress => {}
    }
}

#[test]
fn terminal_exit_is_available_to_external_callers() {
    let cause = DiagnosticTerminalExit::Halted;
    accepts_all_terminal_causes(cause.clone());
    assert_eq!(
        DiagnosticExit::Terminal(cause),
        DiagnosticExit::Terminal(DiagnosticTerminalExit::Halted)
    );
}

#[test]
fn machine_bus_executes_real_rdtsc_and_preserves_mask_and_rip() {
    let initial_defined = INTEGER_FLAGS & !(1 << 6);
    let mut cpu = session(
        &[0x0f, 0x31],
        PrivilegeLevel::Ring0,
        initial_defined,
        2_500_000_000,
        false,
    );
    cpu.advance_time_ns(1_000_000_001).unwrap();
    let result = run(&mut cpu, 1);
    assert_eq!(cpu.state().gpr[0], 2_500_000_002);
    assert_eq!(cpu.state().gpr[2], 0);
    assert_eq!(cpu.state().rip, RAM + 2);
    assert_eq!(cpu.rflags_defined(), initial_defined);
    assert_eq!(result.rflags_defined, initial_defined);
}

#[test]
fn split_budgets_and_host_calls_do_not_change_time_or_tsc() {
    let initial_defined = INTEGER_FLAGS & !(1 << 6);
    let mut one = session(
        &[0x0f, 0x31, 0x0f, 0x31],
        PrivilegeLevel::Ring0,
        initial_defined,
        1_000_000_000,
        false,
    );
    let mut split = session(
        &[0x0f, 0x31, 0x0f, 0x31],
        PrivilegeLevel::Ring0,
        initial_defined,
        1_000_000_000,
        false,
    );
    one.advance_time_ns(42).unwrap();
    split.advance_time_ns(42).unwrap();
    assert_eq!(run(&mut one, 2).instructions_executed, 2);
    run(&mut split, 1);
    run(&mut split, 1);
    assert_eq!(one.state(), split.state());
    assert_eq!(one.rflags_defined(), initial_defined);
    assert_eq!(split.rflags_defined(), initial_defined);
    assert_eq!(one.virtual_clock().unwrap().tsc(), 42);
    assert_eq!(split.virtual_clock().unwrap().tsc(), 42);
    assert_eq!(
        run(&mut one, 0).exit,
        DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
    );
    assert_eq!(one.virtual_clock().unwrap().now_ns(), 42);
}

#[test]
fn pause_stop_and_terminal_states_reject_advance_without_mutation() {
    let mut cpu = session(&[0x90], PrivilegeLevel::Ring0, INTEGER_FLAGS, 1, false);
    cpu.advance_time_ns(7).unwrap();
    cpu.pause().unwrap();
    assert!(matches!(
        cpu.advance_time_ns(5),
        Err(DiagnosticVcpuControlError::NotRunnable(_))
    ));
    assert_eq!(cpu.virtual_clock().unwrap().now_ns(), 7);
    cpu.resume().unwrap();
    cpu.stop();
    assert!(matches!(
        cpu.advance_time_ns(0),
        Err(DiagnosticVcpuControlError::NotRunnable(_))
    ));
    assert_eq!(cpu.virtual_clock().unwrap().now_ns(), 7);

    let mut terminal = session(
        &[0x0f, 0x0b],
        PrivilegeLevel::Ring0,
        INTEGER_FLAGS,
        1,
        false,
    );
    assert!(matches!(
        run(&mut terminal, 1).exit,
        DiagnosticExit::Terminal(DiagnosticTerminalExit::Fault(_))
    ));
    assert!(matches!(
        terminal.advance_time_ns(1),
        Err(DiagnosticVcpuControlError::NotRunnable(
            DiagnosticVcpuState::Terminal(DiagnosticTerminalExit::Fault(_))
        ))
    ));
    assert_eq!(terminal.virtual_clock().unwrap().now_ns(), 0);
}

#[test]
fn zero_advance_is_allowed_and_bad_defined_mask_is_rejected() {
    let mut cpu = session(&[], PrivilegeLevel::Ring0, INTEGER_FLAGS, 1, false);
    cpu.advance_time_ns(0).unwrap();
    assert_eq!(cpu.virtual_clock().unwrap().now_ns(), 0);
    let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(RAM), RAM_LEN).unwrap();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
    )
    .unwrap();
    let bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    let result = DiagnosticVcpuSession::new_with_tsc(
        LongModeState::default(),
        bus,
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
        u64::MAX,
        1,
        RdtscControls { cr4_tsd: false },
    );
    assert!(matches!(
        result,
        Err(DiagnosticVcpuControlError::ExecutionInput(_))
    ));
}

#[test]
fn rdtsc_respects_tsd_cpl_and_fetch_fault_boundaries() {
    let mut denied = session(
        &[0x0f, 0x31],
        PrivilegeLevel::Ring3,
        INTEGER_FLAGS,
        10,
        true,
    );
    let result = run(&mut denied, 1);
    assert_eq!(
        result.exit,
        DiagnosticExit::Terminal(DiagnosticTerminalExit::Fault(
            od_isa::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            }
        ))
    );
    assert_eq!(denied.state().rip, RAM);
    assert_eq!(denied.virtual_clock().unwrap().tsc(), 0);

    let mut allowed = session(
        &[0x0f, 0x31],
        PrivilegeLevel::Ring0,
        INTEGER_FLAGS,
        10,
        true,
    );
    assert_eq!(run(&mut allowed, 1).instructions_executed, 1);

    let mut truncated = session_at(
        &[0x0f],
        RAM + RAM_LEN - 1,
        PrivilegeLevel::Ring0,
        INTEGER_FLAGS,
        10,
        false,
    );
    let result = run(&mut truncated, 1);
    assert!(
        matches!(
            result.exit,
            DiagnosticExit::Terminal(DiagnosticTerminalExit::BackendError(_))
        ),
        "unexpected fetch failure classification: {result:?}"
    );
    assert_eq!(truncated.virtual_clock().unwrap().tsc(), 0);
}

#[test]
fn time_advance_overflow_is_atomic_and_non_runnable() {
    let mut cpu = session(&[0x90], PrivilegeLevel::Ring0, INTEGER_FLAGS, 1, false);
    cpu.advance_time_ns(u64::MAX).unwrap();
    assert!(matches!(
        cpu.advance_time_ns(1),
        Err(DiagnosticVcpuControlError::Clock(
            VirtualClockError::TimeOverflow
        ))
    ));
    assert_eq!(cpu.virtual_clock().unwrap().now_ns(), u64::MAX);
    cpu.pause().unwrap();
    assert!(matches!(
        cpu.advance_time_ns(0),
        Err(DiagnosticVcpuControlError::NotRunnable(
            DiagnosticVcpuState::Paused
        ))
    ));
}
