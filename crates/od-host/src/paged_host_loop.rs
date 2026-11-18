//! S3-38o: the production host loop of one vCPU over [`PagedFetchedLoadSession`].
//!
//! One thread, one vCPU, one session. The loop owns no executable memory and no
//! thread: it borrows the caller's session, the caller's [`PagedHostLoopControl`]
//! (STOP byte and request flags) and the caller's CPU state and bus. S6-42 runs
//! the same loop on vCPU threads, one session (and one slot part of the process
//! `MAP_JIT` region, `crate::jit_region`) per thread; the loop itself only gained
//! the call of the session's safe point and the acknowledgement of
//! [`PagedVcpuGroup`] (S6-40 section 5, S1 and S3).
//!
//! # Iteration
//!
//! Every iteration starts on the safe point of S3-25c: the previous native call
//! (if any) returned, its result and counters were checked, the `committed_write`
//! notifications of the step were delivered, and no lookup has started. In order:
//!
//! 1. **S6-40 section 5, S3, step 2.** The loop takes the STOP byte down
//!    (`swap(0, SeqCst)`) and then rereads the request flags (`SeqCst`). Only the
//!    loop takes the byte down; the loop never clears a flag. This happens once
//!    per iteration (a JIT step or the fallback interpreter step); the flags are
//!    not reread between a JIT refusal and the interpreter step that follows it,
//!    which stays within the bound of one instruction.
//! 2. Exit checks, in priority order: STOP flag, finish flag, budget, event flag
//!    (a raised flag means no step runs; the loop returns the reason on the safe
//!    point). STOP beats the event; the budget beats the event as in the
//!    dispatcher of S3-28c.
//! 3. One step: the JIT path of the session ([`PagedFetchedLoadSession::
//!    execute_forms_with_stop`], admitted one-instruction forms, the caller's STOP
//!    byte passed as `RuntimeContextV1::stop_requested`). If the session refused
//!    without effects of the instruction, exactly one step of the shared paged
//!    interpreter runs: `od_machine::run_paged_probe` with a budget of one, the
//!    interpreter the board uses (`FirmwareBoard::run_quantum`). Every RAM write of
//!    both paths goes through `commit_write` inside the bus (the page generation
//!    grows); the walk writes are delivered to the caller's `committed_write`.
//!
//! The budget is shared: `retired_jit + retired_interpreter + remaining ==
//! incoming`.
//!
//! # STOP byte protocol (single-thread form of S6-40 section 5, S3)
//!
//! A requester sets its request flag (`SeqCst`) and then the byte (`SeqCst`):
//! [`PagedHostLoopControl::request_stop`], [`PagedHostLoopControl::request_event`],
//! [`PagedHostLoopControl::request_finish`]. A request raised before the loop
//! rereads the flags means the step does not run (retired 0 for that iteration). A
//! request raised later leaves the byte set: a native block that sees it exits
//! `StopRequested` at its next guard (before the instruction, retired 0, no
//! effect, or right after the instruction, retired 1). That is an ordinary result
//! and the instruction is never repeated; the next iteration sees the flag. A byte
//! without a flag (the request was withdrawn) is harmless. The flag set is
//! extensible: S6-42 adds `stop_all` and `split_owner` to the reread, the order
//! (flag, byte; take byte down, reread flags) does not change.
//!
//! # S6-42c: the guest side of the boundary (opt-in)
//!
//! [`run_paged_host_loop_shared_set`] is the same loop with a [`PagedHostSetEnv`]: the interpreter
//! step is one instruction of the CPL0 profile set (`od_machine::run_host_set_step`: the profile of
//! the Linux board's CPL0 quantum, `IRETQ`, `STI`, `LOCK` forms, `#PF` and `#GP(0)` delivered through
//! the guest IDT, the split `LOCK` behind the gate of the run) and, before every step, the loop
//! asks the board source on the BSP and the Local APIC source. A delivery retires nothing and is
//! followed by the next iteration's checks. The loop delivers
//! a native guest fault using its original fault record. The CPL0 set may own CR3, in which case
//! every instruction boundary and native memory invocation uses the CPU's current CR3.
//!
//! # Not covered
//!
//! One vCPU, one thread, no `Send`/`Sync` for the session or its handles. The JIT
//! profile is one instruction per block; the admission mapping per session stays
//! (a region per admission), slots of a region belong to S6-42. S3-27 (host fault
//! recovery in generated code) is open.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::{Duration, Instant};

use crate::{
    MemoryOperationExecutionErrorV2, MemoryOperationExecutionExitV2, PagedFetchedLoadErrorKind,
    PagedFetchedLoadSession, atomic_inline::AtomicInlineExit,
};
use od_isa::execute::Cpl0ProfileSet;
use od_isa::memory::{ArchitecturalFault, PageFaultRecord};

const RFLAGS_RF: u64 = 1 << 16;

/// The request flags of one vCPU loop, as one value read after the STOP byte was
/// taken down.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PagedHostLoopRequests {
    stop: bool,
    finish: bool,
    event: bool,
}

/// STOP byte and request flags of one vCPU loop. Owned by the caller and shared
/// by reference with requesters; the session never owns it.
///
/// Every field is an atomic, so the value is `Sync`: a requester on another
/// thread calls `request_*` while the loop thread runs. The session, the bus and
/// the CPU state stay on the loop thread.
#[derive(Debug, Default)]
pub struct PagedHostLoopControl {
    stop_byte: AtomicU8,
    stop: AtomicBool,
    finish: AtomicBool,
    event: AtomicBool,
    /// S6-34 `safe`: raised by the vCPU thread on the safe point after the loop
    /// returned for a STOP request (S6-40 section 5, S1), lowered before the
    /// thread reads the request flags again. Never raised inside a native call.
    safe: AtomicBool,
}

impl PagedHostLoopControl {
    pub const fn new() -> Self {
        Self {
            stop_byte: AtomicU8::new(0),
            stop: AtomicBool::new(false),
            finish: AtomicBool::new(false),
            event: AtomicBool::new(false),
            safe: AtomicBool::new(false),
        }
    }

    /// Whether a STOP request is raised (the flag, not the byte).
    pub fn stop_pending(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// The vCPU thread's acknowledgement of a stop (S6-34 `safe`). Call it only
    /// on the safe point, after [`run_paged_host_loop`] returned
    /// [`PagedHostLoopExit::StopRequested`]: the native call is over and the
    /// state of the vCPU is entirely in its `LongModeState` (S6-40 section 5, S2).
    pub fn acknowledge_safe(&self) {
        self.safe.store(true, Ordering::SeqCst);
    }

    /// Lowers the acknowledgement. The thread does this first, and only then
    /// rereads the request flags (Dekker order of S6-34): a requester that sets
    /// its flag and then still finds `safe` up knows the thread has not read the
    /// flag yet, and the thread's reread then sees it.
    pub fn withdraw_safe(&self) {
        self.safe.store(false, Ordering::SeqCst);
    }

    pub fn is_safe(&self) -> bool {
        self.safe.load(Ordering::SeqCst)
    }

    /// The byte generated code polls (`RuntimeContextV1::stop_requested`).
    pub fn stop_byte(&self) -> &AtomicU8 {
        &self.stop_byte
    }

    /// Step 1 of the protocol: the request flag, then the byte (both `SeqCst`).
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.stop_byte.store(1, Ordering::SeqCst);
    }

    /// Withdraws the STOP request (the requester's `resume`). The byte is not
    /// touched: only the loop takes it down.
    pub fn clear_stop_request(&self) {
        self.stop.store(false, Ordering::SeqCst);
    }

    pub fn request_finish(&self) {
        self.finish.store(true, Ordering::SeqCst);
        self.stop_byte.store(1, Ordering::SeqCst);
    }

    pub fn clear_finish_request(&self) {
        self.finish.store(false, Ordering::SeqCst);
    }

    pub fn request_event(&self) {
        self.event.store(true, Ordering::SeqCst);
        self.stop_byte.store(1, Ordering::SeqCst);
    }

    pub fn clear_event_request(&self) {
        self.event.store(false, Ordering::SeqCst);
    }

    /// Step 2 of the protocol, called only by the loop: takes the byte down
    /// (`SeqCst`) and then rereads every flag (`SeqCst`). Returns whether the byte
    /// was set.
    fn take_byte_and_reread_flags(&self) -> (bool, PagedHostLoopRequests) {
        let byte_was_set = self.stop_byte.swap(0, Ordering::SeqCst) != 0;
        let requests = PagedHostLoopRequests {
            stop: self.stop.load(Ordering::SeqCst),
            finish: self.finish.load(Ordering::SeqCst),
            event: self.event.load(Ordering::SeqCst),
        };
        (byte_was_set, requests)
    }
}

/// S6-34 `stop_all` and `resume` for a set of vCPU threads that run
/// [`run_paged_host_loop`] (S6-40 section 5, S3), on the host side of the JIT
/// loop. The controller sets each vCPU's flag and then its STOP byte
/// ([`PagedHostLoopControl::request_stop`]) and waits until every vCPU has
/// acknowledged. A vCPU thread acknowledges only on its safe point, after the
/// loop returned; a thread that is inside a native call never acknowledges
/// until the call has returned and its result is accounted.
///
/// The thread side is the driver loop: call [`run_paged_host_loop`]; on
/// [`PagedHostLoopExit::StopRequested`] call
/// [`PagedHostLoopControl::acknowledge_safe`], wait until
/// [`PagedHostLoopControl::stop_pending`] is false, call
/// [`PagedHostLoopControl::withdraw_safe`], and call the loop again.
///
/// This type is not the `ParallelControl` of `od-machine`; it is the same
/// protocol for loops that run in threads of the caller's own. S6-42b binds the
/// loop to the real `ParallelControl` instead: `crate::host_threads::JitHostQuantum`
/// is the quantum of the vCPU threads of `od_machine::run_host_threads`, where
/// `stop_all` of the run reaches the loop as the `stop` poll of
/// [`run_paged_host_loop_shared`]. This group stays for tests of the protocol and
/// for hosts that own their threads.
pub struct PagedVcpuGroup<'a> {
    controls: Vec<&'a PagedHostLoopControl>,
}

/// A vCPU that did not acknowledge within the deadline of
/// [`PagedVcpuGroup::stop_all`].
#[derive(Debug, Eq, PartialEq)]
pub struct PagedStopTimeout {
    /// Indices (in the order given to [`PagedVcpuGroup::new`]) of the vCPUs
    /// whose `safe` was not up at the deadline.
    pub missing: Vec<usize>,
}

impl<'a> PagedVcpuGroup<'a> {
    pub fn new(controls: Vec<&'a PagedHostLoopControl>) -> Self {
        Self { controls }
    }

    /// Requests a stop of every vCPU and waits for every acknowledgement. When
    /// it returns `Ok`, no vCPU executes an instruction until [`Self::resume`]:
    /// each flag was raised before the acknowledgement was read, and a thread
    /// reads its flags after it lowered `safe` (Dekker order).
    ///
    /// `Ok` may be returned from a `safe` left over from the previous stop: by
    /// the Dekker order this is still correct, because the thread rereads its
    /// flags before the next step and sees the raised request. On `Err` the
    /// requests stay raised; the caller must call [`Self::resume`].
    pub fn stop_all(&self, timeout: Duration) -> Result<(), PagedStopTimeout> {
        for control in &self.controls {
            control.request_stop();
        }
        let deadline = Instant::now() + timeout;
        loop {
            let missing: Vec<usize> = self
                .controls
                .iter()
                .enumerate()
                .filter(|(_, control)| !control.is_safe())
                .map(|(index, _)| index)
                .collect();
            if missing.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(PagedStopTimeout { missing });
            }
            std::thread::yield_now();
        }
    }

    /// Withdraws the stop requests. The STOP bytes are not touched (the loops take
    /// them down).
    pub fn resume(&self) {
        for control in &self.controls {
            control.clear_stop_request();
        }
    }
}

/// Initial translation and access context of the vCPU. A CPL0 set that owns CR3 uses this value
/// to initialize a state without CR3, then uses the current CPU value on each step.
#[derive(Clone, Copy, Debug)]
pub struct PagedHostLoopConfig {
    pub cr3: u64,
    pub efer: u64,
    pub access_context: od_isa::memory::AccessContext,
}

/// Why a call of the loop returned. Every exit is on the safe point.
#[derive(Debug, Eq, PartialEq)]
pub enum PagedHostLoopExit {
    /// The incoming budget is spent.
    BudgetExhausted,
    /// The STOP flag was raised when the loop reread the flags: no step ran after
    /// that. The flag is the requester's to clear.
    StopRequested,
    FinishRequested,
    /// The event flag is raised (no STOP, finish or budget exit applies). It is
    /// reported before the next step and left raised.
    EventPending,
    /// The instruction raised an architectural fault (`#PF` of the data operand,
    /// instruction fetch, ...). The CPU state is that of the interpreter's
    /// `Fault` exit: nothing of the instruction retired. See
    /// [`PagedHostLoopReport::page_fault`].
    GuestFault(od_isa::memory::ArchitecturalFault),
    /// A terminal exit of the shared interpreter (`Halted`, `Shutdown`,
    /// `Unsupported`, `Backend`, `PortIo`, `Rejected`, `IntegrationError`).
    Interpreter(od_machine::PagedProbeExit),
    /// The JIT boundary failed in a way that does not prove the CPU state is
    /// unchanged. The step is not retried.
    JitHostFailure(MemoryOperationExecutionErrorV2),
    /// A V4 atomic invocation failed at the host boundary.
    AtomicHostFailure(PagedFetchedLoadErrorKind),
    /// `rflags_defined` has bits outside `INTEGER_FLAGS`; nothing ran.
    InvalidRflagsDefined {
        value: u64,
    },
    /// S6-42c (set mode only): a vector waiting in the vCPU's Local APIC could not be delivered
    /// (no IDT, a gate or stack that faults, a refused state). The text names the vector and the
    /// reason; the state is what the failed delivery left.
    IrqDeliveryFailed(String),
    /// S6-42c (set mode only): the interpreter step could not be resumed from (an integration
    /// error of the CPL0 set); the text is the mapping error.
    InterpreterIntegration(String),
    /// The CPL0 quantum returns to reconcile a timer deadline or queued device work.
    DeviceBoundary,
}

/// Counters and exit of one call of [`run_paged_host_loop`].
#[derive(Debug)]
pub struct PagedHostLoopReport {
    pub exit: PagedHostLoopExit,
    pub incoming_budget: u64,
    pub remaining_budget: u64,
    /// Instructions retired by native blocks of the session.
    pub retired_jit: u64,
    /// Instructions retired by the shared interpreter.
    pub retired_interpreter: u64,
    /// Steps completed by the JIT path (an instruction retired).
    pub steps_jit: u64,
    /// Steps completed by the interpreter after a JIT refusal.
    pub steps_interpreter: u64,
    /// JIT attempts that the session refused without effects of the instruction
    /// and the interpreter then ran once.
    pub jit_refusals: u64,
    /// Native calls that ended `StopRequested` (the byte was raised after the
    /// loop reread the flags). Those with retired 1 count in `steps_jit`.
    pub native_stop_exits: u64,
    /// Iterations where the STOP byte was set when the loop took it down.
    pub stop_byte_observations: u64,
    /// Instructions retired (both paths) at the most recent such iteration. The
    /// value is written after the instructions of earlier steps were counted. The
    /// loop runs no step after a raised STOP flag, so for a STOP exit
    /// `retired_total()` minus this value is always 0 by the protocol.
    pub retired_at_stop_byte: Option<u64>,
    /// The `#PF` record of a `GuestFault` exit.
    pub page_fault: Option<od_isa::memory::PageFaultRecord>,
    /// S6-42c (set mode): external interrupts delivered to the guest through its IDT.
    pub irqs_delivered: u64,
    /// S6-42c (set mode): guest `#PF`/`#GP(0)` delivered through its IDT.
    pub exceptions_delivered: u64,
    /// Native faults delivered through the guest IDT from their original fault records.
    pub jit_faults_delivered: u64,
    /// Retirements via the V4 atomic JIT, included in `retired_jit`.
    pub atomic_jit_steps: u64,
}

impl PagedHostLoopReport {
    fn new(incoming: u64) -> Self {
        Self {
            exit: PagedHostLoopExit::BudgetExhausted,
            incoming_budget: incoming,
            remaining_budget: incoming,
            retired_jit: 0,
            retired_interpreter: 0,
            steps_jit: 0,
            steps_interpreter: 0,
            jit_refusals: 0,
            native_stop_exits: 0,
            stop_byte_observations: 0,
            retired_at_stop_byte: None,
            page_fault: None,
            irqs_delivered: 0,
            exceptions_delivered: 0,
            jit_faults_delivered: 0,
            atomic_jit_steps: 0,
        }
    }

    /// `retired_jit + retired_interpreter + remaining == incoming`.
    pub fn budget_is_consistent(&self) -> bool {
        self.retired_jit
            .checked_add(self.retired_interpreter)
            .and_then(|retired| retired.checked_add(self.remaining_budget))
            == Some(self.incoming_budget)
    }

    pub fn retired_total(&self) -> u64 {
        self.retired_jit + self.retired_interpreter
    }
}

/// Runs the loop of one vCPU until an exit of [`PagedHostLoopExit`].
///
/// `committed_write` receives every walk write of the instruction fetch and of
/// the data operand, JIT path (after the native call returned) and interpreter
/// path alike, in order. `rflags_defined` is the interpreter's definedness mask,
/// read and updated across steps.
pub fn run_paged_host_loop(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    config: PagedHostLoopConfig,
    state: &mut od_isa::LongModeState,
    bus: &mut od_machine::MachineBus,
    rflags_defined: &mut u64,
    incoming_budget: u64,
    committed_write: impl FnMut(od_machine::GuestPhysicalRange),
) -> PagedHostLoopReport {
    run_paged_host_loop_with_hook(
        session,
        control,
        config,
        state,
        bus,
        rflags_defined,
        incoming_budget,
        committed_write,
        |_, _| {},
    )
}

/// [`run_paged_host_loop`] with a hook called on every iteration that is about
/// to run a step: after the byte was taken down and the flags were reread, right
/// before the step. The argument is the number of steps started so far. Tests use
/// it to raise a request exactly in the window the protocol leaves open
/// (after the reread, before the native entry).
pub(crate) fn run_paged_host_loop_with_hook(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    config: PagedHostLoopConfig,
    state: &mut od_isa::LongModeState,
    bus: &mut od_machine::MachineBus,
    rflags_defined: &mut u64,
    incoming_budget: u64,
    committed_write: impl FnMut(od_machine::GuestPhysicalRange),
    before_step: impl FnMut(&PagedHostLoopControl, u64),
) -> PagedHostLoopReport {
    run_paged_host_loop_core(
        session,
        control,
        config,
        state,
        LoopBusRef::Exclusive(bus),
        rflags_defined,
        incoming_budget,
        committed_write,
        before_step,
        || false,
        None,
    )
}

/// What the loop reaches the machine through (S6-42b). `Exclusive`: the caller owns the bus
/// (`&mut MachineBus`, the single-vCPU paths of S3-38o and S6-42). `Shared`: the loop runs on a vCPU
/// thread of the parallel mode and holds only `&MachineBus` (A7 of S6-40): the JIT session, the
/// tracked fetch and the data operand reach RAM through the shared reference
/// (`PhysicalMemory` by `&self`), and the interpreter fallback takes the device lock for one port
/// access at a time. The loop itself holds no lock of the bus between two steps.
pub(crate) enum LoopBusRef<'a, 's> {
    Exclusive(&'a mut od_machine::MachineBus),
    Shared(&'a od_machine::MachineBus, usize),
    /// S6-42c: `Shared` with the guest side of the boundary (see [`PagedHostSetEnv`]).
    SharedSet(&'a od_machine::MachineBus, usize, PagedHostSetEnv<'a, 's>),
}

impl LoopBusRef<'_, '_> {
    fn machine(&self) -> &od_machine::MachineBus {
        match self {
            Self::Exclusive(bus) => bus,
            Self::Shared(bus, _) | Self::SharedSet(bus, _, _) => bus,
        }
    }
}

/// What the loop of a vCPU thread needs to deliver interrupts and exceptions to the guest and to run
/// its interpreter step as the CPL0 profile set (S6-42c).
pub struct PagedHostSetEnv<'a, 's> {
    /// The Local APIC of the vCPU (`od_machine::HostQuantumCall::lapic`).
    pub lapic: &'a od_machine::LocalApicHandle,
    /// The board interrupt source lent to the boot processor for this call.
    pub board_irq: od_machine::HostBoardIrqSource<'a>,
    /// The board's COM1 and timer work at host-call boundaries.
    pub devices: od_machine::HostDeviceServices<'a>,
    /// The gate of the split `LOCK` of the thread (`od_machine::HostQuantumCall::gate`).
    pub gate: od_machine::HostSplitGate<'a>,
    /// The CPL0 profile set of the interpreter step, optionally owning the vCPU's CR3.
    pub set: &'a Cpl0ProfileSet<'s>,
    /// The REP rule of the interrupt boundary: the previous call ended inside a partial `REP`, so
    /// no interrupt is delivered at the first boundary. Kept by the caller across calls.
    pub skip_first_boundary: &'a mut bool,
}

/// [`run_paged_host_loop`] for a vCPU thread of the parallel mode (S6-42b): the same loop over
/// `&MachineBus` of vCPU `vcpu`, with `stop` polled on every iteration next to the flags of
/// `control` (the run's stop poll of `od_machine::HostQuantumCall::stop`: end of the run, a
/// `stop_all` request, another vCPU's split `LOCK`). A true `stop` ends the call like a raised STOP
/// flag: `StopRequested`, no step ran after it, on the safe point. The caller owns the thread's
/// session; the loop takes no lock of the bus (the device lock is taken by the interpreter fallback
/// for one port access at a time).
pub fn run_paged_host_loop_shared(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    config: PagedHostLoopConfig,
    state: &mut od_isa::LongModeState,
    bus: &od_machine::MachineBus,
    vcpu: usize,
    rflags_defined: &mut u64,
    incoming_budget: u64,
    committed_write: impl FnMut(od_machine::GuestPhysicalRange),
    stop: impl FnMut() -> bool,
) -> PagedHostLoopReport {
    run_paged_host_loop_core(
        session,
        control,
        config,
        state,
        LoopBusRef::Shared(bus, vcpu),
        rflags_defined,
        incoming_budget,
        committed_write,
        |_, _| {},
        stop,
        None,
    )
}

/// [`run_paged_host_loop_shared`] with the guest side of the boundary (S6-42c, see the module
/// documentation): interrupts of the vCPU's Local APIC delivered through the guest IDT before every
/// step, the interpreter step the CPL0 profile set of `env`, guest faults delivered, the split
/// `LOCK` behind the gate. Nothing else differs.
pub fn run_paged_host_loop_shared_set<'a>(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    config: PagedHostLoopConfig,
    state: &mut od_isa::LongModeState,
    bus: &'a od_machine::MachineBus,
    vcpu: usize,
    env: PagedHostSetEnv<'a, '_>,
    rflags_defined: &mut u64,
    incoming_budget: u64,
    committed_write: impl FnMut(od_machine::GuestPhysicalRange),
    stop: impl FnMut() -> bool,
) -> PagedHostLoopReport {
    run_paged_host_loop_core(
        session,
        control,
        config,
        state,
        LoopBusRef::SharedSet(bus, vcpu, env),
        rflags_defined,
        incoming_budget,
        committed_write,
        |_, _| {},
        stop,
        None,
    )
}

/// The fetched session accepts paging as an explicit input. Adapt just its invocation, where the
/// admitted forms cannot read or write CR3, and restore the architectural state before returning
/// to the loop. No guest boundary or caller write callback runs inside this adapter.
fn with_native_paging<T>(
    state: &mut od_isa::LongModeState,
    cpu_owns_cr3: bool,
    invoke: impl FnOnce(&mut od_isa::LongModeState) -> T,
) -> T {
    if !cpu_owns_cr3 {
        return invoke(state);
    }
    let cr3 = state.cr3.take();
    let result = invoke(state);
    state.cr3 = cr3;
    result
}

fn deliver_native_fault(
    report: &mut PagedHostLoopReport,
    state: &mut od_isa::LongModeState,
    bus: &od_machine::MachineBus,
    config: PagedHostLoopConfig,
    fault: &ArchitecturalFault,
    page_fault: Option<&PageFaultRecord>,
    consecutive: u64,
    committed_write: &mut dyn FnMut(od_machine::GuestPhysicalRange),
) -> bool {
    let Ok(paging) = od_machine::Long64ProbeConfig::from_cpu_state(state, config.cr3, config.efer)
    else {
        return false;
    };
    if !od_machine::deliver_host_native_fault(
        state,
        bus,
        &paging,
        config.access_context,
        fault,
        page_fault,
        consecutive,
        committed_write,
    ) {
        return false;
    }
    report.exceptions_delivered += 1;
    report.jit_faults_delivered += 1;
    true
}

/// True when the loop must return to its instruction boundary. A pre-entry
/// refusal or operand decline leaves the instruction to the CPL0 interpreter.
fn try_atomic_step(
    session: &mut PagedFetchedLoadSession,
    state: &mut od_isa::LongModeState,
    bus: &od_machine::MachineBus,
    config: PagedHostLoopConfig,
    cpu_owns_cr3: bool,
    stop_byte: &AtomicU8,
    rflags_defined: &mut u64,
    report: &mut PagedHostLoopReport,
    consecutive_native_faults: &mut u64,
    committed_write: &mut dyn FnMut(od_machine::GuestPhysicalRange),
) -> Result<bool, PagedHostLoopExit> {
    let jit = with_native_paging(state, cpu_owns_cr3, |state| {
        session.execute_atomic_with_stop(
            state,
            bus,
            config.cr3,
            config.efer,
            config.access_context,
            1,
            stop_byte,
            1,
        )
    });
    let jit = match jit {
        Ok(jit) => jit,
        Err(refusal) => {
            for range in refusal.fetch_committed_writes {
                committed_write(range);
            }
            if matches!(refusal.kind, PagedFetchedLoadErrorKind::AtomicRun(_)) {
                return Err(PagedHostLoopExit::AtomicHostFailure(refusal.kind));
            }
            return Ok(false);
        }
    };
    for range in jit
        .fetch_committed_writes
        .into_iter()
        .chain(jit.committed_writes)
    {
        committed_write(range);
    }
    let run = jit.run;
    let invalid =
        PagedHostLoopExit::JitHostFailure(MemoryOperationExecutionErrorV2::InvalidAccounting);
    if jit.block_instructions != 1 || 1u64.checked_sub(run.retired) != Some(run.remaining_budget) {
        return Err(invalid);
    }
    match (run.exit, run.retired) {
        (AtomicInlineExit::Complete | AtomicInlineExit::CodeInvalidated, 1) => {
            *rflags_defined = jit.first_flags.apply(*rflags_defined);
            state.rflags &= !RFLAGS_RF;
            report.retired_jit += 1;
            report.steps_jit += 1;
            report.atomic_jit_steps += 1;
            report.remaining_budget -= 1;
            *consecutive_native_faults = 0;
            Ok(true)
        }
        (AtomicInlineExit::StopRequested, 0) => {
            report.native_stop_exits += 1;
            Ok(true)
        }
        (AtomicInlineExit::Declined, 0) => Ok(false),
        (AtomicInlineExit::Fault, 0) => {
            let fault = run.fault.ok_or(PagedHostLoopExit::JitHostFailure(
                MemoryOperationExecutionErrorV2::InvalidOutcome,
            ))?;
            if deliver_native_fault(
                report,
                state,
                bus,
                config,
                &fault,
                run.page_fault.as_ref(),
                *consecutive_native_faults,
                committed_write,
            ) {
                *consecutive_native_faults += 1;
                return Ok(true);
            }
            report.page_fault = run.page_fault;
            Err(PagedHostLoopExit::GuestFault(fault))
        }
        _ => Err(invalid),
    }
}

pub(crate) fn run_paged_host_loop_core(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    config: PagedHostLoopConfig,
    state: &mut od_isa::LongModeState,
    mut bus: LoopBusRef<'_, '_>,
    rflags_defined: &mut u64,
    incoming_budget: u64,
    mut committed_write: impl FnMut(od_machine::GuestPhysicalRange),
    mut before_step: impl FnMut(&PagedHostLoopControl, u64),
    mut external_stop: impl FnMut() -> bool,
    native_stop: Option<&AtomicU8>,
) -> PagedHostLoopReport {
    let mut report = PagedHostLoopReport::new(incoming_budget);
    if *rflags_defined & !od_isa::INTEGER_FLAGS != 0 {
        report.exit = PagedHostLoopExit::InvalidRflagsDefined {
            value: *rflags_defined,
        };
        return report;
    }
    let stop_byte = native_stop.unwrap_or_else(|| control.stop_byte());
    let mut steps_started = 0u64;
    let cpl0_set = matches!(bus, LoopBusRef::SharedSet(..));
    let cpu_owns_cr3 =
        matches!(&bus, LoopBusRef::SharedSet(_, _, env) if env.set.has_cpu_owned_cr3());
    let mut consecutive_native_faults = 0;
    loop {
        // Safe point (S3-25c): nothing is executing, no lookup has started. The
        // session applies the slot releases it deferred (A4).
        session.safe_point();
        // Clear the worker's native byte before its request poll. A request posted
        // after the poll then leaves this same byte raised for native execution.
        let worker_byte_was_set =
            native_stop.is_some_and(|byte| byte.swap(0, Ordering::SeqCst) != 0);
        let (byte_was_set, requests) = control.take_byte_and_reread_flags();
        if worker_byte_was_set || byte_was_set {
            report.stop_byte_observations += 1;
            report.retired_at_stop_byte = Some(report.retired_total());
        }
        if requests.stop || external_stop() {
            report.exit = PagedHostLoopExit::StopRequested;
            return report;
        }
        if requests.finish {
            report.exit = PagedHostLoopExit::FinishRequested;
            return report;
        }
        if report.remaining_budget == 0 {
            report.exit = PagedHostLoopExit::BudgetExhausted;
            return report;
        }
        if requests.event {
            report.exit = PagedHostLoopExit::EventPending;
            return report;
        }
        // The interpreter publishes a retired MOV CR3 in the CPU state. Fetch, operand walks and
        // event delivery all use that value before any subsequent instruction or IRQ can run.
        let config = if cpu_owns_cr3 {
            PagedHostLoopConfig {
                cr3: *state.cr3.get_or_insert(config.cr3),
                ..config
            }
        } else {
            config
        };
        // Deliver IRQs before native and interpreted steps. A delivery retires nothing.
        if let LoopBusRef::SharedSet(machine, _, env) = &mut bus {
            if steps_started == 0 && env.devices.pending() {
                report.exit = PagedHostLoopExit::DeviceBoundary;
                return report;
            }
            match env.board_irq.deliver_pending(
                state,
                machine,
                env.lapic,
                config.cr3,
                config.efer,
                config.access_context,
                env.skip_first_boundary,
                &mut committed_write,
            ) {
                od_machine::HostIrqBoundary::None => {}
                od_machine::HostIrqBoundary::Delivered => {
                    report.irqs_delivered += 1;
                    continue;
                }
                od_machine::HostIrqBoundary::Failed(text) => {
                    report.exit = PagedHostLoopExit::IrqDeliveryFailed(text);
                    return report;
                }
            }
        }
        before_step(control, steps_started);
        steps_started += 1;

        // JIT path for the admitted forms; a native block gets the caller's byte.
        let attempt = with_native_paging(state, cpu_owns_cr3, |state| {
            session.execute_forms_with_stop(
                state,
                bus.machine(),
                config.cr3,
                config.efer,
                config.access_context,
                stop_byte,
            )
        });
        match attempt {
            Ok(jit) => {
                // The walk writes are applied at the safe point after the call.
                for range in jit
                    .fetch_committed_writes
                    .iter()
                    .chain(jit.execution.committed_writes.iter())
                {
                    committed_write(*range);
                }
                match &jit.execution.callback_result {
                    // The RAM-only adapter refused the data operand (MMIO or
                    // another non-RAM target): no effect of the instruction, the
                    // interpreter runs once.
                    Err(MemoryOperationExecutionErrorV2::MemoryError(_)) => {}
                    Err(error) => {
                        report.exit = PagedHostLoopExit::JitHostFailure(error.clone());
                        return report;
                    }
                    Ok(result) => match &result.exit {
                        MemoryOperationExecutionExitV2::Complete
                        | MemoryOperationExecutionExitV2::CodeInvalidated
                        | MemoryOperationExecutionExitV2::StopRequested => {
                            let retired = result.retired_instructions;
                            if retired > 1 || retired > report.remaining_budget {
                                report.exit = PagedHostLoopExit::JitHostFailure(
                                    MemoryOperationExecutionErrorV2::InvalidAccounting,
                                );
                                return report;
                            }
                            if result.exit == MemoryOperationExecutionExitV2::StopRequested {
                                report.native_stop_exits += 1;
                            }
                            // `StopRequested` with retired 0 is the byte seen before
                            // the instruction: no effect, back to the safe point,
                            // where the next iteration sees the flag. With retired 1
                            // the instruction ran and is never repeated.
                            report.retired_jit += retired;
                            report.remaining_budget -= retired;
                            report.steps_jit += retired;
                            if retired != 0 {
                                consecutive_native_faults = 0;
                                if cpl0_set {
                                    state.rflags &= !RFLAGS_RF;
                                }
                            }
                            continue;
                        }
                        MemoryOperationExecutionExitV2::GuestFault(fault) => {
                            if cpl0_set
                                && deliver_native_fault(
                                    &mut report,
                                    state,
                                    bus.machine(),
                                    config,
                                    fault,
                                    jit.execution.page_fault.as_ref(),
                                    consecutive_native_faults,
                                    &mut committed_write,
                                )
                            {
                                consecutive_native_faults += 1;
                                continue;
                            }
                            report.page_fault = jit.execution.page_fault.clone();
                            report.exit = PagedHostLoopExit::GuestFault(fault.clone());
                            return report;
                        }
                        // A single-instruction block never ends this way.
                        MemoryOperationExecutionExitV2::BudgetExhausted
                        | MemoryOperationExecutionExitV2::StopAddress => {
                            report.exit = PagedHostLoopExit::JitHostFailure(
                                MemoryOperationExecutionErrorV2::InvalidOutcome,
                            );
                            return report;
                        }
                    },
                }
            }
            // Refused before the instruction ran; only walk writes of the fetch.
            Err(refusal) => {
                for range in &refusal.fetch_committed_writes {
                    committed_write(*range);
                }
                if cpl0_set
                    && matches!(&refusal.kind,
                    PagedFetchedLoadErrorKind::UnsupportedForm { bytes }
                        if od_jit::atomic_adapter::could_start_atomic(bytes))
                {
                    match try_atomic_step(
                        session,
                        state,
                        bus.machine(),
                        config,
                        cpu_owns_cr3,
                        stop_byte,
                        rflags_defined,
                        &mut report,
                        &mut consecutive_native_faults,
                        &mut committed_write,
                    ) {
                        Ok(true) => continue,
                        Ok(false) => {}
                        Err(exit) => {
                            report.exit = exit;
                            return report;
                        }
                    }
                }
            }
        }

        // Exactly one step of the shared interpreter.
        report.jit_refusals += 1;
        // S6-42c: what the CPL0-set step adds to the probe report (set mode only).
        let mut set_step: Option<(bool, u64, bool)> = None;
        let quantum = match &mut bus {
            LoopBusRef::Exclusive(bus) => od_machine::run_paged_probe(
                state,
                bus,
                config.cr3,
                config.efer,
                config.access_context,
                1,
                *rflags_defined,
                || false,
                &mut committed_write,
            ),
            LoopBusRef::Shared(bus, vcpu) => od_machine::run_paged_probe_shared(
                state,
                bus,
                *vcpu,
                config.cr3,
                config.efer,
                config.access_context,
                1,
                *rflags_defined,
                || false,
                &mut committed_write,
            ),
            LoopBusRef::SharedSet(bus, vcpu, env) => {
                match od_machine::run_host_set_step(
                    state,
                    bus,
                    od_machine::HostCpl0Step {
                        vcpu: *vcpu,
                        lapic: env.lapic,
                        gate: env.gate,
                        cr3: config.cr3,
                        efer: config.efer,
                        context: config.access_context,
                        rflags_defined: *rflags_defined,
                    },
                    env.set,
                    &mut committed_write,
                ) {
                    Ok(step) => {
                        *env.skip_first_boundary = step.partial_rep;
                        set_step = Some((
                            step.partial_rep,
                            step.delivered_exceptions,
                            step.timer_register_written
                                || (step.pit_written && env.board_irq.pit_write_ends_quantum())
                                || (step.port_accessed && env.devices.pending()),
                        ));
                        step.report
                    }
                    Err(text) => {
                        report.exit = PagedHostLoopExit::InterpreterIntegration(text);
                        return report;
                    }
                }
            }
        };
        *rflags_defined = quantum.rflags_defined;
        let retired = quantum.instructions_executed;
        if retired > 1 || retired > report.remaining_budget {
            report.exit = PagedHostLoopExit::JitHostFailure(
                MemoryOperationExecutionErrorV2::InvalidAccounting,
            );
            return report;
        }
        report.retired_interpreter += retired;
        if retired != 0 {
            consecutive_native_faults = 0;
        }
        report.remaining_budget -= retired;
        report.steps_interpreter += retired;
        if let Some((_, delivered, _)) = set_step {
            report.exceptions_delivered += delivered;
        }
        match quantum.exit {
            od_machine::PagedProbeExit::BudgetExhausted => {
                if retired != 1 {
                    report.exit = PagedHostLoopExit::JitHostFailure(
                        MemoryOperationExecutionErrorV2::InvalidAccounting,
                    );
                    return report;
                }
                if matches!(set_step, Some((_, _, true))) {
                    report.exit = PagedHostLoopExit::DeviceBoundary;
                    return report;
                }
            }
            // S6-42c: a REP that made progress without finishing: the call goes on at the same
            // instruction (the next boundary takes no interrupt, see `skip_first_boundary`).
            od_machine::PagedProbeExit::Stopped if matches!(set_step, Some((true, _, _))) => {}
            od_machine::PagedProbeExit::Fault(fault) => {
                report.page_fault = quantum.page_fault;
                report.exit = PagedHostLoopExit::GuestFault(fault);
                return report;
            }
            other => {
                report.page_fault = quantum.page_fault;
                report.exit = PagedHostLoopExit::Interpreter(other);
                return report;
            }
        }
    }
}

#[cfg(test)]
#[cfg_attr(
    not(all(target_os = "macos", target_arch = "aarch64")),
    allow(dead_code)
)]
mod tests {
    use super::*;
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::{LongModeState, Register};
    use od_machine::{
        AccessWidth, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, RamLimits,
    };

    const CODE_RIP: u64 = 0x0040_1000;
    const DATA_LINEAR: u64 = 0x0000_0080_0000_0120;
    const CR0: u64 = 0x8001_0033;
    const CR3: u64 = 0x1000;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
    const RFLAGS_DEFINED: u64 = od_isa::INTEGER_FLAGS & !(1 << 7);
    const RAM_BYTES: u64 = 0xb000;
    const CODE_PHYSICAL: u64 = 0x8000;
    const DATA_PHYSICAL: u64 = 0x9000;
    const DATA_QWORD: u64 = DATA_PHYSICAL + 0x120;
    const INITIAL_DATA: u64 = 0x0123_4567_89ab_cdef;

    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    fn config() -> PagedHostLoopConfig {
        PagedHostLoopConfig {
            cr3: CR3,
            efer: EFER,
            access_context: context(),
        }
    }

    fn put_qword(bus: &mut MachineBus, address: u64, value: u64) {
        bus.write_physical(
            GuestPhysicalAddress::new(address),
            AccessWidth::Qword,
            value,
            |_| {},
        )
        .unwrap();
    }

    /// Identity-free 4-level tables: code at linear `CODE_RIP` (physical
    /// 0x8000, writable), data at `DATA_LINEAR` (physical 0x9000).
    fn make_bus(code: &[u8]) -> MachineBus {
        let ram = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), RAM_BYTES).unwrap();
        let map =
            MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
                .unwrap();
        let mut bus =
            MachineBus::new(map, RamLimits::new(RAM_BYTES), vec![], vec![], vec![]).unwrap();
        populate_tables(&mut bus, code);
        bus
    }

    /// The page tables, the code page and the data qword of [`make_bus`], written into any
    /// bus whose RAM covers `0..RAM_BYTES` (S6-42b: also the RAM of a board).
    pub(crate) fn populate_tables(bus: &mut MachineBus, code: &[u8]) {
        for (address, value) in [
            (0x1000, 0x2007),
            (0x2000, 0x4007),
            (0x4010, 0x6007),
            (0x6008, CODE_PHYSICAL | 0x7),
            (0x1008, 0x3007),
            (0x3000, 0x5007),
            (0x5000, 0x7007),
            (0x7000, DATA_PHYSICAL | 0x7),
        ] {
            put_qword(bus, address, value);
        }
        bus.patch_physical_ram(GuestPhysicalAddress::new(CODE_PHYSICAL), code)
            .unwrap();
        put_qword(bus, DATA_QWORD, INITIAL_DATA);
    }

    fn make_state(rax: u64, rbx: u64, rcx: u64) -> LongModeState {
        let mut state = LongModeState {
            rip: CODE_RIP,
            cr0: Some(CR0),
            cr3: None,
            cr4: Some(CR4),
            rflags: 0x202,
            activity: Some(od_isa::state::CpuActivity::Running),
            sti_shadow: Some(od_isa::state::StiShadow::Clear),
            ..Default::default()
        };
        state.gpr[Register::Rax as usize] = rax;
        state.gpr[Register::Rbx as usize] = rbx;
        state.gpr[Register::Rcx as usize] = rcx;
        state
    }

    #[derive(Debug, PartialEq)]
    struct Observation {
        gpr: [u64; 16],
        rip: u64,
        rflags: u64,
        ram: Vec<u64>,
    }

    /// Registers, RIP, defined flags and every RAM qword. With `skip_code_page`
    /// the code page (physical 0x8000) is not compared: a JIT store form
    /// (`48 89 03`) has no counterpart in the shared interpreter, which admits
    /// only `48 89 0B`, so the reference runs the alias bytes there.
    fn observe(state: &LongModeState, bus: &mut MachineBus, skip_code_page: bool) -> Observation {
        Observation {
            gpr: state.gpr,
            rip: state.rip,
            rflags: state.rflags & RFLAGS_DEFINED,
            ram: (0..RAM_BYTES)
                .step_by(8)
                .filter(|address| {
                    !(skip_code_page && (CODE_PHYSICAL..CODE_PHYSICAL + 0x1000).contains(address))
                })
                .map(|address| {
                    bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                        .unwrap()
                })
                .collect(),
        }
    }

    /// `assert_eq!` on observations that names the differing field and qword.
    fn assert_same(actual: &Observation, expected: &Observation, what: &str) {
        assert_eq!(actual.gpr, expected.gpr, "{what}: registers");
        assert_eq!(actual.rip, expected.rip, "{what}: rip");
        assert_eq!(actual.rflags, expected.rflags, "{what}: rflags");
        let first_difference = actual
            .ram
            .iter()
            .zip(&expected.ram)
            .position(|(left, right)| left != right);
        assert_eq!(
            first_difference.map(|index| (index * 8, actual.ram[index], expected.ram[index])),
            None,
            "{what}: first differing RAM qword (index, actual, expected)"
        );
    }

    /// The page-table qwords of the instruction-fetch walk of `CODE_RIP`
    /// (PML4, PDPT, PD, PT entry).
    const FETCH_WALK_QWORDS: [u64; 4] = [0x1000, 0x2000, 0x4010, 0x6008];
    const ACCESSED: u64 = 0x20;

    /// Clears ONLY the Accessed bit (0x20) of the four qwords of the fetch walk.
    /// A native block that exits `StopRequested` before its first instruction ran
    /// has still performed the tracked instruction fetch, whose Accessed bits are
    /// committed (an effect of the fetch, not of the instruction); the
    /// interpreter reference that retired no instruction has not fetched yet.
    /// Dirty bits and the data-walk qwords are NOT touched: an unexpected data
    /// walk or Dirty bit of the not-run instruction stays visible.
    fn without_fetch_accessed_bits(mut observation: Observation) -> Observation {
        for address in FETCH_WALK_QWORDS {
            observation.ram[(address / 8) as usize] &= !ACCESSED;
        }
        observation
    }

    fn ranges(list: &[GuestPhysicalRange]) -> Vec<(u64, u64)> {
        list.iter().map(|r| (r.start().get(), r.len())).collect()
    }

    struct Reference {
        observation: Observation,
        exit: od_machine::PagedProbeExit,
        retired: u64,
        writes: Vec<(u64, u64)>,
        page_fault: Option<od_isa::memory::PageFaultRecord>,
    }

    /// The pure shared interpreter: one `run_paged_probe` quantum of `budget`.
    fn reference(
        code: &[u8],
        state: &LongModeState,
        budget: u64,
        skip_code_page: bool,
    ) -> Reference {
        let mut bus = make_bus(code);
        let mut state = state.clone();
        let mut writes = Vec::new();
        let quantum = od_machine::run_paged_probe(
            &mut state,
            &mut bus,
            CR3,
            EFER,
            context(),
            budget,
            RFLAGS_DEFINED,
            || false,
            |range| writes.push(range),
        );
        Reference {
            observation: observe(&state, &mut bus, skip_code_page),
            exit: quantum.exit,
            retired: quantum.instructions_executed,
            writes: ranges(&writes),
            page_fault: quantum.page_fault,
        }
    }

    /// `mov rax,[rbx]` (JIT), `mov [rbx],rax` (JIT store), `push r12`, `ret`:
    /// an endless guest loop, two JIT steps and two interpreter steps per turn,
    /// the stack balanced (`r12` holds the loop address).
    const LOOP_JIT_STORE: [u8; 9] = [0x48, 0x8b, 0x03, 0x48, 0x89, 0x03, 0x41, 0x54, 0xc3];
    /// The same loop with the store the interpreter implements, `mov [rbx],rcx`.
    const LOOP_ALIAS: [u8; 9] = [0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b, 0x41, 0x54, 0xc3];

    fn loop_state() -> LongModeState {
        let mut state = make_state(0, DATA_LINEAR, INITIAL_DATA);
        state.gpr[Register::Rsp as usize] = 0x0000_0080_0000_1000;
        state.gpr[Register::R12 as usize] = CODE_RIP;
        state
    }

    struct Run {
        report: PagedHostLoopReport,
        writes: Vec<(u64, u64)>,
    }

    fn run_loop(
        session: &mut PagedFetchedLoadSession,
        control: &PagedHostLoopControl,
        bus: &mut MachineBus,
        state: &mut LongModeState,
        rflags_defined: &mut u64,
        budget: u64,
    ) -> Run {
        let mut writes = Vec::new();
        let report = run_paged_host_loop(
            session,
            control,
            config(),
            state,
            bus,
            rflags_defined,
            budget,
            |range| writes.push((range.start().get(), range.len())),
        );
        Run { report, writes }
    }

    /// S6-42: how many parts the slot-backend region of a test has. The tests
    /// that open several sessions in a row take a part for each.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    const SLOT_BACKEND_PARTS: usize = 32;

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    thread_local! {
        /// Set by [`with_slot_backend`]: the next [`jit_guard`] of this thread
        /// maps a shared region and [`new_session`] takes a part of it.
        static SLOT_BACKEND: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static SLOT_REGION: std::cell::RefCell<Option<crate::jit_region::SharedJitRegion>> =
            const { std::cell::RefCell::new(None) };
    }

    /// Holds the process-wide JIT test lock and, on the slot backend, the shared
    /// region. The region is dropped before the lock is released, so no other
    /// test can see the process lease taken.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) struct JitGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    impl Drop for JitGuard {
        fn drop(&mut self) {
            SLOT_REGION.with(|cell| cell.borrow_mut().take());
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn jit_guard() -> JitGuard {
        let lock = crate::tests::JIT_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if SLOT_BACKEND.with(|flag| flag.get()) {
            let region = crate::jit_region::SharedJitRegion::new(SLOT_BACKEND_PARTS, 2)
                .expect("the slot backend maps the process region");
            SLOT_REGION.with(|cell| *cell.borrow_mut() = Some(region));
        }
        JitGuard { _lock: lock }
    }

    /// A session of the test: a slot of the guard's shared region on the slot
    /// backend, a region of its own otherwise.
    pub(crate) fn new_session() -> PagedFetchedLoadSession {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let part = SLOT_REGION.with(|cell| {
                cell.borrow()
                    .as_ref()
                    .map(|region| region.take_part().expect("a part is left"))
            });
            if let Some(part) = part {
                return PagedFetchedLoadSession::with_region_part(part);
            }
        }
        PagedFetchedLoadSession::new()
    }

    /// Runs `test` with the slot backend: every session of it publishes into a
    /// slot of one shared region (S6-42). The test must take [`jit_guard`] first.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(crate) fn with_slot_backend(test: impl FnOnce()) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                SLOT_BACKEND.with(|flag| flag.set(false));
            }
        }
        SLOT_BACKEND.with(|flag| flag.set(true));
        let _reset = Reset;
        test();
    }

    /// Runs the loop to `budget` and the interpreter to the same budget over
    /// identical RAM; asserts equal registers, RAM, retirement and write lists.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn assert_loop_equals_reference(budget: u64) -> (PagedHostLoopReport, u64) {
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), budget, true);
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            budget,
        );
        assert_eq!(run.report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!(
            run.report.retired_total(),
            reference_run.retired,
            "budget {budget}"
        );
        assert!(run.report.budget_is_consistent(), "budget {budget}");
        assert_eq!(
            observe(&state, &mut bus, true),
            reference_run.observation,
            "budget {budget}"
        );
        assert_eq!(run.writes, reference_run.writes, "budget {budget}");
        (run.report, session.admissions())
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_mixed_program_with_a_guest_loop_matches_the_interpreter_on_both_paths() {
        let _guard = jit_guard();
        let (report, admissions) = assert_loop_equals_reference(103);
        // 103 instructions = 25 turns of (load, store, push, ret) and a load,
        // a store and a push: 52 on the native path, 51 on the interpreter.
        assert_eq!(report.retired_jit, 52);
        assert_eq!(report.retired_interpreter, 51);
        assert_eq!(report.steps_jit, 52);
        // One admission per native step (the one-instruction block is
        // compiled again for every native step).
        assert_eq!(admissions, 52);
        assert_eq!(report.steps_interpreter, 51);
        assert_eq!(report.jit_refusals, 51);
        assert!(report.retired_jit > 0 && report.retired_interpreter > 0);
        assert_eq!(report.native_stop_exits, 0);
        assert_eq!(report.stop_byte_observations, 0);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_budgets_zero_to_n_on_the_mixed_program_equal_the_interpreter() {
        let _guard = jit_guard();
        for budget in 0..=13 {
            let (report, _) = assert_loop_equals_reference(budget);
            assert_eq!(report.remaining_budget, 0);
            assert_eq!(report.retired_jit + report.retired_interpreter, budget);
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_flag_raised_before_the_reread_runs_no_step() {
        let _guard = jit_guard();
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let before = observe(&state, &mut bus, false);
        let mut rflags_defined = RFLAGS_DEFINED;
        control.request_stop();
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            50,
        );
        assert_eq!(run.report.exit, PagedHostLoopExit::StopRequested);
        assert_eq!(run.report.retired_total(), 0);
        assert_eq!(run.report.remaining_budget, 50);
        assert_eq!(run.report.stop_byte_observations, 1);
        assert_eq!(run.report.retired_at_stop_byte, Some(0));
        assert!(run.writes.is_empty());
        assert!(!session.has_admitted_block() && session.admissions() == 0);
        assert_eq!(observe(&state, &mut bus, false), before);
        // The loop took the byte down and kept the flag: the requester clears it.
        assert_eq!(control.stop_byte().load(Ordering::SeqCst), 0);
        let again = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            50,
        );
        assert_eq!(again.report.exit, PagedHostLoopExit::StopRequested);
        assert_eq!(again.report.stop_byte_observations, 0);
        assert_eq!(again.report.retired_total(), 0);
        // finish is also a request flag; STOP wins over it.
        control.clear_stop_request();
        control.request_finish();
        let finish = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            50,
        );
        assert_eq!(finish.report.exit, PagedHostLoopExit::FinishRequested);
        assert_eq!(finish.report.retired_total(), 0);
        control.request_stop();
        let both = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            50,
        );
        assert_eq!(both.report.exit, PagedHostLoopExit::StopRequested);
    }

    /// STOP raised in the window the protocol leaves open: after the loop took the
    /// byte down and reread the flags, before the step. The step the byte reaches
    /// is the native one (`mov rax,[rbx]`, step 0) or the interpreter one (`push`,
    /// step 2; the interpreter never polls the byte).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_stop_after_the_reread_is_a_normal_exit_without_replay() {
        let _guard = jit_guard();
        for (raise_at_step, expected_native_exits, expected_retired) in
            [(0u64, 1u64, 0u64), (1, 1, 0), (2, 0, 1), (3, 0, 1)]
        {
            let mut session = new_session();
            let control = PagedHostLoopControl::new();
            let mut bus = make_bus(&LOOP_JIT_STORE);
            let mut state = loop_state();
            let mut rflags_defined = RFLAGS_DEFINED;
            let mut writes = Vec::new();
            let report = run_paged_host_loop_with_hook(
                &mut session,
                &control,
                config(),
                &mut state,
                &mut bus,
                &mut rflags_defined,
                1000,
                |range| writes.push((range.start().get(), range.len())),
                |control, step| {
                    if step == raise_at_step {
                        control.request_stop();
                    }
                },
            );
            assert_eq!(
                report.exit,
                PagedHostLoopExit::StopRequested,
                "{raise_at_step}"
            );
            // Steps 0 and 1 are native (load, store): the byte is seen before
            // the instruction (retired 0, no effect). Steps 2 and 3 are the
            // interpreter's: the instruction retires, the next iteration stops.
            assert_eq!(
                report.native_stop_exits, expected_native_exits,
                "{raise_at_step}"
            );
            assert_eq!(
                report.retired_total(),
                raise_at_step + expected_retired,
                "{raise_at_step}"
            );
            assert!(report.budget_is_consistent());
            assert_eq!(report.stop_byte_observations, 1);
            let retired = report.retired_total();
            let reference_run = reference(&LOOP_ALIAS, &loop_state(), retired, true);
            let actual = observe(&state, &mut bus, true);
            if raise_at_step == 0 {
                // Retired 0: the reference has not fetched. The native block did
                // fetch before it saw the byte: exactly the Accessed bits of the
                // four fetch-walk qwords differ, nothing else (no data walk, no
                // Dirty bit, no data change of the not-run load).
                for address in FETCH_WALK_QWORDS {
                    let index = (address / 8) as usize;
                    assert_eq!(
                        actual.ram[index],
                        reference_run.observation.ram[index] | ACCESSED,
                        "fetch walk qword {address:#x}"
                    );
                }
                assert_same(
                    &without_fetch_accessed_bits(actual),
                    &reference_run.observation,
                    "stop at step 0",
                );
                // The write list is the fetch walk's Accessed writes and nothing
                // else: no data-walk write of the load.
                assert!(reference_run.writes.is_empty());
                assert_eq!(
                    writes,
                    FETCH_WALK_QWORDS.map(|address| (address, 8)).to_vec(),
                    "stop at step 0: writes"
                );
            } else {
                // From step 1 on the code page was fetched by an earlier step:
                // no normalization, the state and the walk-write list equal the
                // interpreter's after the same number of retired instructions.
                // At step 1 the native store did not run: no data walk and no
                // Dirty bit of its own (the data-walk qwords are compared).
                assert_same(
                    &actual,
                    &reference_run.observation,
                    &format!("stop at step {raise_at_step}"),
                );
                assert_eq!(
                    writes, reference_run.writes,
                    "stop at step {raise_at_step}: writes"
                );
            }

            // Resume after the requester withdrew the flag: no instruction is
            // repeated or skipped, the rest equals the interpreter's run.
            control.clear_stop_request();
            let resumed = run_loop(
                &mut session,
                &control,
                &mut bus,
                &mut state,
                &mut rflags_defined,
                37,
            );
            assert_eq!(resumed.report.exit, PagedHostLoopExit::BudgetExhausted);
            let total = reference(&LOOP_ALIAS, &loop_state(), retired + 37, true);
            assert_eq!(observe(&state, &mut bus, true), total.observation);
        }
    }

    /// The not-run native store must leave no trace: the value stored differs from
    /// the qword in memory (`rax` != `INITIAL_DATA`), so a store that ran before
    /// the exit would change the data, the Dirty bit of the data PTE and the write
    /// list. `StopRequested` with retired 0 shows none of them; the resume then
    /// runs the store once.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_stop_before_a_native_store_with_a_new_value_leaves_no_data_trace() {
        const NEW_VALUE: u64 = 0xdead_beef_0bad_f00d;
        const DATA_PTE: u64 = 0x7000 + 8 * ((DATA_LINEAR >> 12) & 0x1ff);
        let _guard = jit_guard();
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        // `mov [rbx],rax` as the first instruction.
        let mut bus = make_bus(&[0x48, 0x89, 0x03]);
        let mut state = make_state(NEW_VALUE, DATA_LINEAR, 0);
        let pte_before = bus
            .read_physical(GuestPhysicalAddress::new(DATA_PTE), AccessWidth::Qword)
            .unwrap();
        let mut rflags_defined = RFLAGS_DEFINED;
        let mut writes = Vec::new();
        let report = run_paged_host_loop_with_hook(
            &mut session,
            &control,
            config(),
            &mut state,
            &mut bus,
            &mut rflags_defined,
            1,
            |range| writes.push((range.start().get(), range.len())),
            |control, step| {
                if step == 0 {
                    control.request_stop();
                }
            },
        );
        assert_eq!(report.exit, PagedHostLoopExit::StopRequested);
        assert_eq!(report.retired_total(), 0);
        assert_eq!(report.native_stop_exits, 1);
        assert_eq!(state.rip, CODE_RIP);
        let read = |bus: &mut MachineBus, address: u64| {
            bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                .unwrap()
        };
        assert_eq!(read(&mut bus, DATA_QWORD), INITIAL_DATA);
        assert_eq!(
            read(&mut bus, DATA_PTE),
            pte_before,
            "no data walk, no D bit"
        );
        assert_eq!(
            writes,
            FETCH_WALK_QWORDS.map(|address| (address, 8)).to_vec(),
            "only the Accessed bits of the fetch walk"
        );

        control.clear_stop_request();
        let resumed = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            1,
        );
        assert_eq!(resumed.report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!(resumed.report.retired_jit, 1);
        assert_eq!(read(&mut bus, DATA_QWORD), NEW_VALUE);
        assert_eq!(
            read(&mut bus, DATA_PTE) & 0x60,
            0x60,
            "A and D after the store"
        );
        assert_eq!(state.rip, CODE_RIP + 3);
    }

    /// A request that was raised and withdrawn before the loop looked leaves a
    /// byte without a flag: harmless (S6-40 S4), nothing is replayed.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_a_byte_without_a_flag_is_harmless() {
        let _guard = jit_guard();
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        let report = run_paged_host_loop_with_hook(
            &mut session,
            &control,
            config(),
            &mut state,
            &mut bus,
            &mut rflags_defined,
            12,
            |_| {},
            |control, step| {
                if step == 0 {
                    control.request_stop();
                    control.clear_stop_request();
                }
            },
        );
        // The native block saw the byte before the instruction and exited with
        // retired 0; the loop went back to the safe point, saw no flag and ran
        // the same instruction once.
        assert_eq!(report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!(report.native_stop_exits, 1);
        // The next safe point took the leftover byte down and found no flag.
        assert_eq!(report.stop_byte_observations, 1);
        assert_eq!(report.retired_at_stop_byte, Some(0));
        assert_eq!(report.retired_total(), 12);
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), 12, true);
        assert_same(
            &observe(&state, &mut bus, true),
            &reference_run.observation,
            "state",
        );
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_external_stop_from_another_thread_in_an_endless_guest_loop() {
        use std::sync::atomic::AtomicU64;

        let _guard = jit_guard();
        for round in 0..3u64 {
            let mut session = new_session();
            let control = PagedHostLoopControl::new();
            let progress = AtomicU64::new(0);
            let mut bus = make_bus(&LOOP_JIT_STORE);
            let mut state = loop_state();
            let mut rflags_defined = RFLAGS_DEFINED;
            let threshold = 150 + 331 * round;
            // Set when the loop call ended (also by a panic), so the requester
            // never waits for a threshold the loop will not reach.
            let loop_done = AtomicBool::new(false);
            struct SetOnDrop<'a>(&'a AtomicBool);
            impl Drop for SetOnDrop<'_> {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let progress_after_request = AtomicU64::new(0);
            let report = std::thread::scope(|scope| {
                scope.spawn(|| {
                    while progress.load(Ordering::SeqCst) < threshold {
                        if loop_done.load(Ordering::SeqCst) {
                            return;
                        }
                        std::thread::yield_now();
                    }
                    control.request_stop();
                    progress_after_request.store(progress.load(Ordering::SeqCst), Ordering::SeqCst);
                });
                let _done = SetOnDrop(&loop_done);
                run_paged_host_loop_with_hook(
                    &mut session,
                    &control,
                    config(),
                    &mut state,
                    &mut bus,
                    &mut rflags_defined,
                    u64::MAX,
                    |_| {},
                    |_, step| progress.store(step, Ordering::SeqCst),
                )
            });
            assert_eq!(report.exit, PagedHostLoopExit::StopRequested);
            assert!(report.budget_is_consistent());
            assert!(report.retired_total() >= threshold);
            assert!(report.retired_jit > 0 && report.retired_interpreter > 0);
            // The requester read `progress` (steps started) right after
            // `request_stop`: the loop is at most in the step it started before
            // the flag was written, and it may have read the flag as false
            // just before the write and then run one more step, so
            // retired <= progress + 2. The exact bound (no instruction after
            // the iteration that took the byte down) is proved by the protocol
            // and by the deterministic hook tests above, not by this test.
            let retired = report.retired_total();
            let at_request = progress_after_request.load(Ordering::SeqCst);
            assert!(
                retired <= at_request + 2,
                "retired {retired}, progress at request {at_request}"
            );
            let at_stop = reference(&LOOP_ALIAS, &loop_state(), retired, true);
            assert_eq!(observe(&state, &mut bus, true), at_stop.observation);

            // The requester resumes: clear the flag; a leftover byte is harmless.
            control.clear_stop_request();
            let resumed = run_loop(
                &mut session,
                &control,
                &mut bus,
                &mut state,
                &mut rflags_defined,
                41,
            );
            assert_eq!(resumed.report.exit, PagedHostLoopExit::BudgetExhausted);
            assert_eq!(resumed.report.retired_total(), 41);
            let total = reference(&LOOP_ALIAS, &loop_state(), retired + 41, true);
            assert_eq!(observe(&state, &mut bus, true), total.observation);
        }
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_event_exits_on_the_safe_point_before_the_next_step_and_stop_wins() {
        let _guard = jit_guard();
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;

        // Raised during the third step: the three steps retire, the loop exits
        // on the safe point before the fourth.
        let report = run_paged_host_loop_with_hook(
            &mut session,
            &control,
            config(),
            &mut state,
            &mut bus,
            &mut rflags_defined,
            100,
            |_| {},
            |control, step| {
                if step == 2 {
                    control.request_event();
                }
            },
        );
        assert_eq!(report.exit, PagedHostLoopExit::EventPending);
        assert_eq!(report.retired_total(), 3);
        assert!(report.budget_is_consistent());
        assert_eq!(report.stop_byte_observations, 1);
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), 3, true);
        assert_same(
            &observe(&state, &mut bus, true),
            &reference_run.observation,
            "state",
        );

        // The loop does not clear the flag: the next call exits at once.
        let again = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            100,
        );
        assert_eq!(again.report.exit, PagedHostLoopExit::EventPending);
        assert_eq!(again.report.retired_total(), 0);
        assert_same(
            &observe(&state, &mut bus, true),
            &reference_run.observation,
            "state",
        );

        // A zero budget is reported before the event (as the dispatcher does).
        let zero = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            0,
        );
        assert_eq!(zero.report.exit, PagedHostLoopExit::BudgetExhausted);

        // STOP has priority over the pending event.
        control.request_stop();
        let stopped = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            100,
        );
        assert_eq!(stopped.report.exit, PagedHostLoopExit::StopRequested);
        assert_eq!(stopped.report.retired_total(), 0);

        // Both withdrawn: the guest resumes where it stopped.
        control.clear_stop_request();
        control.clear_event_request();
        let resumed = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            9,
        );
        assert_eq!(resumed.report.exit, PagedHostLoopExit::BudgetExhausted);
        let total = reference(&LOOP_ALIAS, &loop_state(), 12, true);
        assert_eq!(observe(&state, &mut bus, true), total.observation);
    }

    /// Code page writes by both paths, then the next lookup: the interpreter's
    /// store replaces a form the session had admitted (new bytes, new
    /// generation), and the native store `mov [rbx],rax` rewrites the
    /// instruction after it.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_code_page_write_by_the_interpreter_is_seen_by_the_next_lookup() {
        let _guard = jit_guard();
        // +0 mov [rbx],rcx (interpreter)   +3 mov rax,[rbx] (native, old form)
        // +6 hlt ...
        let code = [
            0x48, 0x89, 0x0b, 0x48, 0x8b, 0x03, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4,
        ];
        // Patch for +3: `mov eax,[rbx]` (native dword form), then hlt.
        let patch = u64::from_le_bytes([0x8b, 0x03, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4]);
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&code);
        let mut rflags_defined = RFLAGS_DEFINED;
        let code_page = GuestPhysicalAddress::new(CODE_PHYSICAL);

        // Phase A: the old native load at +3 runs once and stays admitted.
        let mut state = make_state(0, CODE_RIP + 3, patch);
        state.rip = CODE_RIP + 3;
        let a = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            1,
        );
        assert_eq!(a.report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!((a.report.steps_jit, a.report.steps_interpreter), (1, 0));
        assert!(session.has_admitted_block());
        assert_eq!(session.admissions(), 1);
        let old_qword = u64::from_le_bytes([0x48, 0x8b, 0x03, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4]);
        assert_eq!(state.gpr[Register::Rax as usize], old_qword);
        let generation_before = bus.code_page_generation(code_page).unwrap();

        // Phase B: the interpreter's store rewrites +3, the next lookup at +3
        // sees new bytes and a new generation and admits the dword form.
        state.rip = CODE_RIP;
        let b = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            20,
        );
        assert!(bus.code_page_generation(code_page).unwrap() > generation_before);
        assert_eq!(
            b.report.exit,
            PagedHostLoopExit::Interpreter(od_machine::PagedProbeExit::Halted)
        );
        assert_eq!(session.admissions(), 2);
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0xf4f4_038b,
            "the new dword form ran, not the old qword block"
        );
        assert!(b.report.steps_jit == 1 && b.report.steps_interpreter >= 1);
        assert!(b.report.budget_is_consistent());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_code_page_write_by_the_native_store_is_seen_by_the_next_lookup() {
        let _guard = jit_guard();
        // +0 mov [rbx],rax (native store)   +3 mov rax,[rbx] (old form)  +6 hlt
        let code = [
            0x48, 0x89, 0x03, 0x48, 0x8b, 0x03, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4,
        ];
        // Patch for +3: `mov eax,[rbx]` (native dword form), hlt. The rewritten
        // bytes load a dword; the old bytes would load all eight.
        let patch = u64::from_le_bytes([0x8b, 0x03, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4, 0xf4]);
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&code);
        let mut state = make_state(patch, CODE_RIP + 3, 0);
        let mut rflags_defined = RFLAGS_DEFINED;
        let code_page = GuestPhysicalAddress::new(CODE_PHYSICAL);
        let generation_before = bus.code_page_generation(code_page).unwrap();

        // The store block is retired by its own CodeInvalidated exit.
        let first = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            1,
        );
        assert_eq!(
            (first.report.steps_jit, first.report.steps_interpreter),
            (1, 0)
        );
        assert!(session.admitted.as_ref().is_some_and(|block| block.retired));
        assert!(bus.code_page_generation(code_page).unwrap() > generation_before);
        assert_eq!(state.rip, CODE_RIP + 3);

        let rest = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            20,
        );
        assert_eq!(
            rest.report.exit,
            PagedHostLoopExit::Interpreter(od_machine::PagedProbeExit::Halted)
        );
        assert_eq!(
            state.gpr[Register::Rax as usize],
            0xf4f4_038b,
            "the rewritten bytes ran, not the old `mov rax,[rbx]`"
        );
        // The old form at +3 was never admitted: the store, then the new form.
        assert_eq!(session.admissions(), 2);
        assert_eq!(rest.report.steps_jit, 1);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_guest_fault_of_the_native_form_equals_the_interpreters() {
        let _guard = jit_guard();
        let unmapped = 0x0000_0080_0000_2000;
        let mut state = loop_state();
        state.gpr[Register::Rbx as usize] = unmapped;
        let reference_run = reference(&LOOP_ALIAS, &state, 5, true);
        let od_machine::PagedProbeExit::Fault(fault) = &reference_run.exit else {
            panic!("expected a fault, got {:?}", reference_run.exit);
        };
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut rflags_defined = RFLAGS_DEFINED;
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            5,
        );
        assert_eq!(
            run.report.exit,
            PagedHostLoopExit::GuestFault(fault.clone())
        );
        assert_eq!(run.report.page_fault, reference_run.page_fault);
        assert_eq!(run.report.retired_total(), reference_run.retired);
        assert_eq!(run.report.steps_jit, 0);
        assert_same(
            &observe(&state, &mut bus, true),
            &reference_run.observation,
            "state",
        );
        assert_eq!(run.writes, reference_run.writes);
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn s3_38o_the_map_jit_lease_ends_with_the_session_and_a_new_loop_runs() {
        use crate::{MapJitLease, map_jit_region_live};

        let _guard = jit_guard();
        let control = PagedHostLoopControl::new();
        assert!(!map_jit_region_live());
        let mut session = PagedFetchedLoadSession::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        // Ends right after a native step: the block is resident.
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            1,
        );
        assert_eq!(run.report.steps_jit, 1);
        assert!(session.has_admitted_block() && map_jit_region_live());
        // The loop holds nothing: the lease belongs to the session, so a second
        // owner is refused while it lives and accepted after the drop.
        assert!(matches!(
            MapJitLease::acquire(),
            Err(crate::ExecutableMemoryError::MappingAlreadyLive)
        ));
        drop(session);
        assert!(!map_jit_region_live());
        drop(MapJitLease::acquire().expect("the lease is free after the session dropped"));

        // A new session and a new loop work.
        let mut session = PagedFetchedLoadSession::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            9,
        );
        assert_eq!(run.report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!(run.report.retired_total(), 9);
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), 9, true);
        assert_same(
            &observe(&state, &mut bus, true),
            &reference_run.observation,
            "state",
        );
        // The ninth instruction is a native load: the block is resident, and a
        // reset on the safe point after the loop releases the region too.
        assert!(session.has_admitted_block());
        assert!(session.reset());
        assert!(!map_jit_region_live());
    }

    #[test]
    fn s3_38o_invalid_rflags_definedness_runs_nothing() {
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_ALIAS);
        let mut state = loop_state();
        let before = observe(&state, &mut bus, false);
        let mut rflags_defined = !0;
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            5,
        );
        assert_eq!(
            run.report.exit,
            PagedHostLoopExit::InvalidRflagsDefined { value: !0 }
        );
        assert_eq!(run.report.retired_total(), 0);
        assert_eq!(observe(&state, &mut bus, false), before);
    }

    /// Where the native path is unavailable the session refuses every admission
    /// and the loop runs the shared interpreter only (an explicit skip of the
    /// native checks): same state as the pure interpreter, no native step.
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    #[test]
    fn s3_38o_without_native_support_the_loop_is_interpreter_only() {
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), 40, false);
        let mut session = new_session();
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_ALIAS);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        let run = run_loop(
            &mut session,
            &control,
            &mut bus,
            &mut state,
            &mut rflags_defined,
            40,
        );
        assert_eq!(run.report.exit, PagedHostLoopExit::BudgetExhausted);
        assert_eq!(run.report.steps_jit, 0);
        assert_eq!(run.report.steps_interpreter, 40);
        assert_eq!(observe(&state, &mut bus, false), reference_run.observation);
    }

    /// Hold the first false poll until another thread posts the native byte.
    /// The host loop must act on the new request through native execution.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    fn run_after_stop_is_posted(
        quantum: &crate::host_threads::JitHostQuantum,
        mut call: od_machine::HostQuantumCall<'_>,
        at_poll: &AtomicBool,
        timeout: std::time::Duration,
    ) -> Result<od_machine::HostQuantumResult, &'static str> {
        let byte = call
            .native_stop
            .expect("the worker supplies a native STOP byte");
        let poll = &mut *call.stop;
        let mut delay_first_poll = || {
            let requested = poll();
            if !requested && !at_poll.swap(true, Ordering::SeqCst) {
                let until = std::time::Instant::now() + timeout;
                while byte.load(Ordering::SeqCst) == 0 {
                    assert!(
                        std::time::Instant::now() < until,
                        "native STOP was not posted"
                    );
                    std::thread::yield_now();
                }
            }
            requested
        };
        quantum.run(od_machine::HostQuantumCall {
            stop: &mut delay_first_poll,
            board_irq: call.board_irq.reborrow(),
            ..call
        })
    }

    /// S6-42: tests of several vCPU threads over the slot backend and the
    /// cross-vCPU invalidation (child module: it uses the helpers above).
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    mod s6_42;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    mod s6_42b;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    mod s6_42c;
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    mod s6_43a;
}
