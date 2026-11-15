//! Test-only interleaved run of N vCPUs on one virtual clock (S4-23i).
//!
//! Puts the three accepted building blocks into one closed loop, on real `od_isa::execute`
//! and identity RAM: the idle arbiter ([`crate::idle_arbiter`]), the single clock owner and the
//! step budget ([`crate::clock_owner`]). Each iteration of [`Interleaved::run_until`]:
//!
//! 1. stops at the horizon;
//! 2. consumes the board deadlines that are due (and drops them from the list);
//! 3. wakes the halted vCPUs whose vector is eligible (the scheduler's job; the vector is
//!    acknowledged and ended at once, entering the IDT is not modelled);
//! 4. steps the next `Running` vCPU in round-robin order with the budget
//!    [`ClockOwner::step_budget`] gives, then charges what it retired
//!    ([`ClockOwner::account_retired`], one clock move and one timer dispatch per vCPU);
//! 5. when nobody runs, calls [`ClockOwner::idle_step`] once: one clock jump to the earliest
//!    deadline, no waiting and no polling.
//!
//! Time is virtual only (D7: one retired instruction is one nanosecond), so a run is a pure
//! function of its inputs. The module is `cfg(test)`: nothing here is wired to the board, the pump,
//! `firmware_board.rs`, the S6-15 scheduler or `od-isa`.
//!
//! Conditions inherited from the S4-23h review:
//! - a due board deadline is consumed by the caller (here: this loop) before the next
//!   `step_budget`, otherwise every step shrinks to one instruction ([`Interleaved::consume_due`]
//!   switches this off only for the negative control);
//! - `quantum == 0` is not allowed (`debug_assert!` in [`Interleaved::run_until`]); the
//!   scheduler must not use 0 for "do not run";
//! - limitation: the guest never touches the LAPIC inside a step. A guest write to the initial
//!   count or the LVT would create a deadline that a budget computed before the step does not
//!   know about; wiring needs a step end on LAPIC MMIO or a budget recompute. Only the harness
//!   arms timers, between steps, so there is nothing to test here;
//! - test horizons must differ from every timer and board deadline, or the horizon cap alone
//!   would hide a missing cap.

use crate::clock_owner::{ClockOwner, IdleOutcome};
use crate::idle_arbiter::{SchedActivity, WakeView};
use crate::{
    AccessWidth, BusDevice, DeviceAccess, IoSpace, LocalApic, LocalApicHandle, VirtualClockHandle,
};
use od_isa::execute::{RunConfig, RunExit, RunResult, run, run_with_halt_profile};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError, PrivilegeLevel};
use od_isa::{CpuActivity, HaltProfile, LongModeState, OperandSize, Register, StiShadow};

pub(crate) const HZ: u64 = 1_000_000_000;
pub(crate) const VECTOR: u8 = 0x41;
const IF_BIT: u64 = 1 << 9;
/// `mov [rbx], rax`: the filler instruction. The exact HLT profile of `od_isa` admits no filler,
/// so the filler runs under plain `run` and only the `hlt` under `run_with_halt_profile`.
pub(crate) const FILL: [u8; 3] = [0x48, 0x89, 0x03];
pub(crate) const HLT: u8 = 0xf4;
/// `jmp -2`: retires instructions forever and never halts.
pub(crate) const SPIN: [u8; 2] = [0xeb, 0xfe];
pub(crate) const RAM_SIZE: usize = 0x4000;
/// A stop address no program reaches.
pub(crate) const NEVER_RIP: u64 = 0x3f00;
/// Guard against a loop that does not end: far above what any test needs.
const MAX_ITERATIONS: u64 = 100_000;

/// Identity RAM: the linear address is the byte index.
pub(crate) struct Ram(pub(crate) Vec<u8>);

impl Ram {
    fn new() -> Self {
        Self(vec![0; RAM_SIZE])
    }

    pub(crate) fn load(&mut self, at: u64, bytes: &[u8]) {
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

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
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

pub(crate) struct Vcpu {
    pub(crate) state: LongModeState,
    pub(crate) retired: u64,
    /// Address of the `hlt` that ends the current program segment.
    pub(crate) hlt_rip: u64,
    /// `hlt` addresses of the later segments, one per wake-up; [`NEVER_RIP`] after the last.
    pub(crate) later_hlt: Vec<u64>,
}

impl Vcpu {
    /// `rax` is stored at `[rbx]` by every filler instruction.
    pub(crate) fn new(rip: u64, hlt_rip: u64, rbx: u64, rax: u64) -> Self {
        let mut gpr = [0; 16];
        gpr[Register::Rbx as usize] = rbx;
        gpr[Register::Rax as usize] = rax;
        Self {
            hlt_rip,
            later_hlt: Vec::new(),
            state: LongModeState {
                gpr,
                cr3: None,
                cr4: None,
                rip,
                rflags: 0x2 | 1 << 10 | 1 << 0 | IF_BIT,
                sti_shadow: Some(StiShadow::Clear),
                activity: Some(CpuActivity::Running),
                ..LongModeState::default()
            },
            retired: 0,
        }
    }

    fn activity(&self) -> SchedActivity {
        SchedActivity::from(self.state.activity.unwrap())
    }
}

/// One vCPU step as the loop saw it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct StepRecord {
    pub(crate) cpu: usize,
    pub(crate) before: u64,
    pub(crate) budget: u64,
    pub(crate) retired: u64,
    pub(crate) after: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Finished {
    /// The clock reached the horizon.
    Horizon,
    /// Nothing can ever run or wake again.
    AllIdle,
}

pub(crate) struct Interleaved {
    pub(crate) clock: VirtualClockHandle,
    device: LocalApic,
    pub(crate) owner: ClockOwner,
    pub(crate) ram: Ram,
    pub(crate) cpus: Vec<Vcpu>,
    /// Board deadlines not yet consumed (absolute virtual ns).
    pub(crate) board_deadlines: Vec<u64>,
    /// End of the run (absolute virtual ns). Also a board deadline for the budget.
    horizon: u64,
    /// Negative control: when false, due board deadlines are never dropped.
    pub(crate) consume_due: bool,
    pub(crate) trace: Vec<u8>,
    turn: usize,
    pub(crate) iterations: u64,
    pub(crate) idle_calls: u64,
    pub(crate) steps: Vec<StepRecord>,
    /// `(cpu, now)` of every wake-up.
    pub(crate) wakes: Vec<(usize, u64)>,
    /// `(deadline, now)` of every consumed board deadline.
    pub(crate) consumed: Vec<(u64, u64)>,
    /// `now` at the start of each `idle_step` call.
    pub(crate) idle_started_at: Vec<u64>,
}

fn put(trace: &mut Vec<u8>, value: u64) {
    trace.extend_from_slice(&value.to_le_bytes());
}

impl Interleaved {
    /// The clock runs at `HZ` with one LAPIC tick per nanosecond. The LAPICs are enabled here,
    /// before any step.
    pub(crate) fn new(cpus: Vec<Vcpu>, horizon: u64, board_deadlines: Vec<u64>) -> Self {
        let clock = VirtualClockHandle::new(HZ).unwrap();
        let device = LocalApic::with_shared_timers(clock.clone(), HZ, cpus.len()).unwrap();
        let apics = (0..cpus.len())
            .map(|cpu| device.handle_for(cpu).unwrap())
            .collect();
        let owner = ClockOwner::new(&clock, apics).unwrap();
        let mut me = Self {
            clock,
            device,
            owner,
            ram: Ram::new(),
            cpus,
            board_deadlines,
            horizon,
            consume_due: true,
            trace: Vec::new(),
            turn: 0,
            iterations: 0,
            idle_calls: 0,
            steps: Vec::new(),
            wakes: Vec::new(),
            consumed: Vec::new(),
            idle_started_at: Vec::new(),
        };
        for cpu in 0..me.cpus.len() {
            me.write(cpu, 0xf0, 0x1ff);
        }
        me
    }

    fn write(&mut self, cpu: usize, offset: u64, value: u64) {
        let access = DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        };
        self.device.write_from_cpu(cpu, access, value).unwrap();
    }

    /// One-shot LAPIC timer of `cpu`, expiring `count` ticks after the current time. Called by the
    /// harness between steps, never by the guest.
    pub(crate) fn arm(&mut self, cpu: usize, count: u64) {
        self.write(cpu, 0x320, u64::from(VECTOR));
        self.write(cpu, 0x3e0, 0xb);
        self.write(cpu, 0x380, count);
    }

    pub(crate) fn apic(&self, cpu: usize) -> LocalApicHandle {
        self.device.handle_for(cpu).unwrap()
    }

    pub(crate) fn now(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    pub(crate) fn retired_total(&self) -> u64 {
        self.cpus.iter().map(|cpu| cpu.retired).sum()
    }

    fn views(&self) -> Vec<WakeView> {
        self.cpus
            .iter()
            .enumerate()
            .map(|(cpu, vcpu)| WakeView {
                activity: vcpu.activity(),
                if_set: vcpu.state.rflags & IF_BIT != 0,
                apic: self.apic(cpu),
            })
            .collect()
    }

    /// Run `cpu` for at most `budget` instructions (filler under plain `run`, then the `hlt` under
    /// the HLT profile), then charge what it retired in one go.
    fn step_cpu(&mut self, cpu: usize, budget: u64) {
        let before = self.now();
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
        let retired = result.instructions_executed;
        self.cpus[cpu].retired += retired;
        let fired = self.owner.account_retired(retired).unwrap();
        let after = self.now();
        self.steps.push(StepRecord {
            cpu,
            before,
            budget,
            retired,
            after,
        });
        self.trace.push(0xa0 + cpu as u8);
        put(&mut self.trace, budget);
        put(&mut self.trace, retired);
        put(&mut self.trace, self.cpus[cpu].state.rip);
        put(&mut self.trace, after);
        self.trace
            .extend(fired.iter().map(|raised| u8::from(*raised)));
    }

    /// Drop the board deadlines that are due at `now` (condition a of the S4-23h review).
    fn consume_due_deadlines(&mut self, now: u64) {
        if !self.consume_due {
            return;
        }
        let mut kept = Vec::with_capacity(self.board_deadlines.len());
        for deadline in std::mem::take(&mut self.board_deadlines) {
            if deadline <= now {
                self.consumed.push((deadline, now));
                self.trace.push(0xc0);
                put(&mut self.trace, deadline);
                put(&mut self.trace, now);
            } else {
                kept.push(deadline);
            }
        }
        self.board_deadlines = kept;
    }

    /// Set every halted vCPU with IF=1 and an eligible vector to `Running`: it continues after
    /// its `hlt`, with the next segment's stop address. The vector is taken and ended at once.
    fn wake_ready(&mut self) {
        let now = self.now();
        for cpu in 0..self.cpus.len() {
            let ready = self.cpus[cpu].activity() == SchedActivity::Halted
                && self.cpus[cpu].state.rflags & IF_BIT != 0
                && self.apic(cpu).eligible().is_some();
            if !ready {
                continue;
            }
            let vector = self.apic(cpu).acknowledge().expect("eligible vector");
            self.write(cpu, 0xb0, 0);
            let vcpu = &mut self.cpus[cpu];
            vcpu.state.activity = Some(CpuActivity::Running);
            vcpu.hlt_rip = if vcpu.later_hlt.is_empty() {
                NEVER_RIP
            } else {
                vcpu.later_hlt.remove(0)
            };
            self.wakes.push((cpu, now));
            self.trace.push(0xd0 + cpu as u8);
            self.trace.push(vector);
            put(&mut self.trace, now);
        }
    }

    /// Next `Running` vCPU at or after the round-robin pointer.
    fn pick_running(&mut self) -> Option<usize> {
        let n = self.cpus.len();
        let cpu = (0..n)
            .map(|offset| (self.turn + offset) % n)
            .find(|&cpu| self.cpus[cpu].activity() == SchedActivity::Running)?;
        self.turn = (cpu + 1) % n;
        Some(cpu)
    }

    /// Board deadline for `step_budget`: the earliest unconsumed one, or the horizon.
    fn budget_deadline(&self) -> u64 {
        self.board_deadlines
            .iter()
            .copied()
            .chain([self.horizon])
            .min()
            .unwrap()
    }

    /// Run until the horizon or until nothing can run or wake. `quantum` is the largest step in
    /// instructions and must be positive.
    pub(crate) fn run_until(&mut self, quantum: u64) -> Finished {
        debug_assert!(
            quantum > 0,
            "quantum must be positive: 0 does not mean `do not run`"
        );
        loop {
            self.iterations += 1;
            assert!(
                self.iterations <= MAX_ITERATIONS,
                "the loop does not end: polling or no progress"
            );
            let now = self.now();
            if now >= self.horizon {
                return Finished::Horizon;
            }
            self.consume_due_deadlines(now);
            self.wake_ready();
            if let Some(cpu) = self.pick_running() {
                let budget = self
                    .owner
                    .step_budget(quantum, Some(self.budget_deadline()));
                self.step_cpu(cpu, budget);
                continue;
            }
            // Nobody runs: one idle decision with at most one clock jump.
            self.idle_calls += 1;
            self.idle_started_at.push(now);
            let views = self.views();
            let deadlines: Vec<u64> = self
                .board_deadlines
                .iter()
                .copied()
                .chain([self.horizon])
                .collect();
            let step = self.owner.idle_step(false, &views, &deadlines).unwrap();
            let (tag, payload) = match step.outcome {
                IdleOutcome::Stopped => (1, 0),
                IdleOutcome::Runnable(idx) => (2, idx as u64),
                IdleOutcome::Woken(idx) => (3, idx as u64),
                IdleOutcome::AllIdle => (4, 0),
                IdleOutcome::NoProgress => (5, 0),
                IdleOutcome::Advanced(next) => (6, next),
            };
            self.trace.push(0xb0 + tag);
            put(&mut self.trace, payload);
            put(&mut self.trace, step.idle_ns);
            let now_after = self.now();
            put(&mut self.trace, now_after);
            self.trace
                .extend(step.fired.iter().map(|raised| u8::from(*raised)));
            match step.outcome {
                IdleOutcome::AllIdle => return Finished::AllIdle,
                IdleOutcome::Stopped | IdleOutcome::Runnable(_) => {
                    unreachable!("no stop request, and nobody runs")
                }
                // `Woken` and `Advanced` are handled by the next iteration (the wake pass).
                IdleOutcome::Woken(_) | IdleOutcome::Advanced(_) => {}
                // A jump to a board deadline or the horizon: the next iteration consumes it or
                // ends. No jump at all means a stale deadline was passed in.
                IdleOutcome::NoProgress => {
                    assert!(step.idle_ns > 0, "a stale board deadline reached idle_step");
                }
            }
        }
    }

    /// The byte trace plus the final observable state of the whole machine.
    pub(crate) fn finish(mut self) -> Vec<u8> {
        let now = self.now();
        put(&mut self.trace, now);
        put(&mut self.trace, self.iterations);
        put(&mut self.trace, self.idle_calls);
        for cpu in 0..self.cpus.len() {
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

#[cfg(test)]
mod tests {
    use super::*;

    const CODE0: u64 = 0x1000;
    const CODE1: u64 = 0x2000;
    const QUANTUM: u64 = 8;

    /// vCPU0: two fillers, `hlt`, one filler, `hlt` (3 retired, then 2 after a wake-up).
    /// vCPU1: `fills` fillers and `hlt`, or an endless spin.
    fn scenario(
        timer_at: u64,
        horizon: u64,
        board: &[u64],
        vcpu1_fills: Option<usize>,
    ) -> Interleaved {
        let mut code0 = FILL.repeat(2);
        let first_hlt = CODE0 + code0.len() as u64;
        code0.push(HLT);
        code0.extend_from_slice(&FILL);
        let second_hlt = CODE0 + code0.len() as u64;
        code0.push(HLT);
        let mut v0 = Vcpu::new(CODE0, first_hlt, 0x3000, 0x11);
        v0.later_hlt = vec![second_hlt];
        let (code1, hlt1) = match vcpu1_fills {
            Some(fills) => {
                let mut code = FILL.repeat(fills);
                let hlt = CODE1 + code.len() as u64;
                code.push(HLT);
                (code, hlt)
            }
            None => (SPIN.to_vec(), NEVER_RIP),
        };
        let v1 = Vcpu::new(CODE1, hlt1, 0x3008, 0x22);
        let mut h = Interleaved::new(vec![v0, v1], horizon, board.to_vec());
        h.ram.load(CODE0, &code0);
        h.ram.load(CODE1, &code1);
        h.arm(0, timer_at);
        assert_eq!(h.apic(0).next_timer_deadline(), Some(timer_at));
        h
    }

    #[test]
    fn hlt_wake_by_timer_without_polling() {
        const FILLS: usize = 20;
        // The idle length differs by three orders of magnitude, the horizon is always another
        // value than the timer and the board deadline.
        let mut idle_calls = Vec::new();
        for (timer_at, horizon) in [(700, 1_060), (700_000, 700_060)] {
            let mut h = scenario(timer_at, horizon, &[850], Some(FILLS));
            assert_ne!(horizon, timer_at);
            assert_ne!(horizon, 850);
            assert_eq!(h.run_until(QUANTUM), Finished::Horizon);

            // Both vCPUs halted at once: the clock is exactly the retired count (3 + FILLS + 1).
            let first_idle = h.idle_started_at[0];
            assert_eq!(first_idle, 3 + FILLS as u64 + 1);
            // The wake-up lands exactly on the timer, and only vCPU0 wakes.
            assert_eq!(h.wakes, [(0, timer_at)]);
            assert_eq!(h.apic(1).vector_state(VECTOR), (false, false));
            assert_eq!(h.apic(1).pending(), None);
            assert_eq!(h.cpus[1].state.activity, Some(CpuActivity::Halted));
            // vCPU0 went on after its HLT and stored again, then halted a second time.
            assert_eq!(h.cpus[0].retired, 3 + 2);
            assert_eq!(h.cpus[0].state.activity, Some(CpuActivity::Halted));
            assert_eq!(h.cpus[0].state.rip, CODE0 + 3 * 3 + 2);
            // The board deadline was consumed on the dot, and the run ended on the horizon.
            assert_eq!(h.consumed, [(850, 850)]);
            assert!(h.board_deadlines.is_empty());
            assert_eq!(h.now(), horizon);
            // The idle time is skipped, not retired: the clock is far past the retired count.
            assert_eq!(h.retired_total(), 3 + FILLS as u64 + 1 + 2);
            assert!(h.now() > 10 * h.retired_total());
            idle_calls.push((h.idle_calls, h.iterations, h.steps.len()));
            assert_eq!(h.ram.0[0x3000], 0x11);
            assert_eq!(h.ram.0[0x3008], 0x22);
        }
        // No polling: the number of idle decisions (jump to T, jump to the board deadline, jump to
        // the horizon) and of loop iterations does not depend on how long the vCPUs sleep.
        assert_eq!(idle_calls[0], idle_calls[1]);
        assert_eq!(idle_calls[0].0, 3);
        assert!(idle_calls[0].1 < 20, "{idle_calls:?}");
    }

    /// vCPU0 halts early; vCPU1 spins through the timer (700), a board deadline (850) and ends at
    /// the horizon (1060).
    fn spinning(consume_due: bool) -> Interleaved {
        let mut h = scenario(700, 1_060, &[850], None);
        h.consume_due = consume_due;
        assert_eq!(h.run_until(QUANTUM), Finished::Horizon);
        h
    }

    #[test]
    fn board_deadline_is_consumed_and_does_not_shrink_steps_forever() {
        let h = spinning(true);
        // The timer fired while vCPU1 was running: vCPU0 woke exactly at 700, not at a quantum
        // boundary past it.
        assert_eq!(h.wakes, [(0, 700)]);
        // The board deadline is reached exactly (a step ends on it) and consumed at that time.
        assert_eq!(h.consumed, [(850, 850)]);
        assert!(h.steps.iter().any(|s| s.after == 850));
        assert!(h.board_deadlines.is_empty());
        assert_eq!(h.now(), 1_060);
        // After the consumption the step budget is the plain quantum, cut only by the horizon:
        // no degradation to one instruction per step.
        let after: Vec<&StepRecord> = h.steps.iter().filter(|s| s.before >= 850).collect();
        assert!(!after.is_empty());
        for step in &after {
            assert_eq!(step.budget, QUANTUM.min(1_060 - step.before), "{step:?}");
            assert_eq!(step.retired, step.budget);
        }
        assert!(after.iter().filter(|s| s.budget == 1).count() <= 1);
        assert!(after.len() < 30, "{}", after.len());
        // No step crossed the board deadline or the timer on the way.
        assert!(h.steps.iter().all(|s| !(s.before < 850 && s.after > 850)));
        assert!(h.steps.iter().all(|s| !(s.before < 700 && s.after > 700)));

        // Negative control: a caller that keeps the due deadline gets one-instruction steps for
        // the rest of the run, so the check above is sensitive to the consumption.
        let stuck = spinning(false);
        assert!(stuck.consumed.is_empty());
        let tail: Vec<&StepRecord> = stuck.steps.iter().filter(|s| s.before >= 850).collect();
        assert_eq!(tail.len(), 1_060 - 850);
        assert!(tail.iter().all(|s| s.budget == 1));
    }

    #[test]
    fn trace_is_byte_identical_for_same_inputs() {
        let run = |timer_at: u64, board: &[u64]| {
            let mut h = scenario(timer_at, 1_060, board, Some(20));
            assert_eq!(h.run_until(QUANTUM), Finished::Horizon);
            h.finish()
        };
        let first = run(700, &[850]);
        let second = run(700, &[850]);
        assert!(first.len() > RAM_SIZE);
        assert_eq!(first, second);
        // The trace depends on the inputs: another timer or another board deadline changes it.
        assert_ne!(first, run(701, &[850]));
        assert_ne!(first, run(700, &[851]));
        assert_ne!(first, run(700, &[]));
        // The same holds for the spinning scenario with steps cut by deadlines.
        let spin = |timer_at: u64| {
            let mut h = scenario(timer_at, 1_060, &[850], None);
            assert_eq!(h.run_until(QUANTUM), Finished::Horizon);
            h.finish()
        };
        assert_eq!(spin(700), spin(700));
        assert_ne!(spin(700), spin(703));
    }

    /// Decision: a zero quantum is a caller bug caught by `debug_assert!` in `run_until` (it
    /// panics in debug builds, which is what the tests use). `step_budget` itself stays total and
    /// returns 1 for it (S4-23h), so a release build cannot loop without progress either.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "quantum must be positive")]
    fn quantum_zero_is_rejected_in_debug() {
        let mut h = scenario(700, 1_060, &[850], None);
        h.run_until(0);
    }
}
