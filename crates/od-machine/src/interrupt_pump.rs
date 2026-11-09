//! S4-27i1: flat event pump over the accepted diagnostic vCPU primitives.
//!
//! `InterruptPump` owns one flat Long64/CPL0 [`DiagnosticVcpuSession`] (or its
//! [`DiagnosticHaltWait`] after HLT) and the [`LocalApicHandle`] it polls. It is
//! a synchronous owner loop without a thread or event queue:
//! interrupt requests come from the caller (through the APIC handle) or from
//! guest MMIO (for example a self-IPI), and `run(budget, stop)` examines each
//! instruction boundary exactly once, before fetch.
//!
//! S4-27i2 (owner-driven timer): when the APIC has a shared-clock timer, the
//! pump calls `LocalApicHandle::dispatch_timer` on every boundary (Running and
//! Waiting). In Waiting with no eligible vector it advances the session clock
//! to `next_timer_deadline()` and dispatches; no deadline (or IF=0, where no
//! maskable vector can wake the CPU) is `IdleHalt` without advancing time.
//! Time per instruction is 0 ns: the clock moves only by that idle advance
//! (or, outside the pump, by the owner of the session).
//!
//! S4-27i3 (IRQ event queue): the pump owns one [`EventQueue`] of fixed-vector
//! IRQ events. The caller schedules `(key, vector, due_ns)`; the owner takes due
//! events on every boundary: `peek_due`, `request_fixed_edge`, and `remove_due`
//! only after the request succeeded, so a failed request keeps the event
//! (`PumpRejection::QueueRequest`, resumable). A due event whose vector is
//! already in the IRR merges into that edge (trace `Coalesced`). `reset_irq_queue`
//! starts a new generation: tokens of the old generation are refused. In Waiting
//! the idle advance goes to the earlier of the timer deadline and the queue
//! deadline, but only for sources that can still change the IRR: a source whose
//! vector is already in the IRR while nothing is eligible (blocked by ISR/TPR)
//! cannot wake the CPU, so it is not waited for (ISSUE-070: `IdleHalt` instead of
//! burning the budget).
//!
//! Boundary order: STOP, then the budget, then timer and queue dispatch, then (Running only) the APIC check
//! (IF=1, STI shadow Clear, completed boundary, `eligible()`), then either IRQ
//! delivery through `run_apic_quantum` or one instruction. HLT never goes
//! through `run(1)` (the ordinary interpreter has no HLT: terminal): it enters the halt wait,
//! and the wait is left only by a wake through `poll_apic`. STI and CLI go
//! through `run_sti_apic_quantum` (the ordinary interpreter does not execute
//! them), as does every instruction in the STI shadow; IRETQ goes through
//! `run_same_cpl0_iretq`. The effects of a step are seen by the next boundary
//! check; there is no separate check after an instruction.
//!
//! S4-27i5 (owner hook): an owner can attach a [`PumpHost`] (the S4-24e UART
//! probe does). The host runs owner-side work at the start of every step
//! (`dispatch`, for example a device event queue and the device-to-IOAPIC line),
//! executes instructions the ordinary interpreter does not (`step`, for example
//! port I/O) and may add a deadline source for the idle advance (`next_deadline`;
//! it is waited for even with IF=0, because the host's work may itself raise an
//! IRR bit). Host events appear in the pump trace as [`PumpTraceKind::Host`].
//! With [`InterruptPump::with_entry_only_delivery`] a boundary delivery only
//! enters the handler (`deliver_pending_irq`): the first handler instruction is
//! a separate step, so the handler entry state is observable and the first
//! handler instruction is not limited to ordinary instructions. Without a host
//! and without that flag the pump behaves exactly as in S4-27i1..i3.
//!
//! S4-27i4b (backend trait): the CPU side of the pump is the [`PumpBackend`]
//! trait. The pump keeps the owner loop (STOP, budget, timer and queue
//! dispatch, idle advance, trace); a backend runs one step at a boundary or in
//! the halt wait. [`FlatBackend`] holds the S4-27i1..i3 flat session logic
//! unchanged; `PagedBackend` (`paged_pump_backend`) runs the same pump over the
//! S4-27h/S4-27i4a paged primitives. `InterruptPump` defaults to the flat
//! backend, so existing callers and tests are unchanged.
//!
//! Accepted APIs are used as they are. Consequences recorded as limits:
//! the first handler instruction runs inside `run_apic_quantum` and must be an
//! instruction the ordinary interpreter executes (not STI, CLI, HLT, IRETQ); a
//! partial REP (iteration budget exhausted) leaves the boundary incomplete, so
//! the pump finishes the instruction before the next APIC check (interrupt
//! latency deviates from hardware, which may deliver between iterations).

use crate::diagnostic_vcpu::{DiagnosticHaltWait, DiagnosticHaltWaitError, DiagnosticIrqEntry};
use crate::{
    DeviceError, DiagnosticApicQuantumResult, DiagnosticExit, DiagnosticHaltEnterReason,
    DiagnosticHaltWakeResult, DiagnosticIretqResult, DiagnosticRunResult,
    DiagnosticStiApicQuantumResult, DiagnosticTerminalExit, DiagnosticVcpuControlError,
    DiagnosticVcpuSession, EventGeneration, EventQueue, EventQueueError, LocalApicHandle,
    WorkLimitReason,
};
use od_isa::StiShadow;
use od_isa::interrupt::InterruptOutsideProfile;
use std::any::Any;
use std::collections::BTreeMap;

/// Tracing bound; exceeding it ends the run instead of growing without limit.
pub const INTERRUPT_PUMP_TRACE_LIMIT: usize = 512;

/// Capacity of the pump's IRQ event queue (S4-27i3).
pub const INTERRUPT_PUMP_QUEUE_CAPACITY: usize = 16;

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
    /// S4-27i2: the pump does not own the timer: no `dispatch_timer` and no
    /// idle advance, so a halted guest waiting for its timer stays idle.
    NoTimerDispatch,
    /// S4-27i3: the queued event is removed before `request_fixed_edge`, so a
    /// failed request loses the event instead of keeping it for a retry.
    PopBeforeRequest,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpEntrySource {
    /// Entered at an instruction boundary of a running CPU.
    Boundary,
    /// Entered from the halt wait; the saved RIP is the instruction after HLT.
    HaltWake,
}

/// One owner event recorded in the pump trace (S4-27i5). The pump does not
/// interpret `name`, `a` and `b`; the owner defines them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PumpHostEvent {
    pub name: &'static str,
    pub a: u64,
    pub b: u64,
}

/// Result of one instruction executed by the host (S4-27i5).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PumpHostStep {
    pub result: DiagnosticRunResult,
    /// Recorded after the instruction was accounted (not when it was terminal).
    pub events: Vec<PumpHostEvent>,
}

/// Owner hook of the pump (S4-27i5). Implemented by an owner that has devices
/// or instructions beyond the pump's interrupt model.
pub trait PumpHost: Any {
    /// Owner-side work at the start of every step, before the IRR is observed.
    /// `now_ns` is the session clock (0 without a clock).
    fn dispatch(&mut self, now_ns: u64) -> Result<Vec<PumpHostEvent>, String>;
    /// Earliest owner deadline strictly after `now_ns` that can change the IRR.
    fn next_deadline(&self, now_ns: u64) -> Option<u64>;
    /// Execute the instruction at RIP if the host owns it; `None` otherwise.
    fn step(
        &mut self,
        session: &mut DiagnosticVcpuSession,
    ) -> Option<Result<PumpHostStep, PumpRejection>>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
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
    /// S4-27i3: a due queue event merged into an IRR bit that was already set
    /// (no new edge; the IRR rise is the `Irr` event).
    Coalesced {
        vector: u8,
    },
    /// S4-27i5: an event recorded by the owner hook.
    Host(PumpHostEvent),
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
            Self::Coalesced { .. } => "coalesced",
            Self::Host(event) => event.name,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PumpTraceEvent {
    pub seq: u64,
    /// RIP of the CPU when the event was observed.
    pub rip: u64,
    /// Virtual time (session clock) when the event was recorded (S4-27i5);
    /// `None` without a clock.
    pub clock_ns: Option<u64>,
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
    /// `dispatch_timer` failed (S4-27i2).
    TimerDispatch(DeviceError),
    /// The idle clock advance failed (S4-27i2).
    TimerAdvance(DiagnosticHaltWaitError),
    /// The next timer or queue deadline is not in the future after dispatch, so
    /// idle advance cannot make progress (S4-27i2, S4-27i3).
    TimerNoProgress,
    /// S4-27i3: `request_fixed_edge` failed for a due queue event. The event is
    /// kept in the queue (unless the `PopBeforeRequest` injection removed it
    /// first); guest state is unchanged and the run is resumable.
    QueueRequest {
        vector: u8,
        error: DeviceError,
    },
    /// The IRQ event queue refused an inspection or removal (S4-27i3).
    QueueState(EventQueueError),
    /// A queued key has no vector route (S4-27i3; an internal inconsistency).
    QueueRoute(u64),
    /// The owner hook refused a dispatch or an instruction (S4-27i5).
    Host(String),
}

/// Refusal of a caller operation on the pump's IRQ event queue (S4-27i3).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PumpQueueError {
    Queue(EventQueueError),
    /// Vectors below 16 cannot be requested through the Local APIC.
    InvalidVector(u8),
    /// The session has no virtual clock, so events cannot be due.
    NoClock,
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

/// What a backend step asks the pump to do next (S4-27i4b).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BackendFlow {
    Continue,
    /// The CPU is halted and nothing is eligible: the pump may advance the clock
    /// (idle advance) or report `IdleHalt`.
    Idle,
    Exit(PumpExit),
}

/// Result of one backend step (S4-27i4b): the instructions it retired, the trace
/// events it produced (recorded by the pump in order, with the state after the
/// step) and the next action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendStep {
    pub retired: u64,
    pub events: Vec<PumpTraceKind>,
    pub flow: BackendFlow,
}

impl BackendStep {
    pub fn new(flow: BackendFlow) -> Self {
        Self {
            retired: 0,
            events: Vec::new(),
            flow,
        }
    }

    pub fn exit(exit: PumpExit) -> Self {
        Self::new(BackendFlow::Exit(exit))
    }
}

/// What the pump lends a backend for one step (S4-27i4b).
pub struct BackendEnv<'a> {
    pub apic: &'a LocalApicHandle,
    pub injection: Option<PumpInjection>,
    /// The owner hook (S4-27i5). Only the flat backend uses it (the hook's `step`
    /// takes a flat session); the paged backend ignores it.
    pub host: Option<&'a mut (dyn PumpHost + 'static)>,
}

/// The CPU side of the pump (S4-27i4b): one vCPU on some memory model that can
/// run one pump step at a boundary. The pump owns everything else (APIC polling
/// order, timer and queue dispatch, idle advance, trace, budget, STOP); a
/// backend decides what one step does: wake from the halt wait, enter the
/// handler at a boundary, or execute the instruction at RIP (`STI`, `HLT`,
/// `IRETQ`, ...). The flat implementation is [`FlatBackend`] (S4-27i1..i3, i5);
/// the paged one is `PagedBackend` (S4-27i4b).
pub trait PumpBackend {
    fn state(&self) -> &od_isa::LongModeState;
    /// The CPU is in the halt wait (a step wakes it or reports `Idle`).
    fn is_waiting(&self) -> bool;
    /// Virtual time of the backend's clock; `None` without a clock.
    fn virtual_time_ns(&self) -> Option<u64>;
    fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PumpRejection>;
    fn stop(&mut self);
    /// Read one qword of guest RAM without side effects (evidence only).
    fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64>;
    /// One step at an instruction boundary (or in the halt wait).
    fn step(&mut self, env: BackendEnv<'_>) -> BackendStep;
}

enum Cpu {
    Running(DiagnosticVcpuSession),
    Waiting(DiagnosticHaltWait),
}

/// The flat Long64/CPL0 backend: a [`DiagnosticVcpuSession`] (or its
/// [`DiagnosticHaltWait`] after HLT) driven through the accepted
/// `run_apic_quantum`, `run_sti_apic_quantum`, `run_same_cpl0_iretq` and
/// `poll_apic` primitives (S4-27i1..i3).
pub struct FlatBackend {
    cpu: Option<Cpu>,
    /// The last instruction stopped inside a REP: the boundary is incomplete.
    mid_rep: bool,
    /// S4-27i5: a boundary delivery only enters the handler.
    entry_only: bool,
}

impl FlatBackend {
    pub fn new(session: DiagnosticVcpuSession) -> Self {
        Self {
            cpu: Some(Cpu::Running(session)),
            mid_rep: false,
            entry_only: false,
        }
    }

    /// Account one run result; `Some` ends the call.
    fn account(
        &mut self,
        result: &DiagnosticRunResult,
        out: &mut BackendStep,
    ) -> Option<BackendFlow> {
        out.retired += result.instructions_executed;
        match &result.exit {
            DiagnosticExit::Terminal(cause) => {
                Some(BackendFlow::Exit(PumpExit::Terminal(cause.clone())))
            }
            DiagnosticExit::Stopped => Some(BackendFlow::Exit(PumpExit::Stopped)),
            DiagnosticExit::WorkLimit(WorkLimitReason::RepIterationBudget) => {
                self.mid_rep = true;
                None
            }
            DiagnosticExit::WorkLimit(_) if result.instructions_executed == 1 => {
                self.mid_rep = false;
                None
            }
            DiagnosticExit::WorkLimit(reason) => Some(BackendFlow::Exit(PumpExit::Rejected(
                PumpRejection::NoRetirement(*reason),
            ))),
        }
    }

    fn reject(rejection: PumpRejection) -> BackendStep {
        BackendStep::exit(PumpExit::Rejected(rejection))
    }

    fn terminal(cause: DiagnosticTerminalExit) -> BackendStep {
        BackendStep::exit(PumpExit::Terminal(cause))
    }

    fn step_waiting(&mut self, mut wait: DiagnosticHaltWait, env: &BackendEnv<'_>) -> BackendStep {
        match wait.poll_apic(env.apic) {
            DiagnosticHaltWakeResult::Entered { vector } => match wait.into_running() {
                Ok(session) => {
                    self.cpu = Some(Cpu::Running(session));
                    let mut out = BackendStep::new(BackendFlow::Continue);
                    out.events.push(PumpTraceKind::Entry {
                        vector,
                        source: PumpEntrySource::HaltWake,
                    });
                    out
                }
                Err(wait) => {
                    self.cpu = Some(Cpu::Waiting(wait));
                    Self::reject(PumpRejection::HaltEntry(
                        DiagnosticHaltEnterReason::ProfileRejected,
                    ))
                }
            },
            DiagnosticHaltWakeResult::Waiting => {
                self.cpu = Some(Cpu::Waiting(wait));
                BackendStep::new(BackendFlow::Idle)
            }
            DiagnosticHaltWakeResult::OutsideProfile(reason) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Self::reject(PumpRejection::HaltWake(reason))
            }
            DiagnosticHaltWakeResult::Stopped => {
                self.cpu = Some(Cpu::Waiting(wait));
                BackendStep::exit(PumpExit::Stopped)
            }
            DiagnosticHaltWakeResult::Terminal(cause) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Self::terminal(cause)
            }
        }
    }

    fn boundary_allows_delivery(
        &self,
        session: &DiagnosticVcpuSession,
        env: &BackendEnv<'_>,
    ) -> bool {
        let state = session.state();
        let shadow_clear = state.sti_shadow == Some(StiShadow::Clear)
            || (env.injection == Some(PumpInjection::SkipShadowCheck)
                && state.sti_shadow == Some(StiShadow::Blocking));
        !self.mid_rep
            && state.rflags & RFLAGS_IF != 0
            && shadow_clear
            && env.apic.eligible().is_some()
    }

    fn step_running(
        &mut self,
        mut session: DiagnosticVcpuSession,
        env: BackendEnv<'_>,
    ) -> BackendStep {
        // A partial REP leaves an incomplete boundary: finish the instruction
        // without an APIC check (documented latency deviation).
        if self.mid_rep {
            let outcome = session.run(1, || false);
            return self.finish_plain(session, outcome);
        }
        if self.boundary_allows_delivery(&session, &env) {
            if !self.entry_only {
                let outcome = session.run_apic_quantum(env.apic, || false);
                return self.finish_apic_quantum(session, outcome);
            }
            // S4-27i5: enter the handler without executing its first instruction.
            match session.deliver_pending_irq(env.apic) {
                Ok(Some(DiagnosticIrqEntry::Entered { vector })) => {
                    self.cpu = Some(Cpu::Running(session));
                    let mut out = BackendStep::new(BackendFlow::Continue);
                    out.events.push(PumpTraceKind::Entry {
                        vector,
                        source: PumpEntrySource::Boundary,
                    });
                    return out;
                }
                Ok(Some(DiagnosticIrqEntry::OutsideProfile(reason))) => {
                    self.cpu = Some(Cpu::Running(session));
                    return Self::reject(PumpRejection::InterruptEntry(reason));
                }
                Ok(Some(DiagnosticIrqEntry::Terminal(cause))) => {
                    self.cpu = Some(Cpu::Running(session));
                    return Self::terminal(cause);
                }
                Ok(None) => {}
                Err(error) => {
                    self.cpu = Some(Cpu::Running(session));
                    return Self::reject(PumpRejection::Control(error));
                }
            }
        }
        if let Some(outcome) = env.host.and_then(|host| host.step(&mut session)) {
            return self.finish_host(session, outcome);
        }
        let code = session.peek_code(2);
        let in_sti_shadow = session.state().sti_shadow == Some(StiShadow::Blocking);
        match code.as_slice() {
            [0xf4, ..] => self.enter_halt(session, env.injection),
            [0xfa | 0xfb, ..] => {
                let kind = if code[0] == 0xfb {
                    PumpTraceKind::Sti
                } else {
                    PumpTraceKind::Cli
                };
                self.step_sti_quantum(session, env.apic, Some(kind))
            }
            [0x48, 0xcf, ..] => match session.run_same_cpl0_iretq() {
                Ok(DiagnosticIretqResult::Retired) => {
                    self.cpu = Some(Cpu::Running(session));
                    let mut out = BackendStep::new(BackendFlow::Continue);
                    out.retired = 1;
                    out.events.push(PumpTraceKind::Iretq);
                    out
                }
                Ok(DiagnosticIretqResult::Terminal(cause)) => {
                    self.cpu = Some(Cpu::Running(session));
                    Self::terminal(cause)
                }
                Err(error) => {
                    self.cpu = Some(Cpu::Running(session));
                    Self::reject(PumpRejection::Control(error))
                }
            },
            _ if in_sti_shadow => self.step_sti_quantum(session, env.apic, None),
            _ => {
                let outcome = session.run_apic_quantum(env.apic, || false);
                self.finish_apic_quantum(session, outcome)
            }
        }
    }

    fn finish_host(
        &mut self,
        session: DiagnosticVcpuSession,
        outcome: Result<PumpHostStep, PumpRejection>,
    ) -> BackendStep {
        self.cpu = Some(Cpu::Running(session));
        match outcome {
            Ok(step) => {
                let mut out = BackendStep::new(BackendFlow::Continue);
                if let Some(flow) = self.account(&step.result, &mut out) {
                    out.flow = flow;
                    return out;
                }
                out.events
                    .extend(step.events.into_iter().map(PumpTraceKind::Host));
                out
            }
            Err(rejection) => Self::reject(rejection),
        }
    }

    fn finish_plain(
        &mut self,
        session: DiagnosticVcpuSession,
        outcome: Result<DiagnosticRunResult, DiagnosticVcpuControlError>,
    ) -> BackendStep {
        self.cpu = Some(Cpu::Running(session));
        match outcome {
            Ok(result) => {
                let mut out = BackendStep::new(BackendFlow::Continue);
                if let Some(flow) = self.account(&result, &mut out) {
                    out.flow = flow;
                }
                out
            }
            Err(error) => Self::reject(PumpRejection::Control(error)),
        }
    }

    fn finish_apic_quantum(
        &mut self,
        session: DiagnosticVcpuSession,
        outcome: Result<DiagnosticApicQuantumResult, DiagnosticVcpuControlError>,
    ) -> BackendStep {
        self.cpu = Some(Cpu::Running(session));
        let mut out = BackendStep::new(BackendFlow::Continue);
        match outcome {
            Ok(DiagnosticApicQuantumResult::Ordinary(result)) => {
                if let Some(flow) = self.account(&result, &mut out) {
                    out.flow = flow;
                }
                out
            }
            Ok(DiagnosticApicQuantumResult::Delivered { vector, handler }) => {
                out.events.push(PumpTraceKind::Entry {
                    vector,
                    source: PumpEntrySource::Boundary,
                });
                if let Some(flow) = self.account(&handler, &mut out) {
                    out.flow = flow;
                }
                out
            }
            Ok(DiagnosticApicQuantumResult::ProfileRejected) => {
                Self::reject(PumpRejection::ApicQuantumProfile)
            }
            Ok(DiagnosticApicQuantumResult::OutsideProfile(reason)) => {
                Self::reject(PumpRejection::InterruptEntry(reason))
            }
            Ok(DiagnosticApicQuantumResult::Stopped) => BackendStep::exit(PumpExit::Stopped),
            Ok(DiagnosticApicQuantumResult::Terminal(cause)) => Self::terminal(cause),
            Err(error) => Self::reject(PumpRejection::Control(error)),
        }
    }

    fn step_sti_quantum(
        &mut self,
        mut session: DiagnosticVcpuSession,
        apic: &LocalApicHandle,
        event: Option<PumpTraceKind>,
    ) -> BackendStep {
        let outcome = session.run_sti_apic_quantum(apic);
        self.cpu = Some(Cpu::Running(session));
        let mut out = BackendStep::new(BackendFlow::Continue);
        match outcome {
            DiagnosticStiApicQuantumResult::Ordinary(result) => {
                if let Some(flow) = self.account(&result, &mut out) {
                    out.flow = flow;
                    return out;
                }
                if let Some(kind) = event {
                    out.events.push(kind);
                }
                out
            }
            DiagnosticStiApicQuantumResult::TerminalExecution(result) => {
                if let Some(flow) = self.account(&result, &mut out) {
                    out.flow = flow;
                }
                out
            }
            DiagnosticStiApicQuantumResult::Delivered { vector, handler } => {
                out.events.push(PumpTraceKind::Entry {
                    vector,
                    source: PumpEntrySource::Boundary,
                });
                if let Some(flow) = self.account(&handler, &mut out) {
                    out.flow = flow;
                }
                out
            }
            DiagnosticStiApicQuantumResult::ProfileRejected => {
                Self::reject(PumpRejection::StiQuantumProfile)
            }
            DiagnosticStiApicQuantumResult::OutsideProfile(reason) => {
                Self::reject(PumpRejection::InterruptEntry(reason))
            }
            DiagnosticStiApicQuantumResult::Terminal(cause) => Self::terminal(cause),
        }
    }

    fn enter_halt(
        &mut self,
        mut session: DiagnosticVcpuSession,
        injection: Option<PumpInjection>,
    ) -> BackendStep {
        if injection == Some(PumpInjection::HltAsOrdinary) {
            let outcome = session.run(1, || false);
            return self.finish_plain(session, outcome);
        }
        match session.into_halt_wait() {
            Ok(wait) => {
                self.cpu = Some(Cpu::Waiting(wait));
                let mut out = BackendStep::new(BackendFlow::Continue);
                out.retired = 1;
                out.events.push(PumpTraceKind::Hlt);
                out
            }
            Err(failure) => {
                self.cpu = Some(Cpu::Running(failure.session));
                Self::reject(PumpRejection::HaltEntry(failure.reason))
            }
        }
    }
}

impl PumpBackend for FlatBackend {
    fn state(&self) -> &od_isa::LongModeState {
        match self.cpu.as_ref().expect("cpu is always restored") {
            Cpu::Running(session) => session.state(),
            Cpu::Waiting(wait) => wait.state(),
        }
    }

    fn is_waiting(&self) -> bool {
        matches!(self.cpu, Some(Cpu::Waiting(_)))
    }

    fn virtual_time_ns(&self) -> Option<u64> {
        match self.cpu.as_ref().expect("cpu is always restored") {
            Cpu::Running(session) => session.virtual_clock(),
            Cpu::Waiting(wait) => wait.virtual_clock(),
        }
        .map(|clock| clock.now_ns())
    }

    fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PumpRejection> {
        match self.cpu.as_mut().expect("cpu is always restored") {
            Cpu::Running(session) => session
                .advance_time_ns(delta_ns)
                .map_err(PumpRejection::Control),
            Cpu::Waiting(wait) => wait
                .advance_time_ns(delta_ns)
                .map_err(PumpRejection::TimerAdvance),
        }
    }

    fn stop(&mut self) {
        match self.cpu.as_mut().expect("cpu is always restored") {
            Cpu::Running(session) => session.stop(),
            Cpu::Waiting(wait) => wait.stop(),
        }
    }

    fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        match self.cpu.as_mut().expect("cpu is always restored") {
            Cpu::Running(session) => session.peek_ram_qword(gpa),
            Cpu::Waiting(wait) => wait.peek_ram_qword(gpa),
        }
    }

    fn step(&mut self, env: BackendEnv<'_>) -> BackendStep {
        match self.cpu.take().expect("cpu is always restored") {
            Cpu::Waiting(wait) => self.step_waiting(wait, &env),
            Cpu::Running(session) => self.step_running(session, env),
        }
    }
}

enum Flow {
    Continue,
    Exit(PumpExit),
}

/// The event pump over a CPU backend (S4-27i4b). The default backend is the
/// flat one, so `InterruptPump` without a parameter is the S4-27i1..i3 pump.
pub struct InterruptPump<B: PumpBackend = FlatBackend> {
    backend: B,
    apic: LocalApicHandle,
    watched: Vec<(u8, (bool, bool))>,
    trace: Vec<PumpTraceEvent>,
    retired_total: u64,
    injection: Option<PumpInjection>,
    finished: Option<PumpExit>,
    /// S4-27i3: IRQ events (key -> fixed vector and deadline).
    queue: EventQueue,
    routes: BTreeMap<u64, (u8, u64)>,
    coalesced: u64,
    /// S4-27i5: optional owner hook (flat backend only).
    host: Option<Box<dyn PumpHost>>,
}

/// Outcome of a halted step that tried to advance the clock (S4-27i5).
enum IdleOutcome {
    Halt,
    Advanced {
        /// Set when the clock stopped at an owner deadline.
        host_to: Option<u64>,
    },
}

impl InterruptPump<FlatBackend> {
    /// Take ownership of a runnable session at a completed instruction
    /// boundary and the APIC it is polled against. The caller certifies the
    /// prerequisites of the accepted primitives (one vCPU, private synchronous
    /// RAM, flat Long64/CPL0, no pending synchronous/debug/NMI events).
    pub fn new(session: DiagnosticVcpuSession, apic: LocalApicHandle) -> Self {
        Self::with_backend(FlatBackend::new(session), apic)
    }

    /// Attach the owner hook (S4-27i5).
    pub fn with_host(mut self, host: Box<dyn PumpHost>) -> Self {
        self.host = Some(host);
        self
    }

    /// A boundary delivery only enters the handler; its first instruction is a
    /// separate step (S4-27i5).
    pub fn with_entry_only_delivery(mut self) -> Self {
        self.backend.entry_only = true;
        self
    }
}

impl<B: PumpBackend> InterruptPump<B> {
    /// Build a pump over any backend (S4-27i4b). The caller certifies the
    /// backend's prerequisites.
    pub fn with_backend(backend: B, apic: LocalApicHandle) -> Self {
        Self {
            backend,
            apic,
            watched: Vec::new(),
            trace: Vec::new(),
            retired_total: 0,
            injection: None,
            finished: None,
            queue: EventQueue::new(INTERRUPT_PUMP_QUEUE_CAPACITY),
            routes: BTreeMap::new(),
            coalesced: 0,
            host: None,
        }
    }

    /// The backend (evidence access).
    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    /// The owner hook, if it has type `T` (S4-27i5).
    pub fn host<T: PumpHost>(&self) -> Option<&T> {
        self.host.as_ref()?.as_any().downcast_ref::<T>()
    }

    pub fn host_mut<T: PumpHost>(&mut self) -> Option<&mut T> {
        self.host.as_mut()?.as_any_mut().downcast_mut::<T>()
    }

    /// Advance the session clock between steps (S4-27i5). Only while the CPU is
    /// runnable or waiting.
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PumpRejection> {
        self.backend.advance_time_ns(delta_ns)
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
        self.backend.state()
    }

    pub fn is_waiting(&self) -> bool {
        self.backend.is_waiting()
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

    /// Virtual time in nanoseconds of the session clock (S4-27i2); `None`
    /// when the session has no clock.
    pub fn virtual_time_ns(&self) -> Option<u64> {
        self.backend.virtual_time_ns()
    }

    /// Token of the current generation of the IRQ event queue (S4-27i3).
    pub fn irq_generation(&self) -> EventGeneration {
        self.queue.generation()
    }

    /// Schedule one fixed-vector IRQ event at an absolute virtual time
    /// (S4-27i3). A deadline at or before the current time is due at the next
    /// boundary. The producer is the caller: the pump never creates events.
    pub fn schedule_irq(
        &mut self,
        generation: EventGeneration,
        key: u64,
        vector: u8,
        due_ns: u64,
    ) -> Result<(), PumpQueueError> {
        if vector < 16 {
            return Err(PumpQueueError::InvalidVector(vector));
        }
        if self.virtual_time_ns().is_none() {
            return Err(PumpQueueError::NoClock);
        }
        self.queue
            .schedule(generation, key, due_ns)
            .map_err(PumpQueueError::Queue)?;
        self.routes.insert(key, (vector, due_ns));
        Ok(())
    }

    /// Cancel a pending IRQ event (S4-27i3).
    pub fn cancel_irq(
        &mut self,
        generation: EventGeneration,
        key: u64,
    ) -> Result<(), PumpQueueError> {
        self.queue
            .cancel(generation, key)
            .map_err(PumpQueueError::Queue)?;
        self.routes.remove(&key);
        Ok(())
    }

    /// Drop every pending IRQ event and start a new generation; tokens and
    /// events of the old generation are refused afterwards (S4-27i3).
    pub fn reset_irq_queue(&mut self) -> Result<EventGeneration, PumpQueueError> {
        let next = self
            .queue
            .reset(self.queue.generation())
            .map_err(PumpQueueError::Queue)?;
        self.routes.clear();
        Ok(next)
    }

    /// Number of pending IRQ events.
    pub fn irq_queue_len(&self) -> usize {
        self.queue.len()
    }

    /// Earliest deadline among pending IRQ events, due or not.
    pub fn irq_queue_next_deadline(&self) -> Option<u64> {
        self.queue.next_deadline()
    }

    /// Due events that merged into an already pending IRR bit (S4-27i3).
    pub fn coalesced_irqs(&self) -> u64 {
        self.coalesced
    }

    /// Read one qword of guest RAM without side effects (evidence only).
    pub fn peek_ram_qword(&mut self, gpa: u64) -> Option<u64> {
        self.backend.peek_ram_qword(gpa)
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
                self.backend.stop();
                self.finished = Some(PumpExit::Stopped);
                break PumpExit::Stopped;
            }
            if quanta >= budget {
                break PumpExit::Budget;
            }
            match self.step() {
                Flow::Continue => quanta += 1,
                Flow::Exit(exit) => {
                    // IdleHalt and a failed queue request (the event is kept) resume.
                    if !matches!(
                        exit,
                        PumpExit::IdleHalt | PumpExit::Rejected(PumpRejection::QueueRequest { .. })
                    ) {
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

    fn push(&mut self, kind: PumpTraceKind) -> Result<(), PumpExit> {
        if self.trace.len() >= INTERRUPT_PUMP_TRACE_LIMIT {
            return Err(PumpExit::Rejected(PumpRejection::TraceLimit));
        }
        let rip = self.state().rip;
        let clock_ns = self.virtual_time_ns();
        self.trace.push(PumpTraceEvent {
            seq: self.trace.len() as u64,
            rip,
            clock_ns,
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

    /// The pump owns the timer only when the APIC has one and the injection
    /// does not disable it.
    fn owns_timer(&self) -> bool {
        self.injection != Some(PumpInjection::NoTimerDispatch) && self.apic.has_timer()
    }

    /// Dispatch at most one due timer event (S4-27i2).
    fn dispatch_due_timer(&mut self) -> Result<(), PumpExit> {
        if self.owns_timer() {
            self.apic
                .dispatch_timer()
                .map_err(|error| PumpExit::Rejected(PumpRejection::TimerDispatch(error)))?;
        }
        Ok(())
    }

    /// Take every due IRQ event (S4-27i3): `peek_due`, `request_fixed_edge`,
    /// then `remove_due` only after the request succeeded.
    fn dispatch_due_queue(&mut self) -> Result<(), PumpExit> {
        if self.routes.is_empty() {
            return Ok(());
        }
        let Some(now) = self.virtual_time_ns() else {
            return Ok(());
        };
        let generation = self.queue.generation();
        let pop_first = self.injection == Some(PumpInjection::PopBeforeRequest);
        let reject = |rejection| PumpExit::Rejected(rejection);
        for _ in 0..INTERRUPT_PUMP_QUEUE_CAPACITY {
            let Some(event) = self
                .queue
                .peek_due(generation, now)
                .map_err(|error| reject(PumpRejection::QueueState(error)))?
            else {
                break;
            };
            let (vector, _) = *self
                .routes
                .get(&event.key)
                .ok_or_else(|| reject(PumpRejection::QueueRoute(event.key)))?;
            let merges = self.apic.vector_state(vector).0;
            if pop_first {
                self.remove_due_event(generation, now)?;
            }
            self.apic
                .request_fixed_edge(vector)
                .map_err(|error| reject(PumpRejection::QueueRequest { vector, error }))?;
            if !pop_first {
                self.remove_due_event(generation, now)?;
            }
            self.observe()?;
            if merges {
                self.coalesced += 1;
                self.push(PumpTraceKind::Coalesced { vector })?;
            }
        }
        Ok(())
    }

    fn remove_due_event(&mut self, generation: EventGeneration, now: u64) -> Result<(), PumpExit> {
        let removed = self
            .queue
            .remove_due(generation, now)
            .map_err(|error| PumpExit::Rejected(PumpRejection::QueueState(error)))?;
        if let Some(event) = removed {
            self.routes.remove(&event.key);
        }
        Ok(())
    }

    /// Owner-side work before the IRR is observed (S4-27i5).
    fn dispatch_host(&mut self) -> Result<(), PumpExit> {
        let now = self.virtual_time_ns().unwrap_or(0);
        let Some(host) = self.host.as_mut() else {
            return Ok(());
        };
        let events = host
            .dispatch(now)
            .map_err(|detail| PumpExit::Rejected(PumpRejection::Host(detail)))?;
        for event in events {
            self.push(PumpTraceKind::Host(event))?;
        }
        Ok(())
    }

    fn step_inner(&mut self) -> Result<Flow, PumpExit> {
        self.dispatch_host()?;
        self.observe()?;
        self.dispatch_due_timer()?;
        self.dispatch_due_queue()?;
        self.observe()?;
        let step = {
            let env = BackendEnv {
                apic: &self.apic,
                injection: self.injection,
                host: self.host.as_deref_mut(),
            };
            self.backend.step(env)
        };
        self.retired_total += step.retired;
        for kind in step.events {
            self.push(kind)?;
        }
        let flow = match step.flow {
            BackendFlow::Continue => Flow::Continue,
            BackendFlow::Exit(exit) => Flow::Exit(exit),
            BackendFlow::Idle => self.idle_flow()?,
        };
        self.observe()?;
        Ok(flow)
    }

    /// The halted CPU has nothing eligible: advance the clock or go idle.
    fn idle_flow(&mut self) -> Result<Flow, PumpExit> {
        match self.idle_advance() {
            Ok(IdleOutcome::Advanced { host_to }) => {
                if let Some(to_ns) = host_to {
                    self.push(PumpTraceKind::Host(PumpHostEvent {
                        name: "idle_advance",
                        a: to_ns,
                        b: 0,
                    }))?;
                }
                Ok(Flow::Continue)
            }
            Ok(IdleOutcome::Halt) => Ok(Flow::Exit(PumpExit::IdleHalt)),
            Err(rejection) => Ok(Flow::Exit(PumpExit::Rejected(rejection))),
        }
    }

    /// Timer deadline that can still wake the CPU: not when its vector already
    /// waits in the IRR behind an in-service vector or the TPR (ISSUE-070).
    fn useful_timer_deadline(&self) -> Option<u64> {
        if !self.owns_timer() {
            return None;
        }
        let deadline = self.apic.next_timer_deadline()?;
        let blocked = self
            .apic
            .timer_vector()
            .is_some_and(|vector| self.apic.vector_state(vector).0)
            && self.apic.eligible().is_none();
        (!blocked).then_some(deadline)
    }

    /// Earliest queue deadline whose vector is not already pending in the IRR.
    fn useful_queue_deadline(&self) -> Option<u64> {
        self.routes
            .values()
            .filter(|(vector, _)| !self.apic.vector_state(*vector).0)
            .map(|(_, deadline)| *deadline)
            .min()
    }

    /// Waiting with no eligible vector (S4-27i2, S4-27i3): advance the clock to
    /// the earlier of the next timer deadline and the next queue deadline that
    /// can still change the IRR, and dispatch the timer. `Halt` means
    /// nothing can wake the CPU (IF=0, no such deadline, or the only sources
    /// are blocked in the IRR: ISSUE-070). S4-27i5: an owner deadline
    /// (`PumpHost::next_deadline`) is waited for regardless of IF.
    fn idle_advance(&mut self) -> Result<IdleOutcome, PumpRejection> {
        let now = self.backend.virtual_time_ns();
        let maskable = if self.backend.state().rflags & RFLAGS_IF == 0 {
            None
        } else {
            [self.useful_timer_deadline(), self.useful_queue_deadline()]
                .into_iter()
                .flatten()
                .min()
        };
        let host_deadline = match (&self.host, now) {
            (Some(host), Some(now)) => host.next_deadline(now),
            _ => None,
        };
        let Some(deadline) = [maskable, host_deadline].into_iter().flatten().min() else {
            return Ok(IdleOutcome::Halt);
        };
        let now = now.ok_or(PumpRejection::TimerAdvance(DiagnosticHaltWaitError::Clock(
            crate::VirtualClockError::ClockNotConfigured,
        )))?;
        if deadline <= now {
            return Err(PumpRejection::TimerNoProgress);
        }
        self.backend.advance_time_ns(deadline - now)?;
        if self.owns_timer() {
            self.apic
                .dispatch_timer()
                .map_err(PumpRejection::TimerDispatch)?;
        }
        Ok(IdleOutcome::Advanced {
            host_to: (host_deadline == Some(deadline)).then_some(deadline),
        })
    }
}
