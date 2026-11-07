//! Synchronous, bounded diagnostic execution over one owned CPU and machine.

use crate::{IdentityMemoryAdapter, LocalApicHandle, MachineBus};
use crate::{VirtualClock, VirtualClockError};
use od_isa::execute::{
    RdtscControls, RunConfig, RunExit, run_with_rflags_defined,
    run_with_rflags_defined_and_rdtsc_source,
};
use od_isa::interrupt::{
    InterruptDeliveryPhase, InterruptDeliveryResult, InterruptOutsideProfile,
    deliver_maskable_interrupt,
};
use od_isa::iretq::{IretqProfile, IretqReadPhase, IretqRunExit, run_with_iretq_profile};
use od_isa::memory::{AccessContext, ArchitecturalFault, MemoryError};
use od_isa::{INTEGER_FLAGS, LongModeState};

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

    /// Attempt one same-CPL0 IRETQ. The caller certifies private synchronous
    /// RAM and the remaining prerequisites of the ISA IRETQ profile.
    pub fn run_same_cpl0_iretq(
        &mut self,
    ) -> Result<DiagnosticIretqResult, DiagnosticVcpuControlError> {
        if self.status != DiagnosticVcpuState::Runnable {
            return Err(DiagnosticVcpuControlError::NotRunnable(self.status.clone()));
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

    fn irq_session(rsp: u64) -> (DiagnosticVcpuSession, crate::IoApicHandle, LocalApicHandle) {
        use crate::{IO_APIC_MMIO_BASE, IoApic, LOCAL_APIC_MMIO_BASE, LocalApic, MmioBinding};
        let local = LocalApic::new();
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
        (
            DiagnosticVcpuSession::new(state, bus, context(), 0).unwrap(),
            io_handle,
            apic,
        )
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
}
