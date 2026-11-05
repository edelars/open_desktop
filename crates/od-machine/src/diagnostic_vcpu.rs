//! Synchronous, bounded diagnostic execution over one owned CPU and machine.

use crate::{IdentityMemoryAdapter, MachineBus};
use crate::{VirtualClock, VirtualClockError};
use od_isa::execute::{
    RdtscControls, RunConfig, RunExit, run_with_rflags_defined,
    run_with_rflags_defined_and_rdtsc_source,
};
use od_isa::memory::{AccessContext, ArchitecturalFault, MemoryError};
use od_isa::{INTEGER_FLAGS, LongModeState};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticTerminalExit {
    Halted,
    Fault(ArchitecturalFault),
    Unsupported { opcode: u8 },
    BackendError(MemoryError),
    StopAddress,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticVcpuState {
    Runnable,
    Paused,
    Stopped,
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkLimitReason {
    InstructionBudget,
    RepIterationBudget,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticExit {
    WorkLimit(WorkLimitReason),
    Stopped,
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticRunResult {
    pub exit: DiagnosticExit,
    pub instructions_executed: u64,
    pub rflags_defined: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticVcpuControlError {
    NotRunnable(DiagnosticVcpuState),
    CannotPause(DiagnosticVcpuState),
    CannotResume(DiagnosticVcpuState),
    ExecutionInput(od_isa::execute::RunInputError),
    Clock(VirtualClockError),
}

/// One caller-driven vCPU session. It owns its CPU state and machine bus and
/// never schedules work or creates a worker thread.
pub struct DiagnosticVcpuSession {
    state: LongModeState,
    bus: MachineBus,
    access_context: AccessContext,
    rflags_defined: u64,
    status: DiagnosticVcpuState,
    clock: Option<VirtualClock>,
    rdtsc_controls: Option<RdtscControls>,
}

impl DiagnosticVcpuSession {
    pub fn new(
        state: LongModeState,
        bus: MachineBus,
        access_context: AccessContext,
        rflags_defined: u64,
    ) -> Result<Self, od_isa::execute::RunInputError> {
        if rflags_defined & !INTEGER_FLAGS != 0 {
            return Err(od_isa::execute::RunInputError::InvalidRflagsDefined {
                value: rflags_defined,
            });
        }
        Ok(Self {
            state,
            bus,
            access_context,
            rflags_defined,
            status: DiagnosticVcpuState::Runnable,
            clock: None,
            rdtsc_controls: None,
        })
    }

    /// Construct a session with explicit caller-driven virtual time and RDTSC.
    pub fn new_with_tsc(
        state: LongModeState,
        bus: MachineBus,
        access_context: AccessContext,
        rflags_defined: u64,
        tsc_hz: u64,
        rdtsc_controls: RdtscControls,
    ) -> Result<Self, DiagnosticVcpuControlError> {
        if rflags_defined & !INTEGER_FLAGS != 0 {
            return Err(DiagnosticVcpuControlError::ExecutionInput(
                od_isa::execute::RunInputError::InvalidRflagsDefined {
                    value: rflags_defined,
                },
            ));
        }
        let clock = VirtualClock::new(tsc_hz).map_err(DiagnosticVcpuControlError::Clock)?;
        Ok(Self {
            state,
            bus,
            access_context,
            rflags_defined,
            status: DiagnosticVcpuState::Runnable,
            clock: Some(clock),
            rdtsc_controls: Some(rdtsc_controls),
        })
    }

    pub const fn virtual_clock(&self) -> Option<&VirtualClock> {
        self.clock.as_ref()
    }

    /// Advance virtual time only between runnable calls.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        self.clock
            .as_mut()
            .ok_or(DiagnosticVcpuControlError::Clock(
                VirtualClockError::ClockNotConfigured,
            ))?
            .advance_time_ns(delta_ns)
            .map_err(DiagnosticVcpuControlError::Clock)
    }

    pub fn state(&self) -> &LongModeState {
        &self.state
    }

    pub const fn rflags_defined(&self) -> u64 {
        self.rflags_defined
    }

    pub fn status(&self) -> &DiagnosticVcpuState {
        &self.status
    }

    /// Pause takes effect between calls and leaves guest state and RAM intact.
    pub fn pause(&mut self) -> Result<(), DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::CannotPause(self.status.clone()));
        }
        self.status = DiagnosticVcpuState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Paused {
            return Err(DiagnosticVcpuControlError::CannotResume(
                self.status.clone(),
            ));
        }
        self.status = DiagnosticVcpuState::Runnable;
        Ok(())
    }

    /// Stop is idempotent. During a synchronous run, supply a stop source to
    /// `run`; it is polled by the existing CPU execution loop before fetch and
    /// at its established internal poll points.
    pub fn stop(&mut self) {
        if !matches!(self.status, DiagnosticVcpuState::Terminal(_)) {
            self.status = DiagnosticVcpuState::Stopped;
        }
    }

    pub fn run(
        &mut self,
        instruction_budget: u64,
        mut stop_requested: impl FnMut() -> bool,
    ) -> Result<DiagnosticRunResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }

        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        let config = RunConfig {
            instruction_budget,
            stop_rip: None,
            context: self.access_context,
        };
        let result = if let (Some(clock), Some(rdtsc_controls)) = (&self.clock, self.rdtsc_controls)
        {
            let mut source = || clock.tsc();
            run_with_rflags_defined_and_rdtsc_source(
                &mut self.state,
                &mut memory,
                config,
                self.rflags_defined,
                rdtsc_controls,
                &mut source,
                &mut stop_requested,
            )
            .map_err(DiagnosticVcpuControlError::ExecutionInput)?
        } else {
            run_with_rflags_defined(
                &mut self.state,
                &mut memory,
                config,
                self.rflags_defined,
                &mut stop_requested,
            )
            .map_err(DiagnosticVcpuControlError::ExecutionInput)?
        };
        self.rflags_defined = result.rflags_defined;

        let exit = match result.exit {
            RunExit::InstructionBudgetExhausted => {
                DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
            }
            RunExit::RepIterationBudgetExhausted => {
                DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget)
            }
            RunExit::StopRequested => {
                self.status = DiagnosticVcpuState::Stopped;
                DiagnosticExit::Stopped
            }
            RunExit::Halted => self.terminal(DiagnosticTerminalExit::Halted),
            RunExit::Fault(fault) => self.terminal(DiagnosticTerminalExit::Fault(fault)),
            RunExit::Unsupported { opcode } => {
                self.terminal(DiagnosticTerminalExit::Unsupported { opcode })
            }
            RunExit::MemoryError(error) => {
                self.terminal(DiagnosticTerminalExit::BackendError(error))
            }
            RunExit::StopAddress => self.terminal(DiagnosticTerminalExit::StopAddress),
        };

        Ok(DiagnosticRunResult {
            exit,
            instructions_executed: result.instructions_executed,
            rflags_defined: result.rflags_defined,
        })
    }

    fn terminal(&mut self, cause: DiagnosticTerminalExit) -> DiagnosticExit {
        self.status = DiagnosticVcpuState::Terminal(cause.clone());
        DiagnosticExit::Terminal(cause)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryRegionKind,
        MemoryRegionSpec, RamLimits,
    };
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};

    const RAM_START: u64 = 0x1000;
    const RAM_LEN: u64 = 0x1000;
    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn session(code: &[u8], rip: u64) -> DiagnosticVcpuSession {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(RAM_START), RAM_LEN).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
        let mut commits = 0;
        for (offset, byte) in code.iter().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(RAM_START + offset as u64),
                AccessWidth::Byte,
                u64::from(*byte),
                |_| commits += 1,
            )
            .unwrap();
        }
        let mut state = LongModeState::default();
        state.rip = rip;
        DiagnosticVcpuSession::new(state, bus, context(), 0).unwrap()
    }

    fn run(session: &mut DiagnosticVcpuSession, budget: u64) -> DiagnosticRunResult {
        session.run(budget, || false).unwrap()
    }

    #[test]
    fn fetches_executes_loads_and_stores_through_identity_adapter() {
        let code = [
            0x48, 0xb8, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0, // mov rax, imm64
            0x48, 0xbb, 0x00, 0x11, 0, 0, 0, 0, 0, 0, // mov rbx, DATA
            0x48, 0x89, 0x03, // mov [rbx], rax
            0x48, 0x8b, 0x0b, // mov rcx, [rbx]
        ];
        let mut cpu = session(&code, RAM_START);
        let result = run(&mut cpu, 4);
        assert_eq!(result.instructions_executed, 4);
        assert_eq!(cpu.state().gpr[1], 0x1234_5678);
        assert_eq!(cpu.state().rip, RAM_START + code.len() as u64);
        assert_eq!(
            result.exit,
            DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
        );
    }

    #[test]
    fn one_budget_matches_multiple_quanta_including_flag_definedness() {
        let code = [
            0x48, 0x83, 0xc0, 1, // add rax, 1
            0x48, 0x83, 0xf8, 5, // cmp rax, 5
            0x90,
        ];
        let mut single = session(&code, RAM_START);
        let mut quanta = session(&code, RAM_START);
        single.state.gpr[0] = 4;
        quanta.state.gpr[0] = 4;
        let single_exit = run(&mut single, 3);
        let quantum_exits = [
            run(&mut quanta, 1),
            run(&mut quanta, 1),
            run(&mut quanta, 1),
        ];
        assert!(quantum_exits.iter().all(|exit| matches!(
            exit.exit,
            DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
        )));
        assert_eq!(single.state, quanta.state);
        assert_eq!(single.rflags_defined(), quanta.rflags_defined());
        assert_eq!(single_exit.rflags_defined, quantum_exits[2].rflags_defined);
    }

    #[test]
    fn pause_resume_and_stop_between_calls_do_not_mutate_cpu_state() {
        let code = [
            0x48, 0xb8, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0, 0x48, 0xbb, 0x00, 0x11, 0, 0, 0, 0, 0,
            0, 0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b,
        ];
        let mut cpu = session(&code, RAM_START);
        assert_eq!(run(&mut cpu, 3).instructions_executed, 3);
        let before = cpu.state().clone();
        cpu.pause().unwrap();
        assert_eq!(cpu.state(), &before);
        assert!(matches!(
            cpu.run(1, || false),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
        cpu.resume().unwrap();
        assert_eq!(cpu.state(), &before);
        assert_eq!(run(&mut cpu, 1).instructions_executed, 1);
        assert_eq!(cpu.state().gpr[1], 0x1234_5678);
        let before_stop = cpu.state().clone();
        cpu.stop();
        cpu.stop();
        assert_eq!(cpu.status(), &DiagnosticVcpuState::Stopped);
        assert_eq!(cpu.state(), &before_stop);
        assert!(matches!(
            cpu.run(1, || false),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
    }

    #[test]
    fn stop_is_checked_before_fetch_and_between_instructions() {
        let mut before_fetch = session(&[0x90], RAM_START);
        let result = before_fetch.run(1, || true).unwrap();
        assert_eq!(result.exit, DiagnosticExit::Stopped);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(before_fetch.state().rip, RAM_START);

        let code = [
            0x48, 0xb8, 0x78, 0x56, 0x34, 0x12, 0, 0, 0, 0, 0x48, 0xbb, 0x00, 0x11, 0, 0, 0, 0, 0,
            0, 0x48, 0x89, 0x03,
        ];
        let mut during = session(&code, RAM_START);
        let mut polls = 0;
        let result = during
            .run(8, || {
                polls += 1;
                polls == 3
            })
            .unwrap();
        assert_eq!(result.exit, DiagnosticExit::Stopped);
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(during.state().rip, RAM_START + 20);
        assert_eq!(during.status(), &DiagnosticVcpuState::Stopped);
    }

    #[test]
    fn guest_fault_unsupported_and_backend_failure_are_distinct_terminal_exits() {
        let mut fault = session(&[0x0f, 0x0b], RAM_START); // UD2
        assert!(matches!(
            run(&mut fault, 4).exit,
            DiagnosticExit::Terminal(DiagnosticTerminalExit::Fault(ArchitecturalFault {
                vector: 6,
                ..
            }))
        ));

        let mut unsupported = session(&[0x0f, 0xff], RAM_START);
        assert!(matches!(
            run(&mut unsupported, 4).exit,
            DiagnosticExit::Terminal(DiagnosticTerminalExit::Unsupported { .. })
        ));

        let mut backend = session(&[], RAM_START + RAM_LEN);
        assert!(matches!(
            run(&mut backend, 4).exit,
            DiagnosticExit::Terminal(DiagnosticTerminalExit::BackendError(MemoryError::Backend(
                _
            )))
        ));
    }

    #[test]
    fn work_limit_returns_control_without_automatic_continuation() {
        let mut cpu = session(&[0x90, 0x90], RAM_START);
        let first = run(&mut cpu, 1);
        assert_eq!(first.instructions_executed, 1);
        assert_eq!(
            first.exit,
            DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
        );
        assert_eq!(cpu.state().rip, RAM_START + 1);
        assert_eq!(cpu.status(), &DiagnosticVcpuState::Runnable);
    }
}
