//! Single owner of the board clock for N vCPUs (S4-23g).
//!
//! The owner claims the clock's one advancer capability for good and is therefore the only thing
//! that can move virtual time. Time moves in exactly three ways, all decision D7 of S6 (retired
//! and idle, plus the device ticks of S4-37/38 that S4-23j adds):
//!
//! - retirement: every retired guest instruction costs [`NS_PER_RETIRED`] virtual nanoseconds
//!   ([`ClockOwner::account_retired`]);
//! - device ticks: the PM timer, PIT and RDTSC ticks of the board loop are charged as plain
//!   nanoseconds ([`ClockOwner::account_device_ns`]);
//! - idle skip: when every vCPU is halted, the clock jumps straight to the earliest useful
//!   deadline ([`ClockOwner::idle_step`]); no host time is read or waited for.
//!
//! The guest therefore sees virtual time only, and a run is a pure function of its inputs.
//!
//! Wake-up from `HLT` by a deadline is one decision per call: [`ClockOwner::idle_step`] asks
//! [`crate::idle_arbiter::decide`], carries out an `AdvanceTo` with
//! [`crate::idle_arbiter::apply`] (one clock move, one timer dispatch per vCPU in index order) and
//! decides once more. There is no loop and no waiting inside, so polling is impossible by
//! construction. [`IdleOutcome::Woken`] only means that the vCPU may be set to `Running`: its
//! vector is in the IRR and eligible. Entering the IDT is not done here.
//!
//! Wiring (S4-23j): the Linux board loop (`LoadedFirmwareBoard::run_linux_boot_cpl0_set`) takes
//! the owner on request only (`enable_clock_owner`, the `clock_owner` member of the Linux direct
//! run); by default the board keeps advancing its clock itself and the owner is never created.
//!
//! Limits (not part of this slice): not wired to the pump or a scheduler; no quanta, seed or
//! schedule (S6-15); no NMI/INIT/SIPI or host wait. The tick sources keep their S4-37/38 cost
//! model (the Linux-visible frequency is S4-38). The caller tracks
//! vCPU activity itself and passes it in the views; `state.activity` is not consulted. A timer
//! dispatch raises at most one request per call, as `LocalApicHandle::dispatch_timer` does: a
//! periodic timer folds missed periods into one interrupt and keeps its phase, a one-shot fires at
//! the step boundary. A caller must therefore cap a vCPU's step budget at
//! `(next_deadline - now) / NS_PER_RETIRED`, or the tick count depends on the quantum size; that
//! rule is [`ClockOwner::step_budget`] (S4-23h), used by the opt-in board loop (S4-23j) and the
//! test harness.

// The board loop uses the owner opt-in; items only the tests use stay allowed.
#![cfg_attr(not(test), allow(dead_code))]

use crate::idle_arbiter::{ApplyError, IdleDecision, WakeView, apply, decide};
use crate::virtual_clock::VirtualClockAdvancer;
use crate::{DeviceError, LocalApicHandle, VirtualClockError, VirtualClockHandle};

/// Virtual nanoseconds charged per retired instruction (D7).
pub(crate) const NS_PER_RETIRED: u64 = 1;

/// Instruction budget of one vCPU step: the scheduler quantum, cut so that the step cannot retire
/// past the earliest deadline (S4-23h). Pure; `rate` is the nanoseconds charged per retired
/// instruction.
///
/// - no deadline: `quantum`;
/// - deadline after `now`: `min(quantum, (deadline - now) / rate)`, so the step ends at or before
///   the deadline and the timer dispatch that follows it sees the deadline at its own time;
/// - deadline not after `now` (stale or already due): 1. The step is never empty, so a caller
///   cannot loop without progress, and it is one retirement only, so a stale deadline does not
///   open a long overshoot;
/// - the result is never 0 (also for `quantum == 0`, and when the gap is below `rate`).
fn budget_with_rate(quantum: u64, now: u64, deadline: Option<u64>, rate: u64) -> u64 {
    let budget = match deadline {
        None => quantum,
        Some(deadline) => quantum.min(deadline.saturating_sub(now) / rate),
    };
    budget.max(1)
}

/// [`budget_with_rate`] at the D7 rate [`NS_PER_RETIRED`].
pub(crate) fn capped_budget(quantum: u64, now: u64, deadline: Option<u64>) -> u64 {
    budget_with_rate(quantum, now, deadline, NS_PER_RETIRED)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ClockOwnerError {
    /// The clock refused to advance (time or TSC overflow, or somebody else owns it). Neither the
    /// clock nor any timer queue changed.
    Clock(VirtualClockError),
    /// The timer dispatch of vCPU `cpu` failed; the clock has already advanced and the vCPUs
    /// before `cpu` have been dispatched.
    Dispatch { cpu: usize, error: DeviceError },
}

/// What one [`ClockOwner::idle_step`] decided.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdleOutcome {
    /// STOP was requested; nothing changed.
    Stopped,
    /// vCPU `idx` is running, so nothing is idle and nothing was done.
    Runnable(usize),
    /// vCPU `idx` is halted with an eligible vector and IF=1: the caller may set it `Running`.
    Woken(usize),
    /// Nothing can ever wake a vCPU. The clock stays where it is.
    AllIdle,
    /// The earliest deadline is not after `now`: advancing cannot make progress. Tell the two
    /// causes apart by `IdleStep::idle_ns`: 0 means the clock did not move (a stale deadline was
    /// passed in); above 0 means this very call jumped to a board deadline and the repeated
    /// decision still sees it, so the caller must consume it and stop passing it.
    NoProgress,
    /// The clock jumped and nobody can be woken yet; the next useful deadline is the payload.
    /// The caller calls `idle_step` again.
    Advanced(u64),
}

impl From<IdleDecision> for IdleOutcome {
    /// `AdvanceTo` maps to `Advanced`: it is what a decision taken after a jump looks like.
    fn from(decision: IdleDecision) -> Self {
        match decision {
            IdleDecision::Stopped => Self::Stopped,
            IdleDecision::Runnable(idx) => Self::Runnable(idx),
            IdleDecision::WakeNow(idx) => Self::Woken(idx),
            IdleDecision::AllIdle => Self::AllIdle,
            IdleDecision::NoProgress => Self::NoProgress,
            IdleDecision::AdvanceTo(next) => Self::Advanced(next),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IdleStep {
    pub(crate) outcome: IdleOutcome,
    /// Virtual nanoseconds skipped by this call (0 when the clock did not move).
    pub(crate) idle_ns: u64,
    /// Per vCPU, whether the timer dispatch of the jump raised a request. Empty when no jump.
    pub(crate) fired: Vec<bool>,
}

impl IdleStep {
    fn still(outcome: IdleOutcome) -> Self {
        Self {
            outcome,
            idle_ns: 0,
            fired: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct ClockOwner {
    advancer: VirtualClockAdvancer,
    apics: Vec<LocalApicHandle>,
}

impl ClockOwner {
    /// Claim the clock for good and take the LAPICs of the vCPUs in index order. Fails with
    /// `ClockAlreadyOwned` when the clock already has an owner.
    pub(crate) fn new(
        clock: &VirtualClockHandle,
        apics: Vec<LocalApicHandle>,
    ) -> Result<Self, VirtualClockError> {
        Ok(Self {
            advancer: clock.claim_advancer()?,
            apics,
        })
    }

    pub(crate) fn now_ns(&self) -> u64 {
        self.advancer.snapshot().now_ns()
    }

    /// Earliest virtual-time deadline of the owner's LAPIC timers, merged with one optional
    /// deadline of the board (for example the end of a run). Read-only.
    pub(crate) fn nearest_deadline(&self, board_deadline: Option<u64>) -> Option<u64> {
        self.apics
            .iter()
            .filter_map(LocalApicHandle::next_timer_deadline)
            .chain(board_deadline)
            .min()
    }

    /// Instruction budget for the next vCPU step under the quantum, bounded by the nearest
    /// deadline (see [`capped_budget`]). Never 0. A step of this size ends no later than the
    /// deadline, so [`Self::account_retired`] dispatches the timer at exactly its deadline and the
    /// tick count and times do not depend on `quantum`.
    pub(crate) fn step_budget(&self, quantum: u64, board_deadline: Option<u64>) -> u64 {
        capped_budget(
            quantum,
            self.now_ns(),
            self.nearest_deadline(board_deadline),
        )
    }

    /// Charge `retired` instructions to the clock, then dispatch every LAPIC timer in index order
    /// (the dispatch precedes any idle decision, as the arbiter's contract requires). Returns, per
    /// vCPU, whether its dispatch raised a request. If the clock refuses (overflow) nothing
    /// changes, queues included.
    pub(crate) fn account_retired(&mut self, retired: u64) -> Result<Vec<bool>, ClockOwnerError> {
        let delta = retired
            .checked_mul(NS_PER_RETIRED)
            .ok_or(ClockOwnerError::Clock(VirtualClockError::TimeOverflow))?;
        self.advancer
            .advance_time_ns(delta)
            .map_err(ClockOwnerError::Clock)?;
        self.dispatch_all()
    }

    /// Charge `ns` virtual nanoseconds of device time (the PM timer, PIT and RDTSC ticks of the
    /// board loop, S4-23j) to the clock, then dispatch every LAPIC timer in index order, exactly as
    /// [`Self::account_retired`] does, but without the per-instruction rate: the caller already
    /// computed the nanoseconds. If the clock refuses (overflow) nothing changes, queues included.
    /// `ns == 0` moves nothing and dispatches whatever is due at `now` (idempotent).
    pub(crate) fn account_device_ns(&mut self, ns: u64) -> Result<Vec<bool>, ClockOwnerError> {
        self.advancer
            .advance_time_ns(ns)
            .map_err(ClockOwnerError::Clock)?;
        self.dispatch_all()
    }

    /// One timer dispatch per vCPU of the owner's list, in index order.
    fn dispatch_all(&self) -> Result<Vec<bool>, ClockOwnerError> {
        let mut fired = Vec::with_capacity(self.apics.len());
        for (cpu, apic) in self.apics.iter().enumerate() {
            let raised = if apic.has_timer() {
                apic.dispatch_timer()
                    .map_err(|error| ClockOwnerError::Dispatch { cpu, error })?
            } else {
                false
            };
            fired.push(raised);
        }
        Ok(fired)
    }

    /// One idle decision, with at most one clock jump. `views` are in vCPU index order with the
    /// activity the caller tracks; `board_deadlines` follow the contract of
    /// [`crate::idle_arbiter::decide`]. The caller has dispatched due timers beforehand (as
    /// [`Self::account_retired`] does after every charge).
    ///
    /// Contract of `views`: it holds the LAPICs of the same vCPUs in the same order as the owner's
    /// list (`views.len() == self.apics.len()`, checked in debug builds). The jump's timer dispatch
    /// runs over `views[i].apic` (through `apply`), whereas `account_retired` runs over the owner's
    /// list; the two must therefore name the same LAPICs. `&mut self` keeps the clock read and its
    /// advance in one exclusive borrow of the single owner.
    pub(crate) fn idle_step(
        &mut self,
        stop: bool,
        views: &[WakeView],
        board_deadlines: &[u64],
    ) -> Result<IdleStep, ClockOwnerError> {
        let now = self.now_ns();
        debug_assert_eq!(
            views.len(),
            self.apics.len(),
            "views must cover the owner's LAPICs, one per vCPU"
        );
        let to_ns = match decide(now, stop, views, board_deadlines) {
            IdleDecision::AdvanceTo(to_ns) => to_ns,
            other => return Ok(IdleStep::still(other.into())),
        };
        let fired =
            apply(now, to_ns, |d| self.advancer.advance_time_ns(d), views).map_err(|error| {
                match error {
                    ApplyError::Advance(error) => ClockOwnerError::Clock(error),
                    ApplyError::Dispatch { cpu, error } => ClockOwnerError::Dispatch { cpu, error },
                    // `decide` only returns `AdvanceTo` for a deadline after `now`.
                    ApplyError::NoProgress => unreachable!("AdvanceTo is always after now"),
                }
            })?;
        let outcome = decide(self.now_ns(), stop, views, board_deadlines).into();
        Ok(IdleStep {
            outcome,
            idle_ns: to_ns - now,
            fired,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::idle_arbiter::SchedActivity;
    use crate::{AccessWidth, BusDevice, DeviceAccess, IoSpace, LocalApic};
    use od_isa::execute::{RunConfig, RunExit, RunResult, run, run_with_halt_profile};
    use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError, PrivilegeLevel};
    use od_isa::{CpuActivity, HaltProfile, LongModeState, OperandSize, Register, StiShadow};

    const HZ: u64 = 1_000_000_000;
    const IF_BIT: u64 = 1 << 9;
    const VECTOR: u8 = 0x41;
    const CODE0: u64 = 0x1000;
    const CODE1: u64 = 0x2000;
    /// `mov [rbx], rax`: the filler instruction. The exact HLT profile of `od_isa` admits no
    /// filler (at most `sti`/`cli` and `hlt`), so the filler runs under plain `run` and only the
    /// `hlt` runs under `run_with_halt_profile`.
    const FILL: [u8; 3] = [0x48, 0x89, 0x03];
    const HLT: u8 = 0xf4;
    const RAM_SIZE: usize = 0x4000;

    /// Identity RAM: the linear address is the byte index.
    struct Ram(Vec<u8>);

    impl Ram {
        fn new() -> Self {
            Self(vec![0; RAM_SIZE])
        }

        fn load(&mut self, at: u64, bytes: &[u8]) {
            let at = at as usize;
            self.0[at..at + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl GuestMemory for Ram {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }

        fn read(
            &mut self,
            address: u64,
            _: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            let at = address as usize;
            let bytes = self
                .0
                .get(at..at + destination.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
            destination.copy_from_slice(bytes);
            Ok(())
        }

        fn write(
            &mut self,
            address: u64,
            _: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            let at = address as usize;
            let bytes = self
                .0
                .get_mut(at..at + source.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Write,
                })?;
            bytes.copy_from_slice(source);
            Ok(())
        }

        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            unreachable!()
        }
    }

    struct Vcpu {
        state: LongModeState,
        retired: u64,
        /// Address of the `hlt` that ends the program.
        hlt_rip: u64,
    }

    impl Vcpu {
        /// `rax` is stored at `[rbx]` by every filler instruction.
        fn new(rip: u64, if_set: bool, rbx: u64, rax: u64) -> Self {
            let mut gpr = [0; 16];
            gpr[Register::Rbx as usize] = rbx;
            gpr[Register::Rax as usize] = rax;
            Self {
                hlt_rip: rip,
                state: LongModeState {
                    gpr,
                    cr3: None,
                    cr4: None,
                    rip,
                    rflags: 0x2 | 1 << 10 | 1 << 0 | if if_set { IF_BIT } else { 0 },
                    sti_shadow: Some(StiShadow::Clear),
                    activity: Some(CpuActivity::Running),
                    ..LongModeState::default()
                },
                retired: 0,
            }
        }

        fn view(&self, apic: LocalApicHandle) -> WakeView {
            WakeView {
                activity: SchedActivity::from(self.state.activity.unwrap()),
                if_set: self.state.rflags & IF_BIT != 0,
                apic,
            }
        }
    }

    fn put(trace: &mut Vec<u8>, value: u64) {
        trace.extend_from_slice(&value.to_le_bytes());
    }

    /// Two vCPUs on one board clock, driven by real `od_isa` execution on identity RAM. The
    /// harness arms the timers and keeps a byte trace of everything observable.
    struct Harness {
        clock: VirtualClockHandle,
        device: LocalApic,
        owner: ClockOwner,
        ram: Ram,
        cpus: [Vcpu; 2],
        trace: Vec<u8>,
    }

    impl Harness {
        fn new(tsc_hz: u64, apic_hz: u64) -> Self {
            let clock = VirtualClockHandle::new(tsc_hz).unwrap();
            let device = LocalApic::with_shared_timers(clock.clone(), apic_hz, 2).unwrap();
            let apics = (0..2).map(|cpu| device.handle_for(cpu).unwrap()).collect();
            let owner = ClockOwner::new(&clock, apics).unwrap();
            let mut harness = Self {
                clock,
                device,
                owner,
                ram: Ram::new(),
                cpus: [
                    Vcpu::new(CODE0, true, 0x3000, 0x11),
                    Vcpu::new(CODE1, true, 0x3008, 0x22),
                ],
                trace: Vec::new(),
            };
            for cpu in 0..2 {
                harness.write(cpu, 0xf0, 0x1ff);
            }
            harness
        }

        fn write(&mut self, cpu: usize, offset: u64, value: u64) {
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: AccessWidth::Dword,
            };
            self.device.write_from_cpu(cpu, access, value).unwrap();
        }

        /// One-shot LAPIC timer of `cpu`, expiring `count` ticks after the current time.
        fn arm(&mut self, cpu: usize, count: u64) {
            self.write(cpu, 0x320, u64::from(VECTOR));
            self.write(cpu, 0x3e0, 0xb);
            self.write(cpu, 0x380, count);
        }

        fn apic(&self, cpu: usize) -> LocalApicHandle {
            self.device.handle_for(cpu).unwrap()
        }

        fn now(&self) -> u64 {
            self.clock.snapshot().now_ns()
        }

        fn retired_total(&self) -> u64 {
            self.cpus.iter().map(|cpu| cpu.retired).sum()
        }

        fn views(&self) -> Vec<WakeView> {
            (0..2)
                .map(|cpu| self.cpus[cpu].view(self.apic(cpu)))
                .collect()
        }

        /// Run `cpu` for at most `budget` instructions (filler under plain `run`, then the `hlt`
        /// under the HLT profile), then charge what it retired in one go.
        fn run(&mut self, cpu: usize, budget: u64) -> RunResult {
            let context = AccessContext {
                privilege: PrivilegeLevel::Ring0,
                ..AccessContext::default()
            };
            let hlt_rip = self.cpus[cpu].hlt_rip;
            let state = &mut self.cpus[cpu].state;
            let mut result = RunResult {
                exit: RunExit::StopAddress,
                instructions_executed: 0,
                rflags_defined: 0,
            };
            if state.rip != hlt_rip {
                result = run(
                    state,
                    &mut self.ram,
                    RunConfig {
                        instruction_budget: budget,
                        stop_rip: Some(hlt_rip),
                        context,
                    },
                    || false,
                );
            }
            let left = budget - result.instructions_executed;
            if result.exit == RunExit::StopAddress && left > 0 {
                let retired = result.instructions_executed;
                result = run_with_halt_profile(
                    state,
                    &mut self.ram,
                    RunConfig {
                        instruction_budget: left,
                        stop_rip: None,
                        context,
                    },
                    &HaltProfile::Long64V1,
                    || false,
                );
                result.instructions_executed += retired;
            }
            self.cpus[cpu].retired += result.instructions_executed;
            let fired = self.owner.account_retired(result.instructions_executed);
            self.trace.push(0xa0 + cpu as u8);
            put(&mut self.trace, result.instructions_executed);
            put(&mut self.trace, self.cpus[cpu].state.rip);
            let now = self.now();
            put(&mut self.trace, now);
            self.trace
                .extend(fired.unwrap().iter().map(|raised| u8::from(*raised)));
            result
        }

        fn idle(&mut self, stop: bool, board_deadlines: &[u64]) -> IdleStep {
            let views = self.views();
            let step = self.owner.idle_step(stop, &views, board_deadlines).unwrap();
            let (tag, idx) = match step.outcome {
                IdleOutcome::Stopped => (1, 0),
                IdleOutcome::Runnable(idx) => (2, idx),
                IdleOutcome::Woken(idx) => (3, idx),
                IdleOutcome::AllIdle => (4, 0),
                IdleOutcome::NoProgress => (5, 0),
                IdleOutcome::Advanced(next) => (6, next as usize),
            };
            self.trace.push(0xb0 + tag);
            put(&mut self.trace, idx as u64);
            put(&mut self.trace, step.idle_ns);
            let now = self.now();
            put(&mut self.trace, now);
            self.trace
                .extend(step.fired.iter().map(|raised| u8::from(*raised)));
            step
        }

        /// The byte trace plus the final observable state of the whole machine.
        fn finish(mut self) -> Vec<u8> {
            let now = self.now();
            put(&mut self.trace, now);
            for cpu in 0..2 {
                let vcpu = &self.cpus[cpu];
                put(&mut self.trace, vcpu.retired);
                put(&mut self.trace, vcpu.state.rip);
                put(&mut self.trace, vcpu.state.rflags);
                self.trace
                    .push(u8::from(vcpu.state.activity == Some(CpuActivity::Halted)));
                let apic = self.apic(cpu);
                self.trace.push(u8::from(apic.vector_state(VECTOR).0));
                self.trace.push(u8::from(apic.pending().is_some()));
                put(
                    &mut self.trace,
                    apic.next_timer_deadline().unwrap_or(u64::MAX),
                );
            }
            self.trace.extend_from_slice(&self.ram.0);
            self.trace
        }
    }

    /// vCPU0: two fillers and `hlt` (3 retired). vCPU1: `fills` fillers and `hlt` (`fills + 1`).
    fn load_programs(h: &mut Harness, fills: usize) {
        let mut code0 = FILL.repeat(2);
        h.cpus[0].hlt_rip = CODE0 + code0.len() as u64;
        code0.push(HLT);
        h.ram.load(CODE0, &code0);
        let mut code1 = FILL.repeat(fills);
        h.cpus[1].hlt_rip = CODE1 + code1.len() as u64;
        code1.push(HLT);
        h.ram.load(CODE1, &code1);
    }

    /// Scenario (a): both vCPUs halt, only vCPU0 has a timer, at absolute time `deadline`.
    fn halted_pair(deadline: u64, fills: usize) -> Harness {
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, fills);
        h.arm(0, deadline);
        assert_eq!(h.apic(0).next_timer_deadline(), Some(deadline));
        let r0 = h.run(0, 100);
        assert_eq!(r0.exit, RunExit::Halted);
        assert_eq!(r0.instructions_executed, 3);
        let r1 = h.run(1, 100);
        assert_eq!(r1.exit, RunExit::Halted);
        assert_eq!(r1.instructions_executed, fills as u64 + 1);
        h
    }

    #[test]
    fn two_vcpus_one_halted_timer_clock_jumps_exactly_to_deadline_and_wakes_only_it() {
        const T: u64 = 1_000;
        const K: usize = 5;
        let mut h = halted_pair(T, K);
        // Retirement only: one virtual nanosecond per retired instruction, TSC in step.
        assert_eq!(h.retired_total(), 3 + K as u64 + 1);
        assert_eq!(h.now(), h.retired_total());
        assert_eq!(h.clock.snapshot().tsc(), h.now());
        assert_eq!(h.cpus[0].state.activity, Some(CpuActivity::Halted));
        assert_eq!(h.cpus[1].state.activity, Some(CpuActivity::Halted));
        // The deadline has not fired and nothing is pending anywhere before the idle step.
        assert_eq!(h.apic(0).next_timer_deadline(), Some(T));
        assert_eq!(h.apic(0).pending(), None);

        let before = h.now();
        let step = h.idle(false, &[]);
        assert_eq!(step.outcome, IdleOutcome::Woken(0));
        assert_eq!(step.idle_ns, T - before);
        assert_eq!(step.fired, [true, false]);
        // The jump landed exactly on the deadline, not past it.
        assert_eq!(h.now(), T);
        assert_eq!(h.clock.snapshot().tsc(), T);
        assert_eq!(h.apic(0).vector_state(VECTOR), (true, false));
        assert_eq!(h.apic(0).eligible(), Some(VECTOR));
        assert_eq!(h.apic(0).next_timer_deadline(), None);
        // vCPU1 stays halted, with an empty IRR and ISR and no timer.
        assert_eq!(h.cpus[1].state.activity, Some(CpuActivity::Halted));
        assert_eq!(h.apic(1).vector_state(VECTOR), (false, false));
        assert_eq!(h.apic(1).pending(), None);
        assert_eq!(h.apic(1).next_timer_deadline(), None);
        // Both programs ran to just past their HLT.
        assert_eq!(h.cpus[0].state.rip, CODE0 + 2 * 3 + 1);
        assert_eq!(h.cpus[1].state.rip, CODE1 + 3 * K as u64 + 1);
        // Their stores landed byte for byte: `rax` as a little-endian qword at `[rbx]`.
        assert_eq!(h.ram.0[0x3000..0x3010], {
            let mut want = [0u8; 16];
            want[0] = 0x11;
            want[8] = 0x22;
            want
        });

        // A second call is a pure observation: the same wake, no time, nothing fired.
        let again = h.idle(false, &[]);
        assert_eq!(again.outcome, IdleOutcome::Woken(0));
        assert_eq!(again.idle_ns, 0);
        assert!(again.fired.is_empty());
        assert_eq!(h.now(), T);
    }

    #[test]
    fn deadline_crossed_while_the_other_vcpu_runs_wakes_at_the_boundary_without_idle_skip() {
        const T: u64 = 12;
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, 15);
        h.arm(0, T);
        assert_eq!(h.run(0, 100).exit, RunExit::Halted);
        assert_eq!(h.now(), 3);
        // vCPU1 runs in chunks of 4: the clock reads 7, 11, 15 and 19 at the boundaries.
        for boundary in [7, 11] {
            let result = h.run(1, 4);
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert_eq!(h.now(), boundary);
            assert_eq!(h.apic(0).pending(), None, "boundary {boundary}");
            assert_eq!(h.apic(0).next_timer_deadline(), Some(T));
        }
        // Boundary 15 is the first one after the deadline 12: the vector is raised there, and the
        // clock is at the boundary, not at the deadline.
        assert_eq!(h.run(1, 4).exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(h.now(), 15);
        assert_eq!(h.apic(0).vector_state(VECTOR), (true, false));
        assert_eq!(h.apic(0).next_timer_deadline(), None);
        assert_eq!(h.apic(1).pending(), None);
        // vCPU1 still runs: nothing is idle, the arbiter does nothing and the clock stays.
        let step = h.idle(false, &[]);
        assert_eq!(step.outcome, IdleOutcome::Runnable(1));
        assert_eq!(step.idle_ns, 0);
        assert_eq!(h.now(), 15);
        // vCPU1 reaches its HLT: 15 fillers and the HLT are 16 instructions in all.
        assert_eq!(h.run(1, 4).exit, RunExit::Halted);
        assert_eq!(h.now(), 3 + 16);
        let step = h.idle(false, &[]);
        assert_eq!(step.outcome, IdleOutcome::Woken(0));
        assert_eq!(step.idle_ns, 0);
        assert!(step.fired.is_empty());
        // No idle skip happened: the clock is exactly the retired instruction count.
        assert_eq!(h.now(), h.retired_total());
    }

    #[test]
    fn same_inputs_give_byte_identical_trace() {
        let run = |deadline: u64, fills: usize| {
            let mut h = halted_pair(deadline, fills);
            let step = h.idle(false, &[]);
            assert_eq!(step.outcome, IdleOutcome::Woken(0));
            h.finish()
        };
        let first = run(1_000, 5);
        let second = run(1_000, 5);
        assert!(first.len() > RAM_SIZE);
        assert_eq!(first, second);
        // The trace really depends on the inputs: another deadline moves the clock elsewhere,
        // and another program length changes retirement.
        let other_deadline = run(1_001, 5);
        assert_ne!(first, other_deadline);
        assert_eq!(first.len(), other_deadline.len());
        let differing: Vec<usize> = (0..first.len())
            .filter(|&at| first[at] != other_deadline[at])
            .collect();
        assert!(!differing.is_empty());
        assert_ne!(first, run(1_000, 6));
        // Both runs end with the same RAM bytes (only the clock and its readers differ).
        assert_eq!(
            first[first.len() - RAM_SIZE..],
            other_deadline[other_deadline.len() - RAM_SIZE..]
        );
    }

    #[test]
    fn the_owner_is_the_only_advancer() {
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, 2);
        h.arm(0, 10);
        assert_eq!(h.run(0, 100).exit, RunExit::Halted);
        let now = h.now();
        // The claim is permanent: nobody else can claim or advance this clock.
        assert_eq!(
            h.clock.claim_advancer().err(),
            Some(VirtualClockError::ClockAlreadyOwned)
        );
        assert_eq!(
            ClockOwner::new(&h.clock, vec![h.apic(0)]).err(),
            Some(VirtualClockError::ClockAlreadyOwned)
        );
        assert_eq!(
            h.clock.advance_standalone(5),
            Err(VirtualClockError::ClockAlreadyOwned)
        );
        assert_eq!(h.now(), now);

        // Overflow in `account_retired` leaves the clock and the queues alone.
        assert_eq!(
            h.owner.account_retired(u64::MAX),
            Err(ClockOwnerError::Clock(VirtualClockError::TimeOverflow))
        );
        assert_eq!(h.now(), now);
        assert_eq!(h.apic(0).next_timer_deadline(), Some(10));
        assert_eq!(h.apic(0).pending(), None);
        // The failed charge consumed nothing: a normal charge still works and fires on time.
        assert_eq!(h.owner.account_retired(7).unwrap(), [true, false]);
        assert_eq!(h.now(), now + 7);
        assert_eq!(h.apic(0).vector_state(VECTOR), (true, false));
    }

    #[test]
    fn a_refused_jump_in_idle_step_changes_neither_clock_nor_queues() {
        // 1000x TSC: the TSC overflows at about 1.8e16 ns. One LAPIC tick is 1 ms.
        let mut h = Harness::new(1_000 * HZ, 1_000);
        const NEAR_LIMIT: u64 = 18_000_000_000_000_000;
        assert_eq!(h.owner.account_retired(NEAR_LIMIT).unwrap(), [false, false]);
        h.arm(0, 4_000_000_000);
        let deadline = h.apic(0).next_timer_deadline().unwrap();
        assert!(
            deadline > u64::MAX / 1_000,
            "the jump must overflow the TSC"
        );
        h.cpus[0].state.activity = Some(CpuActivity::Halted);
        h.cpus[1].state.activity = Some(CpuActivity::Halted);
        let tsc = h.clock.snapshot().tsc();
        let views = h.views();
        let result = h.owner.idle_step(false, &views, &[]);
        assert_eq!(
            result,
            Err(ClockOwnerError::Clock(VirtualClockError::TscOverflow))
        );
        assert_eq!(h.now(), NEAR_LIMIT);
        assert_eq!(h.clock.snapshot().tsc(), tsc);
        assert_eq!(h.apic(0).next_timer_deadline(), Some(deadline));
        assert_eq!(h.apic(0).vector_state(VECTOR), (false, false));
        assert_eq!(h.apic(0).pending(), None);
    }

    #[test]
    fn device_ns_move_the_clock_without_a_rate_and_dispatch_the_timers() {
        // S4-23j: the plain nanoseconds of the board ticks (PM, PIT, RDTSC).
        let mut h = Harness::new(HZ, HZ);
        h.arm(0, 500);
        // Not due yet: the clock moves by exactly `ns`, nothing fires, zero is harmless.
        assert_eq!(h.owner.account_device_ns(499).unwrap(), [false, false]);
        assert_eq!(h.owner.account_device_ns(0).unwrap(), [false, false]);
        assert_eq!(h.now(), 499);
        // The step that reaches the deadline dispatches it at once, as `account_retired` does.
        assert_eq!(h.owner.account_device_ns(1).unwrap(), [true, false]);
        assert_eq!(h.now(), 500);
        assert_eq!(h.apic(0).vector_state(VECTOR), (true, false));
        // An overflow changes neither the clock nor a queue.
        let mut h = Harness::new(1_000 * HZ, 1_000);
        const NEAR_LIMIT: u64 = 18_000_000_000_000_000;
        h.owner.account_device_ns(NEAR_LIMIT).unwrap();
        h.arm(0, 4_000_000_000);
        let (deadline, tsc) = (h.apic(0).next_timer_deadline(), h.clock.snapshot().tsc());
        assert_eq!(
            h.owner.account_device_ns(4_000_000_000_000_000),
            Err(ClockOwnerError::Clock(VirtualClockError::TscOverflow))
        );
        assert_eq!(h.now(), NEAR_LIMIT);
        assert_eq!(h.clock.snapshot().tsc(), tsc);
        assert_eq!(h.apic(0).next_timer_deadline(), deadline);
    }

    #[test]
    fn no_deadline_or_stop_ends_in_one_call() {
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, 2);
        assert_eq!(h.run(0, 100).exit, RunExit::Halted);
        assert_eq!(h.run(1, 100).exit, RunExit::Halted);
        let now = h.now();
        assert_eq!(now, 3 + 3);
        // No timer anywhere: nothing can ever wake a vCPU, and the answer is immediate.
        for _ in 0..3 {
            let step = h.idle(false, &[]);
            assert_eq!(step.outcome, IdleOutcome::AllIdle);
            assert_eq!(step.idle_ns, 0);
            assert!(step.fired.is_empty());
            assert_eq!(h.now(), now);
        }
        // STOP beats an armed timer: the clock stays and the timer stays armed.
        h.arm(0, 500);
        let step = h.idle(true, &[]);
        assert_eq!(step.outcome, IdleOutcome::Stopped);
        assert_eq!(step.idle_ns, 0);
        assert!(step.fired.is_empty());
        assert_eq!(h.now(), now);
        assert_eq!(h.apic(0).next_timer_deadline(), Some(now + 500));
        assert_eq!(h.apic(0).pending(), None);
        // Without STOP the same state jumps in that very one call.
        let step = h.idle(false, &[]);
        assert_eq!(step.outcome, IdleOutcome::Woken(0));
        assert_eq!(step.idle_ns, 500);
        assert_eq!(h.now(), now + 500);
    }

    #[test]
    fn a_board_deadline_jump_reports_no_progress_until_the_caller_consumes_it() {
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, 2);
        assert_eq!(h.run(0, 100).exit, RunExit::Halted);
        assert_eq!(h.run(1, 100).exit, RunExit::Halted);
        let now = h.now();
        let step = h.idle(false, &[now + 50]);
        // The clock jumped to the board deadline and nobody has a vector to take.
        assert_eq!(step.idle_ns, 50);
        assert_eq!(step.fired, [false, false]);
        assert_eq!(step.outcome, IdleOutcome::NoProgress);
        assert_eq!(h.now(), now + 50);
        // Once the caller drops the consumed deadline the answer is AllIdle and time stays.
        let step = h.idle(false, &[]);
        assert_eq!(step.outcome, IdleOutcome::AllIdle);
        assert_eq!(h.now(), now + 50);
    }

    #[test]
    fn a_later_timer_of_another_vcpu_needs_a_second_call_not_a_loop() {
        let mut h = Harness::new(HZ, HZ);
        load_programs(&mut h, 2);
        assert_eq!(h.run(0, 100).exit, RunExit::Halted);
        assert_eq!(h.run(1, 100).exit, RunExit::Halted);
        let now = h.now();
        // vCPU0's vector is blocked by the TPR, so waking it needs nothing; vCPU1's later timer is
        // the only useful deadline after the first jump.
        h.write(0, 0x80, 0xf0);
        h.arm(0, 100);
        h.arm(1, 300);
        // First call: jump to 100, vCPU0's vector lands in the IRR but the TPR blocks it.
        let first = h.idle(false, &[]);
        assert_eq!(first.outcome, IdleOutcome::Advanced(now + 300));
        assert_eq!(first.idle_ns, 100);
        assert_eq!(first.fired, [true, false]);
        assert_eq!(h.now(), now + 100);
        assert_eq!(h.apic(1).pending(), None);
        // Second call: the next jump wakes vCPU1.
        let second = h.idle(false, &[]);
        assert_eq!(second.outcome, IdleOutcome::Woken(1));
        assert_eq!(second.idle_ns, 200);
        assert_eq!(second.fired, [false, true]);
        assert_eq!(h.now(), now + 300);
    }

    // S4-23h: the step budget is bounded by the nearest deadline.

    /// `jmp -2`: a vCPU that retires instructions forever and never halts.
    const SPIN: [u8; 2] = [0xeb, 0xfe];
    const PERIODIC_LVT: u64 = 1 << 17;
    /// Far from both programs: the spinning vCPUs never reach it.
    const NEVER_RIP: u64 = 0x3f00;

    fn spin_harness() -> Harness {
        let mut h = Harness::new(HZ, HZ);
        h.ram.load(CODE0, &SPIN);
        h.ram.load(CODE1, &SPIN);
        h.cpus[0].hlt_rip = NEVER_RIP;
        h.cpus[1].hlt_rip = NEVER_RIP;
        h
    }

    /// Periodic LAPIC timer of `cpu` with a period of `period` ticks (1 tick = 1 ns here).
    fn arm_periodic(h: &mut Harness, cpu: usize, period: u64) {
        h.write(cpu, 0x320, PERIODIC_LVT | u64::from(VECTOR));
        h.write(cpu, 0x3e0, 0xb);
        h.write(cpu, 0x380, period);
    }

    /// Consume the interrupt request of `cpu` if there is one (acknowledge and end of interrupt).
    fn take_fire(h: &mut Harness, cpu: usize) -> bool {
        if h.apic(cpu).acknowledge().is_some() {
            h.write(cpu, 0xb0, 0);
            true
        } else {
            false
        }
    }

    struct Step {
        before: u64,
        deadline: Option<u64>,
        budget: u64,
        after: u64,
    }

    struct Observed {
        /// `(cpu, now)` of every timer request, in the order they were raised.
        fires: Vec<(usize, u64)>,
        steps: Vec<Step>,
        final_now: u64,
    }

    /// Two spinning vCPUs, periodic timers of 100 ns (vCPU0) and 70 ns (vCPU1), stepped in turn
    /// until `horizon`. With `capped` the budget follows the owner's rule (the horizon is a board
    /// deadline); without it every step is the plain quantum, as a caller ignoring the rule does.
    fn drive(quantum: u64, capped: bool, horizon: u64) -> Observed {
        let mut h = spin_harness();
        arm_periodic(&mut h, 0, 100);
        arm_periodic(&mut h, 1, 70);
        let mut observed = Observed {
            fires: Vec::new(),
            steps: Vec::new(),
            final_now: 0,
        };
        let mut turn = 0;
        while h.now() < horizon {
            let before = h.now();
            let deadline = h.owner.nearest_deadline(Some(horizon));
            let budget = if capped {
                h.owner.step_budget(quantum, Some(horizon))
            } else {
                quantum
            };
            assert!(budget >= 1);
            let result = h.run(turn % 2, budget);
            assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            assert_eq!(result.instructions_executed, budget);
            turn += 1;
            let after = h.now();
            for cpu in 0..2 {
                if take_fire(&mut h, cpu) {
                    observed.fires.push((cpu, after));
                }
            }
            observed.steps.push(Step {
                before,
                deadline,
                budget,
                after,
            });
        }
        observed.final_now = h.now();
        observed
    }

    fn expected_fires(horizon: u64) -> Vec<(usize, u64)> {
        let mut want: Vec<(usize, u64)> = (1..=horizon / 100)
            .map(|k| (0, k * 100))
            .chain((1..=horizon / 70).map(|k| (1, k * 70)))
            .collect();
        want.sort_by_key(|&(cpu, now)| (now, cpu));
        want
    }

    #[test]
    fn tick_count_does_not_depend_on_quantum() {
        const HORIZON: u64 = 1_050;
        let want = expected_fires(HORIZON);
        assert_eq!(want.len(), 10 + 15);
        for quantum in [1, 7, 1_000] {
            let got = drive(quantum, true, HORIZON);
            assert_eq!(got.final_now, HORIZON, "quantum {quantum}");
            assert_eq!(got.fires, want, "quantum {quantum}");
        }
        // Without the rule the same program ticks differently: the rule is what fixes the trace.
        let overshoot = drive(1_000, false, HORIZON);
        assert_ne!(overshoot.fires, want);
        let shifted = drive(7, false, HORIZON);
        assert_ne!(shifted.fires, want);
        assert_ne!(shifted.fires, overshoot.fires);
    }

    #[test]
    fn budget_never_crosses_a_deadline() {
        const HORIZON: u64 = 1_050;
        for quantum in [1, 7, 64, 1_000] {
            let got = drive(quantum, true, HORIZON);
            let mut cut = 0;
            for step in &got.steps {
                // The owner dispatches after every charge, so a live deadline is always ahead.
                let deadline = step.deadline.expect("timers are armed");
                assert!(deadline > step.before, "quantum {quantum}");
                assert!(step.budget <= quantum);
                assert!(
                    step.after <= deadline,
                    "quantum {quantum}: step {}..{} crossed {deadline}",
                    step.before,
                    step.after
                );
                if step.budget < quantum {
                    cut += 1;
                }
            }
            if quantum > 1 {
                assert!(cut > 0, "quantum {quantum}: the rule never cut a step");
            }
        }
    }

    #[test]
    fn no_deadline_budget_is_quantum() {
        let mut h = spin_harness();
        for quantum in [1, 7, 1_000, u64::MAX] {
            assert_eq!(h.owner.nearest_deadline(None), None);
            assert_eq!(h.owner.step_budget(quantum, None), quantum);
        }
        // Retirement alone does not create a deadline.
        assert_eq!(h.run(0, 25).instructions_executed, 25);
        assert_eq!(h.owner.step_budget(1_000, None), 1_000);
        assert_eq!(capped_budget(1_000, 25, None), 1_000);
        // A masked timer has no useful deadline either.
        arm_periodic(&mut h, 0, 100);
        h.write(0, 0x320, PERIODIC_LVT | (1 << 16) | u64::from(VECTOR));
        assert_eq!(h.apic(0).next_timer_deadline(), None);
        assert_eq!(h.owner.step_budget(1_000, None), 1_000);
        // A timer or a board deadline bounds it again, whichever is nearer.
        h.write(0, 0x320, PERIODIC_LVT | u64::from(VECTOR));
        assert_eq!(h.apic(0).next_timer_deadline(), Some(125));
        assert_eq!(h.owner.step_budget(1_000, None), 100);
        assert_eq!(h.owner.step_budget(1_000, Some(40)), 15);
        assert_eq!(h.owner.step_budget(1_000, Some(500)), 100);
        assert_eq!(h.owner.step_budget(50, None), 50);
    }

    #[test]
    fn budget_is_never_zero() {
        // Stale or due deadlines and a zero quantum give one retirement, not an empty step.
        assert_eq!(capped_budget(100, 500, Some(500)), 1);
        assert_eq!(capped_budget(100, 500, Some(100)), 1);
        assert_eq!(capped_budget(100, 500, Some(0)), 1);
        assert_eq!(capped_budget(100, 500, Some(501)), 1);
        assert_eq!(capped_budget(0, 500, None), 1);
        assert_eq!(capped_budget(0, 0, Some(9)), 1);
        assert_eq!(capped_budget(u64::MAX, 0, Some(u64::MAX)), u64::MAX);
        // A gap below one retirement's cost rounds down to 0 and is lifted to 1.
        assert_eq!(budget_with_rate(100, 0, Some(3), 4), 1);
        assert_eq!(budget_with_rate(100, 0, Some(8), 4), 2);
        assert_eq!(budget_with_rate(100, 0, Some(11), 4), 2);
        // On the owner, with a real clock past the deadline.
        let mut h = spin_harness();
        assert_eq!(h.run(0, 10).instructions_executed, 10);
        assert_eq!(h.now(), 10);
        for deadline in [0, 9, 10, 11] {
            assert_eq!(h.owner.step_budget(100, Some(deadline)), 1, "{deadline}");
        }
        assert_eq!(h.owner.step_budget(0, None), 1);
        // The stale step retires one instruction and then moves on.
        assert_eq!(h.run(0, 1).instructions_executed, 1);
        assert_eq!(h.now(), 11);
    }
}
