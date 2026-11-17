//! Scheduler of the interleaved mode: one host thread runs N vCPUs in turns (S6-15).
//!
//! The scheduler is a loop over three decisions, all taken at a boundary between steps (an
//! instruction, or the restart point of a partly executed `REP`, SMP_CONTRACT.md 8):
//!
//! 1. STOP is read; the shared instruction budget is read.
//! 2. The boundary of every vCPU, in index order: first the INIT and SIPI waiting in its own Local
//!    APIC ([`SmpHost::boundary`]: `apply_pending_startup`), then its NMI (`deliver_pending_nmi`).
//!    A halted vCPU whose Local APIC holds an acceptable vector (IF = 1) is woken.
//! 3. The next vCPU in `Running` state runs one turn: a quantum of steps through the existing
//!    interpreter loop ([`SmpHost::run`]), cut so that it cannot pass the nearest timer deadline of
//!    any Local APIC ([`ClockOwner::step_budget`]). When no vCPU is `Running` the clock jumps to the
//!    nearest deadline of all vCPUs in one move ([`ClockOwner::idle_step`], the idle arbiter): there
//!    is no waiting and no host time.
//!
//! `WaitForSipi` and `Shutdown` vCPUs are never run; a `Halted` one is not run until it is woken.
//!
//! The quantum is fixed (default), or drawn per turn from a seeded SplitMix64 between recorded
//! limits (1 step included), or the caller gives the sequence of steps ("whose step"): one entry is
//! one step of the named vCPU, nothing else is decided. Quantum and step are counted in steps: a
//! retired instruction is one step, and so is one return of the interpreter with partial progress
//! of a `REP`. The shared budget is in retired instructions; the counters are per vCPU.
//!
//! Time (D7): the [`ClockOwner`] is the only mover of virtual time. The scheduler charges what a
//! turn retired (1 ns each) and the device nanoseconds the host reports, and asks it to skip idle
//! time; the host never touches the clock.
//!
//! Not here: threads, priorities, fairness heuristics (no measurement asks for them), switching
//! inside an instruction. A LAPIC event raised during a turn (an ICR write for another vCPU or for
//! the running one) is applied at the next boundary, after the turn; a host may end its call
//! earlier when it sees one. The loop is not wired into `run_linux_boot_cpl0_set`: that loop runs
//! the boot processor only (see [`BoardSmpHost`]; since S6-16c/S6-16d the other vCPUs execute under
//! this scheduler, in the tests of those slices).

// No production caller yet: the scheduler is wired in by S6-16/S6-17; tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::clock_owner::{ClockOwner, ClockOwnerError, IdleOutcome};
use crate::idle_arbiter::{SchedActivity, WakeView};
use crate::vcpu::{BOOT_VCPU_INDEX, NmiBoundary};
use crate::{FirmwareBoardError, LoadedFirmwareBoard, LocalApicHandle};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Quantum of the default policy, in steps. Not measured: no measurement asks for another value.
pub(crate) const DEFAULT_QUANTUM: u64 = 1024;
/// Limits of the seeded quantum, in steps, both ends included. A turn of 1 step is possible.
pub(crate) const SEED_QUANTUM_MIN: u64 = 1;
pub(crate) const SEED_QUANTUM_MAX: u64 = 256;

/// SplitMix64 (Steele, Lea, Flood 2014): `state += 0x9E3779B97F4A7C15`, then the two xor-shift and
/// multiply rounds with `0xBF58476D1CE4E5B9` and `0x94D049BB133111EB`, then `z ^ (z >> 31)`. Any
/// seed (0 included) is valid. Own code so that the sequence is fixed by this file and not by a
/// dependency.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SplitMix64(u64);

impl SplitMix64 {
    pub(crate) const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `min..=max` (`1 <= min <= max`): `min + next_u64() % (max - min + 1)`. The modulo
    /// bias is below 2^-56 for the limits used here and is accepted.
    pub(crate) fn in_range(&mut self, min: u64, max: u64) -> u64 {
        debug_assert!(1 <= min && min <= max);
        min + self.next_u64() % (max - min + 1)
    }
}

/// How the length of a turn is decided.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum QuantumPolicy {
    /// Every turn is this many steps (at least 1).
    Fixed(u64),
    /// Every turn is drawn from [`SplitMix64`]`(seed)` in `min..=max` (`1 <= min <= max`).
    Seeded { seed: u64, min: u64, max: u64 },
    /// The sequence of vCPU indexes, one entry per step. A turn is one step of the named vCPU; the
    /// vCPU must be `Running` at that point. After the last entry the run ends ([`Finished::ScheduleEnd`]).
    Schedule(Vec<usize>),
}

impl QuantumPolicy {
    /// The seeded policy with the recorded limits.
    pub(crate) const fn seeded(seed: u64) -> Self {
        Self::Seeded {
            seed,
            min: SEED_QUANTUM_MIN,
            max: SEED_QUANTUM_MAX,
        }
    }
}

impl Default for QuantumPolicy {
    fn default() -> Self {
        Self::Fixed(DEFAULT_QUANTUM)
    }
}

/// A STOP request, shared between the scheduler and whoever asks (the host of a run, a device, a
/// test). Read at every boundary and by the interpreter loop of a turn through the stop predicate.
#[derive(Clone, Debug, Default)]
pub(crate) struct StopRequest(Arc<AtomicBool>);

impl StopRequest {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn request(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub(crate) fn clear(&self) {
        self.0.store(false, Ordering::Release);
    }

    pub(crate) fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// What one call of [`SmpHost::run`] did.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Executed {
    /// Instructions retired by the call (the unit of the budget; one ns each).
    pub(crate) retired: u64,
    /// The call ended on the restart point of a `REP` that made progress without finishing: one
    /// more step, no retirement.
    pub(crate) partial_rep: bool,
    /// Nanoseconds of device time (PM timer, PIT, RDTSC ticks) the call has to charge to the clock,
    /// after the retired instructions.
    pub(crate) device_ns: u64,
}

/// What one boundary of one vCPU applied.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BoundaryReport {
    pub(crate) init: bool,
    pub(crate) started: bool,
    pub(crate) nmi_delivered: bool,
}

/// What the scheduler needs from the machine it drives.
pub(crate) trait SmpHost {
    type Error;

    fn vcpu_count(&self) -> usize;
    fn activity(&self, cpu: usize) -> SchedActivity;
    /// `RFLAGS.IF` of the vCPU.
    fn if_set(&self, cpu: usize) -> bool;
    fn apic(&self, cpu: usize) -> LocalApicHandle;
    /// The single owner of the clock of the machine.
    fn clock_owner(&mut self) -> &mut ClockOwner;
    /// The nearest virtual time (ns) at which the machine, apart from the Local APIC timers the
    /// owner knows, has an edge a vCPU must not run past: the deadline of the board's interrupt
    /// source (the PIT line of the boot processor). `None` (the default) when nothing limits a
    /// turn. The scheduler passes it to [`ClockOwner::step_budget`] for every call of a turn and
    /// to [`ClockOwner::idle_step`] for an idle jump (S6-16d, condition (a)). A deadline the clock
    /// has reached is the host's to consume on its next call: an idle jump that reaches it ends in
    /// `NoProgress` with a skip above 0, which is not an error; the same answer without a skip
    /// (the host did not consume it) is [`SmpError::IdleNoProgress`].
    fn board_deadline(&self) -> Option<u64> {
        None
    }
    /// The deadline the idle arbiter jumps to when nobody runs: the nearest virtual time at which
    /// the board can wake a halted vCPU (the next edge of its source that would be deliverable), as
    /// opposed to [`Self::board_deadline`], every edge a running vCPU must not run past. `None`
    /// when the board cannot wake anybody (the line is masked or in service, no edge is scheduled):
    /// the jump then goes to a Local APIC timer or the run ends as [`Finished::AllIdle`]. The
    /// default is [`Self::board_deadline`] (S6-21 step 1).
    fn idle_deadline(&self) -> Option<u64> {
        self.board_deadline()
    }
    /// Whether the interrupt source of the board has a vector for the halted `cpu` now, so that the
    /// scheduler wakes it (IF = 1 is the scheduler's own test) although no vector waits in its
    /// Local APIC. Only the boot processor is offered the source of the board; `false` for every
    /// other vCPU and by default (S6-21 step 1).
    fn board_wakes(&self, _cpu: usize) -> bool {
        false
    }
    /// The boundary of `cpu`: the INIT and SIPI waiting in its Local APIC, then its NMI, in that
    /// order. Nothing waiting is not an error.
    fn boundary(&mut self, cpu: usize) -> Result<BoundaryReport, Self::Error>;
    /// Set a halted `cpu` (IF = 1, an acceptable vector in its Local APIC) to `Running`.
    fn wake(&mut self, cpu: usize) -> Result<(), Self::Error>;
    /// The acknowledgement of a stop request by `cpu` (S6-24, SMP_CONTRACT.md 9): `Some` when the
    /// vCPU stands on a safe point, that is when everything that can change its guest state is in
    /// the host's vCPU structure (no live host-register value, no instruction in flight), or when
    /// it is asleep; `None` when it has not arrived yet. The default is what the interleaved mode
    /// gives by construction: only one vCPU runs and its call has ended on a boundary.
    fn stop_ack(&self, cpu: usize) -> Option<StopAck> {
        Some(match self.activity(cpu) {
            SchedActivity::Running => StopAck::Boundary,
            asleep => StopAck::Asleep(asleep),
        })
    }
    /// One round of progress of `cpu` toward its safe point, called by [`SmpScheduler::stop_all`]
    /// while [`Self::stop_ack`] answers `None`. It runs no guest instruction and does not move the
    /// clock. The default does nothing: a host whose vCPUs always acknowledge never needs it.
    fn settle(&mut self, _cpu: usize) -> Result<(), Self::Error> {
        Ok(())
    }
    /// Run `cpu` (in `Running` state) for at most `budget` (at least 1) steps with the existing
    /// interpreter loop, ending on a boundary: the budget is used up, or a partial `REP` progress
    /// (counted as a step), or the vCPU halts or shuts down, or `stop()` answers true. It must
    /// poll `stop` between steps. The host does not move the clock.
    ///
    /// `device_ns` is charged after the whole call; device time depends on the number of calls, so
    /// the host must call `run` so that device ticks are not skipped (see the test
    /// `device_time_is_charged_after_the_retired_instructions`).
    fn run(
        &mut self,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Executed, Self::Error>;
}

/// Why a run ended without an error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Finished {
    /// STOP was requested; no vCPU was run after the boundary that saw it.
    Stopped,
    /// The shared instruction budget is used up.
    BudgetExhausted,
    /// Nothing runs and nothing can wake: every vCPU waits for SIPI or has shut down, or is halted
    /// with no acceptable vector and no deadline that could give one.
    AllIdle,
    /// The explicit schedule has no more entries.
    ScheduleEnd,
}

/// A policy or a vCPU count the scheduler cannot be built with.
#[derive(Debug, Eq, PartialEq)]
pub(crate) struct PolicyError(pub(crate) &'static str);

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum SmpError<E> {
    /// The scheduler was built for another number of vCPUs than the host has.
    VcpuCountMismatch {
        scheduler: usize,
        host: usize,
    },
    Host(E),
    Clock(ClockOwnerError),
    /// The explicit schedule names a vCPU the host does not have.
    ScheduleCpuOutOfRange {
        position: usize,
        cpu: usize,
    },
    /// The explicit schedule names a vCPU that is not `Running` at that point.
    ScheduleCpuNotRunning {
        position: usize,
        cpu: usize,
        activity: SchedActivity,
    },
    /// A running vCPU came back from the host without a step: the loop would never end.
    NoProgress {
        cpu: usize,
    },
    /// The idle arbiter found a due deadline that no dispatch consumed.
    IdleNoProgress,
}

/// Per vCPU counters (the budget is shared, these are not).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct VcpuCounters {
    pub(crate) turns: u64,
    /// Steps: retired instructions and partial `REP` returns.
    pub(crate) steps: u64,
    pub(crate) retired: u64,
    pub(crate) wakes: u64,
    pub(crate) init_applied: u64,
    pub(crate) started: u64,
    pub(crate) nmi_delivered: u64,
}

/// One turn as it ran (kept only with [`SmpScheduler::record_turns`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TurnRecord {
    pub(crate) cpu: usize,
    /// The quantum the policy gave (before the cut by the nearest deadline and by the budget).
    pub(crate) quantum: u64,
    pub(crate) steps: u64,
    pub(crate) retired: u64,
}

/// A turn that STOP or the end of the shared budget cut before its quantum was used up (S6-24).
/// The next [`SmpScheduler::run`] continues it without a boundary: the continuous run has none
/// inside a turn either, so a run cut and resumed is the run that was never cut.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TurnCarry {
    cpu: usize,
    /// The quantum the policy gave (for the [`TurnRecord`]).
    quantum: u64,
    /// Steps of the quantum not used yet (not cut by the budget).
    left: u64,
    /// What the turn did before it was cut.
    steps: u64,
    retired: u64,
}

/// Everything the scheduler remembers between two calls of [`SmpScheduler::run`] (S6-24): the
/// position of the quantum generator, the schedule position, the turn that was cut and the
/// round-robin start (the current vCPU), the counters and the idle totals. The STOP handle is
/// the caller's and is not part of it. A value, not a file: the format is `S9-24`/`S9-25`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SchedulerState {
    policy: QuantumPolicy,
    rng: SplitMix64,
    position: usize,
    next_cpu: usize,
    carry: Option<TurnCarry>,
    resume_turn: bool,
    counters: Vec<VcpuCounters>,
    turns: Option<Vec<TurnRecord>>,
    idle_ns: u64,
    idle_jumps: u64,
}

/// How one vCPU acknowledged a stop request (SMP_CONTRACT.md 9).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StopAck {
    /// A completed instruction: the vCPU stands on a boundary.
    Boundary,
    /// The restart point of a partly executed `REP` (progress after some iteration).
    RepRestart,
    /// `Halted`, `WaitForSipi` or `Shutdown`: no guest instruction is needed to stop.
    Asleep(SchedActivity),
}

/// The proof that every vCPU acknowledged the stop (S6-24): one acknowledgement per vCPU, in
/// index order, and the virtual time at which they stood. A flag without it is not a stop (G6).
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SafePoint {
    pub(crate) acks: Vec<StopAck>,
    pub(crate) now_ns: u64,
    /// Rounds the stop needed (0 when every vCPU stood on a safe point at once).
    pub(crate) rounds: u64,
}

/// Why [`SmpScheduler::stop_all`] gave no [`SafePoint`].
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum StopError<E> {
    VcpuCountMismatch {
        scheduler: usize,
        host: usize,
    },
    /// The round limit passed with these vCPUs (index order) still not on a safe point. The STOP
    /// request was withdrawn and nothing else changed: every vCPU has the activity it had, every
    /// pending event and deadline is where it was, and no snapshot exists.
    Timeout {
        unacked: Vec<usize>,
        rounds: u64,
    },
    Host(E),
}

#[derive(Debug)]
pub(crate) struct SmpScheduler {
    policy: QuantumPolicy,
    rng: SplitMix64,
    /// Next entry of an explicit schedule.
    position: usize,
    /// Where the round-robin search for the next `Running` vCPU starts.
    next_cpu: usize,
    /// The turn the last run left unfinished, if STOP or the budget cut it.
    carry: Option<TurnCarry>,
    /// [`Self::resume`] was called: the next run continues `carry`.
    resume_turn: bool,
    stop: StopRequest,
    counters: Vec<VcpuCounters>,
    turns: Option<Vec<TurnRecord>>,
    idle_ns: u64,
    idle_jumps: u64,
}

/// The first value of the generator for a policy.
fn initial_seed(policy: &QuantumPolicy) -> u64 {
    match policy {
        QuantumPolicy::Seeded { seed, .. } => *seed,
        _ => 0,
    }
}

impl SmpScheduler {
    pub(crate) fn new(
        vcpu_count: usize,
        policy: QuantumPolicy,
        stop: StopRequest,
    ) -> Result<Self, PolicyError> {
        if vcpu_count == 0 {
            return Err(PolicyError("no vCPUs"));
        }
        match &policy {
            QuantumPolicy::Fixed(0) => return Err(PolicyError("quantum 0")),
            QuantumPolicy::Seeded { min, max, .. } if *min == 0 || min > max => {
                return Err(PolicyError("seeded quantum needs 1 <= min <= max"));
            }
            _ => {}
        }
        Ok(Self {
            rng: SplitMix64::new(initial_seed(&policy)),
            policy,
            position: 0,
            next_cpu: 0,
            carry: None,
            resume_turn: false,
            stop,
            counters: vec![VcpuCounters::default(); vcpu_count],
            turns: None,
            idle_ns: 0,
            idle_jumps: 0,
        })
    }

    /// Keep a [`TurnRecord`] of every turn from now on (off by default: a long run is long).
    pub(crate) fn record_turns(&mut self) {
        self.turns.get_or_insert_with(Vec::new);
    }

    pub(crate) fn counters(&self) -> &[VcpuCounters] {
        &self.counters
    }

    pub(crate) fn turns(&self) -> &[TurnRecord] {
        self.turns.as_deref().unwrap_or(&[])
    }

    /// The recorded turns as the sequence of "whose step": a turn of `n` steps is `n` entries.
    /// This is the input form of [`QuantumPolicy::Schedule`]. The schedule reproduces the run if
    /// the host does not depend on how its calls are cut and no boundary event was raised for the
    /// running vCPU during a turn. An event raised during a turn (an ICR write) is applied at the
    /// next boundary, so the boundary of the recorded and of the replayed run may differ. This is a
    /// limit for S6-19.
    pub(crate) fn executed_schedule(&self) -> Vec<usize> {
        self.turns()
            .iter()
            .flat_map(|turn| std::iter::repeat_n(turn.cpu, turn.steps as usize))
            .collect()
    }

    pub(crate) fn retired_total(&self) -> u64 {
        self.counters.iter().map(|c| c.retired).sum()
    }

    /// Virtual nanoseconds skipped while nothing ran, and the number of clock jumps.
    pub(crate) fn idle(&self) -> (u64, u64) {
        (self.idle_ns, self.idle_jumps)
    }

    /// Run until STOP, the end of the budget (retired instructions of all vCPUs together), nothing
    /// left to run, or the end of an explicit schedule. Counters keep adding up over calls.
    pub(crate) fn run<H: SmpHost>(
        &mut self,
        host: &mut H,
        budget: u64,
    ) -> Result<Finished, SmpError<H::Error>> {
        let count = host.vcpu_count();
        if count != self.counters.len() {
            return Err(SmpError::VcpuCountMismatch {
                scheduler: self.counters.len(),
                host: count,
            });
        }
        let mut remaining = budget;
        loop {
            if self.stop.requested() {
                return Ok(Finished::Stopped);
            }
            if remaining == 0 {
                return Ok(Finished::BudgetExhausted);
            }
            if let QuantumPolicy::Schedule(list) = &self.policy
                && self.position >= list.len()
            {
                return Ok(Finished::ScheduleEnd);
            }
            // A turn cut by STOP or by the end of the budget goes on from where it stood, with no
            // boundary (the continuous run has none inside a turn), but only after the explicit
            // `resume` (S6-24). A plain new `run` starts a new turn on a boundary, as it always
            // did: what the caller changed between the runs is taken at once.
            if let Some(carry) = self.carry.take()
                && std::mem::take(&mut self.resume_turn)
            {
                self.turn(host, carry.cpu, carry.quantum, Some(carry), &mut remaining)?;
                continue;
            }
            self.resume_turn = false;
            self.boundaries(host)?;
            self.wake_ready(host)?;
            if !(0..count).any(|cpu| host.activity(cpu) == SchedActivity::Running) {
                if let Some(finished) = self.idle_step(host)? {
                    return Ok(finished);
                }
                continue;
            }
            let (cpu, quantum) = self.select(host)?;
            self.turn(host, cpu, quantum, None, &mut remaining)?;
        }
    }

    /// The scheduler's own state as a value (S6-24). Call it on a stopped scheduler. With
    /// [`Self::record_turns`] on the state holds a copy of the whole turn record, so taking it,
    /// putting it back and comparing it cost as much as the record is long (one entry a turn, for
    /// the whole run so far): a long recorded run makes every snapshot as big as the record.
    pub(crate) fn state(&self) -> SchedulerState {
        SchedulerState {
            policy: self.policy.clone(),
            rng: self.rng.clone(),
            position: self.position,
            next_cpu: self.next_cpu,
            carry: self.carry,
            resume_turn: self.resume_turn,
            counters: self.counters.clone(),
            turns: self.turns.clone(),
            idle_ns: self.idle_ns,
            idle_jumps: self.idle_jumps,
        }
    }

    /// Put the scheduler back into a state taken by [`Self::state`] (S6-24). The STOP handle is
    /// kept. A state of another number of vCPUs is refused and nothing changes.
    pub(crate) fn set_state(&mut self, state: SchedulerState) -> Result<(), PolicyError> {
        if state.counters.len() != self.counters.len() {
            return Err(PolicyError("state of another number of vCPUs"));
        }
        let SchedulerState {
            policy,
            rng,
            position,
            next_cpu,
            carry,
            resume_turn,
            counters,
            turns,
            idle_ns,
            idle_jumps,
        } = state;
        self.policy = policy;
        self.rng = rng;
        self.position = position;
        self.next_cpu = next_cpu;
        self.carry = carry;
        self.resume_turn = resume_turn;
        self.counters = counters;
        self.turns = turns;
        self.idle_ns = idle_ns;
        self.idle_jumps = idle_jumps;
        Ok(())
    }

    /// The state of power-on: the generator at its seed, no schedule position, no cut turn, the
    /// first vCPU current, counters and idle totals at 0 (the turn record, if kept, empty). The
    /// policy and the STOP handle are kept (S6-24, the board reset of the scheduler).
    pub(crate) fn reset(&mut self) {
        self.rng = SplitMix64::new(initial_seed(&self.policy));
        self.position = 0;
        self.next_cpu = 0;
        self.carry = None;
        self.resume_turn = false;
        self.counters.fill(VcpuCounters::default());
        if let Some(turns) = self.turns.as_mut() {
            turns.clear();
        }
        self.idle_ns = 0;
        self.idle_jumps = 0;
    }

    /// The number of vCPUs the scheduler was built for.
    pub(crate) fn vcpu_count(&self) -> usize {
        self.counters.len()
    }

    /// Whether a stop is in force: [`Self::stop_all`] succeeded (or STOP was requested) and
    /// [`Self::resume`] has not been called.
    pub(crate) fn paused(&self) -> bool {
        self.stop.requested()
    }

    /// The explicit command that lets the guest run again after [`Self::stop_all`] (SMP_CONTRACT.md 9):
    /// the stop request is withdrawn and the turn that the stop (or the end of the budget) cut is
    /// continued by the next `run`, with no boundary before it. Nothing else changes.
    pub(crate) fn resume(&mut self) {
        self.stop.clear();
        self.resume_turn = true;
    }

    /// Stop every vCPU (S6-24, SMP_CONTRACT.md 9): request STOP and collect the acknowledgement of
    /// each vCPU ([`SmpHost::stop_ack`]: a completed instruction, the restart point of a `REP`, or
    /// asleep), in rounds. A vCPU that does not acknowledge gets [`SmpHost::settle`] before the next
    /// round; after `round_limit` rounds (the common bounded deadline) the stop fails with the list
    /// of vCPUs that did not arrive ([`StopError::Timeout`]): the request is withdrawn and nothing
    /// is published. On success the request stays in force (every `run` ends as `Stopped`) until
    /// [`Self::resume`]. The clock is not touched: virtual time stands over the whole stop. A vector
    /// or IPI that arrives later stays pending in its Local APIC.
    pub(crate) fn stop_all<H: SmpHost>(
        &mut self,
        host: &mut H,
        round_limit: u64,
    ) -> Result<SafePoint, StopError<H::Error>> {
        let count = host.vcpu_count();
        if count != self.counters.len() {
            return Err(StopError::VcpuCountMismatch {
                scheduler: self.counters.len(),
                host: count,
            });
        }
        self.stop.request();
        let mut rounds = 0;
        loop {
            let acks: Vec<Option<StopAck>> = (0..count).map(|cpu| host.stop_ack(cpu)).collect();
            if acks.iter().all(Option::is_some) {
                return Ok(SafePoint {
                    acks: acks.into_iter().flatten().collect(),
                    now_ns: host.clock_owner().now_ns(),
                    rounds,
                });
            }
            if rounds >= round_limit {
                self.stop.clear();
                return Err(StopError::Timeout {
                    unacked: (0..count).filter(|&cpu| acks[cpu].is_none()).collect(),
                    rounds,
                });
            }
            for cpu in (0..count).filter(|&cpu| acks[cpu].is_none()) {
                if let Err(error) = host.settle(cpu) {
                    self.stop.clear();
                    return Err(StopError::Host(error));
                }
            }
            rounds += 1;
        }
    }

    /// The boundary of every vCPU in index order: INIT and SIPI, then NMI (done by the host).
    fn boundaries<H: SmpHost>(&mut self, host: &mut H) -> Result<(), SmpError<H::Error>> {
        for cpu in 0..self.counters.len() {
            let report = host.boundary(cpu).map_err(SmpError::Host)?;
            let counters = &mut self.counters[cpu];
            counters.init_applied += u64::from(report.init);
            counters.started += u64::from(report.started);
            counters.nmi_delivered += u64::from(report.nmi_delivered);
        }
        Ok(())
    }

    /// Wake every halted vCPU with IF = 1 and an acceptable vector (the same test as the arbiter's
    /// `WakeNow`; for the boot processor also a vector of the board's source,
    /// [`SmpHost::board_wakes`]). One that is not acceptable yet stays halted and is skipped.
    fn wake_ready<H: SmpHost>(&mut self, host: &mut H) -> Result<(), SmpError<H::Error>> {
        for cpu in 0..self.counters.len() {
            if host.activity(cpu) == SchedActivity::Halted
                && host.if_set(cpu)
                && (host.apic(cpu).eligible().is_some() || host.board_wakes(cpu))
            {
                host.wake(cpu).map_err(SmpError::Host)?;
                self.counters[cpu].wakes += 1;
            }
        }
        Ok(())
    }

    /// Nobody runs: one idle decision with at most one clock jump. `None` means "go round again".
    fn idle_step<H: SmpHost>(
        &mut self,
        host: &mut H,
    ) -> Result<Option<Finished>, SmpError<H::Error>> {
        let views: Vec<WakeView> = (0..self.counters.len())
            .map(|cpu| WakeView {
                activity: host.activity(cpu),
                if_set: host.if_set(cpu),
                apic: host.apic(cpu),
            })
            .collect();
        let deadlines: Vec<u64> = host.idle_deadline().into_iter().collect();
        let step = host
            .clock_owner()
            .idle_step(self.stop.requested(), &views, &deadlines)
            .map_err(SmpError::Clock)?;
        self.idle_ns += step.idle_ns;
        self.idle_jumps += u64::from(step.idle_ns > 0);
        match step.outcome {
            IdleOutcome::Stopped => Ok(Some(Finished::Stopped)),
            IdleOutcome::AllIdle => Ok(Some(Finished::AllIdle)),
            // The clock moved (or a vCPU can be woken now): the next round wakes and picks.
            IdleOutcome::Advanced(_) | IdleOutcome::Woken(_) | IdleOutcome::Runnable(_) => Ok(None),
            // The jump reached the deadline of the board and nobody has a vector yet: the host
            // consumes the deadline on its next call (condition (a)). Without a skip nothing moved.
            IdleOutcome::NoProgress if step.idle_ns > 0 && !deadlines.is_empty() => Ok(None),
            IdleOutcome::NoProgress => Err(SmpError::IdleNoProgress),
        }
    }

    /// The vCPU of the next turn and the quantum the policy gives it. At least one vCPU runs.
    fn select<H: SmpHost>(&mut self, host: &H) -> Result<(usize, u64), SmpError<H::Error>> {
        let count = self.counters.len();
        if let QuantumPolicy::Schedule(list) = &self.policy {
            let position = self.position;
            let cpu = list[position];
            if cpu >= count {
                return Err(SmpError::ScheduleCpuOutOfRange { position, cpu });
            }
            let activity = host.activity(cpu);
            if activity != SchedActivity::Running {
                return Err(SmpError::ScheduleCpuNotRunning {
                    position,
                    cpu,
                    activity,
                });
            }
            self.position += 1;
            return Ok((cpu, 1));
        }
        let cpu = (0..count)
            .map(|offset| (self.next_cpu + offset) % count)
            .find(|&cpu| host.activity(cpu) == SchedActivity::Running)
            .expect("the caller saw a running vCPU");
        self.next_cpu = (cpu + 1) % count;
        let quantum = match &self.policy {
            QuantumPolicy::Fixed(quantum) => *quantum,
            QuantumPolicy::Seeded { min, max, .. } => self.rng.in_range(*min, *max),
            QuantumPolicy::Schedule(_) => unreachable!("handled above"),
        };
        Ok((cpu, quantum))
    }

    /// One turn of `cpu`: calls of the host until the quantum (cut to the shared budget) is used
    /// up, the vCPU stops running, or STOP is seen. Each call is cut by the nearest deadline and
    /// followed by the charge of what it retired and by the timer dispatch of the owner. A turn
    /// that STOP or the end of the budget interrupts with quantum left is kept in `carry`, to be
    /// continued by the next run (`resume` is that carry; the turn then is not counted again).
    fn turn<H: SmpHost>(
        &mut self,
        host: &mut H,
        cpu: usize,
        quantum: u64,
        resume: Option<TurnCarry>,
        remaining: &mut u64,
    ) -> Result<(), SmpError<H::Error>> {
        let stop = self.stop.clone();
        let (mut quantum_left, mut steps, mut retired) = resume.map_or((quantum, 0, 0), |carry| {
            (carry.left, carry.steps, carry.retired)
        });
        if resume.is_none() {
            self.counters[cpu].turns += 1;
        } else if let Some(turns) = self.turns.as_mut() {
            // The record of the cut turn was kept with its steps so far; the finished turn
            // replaces it.
            turns.pop();
        }
        if resume.is_some() && host.activity(cpu) != SchedActivity::Running {
            // The vCPU is not running any more (the state was replaced between the runs): the
            // turn ends here.
            quantum_left = 0;
        }
        while quantum_left > 0 && *remaining > 0 {
            // The cut is recomputed at every call (S6-24 review M1): a partial `REP` spends a step
            // of the quantum but retires nothing, so the quantum left can fall behind the shared
            // budget only by steps; the part of the budget that is left must go on with this vCPU,
            // as the continuous run would, not be given to another vCPU by a new turn.
            let left = quantum_left.min(*remaining);
            let deadline = host.board_deadline();
            let budget = host.clock_owner().step_budget(left, deadline);
            let now_before = host.clock_owner().now_ns();
            let executed = host
                .run(cpu, budget, &mut || stop.requested())
                .map_err(SmpError::Host)?;
            let owner = host.clock_owner();
            // Condition (c): only the scheduler moves the clock; a host that touched it would make
            // the charge below a double count.
            debug_assert_eq!(
                owner.now_ns(),
                now_before,
                "SmpHost::run must not move the clock"
            );
            if executed.retired != 0 {
                owner
                    .account_retired(executed.retired)
                    .map_err(SmpError::Clock)?;
            }
            if executed.device_ns != 0 {
                owner
                    .account_device_ns(executed.device_ns)
                    .map_err(SmpError::Clock)?;
            }
            let done = executed.retired + u64::from(executed.partial_rep);
            let counters = &mut self.counters[cpu];
            counters.steps += done;
            counters.retired += executed.retired;
            steps += done;
            retired += executed.retired;
            quantum_left = quantum_left.saturating_sub(done);
            *remaining = remaining.saturating_sub(executed.retired);
            let running = host.activity(cpu) == SchedActivity::Running;
            if done == 0 && executed.device_ns == 0 && running && !stop.requested() {
                return Err(SmpError::NoProgress { cpu });
            }
            if !running {
                quantum_left = 0;
            }
            if !running || stop.requested() {
                break;
            }
        }
        if quantum_left > 0 && (stop.requested() || *remaining == 0) {
            self.carry = Some(TurnCarry {
                cpu,
                quantum,
                left: quantum_left,
                steps,
                retired,
            });
        }
        if let Some(turns) = self.turns.as_mut() {
            turns.push(TurnRecord {
                cpu,
                quantum,
                steps,
                retired,
            });
        }
        Ok(())
    }
}

/// The board as a [`SmpHost`]: activity, IF and Local APICs of its vCPUs, the board's single clock
/// owner ([`LoadedFirmwareBoard::enable_clock_owner`] must have been called), and at every boundary
/// `apply_pending_startup` (INIT, SIPI) then `deliver_pending_nmi`.
///
/// The quantum itself is the caller's: `run` is the existing interpreter loop of the board. For a
/// vCPU in long mode it is [`LoadedFirmwareBoard::run_vcpu_set_quantum`] (S6-16d: the vCPU's own CR3
/// and EFER, its own Local APIC, no clock calls) with the `Cpl0ProfileSet` the caller builds; the
/// boot processor's Linux loop is not replaced by it. A vCPU in a legacy mode (an application
/// processor after SIPI, `Vcpu::legacy` is `Some`, S6-16c) is not run by `run`: the host executes its
/// trampoline on the RAM of the board ([`crate::ap_legacy_exec::run_legacy`]) and, when the
/// trampoline's 64-bit jump has retired, calls `run` for the rest of the budget in the same call (a
/// call that retired nothing would be `NoProgress` to the scheduler). Whatever the legacy models
/// refuse is an error, never a silent skip.
///
/// The interrupt source of the board (PIC, IOAPIC, PIT: `irq_source`) belongs to the boot processor
/// (S6-21 step 1): its quantum asks the source at every instruction boundary and delivers through
/// its IDT (`run_vcpu_set_quantum`), the host wakes a halted boot processor when the source has a
/// vector ([`SmpHost::board_wakes`]), lifts a deadline the clock has reached
/// ([`SmpHost::board_deadline`]) and gives the idle jump the wake time of the source
/// ([`SmpHost::idle_deadline`]). No other vCPU is offered the source.
///
/// The `run` closure must not call `charge_retired`, `charge_ns`, `idle_to` or any other
/// [`ClockOwner`] method: only the scheduler moves the clock. This is a condition of wiring the
/// closure in (S6-16/S6-17).
pub(crate) struct BoardSmpHost<'a, R> {
    board: &'a mut LoadedFirmwareBoard,
    run: R,
}

impl<'a, R> BoardSmpHost<'a, R>
where
    R: FnMut(
        &mut LoadedFirmwareBoard,
        usize,
        u64,
        &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError>,
{
    pub(crate) fn new(
        board: &'a mut LoadedFirmwareBoard,
        run: R,
    ) -> Result<Self, FirmwareBoardError> {
        if !board.clock_owner_enabled() {
            return Err(FirmwareBoardError::Config(
                "smp scheduler: the board has no clock owner (enable_clock_owner)",
            ));
        }
        Ok(Self { board, run })
    }

    pub(crate) fn board(&self) -> &LoadedFirmwareBoard {
        self.board
    }
}

impl<R> SmpHost for BoardSmpHost<'_, R>
where
    R: FnMut(
        &mut LoadedFirmwareBoard,
        usize,
        u64,
        &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError>,
{
    type Error = FirmwareBoardError;

    fn vcpu_count(&self) -> usize {
        self.board.vcpu_count()
    }

    fn activity(&self, cpu: usize) -> SchedActivity {
        SchedActivity::from(
            self.board
                .vcpu(cpu)
                .expect("a vCPU of the board")
                .activity(),
        )
    }

    fn if_set(&self, cpu: usize) -> bool {
        self.board
            .vcpu(cpu)
            .expect("a vCPU of the board")
            .interrupts_enabled()
    }

    fn apic(&self, cpu: usize) -> LocalApicHandle {
        self.board
            .local_apic_of(cpu)
            .expect("a Local APIC per vCPU")
    }

    fn clock_owner(&mut self) -> &mut ClockOwner {
        self.board
            .clock_owner_mut()
            .expect("checked in BoardSmpHost::new")
    }

    /// The edge of the board's source the clock has not reached. An edge the clock has reached is
    /// the boot processor's to take at its own boundary: its quantum asks the source at every
    /// instruction boundary ([`LoadedFirmwareBoard::run_vcpu_set_quantum`]) and the real sources
    /// apply every due edge when they are asked (`catch_up`), after which their
    /// `next_deadline_ns` is not in the past (`BoardIrqSource::next_deadline_ns`). The filter is
    /// needed for test sources and for the invariant of the trait: a deadline at or before now
    /// would otherwise cut the turns of the other vCPUs to one step each (S6-21 step 1).
    fn board_deadline(&self) -> Option<u64> {
        let now = self.board.clock().now_ns();
        self.board
            .irq_deadline_ns()
            .filter(|deadline| *deadline > now)
    }

    /// Where the idle jump goes: the next time the board's source can wake the halted boot
    /// processor, and only if the processor can take it (IF = 1). A source that cannot wake it (a
    /// masked or in-service line, no edge) gives no deadline, so the run ends as `AllIdle` and does
    /// not jump from edge to edge for ever (the Linux loop ends as `halted` in the same case).
    fn idle_deadline(&self) -> Option<u64> {
        let boot = self.board.vcpu(BOOT_VCPU_INDEX)?;
        if boot.activity() != od_isa::CpuActivity::Halted || !boot.interrupts_enabled() {
            return None;
        }
        let now = self.board.clock().now_ns();
        self.board.irq_wake_ns().filter(|wake| *wake > now)
    }

    fn board_wakes(&self, cpu: usize) -> bool {
        cpu == BOOT_VCPU_INDEX
            && self
                .board
                .irq_wake_ns()
                .is_some_and(|wake| wake <= self.board.clock().now_ns())
    }

    fn boundary(&mut self, cpu: usize) -> Result<BoundaryReport, FirmwareBoardError> {
        let startup = self.board.apply_pending_startup(cpu)?;
        let nmi = self.board.deliver_pending_nmi(cpu)?;
        Ok(BoundaryReport {
            init: startup.init,
            started: startup.started,
            nmi_delivered: nmi == NmiBoundary::Delivered,
        })
    }

    /// A `Running` vCPU of the board stands on a completed instruction, or, when its last call
    /// ended inside a `REP` (`skip_first_boundary`), on the restart point of that `REP`: all of its
    /// state is the [`crate::vcpu::Vcpu`] (the quantum reads and writes it at every call). A
    /// vCPU that is not there, or whose Local APIC the board cannot find, does not acknowledge.
    fn stop_ack(&self, cpu: usize) -> Option<StopAck> {
        let vcpu = self.board.vcpu(cpu)?;
        self.board.local_apic_of(cpu)?;
        Some(match SchedActivity::from(vcpu.activity()) {
            SchedActivity::Running if vcpu.skip_first_boundary => StopAck::RepRestart,
            SchedActivity::Running => StopAck::Boundary,
            asleep => StopAck::Asleep(asleep),
        })
    }

    fn wake(&mut self, cpu: usize) -> Result<(), FirmwareBoardError> {
        let vcpu = self
            .board
            .vcpu_mut(cpu)
            .ok_or(FirmwareBoardError::Config("smp scheduler: no such vCPU"))?;
        vcpu.state.activity = Some(od_isa::CpuActivity::Running);
        Ok(())
    }

    fn run(
        &mut self,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError> {
        let vcpu = self
            .board
            .vcpu(cpu)
            .ok_or(FirmwareBoardError::Config("smp scheduler: no such vCPU"))?;
        if vcpu.legacy.is_none() {
            return (self.run)(self.board, cpu, budget, stop);
        }
        let legacy = crate::ap_legacy_exec::run_legacy(self.board, cpu, budget, stop)?;
        let mut executed = legacy.executed;
        // The trampoline reached long mode: the rest of the budget is the long-mode quantum, in this
        // call, because the scheduler takes a call without a step for lack of progress.
        let left = budget.saturating_sub(executed.retired);
        if legacy.entered_long_mode && left > 0 {
            let rest = (self.run)(self.board, cpu, left, stop)?;
            executed.retired += rest.retired;
            executed.partial_rep = rest.partial_rep;
            executed.device_ns += rest.device_ns;
        }
        Ok(executed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interleaved_harness::{
        FILL, HLT, HZ, NEVER_RIP, Ram, SPIN, VECTOR, Vcpu as TestCpu,
    };
    use crate::{
        AccessWidth, BusDevice, DeviceAccess, FirmwareBoardConfig, IoSpace, LocalApic, PciEcam,
        VirtualClockHandle, firmware_board::assemble_board,
    };
    use od_isa::execute::{RunConfig, RunExit, RunResult, run, run_with_halt_profile};
    use od_isa::memory::{AccessContext, PrivilegeLevel};
    use od_isa::{CpuActivity, HaltProfile, LongModeState, Register};
    use std::convert::Infallible;

    const IF_BIT: u64 = 1 << 9;
    /// The cell the racing programs update.
    const CELL: u64 = 0x3e00;
    const CODE: [u64; 4] = [0x100, 0x200, 0x300, 0x400];

    /// `mov rax,[rbx]; inc rax; mov [rbx],rax; dec rcx; jnz top; hlt`: a non-atomic increment
    /// of the cell, `rcx` times. Five steps an iteration, then the `hlt`.
    const INC_LOOP: [u8; 15] = [
        0x48, 0x8b, 0x03, 0x48, 0xff, 0xc0, 0x48, 0x89, 0x03, 0x48, 0xff, 0xc9, 0x75, 0xf2, 0xf4,
    ];
    const INC_LOOP_HLT: u64 = 14;

    /// The machine under the scheduler: real `od_isa::execute` on identity RAM, one LAPIC per vCPU
    /// on one clock, the single clock owner.
    struct Rig {
        ram: Ram,
        cpus: Vec<TestCpu>,
        device: LocalApic,
        owner: ClockOwner,
        stop: StopRequest,
        /// `(cpu, n)`: that vCPU asks for STOP before its (n + 1)-th instruction of the run.
        stop_hook: Option<(usize, u64)>,
        polls: u64,
        /// Device nanoseconds every call reports.
        device_ns: u64,
        /// `(cpu, budget)` of every call of the host.
        calls: Vec<(usize, u64)>,
        boundaries: Vec<usize>,
        /// `(cpu, now)` of every wake.
        wakes: Vec<(usize, u64)>,
        /// vCPUs with an NMI waiting; the imitated boundary takes it (see `boundary`).
        nmi_waiting: Vec<usize>,
        /// vCPUs that took an NMI (vector 2), in order.
        nmi_taken: Vec<usize>,
        /// The deadline of the board's interrupt source; the imitated boundary consumes it once
        /// the clock has reached it. The real source does not keep a past deadline after its
        /// catch-up; this field models a test source that does, to exercise the host's filter.
        board_deadline: Option<u64>,
        /// Keep a reached deadline instead of consuming it (a host that never consumes it).
        keep_board_deadline: bool,
        /// vCPUs that do not acknowledge a stop (`stop_ack` is `None`) until `settle` brings them
        /// (`settle_arrives`) or for ever.
        stuck: Vec<usize>,
        settle_arrives: bool,
        /// The vCPU of every `settle` call.
        settles: Vec<usize>,
    }

    impl Rig {
        fn new(cpus: Vec<TestCpu>) -> Self {
            let clock = VirtualClockHandle::new(HZ).unwrap();
            let device = LocalApic::with_shared_timers(clock.clone(), HZ, cpus.len()).unwrap();
            let apics = (0..cpus.len())
                .map(|cpu| device.handle_for(cpu).unwrap())
                .collect();
            let owner = ClockOwner::new(&clock, apics).unwrap();
            let mut rig = Self {
                ram: Ram(vec![0; crate::interleaved_harness::RAM_SIZE]),
                cpus,
                device,
                owner,
                stop: StopRequest::new(),
                stop_hook: None,
                polls: 0,
                device_ns: 0,
                calls: Vec::new(),
                boundaries: Vec::new(),
                wakes: Vec::new(),
                nmi_waiting: Vec::new(),
                nmi_taken: Vec::new(),
                board_deadline: None,
                keep_board_deadline: false,
                stuck: Vec::new(),
                settle_arrives: false,
                settles: Vec::new(),
            };
            for cpu in 0..rig.cpus.len() {
                rig.write(cpu, 0xf0, 0x1ff);
            }
            rig
        }

        /// `INC_LOOP` on each of `count` vCPUs (code at `CODE[i]`), `iterations` times, one cell.
        fn racing(count: usize, iterations: u64) -> Self {
            let cpus = (0..count)
                .map(|i| {
                    let mut cpu = TestCpu::new(CODE[i], CODE[i] + INC_LOOP_HLT, CELL, 0);
                    cpu.state.gpr[Register::Rcx as usize] = iterations;
                    cpu
                })
                .collect();
            let mut rig = Self::new(cpus);
            for i in 0..count {
                rig.ram.load(CODE[i], &INC_LOOP);
            }
            rig
        }

        /// vCPU programs that never end: `jmp -2`.
        fn spinning(count: usize) -> Self {
            let cpus = (0..count)
                .map(|i| TestCpu::new(CODE[i], NEVER_RIP, CELL, 0))
                .collect();
            let mut rig = Self::new(cpus);
            for i in 0..count {
                rig.ram.load(CODE[i], &SPIN);
            }
            rig
        }

        fn write(&mut self, cpu: usize, offset: u64, value: u64) {
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: AccessWidth::Dword,
            };
            self.device.write_from_cpu(cpu, access, value).unwrap();
        }

        /// One-shot LAPIC timer of `cpu`, `count` ns after now.
        fn arm(&mut self, cpu: usize, count: u64) {
            self.write(cpu, 0x320, u64::from(VECTOR));
            self.write(cpu, 0x3e0, 0xb);
            self.write(cpu, 0x380, count);
        }

        fn now(&self) -> u64 {
            self.owner.now_ns()
        }

        fn cell(&self) -> u64 {
            let at = CELL as usize;
            u64::from_le_bytes(self.ram.0[at..at + 8].try_into().unwrap())
        }

        fn scheduler(&self, policy: QuantumPolicy) -> SmpScheduler {
            SmpScheduler::new(self.cpus.len(), policy, self.stop.clone()).unwrap()
        }
    }

    impl SmpHost for Rig {
        type Error = Infallible;

        fn vcpu_count(&self) -> usize {
            self.cpus.len()
        }

        fn activity(&self, cpu: usize) -> SchedActivity {
            SchedActivity::from(self.cpus[cpu].state.activity.unwrap())
        }

        fn if_set(&self, cpu: usize) -> bool {
            self.cpus[cpu].state.rflags & IF_BIT != 0
        }

        fn apic(&self, cpu: usize) -> LocalApicHandle {
            self.device.handle_for(cpu).unwrap()
        }

        fn clock_owner(&mut self) -> &mut ClockOwner {
            &mut self.owner
        }

        fn board_deadline(&self) -> Option<u64> {
            self.board_deadline
        }

        fn stop_ack(&self, cpu: usize) -> Option<StopAck> {
            if self.stuck.contains(&cpu) {
                return None;
            }
            Some(match self.activity(cpu) {
                SchedActivity::Running => StopAck::Boundary,
                asleep => StopAck::Asleep(asleep),
            })
        }

        fn settle(&mut self, cpu: usize) -> Result<(), Infallible> {
            self.settles.push(cpu);
            if self.settle_arrives {
                self.stuck.retain(|&stuck| stuck != cpu);
            }
            Ok(())
        }

        fn boundary(&mut self, cpu: usize) -> Result<BoundaryReport, Infallible> {
            self.boundaries.push(cpu);
            if !self.keep_board_deadline
                && self
                    .board_deadline
                    .is_some_and(|deadline| deadline <= self.now())
            {
                self.board_deadline = None;
            }
            // A minimal imitation of `deliver_pending_nmi` for the order of the boundary: an NMI
            // is taken by a running or halted vCPU whatever IF is and makes it `Running` (entering
            // the IDT is not modelled); a vCPU waiting for SIPI or shut down holds it. The real
            // delivery needs the board with paging and an IDT and is covered by the board's tests.
            let mut report = BoundaryReport::default();
            let takes = matches!(
                self.cpus[cpu].state.activity,
                Some(CpuActivity::Running | CpuActivity::Halted)
            );
            if takes && let Some(at) = self.nmi_waiting.iter().position(|&waiting| waiting == cpu) {
                self.nmi_waiting.remove(at);
                self.nmi_taken.push(cpu);
                self.cpus[cpu].state.activity = Some(CpuActivity::Running);
                self.cpus[cpu].hlt_rip = NEVER_RIP;
                report.nmi_delivered = true;
            }
            Ok(report)
        }

        /// The vector is taken and ended at once (entering the IDT is not modelled); the vCPU
        /// continues after its `hlt` with the stop address of the next program segment.
        fn wake(&mut self, cpu: usize) -> Result<(), Infallible> {
            let now = self.now();
            let vector = self.apic(cpu).acknowledge();
            assert!(vector.is_some(), "woken with an eligible vector");
            self.write(cpu, 0xb0, 0);
            let vcpu = &mut self.cpus[cpu];
            vcpu.state.activity = Some(CpuActivity::Running);
            vcpu.hlt_rip = if vcpu.later_hlt.is_empty() {
                NEVER_RIP
            } else {
                vcpu.later_hlt.remove(0)
            };
            self.wakes.push((cpu, now));
            Ok(())
        }

        fn run(
            &mut self,
            cpu: usize,
            budget: u64,
            stop: &mut dyn FnMut() -> bool,
        ) -> Result<Executed, Infallible> {
            assert_eq!(
                self.activity(cpu),
                SchedActivity::Running,
                "the scheduler runs only a running vCPU"
            );
            assert!(budget >= 1);
            self.calls.push((cpu, budget));
            let context = AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            };
            let Self {
                ram,
                cpus,
                polls,
                stop_hook,
                stop: request,
                device_ns,
                ..
            } = self;
            let hook = *stop_hook;
            let mut poll = || {
                if let Some((hook_cpu, after)) = hook
                    && hook_cpu == cpu
                {
                    *polls += 1;
                    if *polls > after {
                        request.request();
                    }
                }
                stop()
            };
            let vcpu = &mut cpus[cpu];
            let hlt_rip = vcpu.hlt_rip;
            let mut result = RunResult {
                exit: RunExit::StopAddress,
                instructions_executed: 0,
                rflags_defined: 0,
            };
            if vcpu.state.rip != hlt_rip {
                result = run(
                    &mut vcpu.state,
                    ram,
                    RunConfig {
                        instruction_budget: budget,
                        stop_rip: Some(hlt_rip),
                        context,
                    },
                    &mut poll,
                );
            }
            let mut retired = result.instructions_executed;
            if result.exit == RunExit::StopAddress && budget > retired {
                result = run_with_halt_profile(
                    &mut vcpu.state,
                    ram,
                    RunConfig {
                        instruction_budget: budget - retired,
                        stop_rip: None,
                        context,
                    },
                    &HaltProfile::Long64V1,
                    &mut poll,
                );
                retired += result.instructions_executed;
            }
            assert!(
                matches!(
                    result.exit,
                    RunExit::StopAddress
                        | RunExit::Halted
                        | RunExit::InstructionBudgetExhausted
                        | RunExit::RepIterationBudgetExhausted
                        | RunExit::StopRequested
                ),
                "{:?}",
                result.exit
            );
            vcpu.retired += retired;
            Ok(Executed {
                retired,
                partial_rep: result.exit == RunExit::RepIterationBudgetExhausted,
                device_ns: *device_ns,
            })
        }
    }

    fn finished(rig: &mut Rig, policy: QuantumPolicy, budget: u64) -> (SmpScheduler, Finished) {
        let mut scheduler = rig.scheduler(policy);
        scheduler.record_turns();
        let result = scheduler.run(rig, budget).unwrap();
        (scheduler, result)
    }

    #[test]
    fn splitmix64_matches_the_reference_sequence_and_the_range_includes_both_ends() {
        // The first outputs of the reference implementation with seed 0.
        let mut rng = SplitMix64::new(0);
        assert_eq!(rng.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(rng.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        assert_eq!(rng.next_u64(), 0x06C4_5D18_8009_454F);
        let mut rng = SplitMix64::new(42);
        let drawn: Vec<u64> = (0..200).map(|_| rng.in_range(1, 3)).collect();
        assert!(drawn.iter().all(|n| (1..=3).contains(n)));
        for wanted in 1..=3 {
            assert!(drawn.contains(&wanted), "{wanted} is drawn");
        }
        assert!((0..50).all(|_| rng.in_range(1, 1) == 1));
    }

    #[test]
    fn a_policy_without_a_quantum_is_refused() {
        let stop = StopRequest::new();
        let build = |policy| SmpScheduler::new(2, policy, stop.clone()).err();
        assert!(build(QuantumPolicy::Fixed(0)).is_some());
        assert!(
            build(QuantumPolicy::Seeded {
                seed: 1,
                min: 0,
                max: 4
            })
            .is_some()
        );
        assert!(
            build(QuantumPolicy::Seeded {
                seed: 1,
                min: 5,
                max: 4
            })
            .is_some()
        );
        assert!(SmpScheduler::new(0, QuantumPolicy::default(), stop.clone()).is_err());
        assert!(build(QuantumPolicy::default()).is_none());
        assert!(build(QuantumPolicy::seeded(7)).is_none());
        assert!(build(QuantumPolicy::Schedule(vec![])).is_none());
    }

    /// The same program as one vCPU of `Rig::racing`, run straight through the interpreter.
    fn direct_run(iterations: u64) -> (LongModeState, Vec<u8>, u64) {
        let mut cpu = TestCpu::new(CODE[0], CODE[0] + INC_LOOP_HLT, CELL, 0);
        cpu.state.gpr[Register::Rcx as usize] = iterations;
        let mut ram = Ram(vec![0; crate::interleaved_harness::RAM_SIZE]);
        ram.load(CODE[0], &INC_LOOP);
        let context = AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        };
        let first = run(
            &mut cpu.state,
            &mut ram,
            RunConfig {
                instruction_budget: 1_000_000,
                stop_rip: Some(CODE[0] + INC_LOOP_HLT),
                context,
            },
            || false,
        );
        let second = run_with_halt_profile(
            &mut cpu.state,
            &mut ram,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context,
            },
            &HaltProfile::Long64V1,
            || false,
        );
        let retired = first.instructions_executed + second.instructions_executed;
        (cpu.state, ram.0, retired)
    }

    #[test]
    fn one_vcpu_gives_the_result_of_a_direct_run_under_every_policy() {
        let (state, memory, retired) = direct_run(20);
        assert_eq!(retired, 20 * 5 + 1);
        assert_eq!(
            u64::from_le_bytes(memory[CELL as usize..CELL as usize + 8].try_into().unwrap()),
            20
        );
        let policies = [
            QuantumPolicy::Fixed(1),
            QuantumPolicy::Fixed(7),
            QuantumPolicy::default(),
            QuantumPolicy::seeded(1),
            QuantumPolicy::seeded(99),
            QuantumPolicy::Seeded {
                seed: 3,
                min: 1,
                max: 2,
            },
        ];
        for policy in policies {
            let mut rig = Rig::racing(1, 20);
            let (scheduler, result) = finished(&mut rig, policy.clone(), 1_000_000);
            assert_eq!(result, Finished::AllIdle, "{policy:?}");
            assert_eq!(rig.cpus[0].state, state, "{policy:?}");
            assert_eq!(rig.ram.0, memory, "{policy:?}");
            assert_eq!(scheduler.counters()[0].retired, retired, "{policy:?}");
            // D7: one retired instruction is one nanosecond, and nothing else moved the clock.
            assert_eq!(rig.now(), retired, "{policy:?}");
            assert_eq!(scheduler.idle(), (0, 0));
        }
    }

    /// Final cell, per-vCPU counters, the executed schedule, the whole RAM and the architectural
    /// state of every vCPU of a seeded two-vCPU race.
    type Race = (
        u64,
        Vec<VcpuCounters>,
        Vec<usize>,
        Vec<u8>,
        Vec<LongModeState>,
    );

    fn race(seed: u64) -> Race {
        let mut rig = Rig::racing(2, 20);
        // Short turns: the programs are a hundred steps long.
        let policy = QuantumPolicy::Seeded {
            seed,
            min: 1,
            max: 16,
        };
        let (scheduler, result) = finished(&mut rig, policy, 1_000_000);
        assert_eq!(result, Finished::AllIdle);
        (
            rig.cell(),
            scheduler.counters().to_vec(),
            scheduler.executed_schedule(),
            rig.ram.0.clone(),
            rig.cpus.iter().map(|cpu| cpu.state.clone()).collect(),
        )
    }

    #[test]
    fn the_same_seed_gives_the_same_run_and_different_seeds_give_different_schedules() {
        let first = race(5);
        assert_eq!(race(5), first);
        // Both vCPUs ran all their steps: 20 iterations of 5 steps and the `hlt`.
        assert_eq!(first.1[0].retired, 101);
        assert_eq!(first.1[1].retired, 101);
        assert_eq!(first.2.len(), 202);
        // The update is not atomic, so the cell is the observable of the interleaving.
        assert!(first.0 >= 20 && first.0 <= 40, "{}", first.0);
        let runs: Vec<_> = (1..=8).map(race).collect();
        for (seed, run) in (1..=8).zip(&runs) {
            assert_eq!(&race(seed), run, "seed {seed} twice");
        }
        for pair in runs.windows(2) {
            assert_ne!(pair[0].2, pair[1].2, "neighbouring seeds share a schedule");
        }
        let cells: std::collections::BTreeSet<u64> = runs.iter().map(|run| run.0).collect();
        assert!(
            cells.len() > 1,
            "the final cell never depends on the seed: {cells:?}"
        );
        // The quanta stay inside the recorded limits, and a turn of one step happens.
        let mut rig = Rig::racing(2, 20);
        let (scheduler, _) = finished(
            &mut rig,
            QuantumPolicy::Seeded {
                seed: 11,
                min: 1,
                max: 4,
            },
            1_000_000,
        );
        assert!(
            scheduler
                .turns()
                .iter()
                .all(|turn| (1..=4).contains(&turn.quantum))
        );
        assert!(scheduler.turns().iter().any(|turn| turn.quantum == 1));
    }

    #[test]
    fn a_recorded_schedule_replays_to_the_same_result() {
        // The test host does not depend on how its calls are cut and the programs raise no
        // boundary events, which is what a replay by schedule needs (an event raised during a turn
        // is applied at the next boundary, so a real run may diverge: a limit for S6-19).
        let (cell, counters, schedule, ram, cpu_states) = race(9);
        let mut rig = Rig::racing(2, 20);
        let (scheduler, result) = finished(
            &mut rig,
            QuantumPolicy::Schedule(schedule.clone()),
            1_000_000,
        );
        assert_eq!(result, Finished::ScheduleEnd);
        assert_eq!(scheduler.executed_schedule(), schedule);
        assert_eq!(rig.cell(), cell);
        assert_eq!(scheduler.counters()[0].retired, counters[0].retired);
        assert_eq!(scheduler.counters()[1].retired, counters[1].retired);
        // The whole RAM and the state of every vCPU, not only the cell and the counters.
        assert!(rig.ram.0 == ram, "RAM differs after the replay");
        let states: Vec<LongModeState> = rig.cpus.iter().map(|cpu| cpu.state.clone()).collect();
        assert_eq!(states, cpu_states);
    }

    #[test]
    fn an_explicit_schedule_runs_exactly_as_given() {
        // One iteration: six steps a vCPU (load, inc, store, dec, jnz, hlt).
        // Interleaved load/load/.../store/store loses an update: the cell is 1.
        let lost = [0, 1, 0, 0, 1, 1, 0, 0, 0, 1, 1, 1];
        let mut rig = Rig::racing(2, 1);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Schedule(lost.to_vec()), 1000);
        assert_eq!(result, Finished::ScheduleEnd);
        assert_eq!(scheduler.executed_schedule(), lost);
        assert!(scheduler.turns().iter().all(|turn| turn.steps == 1));
        assert_eq!(rig.cell(), 1);
        assert_eq!(
            rig.calls.iter().map(|call| call.0).collect::<Vec<_>>(),
            lost
        );
        // One vCPU after the other: both updates count.
        let serial = [0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1];
        let mut rig = Rig::racing(2, 1);
        let (scheduler, result) =
            finished(&mut rig, QuantumPolicy::Schedule(serial.to_vec()), 1000);
        assert_eq!((result, rig.cell()), (Finished::ScheduleEnd, 2));
        assert_eq!(scheduler.counters()[0].retired, 6);
        assert_eq!(scheduler.counters()[1].retired, 6);
        // A prefix leaves the rest untouched.
        let mut rig = Rig::racing(2, 1);
        let (scheduler, _) = finished(&mut rig, QuantumPolicy::Schedule(vec![1, 1]), 1000);
        assert_eq!(
            (
                scheduler.counters()[0].retired,
                scheduler.counters()[1].retired
            ),
            (0, 2)
        );
    }

    #[test]
    fn a_schedule_naming_a_vcpu_that_cannot_run_is_an_error_and_runs_nothing_more() {
        let mut rig = Rig::racing(2, 1);
        let mut scheduler = rig.scheduler(QuantumPolicy::Schedule(vec![0, 0, 0, 0, 0, 0, 0]));
        // cpu 0 halts after its sixth step; the seventh entry names it again while cpu 1 runs.
        let error = scheduler.run(&mut rig, 1000).unwrap_err();
        assert_eq!(
            error,
            SmpError::ScheduleCpuNotRunning {
                position: 6,
                cpu: 0,
                activity: SchedActivity::Halted
            }
        );
        assert_eq!(rig.calls.len(), 6);
        let mut rig = Rig::racing(2, 1);
        let mut scheduler = rig.scheduler(QuantumPolicy::Schedule(vec![0, 2]));
        assert_eq!(
            scheduler.run(&mut rig, 1000).unwrap_err(),
            SmpError::ScheduleCpuOutOfRange {
                position: 1,
                cpu: 2
            }
        );
    }

    /// `rep stosd` with 32-bit addresses (`f3 67 ab`): the interpreter stops it after
    /// `REP_ITERATION_WORK_LIMIT` iterations with RIP on the `REP`.
    fn rep_and_loop(count: u64) -> Rig {
        let mut rep = TestCpu::new(0x100, 0x103, 0, 0xaaaa);
        // The harness vCPU has DF set; the REP goes upwards.
        rep.state.rflags &= !(1 << 10);
        rep.state.gpr[Register::Rcx as usize] = count;
        rep.state.gpr[Register::Rdi as usize] = 0x1000;
        let mut looper = TestCpu::new(CODE[1], CODE[1] + INC_LOOP_HLT, CELL, 0);
        looper.state.gpr[Register::Rcx as usize] = 3;
        let mut rig = Rig::new(vec![rep, looper]);
        rig.ram.load(0x100, &[0xf3, 0x67, 0xab, HLT]);
        rig.ram.load(CODE[1], &INC_LOOP);
        rig
    }

    #[test]
    fn a_long_rep_does_not_hold_the_second_vcpu_longer_than_a_quantum() {
        // 2500 iterations are three interpreter calls: 1024, 1024 and 452 (the last retires).
        let mut rig = rep_and_loop(2500);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(2), 10_000);
        assert_eq!(result, Finished::AllIdle);
        // The first turn is two partial-progress returns: two steps, nothing retired, and the
        // REP is far from done when vCPU 1 gets its turn.
        assert_eq!(
            scheduler.turns()[..3],
            [
                TurnRecord {
                    cpu: 0,
                    quantum: 2,
                    steps: 2,
                    retired: 0
                },
                TurnRecord {
                    cpu: 1,
                    quantum: 2,
                    steps: 2,
                    retired: 2
                },
                TurnRecord {
                    cpu: 0,
                    quantum: 2,
                    steps: 2,
                    retired: 2
                },
            ]
        );
        assert_eq!(rig.calls[..3], [(0, 2), (0, 1), (1, 2)]);
        // The REP finished: all 2500 dwords are written, and not one more.
        let dword = |at: usize| u32::from_le_bytes(rig.ram.0[at..at + 4].try_into().unwrap());
        assert_eq!(dword(0x1000), 0xaaaa);
        assert_eq!(dword(0x1000 + 4 * 2499), 0xaaaa);
        assert_eq!(dword(0x1000 + 4 * 2500), 0);
        assert_eq!(rig.cpus[0].state.gpr[Register::Rcx as usize], 0);
        assert_eq!(rig.cell(), 3);
        // The REP is one retirement and one nanosecond, however many calls it took.
        assert_eq!(scheduler.counters()[0].retired, 2);
        assert_eq!(scheduler.counters()[0].steps, 4);
    }

    #[test]
    fn all_halted_the_timer_of_one_lapic_wakes_it_and_time_skips_to_the_nearest_deadline() {
        // Segment one, `hlt`, segment two, `hlt`: each segment is one filler and one `hlt`.
        let cpu = |base: u64| {
            let mut cpu = TestCpu::new(base, base + 3, CELL, 7);
            cpu.later_hlt = vec![base + 7];
            cpu
        };
        let mut rig = Rig::new(vec![cpu(CODE[0]), cpu(CODE[1])]);
        for base in [CODE[0], CODE[1]] {
            rig.ram.load(base, &[FILL[0], FILL[1], FILL[2], HLT]);
            rig.ram.load(base + 4, &[FILL[0], FILL[1], FILL[2], HLT]);
        }
        // The later deadline belongs to vCPU 0, the earlier to vCPU 1: the queues of all vCPUs count.
        rig.arm(0, 9000);
        rig.arm(1, 5000);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(100), 1000);
        assert_eq!(result, Finished::AllIdle);
        assert_eq!(rig.wakes, [(1, 5000), (0, 9000)]);
        let counters = scheduler.counters();
        assert_eq!((counters[0].wakes, counters[1].wakes), (1, 1));
        assert_eq!((counters[0].retired, counters[1].retired), (4, 4));
        // 4 instructions, a jump to 5000, 2 more, a jump to 9000, 2 more.
        assert_eq!(rig.now(), 9002);
        assert_eq!(scheduler.idle(), (4996 + 3998, 2));
        assert_eq!(rig.now(), scheduler.retired_total() + scheduler.idle().0);
        // Both ended halted for good: nothing can wake them any more.
        assert_eq!(rig.activity(0), SchedActivity::Halted);
        assert_eq!(rig.activity(1), SchedActivity::Halted);
    }

    #[test]
    fn a_halted_vcpu_with_a_vector_it_cannot_take_stays_skipped_until_it_can() {
        // vCPU 0 spins; vCPU 1 is halted with a pending vector and IF = 0; vCPU 2 waits for SIPI;
        // vCPU 3 has shut down. Only vCPU 0 runs.
        let mut rig = Rig::spinning(4);
        rig.cpus[1].state.activity = Some(CpuActivity::Halted);
        rig.cpus[1].state.rflags &= !IF_BIT;
        rig.cpus[2].state.activity = Some(CpuActivity::WaitForSipi);
        rig.cpus[3].state.activity = Some(CpuActivity::Shutdown);
        rig.apic(1).request_fixed_edge(VECTOR).unwrap();
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(7), 50);
        assert_eq!(result, Finished::BudgetExhausted);
        let retired: Vec<u64> = scheduler.counters().iter().map(|c| c.retired).collect();
        assert_eq!(retired, [50, 0, 0, 0]);
        assert_eq!(scheduler.counters()[1].turns, 0);
        assert_eq!(scheduler.counters()[1].wakes, 0);
        assert!(rig.wakes.is_empty());
        // IF = 1: the vector is acceptable, the vCPU wakes at the next boundary and runs.
        rig.cpus[1].state.rflags |= IF_BIT;
        rig.cpus[1].hlt_rip = NEVER_RIP;
        let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(7));
        assert_eq!(
            scheduler.run(&mut rig, 50).unwrap(),
            Finished::BudgetExhausted
        );
        assert_eq!(scheduler.counters()[1].wakes, 1);
        assert!(scheduler.counters()[1].retired > 0);
        assert_eq!(
            scheduler.counters()[2].turns + scheduler.counters()[3].turns,
            0
        );
    }

    #[test]
    fn an_nmi_on_the_boundary_comes_before_the_wake_of_a_halted_vcpu_with_if_clear() {
        // IF = 0 with no vector: only the NMI can start vCPU 1. IF = 1 with an acceptable vector:
        // the NMI is still taken first, so the vCPU is already `Running` when the wake is looked at
        // and the wake path does not run (the NMI is not lost behind the interrupt).
        for (interrupt_flag, vector) in [(false, false), (true, true)] {
            let mut rig = Rig::spinning(2);
            rig.cpus[1].state.activity = Some(CpuActivity::Halted);
            if interrupt_flag {
                rig.cpus[1].state.rflags |= IF_BIT;
            } else {
                rig.cpus[1].state.rflags &= !IF_BIT;
            }
            if vector {
                rig.apic(1).request_fixed_edge(VECTOR).unwrap();
            }
            rig.nmi_waiting.push(1);
            let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(5), 40);
            assert_eq!(result, Finished::BudgetExhausted);
            let counters = scheduler.counters();
            assert_eq!(counters[1].nmi_delivered, 1);
            assert_eq!(counters[1].wakes, 0, "interrupt flag {interrupt_flag}");
            assert!(rig.wakes.is_empty());
            assert_eq!(rig.nmi_taken, [1]);
            assert!(rig.nmi_waiting.is_empty());
            assert_eq!(rig.activity(1), SchedActivity::Running);
            assert!(counters[1].retired > 0 && counters[1].turns > 0);
            // Both vCPUs ran, in turn.
            assert!(counters[0].retired > 0);
        }
    }

    #[test]
    fn nothing_runs_and_nothing_can_wake_ends_the_run_without_moving_the_clock() {
        let mut rig = Rig::spinning(3);
        rig.cpus[0].state.activity = Some(CpuActivity::WaitForSipi);
        rig.cpus[1].state.activity = Some(CpuActivity::Shutdown);
        rig.cpus[2].state.activity = Some(CpuActivity::WaitForSipi);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::default(), 100);
        assert_eq!(result, Finished::AllIdle);
        assert_eq!((rig.now(), scheduler.retired_total()), (0, 0));
        assert!(rig.calls.is_empty());
        // The boundary of every vCPU ran, in index order.
        assert_eq!(rig.boundaries, [0, 1, 2]);
    }

    #[test]
    fn stop_during_the_quantum_of_the_second_vcpu_stops_every_vcpu() {
        let mut rig = Rig::spinning(2);
        rig.stop_hook = Some((1, 3));
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(10), 1000);
        assert_eq!(result, Finished::Stopped);
        // vCPU 0 had its whole quantum; vCPU 1 stopped after 3 of its 10 steps; nobody ran after.
        assert_eq!(scheduler.counters()[0].retired, 10);
        assert_eq!(scheduler.counters()[1].retired, 3);
        assert_eq!(rig.calls, [(0, 10), (1, 10)]);
        assert_eq!(rig.now(), 13);
        // Stopped stays stopped at the next boundary, whatever is asked, and runs nothing.
        let mut again = rig.scheduler(QuantumPolicy::Fixed(10));
        assert_eq!(again.run(&mut rig, 1000).unwrap(), Finished::Stopped);
        assert_eq!(rig.calls.len(), 2);
        // The caller clears the request to go on; the run continues from where it stopped.
        rig.stop.clear();
        rig.stop_hook = None;
        assert_eq!(again.run(&mut rig, 20).unwrap(), Finished::BudgetExhausted);
        assert_eq!(again.retired_total(), 20);
    }

    /// Cut the run of `racing(2, 40)` into two with the cut taken from `cut`, and compare it with
    /// the run that was not cut: counters, turns, the cell and the clock.
    fn raced_in_two(policy: QuantumPolicy, cut: u64, through_state: bool) {
        let whole = {
            let mut rig = Rig::racing(2, 40);
            let mut scheduler = rig.scheduler(policy.clone());
            scheduler.record_turns();
            assert_eq!(scheduler.run(&mut rig, 1_000).unwrap(), Finished::AllIdle);
            (
                scheduler.counters().to_vec(),
                scheduler.executed_schedule(),
                scheduler.turns().to_vec(),
                rig.cell(),
                rig.now(),
            )
        };
        let mut rig = Rig::racing(2, 40);
        let mut scheduler = rig.scheduler(policy.clone());
        scheduler.record_turns();
        assert_eq!(
            scheduler.run(&mut rig, cut).unwrap(),
            Finished::BudgetExhausted
        );
        if through_state {
            let state = scheduler.state();
            scheduler.reset();
            assert_eq!(scheduler.retired_total(), 0);
            assert_ne!(scheduler.state(), state);
            scheduler.set_state(state.clone()).unwrap();
            assert_eq!(scheduler.state(), state);
        }
        scheduler.resume();
        assert_eq!(
            scheduler.run(&mut rig, 1_000 - cut).unwrap(),
            Finished::AllIdle
        );
        let split = (
            scheduler.counters().to_vec(),
            scheduler.executed_schedule(),
            scheduler.turns().to_vec(),
            rig.cell(),
            rig.now(),
        );
        assert_eq!(split, whole, "{policy:?} cut {cut}");
    }

    /// `rep stosb; hlt` on each of `count` vCPUs: `iterations` iterations, so a REP longer than
    /// the work limit of one call (1024) comes back as a partial `REP` that retires nothing.
    fn repping(count: usize, iterations: u64) -> Rig {
        let cpus = (0..count)
            .map(|i| {
                let mut cpu = TestCpu::new(CODE[i], CODE[i] + 2, CELL, 0);
                cpu.state.gpr[Register::Rcx as usize] = iterations;
                cpu.state.gpr[Register::Rdi as usize] = 0x3d00 - 0x900 * i as u64;
                cpu
            })
            .collect();
        let mut rig = Rig::new(cpus);
        for i in 0..count {
            rig.ram.load(CODE[i], &[0xf3, 0xaa, 0xf4]);
        }
        rig
    }

    /// Run `repping(2, 2100)` whole and in pieces of `cut` retired instructions each, resuming
    /// every piece, and compare counters, turns, memory and the clock.
    fn repped_in_pieces(policy: QuantumPolicy, cut: u64) {
        let outcome = |cut: Option<u64>| {
            let mut rig = repping(2, 2100);
            let mut scheduler = rig.scheduler(policy.clone());
            scheduler.record_turns();
            match cut {
                None => assert_eq!(scheduler.run(&mut rig, 1_000).unwrap(), Finished::AllIdle),
                Some(cut) => {
                    let mut pieces = 0;
                    loop {
                        match scheduler.run(&mut rig, cut).unwrap() {
                            Finished::AllIdle => break,
                            Finished::BudgetExhausted => scheduler.resume(),
                            other => panic!("{other:?}"),
                        }
                        pieces += 1;
                        assert!(pieces < 1_000, "the pieces never end");
                    }
                }
            }
            (
                scheduler.counters().to_vec(),
                scheduler.executed_schedule(),
                scheduler.turns().to_vec(),
                rig.ram.0.clone(),
                rig.now(),
            )
        };
        assert_eq!(outcome(Some(cut)), outcome(None), "{policy:?} cut {cut}");
    }

    #[test]
    fn a_budget_that_ends_inside_a_partial_rep_goes_on_with_the_same_vcpu() {
        // The call that the budget cut ends on a partial REP: a step of the quantum was spent, no
        // instruction retired, so the budget has a rest. The rest is the same vCPU's (the quantum
        // is not over), as in the run that was not cut (S6-24 review M1).
        let mut rig = repping(2, 2100);
        let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(5_000));
        scheduler.record_turns();
        assert_eq!(
            scheduler.run(&mut rig, 1).unwrap(),
            Finished::BudgetExhausted
        );
        assert_eq!(rig.calls, [(0, 1), (0, 1), (0, 1)], "vCPU 1 got nothing");
        assert_eq!(scheduler.counters()[1].steps, 0);
        assert_eq!(scheduler.counters()[0].retired, 1);
        // Partial, partial, then the REP retires: three steps of the quantum in one turn.
        assert_eq!(scheduler.counters()[0].steps, 3);
        for cut in [1, 2, 3, 4, 7] {
            for quantum in [1, 2, 3, 4, 5, 5_000] {
                repped_in_pieces(QuantumPolicy::Fixed(quantum), cut);
            }
            for seed in 0..20 {
                repped_in_pieces(QuantumPolicy::seeded(seed), cut);
            }
        }
    }

    #[test]
    fn a_run_cut_by_the_budget_goes_on_as_the_run_that_was_not_cut() {
        for seed in 0..40 {
            for cut in [1, 2, 7, 33, 100, 150, 199] {
                raced_in_two(QuantumPolicy::seeded(seed), cut, false);
                raced_in_two(QuantumPolicy::seeded(seed), cut, true);
            }
        }
        for quantum in [1, 3, 64, 1024] {
            for cut in [1, 5, 100, 199] {
                raced_in_two(QuantumPolicy::Fixed(quantum), cut, cut % 2 == 1);
            }
        }
    }

    #[test]
    fn the_stop_inside_a_turn_leaves_the_rest_of_the_quantum_for_the_run_that_follows() {
        let mut rig = Rig::spinning(2);
        rig.stop_hook = Some((1, 3));
        let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(10));
        scheduler.record_turns();
        assert_eq!(scheduler.run(&mut rig, 1000).unwrap(), Finished::Stopped);
        assert_eq!(scheduler.counters()[1].retired, 3);
        // The resumed turn of vCPU 1 has its 7 steps; the record shows one turn of 10.
        rig.stop_hook = None;
        scheduler.resume();
        assert_eq!(
            scheduler.run(&mut rig, 17).unwrap(),
            Finished::BudgetExhausted
        );
        assert_eq!(scheduler.counters()[1].retired, 10);
        assert_eq!(scheduler.counters()[1].turns, 1);
        // The 7 steps that were left, then the next turn of vCPU 0 with the rest of the budget.
        assert_eq!(scheduler.counters()[0].retired, 20);
        assert_eq!(
            scheduler
                .turns()
                .iter()
                .map(|turn| (turn.cpu, turn.steps))
                .collect::<Vec<_>>(),
            [(0, 10), (1, 10), (0, 10)]
        );
    }

    #[test]
    fn a_plain_run_after_a_cut_starts_a_new_turn_and_only_the_resume_continues_the_cut_one() {
        let counters = |resume: bool| {
            let mut rig = Rig::spinning(2);
            let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(10));
            assert_eq!(
                scheduler.run(&mut rig, 5).unwrap(),
                Finished::BudgetExhausted
            );
            if resume {
                scheduler.resume();
            }
            scheduler.run(&mut rig, 10).unwrap();
            let c = scheduler.counters();
            (c[0].retired, c[1].retired, c[0].turns, c[1].turns)
        };
        // Plain: vCPU 0 keeps its 5, the next turn is vCPU 1's, with the full 10 steps.
        assert_eq!(counters(false), (5, 10, 1, 1));
        // Resumed: vCPU 0 finishes its quantum (5 more), vCPU 1 takes the 5 that are left.
        assert_eq!(counters(true), (10, 5, 1, 1));
    }

    fn halted_and_waiting() -> Rig {
        let mut rig = Rig::spinning(3);
        rig.cpus[1].state.activity = Some(CpuActivity::Halted);
        rig.cpus[2].state.activity = Some(CpuActivity::WaitForSipi);
        rig
    }

    #[test]
    fn stop_all_takes_the_acknowledgement_of_every_vcpu_the_sleeping_ones_included() {
        let mut rig = halted_and_waiting();
        let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(10));
        scheduler.run(&mut rig, 25).unwrap();
        let (now, state) = (rig.now(), scheduler.state());
        let point = scheduler.stop_all(&mut rig, 4).unwrap();
        assert_eq!(
            point.acks,
            [
                StopAck::Boundary,
                StopAck::Asleep(SchedActivity::Halted),
                StopAck::Asleep(SchedActivity::WaitForSipi)
            ]
        );
        assert_eq!((point.now_ns, point.rounds), (now, 0));
        // Time stands, the scheduler keeps its state, and nothing runs until the explicit resume.
        assert_eq!(rig.now(), now);
        assert_eq!(scheduler.state(), state);
        assert!(scheduler.paused());
        let calls = rig.calls.len();
        assert_eq!(scheduler.run(&mut rig, 1000).unwrap(), Finished::Stopped);
        assert_eq!((rig.calls.len(), rig.now()), (calls, now));
        scheduler.resume();
        assert!(!scheduler.paused());
        assert_eq!(
            scheduler.run(&mut rig, 10).unwrap(),
            Finished::BudgetExhausted
        );
        assert!(rig.now() > now);
    }

    #[test]
    fn a_stop_without_the_acknowledgement_of_every_vcpu_is_no_stop() {
        let mut rig = halted_and_waiting();
        rig.stuck = vec![1];
        let mut scheduler = rig.scheduler(QuantumPolicy::Fixed(10));
        scheduler.run(&mut rig, 25).unwrap();
        let (now, state, calls) = (rig.now(), scheduler.state(), rig.calls.len());
        // vCPU 1 never arrives: the stop times out with its number, publishes no safe point,
        // withdraws the request and leaves everything where it was.
        let error = scheduler.stop_all(&mut rig, 3).unwrap_err();
        assert_eq!(
            error,
            StopError::Timeout {
                unacked: vec![1],
                rounds: 3
            }
        );
        assert_eq!(rig.settles, [1, 1, 1]);
        assert!(!scheduler.paused());
        assert!(!rig.stop.requested());
        assert_eq!(
            (rig.now(), scheduler.state(), rig.calls.len()),
            (now, state, calls)
        );
        assert_eq!(rig.activity(1), SchedActivity::Halted);
        assert_eq!(rig.activity(2), SchedActivity::WaitForSipi);
        // Two vCPUs stuck are both named; a limit of 0 rounds gives up at once.
        rig.stuck = vec![0, 2];
        assert_eq!(
            scheduler.stop_all(&mut rig, 0).unwrap_err(),
            StopError::Timeout {
                unacked: vec![0, 2],
                rounds: 0
            }
        );
        // A vCPU that the host brings to its safe point in time gives a stop.
        rig.stuck = vec![1];
        rig.settle_arrives = true;
        rig.settles.clear();
        let point = scheduler.stop_all(&mut rig, 3).unwrap();
        assert_eq!((point.rounds, point.acks.len()), (1, 3));
        assert_eq!(rig.settles, [1]);
        assert!(scheduler.paused());
    }

    #[test]
    fn the_state_of_the_scheduler_of_another_number_of_vcpus_is_refused() {
        let rig = Rig::spinning(2);
        let two = rig.scheduler(QuantumPolicy::Fixed(10));
        let mut three = SmpScheduler::new(3, QuantumPolicy::Fixed(10), StopRequest::new()).unwrap();
        let before = three.state();
        assert_eq!(
            three.set_state(two.state()),
            Err(PolicyError("state of another number of vCPUs"))
        );
        assert_eq!(three.state(), before);
    }

    #[test]
    fn the_budget_is_shared_and_cuts_the_last_turn() {
        let mut rig = Rig::spinning(2);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(30), 100);
        assert_eq!(result, Finished::BudgetExhausted);
        let turns: Vec<(usize, u64)> = scheduler.turns().iter().map(|t| (t.cpu, t.steps)).collect();
        assert_eq!(turns, [(0, 30), (1, 30), (0, 30), (1, 10)]);
        assert_eq!(scheduler.retired_total(), 100);
        assert_eq!(rig.now(), 100);
        assert_eq!(
            (
                scheduler.counters()[0].retired,
                scheduler.counters()[1].retired
            ),
            (60, 40)
        );
    }

    #[test]
    fn a_turn_does_not_pass_the_nearest_timer_deadline() {
        let mut rig = Rig::spinning(2);
        rig.arm(1, 5);
        let (scheduler, _) = finished(&mut rig, QuantumPolicy::Fixed(1000), 1000);
        // vCPU 0's turn is cut at the deadline of the timer of vCPU 1 and goes on after the
        // dispatch (one-shot: no further deadline), so the tick time does not depend on the quantum.
        assert_eq!(rig.calls[..2], [(0, 5), (0, 995)]);
        assert_eq!(scheduler.turns()[0].steps, 1000);
        assert!(rig.apic(1).eligible().is_some(), "the timer fired");
    }

    #[test]
    fn device_time_is_charged_after_the_retired_instructions() {
        let mut rig = Rig::racing(1, 3);
        rig.device_ns = 10;
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(4), 1000);
        assert_eq!(result, Finished::AllIdle);
        let calls = rig.calls.len() as u64;
        assert!(calls > 1);
        assert_eq!(rig.now(), scheduler.retired_total() + 10 * calls);
    }

    #[test]
    fn a_board_deadline_cuts_the_first_call_of_a_turn_and_the_clock_stays_the_schedulers() {
        // Without a deadline the whole shared budget is one call.
        let mut rig = Rig::spinning(1);
        finished(&mut rig, QuantumPolicy::Fixed(1000), 120);
        assert_eq!(rig.calls[0], (0, 120));
        // The board source has an edge at 50 ns: the call stops there (D7: one instruction, one
        // ns, so 50 steps), and the owner charges them. Once the clock has reached the deadline
        // and the host has not taken it yet, the turn goes one step at a time.
        let mut rig = Rig::spinning(1);
        rig.board_deadline = Some(50);
        rig.keep_board_deadline = true;
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::Fixed(1000), 120);
        assert_eq!(result, Finished::BudgetExhausted);
        assert_eq!(rig.calls[0], (0, 50));
        assert_eq!(rig.calls[1], (0, 1));
        assert_eq!(rig.now(), 120);
        assert_eq!(scheduler.retired_total(), 120);
    }

    #[test]
    fn an_idle_jump_to_the_board_deadline_is_not_an_error_once_the_host_takes_the_deadline() {
        // Condition (a): the only vCPU is halted with IF = 1 and nothing but the board's source
        // can end its sleep. The clock jumps to the source's edge in one move; the arbiter then
        // answers `NoProgress` (the edge is due and nobody consumed it), which with a skip above 0
        // is not an error. The host takes the edge at the next boundary; nothing can wake the
        // vCPU after that. Rig models the lift; BoardSmpHost lifts a due deadline itself (S6-21 step 1).
        let mut rig = Rig::spinning(1);
        rig.cpus[0].state.activity = Some(CpuActivity::Halted);
        rig.cpus[0].state.rflags |= IF_BIT;
        rig.board_deadline = Some(100);
        let (scheduler, result) = finished(&mut rig, QuantumPolicy::default(), 1000);
        assert_eq!(result, Finished::AllIdle);
        assert_eq!(scheduler.idle(), (100, 1));
        assert_eq!(rig.now(), 100);
        assert!(rig.calls.is_empty(), "nothing ran");
    }

    #[test]
    fn a_board_deadline_the_host_never_takes_is_an_idle_no_progress_error() {
        let mut rig = Rig::spinning(1);
        rig.cpus[0].state.activity = Some(CpuActivity::Halted);
        rig.cpus[0].state.rflags |= IF_BIT;
        rig.board_deadline = Some(100);
        rig.keep_board_deadline = true;
        let mut scheduler = rig.scheduler(QuantumPolicy::default());
        // The first jump reaches the deadline; the second round finds it due and still there.
        assert_eq!(scheduler.run(&mut rig, 1000), Err(SmpError::IdleNoProgress));
        assert_eq!(scheduler.idle(), (100, 1));
    }

    #[test]
    fn the_scheduler_refuses_a_host_with_another_number_of_vcpus() {
        let mut rig = Rig::spinning(2);
        let mut scheduler =
            SmpScheduler::new(3, QuantumPolicy::default(), rig.stop.clone()).unwrap();
        assert_eq!(
            scheduler.run(&mut rig, 10).unwrap_err(),
            SmpError::VcpuCountMismatch {
                scheduler: 3,
                host: 2
            }
        );
    }

    // The board as the host: NMI, INIT and SIPI on the boundary, idle time through the board's owner.

    /// ICR_LOW of INIT assert and SIPI with a vector, physical, no shorthand.
    const INIT_ASSERT: u32 = (5 << 8) | (3 << 14);
    const NMI: u32 = 4 << 8;
    fn sipi(vector: u8) -> u32 {
        (6 << 8) | u32::from(vector)
    }

    fn board(vcpu_count: usize) -> LoadedFirmwareBoard {
        let config = FirmwareBoardConfig {
            vcpu_count,
            ..FirmwareBoardConfig::default()
        };
        let mut board = assemble_board(&config, PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(0x1000, 0x1000)
            .unwrap();
        board.enable_clock_owner().unwrap();
        board
    }

    type Runner = fn(
        &mut LoadedFirmwareBoard,
        usize,
        u64,
        &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError>;

    /// The boot processor sends `icr` words to vCPU 1 on its first call and halts on its second;
    /// no other vCPU can be run.
    fn bsp_sends(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        words: &[u32],
        call: &mut u32,
    ) -> Result<Executed, FirmwareBoardError> {
        assert_eq!(cpu, 0, "only the boot processor can run");
        *call += 1;
        if *call == 1 {
            for word in words {
                board
                    .local_apic_of(0)
                    .unwrap()
                    .write_icr(0, 1, *word)
                    .unwrap();
            }
        } else {
            board.vcpu_mut(0).unwrap().state.activity = Some(CpuActivity::Halted);
        }
        Ok(Executed {
            retired: 1,
            ..Executed::default()
        })
    }

    #[test]
    fn init_and_sipi_are_applied_on_the_boundary_of_the_target_and_the_started_vcpu_runs_in_real_mode()
     {
        let mut board = board(2);
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, _, _: &mut dyn FnMut() -> bool| {
                bsp_sends(board, cpu, &[INIT_ASSERT, sipi(0x08)], &mut call)
            },
        )
        .unwrap();
        // Nothing starts the AP before the boot processor sends the words.
        assert_eq!(host.activity(1), SchedActivity::WaitForSipi);
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        let error = scheduler.run(&mut host, 1000).unwrap_err();
        // INIT and SIPI were applied by the AP's own boundary, INIT first (the AP is started), and
        // then the host ran it in real mode (S6-16c) at the SIPI address: the RAM of the board is
        // zero there, and `00` is no form of the real-mode model, so the model refuses it. A typed
        // error of the vCPU, not a silent skip and not a run as long mode (S6-16c: before it, this
        // error was "a vCPU in real mode cannot run yet").
        let SmpError::Host(FirmwareBoardError::ApLegacyRefused { cpu: 1, refusal }) = error else {
            panic!("{error:?}")
        };
        assert!(matches!(
            refusal,
            crate::ApLegacyRefusal::Real(od_isa::real_mode_exec::RealModeRefusal::Unsupported {
                linear: 0x8000,
                ..
            })
        ));
        let counters = scheduler.counters();
        assert_eq!((counters[1].init_applied, counters[1].started), (1, 1));
        assert_eq!((counters[0].init_applied, counters[0].started), (0, 0));
        assert_eq!(counters[1].retired, 0);
        assert_eq!(
            host.board().vcpu(1).unwrap().activity(),
            CpuActivity::Running
        );
        // The boot processor executed its two calls (send, halt) and nothing else.
        assert_eq!(counters[0].retired, 2);
    }

    #[test]
    fn an_nmi_for_a_vcpu_waiting_for_sipi_stays_pending_and_a_real_mode_target_is_an_error() {
        let mut board = board(2);
        let mut call = 0;
        let mut host = BoardSmpHost::new(
            &mut board,
            |board: &mut LoadedFirmwareBoard, cpu, _, _: &mut dyn FnMut() -> bool| {
                bsp_sends(board, cpu, &[NMI], &mut call)
            },
        )
        .unwrap();
        let mut scheduler =
            SmpScheduler::new(2, QuantumPolicy::default(), StopRequest::new()).unwrap();
        // The AP waits for SIPI: the NMI is held, not an error, and nothing is delivered.
        assert_eq!(scheduler.run(&mut host, 1000).unwrap(), Finished::AllIdle);
        assert_eq!(scheduler.counters()[1].nmi_delivered, 0);
        assert!(host.apic(1).pending_nmi());
        // After SIPI the AP is in real mode, where NMI delivery is not modelled: an error from the
        // boundary of that vCPU, and the NMI stays pending.
        host.apic(0).write_icr(0, 1, sipi(0x08)).unwrap();
        let error = scheduler.run(&mut host, 1000).unwrap_err();
        assert_eq!(
            error,
            SmpError::Host(FirmwareBoardError::Config(
                "deliver_pending_nmi: NMI in real mode not modelled"
            ))
        );
        assert!(host.apic(1).pending_nmi());
        // The SIPI was applied in that same boundary, before the NMI was tried (the counters of a
        // boundary that fails are not updated: the board shows it).
        assert_eq!(
            host.board().vcpu(1).unwrap().activity(),
            CpuActivity::Running
        );
    }

    #[test]
    fn the_board_host_needs_the_clock_owner_and_hands_out_the_boards_own() {
        let config = FirmwareBoardConfig {
            vcpu_count: 2,
            ..FirmwareBoardConfig::default()
        };
        let mut plain = assemble_board(&config, PciEcam::new(), None, None)
            .unwrap()
            .into_linux_board(0x1000, 0x1000)
            .unwrap();
        let none: Runner = |_, _, _, _| unreachable!();
        assert!(matches!(
            BoardSmpHost::new(&mut plain, none),
            Err(FirmwareBoardError::Config(text)) if text.contains("clock owner")
        ));
        // With the owner the host's clock is the board's: the scheduler moves the clock of the
        // board, and nobody else can.
        let mut board = board(2);
        let host = BoardSmpHost::new(&mut board, none).unwrap();
        assert_eq!(host.board().clock().now_ns(), 0);
        let mut host = host;
        host.clock_owner().account_device_ns(5).unwrap();
        assert_eq!(host.board().clock().now_ns(), 5);
    }
}
