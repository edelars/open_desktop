//! Synchronous, bounded diagnostic execution over one owned CPU and machine.

use crate::virtual_clock::VirtualClockAdvancer;
use crate::{IdentityMemoryAdapter, LocalApicHandle, MachineBus};
use crate::{VirtualClock, VirtualClockError, VirtualClockHandle};
use od_isa::execute::{
    RdtscControls, RunConfig, RunExit, StepOutcome, run_with_rflags_defined,
    run_with_rflags_defined_and_rdtsc_source, run_with_sti_profile, step_with_halt_profile,
};
use od_isa::interrupt::{
    InterruptDeliveryPhase, InterruptDeliveryResult, InterruptOutsideProfile,
    deliver_maskable_interrupt,
};
use od_isa::iretq::{IretqProfile, IretqReadPhase, IretqRunExit, run_with_iretq_profile};
use od_isa::memory::{
    AccessContext, ArchitecturalFault, InstructionMode, MemoryError, PrivilegeLevel,
};
use od_isa::{INTEGER_FLAGS, LongModeState};
use od_jit::block::{CodeView, S0_09NopAdmission};
use od_jit::dispatch::{
    ControlledBlockExecutor, DispatchControl, DispatchError, FallbackDispatchResult,
};

#[derive(Debug)]
pub enum DiagnosticJitError<E> {
    ProfileRejected,
    CodeRangeRejected,
    CodeMismatch,
    AdmissionRejected,
    Dispatch(DispatchError<E>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticJitResult {
    pub dispatch: FallbackDispatchResult,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticTerminalExit {
    Halted,
    Fault(ArchitecturalFault),
    Unsupported {
        opcode: u8,
    },
    BackendError(MemoryError),
    StopAddress,
    InterruptDeliveryAborted {
        phase: InterruptDeliveryPhase,
        cause: MemoryError,
    },
    InterruptSelectionChanged,
    IretqUnsupported,
    IretqReadAborted {
        phase: IretqReadPhase,
        cause: MemoryError,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticIretqResult {
    Retired,
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticInterruptResult {
    NoDelivery,
    OutsideProfile(InterruptOutsideProfile),
    Delivered {
        vector: u8,
        handler: DiagnosticRunResult,
    },
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticApicQuantumResult {
    ProfileRejected,
    Stopped,
    Ordinary(DiagnosticRunResult),
    OutsideProfile(InterruptOutsideProfile),
    Delivered {
        vector: u8,
        handler: DiagnosticRunResult,
    },
    Terminal(DiagnosticTerminalExit),
}

/// Result of delivering one IRQ at a boundary without running the handler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DiagnosticIrqEntry {
    Entered { vector: u8 },
    OutsideProfile(InterruptOutsideProfile),
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticStiApicQuantumResult {
    /// Ordinary execution without interrupt delivery in the STI APIC profile.
    Ordinary(DiagnosticRunResult),
    /// The session does not meet the STI APIC profile's admission conditions.
    ProfileRejected,
    /// A terminal result from the ordinary STI-profile quantum.
    TerminalExecution(DiagnosticRunResult),
    OutsideProfile(InterruptOutsideProfile),
    Delivered {
        vector: u8,
        handler: DiagnosticRunResult,
    },
    Terminal(DiagnosticTerminalExit),
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
    InterruptBoundaryUnsupported,
}

/// One caller-driven vCPU session. It owns its CPU state and machine bus and
/// never schedules work or creates a worker thread.
pub struct DiagnosticVcpuSession {
    state: LongModeState,
    bus: MachineBus,
    access_context: AccessContext,
    rflags_defined: u64,
    status: DiagnosticVcpuState,
    clock: Option<VirtualClockAdvancer>,
    rdtsc_controls: Option<RdtscControls>,
    completed_boundary: bool,
}

/// A failed HLT attempt returns the original session to its caller.
pub struct DiagnosticHaltEnterFailure {
    pub session: DiagnosticVcpuSession,
    pub reason: DiagnosticHaltEnterReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticHaltEnterReason {
    ProfileRejected,
    Step(StepOutcome),
    MemoryError(MemoryError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticHaltWakeResult {
    Waiting,
    Entered { vector: u8 },
    OutsideProfile(InterruptOutsideProfile),
    Stopped,
    Terminal(DiagnosticTerminalExit),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticHaltWaitError {
    NotWaiting,
    Clock(VirtualClockError),
}

enum HaltWaitState {
    Waiting,
    Entered(u8),
    Stopped,
    Terminal(DiagnosticTerminalExit),
}

/// Owns the halted CPU until an explicit APIC poll commits interrupt entry.
pub struct DiagnosticHaltWait {
    session: DiagnosticVcpuSession,
    state: HaltWaitState,
}

impl DiagnosticHaltWait {
    pub fn state(&self) -> &LongModeState {
        &self.session.state
    }

    /// Advance only this halted session's clock while it remains in Waiting.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), DiagnosticHaltWaitError> {
        if !matches!(self.state, HaltWaitState::Waiting) {
            return Err(DiagnosticHaltWaitError::NotWaiting);
        }
        self.session
            .clock
            .as_ref()
            .ok_or(DiagnosticHaltWaitError::Clock(
                VirtualClockError::ClockNotConfigured,
            ))?
            .advance_time_ns(delta_ns)
            .map_err(DiagnosticHaltWaitError::Clock)
    }

    /// The caller serializes the whole operation with CPU, APIC and IOAPIC
    /// operations and certifies the ISA interrupt profile's machine assumptions.
    pub fn poll_apic(&mut self, apic: &LocalApicHandle) -> DiagnosticHaltWakeResult {
        match &self.state {
            HaltWaitState::Entered(vector) => {
                return DiagnosticHaltWakeResult::Entered { vector: *vector };
            }
            HaltWaitState::Stopped => return DiagnosticHaltWakeResult::Stopped,
            HaltWaitState::Terminal(cause) => {
                return DiagnosticHaltWakeResult::Terminal(cause.clone());
            }
            HaltWaitState::Waiting => {}
        }
        self.session.set_clock_dispatch_allowed(false);
        if self.session.state.rflags & (1 << 9) == 0 {
            self.session.set_clock_dispatch_allowed(true);
            return DiagnosticHaltWakeResult::Waiting;
        }
        let Some(vector) = apic.eligible() else {
            self.session.set_clock_dispatch_allowed(true);
            return DiagnosticHaltWakeResult::Waiting;
        };
        let mut candidate = self.session.state.clone();
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.session.bus, &mut committed_write);
        match deliver_maskable_interrupt(
            &mut candidate,
            &mut memory,
            vector,
            self.session.access_context,
            self.session.rflags_defined,
        ) {
            InterruptDeliveryResult::OutsideProfile(reason) => {
                self.session.set_clock_dispatch_allowed(true);
                DiagnosticHaltWakeResult::OutsideProfile(reason)
            }
            InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
                let terminal = DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause };
                self.session.terminal(terminal.clone());
                self.state = HaltWaitState::Terminal(terminal.clone());
                DiagnosticHaltWakeResult::Terminal(terminal)
            }
            InterruptDeliveryResult::Delivered { .. } => {
                if !apic.acknowledge_vector(vector, || self.session.state = candidate) {
                    let terminal = DiagnosticTerminalExit::InterruptSelectionChanged;
                    self.session.terminal(terminal.clone());
                    self.state = HaltWaitState::Terminal(terminal.clone());
                    return DiagnosticHaltWakeResult::Terminal(terminal);
                }
                self.state = HaltWaitState::Entered(vector);
                DiagnosticHaltWakeResult::Entered { vector }
            }
        }
    }

    /// Read one qword from RAM without side effects (evidence only).
    pub(crate) fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        self.session.peek_ram_qword(gpa)
    }

    pub fn stop(&mut self) {
        if matches!(
            self.state,
            HaltWaitState::Waiting | HaltWaitState::Entered(_)
        ) {
            self.session.stop();
            self.state = HaltWaitState::Stopped;
        }
    }

    pub fn into_running(self) -> Result<DiagnosticVcpuSession, Self> {
        if matches!(self.state, HaltWaitState::Entered(_)) {
            self.session.set_clock_dispatch_allowed(true);
            Ok(self.session)
        } else {
            Err(self)
        }
    }
}

impl DiagnosticVcpuSession {
    /// The caller guarantees private RAM and no code mutation for the entire call.
    /// The event source remains owned by the caller and is never cleared here.
    pub fn run_jit_nop<E: ControlledBlockExecutor>(
        &mut self,
        code_start: u64,
        expected_code: &[u8],
        budget: u64,
        control: DispatchControl<'_>,
        executor: &mut E,
    ) -> Result<DiagnosticJitResult, DiagnosticJitError<E::Error>> {
        if self.status != DiagnosticVcpuState::Runnable
            || self.access_context.mode != InstructionMode::Long64
            || self.access_context.privilege != PrivilegeLevel::Ring0
            || self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow != Some(od_isa::StiShadow::Clear)
            || self.state.cr0.is_some()
            || self.state.cr3.is_some()
            || self.state.cr4.is_some()
            || self.rflags_defined != INTEGER_FLAGS
            || !self.completed_boundary
        {
            return Err(DiagnosticJitError::ProfileRejected);
        }
        let start = code_start;
        let length = u64::try_from(expected_code.len())
            .map_err(|_| DiagnosticJitError::CodeRangeRejected)?;
        let end = start
            .checked_add(length)
            .filter(|_| length != 0)
            .ok_or(DiagnosticJitError::CodeRangeRejected)?;
        if start / 4096 != (end - 1) / 4096 {
            return Err(DiagnosticJitError::CodeRangeRejected);
        }
        for offset in 0..length {
            let address = crate::GuestPhysicalAddress::new(start + offset);
            let range = crate::GuestPhysicalRange::new(address, 1)
                .map_err(|_| DiagnosticJitError::CodeRangeRejected)?;
            if !self.bus.tracked_physical_range_is_ram(range) {
                return Err(DiagnosticJitError::CodeRangeRejected);
            }
        }
        for (offset, expected) in expected_code.iter().enumerate() {
            let address = crate::GuestPhysicalAddress::new(start + offset as u64);
            let byte = self
                .bus
                .read_physical(address, crate::AccessWidth::Byte)
                .map_err(|_| DiagnosticJitError::CodeRangeRejected)?;
            if byte != u64::from(*expected) {
                return Err(DiagnosticJitError::CodeMismatch);
            }
        }
        let code = CodeView::new(start, expected_code)
            .map_err(|_| DiagnosticJitError::CodeRangeRejected)?;
        let admission = S0_09NopAdmission::from_code(code)
            .map_err(|_| DiagnosticJitError::AdmissionRejected)?;
        if !admission.contains(self.state.rip) {
            return Err(DiagnosticJitError::AdmissionRejected);
        }
        let mut candidate = self.state.clone();
        match od_jit::dispatch::run_with_nop_fallback(
            code,
            &admission,
            &mut candidate,
            budget,
            control,
            executor,
        ) {
            Ok(dispatch) => {
                self.state = candidate;
                if matches!(
                    dispatch.exit,
                    od_jit::dispatch::DispatchExit::Runtime(
                        od_jit::runtime::RuntimeExitTag::StopRequested
                    )
                ) {
                    self.stop();
                }
                Ok(DiagnosticJitResult { dispatch })
            }
            Err(error) => {
                self.stop();
                Err(DiagnosticJitError::Dispatch(error))
            }
        }
    }
    /// Attempt the exact bare HLT at a caller-certified completed boundary.
    /// The caller certifies private synchronous RAM, one vCPU, and absence of
    /// pending synchronous/debug/NMI events and unmodelled inhibition.
    pub fn into_halt_wait(mut self) -> Result<DiagnosticHaltWait, DiagnosticHaltEnterFailure> {
        if self.status != DiagnosticVcpuState::Runnable
            || self.access_context.mode != InstructionMode::Long64
            || self.access_context.privilege != PrivilegeLevel::Ring0
            || self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow.is_none()
            || (self.state.sti_shadow == Some(od_isa::StiShadow::Blocking)
                && self.state.rflags & (1 << 9) == 0)
            || self.state.cr0.is_some()
            || self.state.cr3.is_some()
            || self.state.cr4.is_some()
            || self.rflags_defined != INTEGER_FLAGS
            || self.clock.is_some() != self.rdtsc_controls.is_some()
            || !self.completed_boundary
        {
            return Err(DiagnosticHaltEnterFailure {
                session: self,
                reason: DiagnosticHaltEnterReason::ProfileRejected,
            });
        }
        let mut candidate = self.state.clone();
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        let outcome = step_with_halt_profile(
            &mut candidate,
            &mut memory,
            self.access_context,
            &od_isa::HaltProfile::Long64V1,
        );
        match outcome {
            Ok(StepOutcome::Advanced)
                if candidate.activity == Some(od_isa::CpuActivity::Halted) =>
            {
                self.state = candidate;
                Ok(DiagnosticHaltWait {
                    session: self,
                    state: HaltWaitState::Waiting,
                })
            }
            Ok(outcome) => Err(DiagnosticHaltEnterFailure {
                session: self,
                reason: DiagnosticHaltEnterReason::Step(outcome),
            }),
            Err(error) => Err(DiagnosticHaltEnterFailure {
                session: self,
                reason: DiagnosticHaltEnterReason::MemoryError(error),
            }),
        }
    }
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
            completed_boundary: true,
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
        let clock = VirtualClockHandle::new(tsc_hz).map_err(DiagnosticVcpuControlError::Clock)?;
        Self::new_with_clock_handle(
            state,
            bus,
            access_context,
            rflags_defined,
            clock,
            rdtsc_controls,
        )
    }

    /// Construct a session sharing its caller-created time domain with devices.
    pub fn new_with_clock_handle(
        state: LongModeState,
        bus: MachineBus,
        access_context: AccessContext,
        rflags_defined: u64,
        clock: VirtualClockHandle,
        rdtsc_controls: RdtscControls,
    ) -> Result<Self, DiagnosticVcpuControlError> {
        if rflags_defined & !INTEGER_FLAGS != 0 {
            return Err(DiagnosticVcpuControlError::ExecutionInput(
                od_isa::execute::RunInputError::InvalidRflagsDefined {
                    value: rflags_defined,
                },
            ));
        }
        let clock = clock
            .claim_advancer()
            .map_err(DiagnosticVcpuControlError::Clock)?;
        Ok(Self {
            state,
            bus,
            access_context,
            rflags_defined,
            status: DiagnosticVcpuState::Runnable,
            clock: Some(clock),
            rdtsc_controls: Some(rdtsc_controls),
            completed_boundary: true,
        })
    }

    pub fn virtual_clock(&self) -> Option<VirtualClock> {
        self.clock.as_ref().map(VirtualClockAdvancer::snapshot)
    }

    /// Advance virtual time only between runnable calls.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        self.clock
            .as_ref()
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
        self.set_clock_dispatch_allowed(false);
        self.status = DiagnosticVcpuState::Paused;
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Paused {
            return Err(DiagnosticVcpuControlError::CannotResume(
                self.status.clone(),
            ));
        }
        self.set_clock_dispatch_allowed(true);
        self.status = DiagnosticVcpuState::Runnable;
        Ok(())
    }

    /// Stop is idempotent. During a synchronous run, supply a stop source to
    /// `run`; it is polled by the existing CPU execution loop before fetch and
    /// at its established internal poll points.
    pub fn stop(&mut self) {
        if !matches!(self.status, DiagnosticVcpuState::Terminal(_)) {
            self.set_clock_dispatch_allowed(false);
            self.status = DiagnosticVcpuState::Stopped;
        }
    }

    fn set_clock_dispatch_allowed(&self, allowed: bool) {
        if let Some(clock) = &self.clock {
            clock.set_dispatch_allowed(allowed);
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
            let mut source = || clock.snapshot().tsc();
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
        Ok(self.record_run_result(result))
    }

    fn record_run_result(&mut self, result: od_isa::execute::RunResult) -> DiagnosticRunResult {
        self.rflags_defined = result.rflags_defined;
        if matches!(result.exit, RunExit::RepIterationBudgetExhausted) {
            self.completed_boundary = false;
        } else if result.instructions_executed > 0 {
            self.completed_boundary = true;
        }

        let exit = match result.exit {
            RunExit::InstructionBudgetExhausted => {
                DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
            }
            RunExit::RepIterationBudgetExhausted => {
                DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget)
            }
            RunExit::StopRequested => {
                self.set_clock_dispatch_allowed(false);
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

        DiagnosticRunResult {
            exit,
            instructions_executed: result.instructions_executed,
            rflags_defined: result.rflags_defined,
        }
    }

    /// Deliver one pending IRQ at a caller-certified completed instruction
    /// boundary, then execute one handler instruction. The caller certifies
    /// private synchronous RAM for delivery and serializes the entire operation
    /// with all operations on this APIC and its IOAPIC.
    pub fn deliver_interrupt_and_step(
        &mut self,
        apic: &LocalApicHandle,
    ) -> Result<DiagnosticInterruptResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        if !self.completed_boundary {
            return Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported);
        }
        if self.state.rflags & (1 << 9) == 0
            || self.state.sti_shadow == Some(od_isa::StiShadow::Blocking)
        {
            return Ok(DiagnosticInterruptResult::NoDelivery);
        }
        if self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow != Some(od_isa::StiShadow::Clear)
        {
            return Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported);
        }
        let Some(vector) = apic.eligible() else {
            return Ok(DiagnosticInterruptResult::NoDelivery);
        };
        let mut candidate = self.state.clone();
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        match deliver_maskable_interrupt(
            &mut candidate,
            &mut memory,
            vector,
            self.access_context,
            self.rflags_defined,
        ) {
            InterruptDeliveryResult::OutsideProfile(reason) => {
                Ok(DiagnosticInterruptResult::OutsideProfile(reason))
            }
            InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
                let terminal = DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause };
                self.terminal(terminal.clone());
                Ok(DiagnosticInterruptResult::Terminal(terminal))
            }
            InterruptDeliveryResult::Delivered { .. } => {
                if !apic.acknowledge_vector(vector, || self.state = candidate) {
                    let terminal = DiagnosticTerminalExit::InterruptSelectionChanged;
                    self.terminal(terminal.clone());
                    return Ok(DiagnosticInterruptResult::Terminal(terminal));
                }
                let handler = self.run(1, || false)?;
                Ok(DiagnosticInterruptResult::Delivered { vector, handler })
            }
        }
    }

    /// Poll APIC at a caller-certified completed boundary and attempt one
    /// ordinary instruction. The caller serializes CPU/APIC/IOAPIC operations
    /// and certifies private synchronous RAM, one vCPU, no pending synchronous,
    /// debug or NMI events or MOV-SS inhibition, and the ISA entry exclusions
    /// including disabled CET, VMX, SMM and shadow stacks.
    pub fn run_apic_quantum(
        &mut self,
        apic: &LocalApicHandle,
        mut stop_requested: impl FnMut() -> bool,
    ) -> Result<DiagnosticApicQuantumResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable
            || self.access_context.mode != InstructionMode::Long64
            || self.access_context.privilege != PrivilegeLevel::Ring0
            || self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow != Some(od_isa::StiShadow::Clear)
            || self.state.cr0.is_some()
            || self.state.cr3.is_some()
            || self.state.cr4.is_some()
            || self.rflags_defined & !INTEGER_FLAGS != 0
            || !self.completed_boundary
        {
            return Ok(DiagnosticApicQuantumResult::ProfileRejected);
        }
        if stop_requested() {
            self.stop();
            return Ok(DiagnosticApicQuantumResult::Stopped);
        }
        let vector = if self.state.rflags & (1 << 9) != 0 {
            apic.eligible()
        } else {
            None
        };
        if let Some(vector) = vector {
            let mut candidate = self.state.clone();
            let mut committed_write = |_| {};
            let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
            match deliver_maskable_interrupt(
                &mut candidate,
                &mut memory,
                vector,
                self.access_context,
                self.rflags_defined,
            ) {
                InterruptDeliveryResult::OutsideProfile(reason) => {
                    return Ok(DiagnosticApicQuantumResult::OutsideProfile(reason));
                }
                InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
                    let terminal =
                        DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause };
                    self.terminal(terminal.clone());
                    return Ok(DiagnosticApicQuantumResult::Terminal(terminal));
                }
                InterruptDeliveryResult::Delivered { .. } => {
                    if !apic.acknowledge_vector(vector, || self.state = candidate) {
                        let terminal = DiagnosticTerminalExit::InterruptSelectionChanged;
                        self.terminal(terminal.clone());
                        return Ok(DiagnosticApicQuantumResult::Terminal(terminal));
                    }
                }
            }
            let handler = self.run(1, &mut stop_requested)?;
            return Ok(DiagnosticApicQuantumResult::Delivered { vector, handler });
        }
        Ok(DiagnosticApicQuantumResult::Ordinary(
            self.run(1, &mut stop_requested)?,
        ))
    }

    /// Execute one bounded STI-profile quantum at a caller-certified boundary.
    /// The caller serializes APIC/IOAPIC operations, certifies a fresh completed
    /// boundary, private synchronous RAM, and absence of pending synchronous,
    /// debug or NMI events and MOV-SS inhibition. The STI and interrupt-entry
    /// profile exclusions, including one vCPU, non-PVI Long64 and disabled
    /// VMX, SMM and CET, also apply.
    pub fn run_sti_apic_quantum(
        &mut self,
        apic: &LocalApicHandle,
    ) -> DiagnosticStiApicQuantumResult {
        if self.status != DiagnosticVcpuState::Runnable
            || self.access_context.mode != InstructionMode::Long64
            || self.access_context.privilege != PrivilegeLevel::Ring0
            || self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow.is_none()
            || (self.state.sti_shadow == Some(od_isa::StiShadow::Blocking)
                && self.state.rflags & (1 << 9) == 0)
            || self.state.cr0.is_some()
            || self.state.cr3.is_some()
            || self.state.cr4.is_some()
            || self.rflags_defined != INTEGER_FLAGS
            || self.clock.is_some()
            || self.rdtsc_controls.is_some()
            || !self.completed_boundary
        {
            return DiagnosticStiApicQuantumResult::ProfileRejected;
        }
        let vector = if self.state.rflags & (1 << 9) != 0
            && self.state.sti_shadow == Some(od_isa::StiShadow::Clear)
        {
            apic.eligible()
        } else {
            None
        };
        if let Some(vector) = vector {
            let mut candidate = self.state.clone();
            let mut committed_write = |_| {};
            let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
            match deliver_maskable_interrupt(
                &mut candidate,
                &mut memory,
                vector,
                self.access_context,
                self.rflags_defined,
            ) {
                InterruptDeliveryResult::OutsideProfile(reason) => {
                    return DiagnosticStiApicQuantumResult::OutsideProfile(reason);
                }
                InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
                    let terminal =
                        DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause };
                    self.terminal(terminal.clone());
                    return DiagnosticStiApicQuantumResult::Terminal(terminal);
                }
                InterruptDeliveryResult::Delivered { .. } => {
                    if !apic.acknowledge_vector(vector, || self.state = candidate) {
                        let terminal = DiagnosticTerminalExit::InterruptSelectionChanged;
                        self.terminal(terminal.clone());
                        return DiagnosticStiApicQuantumResult::Terminal(terminal);
                    }
                }
            }
            let handler = self.run_sti_quantum();
            return DiagnosticStiApicQuantumResult::Delivered { vector, handler };
        }
        let result = self.run_sti_quantum();
        if matches!(result.exit, DiagnosticExit::Terminal(_)) {
            DiagnosticStiApicQuantumResult::TerminalExecution(result)
        } else {
            DiagnosticStiApicQuantumResult::Ordinary(result)
        }
    }

    fn run_sti_quantum(&mut self) -> DiagnosticRunResult {
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        let result = run_with_sti_profile(
            &mut self.state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: self.access_context,
            },
            &od_isa::StiProfile::NonPviLong64V1,
            || false,
        );
        self.record_run_result(result)
    }

    /// Attempt one same-CPL0 IRETQ. The caller certifies private synchronous
    /// RAM and the remaining prerequisites of the ISA IRETQ profile.
    pub fn run_same_cpl0_iretq(
        &mut self,
    ) -> Result<DiagnosticIretqResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        if !self.completed_boundary {
            return Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported);
        }
        let mut candidate = self.state.clone();
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        let result = run_with_iretq_profile(
            &mut candidate,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: self.access_context,
            },
            self.rflags_defined,
            IretqProfile::SameCpl0ValidFrameV1,
            || false,
        );
        if result.exit == IretqRunExit::InstructionBudgetExhausted
            && result.instructions_executed == 1
        {
            self.state = candidate;
            self.rflags_defined = result.rflags_defined;
            return Ok(DiagnosticIretqResult::Retired);
        }
        let cause = match result.exit {
            IretqRunExit::Fault(fault) => DiagnosticTerminalExit::Fault(fault),
            IretqRunExit::ReadAborted { phase, cause } => {
                DiagnosticTerminalExit::IretqReadAborted { phase, cause }
            }
            IretqRunExit::Unsupported => DiagnosticTerminalExit::IretqUnsupported,
            // Fixed configuration makes these exits unreachable; treat any
            // non-retiring outcome as terminal rather than publishing state.
            _ => DiagnosticTerminalExit::IretqUnsupported,
        };
        self.terminal(cause.clone());
        Ok(DiagnosticIretqResult::Terminal(cause))
    }

    // Crate-private surface for the S4-24e UART IRQ probe owner. Each method
    // is one caller-driven boundary and never schedules work.

    /// Fetch up to `len` code bytes at RIP from RAM without side effects.
    /// A short result means the next byte is not readable RAM.
    pub(crate) fn peek_code(&mut self, len: usize) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(len);
        for offset in 0..len as u64 {
            let address = crate::GuestPhysicalAddress::new(self.state.rip.wrapping_add(offset));
            if !matches!(
                self.bus.memory_kind(address, 1),
                Ok(crate::MemoryRegionKind::Ram)
            ) {
                break;
            }
            match self.bus.read_physical(address, crate::AccessWidth::Byte) {
                Ok(value) => bytes.push(value as u8),
                Err(_) => break,
            }
        }
        bytes
    }

    /// Read one little-endian qword from RAM without side effects, for
    /// evidence (for example an interrupt frame). `None` when any of the eight
    /// bytes is not readable RAM.
    pub(crate) fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        let mut value = 0_u64;
        for offset in 0..8_u64 {
            let address = crate::GuestPhysicalAddress::new(gpa.checked_add(offset)?);
            if !matches!(
                self.bus.memory_kind(address, 1),
                Ok(crate::MemoryRegionKind::Ram)
            ) {
                return None;
            }
            let byte = self
                .bus
                .read_physical(address, crate::AccessWidth::Byte)
                .ok()?;
            value |= (byte & 0xff) << (8 * offset);
        }
        Some(value)
    }

    /// Deliver one eligible IRQ at a completed boundary without executing any
    /// handler instruction. Returns `Ok(None)` when delivery is not possible
    /// at this boundary (IF clear, STI shadow, no eligible vector).
    pub(crate) fn deliver_pending_irq(
        &mut self,
        apic: &LocalApicHandle,
    ) -> Result<Option<DiagnosticIrqEntry>, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        if !self.completed_boundary {
            return Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported);
        }
        if self.state.rflags & (1 << 9) == 0
            || self.state.sti_shadow == Some(od_isa::StiShadow::Blocking)
        {
            return Ok(None);
        }
        if self.state.activity != Some(od_isa::CpuActivity::Running)
            || self.state.sti_shadow != Some(od_isa::StiShadow::Clear)
        {
            return Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported);
        }
        let Some(vector) = apic.eligible() else {
            return Ok(None);
        };
        let mut candidate = self.state.clone();
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        match deliver_maskable_interrupt(
            &mut candidate,
            &mut memory,
            vector,
            self.access_context,
            self.rflags_defined,
        ) {
            InterruptDeliveryResult::OutsideProfile(reason) => {
                Ok(Some(DiagnosticIrqEntry::OutsideProfile(reason)))
            }
            InterruptDeliveryResult::DeliveryAborted { phase, cause } => {
                let terminal = DiagnosticTerminalExit::InterruptDeliveryAborted { phase, cause };
                self.terminal(terminal.clone());
                Ok(Some(DiagnosticIrqEntry::Terminal(terminal)))
            }
            InterruptDeliveryResult::Delivered { .. } => {
                if !apic.acknowledge_vector(vector, || self.state = candidate) {
                    let terminal = DiagnosticTerminalExit::InterruptSelectionChanged;
                    self.terminal(terminal.clone());
                    return Ok(Some(DiagnosticIrqEntry::Terminal(terminal)));
                }
                Ok(Some(DiagnosticIrqEntry::Entered { vector }))
            }
        }
    }

    /// Execute one flat `LIDT [RSP+disp8]` at RIP.
    pub(crate) fn step_flat_lidt(
        &mut self,
    ) -> Result<DiagnosticRunResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        let mut committed_write = |_| {};
        let mut memory = IdentityMemoryAdapter::new(&mut self.bus, &mut committed_write);
        let result = od_isa::step_flat_lidt(
            od_isa::FlatLidtLong64V1,
            &mut self.state,
            &mut memory,
            self.access_context,
            self.rflags_defined,
        );
        let retired = result.instructions_executed;
        let exit = match result.exit {
            od_isa::PagedProbeExit::Retired => {
                self.completed_boundary = true;
                DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
            }
            od_isa::PagedProbeExit::Fault(fault) => {
                self.terminal(DiagnosticTerminalExit::Fault(fault))
            }
            od_isa::PagedProbeExit::Unsupported { opcode } => {
                self.terminal(DiagnosticTerminalExit::Unsupported { opcode })
            }
            od_isa::PagedProbeExit::Backend(error) => {
                self.terminal(DiagnosticTerminalExit::BackendError(error))
            }
            other => self.terminal(DiagnosticTerminalExit::BackendError(MemoryError::Backend(
                format!("LIDT profile exit {other:?}"),
            ))),
        };
        Ok(DiagnosticRunResult {
            exit,
            instructions_executed: retired,
            rflags_defined: self.rflags_defined,
        })
    }

    /// Execute one bare `IN AL,DX` / `OUT DX,AL` through the machine PIO bus.
    pub(crate) fn step_pio_u8(
        &mut self,
    ) -> Result<DiagnosticRunResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
        }
        let result = crate::step_diagnostic_pio_u8(
            &mut self.state,
            &mut self.bus,
            self.access_context,
            self.rflags_defined,
        );
        let retired = result.instructions_executed;
        self.rflags_defined = result.rflags_defined;
        let exit = match result.exit {
            od_isa::execute::PioU8Exit::Retired => {
                self.completed_boundary = true;
                DiagnosticExit::WorkLimit(WorkLimitReason::InstructionBudget)
            }
            od_isa::execute::PioU8Exit::Fault(fault) => {
                self.terminal(DiagnosticTerminalExit::Fault(fault))
            }
            od_isa::execute::PioU8Exit::Unsupported { opcode } => {
                self.terminal(DiagnosticTerminalExit::Unsupported { opcode })
            }
            od_isa::execute::PioU8Exit::MemoryError(error) => {
                self.terminal(DiagnosticTerminalExit::BackendError(error))
            }
            od_isa::execute::PioU8Exit::PortIoError(error) => {
                self.terminal(DiagnosticTerminalExit::BackendError(MemoryError::Backend(
                    format!("port I/O error: {error:?}"),
                )))
            }
            od_isa::execute::PioU8Exit::Rejected(error) => {
                self.terminal(DiagnosticTerminalExit::BackendError(MemoryError::Backend(
                    format!("port I/O admission: {error:?}"),
                )))
            }
        };
        Ok(DiagnosticRunResult {
            exit,
            instructions_executed: retired,
            rflags_defined: self.rflags_defined,
        })
    }

    fn terminal(&mut self, cause: DiagnosticTerminalExit) -> DiagnosticExit {
        self.set_clock_dispatch_allowed(false);
        self.status = DiagnosticVcpuState::Terminal(cause.clone());
        DiagnosticExit::Terminal(cause)
    }
}

#[cfg(test)]
mod periodic_timer_tests;

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
    fn rdtsc_and_apic_timer_share_session_clock() {
        let apic_base = GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE);
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(RAM_START), RAM_LEN).unwrap();
        let apic_range = GuestPhysicalRange::new(apic_base, crate::LOCAL_APIC_MMIO_SIZE).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(ram, MemoryRegionKind::Ram),
                MemoryRegionSpec::new(apic_range, MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let device = crate::LocalApic::with_shared_timer(clock.clone(), 1_000_000_000).unwrap();
        let apic = device.handle();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(RAM_LEN),
            vec![Box::new(device)],
            vec![crate::MmioBinding {
                region_start: apic_base,
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        for (offset, byte) in [0x0f_u8, 0x31, 0x0f, 0x31].into_iter().enumerate() {
            bus.write_physical(
                GuestPhysicalAddress::new(RAM_START + offset as u64),
                AccessWidth::Byte,
                u64::from(byte),
                |_| {},
            )
            .unwrap();
        }
        let mut state = LongModeState::default();
        state.rip = RAM_START;
        let mut cpu = DiagnosticVcpuSession::new_with_clock_handle(
            state,
            bus,
            context(),
            INTEGER_FLAGS,
            clock,
            RdtscControls { cr4_tsd: false },
        )
        .unwrap();
        let write_mmio = |cpu: &mut DiagnosticVcpuSession, offset: u64, value: u64| {
            cpu.bus
                .write_physical(
                    GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + offset),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .unwrap();
        };
        let read_mmio = |cpu: &mut DiagnosticVcpuSession, offset: u64| {
            cpu.bus
                .read_physical(
                    GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + offset),
                    AccessWidth::Dword,
                )
                .unwrap()
        };

        write_mmio(&mut cpu, 0xf0, 0x1ff);
        write_mmio(&mut cpu, 0x320, 0x40);
        write_mmio(&mut cpu, 0x3e0, 0xb);
        write_mmio(&mut cpu, 0x380, 4);
        cpu.advance_time_ns(2).unwrap();
        assert_eq!(run(&mut cpu, 1).instructions_executed, 1);
        assert_eq!(cpu.state().gpr[od_isa::Register::Rax as usize], 2);
        assert_eq!(read_mmio(&mut cpu, 0x390), 2);
        assert_eq!(
            apic.advance_timer_ns(1),
            Err(crate::DeviceError::UnsupportedConfiguration)
        );

        cpu.advance_time_ns(2).unwrap();
        assert_eq!(run(&mut cpu, 1).instructions_executed, 1);
        assert_eq!(cpu.state().gpr[od_isa::Register::Rax as usize], 4);
        assert_eq!(read_mmio(&mut cpu, 0x390), 0);
        cpu.pause().unwrap();
        assert_eq!(apic.dispatch_timer(), Err(crate::DeviceError::Busy));
        assert_eq!(apic.pending(), None);
        cpu.resume().unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x40));

        apic.reset_checked().unwrap();
        write_mmio(&mut cpu, 0xf0, 0x1ff);
        write_mmio(&mut cpu, 0x320, 0x40);
        write_mmio(&mut cpu, 0x380, 1);
        cpu.advance_time_ns(2).unwrap();
        cpu.stop();
        assert_eq!(apic.dispatch_timer(), Err(crate::DeviceError::Busy));
        assert_eq!(apic.pending(), None);
        assert_eq!(cpu.virtual_clock().unwrap().now_ns(), 6);
    }

    #[test]
    fn jit_code_binding_rejects_before_executor_and_error_does_not_commit() {
        use od_jit::dispatch::{BlockExecutionReport, DispatchControl};
        use std::sync::atomic::AtomicU8;
        struct Fail(usize);
        impl ControlledBlockExecutor for Fail {
            type Error = ();
            fn execute_one_block_controlled(
                &mut self,
                _: &od_jit::block_compile::CompiledBlock,
                state: &mut LongModeState,
                _: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                self.0 += 1;
                state.rip += 1;
                Err(())
            }
        }
        let code = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let mut s = session(&code, RAM_START);
        s.rflags_defined = INTEGER_FLAGS;
        s.state.activity = Some(od_isa::CpuActivity::Running);
        s.state.sti_shadow = Some(od_isa::StiShadow::Clear);
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut executor = Fail(0);
        assert!(matches!(
            s.run_jit_nop(RAM_START, &[0x90], 1, control, &mut executor),
            Err(DiagnosticJitError::CodeMismatch)
        ));
        assert!(matches!(
            s.run_jit_nop(RAM_START, &[], 1, control, &mut executor),
            Err(DiagnosticJitError::CodeRangeRejected)
        ));
        assert_eq!(executor.0, 0);
        let original = s.state.clone();
        assert!(matches!(
            s.run_jit_nop(RAM_START, &code, 1, control, &mut executor),
            Err(DiagnosticJitError::Dispatch(_))
        ));
        assert_eq!(s.state, original);
        assert_eq!(s.status, DiagnosticVcpuState::Stopped);
        assert_eq!(executor.0, 1);
        assert!(matches!(
            s.run_jit_nop(RAM_START, &code, 1, control, &mut executor),
            Err(DiagnosticJitError::ProfileRejected)
        ));
        assert_eq!(executor.0, 1);
    }

    #[test]
    fn jit_resume_uses_full_code_and_checks_boundary_and_all_bytes() {
        use od_jit::dispatch::{BlockExecutionReport, DispatchControl, DispatchExit};
        use std::sync::atomic::{AtomicBool, AtomicU8};
        struct NoExecution;
        impl ControlledBlockExecutor for NoExecution {
            type Error = ();
            fn execute_one_block_controlled(
                &mut self,
                _: &od_jit::block_compile::CompiledBlock,
                _: &mut LongModeState,
                _: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                panic!("unexpected native entry")
            }
        }
        // The backwards JNZ targets the NOP in the prefix.
        let code = [0x90, 0x75, 0xfd];
        let mut s = session(&code, RAM_START + 1);
        s.rflags_defined = INTEGER_FLAGS;
        s.state.activity = Some(od_isa::CpuActivity::Running);
        s.state.sti_shadow = Some(od_isa::StiShadow::Clear);
        let stop = AtomicU8::new(0);
        let event = AtomicBool::new(true);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: Some(&event),
        };
        let mut executor = NoExecution;
        let result = s
            .run_jit_nop(RAM_START, &code, 4, control, &mut executor)
            .unwrap();
        assert_eq!(result.dispatch.exit, DispatchExit::EventPending);
        assert_eq!(s.state.rip, RAM_START + 1);
        for rip in [RAM_START - 1, RAM_START + 2, RAM_START + 4] {
            s.state.rip = rip;
            assert!(matches!(
                s.run_jit_nop(RAM_START, &code, 4, control, &mut executor),
                Err(DiagnosticJitError::AdmissionRejected)
            ));
        }
        s.state.rip = RAM_START + 1;
        assert!(matches!(
            s.run_jit_nop(RAM_START, &[0x91, 0x75, 0xfd], 4, control, &mut executor),
            Err(DiagnosticJitError::CodeMismatch)
        ));
        s.state.rip = RAM_START + code.len() as u64;
        assert_eq!(
            s.run_jit_nop(RAM_START, &code, 4, control, &mut executor)
                .unwrap()
                .dispatch
                .exit,
            DispatchExit::CodeEnd
        );
    }

    #[test]
    fn jit_code_range_rejections_preserve_session_ram_and_device() {
        use crate::{BusDevice, DeviceAccess, DeviceError, MmioBinding};
        use od_jit::dispatch::{BlockExecutionReport, DispatchControl};
        use std::sync::{
            Arc,
            atomic::{AtomicU8, AtomicUsize, Ordering},
        };

        struct CountingDevice(Arc<AtomicUsize>);
        impl BusDevice for CountingDevice {
            fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(0)
            }
            fn write(&mut self, _: DeviceAccess, _: u64) -> Result<(), DeviceError> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        struct CountingExecutor(usize);
        impl ControlledBlockExecutor for CountingExecutor {
            type Error = ();
            fn execute_one_block_controlled(
                &mut self,
                _: &od_jit::block_compile::CompiledBlock,
                _: &mut LongModeState,
                _: u64,
                _: DispatchControl<'_>,
            ) -> Result<BlockExecutionReport, ()> {
                self.0 += 1;
                Err(())
            }
        }

        const MMIO_START: u64 = 0x3000;
        let range =
            |start, len| GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![
                MemoryRegionSpec::new(range(RAM_START, RAM_LEN), MemoryRegionKind::Ram),
                MemoryRegionSpec::new(range(MMIO_START, 0x1000), MemoryRegionKind::Mmio),
            ],
        )
        .unwrap();
        let device_calls = Arc::new(AtomicUsize::new(0));
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(RAM_LEN),
            vec![Box::new(CountingDevice(device_calls.clone()))],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(MMIO_START),
                device_index: 0,
            }],
            vec![],
        )
        .unwrap();
        for (address, value) in [(RAM_START, 0xa5), (RAM_START + RAM_LEN - 1, 0x5a)] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Byte,
                value,
                |_| {},
            )
            .unwrap();
        }
        let mut state = LongModeState::default();
        state.activity = Some(od_isa::CpuActivity::Running);
        state.sti_shadow = Some(od_isa::StiShadow::Clear);
        let mut s = DiagnosticVcpuSession::new(state, bus, context(), 0).unwrap();
        s.rflags_defined = INTEGER_FLAGS;
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        let mut executor = CountingExecutor(0);
        for (rip, code) in [
            (RAM_START + RAM_LEN - 1, &[0x5a, 0x90][..]), // cross 4 KiB boundary
            (u64::MAX, &[0x90][..]),                      // exclusive end overflows
            (0x2000, &[0x90][..]),                        // unmapped
            (MMIO_START, &[0x90][..]),                    // MMIO must never be read
        ] {
            s.state.rip = rip;
            let original = s.state.clone();
            let flags = s.rflags_defined;
            let boundary = s.completed_boundary;
            assert!(
                matches!(
                    s.run_jit_nop(rip, code, 1, control, &mut executor),
                    Err(DiagnosticJitError::CodeRangeRejected)
                ),
                "rip={rip:#x}"
            );
            assert_eq!(s.state, original);
            assert_eq!(s.status, DiagnosticVcpuState::Runnable);
            assert_eq!(s.rflags_defined, flags);
            assert_eq!(s.completed_boundary, boundary);
            assert_eq!(executor.0, 0);
            assert_eq!(device_calls.load(Ordering::SeqCst), 0);
            for (address, value) in [(RAM_START, 0xa5), (RAM_START + RAM_LEN - 1, 0x5a)] {
                assert_eq!(
                    s.bus
                        .read_physical(GuestPhysicalAddress::new(address), AccessWidth::Byte)
                        .unwrap(),
                    value
                );
            }
        }
    }

    fn irq_session(rsp: u64) -> (DiagnosticVcpuSession, crate::IoApicHandle, LocalApicHandle) {
        irq_session_with_apic(rsp, crate::LocalApic::new(), None)
    }

    fn irq_session_with_apic(
        rsp: u64,
        local: crate::LocalApic,
        clock: Option<VirtualClockHandle>,
    ) -> (DiagnosticVcpuSession, crate::IoApicHandle, LocalApicHandle) {
        use crate::{IO_APIC_MMIO_BASE, IoApic, LOCAL_APIC_MMIO_BASE, MmioBinding};
        let apic = local.handle();
        let io = IoApic::new(apic.clone());
        let io_handle = io.handle();
        let specs = [
            (0x1000, 0x7000, MemoryRegionKind::Ram),
            (IO_APIC_MMIO_BASE, 0x1000, MemoryRegionKind::Mmio),
            (LOCAL_APIC_MMIO_BASE, 0x1000, MemoryRegionKind::Mmio),
        ];
        let map = MachineMemoryMap::new(
            36,
            specs
                .into_iter()
                .map(|(base, len, kind)| {
                    MemoryRegionSpec::new(
                        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len).unwrap(),
                        kind,
                    )
                })
                .collect(),
        )
        .unwrap();
        let mut bus = MachineBus::new(
            map,
            RamLimits::new(0x7000),
            vec![Box::new(io), Box::new(local)],
            vec![
                MmioBinding {
                    region_start: GuestPhysicalAddress::new(IO_APIC_MMIO_BASE),
                    device_index: 0,
                },
                MmioBinding {
                    region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
                    device_index: 1,
                },
            ],
            vec![],
        )
        .unwrap();
        fn write(bus: &mut MachineBus, addr: u64, width: AccessWidth, value: u64) {
            bus.write_physical(GuestPhysicalAddress::new(addr), width, value, |_| {})
                .unwrap();
        }
        write(
            &mut bus,
            LOCAL_APIC_MMIO_BASE + 0xf0,
            AccessWidth::Dword,
            0x1ff,
        );
        write(&mut bus, IO_APIC_MMIO_BASE, AccessWidth::Dword, 0x18);
        write(
            &mut bus,
            IO_APIC_MMIO_BASE + 0x10,
            AccessWidth::Dword,
            0x8051,
        );
        // Present 64-bit interrupt gate and already-accessed ring-0 code descriptor.
        write(
            &mut bus,
            0x3000 + 0x51 * 16,
            AccessWidth::Qword,
            0x8e00_0020_4000,
        );
        write(&mut bus, 0x3008 + 0x51 * 16, AccessWidth::Qword, 0);
        write(&mut bus, 0x2020, AccessWidth::Qword, 0x0020_9b00_0000_0000);
        for (i, byte) in [0x48, 0x8b, 0x03, 0x90].into_iter().enumerate() {
            write(&mut bus, 0x4000 + i as u64, AccessWidth::Byte, byte);
        }
        write(&mut bus, 0x5000, AccessWidth::Qword, 0x1234_5678_9abc_def0);
        let mut state = LongModeState::default();
        state.rip = 0x4100;
        state.rflags = 0x202;
        state.gpr[od_isa::Register::Rsp as usize] = rsp;
        state.gpr[od_isa::Register::Rbx as usize] = 0x5000;
        state.cs_selector = Some(8);
        state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        });
        state.ss_selector = Some(0x10);
        state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        });
        state.gdtr = Some(od_isa::state::Gdtr {
            base: 0x2000,
            limit: 0x2f,
        });
        state.idtr = Some(od_isa::state::Idtr {
            base: 0x3000,
            limit: 0xfff,
        });
        state.activity = Some(od_isa::CpuActivity::Running);
        state.sti_shadow = Some(od_isa::StiShadow::Clear);
        let session = if let Some(clock) = clock {
            DiagnosticVcpuSession::new_with_clock_handle(
                state,
                bus,
                context(),
                INTEGER_FLAGS,
                clock,
                RdtscControls { cr4_tsd: false },
            )
            .unwrap()
        } else {
            DiagnosticVcpuSession::new(state, bus, context(), 0).unwrap()
        };
        (session, io_handle, apic)
    }

    #[test]
    fn irq_level_delivery_runs_one_handler_instruction() {
        let (mut session, io, apic) = irq_session(0x7000);
        let before = session.state.clone();
        io.set_intin4(true).unwrap();
        assert!(matches!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::Delivered {
                vector: 0x51,
                handler: DiagnosticRunResult {
                    instructions_executed: 1,
                    ..
                }
            }
        ));
        assert_eq!(session.state.rip, 0x4003);
        assert_eq!(session.state.gpr[0], 0x1234_5678_9abc_def0);
        assert_eq!(session.state.rflags & 0x200, 0);
        assert_eq!(apic.pending(), None);
        for (addr, value) in [
            (0x6ff8, 0x10),
            (0x6ff0, 0x7000),
            (0x6fe8, before.rflags),
            (0x6fe0, 8),
            (0x6fd8, before.rip),
        ] {
            assert_eq!(
                session
                    .bus
                    .read_physical(GuestPhysicalAddress::new(addr), AccessWidth::Qword)
                    .unwrap(),
                value
            );
        }
        let read = |bus: &mut MachineBus, addr| {
            bus.read_physical(GuestPhysicalAddress::new(addr), AccessWidth::Dword)
                .unwrap()
        };
        assert_eq!(
            read(&mut session.bus, crate::LOCAL_APIC_MMIO_BASE + 0x120) & (1 << 17),
            1 << 17
        );
        assert_ne!(
            read(&mut session.bus, crate::IO_APIC_MMIO_BASE + 0x10) & (1 << 14),
            0
        );
        io.set_intin4(false).unwrap();
        session
            .bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0xb0),
                AccessWidth::Dword,
                0,
                |_| {},
            )
            .unwrap();
        assert_eq!(
            read(&mut session.bus, crate::IO_APIC_MMIO_BASE + 0x10) & (1 << 14),
            0
        );
    }

    fn irq_return_session(
        rsp: u64,
    ) -> (DiagnosticVcpuSession, crate::IoApicHandle, LocalApicHandle) {
        let (mut session, io, apic) = irq_session(rsp);
        let mut write = |addr, width, value| {
            session
                .bus
                .write_physical(GuestPhysicalAddress::new(addr), width, value, |_| {})
                .unwrap();
        };
        // MOV [RDI], EAX; IRETQ. The interrupted instruction loads a sentinel.
        for (address, byte) in [
            (0x4000, 0x89),
            (0x4001, 0x07),
            (0x4002, 0x48),
            (0x4003, 0xcf),
            (0x4100, 0x48),
            (0x4101, 0x8b),
            (0x4102, 0x03),
        ] {
            write(address, AccessWidth::Byte, byte);
        }
        write(0x2008, AccessWidth::Qword, 0x00af_9b00_0000_ffff);
        write(0x2010, AccessWidth::Qword, 0x00cf_9300_0000_ffff);
        session.state.gpr[od_isa::Register::Rdi as usize] = crate::LOCAL_APIC_MMIO_BASE + 0xb0;
        session.state.gpr[od_isa::Register::Rax as usize] = 0;
        session.rflags_defined = INTEGER_FLAGS;
        (session, io, apic)
    }

    fn halt_fixture(rsp: u64) -> (DiagnosticVcpuSession, crate::IoApicHandle, LocalApicHandle) {
        let (mut session, io, apic) = irq_return_session(rsp);
        for (address, byte) in [
            (0x4100, 0xf4),
            (0x4101, 0x48),
            (0x4102, 0x8b),
            (0x4103, 0x03),
        ] {
            session
                .bus
                .write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Byte,
                    byte,
                    |_| {},
                )
                .unwrap();
        }
        (session, io, apic)
    }

    fn enter_wait(session: DiagnosticVcpuSession) -> DiagnosticHaltWait {
        match session.into_halt_wait() {
            Ok(wait) => wait,
            Err(failure) => panic!("HLT failed: {:?}", failure.reason),
        }
    }

    #[test]
    fn shared_timer_advances_and_wakes_halt_wait() {
        let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
        let local = crate::LocalApic::with_shared_timer(clock.clone(), 1_000_000_000).unwrap();
        let (mut session, _, apic) = irq_session_with_apic(0x7000, local, Some(clock.clone()));
        put(&mut session, 0x4100, &[0xf4, 0x0f, 0x31]);
        put(&mut session, 0x4000, &[0x89, 0x07, 0x48, 0xcf]);
        for (address, value) in [
            (0x2008, 0x00af_9b00_0000_ffff),
            (0x2010, 0x00cf_9300_0000_ffff),
        ] {
            session
                .bus
                .write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Qword,
                    value,
                    |_| {},
                )
                .unwrap();
        }
        session.state.cs_selector = Some(8);
        session.state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        });
        session.state.ss_selector = Some(0x10);
        session.state.ss_cache = Some(od_isa::StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        });
        session.state.gpr[od_isa::Register::Rdi as usize] = crate::LOCAL_APIC_MMIO_BASE + 0xb0;
        session.state.gpr[od_isa::Register::Rax as usize] = 0;
        for (address, value) in [(0x3400, 0x8e00_0020_4000), (0x3408, 0)] {
            session
                .bus
                .write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Qword,
                    value,
                    |_| {},
                )
                .unwrap();
        }
        for (offset, value) in [(0x320, 0x40), (0x3e0, 0xb), (0x380, 3)] {
            session
                .bus
                .write_physical(
                    GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + offset),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .unwrap();
        }

        let mut wait = enter_wait(session);
        wait.advance_time_ns(1).unwrap();
        assert_eq!(
            wait.advance_time_ns(u64::MAX),
            Err(DiagnosticHaltWaitError::Clock(
                VirtualClockError::TimeOverflow
            ))
        );
        assert_eq!(wait.session.virtual_clock().unwrap().now_ns(), 1);
        assert!(!apic.dispatch_timer().unwrap());
        wait.advance_time_ns(2).unwrap();
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x40));

        // A second due vector must remain queued while interrupt entry commits.
        for (offset, value) in [(0x320, 0x41), (0x380, 1)] {
            wait.session
                .bus
                .write_physical(
                    GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + offset),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .unwrap();
        }
        wait.advance_time_ns(2).unwrap();
        assert_eq!(
            wait.poll_apic(&apic),
            DiagnosticHaltWakeResult::Entered { vector: 0x40 }
        );
        assert_eq!(apic.dispatch_timer(), Err(crate::DeviceError::Busy));
        assert_eq!(
            wait.advance_time_ns(1),
            Err(DiagnosticHaltWaitError::NotWaiting)
        );
        let mut session = match wait.into_running() {
            Ok(session) => session,
            Err(_) => panic!("entry did not wake"),
        };
        assert!(apic.dispatch_timer().unwrap());
        assert_eq!(apic.pending(), Some(0x41));

        assert_eq!(run(&mut session, 1).instructions_executed, 1);
        assert_eq!(
            session.run_same_cpl0_iretq().unwrap(),
            DiagnosticIretqResult::Retired
        );
        assert_eq!(run(&mut session, 1).instructions_executed, 1);
        assert_eq!(session.state.gpr[od_isa::Register::Rax as usize], 5);
    }

    #[test]
    fn halt_wait_entry_eoi_iretq_resumes_after_hlt() {
        for low in 0..16 {
            let (mut session, io, apic) = halt_fixture(0x7000 + low);
            session.state.sti_shadow = Some(od_isa::StiShadow::Blocking);
            let mut wait = enter_wait(session);
            assert_eq!(wait.state().rip, 0x4101);
            assert_eq!(wait.state().activity, Some(od_isa::CpuActivity::Halted));
            assert_eq!(wait.state().sti_shadow, Some(od_isa::StiShadow::Clear));
            wait = match wait.into_running() {
                Err(wait) => wait,
                Ok(_) => panic!("woke without interrupt"),
            };
            assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
            io.set_intin4(true).unwrap();
            io.set_intin4(false).unwrap();
            assert_eq!(
                wait.poll_apic(&apic),
                DiagnosticHaltWakeResult::Entered { vector: 0x51 }
            );
            assert_eq!(
                wait.poll_apic(&apic),
                DiagnosticHaltWakeResult::Entered { vector: 0x51 }
            );
            let mut session = match wait.into_running() {
                Ok(session) => session,
                Err(_) => panic!("entry did not wake"),
            };
            assert_eq!(session.state.activity, Some(od_isa::CpuActivity::Running));
            assert_eq!(session.state.rip, 0x4000);
            for (addr, value) in [
                (0x6ff8, 0x10),
                (0x6ff0, 0x7000 + low),
                (0x6fe8, 0x202),
                (0x6fe0, 8),
                (0x6fd8, 0x4101),
            ] {
                assert_eq!(
                    session
                        .bus
                        .read_physical(GuestPhysicalAddress::new(addr), AccessWidth::Qword)
                        .unwrap(),
                    value
                );
            }
            assert_eq!(run(&mut session, 1).instructions_executed, 1);
            assert_eq!(
                session.run_same_cpl0_iretq().unwrap(),
                DiagnosticIretqResult::Retired
            );
            assert_eq!(session.state.rip, 0x4101);
            assert_eq!(run(&mut session, 1).instructions_executed, 1);
            assert_eq!(session.state.gpr[0], 0x1234_5678_9abc_def0);
        }
    }

    #[test]
    fn halt_wait_rejection_preserves_session_and_boundary() {
        for case in 0..8 {
            let (mut session, _, _) = halt_fixture(0x7000);
            match case {
                0 => session.completed_boundary = false,
                1 => session.state.cr0 = Some(0),
                2 => session.state.cr3 = Some(0),
                3 => session.state.cr4 = Some(0),
                4 => session.state.sti_shadow = None,
                5 => session.rflags_defined = 0,
                6 => {
                    session.state.sti_shadow = Some(od_isa::StiShadow::Blocking);
                    session.state.rflags &= !0x200;
                }
                _ => session.pause().unwrap(),
            }
            let before = session.state.clone();
            let failure = match session.into_halt_wait() {
                Err(failure) => failure,
                Ok(_) => panic!("accepted invalid profile"),
            };
            assert_eq!(failure.reason, DiagnosticHaltEnterReason::ProfileRejected);
            assert_eq!(failure.session.state, before);
        }
        for (rip, bytes) in [
            (0x4100, Some(0x90)),
            (0x9000, None),
            (0x0000_8000_0000_0000, None),
        ] {
            let (mut session, _, _) = halt_fixture(0x7000);
            session.state.rip = rip;
            if let Some(byte) = bytes {
                session
                    .bus
                    .write_physical(
                        GuestPhysicalAddress::new(rip),
                        AccessWidth::Byte,
                        byte,
                        |_| {},
                    )
                    .unwrap();
            }
            let before = session.state.clone();
            let failure = match session.into_halt_wait() {
                Err(failure) => failure,
                Ok(_) => panic!("accepted non-HLT"),
            };
            if rip == 0x0000_8000_0000_0000 {
                assert!(matches!(
                    failure.reason,
                    DiagnosticHaltEnterReason::Step(StepOutcome::Fault(_))
                ));
            }
            assert!(matches!(
                failure.reason,
                DiagnosticHaltEnterReason::Step(StepOutcome::Unsupported { .. })
                    | DiagnosticHaltEnterReason::Step(StepOutcome::Fault(_))
                    | DiagnosticHaltEnterReason::MemoryError(_)
            ));
            assert_eq!(failure.session.state, before);
            assert_eq!(failure.session.status, DiagnosticVcpuState::Runnable);
        }
    }

    #[test]
    fn halt_wait_stop_after_entry_preserves_committed_interrupt() {
        let (session, io, apic) = halt_fixture(0x7000);
        let mut wait = enter_wait(session);
        io.set_intin4(true).unwrap();
        assert_eq!(
            wait.poll_apic(&apic),
            DiagnosticHaltWakeResult::Entered { vector: 0x51 }
        );
        let committed = wait.state().clone();
        assert_eq!(committed.activity, Some(od_isa::CpuActivity::Running));
        assert_eq!(committed.rip, 0x4000);
        wait.stop();
        assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Stopped);
        let mut wait = match wait.into_running() {
            Err(wait) => wait,
            Ok(_) => panic!("stopped session recovered"),
        };
        assert_eq!(wait.session.status, DiagnosticVcpuState::Stopped);
        assert_eq!(wait.state(), &committed);
        assert_eq!(apic.pending(), None);
        for (addr, value) in [
            (0x6ff8, 0x10),
            (0x6ff0, 0x7000),
            (0x6fe8, 0x202),
            (0x6fe0, 8),
            (0x6fd8, 0x4101),
        ] {
            assert_eq!(
                wait.session
                    .bus
                    .read_physical(GuestPhysicalAddress::new(addr), AccessWidth::Qword)
                    .unwrap(),
                value
            );
        }
        assert_ne!(
            wait.session
                .bus
                .read_physical(
                    GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x120),
                    AccessWidth::Dword,
                )
                .unwrap()
                & (1 << 17),
            0
        );
        assert_ne!(
            wait.session
                .bus
                .read_physical(
                    GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                    AccessWidth::Dword,
                )
                .unwrap()
                & (1 << 14),
            0
        );
    }

    #[test]
    fn halt_wait_blocked_stop_and_delivery_abort() {
        let (mut session, io, apic) = halt_fixture(0x7000);
        session.state.rflags &= !0x200;
        let mut wait = enter_wait(session);
        io.set_intin4(true).unwrap();
        assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
        assert_eq!(apic.pending(), Some(0x51));
        wait.stop();
        assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Stopped);
        assert!(wait.into_running().is_err());

        let (mut session, io, apic) = halt_fixture(0x7000);
        session
            .bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Dword,
                0x50,
                |_| {},
            )
            .unwrap();
        let mut wait = enter_wait(session);
        io.set_intin4(true).unwrap();
        assert_eq!(wait.poll_apic(&apic), DiagnosticHaltWakeResult::Waiting);
        assert_eq!(apic.pending(), Some(0x51));

        let (session, io, apic) = halt_fixture(0x1020);
        let mut wait = enter_wait(session);
        let before = wait.state().clone();
        io.set_intin4(true).unwrap();
        let result = wait.poll_apic(&apic);
        assert!(matches!(
            result,
            DiagnosticHaltWakeResult::Terminal(
                DiagnosticTerminalExit::InterruptDeliveryAborted { .. }
            )
        ));
        assert_eq!(wait.state(), &before);
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(wait.poll_apic(&apic), result);
        assert!(wait.into_running().is_err());
    }

    #[test]
    fn guest_eoi_iretq_resumes_all_stack_alignments() {
        for low in 0..16 {
            let rsp = 0x7000 + low;
            let (mut session, io, apic) = irq_return_session(rsp);
            let before = session.state.clone();
            io.set_intin4(true).unwrap();
            io.set_intin4(false).unwrap();
            assert!(matches!(
                session.deliver_interrupt_and_step(&apic).unwrap(),
                DiagnosticInterruptResult::Delivered {
                    vector: 0x51,
                    handler: DiagnosticRunResult {
                        instructions_executed: 1,
                        ..
                    }
                }
            ));
            assert_eq!(session.state.rip, 0x4002);
            assert_eq!(apic.pending(), None);
            assert_eq!(
                session
                    .bus
                    .read_physical(
                        GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                        AccessWidth::Dword
                    )
                    .unwrap()
                    & (1 << 14),
                0
            );
            assert_eq!(
                session
                    .bus
                    .read_physical(
                        GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x120),
                        AccessWidth::Dword
                    )
                    .unwrap()
                    & (1 << 17),
                0
            );
            assert_eq!(
                session.run_same_cpl0_iretq().unwrap(),
                DiagnosticIretqResult::Retired
            );
            assert_eq!(session.state.rip, before.rip);
            assert_eq!(session.state.gpr[od_isa::Register::Rsp as usize], rsp);
            assert_eq!(session.state.cs_selector, before.cs_selector);
            assert_eq!(session.state.ss_selector, before.ss_selector);
            assert_eq!(session.state.rflags, before.rflags);
            assert_ne!(session.state.rflags & 0x200, 0);
            assert_eq!(run(&mut session, 1).instructions_executed, 1);
            assert_eq!(
                session.state.gpr[od_isa::Register::Rax as usize],
                0x1234_5678_9abc_def0
            );
            assert_eq!(session.state.rip, 0x4103);
        }
    }

    #[test]
    fn held_level_retriggers_once_and_waits_for_caller_boundary() {
        let (mut session, io, apic) = irq_return_session(0x7000);
        io.set_intin4(true).unwrap();
        assert!(matches!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::Delivered { .. }
        ));
        assert_eq!(apic.pending(), Some(0x51));
        assert_ne!(
            session
                .bus
                .read_physical(
                    GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                    AccessWidth::Dword
                )
                .unwrap()
                & (1 << 14),
            0
        );
        assert_eq!(
            session.run_same_cpl0_iretq().unwrap(),
            DiagnosticIretqResult::Retired
        );
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(run(&mut session, 1).instructions_executed, 1);
        assert_eq!(apic.pending(), Some(0x51));
        io.set_intin4(false).unwrap();
        // The next explicit boundary consumes the one pending request.
        session.state.gpr[od_isa::Register::Rax as usize] = 0;
        assert!(matches!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::Delivered { vector: 0x51, .. }
        ));
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn iretq_read_abort_after_guest_eoi_is_terminal_and_does_not_retry() {
        for phase in [
            IretqReadPhase::Fetch,
            IretqReadPhase::Frame,
            IretqReadPhase::CsDescriptor,
            IretqReadPhase::SsDescriptor,
        ] {
            let (mut session, io, apic) = irq_return_session(0x7000);
            io.set_intin4(true).unwrap();
            assert!(matches!(
                session.deliver_interrupt_and_step(&apic).unwrap(),
                DiagnosticInterruptResult::Delivered { .. }
            ));
            assert_eq!(apic.pending(), Some(0x51));
            match phase {
                IretqReadPhase::Fetch => session.state.rip = 0x9000,
                IretqReadPhase::Frame => session.state.gpr[od_isa::Register::Rsp as usize] = 0x9000,
                IretqReadPhase::CsDescriptor => session.state.gdtr.as_mut().unwrap().base = 0x9000,
                IretqReadPhase::SsDescriptor => session.state.gdtr.as_mut().unwrap().base = 0x7ff0,
            }
            let before = session.state.clone();
            assert!(
                matches!(session.run_same_cpl0_iretq().unwrap(), DiagnosticIretqResult::Terminal(DiagnosticTerminalExit::IretqReadAborted { phase: p, .. }) if p == phase)
            );
            assert_eq!(session.state, before);
            assert_eq!(apic.pending(), Some(0x51));
            assert!(matches!(
                session.run_same_cpl0_iretq(),
                Err(DiagnosticVcpuControlError::NotRunnable(_))
            ));
            assert!(matches!(
                session.run(1, || false),
                Err(DiagnosticVcpuControlError::NotRunnable(_))
            ));
            assert_eq!(apic.pending(), Some(0x51));
        }
    }

    #[test]
    fn irq_blocked_states_preserve_request_and_cpu() {
        let (mut session, io, apic) = irq_session(0x7000);
        let before = session.state.clone();
        assert_eq!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::NoDelivery
        );
        session
            .bus
            .write_physical(
                GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                AccessWidth::Dword,
                0x18051,
                |_| {},
            )
            .unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::NoDelivery
        );
        assert_eq!(session.state, before);
        assert_eq!(apic.pending(), None);
        io.set_intin4(false).unwrap();
        session
            .bus
            .write_physical(
                GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                AccessWidth::Dword,
                0x8051,
                |_| {},
            )
            .unwrap();
        io.set_intin4(true).unwrap();
        for case in 0..3 {
            session.state = before.clone();
            if case == 0 {
                session.state.rflags &= !0x200;
            }
            if case == 1 {
                session.state.sti_shadow = Some(od_isa::StiShadow::Blocking);
            }
            if case == 2 {
                session
                    .bus
                    .write_physical(
                        GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                        AccessWidth::Dword,
                        0x50,
                        |_| {},
                    )
                    .unwrap();
            }
            let snapshot = session.state.clone();
            assert_eq!(
                session.deliver_interrupt_and_step(&apic).unwrap(),
                DiagnosticInterruptResult::NoDelivery
            );
            assert_eq!(session.state, snapshot);
            assert_eq!(apic.pending(), Some(0x51));
        }
    }

    #[test]
    fn irq_aborted_frame_is_terminal_and_keeps_prefix() {
        let (mut session, io, apic) = irq_session(0x1020);
        let before = session.state.clone();
        io.set_intin4(true).unwrap();
        assert!(matches!(
            session.deliver_interrupt_and_step(&apic).unwrap(),
            DiagnosticInterruptResult::Terminal(
                DiagnosticTerminalExit::InterruptDeliveryAborted { .. }
            )
        ));
        assert_eq!(session.state, before);
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(
            session
                .bus
                .read_physical(GuestPhysicalAddress::new(0x1018), AccessWidth::Qword)
                .unwrap(),
            0x10
        );
        assert!(matches!(
            session.deliver_interrupt_and_step(&apic),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
        assert!(matches!(
            session.run(1, || false),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
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

    fn put(cpu: &mut DiagnosticVcpuSession, address: u64, bytes: &[u8]) {
        for (offset, byte) in bytes.iter().enumerate() {
            cpu.bus
                .write_physical(
                    GuestPhysicalAddress::new(address + offset as u64),
                    AccessWidth::Byte,
                    u64::from(*byte),
                    |_| {},
                )
                .unwrap();
        }
    }

    #[test]
    fn sti_apic_waits_through_cli_sti_and_shadow() {
        let (mut cpu, io, apic) = irq_session(0x7000);
        cpu.rflags_defined = INTEGER_FLAGS;
        cpu.state.rflags &= !0x200;
        put(&mut cpu, 0x4100, &[0xfa, 0xfb, 0x90]);
        put(&mut cpu, 0x4000, &[0xfa]);
        io.set_intin4(true).unwrap();
        for rip in [0x4101, 0x4102, 0x4103] {
            assert!(matches!(
                cpu.run_sti_apic_quantum(&apic),
                DiagnosticStiApicQuantumResult::Ordinary(DiagnosticRunResult {
                    instructions_executed: 1,
                    ..
                })
            ));
            assert_eq!(cpu.state.rip, rip);
            assert_eq!(apic.pending(), Some(0x51));
        }
        assert_eq!(cpu.state.sti_shadow, Some(od_isa::StiShadow::Clear));
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Delivered {
                vector: 0x51,
                handler: DiagnosticRunResult {
                    instructions_executed: 1,
                    ..
                }
            }
        ));
        assert_eq!(apic.pending(), None);
    }

    #[test]
    fn sti_apic_respects_tpr_and_restricted_handler() {
        let (mut cpu, io, apic) = irq_session(0x7000);
        cpu.rflags_defined = INTEGER_FLAGS;
        cpu.state.rflags &= !0x200;
        put(&mut cpu, 0x4100, &[0xfb]);
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Dword,
                0x60,
                |_| {},
            )
            .unwrap();
        io.set_intin4(true).unwrap();
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Ordinary(_)
        ));
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(apic.eligible(), None);
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Dword,
                0,
                |_| {},
            )
            .unwrap();
        put(&mut cpu, 0x4101, &[0x90]);
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Ordinary(_)
        ));
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Delivered {
                vector: 0x51,
                handler: DiagnosticRunResult {
                    exit: DiagnosticExit::Terminal(DiagnosticTerminalExit::Unsupported { .. }),
                    ..
                }
            }
        ));
        assert_eq!(apic.pending(), None);
        assert_eq!(cpu.state.rip, 0x4000);
        assert!(matches!(
            cpu.run(1, || false),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
    }

    #[test]
    fn sti_apic_uses_eligible_not_pending_when_if_is_set() {
        let (mut cpu, io, apic) = irq_session(0x7000);
        cpu.rflags_defined = INTEGER_FLAGS;
        put(&mut cpu, 0x4100, &[0xfb]);
        put(&mut cpu, 0x4000, &[0xfa]);
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Dword,
                0x60,
                |_| {},
            )
            .unwrap();
        io.set_intin4(true).unwrap();
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(apic.eligible(), None);
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Ordinary(DiagnosticRunResult {
                instructions_executed: 1,
                ..
            })
        ));
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                AccessWidth::Dword,
                0,
                |_| {},
            )
            .unwrap();
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Delivered { vector: 0x51, .. }
        ));
    }

    #[test]
    fn sti_apic_delivery_abort_preserves_pending_and_is_terminal() {
        let (mut cpu, io, apic) = irq_session(0x9000);
        cpu.rflags_defined = INTEGER_FLAGS;
        let before = cpu.state.clone();
        io.set_intin4(true).unwrap();
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Terminal(
                DiagnosticTerminalExit::InterruptDeliveryAborted { .. }
            )
        ));
        assert_eq!(cpu.state, before);
        assert_eq!(apic.pending(), Some(0x51));
        assert!(matches!(
            cpu.run(1, || false),
            Err(DiagnosticVcpuControlError::NotRunnable(_))
        ));
    }

    #[test]
    fn sti_apic_rejects_invalid_admission_without_mutation() {
        fn snapshot(bus: &mut MachineBus) -> Vec<u64> {
            let mut values = (0x1000..0x8000)
                .map(|address| {
                    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Byte)
                        .unwrap()
                })
                .collect::<Vec<_>>();
            for offset in [0x80, 0xf0, 0x110, 0x190, 0x210] {
                values.push(
                    bus.read_physical(
                        GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + offset),
                        AccessWidth::Dword,
                    )
                    .unwrap(),
                );
            }
            values.push(
                bus.read_physical(
                    GuestPhysicalAddress::new(crate::IO_APIC_MMIO_BASE + 0x10),
                    AccessWidth::Dword,
                )
                .unwrap(),
            );
            values
        }
        for case in 0..10 {
            let (mut cpu, io, apic) = irq_session(0x7000);
            cpu.rflags_defined = INTEGER_FLAGS;
            io.set_intin4(true).unwrap();
            match case {
                0 => cpu.rflags_defined = 0,
                1 => cpu.state.sti_shadow = None,
                2 => {
                    cpu.clock = Some(
                        VirtualClockHandle::from(VirtualClock::new(1).unwrap())
                            .claim_advancer()
                            .unwrap(),
                    )
                }
                3 => cpu.state.activity = None,
                4 => cpu.access_context.privilege = PrivilegeLevel::Ring3,
                5 => cpu.pause().unwrap(),
                6 => cpu.state.cr0 = Some(0),
                7 => cpu.state.cr3 = Some(0),
                8 => cpu.state.cr4 = Some(0),
                _ => {
                    cpu.state.sti_shadow = Some(od_isa::StiShadow::Blocking);
                    cpu.state.rflags &= !0x200;
                }
            }
            let before = cpu.state.clone();
            let status = cpu.status.clone();
            let pending = apic.pending();
            let eligible = apic.eligible();
            let rflags_defined = cpu.rflags_defined;
            let completed_boundary = cpu.completed_boundary;
            let memory_and_devices = snapshot(&mut cpu.bus);
            assert_eq!(
                cpu.run_sti_apic_quantum(&apic),
                DiagnosticStiApicQuantumResult::ProfileRejected
            );
            assert_eq!(cpu.state, before);
            assert_eq!(cpu.status, status);
            assert_eq!(apic.pending(), pending);
            assert_eq!(apic.eligible(), eligible);
            assert_eq!(cpu.rflags_defined, rflags_defined);
            assert_eq!(cpu.completed_boundary, completed_boundary);
            assert_eq!(snapshot(&mut cpu.bus), memory_and_devices, "case {case}");
        }
    }

    fn self_ipi(cpu: &mut DiagnosticVcpuSession, vector: u8) {
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x300),
                AccessWidth::Dword,
                0x4_4000 | u64::from(vector),
                |_| {},
            )
            .unwrap();
    }

    #[test]
    fn apic_quantum_matches_ordinary_run_with_flags_and_tsc() {
        let (mut a, _, apic) = irq_session(0x7000);
        let (mut b, _, _) = irq_session(0x7000);
        for cpu in [&mut a, &mut b] {
            put(cpu, 0x4100, &[0x0f, 0x31, 0x90]);
            cpu.rflags_defined = 0x40;
            cpu.clock = Some(
                VirtualClockHandle::from(VirtualClock::new(1_000_000_000).unwrap())
                    .claim_advancer()
                    .unwrap(),
            );
            cpu.rdtsc_controls = Some(RdtscControls { cr4_tsd: false });
            cpu.advance_time_ns(1234).unwrap();
        }
        for _ in 0..2 {
            let expected = b.run(1, || false).unwrap();
            assert_eq!(
                a.run_apic_quantum(&apic, || false).unwrap(),
                DiagnosticApicQuantumResult::Ordinary(expected)
            );
            assert_eq!(a.state, b.state);
            assert_eq!(a.rflags_defined, b.rflags_defined);
        }
        assert_eq!(a.state.gpr[od_isa::Register::Rax as usize], 1234);
    }

    #[test]
    fn apic_quantum_blocks_if_and_tpr_and_rejects_invalid_boundaries() {
        for tpr in [false, true] {
            let (mut cpu, _, apic) = irq_session(0x7000);
            put(&mut cpu, 0x4100, &[0x90]);
            self_ipi(&mut cpu, 0x61);
            if tpr {
                cpu.bus
                    .write_physical(
                        GuestPhysicalAddress::new(crate::LOCAL_APIC_MMIO_BASE + 0x80),
                        AccessWidth::Dword,
                        0x70,
                        |_| {},
                    )
                    .unwrap();
            } else {
                cpu.state.rflags &= !0x200;
            }
            assert!(matches!(
                cpu.run_apic_quantum(&apic, || false).unwrap(),
                DiagnosticApicQuantumResult::Ordinary(DiagnosticRunResult {
                    instructions_executed: 1,
                    ..
                })
            ));
            assert_eq!(cpu.state.rip, 0x4101);
            assert_eq!(apic.pending(), Some(0x61));
        }
        for case in 0..9 {
            let (mut cpu, _, apic) = irq_session(0x7000);
            self_ipi(&mut cpu, 0x61);
            match case {
                0 => cpu.state.sti_shadow = None,
                1 => cpu.state.sti_shadow = Some(od_isa::StiShadow::Blocking),
                2 => cpu.state.activity = Some(od_isa::CpuActivity::Halted),
                3 => cpu.completed_boundary = false,
                4 => cpu.pause().unwrap(),
                5 => cpu.stop(),
                6 => {
                    cpu.terminal(DiagnosticTerminalExit::StopAddress);
                }
                7 => cpu.state.cr3 = Some(0),
                _ => cpu.access_context.privilege = PrivilegeLevel::Ring3,
            }
            let before = cpu.state.clone();
            let status = cpu.status.clone();
            assert_eq!(
                cpu.run_apic_quantum(&apic, || panic!("rejected callback"))
                    .unwrap(),
                DiagnosticApicQuantumResult::ProfileRejected
            );
            assert_eq!(cpu.state, before);
            assert_eq!(cpu.status, status);
            assert_eq!(apic.pending(), Some(0x61));
        }
    }

    #[test]
    fn apic_quantum_stop_and_profile_failure_do_not_execute() {
        let (mut cpu, _, apic) = irq_session(0x7000);
        self_ipi(&mut cpu, 0x61);
        cpu.state.rip = 0x9000;
        let before = cpu.state.clone();
        assert_eq!(
            cpu.run_apic_quantum(&apic, || true).unwrap(),
            DiagnosticApicQuantumResult::Stopped
        );
        assert_eq!(cpu.state, before);
        assert_eq!(cpu.status, DiagnosticVcpuState::Stopped);
        assert_eq!(apic.pending(), Some(0x61));
        assert_eq!(
            cpu.run_apic_quantum(&apic, || panic!("stopped callback"))
                .unwrap(),
            DiagnosticApicQuantumResult::ProfileRejected
        );
        assert_eq!(cpu.state, before);
        assert_eq!(cpu.status, DiagnosticVcpuState::Stopped);
        assert_eq!(apic.pending(), Some(0x61));

        let (mut cpu, _, apic) = irq_session(0x7000);
        self_ipi(&mut cpu, 0x61);
        cpu.state.idtr = None;
        let before = cpu.state.clone();
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::OutsideProfile(_)
        ));
        assert_eq!(cpu.state, before);
        assert_eq!(apic.pending(), Some(0x61));
    }

    #[test]
    fn apic_quantum_pending_during_handler_then_delivers_before_resume() {
        let (mut cpu, _, apic) = irq_return_session(0x7000);
        put(&mut cpu, 0x4000, &[0x90, 0x89, 0x07, 0x48, 0xcf]);
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(0x3000 + 0x61 * 16),
                AccessWidth::Qword,
                0x8e00_0020_4000,
                |_| {},
            )
            .unwrap();
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(0x3008 + 0x61 * 16),
                AccessWidth::Qword,
                0,
                |_| {},
            )
            .unwrap();
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(0x3000 + 0x62 * 16),
                AccessWidth::Qword,
                0x8e00_0020_4000,
                |_| {},
            )
            .unwrap();
        cpu.bus
            .write_physical(
                GuestPhysicalAddress::new(0x3008 + 0x62 * 16),
                AccessWidth::Qword,
                0,
                |_| {},
            )
            .unwrap();
        self_ipi(&mut cpu, 0x61);
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Delivered { vector: 0x61, .. }
        ));
        for (addr, value) in [
            (0x6ff8, 0x10),
            (0x6ff0, 0x7000),
            (0x6fe8, 0x202),
            (0x6fe0, 8),
            (0x6fd8, 0x4100),
        ] {
            assert_eq!(
                cpu.bus
                    .read_physical(GuestPhysicalAddress::new(addr), AccessWidth::Qword)
                    .unwrap(),
                value
            );
        }
        self_ipi(&mut cpu, 0x62);
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Ordinary(_)
        ));
        assert_eq!(apic.pending(), Some(0x62));
        assert_eq!(
            cpu.run_same_cpl0_iretq().unwrap(),
            DiagnosticIretqResult::Retired
        );
        assert_eq!(cpu.state.rip, 0x4100);
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Delivered { vector: 0x62, .. }
        ));
        assert_eq!(cpu.state.rip, 0x4001);
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Ordinary(_)
        ));
        assert_eq!(
            cpu.run_same_cpl0_iretq().unwrap(),
            DiagnosticIretqResult::Retired
        );
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Ordinary(_)
        ));
        assert_eq!(cpu.state.gpr[0], 0x1234_5678_9abc_def0);
    }

    #[test]
    fn apic_quantum_handler_failure_and_stop_keep_entry() {
        for case in 0..3 {
            let (mut cpu, io, apic) = irq_session(0x7000);
            if case == 0 {
                put(&mut cpu, 0x4000, &[0x0f, 0x0b]);
            }
            if case == 1 {
                cpu.state.gpr[od_isa::Register::Rbx as usize] = 0x9000;
            }
            io.set_intin4(true).unwrap();
            let mut calls = 0;
            let result = cpu
                .run_apic_quantum(&apic, || {
                    calls += 1;
                    case == 2 && calls > 1
                })
                .unwrap();
            assert!(matches!(
                result,
                DiagnosticApicQuantumResult::Delivered {
                    vector: 0x51,
                    handler: DiagnosticRunResult {
                        exit: DiagnosticExit::Terminal(_) | DiagnosticExit::Stopped,
                        ..
                    }
                }
            ));
            assert_eq!(cpu.state.rip, 0x4000);
            assert_eq!(cpu.state.gpr[od_isa::Register::Rsp as usize], 0x6fd8);
            assert_eq!(apic.pending(), None);
        }
    }

    #[test]
    fn apic_quantum_delivery_abort_does_not_acknowledge_or_retry() {
        let (mut cpu, io, apic) = irq_session(0x9000);
        let before = cpu.state.clone();
        io.set_intin4(true).unwrap();
        assert!(matches!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::Terminal(
                DiagnosticTerminalExit::InterruptDeliveryAborted { .. }
            )
        ));
        assert_eq!(cpu.state, before);
        assert_eq!(apic.pending(), Some(0x51));
        assert_eq!(
            cpu.run_apic_quantum(&apic, || false).unwrap(),
            DiagnosticApicQuantumResult::ProfileRejected
        );
        assert_eq!(apic.pending(), Some(0x51));
    }

    #[test]
    fn apic_quantum_rep_partial_blocks_next_entry() {
        for handler in [false, true] {
            let (mut cpu, io, apic) = irq_session(0x7000);
            put(
                &mut cpu,
                if handler { 0x4000 } else { 0x4100 },
                &[0xf3, 0xaa],
            );
            cpu.state.gpr[od_isa::Register::Rcx as usize] = 1025;
            cpu.state.gpr[od_isa::Register::Rdi as usize] = 0x5000;
            if handler {
                io.set_intin4(true).unwrap();
            }
            let result = cpu.run_apic_quantum(&apic, || false).unwrap();
            let result = match result {
                DiagnosticApicQuantumResult::Ordinary(r) => r,
                DiagnosticApicQuantumResult::Delivered { handler: r, .. } => r,
                other => panic!("{other:?}"),
            };
            assert_eq!(
                result.exit,
                DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget)
            );
            assert!(!cpu.completed_boundary);
            let before = cpu.state.clone();
            assert_eq!(
                cpu.run_apic_quantum(&apic, || false).unwrap(),
                DiagnosticApicQuantumResult::ProfileRejected
            );
            assert_eq!(cpu.state, before);
        }
    }

    #[test]
    fn rep_partial_prevents_apic_entry_until_normal_retirement() {
        let (mut cpu, io, apic) = irq_session(0x7000);
        cpu.rflags_defined = INTEGER_FLAGS;
        put(&mut cpu, 0x4100, &[0xf3, 0xaa]);
        cpu.state.gpr[od_isa::Register::Rcx as usize] = 1025;
        cpu.state.gpr[od_isa::Register::Rdi as usize] = 0x5000;
        let partial = run(&mut cpu, 1);
        assert_eq!(
            partial.exit,
            DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget)
        );
        assert!(!cpu.completed_boundary);
        io.set_intin4(true).unwrap();
        let before = cpu.state.clone();
        assert_eq!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::ProfileRejected
        );
        assert!(matches!(
            cpu.deliver_interrupt_and_step(&apic),
            Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported)
        ));
        assert!(matches!(
            cpu.run_same_cpl0_iretq(),
            Err(DiagnosticVcpuControlError::InterruptBoundaryUnsupported)
        ));
        assert_eq!(cpu.state, before);
        assert_eq!(apic.pending(), Some(0x51));
        let failure = match cpu.into_halt_wait() {
            Err(failure) => failure,
            Ok(_) => panic!("partial REP admitted to HLT wait"),
        };
        assert_eq!(failure.reason, DiagnosticHaltEnterReason::ProfileRejected);
        let mut cpu = failure.session;
        assert_eq!(cpu.state, before);
        assert!(!cpu.completed_boundary);
        assert_eq!(run(&mut cpu, 1).instructions_executed, 1);
        assert!(cpu.completed_boundary);
        assert!(matches!(
            cpu.run_sti_apic_quantum(&apic),
            DiagnosticStiApicQuantumResult::Delivered { .. }
        ));
    }
}
