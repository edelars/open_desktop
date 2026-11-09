//! S4-27i1: flat event pump over the accepted diagnostic vCPU primitives.
//!
//! `InterruptPump` owns one flat Long64/CPL0 [`DiagnosticVcpuSession`] (or its
//! [`DiagnosticHaltWait`] after HLT) and the [`LocalApicHandle`] it polls. It is
//! a synchronous owner loop without a thread, clock, timer or event queue:
//! interrupt requests come from the caller (through the APIC handle) or from
//! guest MMIO (for example a self-IPI), and `run(budget, stop)` examines each
//! instruction boundary exactly once, before fetch.
//!
//! Boundary order: STOP, then the budget, then (Running only) the APIC check
//! (IF=1, STI shadow Clear, completed boundary, `eligible()`), then either IRQ
//! delivery through `run_apic_quantum` or one instruction. HLT never goes
//! through `run(1)` (the ordinary interpreter has no HLT: terminal): it enters the halt wait,
//! and the wait is left only by a wake through `poll_apic`. STI and CLI go
//! through `run_sti_apic_quantum` (the ordinary interpreter does not execute
//! them), as does every instruction in the STI shadow; IRETQ goes through
//! `run_same_cpl0_iretq`. The effects of a step are seen by the next boundary
//! check; there is no separate check after an instruction.
//!
//! Accepted APIs are used as they are. Consequences recorded as limits:
//! the first handler instruction runs inside `run_apic_quantum` and must be an
//! instruction the ordinary interpreter executes (not STI, CLI, HLT, IRETQ); a
//! partial REP (iteration budget exhausted) leaves the boundary incomplete, so
//! the pump finishes the instruction before the next APIC check (interrupt
//! latency deviates from hardware, which may deliver between iterations).

use crate::diagnostic_vcpu::DiagnosticHaltWait;
use crate::{
    DiagnosticApicQuantumResult, DiagnosticExit, DiagnosticHaltEnterReason,
    DiagnosticHaltWakeResult, DiagnosticIretqResult, DiagnosticRunResult,
    DiagnosticStiApicQuantumResult, DiagnosticTerminalExit, DiagnosticVcpuControlError,
    DiagnosticVcpuSession, LocalApicHandle, WorkLimitReason,
};
use od_isa::StiShadow;
use od_isa::interrupt::InterruptOutsideProfile;

/// Tracing bound; exceeding it ends the run instead of growing without limit.
pub const INTERRUPT_PUMP_TRACE_LIMIT: usize = 512;

const RFLAGS_IF: u64 = 1 << 9;

/// Deliberate fault injection for regression evidence. Never used by the
/// accepted configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpInjection {
    /// The boundary check ignores the STI shadow, so the pump tries to deliver
    /// at a boundary where the CPU must not accept an interrupt. The accepted
    /// primitive refuses (`ProfileRejected`), which surfaces as `Rejected`.
    SkipShadowCheck,
    /// HLT is executed as an ordinary instruction (`run(1)`). The ordinary
    /// interpreter has no HLT, so this is a terminal result (`Unsupported`)
    /// instead of a halt wait.
    HltAsOrdinary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpEntrySource {
    /// Entered at an instruction boundary of a running CPU.
    Boundary,
    /// Entered from the halt wait; the saved RIP is the instruction after HLT.
    HaltWake,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PumpTraceKind {
    /// A watched vector appeared in the Local APIC IRR (observed at a boundary).
    Irr {
        vector: u8,
    },
    /// The CPU accepted the vector: IRR cleared, ISR set, frame pushed.
    Entry {
        vector: u8,
        source: PumpEntrySource,
    },
    /// A watched ISR bit was cleared (the guest wrote the EOI).
    Eoi {
        vector: u8,
    },
    Sti,
    Cli,
    /// HLT retired and the CPU entered the halt wait.
    Hlt,
    Iretq,
}

impl PumpTraceKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Irr { .. } => "irr",
            Self::Entry { .. } => "entry",
            Self::Eoi { .. } => "eoi",
            Self::Sti => "sti",
            Self::Cli => "cli",
            Self::Hlt => "hlt",
            Self::Iretq => "iretq",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PumpTraceEvent {
    pub seq: u64,
    /// RIP of the CPU when the event was observed.
    pub rip: u64,
    pub kind: PumpTraceKind,
}

/// A refusal that leaves guest state unchanged by the failed step.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PumpRejection {
    /// `run_apic_quantum` refused the profile or the boundary.
    ApicQuantumProfile,
    /// `run_sti_apic_quantum` refused the profile or the boundary.
    StiQuantumProfile,
    /// The interrupt-entry profile refused the delivery.
    InterruptEntry(InterruptOutsideProfile),
    HaltEntry(DiagnosticHaltEnterReason),
    HaltWake(InterruptOutsideProfile),
    Control(DiagnosticVcpuControlError),
    /// A step neither retired an instruction nor reported a known limit.
    NoRetirement(WorkLimitReason),
    /// The `INTERRUPT_PUMP_TRACE_LIMIT` bound was reached.
    TraceLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PumpExit {
    /// The CPU is halted and no maskable vector can wake it now. Not terminal:
    /// a caller request followed by another `run` resumes it.
    IdleHalt,
    Budget,
    Stopped,
    Terminal(DiagnosticTerminalExit),
    Rejected(PumpRejection),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PumpResult {
    pub exit: PumpExit,
    /// Pump steps taken by this call (instruction, delivery, HLT entry or wake).
    pub quanta: u64,
    /// Guest instructions retired by this call.
    pub retired: u64,
}

enum Cpu {
    Running(DiagnosticVcpuSession),
    Waiting(DiagnosticHaltWait),
}

enum Flow {
    Continue,
    Exit(PumpExit),
}

pub struct InterruptPump {
    cpu: Option<Cpu>,
    apic: LocalApicHandle,
    watched: Vec<(u8, (bool, bool))>,
    trace: Vec<PumpTraceEvent>,
    retired_total: u64,
    /// The last instruction stopped inside a REP: the boundary is incomplete.
    mid_rep: bool,
    injection: Option<PumpInjection>,
    finished: Option<PumpExit>,
}

impl InterruptPump {
    /// Take ownership of a runnable session at a completed instruction
    /// boundary and the APIC it is polled against. The caller certifies the
    /// prerequisites of the accepted primitives (one vCPU, private synchronous
    /// RAM, flat Long64/CPL0, no pending synchronous/debug/NMI events).
    pub fn new(session: DiagnosticVcpuSession, apic: LocalApicHandle) -> Self {
        Self {
            cpu: Some(Cpu::Running(session)),
            apic,
            watched: Vec::new(),
            trace: Vec::new(),
            retired_total: 0,
            mid_rep: false,
            injection: None,
            finished: None,
        }
    }

    /// Vectors whose IRR/ISR transitions are recorded in the trace.
    pub fn with_watched_vectors(mut self, vectors: &[u8]) -> Self {
        self.watched = vectors
            .iter()
            .map(|vector| (*vector, self.apic.vector_state(*vector)))
            .collect();
        self
    }

    pub fn with_injection(mut self, injection: PumpInjection) -> Self {
        self.injection = Some(injection);
        self
    }

    pub fn state(&self) -> &od_isa::LongModeState {
        match self.cpu.as_ref().expect("cpu is always restored") {
            Cpu::Running(session) => session.state(),
            Cpu::Waiting(wait) => wait.state(),
        }
    }

    pub fn is_waiting(&self) -> bool {
        matches!(self.cpu, Some(Cpu::Waiting(_)))
    }

    /// The Local APIC the pump polls. Callers raise requests through it.
    pub fn apic(&self) -> &LocalApicHandle {
        &self.apic
    }

    pub fn trace(&self) -> &[PumpTraceEvent] {
        &self.trace
    }

    pub fn retired_total(&self) -> u64 {
        self.retired_total
    }

    /// Read one qword of guest RAM without side effects (evidence only).
    pub fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        match self.cpu.as_mut().expect("cpu is always restored") {
            Cpu::Running(session) => session.peek_ram_qword(gpa),
            Cpu::Waiting(wait) => wait.peek_ram_qword(gpa),
        }
    }

    /// Run until idle halt, budget, STOP, a terminal result or a rejection.
    /// Terminal, Stopped and Rejected are final; IdleHalt and Budget resume.
    pub fn run(&mut self, budget: u64, mut stop: impl FnMut() -> bool) -> PumpResult {
        let retired_before = self.retired_total;
        let mut quanta = 0;
        let exit = loop {
            if let Some(exit) = &self.finished {
                break exit.clone();
            }
            if stop() {
                self.stop_cpu();
                self.finished = Some(PumpExit::Stopped);
                break PumpExit::Stopped;
            }
            if quanta >= budget {
                break PumpExit::Budget;
            }
            match self.step() {
                Flow::Continue => quanta += 1,
                Flow::Exit(exit) => {
                    if !matches!(exit, PumpExit::IdleHalt) {
                        self.finished = Some(exit.clone());
                    }
                    break exit;
                }
            }
        };
        PumpResult {
            exit,
            quanta,
            retired: self.retired_total - retired_before,
        }
    }

    fn stop_cpu(&mut self) {
        match self.cpu.as_mut().expect("cpu is always restored") {
            Cpu::Running(session) => session.stop(),
            Cpu::Waiting(wait) => wait.stop(),
        }
    }

    fn push(&mut self, kind: PumpTraceKind) -> Result<(), PumpExit> {
        if self.trace.len() >= INTERRUPT_PUMP_TRACE_LIMIT {
            return Err(PumpExit::Rejected(PumpRejection::TraceLimit));
        }
        let rip = self.state().rip;
        self.trace.push(PumpTraceEvent {
            seq: self.trace.len() as u64,
            rip,
            kind,
        });
        Ok(())
    }

    /// Record IRR rises and ISR clears of watched vectors since the last call.
    /// Entries are recorded explicitly from the delivery result instead.
    fn observe(&mut self) -> Result<(), PumpExit> {
        for index in 0..self.watched.len() {
            let (vector, (prev_irr, prev_isr)) = self.watched[index];
            let (irr, isr) = self.apic.vector_state(vector);
            self.watched[index].1 = (irr, isr);
            if prev_isr && !isr {
                self.push(PumpTraceKind::Eoi { vector })?;
            }
            if !prev_irr && irr {
                self.push(PumpTraceKind::Irr { vector })?;
            }
        }
        Ok(())
    }

    fn step(&mut self) -> Flow {
        match self.step_inner() {
            Ok(flow) => flow,
            Err(exit) => Flow::Exit(exit),
        }
    }

    fn step_inner(&mut self) -> Result<Flow, PumpExit> {
        self.observe()?;
        let cpu = self.cpu.take().expect("cpu is always restored");
        let flow = match cpu {
            Cpu::Waiting(wait) => self.step_waiting(wait)?,
            Cpu::Running(session) => self.step_running(session)?,
        };
        self.observe()?;
        Ok(flow)
    }

    fn terminal(cause: DiagnosticTerminalExit) -> Flow {
        Flow::Exit(PumpExit::Terminal(cause))
    }

    fn reject(rejection: PumpRejection) -> Flow {
        Flow::Exit(PumpExit::Rejected(rejection))
    }

    fn step_waiting(&mut self, mut wait: DiagnosticHaltWait) -> Result<Flow, PumpExit> {
        match wait.poll_apic(&self.apic) {
            DiagnosticHaltWakeResult::Entered { vector } => match wait.into_running() {
                Ok(session) => {
                    self.cpu = Some(Cpu::Running(session));
                    self.push(PumpTraceKind::Entry {
                        vector,
                        source: PumpEntrySource::HaltWake,
                    })?;
                    Ok(Flow::Continue)
                }
                Err(wait) => {
                    self.cpu = Some(Cpu::Waiting(wait));
                    Ok(Self::reject(PumpRejection::HaltEntry(
                        DiagnosticHaltEnterReason::ProfileRejected,
                    )))
                }
            },
            DiagnosticHaltWakeResult::Waiting => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Flow::Exit(PumpExit::IdleHalt))
            }
            DiagnosticHaltWakeResult::OutsideProfile(reason) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Self::reject(PumpRejection::HaltWake(reason)))
            }
            DiagnosticHaltWakeResult::Stopped => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Flow::Exit(PumpExit::Stopped))
            }
            DiagnosticHaltWakeResult::Terminal(cause) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Self::terminal(cause))
            }
        }
    }

    /// Account one run result; `Some` ends the call.
    fn account(&mut self, result: &DiagnosticRunResult) -> Option<Flow> {
        self.retired_total += result.instructions_executed;
        match &result.exit {
            DiagnosticExit::Terminal(cause) => Some(Self::terminal(cause.clone())),
            DiagnosticExit::Stopped => Some(Flow::Exit(PumpExit::Stopped)),
            DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget) => {
                self.mid_rep = true;
                None
            }
            DiagnosticExit::WorkLimit(_) if result.instructions_executed == 1 => {
                self.mid_rep = false;
                None
            }
            DiagnosticExit::WorkLimit(reason) => {
                Some(Self::reject(PumpRejection::NoRetirement(*reason)))
            }
        }
    }

    fn boundary_allows_delivery(&self, session: &DiagnosticVcpuSession) -> bool {
        let state = session.state();
        let shadow_clear = state.sti_shadow == Some(StiShadow::Clear)
            || (self.injection == Some(PumpInjection::SkipShadowCheck)
                && state.sti_shadow == Some(StiShadow::Blocking));
        !self.mid_rep
            && state.rflags & RFLAGS_IF != 0
            && shadow_clear
            && self.apic.eligible().is_some()
    }

    fn step_running(&mut self, mut session: DiagnosticVcpuSession) -> Result<Flow, PumpExit> {
        // A partial REP leaves an incomplete boundary: finish the instruction
        // without an APIC check (documented latency deviation).
        if self.mid_rep {
            let outcome = session.run(1, || false);
            return self.finish_plain(session, outcome);
        }
        if self.boundary_allows_delivery(&session) {
            let outcome = session.run_apic_quantum(&self.apic, || false);
            return self.finish_apic_quantum(session, outcome);
        }
        let code = session.peek_code(2);
        let in_sti_shadow = session.state().sti_shadow == Some(StiShadow::Blocking);
        match code.as_slice() {
            [0xf4, ..] => self.enter_halt(session),
            [0xfa | 0xfb, ..] => {
                let kind = if code[0] == 0xfb {
                    PumpTraceKind::Sti
                } else {
                    PumpTraceKind::Cli
                };
                self.step_sti_quantum(session, Some(kind))
            }
            [0x48, 0xcf, ..] => match session.run_same_cpl0_iretq() {
                Ok(DiagnosticIretqResult::Retired) => {
                    self.retired_total += 1;
                    self.cpu = Some(Cpu::Running(session));
                    self.push(PumpTraceKind::Iretq)?;
                    Ok(Flow::Continue)
                }
                Ok(DiagnosticIretqResult::Terminal(cause)) => {
                    self.cpu = Some(Cpu::Running(session));
                    Ok(Self::terminal(cause))
                }
                Err(error) => {
                    self.cpu = Some(Cpu::Running(session));
                    Ok(Self::reject(PumpRejection::Control(error)))
                }
            },
            _ if in_sti_shadow => self.step_sti_quantum(session, None),
            _ => {
                let outcome = session.run_apic_quantum(&self.apic, || false);
                self.finish_apic_quantum(session, outcome)
            }
        }
    }

    fn finish_plain(
        &mut self,
        session: DiagnosticVcpuSession,
        outcome: Result<DiagnosticRunResult, DiagnosticVcpuControlError>,
    ) -> Result<Flow, PumpExit> {
        self.cpu = Some(Cpu::Running(session));
        match outcome {
            Ok(result) => Ok(self.account(&result).unwrap_or(Flow::Continue)),
            Err(error) => Ok(Self::reject(PumpRejection::Control(error))),
        }
    }

    fn finish_apic_quantum(
        &mut self,
        session: DiagnosticVcpuSession,
        outcome: Result<DiagnosticApicQuantumResult, DiagnosticVcpuControlError>,
    ) -> Result<Flow, PumpExit> {
        self.cpu = Some(Cpu::Running(session));
        match outcome {
            Ok(DiagnosticApicQuantumResult::Ordinary(result)) => {
                Ok(self.account(&result).unwrap_or(Flow::Continue))
            }
            Ok(DiagnosticApicQuantumResult::Delivered { vector, handler }) => {
                self.push(PumpTraceKind::Entry {
                    vector,
                    source: PumpEntrySource::Boundary,
                })?;
                Ok(self.account(&handler).unwrap_or(Flow::Continue))
            }
            Ok(DiagnosticApicQuantumResult::ProfileRejected) => {
                Ok(Self::reject(PumpRejection::ApicQuantumProfile))
            }
            Ok(DiagnosticApicQuantumResult::OutsideProfile(reason)) => {
                Ok(Self::reject(PumpRejection::InterruptEntry(reason)))
            }
            Ok(DiagnosticApicQuantumResult::Stopped) => Ok(Flow::Exit(PumpExit::Stopped)),
            Ok(DiagnosticApicQuantumResult::Terminal(cause)) => Ok(Self::terminal(cause)),
            Err(error) => Ok(Self::reject(PumpRejection::Control(error))),
        }
    }

    fn step_sti_quantum(
        &mut self,
        mut session: DiagnosticVcpuSession,
        event: Option<PumpTraceKind>,
    ) -> Result<Flow, PumpExit> {
        let outcome = session.run_sti_apic_quantum(&self.apic);
        self.cpu = Some(Cpu::Running(session));
        match outcome {
            DiagnosticStiApicQuantumResult::Ordinary(result) => {
                if let Some(flow) = self.account(&result) {
                    return Ok(flow);
                }
                if let Some(kind) = event {
                    self.push(kind)?;
                }
                Ok(Flow::Continue)
            }
            DiagnosticStiApicQuantumResult::TerminalExecution(result) => {
                Ok(self.account(&result).unwrap_or(Flow::Continue))
            }
            DiagnosticStiApicQuantumResult::Delivered { vector, handler } => {
                self.push(PumpTraceKind::Entry {
                    vector,
                    source: PumpEntrySource::Boundary,
                })?;
                Ok(self.account(&handler).unwrap_or(Flow::Continue))
            }
            DiagnosticStiApicQuantumResult::ProfileRejected => {
                Ok(Self::reject(PumpRejection::StiQuantumProfile))
            }
            DiagnosticStiApicQuantumResult::OutsideProfile(reason) => {
                Ok(Self::reject(PumpRejection::InterruptEntry(reason)))
            }
            DiagnosticStiApicQuantumResult::Terminal(cause) => Ok(Self::terminal(cause)),
        }
    }

    fn enter_halt(&mut self, mut session: DiagnosticVcpuSession) -> Result<Flow, PumpExit> {
        if self.injection == Some(PumpInjection::HltAsOrdinary) {
            let outcome = session.run(1, || false);
            return self.finish_plain(session, outcome);
        }
        match session.into_halt_wait() {
            Ok(wait) => {
                self.cpu = Some(Cpu::Waiting(wait));
                self.retired_total += 1;
                self.push(PumpTraceKind::Hlt)?;
                Ok(Flow::Continue)
            }
            Err(failure) => {
                self.cpu = Some(Cpu::Running(failure.session));
                Ok(Self::reject(PumpRejection::HaltEntry(failure.reason)))
            }
        }
    }
}
