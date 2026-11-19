//! The parallel mode: one host thread per vCPU, over a shared machine (S6-34).
//!
//! [`run_threads`] splits a board into what the threads share (the bus: RAM without a lock, devices
//! behind their one lock, the Local APICs, the clock owner behind a lock, the COM1 log) and the
//! vCPUs (each thread owns exactly one, behind a mutex that the thread holds only while the vCPU is
//! running). Every thread runs the loop below for its vCPU; the caller's thread is the controller
//! ([`ParallelControl`]): it can stop every vCPU, look at the stopped machine, resume it, and end
//! the run. Nothing here is an asynchronous runtime, and no thread is ever stopped from outside in
//! the middle of an instruction: every stop is requested through an atomic that the interpreter
//! polls between instructions.
//!
//! **One pass of a vCPU thread.** (1) The barrier: end of run, stop-all request, or another vCPU's
//! split `LOCK` make the thread stand still (a stop is acknowledged here); (2) the thread takes
//! its vCPU, applies the INIT and SIPI waiting in its own Local APIC, then its NMI (the boundary of
//! SMP_CONTRACT.md 3, as the interleaved scheduler does for every vCPU); (3) `Running`: one call of
//! the interpreter loop for at most the quantum (cut by the nearest timer deadline and by the
//! shared budget), then the retired instructions and device time are charged to the clock owner;
//! `Halted`: woken when IF = 1 and a vector is acceptable, else the thread sleeps; `WaitForSipi` and
//! `Shutdown`: the thread sleeps.
//!
//! **Kick and sleep.** A thread that has nothing to run parks (`thread::park`): it does not spin.
//! Whatever gives its vCPU work posts it in the vCPU's Local APIC, and the APIC calls the wake
//! sink ([`crate::apic_pair::WakeSink`]) under its lock: the sink sets the vCPU's `kick` flag and
//! unparks its thread. The thread clears the flag BEFORE it looks at its APIC and sleeps only if
//! the flag is still clear: an event posted after the look sets the flag after the clear, so the
//! sleep ends at once; an event posted before the look is seen by it. A wake-up between the check
//! and the park is the park token of `unpark`: it is not lost. `park` may return without a kick;
//! the loop checks the flag again.
//!
//! **Stop.** [`ParallelControl::stop_all`] sets the request, kicks the vCPUs that have not yet
//! acknowledged, and waits (with a deadline) for the acknowledgement of each. A thread
//! acknowledges at the barrier (a completed instruction, or the restart point of a partly executed
//! `REP`: the interpreter returns from the call on the poll between instructions) or when it goes to
//! sleep: a vCPU asleep in `HLT` or waiting for SIPI acknowledges at once and stays asleep, with no
//! guest instruction. The acknowledgement waits for no lock held by a stopped or sleeping vCPU:
//! no thread holds a device lock, the APIC lock or the clock owner's lock across `HLT`, the wait for
//! SIPI, a sleep or a barrier (MACHINE_CONTRACT.md, lock order, rule 3). A sleeper that is kicked
//! (an IPI arrived) and a thread that comes out of the sleep look at the stop request first, so a
//! stopped machine does not start a vCPU whatever arrives: the request stays in the Local APIC.
//!
//! **Order.** The acknowledgement is `Release` (`SeqCst`) after every write the thread made to its
//! vCPU (the mutex of the vCPU is released first), to RAM and to the devices; the controller reads
//! it with `Acquire` (`SeqCst`) and takes the vCPU's mutex: it sees all of them. The resume is the
//! same in the other direction (the controller's writes, then the request cleared with `Release`;
//! the thread reads the request with `Acquire`). The handshake that decides who stands still
//! (a thread clears its `safe` and `quiet` flags and only then reads the requests; a requester sets
//! its request and only then reads the flags) is `SeqCst` on both sides: Dekker's pattern needs the
//! single total order.
//!
//! **Time.** Virtual time stands while a stop is in force: a thread charges the clock only inside
//! its pass, before its acknowledgement, and the idle jump takes the clock owner's lock and reads
//! the stop request under it; the controller takes that lock once after the acknowledgements.
//! In motion the clock is moved only by the clock owner (D7, S6-15/S6-21), which is one object
//! behind a lock: a thread calls `account_retired`/`account_device_ns` after its call, and the idle
//! jump (`idle_step`, one jump per decision) is taken by the thread that goes to sleep last, when no
//! other thread is awake (S6-34 decision, SMP_CONTRACT.md 8).
//!
//! **Split `LOCK`** (step 8): see [`SharedState::split_acquire`].

// The module is exercised by the tests and by the probe session of S6-34; no other production
// caller yet.
#![cfg_attr(not(test), allow(dead_code))]

use crate::bus::SplitLockGate;
use crate::clock_owner::{ClockOwner, IdleOutcome};
use crate::firmware_board::vcpu_quantum::ThreadQuantumEnv;
use crate::firmware_board::{
    FirmwareBoardError, LoadedFirmwareBoard, ThreadParts, deliver_pending_nmi_of,
};
use crate::idle_arbiter::{SchedActivity, WakeView};
use crate::irq_source::{BoardIrqSource, NoIrqSource};
use crate::smp_scheduler::{Executed, RunFailure, StopAck};
use crate::vcpu::{BOOT_VCPU_INDEX, NmiBoundary, Vcpu};
use crate::{ExecutionMode, LocalApicHandle, MachineStopReason, PhysicalMemory};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

const SEQ: Ordering = Ordering::SeqCst;
/// `split_owner` while no vCPU holds the split-lock gate.
const NO_OWNER: usize = usize::MAX;

/// The inputs of a parallel run.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ParallelConfig {
    /// Steps of one call of the interpreter loop of a thread (at least 1), before the cut by the
    /// nearest timer deadline and by the budget.
    pub(crate) quantum: u64,
    /// Retired instructions of all vCPUs together after which the run ends.
    pub(crate) budget: u64,
    /// How long a vCPU waits for the other vCPUs to stand still around a split `LOCK` before the
    /// instruction fails (no effect).
    pub(crate) split_timeout: Duration,
    /// End the run when nothing can run and nothing can wake (the interleaved scheduler's
    /// `Finished::AllIdle`). `false`: the threads sleep and the run goes on until the controller
    /// ends it or something from outside (an IPI posted by the controller, a device) wakes a
    /// vCPU. The default is `true`.
    pub(crate) end_when_idle: bool,
    /// Test injection: called by a thread at the very start of its sleep, after it has looked at
    /// its Local APIC and decided to sleep and before it parks, with the index of its vCPU. The
    /// tests hold the thread there and post an event, which is the window of the lost wake-up.
    #[cfg(test)]
    pub(crate) pre_sleep: Option<fn(usize)>,
    /// Test injection (S6-36, the window of the barrier of S6-34 review m2): called by a thread
    /// in the barrier right after it has lowered its `safe` and `quiet` flags and before it reads
    /// the requests and raises them again. A thread that finds `safe` up and the stop in force
    /// does not lower anything and never gets here (the point of the test).
    #[cfg(test)]
    pub(crate) barrier_window: Option<fn(usize)>,
    /// Test injection (S6-36 review, M-1): called by a thread in the barrier that has read
    /// "not standing" (`safe` and the stop not both up) and has not yet lowered its flags. The
    /// tests hold the thread there, let a stop-all be given its proof (the thread's `safe` is
    /// still up from before), and then release it: it lowers `safe` and raises it again, which
    /// is the residual window that `take_parallel` waits out.
    #[cfg(test)]
    pub(crate) standing_window: Option<fn(usize)>,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            quantum: crate::smp_scheduler::DEFAULT_QUANTUM,
            budget: u64::MAX,
            split_timeout: Duration::from_secs(30),
            end_when_idle: true,
            #[cfg(test)]
            pre_sleep: None,
            #[cfg(test)]
            barrier_window: None,
            #[cfg(test)]
            standing_window: None,
        }
    }
}

/// Why a parallel run ended.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ParallelFinish {
    /// The controller asked for the end (or its closure returned).
    Stopped,
    BudgetExhausted,
    GuestStop(MachineStopReason),
    /// Every vCPU waits for SIPI, has shut down, or is halted with nothing that could wake it.
    AllIdle,
    /// A vCPU's host call, boundary or clock charge failed; every thread was stopped.
    Error {
        cpu: usize,
        error: FirmwareBoardError,
    },
    /// The thread of vCPU `cpu` panicked; every thread was stopped.
    Panicked {
        cpu: usize,
    },
}

impl ParallelFinish {
    fn priority(&self) -> u8 {
        match self {
            Self::Stopped | Self::BudgetExhausted | Self::AllIdle => 0,
            Self::GuestStop(_) => 1,
            Self::Error { .. } | Self::Panicked { .. } => 2,
        }
    }
}

/// What one vCPU's thread did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ParallelCpuReport {
    pub(crate) retired: u64,
    /// Calls of the interpreter loop (the quantum calls).
    pub(crate) calls: u64,
    pub(crate) wakes: u64,
    pub(crate) init_applied: u64,
    pub(crate) started: u64,
    pub(crate) nmi_delivered: u64,
    /// How many times the thread parked to sleep.
    pub(crate) sleeps: u64,
    /// How many times the vCPU held the split-lock gate (split `LOCK` operations executed).
    pub(crate) split_locks: u64,
    /// How many times the vCPU asked for the gate while another vCPU held it or won the exchange,
    /// stood still before any effect of its instruction and asked again (the loser of S6-34 step 8).
    pub(crate) split_losses: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) struct ParallelReport {
    pub(crate) finished: ParallelFinish,
    pub(crate) cpus: Vec<ParallelCpuReport>,
    /// Virtual nanoseconds the idle jumps skipped, and the number of jumps.
    pub(crate) idle_ns: u64,
    pub(crate) idle_jumps: u64,
    /// Threads started and threads that ran to their end: both are the number of vCPUs when the
    /// run returns (every thread has been joined; none is left running).
    pub(crate) threads_started: usize,
    pub(crate) threads_exited: usize,
}

/// The proof of a stop of every vCPU (SMP_CONTRACT.md 9).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct StopProof {
    pub(crate) acks: Vec<StopAck>,
    /// The virtual time at which the machine stood still.
    pub(crate) now_ns: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum ParallelStopError {
    /// These vCPUs (index order) did not reach a safe point in time. The request was withdrawn and
    /// every vCPU went on with what it had: no state, pending event or deadline changed.
    Timeout { unacked: Vec<usize> },
    /// The run is already over.
    Finished,
}

/// One kick cell: the flag the sleeping thread watches and the thread to unpark.
struct KickCell {
    flag: AtomicBool,
    native_stop: Option<AtomicU8>,
    thread: OnceLock<Thread>,
}

/// The kicks of all vCPUs. `'static` (behind an `Arc`) because the wake sink of the Local APICs
/// holds it.
struct Kicks {
    cells: Vec<KickCell>,
}

impl Kicks {
    fn new(count: usize, native_stop: bool) -> Self {
        Self {
            cells: (0..count)
                .map(|_| KickCell {
                    flag: AtomicBool::new(false),
                    native_stop: native_stop.then(|| AtomicU8::new(0)),
                    thread: OnceLock::new(),
                })
                .collect(),
        }
    }

    /// Kick vCPU `cpu`: publish the event flag, then the native STOP byte, then unpark.
    /// Called under the APIC lock by the wake sink: takes no lock.
    fn kick(&self, cpu: usize) {
        let cell = &self.cells[cpu];
        cell.flag.store(true, SEQ);
        if let Some(byte) = &cell.native_stop {
            byte.store(1, SEQ);
        }
        if let Some(thread) = cell.thread.get() {
            thread.unpark();
        }
    }

    fn kick_all(&self) {
        for cpu in 0..self.cells.len() {
            self.kick(cpu);
        }
    }

    /// Unpark without a kick (a thread waits for a condition other than its flag).
    fn unpark(&self, cpu: usize) {
        if let Some(thread) = self.cells[cpu].thread.get() {
            thread.unpark();
        }
    }

    fn flag(&self, cpu: usize) -> bool {
        self.cells[cpu].flag.load(SEQ)
    }
}

fn encode_activity(activity: SchedActivity) -> u8 {
    match activity {
        SchedActivity::Running => 0,
        SchedActivity::Halted => 1,
        SchedActivity::WaitForSipi => 2,
        SchedActivity::Shutdown => 3,
    }
}

fn decode_activity(code: u8) -> SchedActivity {
    match code {
        0 => SchedActivity::Running,
        1 => SchedActivity::Halted,
        2 => SchedActivity::WaitForSipi,
        _ => SchedActivity::Shutdown,
    }
}

/// What the other threads (and the controller) may know of a vCPU between its passes: published by
/// the owner thread at every change, read by the idle arbiter and by the stop.
struct Cell {
    /// The thread is doing something other than sleeping or standing still. The idle arbiter
    /// concludes "nobody runs" only when no thread is busy and no kick is pending.
    busy: AtomicBool,
    activity: AtomicU8,
    if_set: AtomicBool,
    /// The vCPU stands on the restart point of a `REP` (its last call ended inside one).
    rep_restart: AtomicBool,
    /// The thread has acknowledged a stop: the vCPU's state is quiescent and its mutex is free.
    safe: AtomicBool,
    /// The thread touches no RAM: it stands on a boundary, sleeps, or waits inside its own
    /// `split_acquire` before any effect of its instruction.
    quiet: AtomicBool,
    /// A call on an NVMe board may submit device work, or already owns an unfinished drain.
    nvme_work: AtomicBool,
    retired: AtomicU64,
    calls: AtomicU64,
    wakes: AtomicU64,
    init_applied: AtomicU64,
    started: AtomicU64,
    nmi_delivered: AtomicU64,
    sleeps: AtomicU64,
    split_locks: AtomicU64,
    split_losses: AtomicU64,
}

impl Cell {
    fn new() -> Self {
        Self {
            busy: AtomicBool::new(true),
            activity: AtomicU8::new(0),
            if_set: AtomicBool::new(false),
            rep_restart: AtomicBool::new(false),
            safe: AtomicBool::new(false),
            quiet: AtomicBool::new(false),
            nvme_work: AtomicBool::new(false),
            retired: AtomicU64::new(0),
            calls: AtomicU64::new(0),
            wakes: AtomicU64::new(0),
            init_applied: AtomicU64::new(0),
            started: AtomicU64::new(0),
            nmi_delivered: AtomicU64::new(0),
            sleeps: AtomicU64::new(0),
            split_locks: AtomicU64::new(0),
            split_losses: AtomicU64::new(0),
        }
    }

    fn report(&self) -> ParallelCpuReport {
        ParallelCpuReport {
            retired: self.retired.load(SEQ),
            calls: self.calls.load(SEQ),
            wakes: self.wakes.load(SEQ),
            init_applied: self.init_applied.load(SEQ),
            started: self.started.load(SEQ),
            nmi_delivered: self.nmi_delivered.load(SEQ),
            sleeps: self.sleeps.load(SEQ),
            split_locks: self.split_locks.load(SEQ),
            split_losses: self.split_losses.load(SEQ),
        }
    }
}

/// What a quantum function of the run is handed.
pub(crate) struct QuantumCall<'a, 'b> {
    pub(crate) env: &'a ThreadQuantumEnv<'b>,
    pub(crate) vcpu: &'a mut Vcpu,
    pub(crate) cpu: usize,
    pub(crate) lapic: &'a LocalApicHandle,
    /// The interrupt source of the board for the boot processor; an inert source for every other
    /// vCPU.
    pub(crate) source: &'a mut dyn BoardIrqSource,
    pub(crate) gate: &'a dyn SplitLockGate,
    pub(crate) budget: u64,
    pub(crate) consecutive_faults: &'a mut u64,
    pub(crate) stop: &'a mut dyn FnMut() -> bool,
    pub(crate) native_stop: Option<&'a AtomicU8>,
    /// A guest machine stop requested by the host quantum, published after its charge.
    pub(crate) machine_stop: &'a mut Option<MachineStopReason>,
    /// Pending service for an NVMe-equipped run. A pending call services the device without
    /// executing guest instructions, even when the guest budget is zero.
    pub(crate) nvme_pending: Option<&'a mut bool>,
}

/// One quantum of a vCPU: the caller builds its CPL0 set and calls
/// [`run_vcpu_set_quantum_shared`] with the parts of the call (the set borrows local profile
/// values, so it cannot be built outside the call).
pub(crate) type QuantumFn<'f> =
    dyn Fn(QuantumCall<'_, '_>) -> Result<Executed, RunFailure<FirmwareBoardError>> + Sync + 'f;

/// What a vCPU thread runs on its own thread after its last quantum, before it is counted as
/// exited (the argument is the vCPU index): per-thread state that must be released by its owner
/// thread (and not by a `thread_local!` destructor, which `thread::scope` does not wait for).
pub(crate) type ThreadExitFn<'f> = dyn Fn(usize) + Sync + 'f;

/// The host quantum borrows the board source through this adapter. The source lock ends at
/// each method return, before the guest interrupt frame or an instruction can run.
struct ScopedBoardSource<'a, 'board>(&'a Mutex<&'board mut dyn BoardIrqSource>);

impl BoardIrqSource for ScopedBoardSource<'_, '_> {
    fn pending_vector(&self) -> Option<u8> {
        lock(self.0).pending_vector()
    }

    fn acknowledge(&mut self, vector: u8) {
        lock(self.0).acknowledge(vector);
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        lock(self.0).next_deadline_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        lock(self.0).pit_write_ends_quantum()
    }

    fn irq0_edges(&self) -> u64 {
        lock(self.0).irq0_edges()
    }

    fn wake_ns(&self) -> Option<u64> {
        lock(self.0).wake_ns()
    }
}

/// State shared by the threads and the controller.
struct SharedState<'a> {
    count: usize,
    config: ParallelConfig,
    /// A7 of S6-40: the boot processor locks the board interrupt source per operation during a
    /// host quantum. The ordinary interpreter quantum takes the lock across the call.
    source_free: bool,
    refresh_device_lines: bool,
    kicks: Arc<Kicks>,
    cells: Vec<Cell>,
    /// The controller asks every vCPU to stand still.
    stop_all: AtomicBool,
    /// The run is over: every thread ends at its next boundary.
    finish: AtomicBool,
    /// The first reason the run ended.
    reason: Mutex<Option<ParallelFinish>>,
    /// The vCPU that holds the split-lock gate, or [`NO_OWNER`].
    split_owner: AtomicUsize,
    controller: OnceLock<Thread>,
    /// The single clock owner (D7): the lock is above the APIC lock in the order (a charge takes
    /// the APIC lock under it); nothing takes it under the APIC lock.
    owner: Mutex<&'a mut ClockOwner>,
    /// The interrupt source of the board, used by the boot processor's thread for its quantum, its
    /// deadline and its wake test, and by the idle arbiter only when every thread is idle.
    irq: Mutex<&'a mut dyn BoardIrqSource>,
    pit_ioapic_source: Option<crate::pit_ioapic_irq_source::PitIoapicIrqSource>,
    /// Retired instructions left in the shared budget.
    remaining: AtomicU64,
    idle_ns: AtomicU64,
    idle_jumps: AtomicU64,
    started: AtomicUsize,
    exited: AtomicUsize,
    lapics: Vec<LocalApicHandle>,
    vcpus: Vec<Mutex<&'a mut Vcpu>>,
}

fn lock<T: ?Sized>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl SharedState<'_> {
    fn other_split(&self, cpu: usize) -> bool {
        let owner = self.split_owner.load(SEQ);
        owner != NO_OWNER && owner != cpu
    }

    /// Poll the run's requests. For a host quantum, a kick also returns to the worker so it
    /// can inspect startup and NMI before the next call.
    fn call_must_end(&self, cpu: usize) -> bool {
        self.finish.load(SEQ)
            || self.stop_all.load(SEQ)
            || self.other_split(cpu)
            || (self.source_free && self.kicks.flag(cpu))
    }

    /// The interrupt source of the board is not in a Local APIC, so nothing posts to the boot
    /// processor when its line becomes deliverable as the clock moves: a thread that moved the
    /// clock looks, only if the boot processor sleeps in `HLT` with IF = 1, and kicks it.
    fn kick_boot_if_board_wakes(&self) {
        let boot = &self.cells[BOOT_VCPU_INDEX];
        if decode_activity(boot.activity.load(SEQ)) != SchedActivity::Halted
            || !boot.if_set.load(SEQ)
        {
            return;
        }
        let now = lock(&self.owner).now_ns();
        match self.irq.try_lock() {
            Ok(source) => {
                if source.wake_ns().is_some_and(|wake| wake <= now) {
                    self.kicks.kick(BOOT_VCPU_INDEX);
                }
            }
            // A thread holds the source and may be about to move its deadline: the look is lost,
            // so wake the boot processor anyway (a spurious wake-up is harmless; a lost one
            // leaves the line unserved while the others sleep).
            Err(std::sync::TryLockError::WouldBlock) => self.kicks.kick(BOOT_VCPU_INDEX),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                if poisoned
                    .into_inner()
                    .wake_ns()
                    .is_some_and(|wake| wake <= now)
                {
                    self.kicks.kick(BOOT_VCPU_INDEX);
                }
            }
        }
    }

    fn wake_controller(&self) {
        if let Some(thread) = self.controller.get() {
            thread.unpark();
        }
    }

    /// Worker failures take precedence over a guest stop, which takes precedence over ordinary
    /// completion. Retain the first reason of equal priority.
    fn finish_with(&self, reason: ParallelFinish) {
        let mut current = lock(&self.reason);
        if current
            .as_ref()
            .is_none_or(|prior| prior.priority() < reason.priority())
        {
            *current = Some(reason);
        }
        drop(current);
        self.finish.store(true, SEQ);
        self.kicks.kick_all();
        self.wake_controller();
    }

    fn fail(&self, cpu: usize, error: FirmwareBoardError) {
        self.finish_with(ParallelFinish::Error { cpu, error });
    }

    fn finish_budget<const SERVICE_NVME: bool>(&self) -> bool {
        if SERVICE_NVME && self.cells.iter().any(|cell| cell.nvme_work.load(SEQ)) {
            return false;
        }
        self.finish_with(ParallelFinish::BudgetExhausted);
        true
    }

    fn refresh_device_lines_until_stop(&self, cpu: usize) -> bool {
        loop {
            let refreshed = match self.irq.try_lock() {
                Ok(source) => source.try_refresh_device_lines(),
                Err(std::sync::TryLockError::Poisoned(error)) => {
                    error.into_inner().try_refresh_device_lines()
                }
                Err(std::sync::TryLockError::WouldBlock) => false,
            };
            if refreshed {
                return true;
            }
            if self.call_must_end(cpu) {
                return false;
            }
            thread::park_timeout(Duration::from_micros(200));
        }
    }

    fn publish(&self, cpu: usize, vcpu: &Vcpu) {
        let cell = &self.cells[cpu];
        cell.activity
            .store(encode_activity(SchedActivity::from(vcpu.activity())), SEQ);
        cell.if_set.store(vcpu.interrupts_enabled(), SEQ);
        cell.rep_restart.store(vcpu.skip_first_boundary, SEQ);
    }

    fn ack_of(&self, cpu: usize) -> StopAck {
        let cell = &self.cells[cpu];
        match decode_activity(cell.activity.load(SEQ)) {
            SchedActivity::Running if cell.rep_restart.load(SEQ) => StopAck::RepRestart,
            SchedActivity::Running => StopAck::Boundary,
            asleep => StopAck::Asleep(asleep),
        }
    }

    /// The barrier at the top of a pass: `true` when the run is over. The thread stands still while
    /// a stop-all is in force or another vCPU holds the split-lock gate, and says so (`safe` for a
    /// stop, `quiet` for both). Leaving it clears the flags FIRST and reads the requests AFTER
    /// (`SeqCst` both ways): a requester that read a stale `safe`/`quiet` has set its request
    /// before the thread's clear, so the thread's read sees the request.
    ///
    /// S6-36 (the window of S6-34 review m2, narrowed): a thread that comes to the barrier with
    /// `safe` already up (it went to sleep, or stood here before) and the stop still in force does
    /// not lower anything: it stands where it stood, with its flags as they were, so a controller
    /// that reads the vCPU of a stopped machine does not find `safe` down for the long moment of
    /// the old barrier. Nothing but this barrier lowers `safe`, and it lowers it only when it has
    /// not read both flags as up; otherwise the old order, clear first and read after, decides.
    ///
    /// A window remains (S6-36 review, M-1; the earlier claim "no thread lowers `safe` between the
    /// proof and `resume`" was false): a thread woken before the stop reads `standing = false`
    /// (the stop is not yet set), the controller then sets the stop and gets its proof (this
    /// thread's `safe` is still up from an earlier stop), and the thread goes on: it lowers `safe`,
    /// reads the stop (now set) and raises `safe` again. That is a few instructions; the thread
    /// holds no vCPU lock and changes nothing of the vCPU in them, so the state is not spoiled, but
    /// a reader that looks at `safe` in that moment finds it down. `VmSnapshot::take_parallel`
    /// waits for the flag within a bound (`READ_WAIT`) and fails with its own error after it
    /// (`SafeFlagTimeout`), not with a false `StaleSafePoint`.
    fn barrier(&self, cpu: usize) -> bool {
        let cell = &self.cells[cpu];
        loop {
            if self.finish.load(SEQ) {
                return true;
            }
            let standing = cell.safe.load(SEQ) && self.stop_all.load(SEQ);
            let (stopping, other_split) = if standing {
                (true, self.other_split(cpu))
            } else {
                #[cfg(test)]
                if let Some(hook) = self.config.standing_window {
                    hook(cpu);
                }
                cell.safe.store(false, SEQ);
                cell.quiet.store(false, SEQ);
                #[cfg(test)]
                if let Some(hook) = self.config.barrier_window {
                    hook(cpu);
                }
                (self.stop_all.load(SEQ), self.other_split(cpu))
            };
            if !stopping && !other_split {
                return false;
            }
            cell.quiet.store(true, SEQ);
            if stopping {
                cell.safe.store(true, SEQ);
                self.wake_controller();
            }
            let owner = self.split_owner.load(SEQ);
            if owner != NO_OWNER {
                self.kicks.unpark(owner);
            }
            loop {
                if self.finish.load(SEQ) {
                    return true;
                }
                let stopping_now = self.stop_all.load(SEQ);
                // STOP can arrive while this worker is parked solely for an exclusive owner.
                // The worker is already at a boundary, so acknowledge without lowering quiet.
                if stopping_now && !cell.safe.load(SEQ) {
                    cell.safe.store(true, SEQ);
                    self.wake_controller();
                }
                if !stopping_now && !self.other_split(cpu) {
                    break;
                }
                thread::park();
            }
        }
    }

    /// Whether every vCPU thread other than `cpu` is quiet (see [`Cell::quiet`]).
    fn others_quiet(&self, cpu: usize) -> bool {
        (0..self.count)
            .filter(|&other| other != cpu)
            .all(|other| self.cells[other].quiet.load(SEQ))
    }

    /// The gate of the split `LOCK` for vCPU `cpu` (S6-34 step 8, MEMORY_MODEL.md D8): called by
    /// the vCPU in the middle of its instruction BEFORE any visible effect of it.
    ///
    /// The vCPU asks to be the one owner (`split_owner`, compare-exchange). The owner then waits
    /// until every other vCPU is quiet: the others poll the owner in their interpreter loop and end
    /// their call on the next boundary, go to the barrier, and say `quiet`; sleeping vCPUs are
    /// quiet already. A vCPU that asks while another owns, or that loses the exchange, is the
    /// loser: it has had no effect, so it says `quiet` itself (it stands where the plan says "back
    /// at the start of its instruction": nothing of the instruction has happened), waits until the
    /// owner releases, and asks again. No vCPU waits for another to move while being itself
    /// needed to be quiet: an owner is not waited for, and a waiting vCPU is quiet.
    ///
    /// An owner that does not get every other vCPU quiet within the configured time gives the gate
    /// back and fails the instruction (no effect); every vCPU that waited goes on.
    fn acquire_exclusive(&self, cpu: usize, count_split_lock: bool) -> Result<(), String> {
        let cell = &self.cells[cpu];
        let deadline = Instant::now() + self.config.split_timeout;
        loop {
            if self.finish.load(SEQ) {
                return Err("the run is ending".to_owned());
            }
            match self.split_owner.compare_exchange(NO_OWNER, cpu, SEQ, SEQ) {
                Ok(_) => {
                    // The owner. Kick the others (they read the owner in their stop poll), then
                    // wait for their `quiet`.
                    self.kicks.kick_all();
                    loop {
                        if self.others_quiet(cpu) {
                            if count_split_lock {
                                cell.split_locks.fetch_add(1, SEQ);
                            }
                            return Ok(());
                        }
                        if self.finish.load(SEQ) || Instant::now() >= deadline {
                            self.split_owner.store(NO_OWNER, SEQ);
                            self.kicks.kick_all();
                            return Err(if self.finish.load(SEQ) {
                                "the run is ending".to_owned()
                            } else {
                                "the other vCPUs did not stand still in time".to_owned()
                            });
                        }
                        thread::park_timeout(Duration::from_micros(200));
                    }
                }
                Err(owner) => {
                    // The loser: nothing of the instruction has happened. Stand still.
                    if count_split_lock {
                        cell.split_losses.fetch_add(1, SEQ);
                    }
                    cell.quiet.store(true, SEQ);
                    self.kicks.unpark(owner);
                    while self.split_owner.load(SEQ) != NO_OWNER {
                        if self.finish.load(SEQ) {
                            cell.quiet.store(false, SEQ);
                            return Err("the run is ending".to_owned());
                        }
                        if Instant::now() >= deadline {
                            cell.quiet.store(false, SEQ);
                            return Err("the exclusive vCPU did not release in time".to_owned());
                        }
                        thread::park_timeout(Duration::from_micros(200));
                    }
                    cell.quiet.store(false, SEQ);
                }
            }
        }
    }

    fn split_release(&self, cpu: usize) {
        debug_assert_eq!(self.split_owner.load(SEQ), cpu);
        self.split_owner.store(NO_OWNER, SEQ);
        self.kicks.kick_all();
    }

    /// Go to sleep (`HLT`, the wait for SIPI, `Shutdown`): say `safe` and `quiet` (a sleeper
    /// acknowledges a stop and a split `LOCK` without a guest instruction), stop being `busy`, let
    /// the idle arbiter decide whether this was the last thread awake, and park until kicked.
    fn sleep(&self, cpu: usize) {
        #[cfg(test)]
        if let Some(hook) = self.config.pre_sleep {
            hook(cpu);
        }
        let cell = &self.cells[cpu];
        cell.sleeps.fetch_add(1, SEQ);
        cell.quiet.store(true, SEQ);
        cell.safe.store(true, SEQ);
        cell.busy.store(false, SEQ);
        self.wake_controller();
        let owner = self.split_owner.load(SEQ);
        if owner != NO_OWNER {
            self.kicks.unpark(owner);
        }
        self.idle_check();
        while !self.kicks.flag(cpu) && !self.finish.load(SEQ) {
            thread::park();
        }
    }

    /// One decision of the idle arbiter when the last awake thread is going to sleep (S6-34;
    /// the interleaved scheduler's `idle_step`): nobody is busy, no kick is pending, no stop is in
    /// force -> `ClockOwner::idle_step` (at most one jump per decision, repeated while it only
    /// advanced). A wake-up makes the woken thread's own pass wake its vCPU (the kick is set by
    /// the dispatch itself, through the wake sink). "Nothing can wake" ends the run.
    fn idle_check(&self) {
        if !self.quiescent_for_idle() {
            return;
        }
        let mut owner = lock(&self.owner);
        for _ in 0..64 {
            // Under the owner's lock: a stop-all that was requested meanwhile is not crossed (the
            // clock stands while stopped).
            if !self.quiescent_for_idle() {
                return;
            }
            let views: Vec<WakeView> = (0..self.count)
                .map(|cpu| WakeView {
                    activity: decode_activity(self.cells[cpu].activity.load(SEQ)),
                    if_set: self.cells[cpu].if_set.load(SEQ),
                    apic: self.lapics[cpu].clone(),
                })
                .collect();
            let now = owner.now_ns();
            let boot_halted = views[BOOT_VCPU_INDEX].activity == SchedActivity::Halted
                && views[BOOT_VCPU_INDEX].if_set;
            let source = match self.irq.try_lock() {
                Ok(source) => source,
                // A thread has the source: it is not idle.
                Err(std::sync::TryLockError::WouldBlock) => return,
                Err(std::sync::TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            };
            let bsp_deadline = if boot_halted { source.wake_ns() } else { None };
            drop(source);
            if bsp_deadline.is_some_and(|wake| wake <= now) {
                self.kicks.kick(BOOT_VCPU_INDEX);
                return;
            }
            let mut deadlines: Vec<u64> = bsp_deadline.into_iter().collect();
            if let Some(pit) = &self.pit_ioapic_source {
                for (cpu, view) in views.iter().enumerate().skip(1) {
                    if view.activity == SchedActivity::Halted
                        && view.if_set
                        && let Some(wake) = pit.wake_ns_for(cpu)
                    {
                        if wake <= now {
                            self.kicks.kick(cpu);
                            return;
                        }
                        deadlines.push(wake);
                    }
                }
            }
            if !self.quiescent_for_idle() {
                return;
            }
            let step = match owner.idle_step(false, &views, &deadlines) {
                Ok(step) => step,
                Err(error) => {
                    drop(owner);
                    self.fail(
                        BOOT_VCPU_INDEX,
                        FirmwareBoardError::Config(match error {
                            crate::clock_owner::ClockOwnerError::Clock(_) => {
                                "parallel idle: the clock refused the jump"
                            }
                            crate::clock_owner::ClockOwnerError::Dispatch { .. } => {
                                "parallel idle: a timer dispatch failed"
                            }
                        }),
                    );
                    return;
                }
            };
            self.idle_ns.fetch_add(step.idle_ns, SEQ);
            self.idle_jumps.fetch_add(u64::from(step.idle_ns > 0), SEQ);
            if step.idle_ns > 0 {
                if let Some(pit) = &self.pit_ioapic_source {
                    pit.next_deadline_ns(); // publish due IOAPIC edges before another idle decision
                }
                if bsp_deadline.is_some_and(|wake| wake <= owner.now_ns()) {
                    self.kicks.kick(BOOT_VCPU_INDEX);
                }
                if !self.quiescent_for_idle() {
                    return;
                }
            }
            match step.outcome {
                IdleOutcome::Stopped | IdleOutcome::Runnable(_) => return,
                IdleOutcome::Woken(_) => {
                    self.kicks.kick_all();
                    return;
                }
                IdleOutcome::Advanced(_) => {}
                IdleOutcome::AllIdle => {
                    drop(owner);
                    if self.config.end_when_idle {
                        self.finish_with(ParallelFinish::AllIdle);
                    }
                    return;
                }
                IdleOutcome::NoProgress => {
                    if step.idle_ns > 0 && !deadlines.is_empty() {
                        continue; // re-read the board sources after consuming the deadline
                    }
                    drop(owner);
                    self.fail(
                        BOOT_VCPU_INDEX,
                        FirmwareBoardError::Config("parallel idle: a due deadline nobody takes"),
                    );
                    return;
                }
            }
        }
    }

    /// No thread is busy, no kick is pending, no stop or split gate or end is in force.
    fn quiescent_for_idle(&self) -> bool {
        !self.finish.load(SEQ)
            && !self.stop_all.load(SEQ)
            && self.split_owner.load(SEQ) == NO_OWNER
            && (0..self.count).all(|cpu| !self.cells[cpu].busy.load(SEQ) && !self.kicks.flag(cpu))
    }
}

struct WorkerGate<'s, 'a> {
    shared: &'s SharedState<'a>,
    cpu: usize,
}

impl SplitLockGate for WorkerGate<'_, '_> {
    fn acquire(&self) -> Result<(), String> {
        self.shared.acquire_exclusive(self.cpu, true)
    }

    fn acquire_for_descriptor(&self) -> Result<(), String> {
        self.shared.acquire_exclusive(self.cpu, false)
    }

    fn release(&self) {
        self.shared.split_release(self.cpu);
    }
}

/// What a thread has besides the shared state.
struct Worker<'s, 'a, 'e, 'b> {
    shared: &'s SharedState<'a>,
    env: &'e ThreadQuantumEnv<'b>,
    run: &'e QuantumFn<'e>,
    /// The root an NMI is delivered through for a vCPU without a CR3 of its own: the board's
    /// (the interpreter paths), or the root named for this vCPU by a host run (S6-42c); `None`
    /// when none was named (the NMI is then an error, see `deliver_pending_nmi_of`).
    page_table_root: Option<u64>,
    /// A host quantum can supply EFER independently of the modelled vCPU MSR.
    nmi_efer: Option<u64>,
    cpu: usize,
}

impl Worker<'_, '_, '_, '_> {
    fn memory(&self) -> &PhysicalMemory {
        self.env.bus.physical_memory()
    }

    /// The loop of the thread of one vCPU.
    fn run(&self) {
        if self.shared.source_free && self.env.nvme.is_some() {
            self.run_loop::<true>();
        } else {
            self.run_loop::<false>();
        }
    }

    fn run_loop<const SERVICE_NVME: bool>(&self) {
        let sh = self.shared;
        let cpu = self.cpu;
        let cell = &sh.cells[cpu];
        let apic = sh.lapics[cpu].clone();
        let mut consecutive_faults = 0;
        let mut nvme_pending = SERVICE_NVME && cell.nvme_work.load(SEQ);
        loop {
            cell.busy.store(true, SEQ);
            if sh.barrier(cpu) {
                break;
            }
            // Clear the kick BEFORE looking at the APIC (see the module documentation).
            sh.kicks.cells[cpu].flag.store(false, SEQ);
            if SERVICE_NVME && !nvme_pending && sh.remaining.load(SEQ) == 0 {
                if !sh.finish_budget::<SERVICE_NVME>() {
                    // Stay busy for the idle arbiter, but hold no vCPU or device lock while
                    // waiting for a peer to finish its drain.
                    thread::park_timeout(Duration::from_micros(200));
                }
                continue;
            }
            let mut slot = lock(&sh.vcpus[cpu]);
            let vcpu: &mut Vcpu = &mut slot;

            // A stopped NVMe drain belongs to this worker even if the guest retired HLT.
            // Finish it before another guest boundary or an idle transition.
            if SERVICE_NVME && nvme_pending {
                let result = self.call::<SERVICE_NVME>(
                    vcpu,
                    &apic,
                    &mut consecutive_faults,
                    &mut nvme_pending,
                );
                sh.publish(cpu, vcpu);
                drop(slot);
                if let Err(error) = result {
                    sh.fail(cpu, error);
                    break;
                }
                continue;
            }

            // The boundary: INIT and SIPI, then NMI (SMP_CONTRACT.md 3, 5; the order of
            // `BoardSmpHost::boundary`).
            let pending = match apic.take_startup() {
                Ok(pending) => pending,
                Err(_) => {
                    drop(slot);
                    sh.fail(
                        cpu,
                        FirmwareBoardError::Config("apply_pending_startup: INIT reset failed"),
                    );
                    break;
                }
            };
            let applied = vcpu.apply_startup(pending);
            if applied.init {
                consecutive_faults = 0;
            }
            cell.init_applied.fetch_add(u64::from(applied.init), SEQ);
            cell.started.fetch_add(u64::from(applied.started), SEQ);
            let efer = self.nmi_efer.unwrap_or(vcpu.guest_efer);
            let gate = WorkerGate { shared: sh, cpu };
            match deliver_pending_nmi_of(
                &apic,
                vcpu,
                self.memory(),
                self.page_table_root,
                efer,
                Some(&gate),
            ) {
                Ok(nmi) => {
                    cell.nmi_delivered
                        .fetch_add(u64::from(nmi == NmiBoundary::Delivered), SEQ);
                }
                Err(error) => {
                    sh.publish(cpu, vcpu);
                    drop(slot);
                    sh.fail(cpu, error);
                    break;
                }
            }
            sh.publish(cpu, vcpu);

            match SchedActivity::from(vcpu.activity()) {
                SchedActivity::Running => {
                    if let Err(error) = self.call::<SERVICE_NVME>(
                        vcpu,
                        &apic,
                        &mut consecutive_faults,
                        &mut nvme_pending,
                    ) {
                        sh.publish(cpu, vcpu);
                        drop(slot);
                        sh.fail(cpu, error);
                        break;
                    }
                    sh.publish(cpu, vcpu);
                    drop(slot);
                }
                SchedActivity::Halted => {
                    let wake = vcpu.interrupts_enabled()
                        && (apic.eligible().is_some() || self.board_wakes());
                    if wake {
                        vcpu.state.activity = Some(od_isa::CpuActivity::Running);
                        cell.wakes.fetch_add(1, SEQ);
                        sh.publish(cpu, vcpu);
                        drop(slot);
                        continue;
                    }
                    sh.publish(cpu, vcpu);
                    drop(slot);
                    sh.sleep(cpu);
                }
                SchedActivity::WaitForSipi | SchedActivity::Shutdown => {
                    drop(slot);
                    sh.sleep(cpu);
                }
            }
        }
        // The thread is leaving: nobody waits for it.
        cell.safe.store(true, SEQ);
        cell.quiet.store(true, SEQ);
        cell.busy.store(false, SEQ);
        sh.wake_controller();
        let owner = sh.split_owner.load(SEQ);
        if owner != NO_OWNER {
            sh.kicks.unpark(owner);
        }
    }

    /// Whether the board's source has a vector for the halted boot processor now.
    fn board_wakes(&self) -> bool {
        let now = lock(&self.shared.owner).now_ns();
        let wake = if self.cpu == BOOT_VCPU_INDEX {
            lock(&self.shared.irq).wake_ns()
        } else {
            self.shared
                .pit_ioapic_source
                .as_ref()
                .and_then(|pit| pit.wake_ns_for(self.cpu))
        };
        wake.is_some_and(|wake| wake <= now)
    }

    /// One call of the interpreter loop of the running vCPU and its charge to the clock.
    fn call<const SERVICE_NVME: bool>(
        &self,
        vcpu: &mut Vcpu,
        apic: &LocalApicHandle,
        consecutive_faults: &mut u64,
        nvme_pending: &mut bool,
    ) -> Result<(), FirmwareBoardError> {
        let sh = self.shared;
        let cpu = self.cpu;
        let cell = &sh.cells[cpu];
        let device_only = SERVICE_NVME && *nvme_pending;
        let mut left = sh.config.quantum.max(1).min(sh.remaining.load(SEQ));
        if SERVICE_NVME && (left != 0 || device_only) {
            cell.nvme_work.store(true, SEQ);
            // A peer that sees zero may finish only after all admitted calls have drained.
            // Recheck after publication so a late caller cannot submit work after that decision.
            left = left.min(sh.remaining.load(SEQ));
        }
        if left == 0 && !device_only {
            if SERVICE_NVME {
                cell.nvme_work.store(false, SEQ);
            }
            sh.finish_budget::<SERVICE_NVME>();
            return Ok(());
        }
        let gate = WorkerGate { shared: sh, cpu };
        let mut stop = || sh.call_must_end(cpu);
        let mut inert = NoIrqSource;
        let pending = SERVICE_NVME.then_some(&mut *nvme_pending);
        let dispatched = if device_only {
            self.dispatch(
                vcpu,
                apic,
                &mut inert,
                &gate,
                0,
                consecutive_faults,
                &mut stop,
                pending,
            )
        } else if cpu == BOOT_VCPU_INDEX && sh.source_free {
            // The lock of the board's source and the clock owner's lock are held for the budget
            // only: the call itself runs under none of them.
            let budget = {
                let source = lock(&sh.irq);
                let owner = lock(&sh.owner);
                let now = owner.now_ns();
                let deadline = source.next_deadline_ns().filter(|deadline| *deadline > now);
                owner.step_budget(left, deadline)
            };
            let mut source = ScopedBoardSource(&sh.irq);
            self.dispatch(
                vcpu,
                apic,
                &mut source,
                &gate,
                budget,
                consecutive_faults,
                &mut stop,
                pending,
            )
        } else if cpu == BOOT_VCPU_INDEX {
            let mut source = lock(&sh.irq);
            let budget = {
                let owner = lock(&sh.owner);
                let now = owner.now_ns();
                let deadline = source.next_deadline_ns().filter(|deadline| *deadline > now);
                owner.step_budget(left, deadline)
            };
            self.dispatch(
                vcpu,
                apic,
                &mut **source,
                &gate,
                budget,
                consecutive_faults,
                &mut stop,
                pending,
            )
        } else {
            let deadline = sh
                .pit_ioapic_source
                .as_ref()
                .and_then(BoardIrqSource::next_deadline_ns);
            let budget = {
                let owner = lock(&sh.owner);
                owner.step_budget(left, deadline.filter(|deadline| *deadline > owner.now_ns()))
            };
            self.dispatch(
                vcpu,
                apic,
                &mut inert,
                &gate,
                budget,
                consecutive_faults,
                &mut stop,
                pending,
            )
        };
        let (executed, machine_stop, failure) = match dispatched {
            Ok((executed, reason)) => (executed, reason, None),
            Err(failure) => {
                let (executed, error) = failure.into_parts();
                if executed == Executed::default() {
                    return Err(error);
                }
                (executed, None, Some(error))
            }
        };
        // The charge: the clock owner is the only mover of time (D7).
        {
            let mut owner = lock(&sh.owner);
            if executed.retired != 0 {
                owner.account_retired(executed.retired).map_err(|_| {
                    FirmwareBoardError::Config("parallel: the clock refused the retired charge")
                })?;
            }
            if executed.device_ns != 0 {
                owner.account_device_ns(executed.device_ns).map_err(|_| {
                    FirmwareBoardError::Config("parallel: the clock refused the device charge")
                })?;
            }
        }
        if let Some(pit) = &sh.pit_ioapic_source {
            pit.next_deadline_ns(); // catch up and signal the selected LAPIC at the charged time
        }
        if cpu != BOOT_VCPU_INDEX {
            sh.kick_boot_if_board_wakes();
        }
        cell.calls.fetch_add(1, SEQ);
        cell.retired.fetch_add(executed.retired, SEQ);
        let left_after = sh
            .remaining
            .try_update(SEQ, SEQ, |left| Some(left.saturating_sub(executed.retired)))
            .map_or(0, |before| before.saturating_sub(executed.retired));
        if let Some(error) = failure {
            return Err(error);
        }
        // A completion may target an AP even with the BSP masked. Likewise, a CQ-head write
        // must publish the new level before the guest reaches EOI. Keep a deferred sample
        // with the same device-only continuation used by an interrupted drain.
        if SERVICE_NVME && sh.refresh_device_lines && machine_stop.is_none() && !*nvme_pending {
            *nvme_pending = !sh.refresh_device_lines_until_stop(cpu);
        }
        if SERVICE_NVME && !*nvme_pending {
            cell.nvme_work.store(false, SEQ);
        }
        if let Some(reason) = machine_stop {
            sh.finish_with(ParallelFinish::GuestStop(reason));
        } else if left_after == 0 {
            sh.finish_budget::<SERVICE_NVME>();
        }
        Ok(())
    }

    /// The host call for a vCPU in a legacy model or in long mode (what `BoardSmpHost::run` does):
    /// the trampoline of an application processor runs on the RAM; when its 64-bit jump has
    /// retired the rest of the budget is the long-mode quantum in the same call.
    fn dispatch(
        &self,
        vcpu: &mut Vcpu,
        apic: &LocalApicHandle,
        source: &mut dyn BoardIrqSource,
        gate: &WorkerGate<'_, '_>,
        budget: u64,
        consecutive_faults: &mut u64,
        stop: &mut dyn FnMut() -> bool,
        mut nvme_pending: Option<&mut bool>,
    ) -> Result<(Executed, Option<MachineStopReason>), RunFailure<FirmwareBoardError>> {
        let cpu = self.cpu;
        let mut machine_stop = None;
        let device_only = nvme_pending.as_deref().copied() == Some(true);
        let mut quantum = |vcpu: &mut Vcpu,
                           source: &mut dyn BoardIrqSource,
                           budget: u64,
                           consecutive_faults: &mut u64,
                           stop: &mut dyn FnMut() -> bool| {
            (self.run)(QuantumCall {
                env: self.env,
                vcpu,
                cpu,
                lapic: apic,
                source,
                gate,
                budget,
                consecutive_faults,
                stop,
                native_stop: self.shared.kicks.cells[cpu].native_stop.as_ref(),
                machine_stop: &mut machine_stop,
                nvme_pending: nvme_pending.as_deref_mut(),
            })
        };
        if device_only || vcpu.legacy.is_none() {
            let executed = quantum(vcpu, source, budget, consecutive_faults, stop)?;
            return Ok((executed, machine_stop));
        }
        let legacy = crate::ap_legacy_exec::run_legacy_on(
            vcpu,
            self.memory(),
            cpu,
            budget,
            stop,
            Some(gate),
        )?;
        let mut executed = legacy.executed;
        if executed.retired != 0 {
            *consecutive_faults = 0;
        }
        let left = budget.saturating_sub(executed.retired);
        if legacy.entered_long_mode && left > 0 {
            let rest = quantum(vcpu, source, left, consecutive_faults, stop)
                .map_err(|failure| failure.with_prefix(executed))?;
            executed = executed.followed_by(rest);
        }
        Ok((executed, machine_stop))
    }
}

/// The controller's side of a parallel run (the caller's thread).
pub(crate) struct ParallelControl<'s, 'a> {
    shared: &'s SharedState<'a>,
    bus: &'s crate::MachineBus,
}

impl ParallelControl<'_, '_> {
    /// The bus of the machine (RAM by shared reference, devices behind their lock).
    pub(crate) fn bus(&self) -> &crate::MachineBus {
        self.bus
    }

    /// Whether the run is over (every thread is leaving or has left).
    pub(crate) fn is_finished(&self) -> bool {
        self.shared.finish.load(SEQ)
    }

    /// The counters of vCPU `cpu` so far (atomics: a running vCPU's are a moment old).
    pub(crate) fn cpu_report(&self, cpu: usize) -> ParallelCpuReport {
        self.shared.cells[cpu].report()
    }

    /// Retired instructions of every vCPU so far.
    pub(crate) fn retired(&self) -> Vec<u64> {
        self.shared
            .cells
            .iter()
            .map(|c| c.retired.load(SEQ))
            .collect()
    }

    /// The Local APIC of vCPU `cpu` (S6-42c: the controller of a host run posts vectors to it).
    pub(crate) fn lapic(&self, cpu: usize) -> Option<LocalApicHandle> {
        self.shared.lapics.get(cpu).cloned()
    }

    /// The number of vCPU threads of the run.
    pub(crate) fn vcpu_count(&self) -> usize {
        self.shared.cells.len()
    }

    /// The virtual time (ns).
    pub(crate) fn now_ns(&self) -> u64 {
        lock(&self.shared.owner).now_ns()
    }

    /// End the run: every thread ends at its next boundary.
    pub(crate) fn finish(&self) {
        self.shared.finish_with(ParallelFinish::Stopped);
    }

    /// Wait until the run is over or `timeout` has passed. `true` when it is over.
    pub(crate) fn wait_finished(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.shared.finish.load(SEQ) {
                return true;
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::park_timeout((deadline - now).min(Duration::from_millis(5)));
        }
    }

    /// Wait until `condition` is true or `timeout` has passed or the run is over. `true` when the
    /// condition held.
    pub(crate) fn wait_until(
        &self,
        timeout: Duration,
        mut condition: impl FnMut() -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if condition() {
                return true;
            }
            if self.shared.finish.load(SEQ) {
                return condition();
            }
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            thread::park_timeout((deadline - now).min(Duration::from_micros(500)));
        }
    }

    /// Stop every vCPU (SMP_CONTRACT.md 9): set the request, kick each vCPU that has not
    /// acknowledged, wait for the acknowledgement of each until `timeout`. On success the stop
    /// stays in force until [`Self::resume`]: no vCPU starts, an IPI that arrives stays pending in
    /// its Local APIC, the clock stands. On a timeout the request is withdrawn and nothing has
    /// changed.
    pub(crate) fn stop_all(&self, timeout: Duration) -> Result<StopProof, ParallelStopError> {
        let sh = self.shared;
        if sh.finish.load(SEQ) {
            return Err(ParallelStopError::Finished);
        }
        sh.stop_all.store(true, SEQ);
        if sh.source_free {
            sh.kicks.kick_all();
        } else {
            // Interpreted workers poll STOP in their loop. An exclusive-gate waiter is already
            // parked, so wake it without publishing a guest event or waking a sleeping guest.
            for cpu in 0..sh.count {
                sh.kicks.unpark(cpu);
            }
        }
        self.wait_for_stop(timeout)
    }

    /// Wait for an already published STOP request to reach every live worker.
    fn wait_for_stop(&self, timeout: Duration) -> Result<StopProof, ParallelStopError> {
        let sh = self.shared;
        let deadline = Instant::now() + timeout;
        loop {
            let unacked: Vec<usize> = (0..sh.count)
                .filter(|&cpu| !sh.cells[cpu].safe.load(SEQ))
                .collect();
            let proof = if unacked.is_empty() {
                // The idle jump that may have been in progress finishes under the owner's lock
                // and takes no further step while the request stands: read the time under it.
                let now_ns = lock(&sh.owner).now_ns();
                let acks = (0..sh.count).map(|cpu| sh.ack_of(cpu)).collect();
                Some(StopProof { acks, now_ns })
            } else {
                None
            };
            // Exiting workers also publish safe, after publishing finish. Only a live run
            // can acknowledge STOP, so check finish after reading the flags and proof state.
            if sh.finish.load(SEQ) {
                sh.stop_all.store(false, SEQ);
                return Err(ParallelStopError::Finished);
            }
            if let Some(proof) = proof {
                return Ok(proof);
            }
            if Instant::now() >= deadline {
                sh.stop_all.store(false, SEQ);
                sh.kicks.kick_all();
                return Err(ParallelStopError::Timeout { unacked });
            }
            for &cpu in &unacked {
                sh.kicks.kick(cpu);
            }
            thread::park_timeout(Duration::from_micros(200));
        }
    }

    /// The explicit command that lets the guest run again after [`Self::stop_all`].
    pub(crate) fn resume(&self) {
        let sh = self.shared;
        sh.stop_all.store(false, Ordering::Release);
        sh.kicks.kick_all();
        // If every vCPU was asleep at the stop, nobody runs the idle arbiter: do it here.
        sh.idle_check();
    }

    /// Whether a stop is in force on a live run.
    pub(crate) fn stopped(&self) -> bool {
        self.shared.stop_all.load(SEQ) && !self.is_finished()
    }

    /// Look at (or change) vCPU `cpu` while the machine is stopped. `None` when no stop is in
    /// force or the vCPU has not acknowledged it.
    pub(crate) fn with_vcpu<R>(&self, cpu: usize, f: impl FnOnce(&mut Vcpu) -> R) -> Option<R> {
        let sh = self.shared;
        if !self.stopped() || !sh.cells[cpu].safe.load(SEQ) {
            return None;
        }
        let mut guard = lock(&sh.vcpus[cpu]);
        // Check after observing safe and acquiring the slot. A worker may have published
        // its terminal safe flag while the controller was entering this access.
        if !self.stopped() {
            return None;
        }
        Some(f(&mut guard))
    }

    /// The activity vCPU `cpu` last published.
    pub(crate) fn activity(&self, cpu: usize) -> SchedActivity {
        decode_activity(self.shared.cells[cpu].activity.load(SEQ))
    }
}

/// Run the vCPUs of `board` on one thread each (the parallel mode), with the caller's thread as the
/// controller: `controller` runs while the vCPUs run, and when it returns the run is ended (if it
/// is not over yet) and every thread is joined before this function returns. The board is put in
/// [`ExecutionMode::Parallel`] for the run (ordinary RAM accesses `SeqCst`, host fences for the
/// guest fences) and back to the mode it had when the run is over.
///
/// `run` is the quantum of a vCPU (the caller's CPL0 set); it is called by every thread. A vCPU
/// that is not `Running` is not run: the boot processor starts `Running` as the loader left it and
/// the others wait for SIPI. With NVMe, scope the board-source lock to individual operations so
/// a worker can service the controller without holding it.
pub(crate) fn run_threads<R>(
    board: &mut LoadedFirmwareBoard,
    config: ParallelConfig,
    run: &QuantumFn<'_>,
    controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
) -> Result<(ParallelReport, R), FirmwareBoardError> {
    let source_free = board.nvme().is_some();
    run_threads_with(board, config, source_free, run, None, controller)
}

/// [`run_threads`] with `source_free`: the host quantum receives a board source adapter that locks
/// each source operation for its duration (A7 of S6-40).
pub(crate) fn run_threads_with<R>(
    board: &mut LoadedFirmwareBoard,
    config: ParallelConfig,
    source_free: bool,
    run: &QuantumFn<'_>,
    on_thread_exit: Option<&ThreadExitFn<'_>>,
    controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
) -> Result<(ParallelReport, R), FirmwareBoardError> {
    run_threads_rooted(
        board,
        config,
        run,
        on_thread_exit,
        ThreadRunOptions {
            source_free,
            nmi_paging: NmiPaging::Board,
            discard_startup_port_ticks: true,
        },
        controller,
    )
}

/// NMI translation inputs for interpreter and host quanta. CR3 in the vCPU
/// state takes precedence over the fallback root.
pub(crate) enum NmiPaging<'a> {
    Board,
    Roots(&'a [Option<u64>]),
    Host(&'a [Option<HostNmiPaging>]),
}

pub(crate) struct ThreadRunOptions<'a> {
    pub(crate) source_free: bool,
    pub(crate) nmi_paging: NmiPaging<'a>,
    pub(crate) discard_startup_port_ticks: bool,
}

/// Paging inputs shared by a host quantum and NMI delivery on its vCPU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostNmiPaging {
    pub root: u64,
    pub efer: u64,
}

/// [`run_threads_with`] with explicit NMI translation inputs for host quanta.
pub(crate) fn run_threads_rooted<R>(
    board: &mut LoadedFirmwareBoard,
    config: ParallelConfig,
    run: &QuantumFn<'_>,
    on_thread_exit: Option<&ThreadExitFn<'_>>,
    options: ThreadRunOptions<'_>,
    controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
) -> Result<(ParallelReport, R), FirmwareBoardError> {
    if config.quantum == 0 {
        return Err(FirmwareBoardError::Config("parallel run: quantum 0"));
    }
    let previous_mode = board.execution_mode();
    board.set_execution_mode(ExecutionMode::Parallel);
    let outcome = run_threads_inner(board, config, run, on_thread_exit, options, controller);
    board.set_execution_mode(previous_mode);
    outcome
}

fn run_threads_inner<R>(
    board: &mut LoadedFirmwareBoard,
    config: ParallelConfig,
    run: &QuantumFn<'_>,
    on_thread_exit: Option<&ThreadExitFn<'_>>,
    options: ThreadRunOptions<'_>,
    controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
) -> Result<(ParallelReport, R), FirmwareBoardError> {
    let ThreadRunOptions {
        source_free,
        nmi_paging,
        discard_startup_port_ticks,
    } = options;
    let parts = board.thread_parts()?;
    let ThreadParts {
        bus,
        vcpus,
        lapic,
        uart,
        acpi_pm,
        pit,
        pit_ioapic_source,
        nvme,
        uart_irq_source,
        uart_ioapic_source,
        irq_source,
        owner,
        rdtsc_clock,
        com1,
        page_table_root,
    } = parts;
    let count = vcpus.len();
    let lapics: Vec<LocalApicHandle> = (0..count)
        .map(|cpu| {
            lapic.for_cpu(cpu).ok_or(FirmwareBoardError::Config(
                "parallel run: a vCPU without a Local APIC",
            ))
        })
        .collect::<Result<_, _>>()?;
    let kicks = Arc::new(Kicks::new(count, source_free));
    let com1 = Mutex::new(com1);
    let env = ThreadQuantumEnv {
        bus,
        uart,
        acpi_pm,
        pit,
        stop_on_pit_write: pit_ioapic_source.is_some(),
        nvme,
        uart_irq_source,
        uart_ioapic_source,
        rdtsc_clock,
        com1: &com1,
    };
    let startup_stop = env.machine_stop_request();
    if discard_startup_port_ticks && startup_stop.is_none() {
        // The interpreter ignores prior port reads. Once workers are active a
        // pending tick may belong to any vCPU and must be accounted for.
        env.take_device_ticks();
    }
    let shared = SharedState {
        count,
        config,
        source_free,
        refresh_device_lines: irq_source.needs_device_line_refresh(),
        kicks: Arc::clone(&kicks),
        cells: (0..count).map(|_| Cell::new()).collect(),
        stop_all: AtomicBool::new(false),
        finish: AtomicBool::new(false),
        reason: Mutex::new(None),
        split_owner: AtomicUsize::new(NO_OWNER),
        controller: OnceLock::new(),
        owner: Mutex::new(owner),
        irq: Mutex::new(irq_source),
        pit_ioapic_source,
        remaining: AtomicU64::new(config.budget),
        idle_ns: AtomicU64::new(0),
        idle_jumps: AtomicU64::new(0),
        started: AtomicUsize::new(0),
        exited: AtomicUsize::new(0),
        lapics: lapics.clone(),
        vcpus: vcpus.iter_mut().map(Mutex::new).collect(),
    };
    // A terminal request on the assembled board takes precedence over the initial scheduler
    // activity, budget, and work queued by the storage controller.
    if let Some(reason) = startup_stop {
        shared.finish_with(ParallelFinish::GuestStop(reason));
    } else if source_free
        && env
            .nvme
            .as_ref()
            .is_some_and(crate::NvmeHandle::doorbell_pending_or_contended)
    {
        // Reserve the carried-in doorbell for the BSP before any peer can finish the run.
        shared.cells[BOOT_VCPU_INDEX].nvme_work.store(true, SEQ);
    }
    {
        let sink_kicks = Arc::clone(&kicks);
        lapics[0].set_wake_sink(Some(Arc::new(move |cpu| sink_kicks.kick(cpu))));
    }
    let _ = shared.controller.set(thread::current());
    let result = thread::scope(|scope| {
        for cpu in 0..count {
            let (page_table_root, nmi_efer) = match nmi_paging {
                NmiPaging::Board => (Some(page_table_root), None),
                NmiPaging::Roots(roots) => (roots.get(cpu).copied().flatten(), None),
                NmiPaging::Host(configs) => configs
                    .get(cpu)
                    .copied()
                    .flatten()
                    .map_or((None, None), |paging| {
                        (Some(paging.root), Some(paging.efer))
                    }),
            };
            let worker = Worker {
                shared: &shared,
                env: &env,
                run,
                page_table_root,
                nmi_efer,
                cpu,
            };
            thread::Builder::new()
                .name(format!("od-vcpu-{cpu}"))
                .spawn_scoped(scope, move || {
                    let shared = worker.shared;
                    let _ = shared.kicks.cells[cpu].thread.set(thread::current());
                    shared.started.fetch_add(1, SEQ);
                    let cpu = worker.cpu;
                    if catch_unwind(AssertUnwindSafe(|| worker.run())).is_err() {
                        shared.finish_with(ParallelFinish::Panicked { cpu });
                        let cell = &shared.cells[cpu];
                        cell.safe.store(true, SEQ);
                        cell.quiet.store(true, SEQ);
                        cell.busy.store(false, SEQ);
                        shared.wake_controller();
                    }
                    // The thread's own state is released here, on the thread (M1 of S6-42b).
                    if let Some(on_exit) = on_thread_exit {
                        let _ = catch_unwind(AssertUnwindSafe(|| on_exit(cpu)));
                    }
                    shared.exited.fetch_add(1, SEQ);
                })
                .expect("a thread of the parallel run starts");
        }
        let control = ParallelControl {
            shared: &shared,
            bus,
        };
        let value = catch_unwind(AssertUnwindSafe(|| controller(&control)));
        // The run ends when the controller is done (the first reason stays if there is one).
        shared.finish_with(ParallelFinish::Stopped);
        value
    });
    lapics[0].set_wake_sink(None);
    let finished = lock(&shared.reason)
        .take()
        .unwrap_or(ParallelFinish::Stopped);
    let report = ParallelReport {
        finished,
        cpus: shared.cells.iter().map(Cell::report).collect(),
        idle_ns: shared.idle_ns.load(SEQ),
        idle_jumps: shared.idle_jumps.load(SEQ),
        threads_started: shared.started.load(SEQ),
        threads_exited: shared.exited.load(SEQ),
    };
    match result {
        Ok(value) => Ok((report, value)),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::firmware_board::vcpu_quantum::{fixture::*, run_vcpu_set_quantum_shared};
    use crate::{GuestPhysicalAddress, LOCAL_APIC_MMIO_BASE};
    use od_isa::{CpuActivity, Register};
    use std::sync::atomic::AtomicBool;

    /// The quantum of the fixture guests on a vCPU thread: the CPL0 set of the S6-16d tests.
    fn fixture_quantum(
        call: QuantumCall<'_, '_>,
    ) -> Result<Executed, RunFailure<FirmwareBoardError>> {
        with_set(|set| fixture_quantum_with_set(call, set))
    }

    fn fixture_quantum_with_set(
        call: QuantumCall<'_, '_>,
        set: &od_isa::execute::Cpl0ProfileSet<'_>,
    ) -> Result<Executed, RunFailure<FirmwareBoardError>> {
        run_vcpu_set_quantum_shared(
            call.env,
            call.vcpu,
            call.cpu,
            call.lapic,
            call.source,
            Some(call.gate),
            call.budget,
            call.stop,
            set,
            call.consecutive_faults,
            call.machine_stop,
            call.nvme_pending,
        )
    }

    const NVME_IDENTIFY_CQ: u64 = 0x30_1000;
    const NVME_IDENTIFY_BUFFER: u64 = 0x30_2000;
    const NVME_IDENTIFY_CID: u32 = 0x51;

    fn nvme_identify_board() -> (LoadedFirmwareBoard, u64, u64) {
        use crate::{AccessWidth, NvmeConfig, PCI_ECAM_DESCRIPTOR, host_parallel_board_with_nvme};

        const ASQ: u64 = 0x30_0000;
        const PT_NVME: u64 = TABLES_0 + 0x5000;
        const PCI_FUNCTION: u64 = PCI_ECAM_DESCRIPTOR.base + (4 << 15);

        let mut board = host_parallel_board_with_nvme(2, NvmeConfig::default()).unwrap();
        let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
        // The first bank is identity mapped. Map the second 2 MiB for the DMA buffer.
        patch(&mut board, cr3 + 0x2000 + 8, &0x20_0083u64.to_le_bytes());
        let bus = board.bus_mut();
        let bar_low = bus
            .read_physical(
                GuestPhysicalAddress::new(PCI_FUNCTION + 0x10),
                AccessWidth::Dword,
            )
            .unwrap();
        let bar_high = bus
            .read_physical(
                GuestPhysicalAddress::new(PCI_FUNCTION + 0x14),
                AccessWidth::Dword,
            )
            .unwrap();
        let bar = (bar_high << 32) | (bar_low & !0xF);
        for (address, width, value) in [
            (PCI_FUNCTION + 4, AccessWidth::Word, 6),
            (bar + 0x24, AccessWidth::Dword, 0x000F_000F),
            (bar + 0x28, AccessWidth::Qword, ASQ),
            (bar + 0x30, AccessWidth::Qword, NVME_IDENTIFY_CQ),
            (bar + 0x14, AccessWidth::Dword, 0x0046_0001),
        ] {
            bus.write_physical(GuestPhysicalAddress::new(address), width, value, |_| {})
                .unwrap();
        }
        let nvme = board.nvme().unwrap();
        assert!(nvme.is_ready());
        let doorbell = bar + 0x1000;
        patch(
            &mut board,
            TABLES_0 + 0x3000 + ((doorbell >> 21) & 0x1ff) * 8,
            &(PT_NVME | 0x63).to_le_bytes(),
        );
        patch(
            &mut board,
            PT_NVME + ((doorbell >> 12) & 0x1ff) * 8,
            &(doorbell | 0x73).to_le_bytes(),
        );
        let mut identify = [0; 64];
        identify[..4].copy_from_slice(&((NVME_IDENTIFY_CID << 16) | 0x06).to_le_bytes());
        identify[24..32].copy_from_slice(&NVME_IDENTIFY_BUFFER.to_le_bytes());
        identify[40..44].copy_from_slice(&1u32.to_le_bytes());
        patch(&mut board, ASQ, &identify);

        (board, doorbell, cr3)
    }

    #[test]
    fn a_guest_nvme_doorbell_completes_dma_before_the_next_instruction_or_budget_stop() {
        for (budget, stop_before_dma) in [(3, false), (64, false), (64, true), (3, true)] {
            let (mut board, doorbell, cr3) = nvme_identify_board();
            let nvme = board.nvme().unwrap();

            let code = [
                mov_eax(1),
                mov_ebx(u32::try_from(doorbell).unwrap()),
                vec![0x89, 0x03], // mov [rbx], eax
                mov_ebx((NVME_IDENTIFY_BUFFER + 4) as u32),
                vec![0x8B, 0x03, 0xF4], // mov eax, [rbx]; hlt
            ]
            .concat();
            patch(&mut board, CODE_0, &code);
            install(
                &mut board,
                0,
                long_state(CODE_0, STACK_0, cr3, false),
                0xD01,
            );
            let page = GuestPhysicalAddress::new(NVME_IDENTIFY_BUFFER);
            let generation = board
                .bus_mut()
                .physical_memory()
                .code_page_generation(page)
                .unwrap();
            let start_ns = board.clock().now_ns();

            let (report, ()) = if stop_before_dma {
                let active_peer = budget == 3;
                if active_peer {
                    patch(&mut board, CODE_1, &[0xF4]);
                    install(
                        &mut board,
                        1,
                        long_state(CODE_1, STACK_1, cr3, false),
                        0xD01,
                    );
                }
                let (notified, doorbell_written) = std::sync::mpsc::sync_channel(1);
                let (drain_announced, drain_started) = std::sync::mpsc::sync_channel(1);
                let observed = AtomicBool::new(false);
                let peer_entered = AtomicBool::new(false);
                let allow_drain = AtomicBool::new(false);
                let quantum = |call: QuantumCall<'_, '_>| {
                    let deadline = Instant::now() + WAIT;
                    if call.cpu == 1 {
                        assert!(active_peer && !peer_entered.swap(true, SEQ));
                        while !(call.stop)() {
                            assert!(Instant::now() < deadline);
                            thread::yield_now();
                        }
                        return Ok(Executed::default());
                    }
                    if active_peer {
                        while !peer_entered.load(SEQ) {
                            assert!(Instant::now() < deadline);
                            thread::yield_now();
                        }
                        if call.nvme_pending.as_deref().copied() == Some(true) {
                            assert_eq!(call.budget, 0);
                            drain_announced.send(()).unwrap();
                            while !allow_drain.load(SEQ) && !(call.stop)() {
                                assert!(Instant::now() < deadline);
                                thread::yield_now();
                            }
                        }
                    }
                    let stop = call.stop;
                    let mut poll = || {
                        if !observed.load(SEQ) && nvme.doorbell_pending() {
                            observed.store(true, SEQ);
                            notified.send(()).unwrap();
                            let deadline = Instant::now() + WAIT;
                            while !stop() {
                                assert!(Instant::now() < deadline);
                                thread::yield_now();
                            }
                            return true;
                        }
                        stop()
                    };
                    with_set(|set| {
                        run_vcpu_set_quantum_shared(
                            call.env,
                            call.vcpu,
                            call.cpu,
                            call.lapic,
                            call.source,
                            Some(call.gate),
                            call.budget,
                            &mut poll,
                            set,
                            call.consecutive_faults,
                            call.machine_stop,
                            call.nvme_pending,
                        )
                    })
                };
                run_threads(&mut board, config(64, budget), &quantum, |control| {
                    doorbell_written.recv_timeout(WAIT).unwrap();
                    let proof = control.stop_all(WAIT).unwrap();
                    assert_eq!(proof.now_ns, start_ns + 3);
                    assert_eq!(control.retired()[0], 3);
                    assert_eq!(cell(control, NVME_IDENTIFY_BUFFER + 4), 0);
                    assert!(nvme.doorbell_pending());
                    control.resume();
                    if active_peer {
                        drain_started.recv_timeout(WAIT).unwrap();
                        // While the owner is draining, the other runnable vCPU cannot end the
                        // run at the exhausted guest budget.
                        assert!(!control.wait_finished(Duration::from_millis(50)));
                        allow_drain.store(true, SEQ);
                    }
                    assert!(control.wait_finished(WAIT));
                })
                .unwrap()
            } else {
                run(&mut board, config(64, budget), |control| {
                    assert!(control.wait_finished(WAIT));
                })
            };
            assert_joined(&report, 2);
            assert_eq!(report.cpus[1].retired, 0);
            assert_eq!(
                read(&mut board, NVME_IDENTIFY_BUFFER + 4, 8),
                b"OD-NVME-",
                "budget {budget}: {report:?}"
            );
            assert_eq!(
                dword(&mut board, NVME_IDENTIFY_CQ + 12),
                NVME_IDENTIFY_CID | (1 << 16)
            );
            assert!(!nvme.doorbell_pending());
            assert_ne!(
                board
                    .bus_mut()
                    .physical_memory()
                    .code_page_generation(page)
                    .unwrap(),
                generation
            );
            if budget == 3 {
                assert_eq!(report.finished, ParallelFinish::BudgetExhausted);
                assert_eq!(report.cpus[0].retired, 3);
                assert_eq!(board.clock().now_ns() - start_ns, 3);
            } else {
                assert_eq!(report.finished, ParallelFinish::AllIdle);
                assert_eq!(report.cpus[0].retired, 6);
                assert_eq!(board.clock().now_ns() - start_ns, 6);
                assert_eq!(
                    board.vcpu(0).unwrap().state.gpr[Register::Rax as usize],
                    u64::from(u32::from_le_bytes(*b"OD-N"))
                );
            }
        }
    }

    #[test]
    fn a_pending_nvme_doorbell_is_serviced_when_the_board_starts_idle() {
        use crate::AccessWidth;
        use od_isa::real_mode_exec::EFLAGS_IF;

        for (budget, legacy, finish) in [
            (64, false, ParallelFinish::AllIdle),
            (0, true, ParallelFinish::BudgetExhausted),
        ] {
            let (mut board, doorbell, _) = nvme_identify_board();
            let bsp = board.vcpu_mut(0).unwrap();
            bsp.state.activity = Some(CpuActivity::Halted);
            bsp.state.rflags &= !u64::from(EFLAGS_IF);
            if legacy {
                bsp.legacy = Vcpu::new_ap(1).legacy;
            }
            let initial_state = bsp.state.clone();
            let initial_legacy = bsp.legacy;
            let start_ns = board.clock().now_ns();
            let frame = GuestPhysicalAddress::new(NVME_IDENTIFY_BUFFER);
            let generation = board
                .bus_mut()
                .physical_memory()
                .code_page_generation(frame)
                .unwrap();
            board
                .bus_mut()
                .write_physical(
                    GuestPhysicalAddress::new(doorbell),
                    AccessWidth::Dword,
                    1,
                    |_| {},
                )
                .unwrap();
            let nvme = board.nvme().unwrap();
            assert!(nvme.doorbell_pending());

            let (report, ()) = run(&mut board, config(64, budget), |control| {
                assert!(control.wait_finished(WAIT));
            });
            assert_eq!(report.finished, finish);
            assert_joined(&report, 2);
            assert_eq!(
                read(&mut board, NVME_IDENTIFY_BUFFER + 4, 8),
                b"OD-NVME-",
                "budget {budget}: {report:?}",
            );
            assert_eq!(
                dword(&mut board, NVME_IDENTIFY_CQ + 12),
                NVME_IDENTIFY_CID | (1 << 16),
            );
            assert!(!nvme.doorbell_pending());
            assert_eq!(report.cpus[0].calls, 1);
            assert_eq!(report.cpus[1].calls, 0);
            assert!(report.cpus.iter().all(|cpu| cpu.retired == 0));
            assert_eq!(board.clock().now_ns(), start_ns);
            let bsp = board.vcpu(0).unwrap();
            assert_eq!(bsp.state, initial_state);
            assert_eq!(bsp.legacy, initial_legacy);
            let completed = board
                .bus_mut()
                .physical_memory()
                .code_page_generation(frame)
                .unwrap();
            assert_ne!(completed, generation);

            let (reopened, ()) = run(&mut board, config(64, budget), |control| {
                assert!(control.wait_finished(WAIT));
            });
            assert_eq!(reopened.finished, finish);
            assert!(reopened.cpus.iter().all(|cpu| cpu.calls == 0));
            assert_eq!(
                board
                    .bus_mut()
                    .physical_memory()
                    .code_page_generation(frame)
                    .unwrap(),
                completed,
            );
            assert_eq!(board.clock().now_ns(), start_ns);
        }
    }

    #[test]
    fn a_startup_nvme_completion_interrupt_wakes_the_halted_destination() {
        use crate::apic_pair::{NVME_PIN, SCI_LEVEL_LOW};
        use crate::{AccessWidth, IO_APIC_MMIO_BASE};

        for (cpu, vector, handler_rip, counter, code_rip, stack) in [
            (0, TO_BSP, HANDLER_0, COUNTER_0, CODE_0, STACK_0),
            (1, TO_AP, HANDLER_1, COUNTER_1, CODE_1, STACK_1),
        ] {
            let (mut board, doorbell, cr3) = nvme_identify_board();
            assert!(board.enable_pit_ioapic_irq());
            assert!(board.enable_uart_ioapic_irq4());
            system(&mut board, false);
            let mut handler = [
                mov_ebx((doorbell + 4).try_into().unwrap()),
                mov_eax(1),
                vec![0x89, 0x03], // mov [rbx], eax: acknowledge the completion queue
                count_and_eoi(counter),
            ]
            .concat();
            handler.extend([0x48, 0xCF]); // iretq
            patch(&mut board, handler_rip, &handler);
            patch(&mut board, code_rip, &[0xF4]);
            let mut target = long_state(code_rip, stack, cr3, true);
            target.activity = Some(CpuActivity::Halted);
            install(&mut board, cpu, target, 0xD01);
            if cpu != BOOT_VCPU_INDEX {
                let bsp = board.vcpu_mut(BOOT_VCPU_INDEX).unwrap();
                bsp.state.activity = Some(CpuActivity::Halted);
                bsp.state.rflags &= !(1 << 9);
            }

            let bus = board.bus_mut();
            bus.write_mmio_shared(
                cpu,
                GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0xf0),
                AccessWidth::Dword,
                0x1ff,
            )
            .unwrap();
            for (address, value) in [
                (IO_APIC_MMIO_BASE, (0x11 + 2 * NVME_PIN) as u64),
                (IO_APIC_MMIO_BASE + 0x10, (cpu as u64) << 24),
                (IO_APIC_MMIO_BASE, (0x10 + 2 * NVME_PIN) as u64),
                (
                    IO_APIC_MMIO_BASE + 0x10,
                    u64::from(SCI_LEVEL_LOW | u32::from(vector)),
                ),
                (doorbell, 1),
            ] {
                bus.write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .unwrap();
            }
            let nvme = board.nvme().unwrap();
            assert!(nvme.doorbell_pending());
            let start_ns = board.clock().now_ns();

            let (report, ()) = run(&mut board, config(64, 64), |control| {
                assert!(control.wait_finished(WAIT));
            });
            assert_eq!(report.finished, ParallelFinish::AllIdle);
            assert_joined(&report, 2);
            assert_eq!(report.cpus[cpu].wakes, 1, "target {cpu}: {report:?}");
            assert_eq!(dword(&mut board, counter), 1);
            assert_eq!(report.cpus[1 - cpu].retired, 0);
            assert_eq!(board.clock().now_ns() - start_ns, report.cpus[cpu].retired);
            assert_eq!(
                board.local_apic_of(cpu).unwrap().vector_state(vector),
                (false, false)
            );
            assert!(!nvme.pci_interrupt_line());
            assert!(!nvme.doorbell_pending());
        }
    }

    #[test]
    fn a_deferred_nvme_irq_sample_survives_stop_at_zero_budget() {
        use crate::AccessWidth;
        use std::sync::mpsc::{SyncSender, sync_channel};

        struct DelayedRefresh {
            ready: Arc<AtomicBool>,
            first: std::cell::Cell<Option<SyncSender<()>>>,
        }

        impl BoardIrqSource for DelayedRefresh {
            fn pending_vector(&self) -> Option<u8> {
                None
            }

            fn acknowledge(&mut self, _vector: u8) {}

            fn next_deadline_ns(&self) -> Option<u64> {
                None
            }

            fn needs_device_line_refresh(&self) -> bool {
                true
            }

            fn try_refresh_device_lines(&self) -> bool {
                if let Some(first) = self.first.take() {
                    first.send(()).unwrap();
                }
                self.ready.load(SEQ)
            }
        }

        let (mut board, doorbell, _) = nvme_identify_board();
        let (first, attempted) = sync_channel(1);
        let ready = Arc::new(AtomicBool::new(false));
        board.set_irq_source(Box::new(DelayedRefresh {
            ready: Arc::clone(&ready),
            first: std::cell::Cell::new(Some(first)),
        }));
        board.vcpu_mut(0).unwrap().state.activity = Some(CpuActivity::Halted);
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(doorbell),
                AccessWidth::Dword,
                1,
                |_| {},
            )
            .unwrap();
        let start_ns = board.clock().now_ns();
        let address = GuestPhysicalAddress::new(NVME_IDENTIFY_BUFFER);
        let (report, generation) = run(&mut board, config(64, 0), |control| {
            attempted.recv_timeout(WAIT).unwrap();
            assert_eq!(control.stop_all(WAIT).unwrap().now_ns, start_ns);
            assert_eq!(control.retired(), [0, 0]);
            assert_eq!(
                cell(control, NVME_IDENTIFY_BUFFER + 4),
                u32::from_le_bytes(*b"OD-N")
            );
            let generation = control
                .bus()
                .physical_memory()
                .code_page_generation(address)
                .unwrap();
            ready.store(true, SEQ);
            control.resume();
            assert!(control.wait_finished(WAIT));
            generation
        });
        assert_eq!(report.finished, ParallelFinish::BudgetExhausted);
        assert_joined(&report, 2);
        assert_eq!(report.cpus[0].calls, 2);
        assert!(report.cpus.iter().all(|cpu| cpu.retired == 0));
        assert_eq!(board.clock().now_ns(), start_ns);
        assert_eq!(
            board
                .bus_mut()
                .physical_memory()
                .code_page_generation(address)
                .unwrap(),
            generation,
        );
    }

    #[test]
    fn an_ioapic_pit_edge_reaches_the_ap_with_the_bsp_masked() {
        use crate::{AccessWidth, IO_APIC_MMIO_BASE, PortAddress};

        for ap_starts_halted in [true, false] {
            let mut board = board(2);
            assert!(board.enable_pit_ioapic_irq());
            system(&mut board, false);
            if ap_starts_halted {
                patch(&mut board, CODE_1, &[0xFA, 0xF4]); // cli; hlt
            } else {
                let code = [
                    mov_ebx(COUNTER_1 as u32),
                    vec![
                        0x8B, 0x03, // mov eax, [rbx]
                        0x85, 0xC0, // test eax, eax
                        0x74, 0xFA, // jz back to mov
                        0xFA, 0xF4, // cli; hlt
                    ],
                ]
                .concat();
                patch(&mut board, CODE_1, &code);
            }
            let cr3 = tables(&mut board, TABLES_1, 0x60_0000, 0);
            let mut ap = long_state(CODE_1, STACK_1, cr3, true);
            if ap_starts_halted {
                ap.activity = Some(CpuActivity::Halted);
            }
            install(&mut board, 1, ap, 0xD01);
            let bsp = board.vcpu_mut(0).unwrap();
            bsp.state.activity = Some(CpuActivity::Halted);
            bsp.state.rflags = 2;

            let bus = board.bus_mut();
            bus.write_mmio_shared(
                1,
                GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + 0xf0),
                AccessWidth::Dword,
                0x1ff,
            )
            .unwrap();
            for (offset, value) in [
                (0, 0x15),
                (0x10, 1 << 24), // INTIN2 destination APIC ID 1
                (0, 0x14),
                (0x10, u64::from(TO_AP)), // unmask and route as an edge
            ] {
                bus.write_mmio_shared(
                    0,
                    GuestPhysicalAddress::new(IO_APIC_MMIO_BASE + offset),
                    AccessWidth::Dword,
                    value,
                )
                .unwrap();
            }
            for (port, value) in [(0x43, 0x34), (0x40, 10), (0x40, 0)] {
                bus.write_port(PortAddress::new(port), AccessWidth::Byte, value)
                    .unwrap();
            }

            let (report, ()) = run(&mut board, config(10_000, 9_000), |control| {
                assert!(control.wait_finished(WAIT));
            });
            assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
            assert_joined(&report, 2);
            assert_eq!(
                (report.cpus[0].wakes, report.cpus[1].wakes),
                (0, u64::from(ap_starts_halted)),
                "{report:?}",
            );
            assert_eq!(dword(&mut board, COUNTER_1), 1);
            assert_eq!(report.cpus[0].retired, 0);
            assert_eq!(
                board.local_apic_of(1).unwrap().vector_state(TO_AP),
                (false, false),
            );
            assert_eq!(report.idle_jumps, u64::from(ap_starts_halted));
            assert!(
                (8_381..8_421).contains(&board.clock().now_ns()),
                "{report:?}"
            );
        }
    }

    #[test]
    fn ap_pit_programming_ends_each_parallel_quantum_at_the_port_write() {
        let mut board = board(2);
        assert!(board.enable_pit_ioapic_irq());
        let bsp = board.vcpu_mut(0).unwrap();
        bsp.state.activity = Some(CpuActivity::Halted);
        bsp.state.rflags = 2;

        let cr3 = tables(&mut board, TABLES_1, 0x60_0000, 0);
        let code = [
            imm(&[0xBA], 0x43), // mov edx, control port
            mov_eax(0x34),
            vec![0xEE],         // out dx, al
            imm(&[0xBA], 0x40), // mov edx, channel 0
            mov_eax(10),
            vec![0xEE],
            mov_eax(0),
            vec![0xEE],
            mov_eax(0x1234),
            vec![0xF4], // hlt
        ]
        .concat();
        patch(&mut board, CODE_1, &code);
        let mut ap = long_state(CODE_1, STACK_1, cr3, false);
        ap.rflags = 2;
        install(&mut board, 1, ap, 0xD01);
        let calls = Mutex::new(Vec::new());
        let quantum = |call: QuantumCall<'_, '_>| {
            assert_eq!(call.cpu, 1);
            let result = fixture_quantum(call)?;
            lock(&calls).push(result.retired);
            Ok(result)
        };
        let (report, ()) = run_threads(&mut board, config(64, 64), &quantum, |control| {
            assert!(control.wait_finished(WAIT));
        })
        .unwrap();
        assert_eq!(report.finished, ParallelFinish::AllIdle);
        assert_joined(&report, 2);
        assert_eq!(*lock(&calls), [3, 3, 2, 2]);
        assert_eq!(
            board.vcpu(1).unwrap().state.gpr[Register::Rax as usize],
            0x1234
        );
    }

    #[test]
    fn a_guest_pm_stop_retires_the_port_write_and_preempts_the_next_instruction() {
        use crate::acpi_pm::{ACPI_PM_BASE_PORT, ACPI_RESET_REG, ACPI_RESET_VALUE};
        use crate::clock_owner::NS_PER_RETIRED;
        use od_isa::real_mode_exec::EFLAGS_IF;

        for (port, value, reason, budget) in [
            (ACPI_PM_BASE_PORT + 5, 0x34, MachineStopReason::Shutdown, 64),
            (
                ACPI_RESET_REG,
                ACPI_RESET_VALUE,
                MachineStopReason::Reset,
                3,
            ),
        ] {
            let mut board = board(2);
            let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
            let [port_low, port_high] = port.to_le_bytes();
            patch(
                &mut board,
                CODE_1,
                &[
                    0xBA, port_low, port_high, 0x00, 0x00, // mov edx, port
                    0xB0, value, // mov al, value
                    0xEE,  // out dx, al
                    0xF4,  // must not retire
                ],
            );
            let bsp = board.vcpu_mut(BOOT_VCPU_INDEX).unwrap();
            bsp.state.activity = Some(CpuActivity::Halted);
            bsp.state.rflags &= !u64::from(EFLAGS_IF);
            install(
                &mut board,
                1,
                long_state(CODE_1, STACK_1, cr3, false),
                0xD01,
            );
            let start_ns = board.clock().now_ns();
            let (report, ()) = run(&mut board, config(64, budget), |control| {
                assert!(control.wait_finished(WAIT));
                assert_eq!(control.retired()[1], 3);
                assert_eq!(control.now_ns(), start_ns + 3 * NS_PER_RETIRED);
            });
            assert_eq!(
                report.finished,
                ParallelFinish::GuestStop(reason),
                "{report:?}"
            );
            assert_joined(&report, 2);
            assert_eq!(report.cpus[1].calls, 1);
            assert_eq!(board.vcpu(1).unwrap().state.rip, CODE_1 + 8);
            assert_eq!(board.acpi_pm().unwrap().stop_request(), Some(reason));

            let (repeated, ()) = run(&mut board, config(64, 64), |control| {
                assert!(control.wait_finished(WAIT));
                assert_eq!(control.retired()[1], 0);
                assert_eq!(control.now_ns(), start_ns + 3 * NS_PER_RETIRED);
            });
            assert_eq!(repeated.finished, ParallelFinish::GuestStop(reason));
            assert_joined(&repeated, 2);
            assert_eq!(board.vcpu(1).unwrap().state.rip, CODE_1 + 8);
        }
    }

    #[test]
    fn a_latched_pm_stop_takes_precedence_at_parallel_startup() {
        use crate::acpi_pm::{
            ACPI_PM_BASE_PORT, ACPI_PM_TMR_BLK, ACPI_RESET_REG, ACPI_RESET_VALUE,
        };
        use crate::{AccessWidth, PortAddress};
        use od_isa::real_mode_exec::EFLAGS_IF;

        for (budget, activity, port, value, reason) in [
            (
                64,
                CpuActivity::Halted,
                ACPI_PM_BASE_PORT + 5,
                0x34,
                MachineStopReason::Shutdown,
            ),
            (
                0,
                CpuActivity::Running,
                ACPI_RESET_REG,
                ACPI_RESET_VALUE,
                MachineStopReason::Reset,
            ),
        ] {
            let mut board = board(2);
            let bsp = board.vcpu_mut(0).unwrap();
            bsp.state.activity = Some(activity);
            bsp.state.rflags &= !u64::from(EFLAGS_IF);
            let initial_state = bsp.state.clone();
            board
                .bus_mut()
                .read_port(PortAddress::new(ACPI_PM_TMR_BLK), AccessWidth::Dword)
                .unwrap();
            board
                .bus_mut()
                .write_port(PortAddress::new(port), AccessWidth::Byte, u32::from(value))
                .unwrap();
            let pm = board.acpi_pm().unwrap();
            assert_eq!(pm.stop_request(), Some(reason));
            assert!(pm.timer_tick_pending());
            let start_ns = board.clock().now_ns();
            let (report, ()) = run(&mut board, config(64, budget), |control| {
                assert!(control.wait_finished(WAIT));
            });
            assert_eq!(
                report.finished,
                ParallelFinish::GuestStop(reason),
                "{report:?}"
            );
            assert_joined(&report, 2);
            assert!(
                report
                    .cpus
                    .iter()
                    .all(|cpu| cpu.calls == 0 && cpu.retired == 0)
            );
            assert_eq!(board.clock().now_ns(), start_ns);
            assert_eq!(board.vcpu(0).unwrap().state, initial_state);
            assert!(pm.timer_tick_pending());
        }
    }

    #[test]
    fn a_latched_pm_stop_leaves_startup_nvme_work_pending() {
        use crate::acpi_pm::{ACPI_RESET_REG, ACPI_RESET_VALUE};
        use crate::{AccessWidth, PortAddress};

        let (mut board, doorbell, _) = nvme_identify_board();
        board
            .bus_mut()
            .write_physical(
                GuestPhysicalAddress::new(doorbell),
                AccessWidth::Dword,
                1,
                |_| {},
            )
            .unwrap();
        board
            .bus_mut()
            .write_port(
                PortAddress::new(ACPI_RESET_REG),
                AccessWidth::Byte,
                u32::from(ACPI_RESET_VALUE),
            )
            .unwrap();
        let nvme = board.nvme().unwrap();
        assert!(nvme.doorbell_pending());
        let start_ns = board.clock().now_ns();

        let (report, ()) = run(&mut board, config(64, 0), |control| {
            assert!(control.wait_finished(WAIT));
        });
        assert_eq!(
            report.finished,
            ParallelFinish::GuestStop(MachineStopReason::Reset)
        );
        assert_joined(&report, 2);
        assert!(
            report
                .cpus
                .iter()
                .all(|cpu| cpu.calls == 0 && cpu.retired == 0)
        );
        assert!(nvme.doorbell_pending());
        assert_eq!(dword(&mut board, NVME_IDENTIFY_BUFFER + 4), 0);
        assert_eq!(dword(&mut board, NVME_IDENTIFY_CQ + 12), 0);
        assert_eq!(board.clock().now_ns(), start_ns);
    }

    #[test]
    fn rdtsc_cuts_shared_quanta_and_charges_the_clock_for_each_vcpu_read() {
        use crate::clock_owner::NS_PER_RETIRED;
        use od_isa::real_mode_exec::EFLAGS_IF;

        const RDTSC_TICK_NS: u64 = 1_000;
        for readers in [1, 2] {
            let mut board = board(2);
            let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
            let code = [
                0x0F, 0x31, // rdtsc
                0x89, 0xC3, // mov ebx, eax
                0x0F, 0x31, // rdtsc
                0xF4, // hlt
            ];
            for (cpu, rip, rsp) in [(0, CODE_0, STACK_0), (1, CODE_1, STACK_1)] {
                patch(&mut board, rip, &code);
                let mut state = long_state(rip, rsp, cr3, false);
                if cpu == BOOT_VCPU_INDEX && readers == 1 {
                    state.activity = Some(CpuActivity::Halted);
                    state.rflags &= !u64::from(EFLAGS_IF);
                }
                install(&mut board, cpu, state, 0xD01);
            }
            let start_clock = board.clock();
            let quantum = |call: QuantumCall<'_, '_>| {
                with_set(|set| fixture_quantum_with_set(call, &set.with_rdtsc()))
            };
            let (report, ()) = run_threads(&mut board, config(64, 64), &quantum, |control| {
                assert!(control.wait_finished(WAIT));
            })
            .unwrap();

            assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
            assert_joined(&report, 2);
            assert_eq!(report.idle_jumps, 0, "{report:?}");
            assert_eq!(
                board.clock().now_ns() - start_clock.now_ns(),
                readers * (4 * NS_PER_RETIRED + 2 * RDTSC_TICK_NS),
            );
            for cpu in (2 - readers as usize)..2 {
                assert_eq!(report.cpus[cpu].calls, 3);
                assert_eq!(report.cpus[cpu].retired, 4);
                let state = &board.vcpu(cpu).unwrap().state;
                let first = state.gpr[Register::Rbx as usize];
                let second = state.gpr[Register::Rax as usize];
                assert!(second > first, "{state:?}");
                if readers == 1 {
                    assert_eq!(first, start_clock.tsc());
                    let mut next_clock = start_clock;
                    next_clock
                        .advance_time_ns(NS_PER_RETIRED + RDTSC_TICK_NS)
                        .unwrap();
                    assert_eq!(second, next_clock.tsc());
                }
            }
        }
    }

    #[test]
    fn a_parallel_quantum_accounts_for_ticks_already_queued_by_its_peer() {
        use crate::acpi_pm::ACPI_PM_TMR_READ_TICK_NS;
        use crate::clock_owner::NS_PER_RETIRED;
        use crate::pit_8254::PIT_PORT_B_READ_TICK_NS;
        use crate::{ACPI_PM_TMR_BLK, AccessWidth, PortAddress};

        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
        for (cpu, rip, stack) in [(0, CODE_0, STACK_0), (1, CODE_1, STACK_1)] {
            patch(&mut board, rip, &[0xf4]); // HLT
            install(&mut board, cpu, long_state(rip, stack, cr3, false), 0xd01);
        }
        // Standalone host inspection must not count as work of either worker.
        board
            .bus_mut()
            .read_port(PortAddress::new(ACPI_PM_TMR_BLK), AccessWidth::Dword)
            .unwrap();
        let start_ns = board.clock().now_ns();
        let queued = AtomicBool::new(false);
        let sampled = AtomicBool::new(false);
        let charged = AtomicU64::new(0);
        let wait = |flag: &AtomicBool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !flag.load(SEQ) {
                assert!(Instant::now() < deadline, "the other vCPU did not arrive");
                thread::yield_now();
            }
        };
        let (report, ended) = run_threads(
            &mut board,
            config(16, 64),
            &|call| {
                if call.cpu == 0 && !queued.load(SEQ) {
                    // Hold this call open after the device access, as if vCPU 0 were
                    // preempted between the port reads and returning its quantum.
                    for (port, width) in [
                        (ACPI_PM_TMR_BLK, AccessWidth::Dword),
                        (0x61, AccessWidth::Byte),
                    ] {
                        call.env
                            .bus
                            .read_port_shared(PortAddress::new(port), width)
                            .unwrap();
                    }
                    queued.store(true, SEQ);
                    wait(&sampled);
                }
                let first_peer_call = call.cpu == 1 && !sampled.load(SEQ);
                if first_peer_call {
                    wait(&queued);
                }
                let executed = fixture_quantum(call)?;
                if first_peer_call {
                    charged.store(executed.device_ns, SEQ);
                    sampled.store(true, SEQ);
                }
                Ok(executed)
            },
            |control| control.wait_finished(Duration::from_secs(15)),
        )
        .unwrap();
        assert!(ended);
        assert_eq!(report.finished, ParallelFinish::AllIdle);
        let device_ns = ACPI_PM_TMR_READ_TICK_NS + PIT_PORT_B_READ_TICK_NS;
        assert_eq!(charged.load(SEQ), device_ns);
        assert_eq!(report.idle_jumps, 0);
        assert!(report.cpus.iter().all(|cpu| cpu.retired == 1));
        assert_eq!(
            board.clock().now_ns() - start_ns,
            2 * NS_PER_RETIRED + device_ns
        );
    }

    #[test]
    fn a_long_mode_error_charges_the_retired_prefix_before_finishing() {
        let mut board = board(1);
        let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
        patch(&mut board, CODE_0, &[0xb8, 0x78, 0x56, 0x34, 0x12, 0xf1]);
        install(
            &mut board,
            0,
            long_state(CODE_0, STACK_0, cr3, false),
            0xd01,
        );
        let start_ns = board.clock().now_ns();
        let (report, at_finish) =
            run_threads(&mut board, config(16, 64), &fixture_quantum, |control| {
                assert!(control.wait_finished(WAIT));
                (control.retired()[0], control.now_ns() - start_ns)
            })
            .unwrap();
        assert!(
            matches!(
                &report.finished,
                ParallelFinish::Error {
                    cpu: 0,
                    error: FirmwareBoardError::VcpuQuantum { .. }
                }
            ),
            "{:?}",
            report.finished
        );
        assert_joined(&report, 1);
        assert_eq!(
            board.vcpu(0).unwrap().state.gpr[Register::Rax as usize],
            0x1234_5678
        );
        assert_eq!(board.vcpu(0).unwrap().state.rip, CODE_0 + 5);
        assert_eq!(report.cpus[0].retired, 1);
        assert_eq!(at_finish, (1, crate::clock_owner::NS_PER_RETIRED));
        assert_eq!(
            board.clock().now_ns() - start_ns,
            crate::clock_owner::NS_PER_RETIRED
        );
    }

    #[test]
    fn stop_acknowledges_a_worker_already_waiting_for_an_exclusive_owner() {
        let mut board = board(2);
        for (cpu, rip, stack) in [(0, CODE_0, STACK_0), (1, CODE_1, STACK_1)] {
            install(
                &mut board,
                cpu,
                long_state(rip, stack, TABLES_0, false),
                0xd01,
            );
        }
        let acquired = AtomicBool::new(false);
        let (report, ()) = run_threads(
            &mut board,
            config(16, 64),
            &|call| {
                if call.cpu == 0 && !acquired.load(SEQ) {
                    call.gate.acquire_for_descriptor().unwrap();
                    acquired.store(true, SEQ);
                    let deadline = Instant::now() + Duration::from_secs(60);
                    while !(call.stop)() && Instant::now() < deadline {
                        thread::yield_now();
                    }
                    let saw_stop = (call.stop)();
                    call.gate.release();
                    assert!(saw_stop, "the owner must observe STOP");
                }
                Ok(Executed::default())
            },
            |control| {
                assert!(control.wait_until(Duration::from_secs(60), || {
                    let peer = &control.shared.cells[1];
                    acquired.load(SEQ) && peer.quiet.load(SEQ) && !peer.safe.load(SEQ)
                }));
                control
                    .stop_all(Duration::from_secs(10))
                    .expect("both workers acknowledge");
                assert!(control.shared.cells.iter().all(|cpu| cpu.safe.load(SEQ)));
            },
        )
        .unwrap();
        assert_eq!(report.finished, ParallelFinish::Stopped);
        assert!(report.cpus.iter().all(|cpu| cpu.retired == 0));
    }

    #[test]
    fn two_workers_loading_one_available_tss_cannot_both_commit_tr() {
        const TSS: u64 = 0x18;
        const AVAILABLE: u64 = 0x0000_8900_0000_0067;
        const BASE_HIGH: u64 = 0x1234;
        let mut board = board(2);
        let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
        system(&mut board, false);
        patch(&mut board, GDT_AT + TSS, &AVAILABLE.to_le_bytes());
        patch(&mut board, GDT_AT + TSS + 8, &BASE_HIGH.to_le_bytes());
        for (cpu, rip, stack) in [(0, CODE_0, STACK_0), (1, CODE_1, STACK_1)] {
            patch(&mut board, rip, &[0x0f, 0x00, 0xd8, 0xf4]); // LTR AX; HLT
            let mut guest = long_state(rip, stack + 0x800, cr3, true);
            guest.gpr[Register::Rax as usize] = TSS;
            guest.gdtr.as_mut().unwrap().limit = (TSS + 15) as u16;
            install(&mut board, cpu, guest, 0xd01);
        }

        let entered = AtomicUsize::new(0);
        let (report, ()) = run_threads(
            &mut board,
            config(16, 64),
            &|call| {
                let deadline = Instant::now() + Duration::from_secs(60);
                entered.fetch_or(1 << call.cpu, SEQ);
                while entered.load(SEQ) != 3 {
                    assert!(Instant::now() < deadline, "both workers must reach LTR");
                    thread::yield_now();
                }
                fixture_quantum(call)
            },
            |control| assert!(control.wait_finished(Duration::from_secs(60))),
        )
        .unwrap();

        let ParallelFinish::Error {
            cpu: loser,
            error: FirmwareBoardError::VcpuQuantum { detail, .. },
        } = &report.finished
        else {
            panic!("the second LTR must fault: {report:?}");
        };
        assert!(
            detail.contains("vector: 13") && detail.contains("error_code: Some(24)"),
            "{detail}"
        );
        let winner = 1 - *loser;
        let tr = board.vcpu(winner).unwrap().state.tr.as_ref().unwrap();
        assert_eq!(tr.selector, TSS as u16);
        assert_eq!(tr.base, BASE_HIGH << 32);
        assert_eq!(tr.access, 0x8b);
        assert!(board.vcpu(*loser).unwrap().state.tr.is_none());
        assert_eq!(report.cpus[*loser].retired, 0);
        assert!(report.cpus.iter().all(|cpu| cpu.split_locks == 0));
        let image = read(&mut board, GDT_AT + TSS, 16);
        assert_eq!(
            u64::from_le_bytes(image[..8].try_into().unwrap()),
            AVAILABLE | (1 << 41)
        );
        assert_eq!(
            u64::from_le_bytes(image[8..].try_into().unwrap()),
            BASE_HIGH
        );
    }

    #[test]
    fn interpreter_keeps_the_fault_limit_across_quantum_boundaries() {
        const LIMIT: u64 = 256;
        const STACK_TOP: u64 = STACK_0 + 0x4800;
        let mut board = board(1);
        let cr3 = tables(&mut board, TABLES_0, 0x20_0000, 0);
        system(&mut board, false);
        let mut gate = read(&mut board, IDT_AT + u64::from(TO_BSP) * 16, 16);
        gate[..2].copy_from_slice(&(CODE_0 as u16).to_le_bytes());
        gate[6..8].copy_from_slice(&((CODE_0 >> 16) as u16).to_le_bytes());
        patch(&mut board, IDT_AT + 14 * 16, &gate);
        patch(&mut board, CODE_0, &[0xf0, 0xff, 0x03]); // LOCK INC dword [RBX]
        let mut state = long_state(CODE_0, STACK_TOP, cr3, true);
        state.gpr[Register::Rbx as usize] = 0x4000_0000; // An absent PDPT entry.
        install(&mut board, 0, state, 0xd01);

        let (report, ()) = run_threads(
            &mut board,
            config(64, 1024),
            &|call| {
                let memory = call.env.bus.physical_memory();
                let frame =
                    GuestPhysicalAddress::new(call.vcpu.state.gpr[Register::Rsp as usize] - 8);
                let before = memory.code_page_generation(frame).unwrap();
                // Split the fault chain at each newly written frame without retiring a step.
                let mut stop =
                    || (call.stop)() || memory.code_page_generation(frame).unwrap() != before;
                with_set(|set| {
                    run_vcpu_set_quantum_shared(
                        call.env,
                        call.vcpu,
                        call.cpu,
                        call.lapic,
                        call.source,
                        Some(call.gate),
                        call.budget,
                        &mut stop,
                        set,
                        call.consecutive_faults,
                        call.machine_stop,
                        call.nvme_pending,
                    )
                })
            },
            |control| assert!(control.wait_finished(Duration::from_secs(60))),
        )
        .unwrap();

        assert!(
            matches!(&report.finished, ParallelFinish::Error {
            cpu: 0, error: FirmwareBoardError::VcpuQuantum { detail, .. }
        } if detail.contains("RepeatLimit")),
            "{report:?}"
        );
        assert_eq!(report.cpus[0].calls, LIMIT);
        assert_eq!(report.cpus[0].retired, 0);
        assert_eq!(
            board.state().gpr[Register::Rsp as usize],
            STACK_TOP - 48 * LIMIT
        );
    }

    fn config(quantum: u64, budget: u64) -> ParallelConfig {
        ParallelConfig {
            quantum,
            budget,
            split_timeout: Duration::from_secs(20),
            end_when_idle: true,
            pre_sleep: None,
            barrier_window: None,
            standing_window: None,
        }
    }

    /// The run does not end when everything sleeps: the test posts the events itself.
    fn open_config(quantum: u64) -> ParallelConfig {
        ParallelConfig {
            end_when_idle: false,
            ..config(quantum, u64::MAX)
        }
    }

    fn run<R>(
        board: &mut LoadedFirmwareBoard,
        config: ParallelConfig,
        controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
    ) -> (ParallelReport, R) {
        run_threads(board, config, &fixture_quantum, controller).expect("the board splits")
    }

    fn assert_joined(report: &ParallelReport, vcpus: usize) {
        assert_eq!(report.threads_started, vcpus, "{report:?}");
        assert_eq!(
            report.threads_exited, vcpus,
            "no vCPU thread is left: {report:?}"
        );
    }

    fn cell(control: &ParallelControl<'_, '_>, at: u64) -> u32 {
        control
            .bus()
            .physical_memory()
            .load_u32(GuestPhysicalAddress::new(at))
            .unwrap()
    }

    const WAIT: Duration = Duration::from_secs(60);

    /// The wake sink hears every request posted to a Local APIC (a Fixed vector from a device or
    /// an IPI, INIT, SIPI, NMI), with the index of the target, and nothing the vCPU does to itself
    /// by reading; a snapshot restore keeps it; with no sink nothing is called.
    #[test]
    fn the_wake_sink_hears_every_request_posted_to_a_local_apic() {
        let lapic = crate::LocalApic::with_cpus(2).unwrap();
        let (a0, a1) = (lapic.handle_for(0).unwrap(), lapic.handle_for(1).unwrap());
        a0.set_global_enabled(true).unwrap();
        a1.set_global_enabled(true).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = {
            let seen = Arc::clone(&seen);
            Arc::new(move |cpu: usize| lock(&seen).push(cpu))
        };
        a0.set_wake_sink(Some(sink));
        let heard = || std::mem::take(&mut *lock(&seen));
        a1.request_fixed_edge(0x40).unwrap();
        assert_eq!(heard(), vec![1], "a device vector");
        a0.write_icr(0, 1, 0x41).unwrap();
        assert_eq!(heard(), vec![1], "a Fixed IPI");
        a0.write_icr(0, 0, 0x42).unwrap();
        assert_eq!(heard(), vec![0], "a Fixed IPI to itself");
        a0.write_icr(0, 1, 0xC500).unwrap();
        assert_eq!(heard(), vec![1], "INIT");
        a0.write_icr(0, 1, 0x0608).unwrap();
        assert_eq!(heard(), vec![1], "SIPI");
        a0.write_icr(0, 1, 0x0400).unwrap();
        assert_eq!(heard(), vec![1], "NMI");
        let _ = (
            a1.pending(),
            a1.eligible(),
            a1.vector_state(0x40),
            a1.pending_nmi(),
        );
        assert_eq!(heard(), Vec::<usize>::new(), "reading posts nothing");
        let state = a0.snapshot_pair();
        a0.restore_pair(&state).unwrap();
        a1.request_fixed_edge(0x43).unwrap();
        assert_eq!(heard(), vec![1], "a restore keeps the sink");
        a0.set_wake_sink(None);
        a1.request_fixed_edge(0x44).unwrap();
        assert_eq!(heard(), Vec::<usize>::new(), "no sink, no call");
    }

    #[test]
    fn two_vcpu_threads_exchange_fixed_ipis_and_end_asleep() {
        let mut board = exchange_board();
        let (report, ()) = run(&mut board, config(32, 1_000_000), |control| {
            assert!(control.wait_finished(WAIT), "the run does not end");
        });
        assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
        assert_joined(&report, 2);
        assert_eq!(board.vcpu(0).unwrap().activity(), CpuActivity::Halted);
        assert_eq!(board.vcpu(1).unwrap().activity(), CpuActivity::Halted);
        assert_eq!(dword(&mut board, COUNTER_1), ROUNDS);
        assert_eq!(dword(&mut board, COUNTER_0), ROUNDS);
        for cpu in 0..2 {
            for vector in [TO_AP, TO_BSP] {
                assert_eq!(
                    board.local_apic_of(cpu).unwrap().vector_state(vector),
                    (false, false),
                    "cpu {cpu} vector {vector:#x}"
                );
            }
        }
        // The wake sink and the execution mode are back to what the board had.
        assert_eq!(board.execution_mode(), ExecutionMode::Interleaved);
    }

    #[test]
    fn the_exchange_is_right_every_time_under_every_quantum() {
        for quantum in [1u64, 2, 3, 7, 100, 1024] {
            for _ in 0..25 {
                let mut board = exchange_board();
                let (report, ()) = run(&mut board, config(quantum, 1_000_000), |control| {
                    assert!(control.wait_finished(WAIT), "quantum {quantum}: no end");
                });
                assert_eq!(
                    report.finished,
                    ParallelFinish::AllIdle,
                    "quantum {quantum}: {report:?}"
                );
                assert_joined(&report, 2);
                assert_eq!(dword(&mut board, COUNTER_1), ROUNDS, "quantum {quantum}");
                assert_eq!(dword(&mut board, COUNTER_0), ROUNDS, "quantum {quantum}");
            }
        }
    }

    /// A vCPU asleep in `HLT` with IF = 1 and its Local APIC enabled, the boot processor asleep for
    /// good (IF = 0): the fixture of the wake-up tests.
    fn sleeping_ap_board() -> LoadedFirmwareBoard {
        let mut board = board(2);
        system(&mut board, false);
        patch(&mut board, CODE_1, &ap_program(false));
        let cr3_1 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3_1, true),
            0xD01,
        );
        let mut sleeper = long_state(CODE_0, STACK_0, cr3_1, false);
        sleeper.rflags = 2;
        sleeper.activity = Some(CpuActivity::Halted);
        install(&mut board, 0, sleeper, 0xD01);
        board
    }

    /// The same, but the boot processor runs a loop (`jmp .`, IF = 0) for good: the machine is never
    /// all idle, so the idle arbiter cannot heal a lost wake-up by finding the pending vector of a
    /// sleeper (it does when everything sleeps), and a thread that sleeps through a post stays
    /// asleep.
    fn sleeping_ap_board_with_a_busy_boot_processor() -> LoadedFirmwareBoard {
        let mut board = sleeping_ap_board();
        patch(&mut board, CODE_0, &[0xEB, 0xFE]);
        let cr3_1 = board.vcpu(1).unwrap().state.cr3.unwrap();
        install(
            &mut board,
            0,
            {
                let mut spinner = long_state(CODE_0, STACK_0, cr3_1, false);
                spinner.rflags = 2;
                spinner
            },
            0xD01,
        );
        board
    }

    fn wait_for_sleep(control: &ParallelControl<'_, '_>, cpu: usize) {
        assert!(
            control.wait_until(WAIT, || {
                control.activity(cpu) == SchedActivity::Halted && control.cpu_report(cpu).sleeps > 0
            }),
            "vCPU {cpu} does not go to sleep"
        );
    }

    /// Step 2: a kick that comes right before the vCPU falls asleep is not lost. The controller
    /// posts the next IPI after a delay of a different length every round, the moment it sees the
    /// handler's count, so the post sweeps the path of the AP from the `IRETQ` to the park (some
    /// IPIs come before the AP has halted and need no wake, some after it sleeps): thousands of
    /// times, each wait bounded. The window itself is held open by the test below.
    #[test]
    fn a_wake_up_posted_around_the_sleep_is_never_lost() {
        wake_up_around_the_sleep(3000);
    }

    /// The long form: `OD_S6_WAKE_ROUNDS` rounds (default 50 000).
    #[test]
    #[ignore = "long: OD_S6_WAKE_ROUNDS=50000 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::wake_up_many_rounds --nocapture"]
    fn wake_up_many_rounds() {
        let rounds = std::env::var("OD_S6_WAKE_ROUNDS")
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(50_000);
        wake_up_around_the_sleep(rounds);
        wake_up_between_the_look_and_the_park(rounds / 10);
    }

    fn wake_up_around_the_sleep(rounds: u32) {
        let mut board = sleeping_ap_board_with_a_busy_boot_processor();
        let lapic = board.local_apic_of(1).unwrap();
        let sender = board.local_apic_of(0).unwrap();
        let rounds_total = rounds;
        let (report, ()) = run(&mut board, open_config(8), |control| {
            wait_for_sleep(control, 1);
            for round in 1..=rounds_total {
                let jitter = u64::from(round).wrapping_mul(2_654_435_761) % 6000;
                for _ in 0..jitter {
                    std::hint::spin_loop();
                }
                sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
                // A tight poll, so that the next post follows the handler's count at once.
                let deadline = Instant::now() + Duration::from_secs(20);
                while cell(control, COUNTER_1) < round {
                    assert!(
                        Instant::now() < deadline,
                        "round {round}: the IPI was lost (counter {}, vectors {:?})",
                        cell(control, COUNTER_1),
                        lapic.vector_state(TO_AP)
                    );
                    std::hint::spin_loop();
                }
            }
            // The last handler counts first and then writes its EOI: let it finish.
            assert!(
                control.wait_until(Duration::from_secs(20), || {
                    lapic.vector_state(TO_AP) == (false, false)
                        && control.activity(1) == SchedActivity::Halted
                }),
                "the last handler does not end"
            );
        });
        assert_joined(&report, 2);
        assert_eq!(dword(&mut board, COUNTER_1), rounds_total);
        assert_eq!(
            board.local_apic_of(1).unwrap().vector_state(TO_AP),
            (false, false)
        );
    }

    // The state of the injection of the test below (a plain `fn` hook cannot capture).
    static HOLD_AT_SLEEP: AtomicBool = AtomicBool::new(false);
    static HELD_AT_SLEEP: AtomicBool = AtomicBool::new(false);
    static RELEASE_SLEEP: AtomicBool = AtomicBool::new(false);
    static TEST_INJECTION: Mutex<()> = Mutex::new(());

    fn hold_vcpu_1_at_the_start_of_its_sleep(cpu: usize) {
        if cpu == 1 && HOLD_AT_SLEEP.load(Ordering::SeqCst) {
            HELD_AT_SLEEP.store(true, Ordering::SeqCst);
            while !RELEASE_SLEEP.load(Ordering::SeqCst) {
                std::hint::spin_loop();
            }
            HELD_AT_SLEEP.store(false, Ordering::SeqCst);
        }
    }

    /// Step 2, the window held open: the AP has looked at its Local APIC, found nothing and gone to
    /// sleep; the test holds it there (before the park), posts the IPI, and releases it. The kick
    /// that the post made (the flag, and the `unpark` token) must end the sleep at once: 500 rounds,
    /// each bounded. A kick that sets only the `unpark` token, or a flag that the sleeper clears
    /// after its look, loses it (the mutations recorded in MACHINE_CONTRACT.md, S6-34).
    #[test]
    fn a_wake_up_posted_while_the_vcpu_is_between_its_look_and_its_park_is_never_lost() {
        wake_up_between_the_look_and_the_park(500);
    }

    fn wake_up_between_the_look_and_the_park(rounds: u32) {
        let _injection = lock(&TEST_INJECTION);
        let mut board = sleeping_ap_board_with_a_busy_boot_processor();
        let sender = board.local_apic_of(0).unwrap();
        let config = ParallelConfig {
            pre_sleep: Some(hold_vcpu_1_at_the_start_of_its_sleep),
            ..open_config(8)
        };
        let rounds_total = rounds;
        HOLD_AT_SLEEP.store(false, Ordering::SeqCst);
        RELEASE_SLEEP.store(false, Ordering::SeqCst);
        let (report, ()) = run_threads(&mut board, config, &fixture_quantum, |control| {
            // Let the first sleep (the start) pass, then hold every later one.
            wait_for_sleep(control, 1);
            for round in 1..=rounds_total {
                RELEASE_SLEEP.store(false, Ordering::SeqCst);
                HELD_AT_SLEEP.store(false, Ordering::SeqCst);
                HOLD_AT_SLEEP.store(true, Ordering::SeqCst);
                // Wake the AP once, so that it comes round to its sleep and is held there.
                sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
                let deadline = Instant::now() + Duration::from_secs(20);
                while !HELD_AT_SLEEP.load(Ordering::SeqCst) {
                    assert!(
                        Instant::now() < deadline,
                        "round {round}: the AP does not come to sleep"
                    );
                    std::hint::spin_loop();
                }
                // It has handled the IPI of this round and looked at its APIC; post the next one
                // now: between the look and the park.
                let handled = cell(control, COUNTER_1);
                sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
                HOLD_AT_SLEEP.store(false, Ordering::SeqCst);
                RELEASE_SLEEP.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + Duration::from_secs(20);
                while cell(control, COUNTER_1) < handled + 1 {
                    assert!(
                        Instant::now() < deadline,
                        "round {round}: the IPI posted between the look and the park was lost"
                    );
                    std::hint::spin_loop();
                }
                // Back to sleep (held by the next round).
                assert!(control.wait_until(Duration::from_secs(20), || {
                    control.activity(1) == SchedActivity::Halted
                }));
            }
        })
        .unwrap();
        HOLD_AT_SLEEP.store(false, Ordering::SeqCst);
        RELEASE_SLEEP.store(true, Ordering::SeqCst);
        assert_joined(&report, 2);
        assert!(dword(&mut board, COUNTER_1) >= rounds_total, "{report:?}");
    }

    /// A vCPU asleep in `HLT` does not spin: while nothing is posted it makes no call and no step.
    #[test]
    fn a_sleeping_vcpu_runs_nothing_and_acknowledges_a_stop_at_once() {
        let mut board = sleeping_ap_board();
        let (report, ()) = run(&mut board, open_config(8), |control| {
            wait_for_sleep(control, 1);
            let before = (control.cpu_report(1), control.retired());
            thread::sleep(Duration::from_millis(100));
            assert_eq!(
                (control.cpu_report(1), control.retired()),
                before,
                "a sleeper makes no call and retires nothing"
            );
            let started = Instant::now();
            let proof = control
                .stop_all(Duration::from_secs(5))
                .expect("both vCPUs sleep");
            assert!(started.elapsed() < Duration::from_secs(1));
            assert_eq!(
                proof.acks,
                vec![
                    StopAck::Asleep(SchedActivity::Halted),
                    StopAck::Asleep(SchedActivity::Halted)
                ]
            );
            control.resume();
        });
        assert_joined(&report, 2);
        assert_eq!(report.cpus[1].calls, report.cpus[1].calls.max(1));
    }

    /// An IPI to a vCPU that sleeps while the machine is stopped does not start it before the
    /// resume, and is not lost: it stays in its Local APIC and is taken after the resume.
    #[test]
    fn an_ipi_to_a_sleeping_stopped_vcpu_waits_for_the_resume_and_is_not_lost() {
        for _ in 0..50 {
            let mut board = sleeping_ap_board();
            let lapic = board.local_apic_of(1).unwrap();
            let sender = board.local_apic_of(0).unwrap();
            let (report, ()) = run(&mut board, open_config(8), |control| {
                wait_for_sleep(control, 1);
                control.stop_all(Duration::from_secs(5)).expect("stop");
                let (calls, retired, now) = (
                    control.cpu_report(1).calls,
                    control.retired(),
                    control.now_ns(),
                );
                sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
                sender
                    .write_icr(0, 1, FIXED | u32::from(TO_AP + 1))
                    .unwrap();
                thread::sleep(Duration::from_millis(30));
                // Not started: no call, no step, the vector still waits in the IRR, the vCPU is
                // still halted, the clock stands.
                assert_eq!(control.cpu_report(1).calls, calls);
                assert_eq!(control.retired(), retired);
                assert_eq!(control.now_ns(), now);
                assert_eq!(cell(control, COUNTER_1), 0);
                assert_eq!(lapic.vector_state(TO_AP), (true, false));
                assert_eq!(control.activity(1), SchedActivity::Halted);
                control.resume();
                assert!(
                    control.wait_until(Duration::from_secs(20), || cell(control, COUNTER_1) >= 1),
                    "the IPI posted during the stop was lost"
                );
                // The handler counts first and writes its EOI after (S6-36 review, m-1): wait for
                // the EOI and for the sleep, or the run may end with the vector still in service.
                assert!(
                    control.wait_until(Duration::from_secs(20), || {
                        lapic.vector_state(TO_AP) == (false, false)
                            && control.activity(1) == SchedActivity::Halted
                    }),
                    "the handler does not end"
                );
            });
            assert_joined(&report, 2);
            assert_eq!(dword(&mut board, COUNTER_1), 1);
            assert_eq!(
                board.local_apic_of(1).unwrap().vector_state(TO_AP),
                (false, false)
            );
        }
    }

    /// A vCPU that cannot reach a safe point (its call is stuck for a while): the stop times out
    /// with the vCPU named, the request is withdrawn, and nothing is lost; a later stop succeeds.
    #[test]
    fn a_stop_that_times_out_names_the_stuck_vcpu_changes_nothing_and_can_be_repeated() {
        let mut board = exchange_board();
        let stuck = AtomicBool::new(true);
        let entered = AtomicBool::new(false);
        let quantum = |call: QuantumCall<'_, '_>| {
            if call.cpu == 1 && stuck.swap(false, Ordering::SeqCst) {
                entered.store(true, Ordering::SeqCst);
                thread::sleep(Duration::from_millis(400));
            }
            fixture_quantum(call)
        };
        let (report, ()) = run_threads(&mut board, config(32, 1_000_000), &quantum, |control| {
            assert!(control.wait_until(WAIT, || entered.load(Ordering::SeqCst)));
            let error = control.stop_all(Duration::from_millis(60)).unwrap_err();
            assert_eq!(error, ParallelStopError::Timeout { unacked: vec![1] });
            assert!(!control.stopped(), "the request is withdrawn");
            let proof = control
                .stop_all(Duration::from_secs(20))
                .expect("the second stop");
            assert_eq!(proof.acks.len(), 2);
            control.resume();
            assert!(control.wait_finished(WAIT));
        })
        .unwrap();
        assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
        assert_joined(&report, 2);
        assert_eq!(dword(&mut board, COUNTER_1), ROUNDS);
        assert_eq!(dword(&mut board, COUNTER_0), ROUNDS);
    }

    #[test]
    fn an_error_during_stop_is_not_acknowledged_by_the_exiting_workers() {
        let mut board = exchange_board();
        let entered = AtomicBool::new(false);
        let quantum = |call: QuantumCall<'_, '_>| {
            if call.cpu == 1 {
                entered.store(true, SEQ);
                let deadline = Instant::now() + WAIT;
                while !(call.stop)() {
                    assert!(
                        Instant::now() < deadline,
                        "the controller must request STOP"
                    );
                    thread::yield_now();
                }
                return Err(FirmwareBoardError::Config("error during STOP").into());
            }
            fixture_quantum(call)
        };
        let (report, ()) = run_threads(&mut board, config(32, u64::MAX), &quantum, |control| {
            assert!(control.wait_until(WAIT, || entered.load(SEQ)));
            // Pause the controller after publishing the request, until the workers finish.
            control.shared.stop_all.store(true, SEQ);
            assert!(control.wait_finished(WAIT));
            let deadline = Instant::now() + WAIT;
            while control.shared.exited.load(SEQ) != control.vcpu_count() {
                assert!(Instant::now() < deadline, "workers must finish");
                thread::yield_now();
            }
            assert!(control.shared.cells.iter().all(|cell| cell.safe.load(SEQ)));
            let stopped = control.stopped();
            let access = control.with_vcpu(1, |_| ());
            assert_eq!(
                control.wait_for_stop(WAIT),
                Err(ParallelStopError::Finished)
            );
            assert!(!stopped);
            assert!(access.is_none());
            assert!(!control.shared.stop_all.load(SEQ));
        })
        .unwrap();
        assert!(matches!(
            report.finished,
            ParallelFinish::Error {
                cpu: 1,
                error: FirmwareBoardError::Config("error during STOP"),
            }
        ));
        assert_joined(&report, 2);
    }

    /// The exchange with `rounds` (at most 127) replies, so that a stop can land in the middle.
    fn long_exchange_board(rounds: u8) -> LoadedFirmwareBoard {
        let mut board = exchange_board();
        let mut code = count_and_eoi(COUNTER_0);
        let send = send_ipi_from_r8(TO_AP, 1);
        code.extend([0x41, 0x83, 0xF9, rounds]); // cmp r9d, rounds
        code.extend([0x73, send.len() as u8]); // jae over the send
        code.extend(send);
        code.extend([0x48, 0xCF]);
        patch(&mut board, HANDLER_0, &code);
        board
    }

    /// Stop in the middle of the IPI exchange: no counter grows while stopped, the clock stands,
    /// the vCPUs are where they were; after the resume the exchange ends right. Repeated.
    #[test]
    fn stopping_in_the_middle_of_an_ipi_exchange_and_resuming_ends_right() {
        stop_in_the_middle_of_an_exchange(100);
    }

    /// The long form: `OD_S6_STOP_RUNS` attempts (default 1000), results in the report of S6-34.
    #[test]
    #[ignore = "long: OD_S6_STOP_RUNS=1000 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::stop_in_the_middle_many_runs --nocapture"]
    fn stop_in_the_middle_many_runs() {
        let attempts = std::env::var("OD_S6_STOP_RUNS")
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(1000);
        stop_in_the_middle_of_an_exchange(attempts);
    }

    fn stop_in_the_middle_of_an_exchange(attempts: usize) {
        const ROUNDS_: u8 = 100;

        let mut landed_inside = 0;
        for attempt in 0..attempts {
            let mut board = long_exchange_board(ROUNDS_);
            let threshold = 40 + (attempt as u64 * 37) % 4000;
            let (report, inside) = run(&mut board, config(1, 10_000_000), |control| {
                let seen = control.wait_until(WAIT, || {
                    control.retired().iter().sum::<u64>() >= threshold || control.is_finished()
                });
                assert!(seen);
                let stop = control.stop_all(Duration::from_secs(10));
                let Ok(proof) = stop else {
                    // The exchange was already over: nothing to stop.
                    assert!(control.wait_finished(WAIT));
                    return false;
                };
                let snapshot = |c: &ParallelControl<'_, '_>| {
                    (
                        c.retired(),
                        (0..2)
                            .map(|cpu| c.cpu_report(cpu).calls)
                            .collect::<Vec<_>>(),
                        cell(c, COUNTER_0),
                        cell(c, COUNTER_1),
                        c.now_ns(),
                    )
                };
                let first = snapshot(control);
                assert_eq!(first.4, proof.now_ns);
                thread::sleep(Duration::from_millis(5));
                assert_eq!(snapshot(control), first, "nothing moves while stopped");
                control.resume();
                assert!(control.wait_finished(WAIT), "the exchange does not end");
                first.2 < u32::from(ROUNDS_) || first.3 < u32::from(ROUNDS_)
            });
            if inside {
                landed_inside += 1;
            }
            assert_eq!(
                report.finished,
                ParallelFinish::AllIdle,
                "attempt {attempt}: {report:?}"
            );
            assert_joined(&report, 2);
            assert_eq!(
                dword(&mut board, COUNTER_1),
                u32::from(ROUNDS_),
                "attempt {attempt}"
            );
            assert_eq!(
                dword(&mut board, COUNTER_0),
                u32::from(ROUNDS_),
                "attempt {attempt}"
            );
            for cpu in 0..2 {
                for vector in [TO_AP, TO_BSP] {
                    assert_eq!(
                        board.local_apic_of(cpu).unwrap().vector_state(vector),
                        (false, false)
                    );
                }
            }
        }
        eprintln!(
            "stop in the middle: {landed_inside} of {attempts} stops landed inside the exchange"
        );
        assert!(
            landed_inside >= attempts / 2,
            "only {landed_inside} of {attempts} stops landed inside the exchange"
        );
    }

    /// Step 5: what the controller reads after the acknowledgement is what the vCPU left: a guest
    /// that stores a counter and then increments the register shows `register - memory` of 0 or 1
    /// at every boundary, never anything else.
    #[test]
    fn the_stopped_state_and_the_ram_the_controller_reads_agree_with_what_the_vcpu_left() {
        let mut board = board(2);
        system(&mut board, false);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        // mov ebx, DATA; xor eax, eax; again: mov [rbx], eax; inc eax; jmp again
        let mut code = mov_ebx(DATA as u32);
        code.extend([0x31, 0xC0, 0x89, 0x03, 0xFF, 0xC0, 0xEB, 0xFA]);
        patch(&mut board, CODE_1, &code);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3, false),
            0xD01,
        );
        let mut sleeper = long_state(CODE_0, STACK_0, cr3, false);
        sleeper.rflags = 2;
        sleeper.activity = Some(CpuActivity::Halted);
        install(&mut board, 0, sleeper, 0xD01);
        let (report, ()) = run(&mut board, open_config(5), |control| {
            let mut last = 0u64;
            for round in 0..300 {
                thread::sleep(Duration::from_micros(100 + (round * 53) % 400));
                control.stop_all(Duration::from_secs(10)).expect("stop");
                let memory = u64::from(cell(control, DATA));
                let register = control
                    .with_vcpu(1, |vcpu| vcpu.state.gpr[Register::Rax as usize])
                    .expect("a stopped vCPU can be read");
                assert!(
                    register == memory || register == memory + 1,
                    "round {round}: eax {register} memory {memory}"
                );
                assert!(memory >= last, "round {round}: the counter went back");
                last = memory;
                control.resume();
            }
            assert!(last > 0);
        });
        assert_joined(&report, 2);
    }

    /// Step 7: the controller ending the run, with a vCPU that never stops by itself, joins every
    /// thread.
    #[test]
    fn the_end_of_the_run_stops_a_vcpu_in_a_loop_and_joins_every_thread() {
        let mut board = board(2);
        system(&mut board, false);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        patch(&mut board, CODE_1, &[0xEB, 0xFE]); // jmp .
        patch(&mut board, CODE_0, &[0xEB, 0xFE]);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3, false),
            0xD01,
        );
        install(
            &mut board,
            0,
            long_state(CODE_0, STACK_0, cr3, false),
            0xD01,
        );
        let (report, ()) = run(&mut board, config(1024, u64::MAX), |control| {
            thread::sleep(Duration::from_millis(50));
            assert!(!control.is_finished());
            control.finish();
        });
        assert_eq!(report.finished, ParallelFinish::Stopped);
        assert_joined(&report, 2);
        assert!(report.cpus[0].retired > 0 && report.cpus[1].retired > 0);
        // The budget ends a run as well.
        let (report, ()) = run(&mut board, config(1024, 50_000), |control| {
            assert!(control.wait_finished(WAIT));
        });
        assert_eq!(report.finished, ParallelFinish::BudgetExhausted);
        assert_joined(&report, 2);
        let total: u64 = report.cpus.iter().map(|cpu| cpu.retired).sum();
        assert!((50_000..50_000 + 2 * 1024).contains(&total), "{total}");
    }

    /// Step 7: the error of one vCPU (here an instruction outside the set) stops every thread and
    /// is the result; a panic in one thread the same.
    #[test]
    fn the_error_or_the_panic_of_one_vcpu_ends_the_run_for_all() {
        let mut board = board(2);
        system(&mut board, false);
        let cr3 = tables(&mut board, TABLES_1, 0x80_0000, 0);
        patch(&mut board, CODE_1, &[0x0F, 0x0B]); // ud2
        patch(&mut board, CODE_0, &[0xEB, 0xFE]);
        install(
            &mut board,
            1,
            long_state(CODE_1, STACK_1, cr3, false),
            0xD01,
        );
        install(
            &mut board,
            0,
            long_state(CODE_0, STACK_0, cr3, false),
            0xD01,
        );
        let (report, ()) = run(&mut board, config(64, u64::MAX), |control| {
            assert!(
                control.wait_finished(WAIT),
                "the error does not end the run"
            );
        });
        assert!(
            matches!(report.finished, ParallelFinish::Error { cpu: 1, .. }),
            "{report:?}"
        );
        assert_joined(&report, 2);

        let panicking = |call: QuantumCall<'_, '_>| {
            if call.cpu == 1 {
                panic!("vCPU 1 panics (expected by the test)");
            }
            fixture_quantum(call)
        };
        let (report, ()) = run_threads(&mut board, config(64, u64::MAX), &panicking, |control| {
            assert!(
                control.wait_finished(WAIT),
                "the panic does not end the run"
            );
        })
        .unwrap();
        assert_eq!(report.finished, ParallelFinish::Panicked { cpu: 1 });
        assert_joined(&report, 2);
    }

    #[test]
    fn a_guest_stop_preserves_a_worker_error_in_either_publication_order() {
        use std::time::Instant;

        for first in [0, 1] {
            let mut board = board(2);
            system(&mut board, false);
            let cr3 = board.cr3();
            for (cpu, code, stack) in [(0, CODE_0, STACK_0), (1, CODE_1, STACK_1)] {
                install(&mut board, cpu, long_state(code, stack, cr3, false), 0xD01);
            }
            let entered = [AtomicBool::new(false), AtomicBool::new(false)];
            let quantum = |call: QuantumCall<'_, '_>| {
                entered[call.cpu].store(true, SEQ);
                let deadline = Instant::now() + WAIT;
                while !entered[1 - call.cpu].load(SEQ) {
                    assert!(Instant::now() < deadline, "both workers must enter");
                    std::thread::yield_now();
                }
                if call.cpu != first {
                    while !(call.stop)() {
                        assert!(
                            Instant::now() < deadline,
                            "first worker must publish a stop"
                        );
                        std::thread::yield_now();
                    }
                }
                if call.cpu == 0 {
                    *call.machine_stop = Some(MachineStopReason::Shutdown);
                    Ok(Executed {
                        retired: 1,
                        partial_rep: false,
                        device_ns: 0,
                    })
                } else {
                    Err(FirmwareBoardError::Config("expected host error").into())
                }
            };
            let (report, ()) = run_threads(&mut board, config(64, u64::MAX), &quantum, |control| {
                assert!(control.wait_finished(WAIT));
            })
            .unwrap();
            assert!(
                matches!(
                    report.finished,
                    ParallelFinish::Error {
                        cpu: 1,
                        error: FirmwareBoardError::Config("expected host error"),
                    }
                ),
                "{report:?}"
            );
            assert_eq!(report.cpus[0].retired, 1);
            assert_joined(&report, 2);
        }
    }

    // ---- the split LOCK (step 8) -------------------------------------------------------------

    /// The operand of the split `LOCK` tests: a dword that starts two bytes before the end of the
    /// 4 KiB page at linear 0x40_0000.
    const SPLIT_LINEAR: u64 = 0x40_0FFE;
    const DONE_LINEAR: u64 = 0x40_0100;

    fn lock_inc_program(operand: u64, repeat: u32) -> Vec<u8> {
        let mut code = mov_ebx(operand as u32);
        code.extend(imm(&[0xB9], repeat)); // mov ecx, repeat
        code.extend([0xF0, 0xFF, 0x03]); // again: lock inc dword [rbx]
        code.extend([0xFF, 0xC9]); // dec ecx
        code.extend([0x75, 0xF9]); // jnz again
        code.extend(mov_ebx(DONE_LINEAR as u32));
        code.extend([0xF0, 0xFF, 0x03]); // lock inc dword [rbx]
        code.push(0xF4); // hlt
        code
    }

    /// `vcpus` processors on one table set; the operand page is a 2 MiB leaf (`pages = false`) or
    /// two 4 KiB pages in distant frames (`pages = true`: the operand crosses the page).
    fn counter_board(vcpus: usize, operand: u64, repeat: u32, pages: bool) -> LoadedFirmwareBoard {
        let mut board = board(vcpus);
        system(&mut board, false);
        let cr3 = tables(&mut board, TABLES_0, 0x60_0000, 0);
        if pages {
            // PD entry 2 -> a page table whose entries 0 and 1 are two distant frames.
            let pt = TABLES_0 + 0x5000;
            let pd = TABLES_0 + 0x2000;
            patch(&mut board, pd + 2 * 8, &(pt | 3).to_le_bytes());
            patch(&mut board, pt, &(0x10_0000u64 | 3).to_le_bytes());
            patch(&mut board, pt + 8, &(0x30_0000u64 | 3).to_le_bytes());
        }
        patch(&mut board, CODE_0, &lock_inc_program(operand, repeat));
        for cpu in 0..vcpus {
            install(
                &mut board,
                cpu,
                long_state(CODE_0, STACK_0 - 0x1000 * cpu as u64, cr3, false),
                0xD01,
            );
        }
        board
    }

    fn operand_value(board: &mut LoadedFirmwareBoard, operand: u64, pages: bool) -> u32 {
        if pages {
            // The two bytes at the end of the first frame, the two at the start of the second.
            let low = read(board, 0x10_0000 + (operand & 0xFFF), 2);
            let high = read(board, 0x30_0000, 2);
            u32::from_le_bytes([low[0], low[1], high[0], high[1]])
        } else {
            dword(board, 0x60_0000 + (operand - 0x40_0000))
        }
    }

    fn done_value(board: &mut LoadedFirmwareBoard, pages: bool) -> u32 {
        dword(board, if pages { 0x10_0100 } else { 0x60_0100 })
    }

    /// The litmus counter: every vCPU does `repeat` times `LOCK INC dword [operand]`; the final
    /// count is exact. With the operand split (the unaligned dword across a cache line and a page,
    /// in one 2 MiB leaf or in two frames) the executing vCPU stops every other one around each
    /// operation (step 8); aligned it is one host atomic.
    fn litmus_counter(vcpus: usize, operand: u64, repeat: u32, pages: bool) -> ParallelReport {
        let mut board = counter_board(vcpus, operand, repeat, pages);
        let (report, ()) = run(&mut board, config(64, u64::MAX), |control| {
            assert!(control.wait_finished(WAIT), "the counters do not finish");
        });
        assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
        assert_joined(&report, vcpus);
        assert_eq!(
            operand_value(&mut board, operand, pages),
            vcpus as u32 * repeat,
            "{vcpus} vCPUs x {repeat}: the counter lost an update ({report:?})"
        );
        assert_eq!(done_value(&mut board, pages), vcpus as u32);
        assert_eq!(board.execution_mode(), ExecutionMode::Interleaved);
        report
    }

    /// S6-36, sensitivity part 2 (SMP.md 5.1): a guest counter of `LOCK INC` on 2 and 4 vCPUs. In
    /// the normal build the count is exact. In the build with the test-only feature
    /// `nonatomic-lock-test` (the locked read-modify-write of RAM is a plain load and a plain
    /// store) the update of one vCPU is lost now and then, so the count is BELOW the exact number:
    /// the test fails if the weakened build still counts exactly (the check would then not see a
    /// non-atomic `LOCK`). The rounds are repeated until a loss is seen (at most `ATTEMPTS`).
    #[test]
    fn the_lock_inc_counter_is_exact_and_loses_updates_in_the_nonatomic_lock_build() {
        const ATTEMPTS: usize = 20;
        let repeat = 20_000u32;
        for vcpus in [2usize, 4] {
            let exact = vcpus as u32 * repeat;
            let mut lost = Vec::new();
            for _ in 0..ATTEMPTS {
                let mut board = counter_board(vcpus, 0x40_0F00, repeat, false);
                let (report, ()) = run(&mut board, config(64, u64::MAX), |control| {
                    assert!(control.wait_finished(WAIT), "the counters do not finish");
                });
                assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
                assert_joined(&report, vcpus);
                let value = operand_value(&mut board, 0x40_0F00, false);
                assert_eq!(done_value(&mut board, false), vcpus as u32);
                assert!(value <= exact, "{vcpus} vCPUs: {value} above {exact}");
                lost.push(exact - value);
                if cfg!(feature = "nonatomic-lock-test") && value < exact {
                    break;
                }
                if !cfg!(feature = "nonatomic-lock-test") {
                    assert_eq!(
                        value, exact,
                        "{vcpus} vCPUs: the LOCK INC counter lost an update"
                    );
                    if lost.len() == 3 {
                        break;
                    }
                }
            }
            eprintln!(
                "LOCK INC counter, {vcpus} vCPUs x {repeat}, exact {exact}, lost per attempt {lost:?} [{}]",
                if cfg!(feature = "nonatomic-lock-test") {
                    "nonatomic-lock-test"
                } else {
                    "normal build"
                }
            );
            if cfg!(feature = "nonatomic-lock-test") {
                assert!(
                    lost.iter().any(|lost| *lost > 0),
                    "{vcpus} vCPUs: no update was lost in {ATTEMPTS} attempts with a non-atomic LOCK"
                );
            }
        }
    }

    #[test]
    fn the_aligned_lock_counter_is_exact_on_two_and_four_vcpus() {
        for vcpus in [2usize, 4] {
            for _ in 0..5 {
                let report = litmus_counter(vcpus, 0x40_0F00, 1500, false);
                assert!(
                    report.cpus.iter().all(|cpu| cpu.split_locks == 0),
                    "{report:?}"
                );
            }
        }
    }

    #[test]
    fn the_split_lock_counter_through_a_page_is_exact_on_two_and_four_vcpus() {
        for pages in [false, true] {
            for vcpus in [2usize, 4] {
                for _ in 0..5 {
                    let report = litmus_counter(vcpus, SPLIT_LINEAR, 400, pages);
                    for cpu in &report.cpus {
                        assert_eq!(cpu.split_locks, 400, "pages {pages}: {report:?}");
                    }
                }
            }
        }
    }

    /// The split operand and an aligned operand overlap: `LOCK INC dword [0x40_0FFE]` on some vCPUs
    /// (bytes FFE..1001) and `LOCK INC dword [0x40_1000]` on the others (bytes 1000..1003, one host
    /// atomic). The two touch the bytes 1000 and 1001 in common; the operations of the first kind
    /// are atomic with respect to the second only because the vCPU that executes one stops the
    /// others around it (step 8). The final bytes are exact for both counters, in one 2 MiB leaf
    /// and across two frames, on two and four vCPUs. (The count of the split operand stays below
    /// 65536, so it never carries into the aligned one.)
    #[test]
    fn a_split_lock_is_atomic_against_an_aligned_lock_on_the_bytes_they_share() {
        for pages in [false, true] {
            for vcpus in [2usize, 4] {
                for _ in 0..5 {
                    mixed_litmus(vcpus, pages, 300);
                }
            }
        }
    }

    /// The long form of the two litmus counters (the split one and the mixed one): `OD_S6_LITMUS_RUNS`
    /// rounds (default 200) over 2 and 4 vCPUs, both table layouts.
    #[test]
    #[ignore = "long: OD_S6_LITMUS_RUNS=200 cargo test --release -p od-machine --offline --lib -- --ignored parallel_run::tests::split_lock_litmus_many_runs --nocapture"]
    fn split_lock_litmus_many_runs() {
        let runs: usize = std::env::var("OD_S6_LITMUS_RUNS")
            .ok()
            .and_then(|text| text.parse().ok())
            .unwrap_or(200);
        let started = Instant::now();
        let mut losses = 0;
        for _ in 0..runs {
            for pages in [false, true] {
                for vcpus in [2usize, 4] {
                    let report = litmus_counter(vcpus, SPLIT_LINEAR, 1000, pages);
                    losses += report.cpus.iter().map(|cpu| cpu.split_losses).sum::<u64>();
                    mixed_litmus(vcpus, pages, 1000);
                }
            }
        }
        eprintln!(
            "split lock litmus: {runs} rounds x 2 and 4 vCPUs x 2 layouts x 2 counters, \
             {losses} lost requests for the gate, {:.1} s",
            started.elapsed().as_secs_f64()
        );
    }

    fn mixed_litmus(vcpus: usize, pages: bool, repeat: u32) {
        let mut board = counter_board(vcpus, SPLIT_LINEAR, repeat, pages);
        // The odd vCPUs run the aligned counter (the code page 1 of the fixture).
        patch(&mut board, CODE_1, &lock_inc_program(0x40_1000, repeat));
        let cr3 = board.vcpu(0).unwrap().state.cr3.unwrap();
        for cpu in (1..vcpus).step_by(2) {
            install(
                &mut board,
                cpu,
                long_state(CODE_1, STACK_0 - 0x1000 * cpu as u64, cr3, false),
                0xD01,
            );
        }
        let (report, ()) = run(&mut board, config(64, u64::MAX), |control| {
            assert!(control.wait_finished(WAIT));
        });
        assert_eq!(report.finished, ParallelFinish::AllIdle, "{report:?}");
        assert_joined(&report, vcpus);
        let (splitters, aligned) = (vcpus.div_ceil(2) as u32, (vcpus / 2) as u32);
        let (low, high) = if pages {
            (
                read(&mut board, 0x10_0000 + 0xFFE, 2),
                read(&mut board, 0x30_0000, 4),
            )
        } else {
            let all = read(&mut board, 0x60_0FFE, 6);
            (all[..2].to_vec(), all[2..].to_vec())
        };
        assert_eq!(
            u32::from(u16::from_le_bytes([low[0], low[1]])),
            splitters * repeat,
            "pages {pages}, {vcpus} vCPUs: the split counter ({report:?})"
        );
        assert_eq!(
            u32::from_le_bytes([high[0], high[1], high[2], high[3]]),
            aligned * repeat,
            "pages {pages}, {vcpus} vCPUs: the aligned counter lost an update ({report:?})"
        );
        assert_eq!(done_value(&mut board, pages), vcpus as u32);
    }

    /// With four vCPUs hammering one split operand the exchange for the gate is lost by somebody:
    /// the losers stood still before any effect and asked again (no operation twice, none lost:
    /// the exact count above proves it, the counter below proves the race really happened).
    #[test]
    fn simultaneous_split_lock_requests_have_a_loser_and_lose_nothing() {
        let mut losses = 0;
        for _ in 0..10 {
            let report = litmus_counter(4, SPLIT_LINEAR, 300, true);
            losses += report.cpus.iter().map(|cpu| cpu.split_losses).sum::<u64>();
        }
        assert!(
            losses > 0,
            "no request ever lost the gate: the race was not exercised"
        );
    }

    /// A split `LOCK` that cannot get the others quiet in time fails before any effect (a typed
    /// error, the run ends in an error); it does not run as two host accesses.
    #[test]
    fn a_split_lock_that_cannot_stop_the_others_fails_before_any_effect() {
        let mut board = counter_board(2, SPLIT_LINEAR, 5, false);
        let one_in_its_call = AtomicBool::new(false);
        let quantum = |call: QuantumCall<'_, '_>| {
            if call.cpu == 1 {
                // vCPU 1 never comes back from its call within the time allowed.
                one_in_its_call.store(true, SEQ);
                thread::sleep(Duration::from_millis(800));
            } else {
                // vCPU 0 asks for the gate only once vCPU 1 is inside its call: a vCPU 1 that has
                // not started yet stands at the barrier, which is quiet (a race of the thread
                // start, not of the gate).
                let until = Instant::now() + Duration::from_secs(5);
                while !one_in_its_call.load(SEQ) && Instant::now() < until {
                    thread::yield_now();
                }
            }
            fixture_quantum(call)
        };
        let config = ParallelConfig {
            split_timeout: Duration::from_millis(100),
            ..config(64, u64::MAX)
        };
        let (report, ()) = run_threads(&mut board, config, &quantum, |control| {
            assert!(control.wait_finished(WAIT));
        })
        .unwrap();
        assert!(
            matches!(report.finished, ParallelFinish::Error { cpu: 0, .. }),
            "{report:?}"
        );
        assert_joined(&report, 2);
        assert_eq!(
            operand_value(&mut board, SPLIT_LINEAR, false),
            0,
            "no effect before the gate"
        );
    }

    #[test]
    fn without_a_gate_the_parallel_split_lock_is_still_refused_and_counted() {
        // The interleaved scheduler, the memory in the parallel mode, no gate: S6-31/S6-32.
        let mut board = counter_board(1, SPLIT_LINEAR, 1, false);
        board.set_execution_mode(ExecutionMode::Parallel);
        let result = quantum(&mut board, 0, 100);
        assert!(result.is_err(), "{result:?}");
        assert_eq!(board.bus_mut().physical_memory().split_lock_refusals(), 1);
        let _ = LOCAL_APIC_MMIO_BASE;
    }
    /// The JIT host quantum can query the board source while leaving the source and clock locks
    /// available to another thread between operations. The ordinary run is the control.
    #[test]
    fn a_source_free_run_holds_no_board_lock_across_the_quantum_call() {
        use std::sync::atomic::AtomicBool;
        for source_free in [false, true] {
            let mut board = board(1);
            system(&mut board, false);
            assert!(board.enable_pit_pic_irq());
            install(
                &mut board,
                0,
                long_state(CODE_0, STACK_0, 0x1000, false),
                0xD01,
            );
            let inside = AtomicBool::new(false);
            let release = AtomicBool::new(false);
            let quantum = |call: QuantumCall<'_, '_>| {
                assert!(call.source.pit_write_ends_quantum(), "real PIT source");
                inside.store(true, SEQ);
                while !release.load(SEQ) && !(call.stop)() {
                    std::hint::spin_loop();
                }
                Ok(Executed {
                    retired: 1,
                    partial_rep: false,
                    device_ns: 0,
                })
            };
            let (report, (irq_free, owner_free)) = run_threads_with(
                &mut board,
                open_config(64),
                source_free,
                &quantum,
                None,
                |control| {
                    assert!(
                        control.wait_until(WAIT, || inside.load(SEQ)),
                        "the quantum is not entered"
                    );
                    let irq_free = control.shared.irq.try_lock().is_ok();
                    let owner_free = control.shared.owner.try_lock().is_ok();
                    release.store(true, SEQ);
                    control.finish();
                    (irq_free, owner_free)
                },
            )
            .unwrap();
            assert_joined(&report, 1);
            assert!(
                owner_free,
                "the clock owner's lock is free (source_free {source_free})"
            );
            assert_eq!(
                irq_free, source_free,
                "the source's lock is free across the call exactly when the run is source free"
            );
        }
    }
}
