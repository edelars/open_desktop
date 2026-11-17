//! Pure deterministic idle arbiter for N vCPUs (S4-23f).
//!
//! When every vCPU is halted, somebody has to decide whether anything can wake one of them and
//! to which virtual time the board clock may jump. This module is that decision as a pure
//! function of the observed state, with no host time, no hash-ordered iteration and no
//! scheduler. It is a building block that no production path calls yet (the board, the pump and
//! the virtual clock are untouched); decision D7 (how the board uses it) is not taken here.
//!
//! Checks run in a fixed order: STOP, then any running vCPU, then a halted vCPU that can already
//! take an interrupt, then the earliest useful deadline. Ties between deadlines are resolved by
//! vCPU index (the lowest index is picked and timer dispatch runs in index order).
//!
//! The per-vCPU timer rule repeats `InterruptPump::useful_timer_deadline`, which the pump applies
//! in the maskable half of `InterruptPump::idle_advance`: a timer deadline of a halted vCPU counts
//! only when IF=1 and its vector is not already waiting in the IRR behind an in-service vector or
//! the TPR (ISSUE-070). The pump's IRQ queue deadlines are not part of that rule here.
//!
//! Contract of `board_deadlines`: the arbiter waits for every value in it regardless of IF and of
//! the IRR, like the pump's owner (host) deadlines. The pump treats its queue deadlines as
//! maskable (waited for only with IF=1 and only when the vector is not already in the IRR), so
//! the caller must pass only deadlines whose target vCPU is `Halted` with IF=1 and whose vector is
//! not in the IRR of that LAPIC, or deadlines of non-maskable sources (owner/host work). The
//! arbiter does not check this.
//!
//! Deviations and limits:
//! - `od_isa::CpuActivity::WaitForSipi` (S6-13) maps to [`SchedActivity::WaitForSipi`]; the arbiter
//!   treats it as a separate branch that is never picked (as is `Shutdown`). NMI, INIT and SIPI
//!   wake-ups are deferred: the board applies a pending INIT/SIPI itself, not the arbiter.
//! - The caller dispatches due timers before deciding, as the pump does at the start of a step.
//!   A deadline that is not after `now` therefore means no progress is possible: `NoProgress`.
//! - Board deadlines are considered only while at least one vCPU is halted; with nobody to wake
//!   the arbiter reports `AllIdle` instead of burning virtual time.
//! - Queue (IRQ route) deadlines are not part of the views; the caller passes them in
//!   `board_deadlines` after the filtering described in the contract above.

// No production caller yet: the arbiter is wired in by a later slice; tests exercise it now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::{DeviceError, LocalApicHandle};

/// Machine-level activity of one vCPU as the arbiter sees it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SchedActivity {
    Running,
    Halted,
    /// Waiting for a startup IPI (`CpuActivity::WaitForSipi`); never picked.
    WaitForSipi,
    Shutdown,
}

impl From<od_isa::CpuActivity> for SchedActivity {
    fn from(activity: od_isa::CpuActivity) -> Self {
        match activity {
            od_isa::CpuActivity::Running => Self::Running,
            od_isa::CpuActivity::Halted => Self::Halted,
            od_isa::CpuActivity::Shutdown => Self::Shutdown,
            od_isa::CpuActivity::WaitForSipi => Self::WaitForSipi,
        }
    }
}

/// What the arbiter needs to know about one vCPU, in vCPU index order.
#[derive(Clone, Debug)]
pub(crate) struct WakeView {
    pub(crate) activity: SchedActivity,
    /// RFLAGS.IF of the vCPU.
    pub(crate) if_set: bool,
    pub(crate) apic: LocalApicHandle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IdleDecision {
    /// STOP was requested; nothing changes, in particular not the time.
    Stopped,
    /// vCPU `idx` is running (lowest index); no idle handling is needed.
    Runnable(usize),
    /// vCPU `idx` is halted and can take a pending interrupt right now (IF=1, eligible vector).
    WakeNow(usize),
    /// Nothing can run now; the clock may advance to this absolute virtual time (ns).
    AdvanceTo(u64),
    /// Nothing can ever wake a vCPU: no useful deadline. The clock stays where it is.
    AllIdle,
    /// The earliest deadline is not after `now`, so advancing cannot make progress.
    NoProgress,
}

/// Timer deadline that can still wake the vCPU (the pump's `useful_timer_deadline`): not when its
/// vector already waits in the IRR behind an in-service vector or the TPR.
fn useful_timer_deadline(apic: &LocalApicHandle) -> Option<u64> {
    if !apic.has_timer() {
        return None;
    }
    let deadline = apic.next_timer_deadline()?;
    let blocked = apic
        .timer_vector()
        .is_some_and(|vector| apic.vector_state(vector).0)
        && apic.eligible().is_none();
    (!blocked).then_some(deadline)
}

/// Decide what the board does when its vCPUs may be idle. Reads state only.
///
/// `Runnable(idx)` means only "not idle": some vCPU is running. Delivering an interrupt to
/// sleeping vCPUs meanwhile is the scheduler's separate job, not the arbiter's.
pub(crate) fn decide(
    now: u64,
    stop: bool,
    views: &[WakeView],
    board_deadlines: &[u64],
) -> IdleDecision {
    if stop {
        return IdleDecision::Stopped;
    }
    if let Some(idx) = views
        .iter()
        .position(|view| view.activity == SchedActivity::Running)
    {
        return IdleDecision::Runnable(idx);
    }
    let halted = |view: &&WakeView| view.activity == SchedActivity::Halted;
    if let Some(idx) = views
        .iter()
        .position(|view| halted(&view) && view.if_set && view.apic.eligible().is_some())
    {
        return IdleDecision::WakeNow(idx);
    }
    if !views.iter().any(|view| halted(&view)) {
        return IdleDecision::AllIdle;
    }
    let timers = views
        .iter()
        .filter(|view| halted(view) && view.if_set)
        .filter_map(|view| useful_timer_deadline(&view.apic));
    match timers.chain(board_deadlines.iter().copied()).min() {
        None => IdleDecision::AllIdle,
        Some(deadline) if deadline <= now => IdleDecision::NoProgress,
        Some(deadline) => IdleDecision::AdvanceTo(deadline),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ApplyError<E> {
    /// `to_ns` is not after `now`; nothing was changed.
    NoProgress,
    /// The clock refused to advance; no clock or queue state changed.
    Advance(E),
    /// The timer dispatch of vCPU `cpu` failed; the clock has already advanced and the timers of
    /// the vCPUs before `cpu` have been dispatched.
    Dispatch { cpu: usize, error: DeviceError },
}

/// Carry out an `AdvanceTo(to_ns)` decision: `advance(delta)` moves the clock exactly to `to_ns`
/// (delta = `to_ns - now`); if it fails nothing else is touched. Then the timer of every vCPU
/// with one is dispatched in index order, exactly once. Returns, per vCPU, whether its dispatch
/// raised a request.
///
/// `now` must equal the caller's clock: `apply` trusts it and only computes the delta from it.
/// Time is shared, so every vCPU with a timer gets its dispatch, whatever its activity (a halted
/// vCPU with IF=0, `Shutdown` or `WaitForSipi`); a running vCPU cannot occur after `AdvanceTo`.
/// If the dispatch of vCPU `cpu` fails, the clock stays advanced, the vCPUs before it keep their
/// dispatch and the vCPUs after it are not dispatched (see [`ApplyError::Dispatch`]).
pub(crate) fn apply<E>(
    now: u64,
    to_ns: u64,
    advance: impl FnOnce(u64) -> Result<(), E>,
    views: &[WakeView],
) -> Result<Vec<bool>, ApplyError<E>> {
    if to_ns <= now {
        return Err(ApplyError::NoProgress);
    }
    advance(to_ns - now).map_err(ApplyError::Advance)?;
    let mut fired = Vec::with_capacity(views.len());
    for (cpu, view) in views.iter().enumerate() {
        let raised = if view.apic.has_timer() {
            view.apic
                .dispatch_timer()
                .map_err(|error| ApplyError::Dispatch { cpu, error })?
        } else {
            false
        };
        fired.push(raised);
    }
    Ok(fired)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interrupt_pump::{
        BackendEnv, BackendFlow, BackendStep, InterruptPump, PumpBackend, PumpExit, PumpRejection,
    };
    use crate::{
        AccessWidth, BusDevice, DeviceAccess, IoSpace, LocalApic, VirtualClockError,
        VirtualClockHandle,
    };

    const IF_SET: bool = true;
    const IF_CLEAR: bool = false;
    const HZ: u64 = 1_000_000_000;

    struct Rig {
        clock: VirtualClockHandle,
        device: LocalApic,
    }

    impl Rig {
        fn new(count: usize) -> Self {
            Self::with_tsc(count, HZ)
        }

        fn with_tsc(count: usize, tsc_hz: u64) -> Self {
            let clock = VirtualClockHandle::new(tsc_hz).unwrap();
            let device = LocalApic::with_shared_timers(clock.clone(), HZ, count).unwrap();
            let mut rig = Self { clock, device };
            for cpu in 0..count {
                rig.write(cpu, 0xf0, 0x1ff);
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

        /// One-shot timer of `cpu` with `vector`, expiring `ns` after the current time.
        fn arm(&mut self, cpu: usize, vector: u64, ns: u64) {
            self.write(cpu, 0x320, vector);
            self.write(cpu, 0x3e0, 0xb);
            self.write(cpu, 0x380, ns);
        }

        fn apic(&self, cpu: usize) -> LocalApicHandle {
            self.device.handle_for(cpu).unwrap()
        }

        fn now(&self) -> u64 {
            self.clock.snapshot().now_ns()
        }

        fn view(&self, cpu: usize, activity: SchedActivity, if_set: bool) -> WakeView {
            WakeView {
                activity,
                if_set,
                apic: self.apic(cpu),
            }
        }

        fn halted(&self, count: usize, if_set: bool) -> Vec<WakeView> {
            (0..count)
                .map(|cpu| self.view(cpu, SchedActivity::Halted, if_set))
                .collect()
        }

        fn advance(&self) -> impl FnOnce(u64) -> Result<(), VirtualClockError> + '_ {
            |delta| self.clock.advance_standalone(delta)
        }
    }

    fn state(rig: &Rig, count: usize, vector: u8) -> Vec<(Option<u64>, (bool, bool))> {
        (0..count)
            .map(|cpu| {
                let apic = rig.apic(cpu);
                (apic.next_timer_deadline(), apic.vector_state(vector))
            })
            .collect()
    }

    #[test]
    fn the_timer_of_one_halted_vcpu_sets_the_deadline_and_wakes_only_it() {
        let mut rig = Rig::new(2);
        rig.arm(1, 0x41, 700);
        let views = rig.halted(2, IF_SET);
        let decision = decide(rig.now(), false, &views, &[]);
        assert_eq!(decision, IdleDecision::AdvanceTo(700));
        let fired = apply(0, 700, rig.advance(), &views).unwrap();
        assert_eq!(fired, [false, true]);
        assert_eq!(rig.now(), 700);
        assert_eq!(rig.apic(0).vector_state(0x41), (false, false));
        assert_eq!(rig.apic(1).vector_state(0x41), (true, false));
        // The vCPU that was dispatched now has a vector it can take.
        assert_eq!(
            decide(rig.now(), false, &views, &[]),
            IdleDecision::WakeNow(1)
        );
    }

    #[test]
    fn equal_deadlines_dispatch_each_vcpu_exactly_once_in_index_order() {
        let mut rig = Rig::new(3);
        rig.arm(2, 0x42, 500);
        rig.arm(0, 0x40, 500);
        rig.arm(1, 0x41, 500);
        let views = rig.halted(3, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AdvanceTo(500));
        let mut order = Vec::new();
        let fired = apply(
            0,
            500,
            |delta| {
                order.push(delta);
                rig.clock.advance_standalone(delta)
            },
            &views,
        )
        .unwrap();
        assert_eq!(fired, [true, true, true]);
        // The clock moved once, exactly to the deadline; every one-shot fired once.
        assert_eq!(order, [500]);
        assert_eq!(rig.now(), 500);
        for cpu in 0..3 {
            assert_eq!(rig.apic(cpu).next_timer_deadline(), None, "cpu {cpu}");
            assert_eq!(rig.apic(cpu).dispatch_timer(), Ok(false), "cpu {cpu}");
        }
        // Equal-deadline ties resolve by index: with the vectors pending, vCPU 0 wins.
        assert_eq!(
            decide(rig.now(), false, &views, &[]),
            IdleDecision::WakeNow(0)
        );
    }

    #[test]
    fn a_vcpu_with_if_clear_does_not_wait_for_its_timer() {
        let mut rig = Rig::new(2);
        rig.arm(0, 0x40, 300);
        rig.arm(1, 0x41, 900);
        let views = [
            rig.view(0, SchedActivity::Halted, IF_CLEAR),
            rig.view(1, SchedActivity::Halted, IF_SET),
        ];
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AdvanceTo(900));
        let only_clear = [
            rig.view(0, SchedActivity::Halted, IF_CLEAR),
            rig.view(1, SchedActivity::Halted, IF_CLEAR),
        ];
        assert_eq!(decide(0, false, &only_clear, &[]), IdleDecision::AllIdle);
        // A pending vector does not wake a vCPU with IF=0 either.
        rig.apic(0).request_fixed_edge(0x60).unwrap();
        assert_eq!(decide(0, false, &only_clear, &[]), IdleDecision::AllIdle);
        // Board deadlines are waited for regardless of IF.
        assert_eq!(
            decide(0, false, &only_clear, &[1_000]),
            IdleDecision::AdvanceTo(1_000)
        );
    }

    #[test]
    fn a_vector_blocked_by_isr_or_tpr_is_not_waited_for() {
        // ISSUE-070: the timer vector sits in the IRR and cannot become eligible.
        let mut rig = Rig::new(2);
        rig.arm(0, 0x40, 300);
        rig.write(0, 0x80, 0xf0); // TPR above the vector class
        rig.apic(0).request_fixed_edge(0x40).unwrap();
        assert_eq!(rig.apic(0).eligible(), None);
        assert_eq!(rig.apic(0).vector_state(0x40), (true, false));
        let views = rig.halted(2, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AllIdle);
        // The unblocked timer of the other vCPU is still waited for.
        rig.arm(1, 0x41, 800);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AdvanceTo(800));
        // Once the vector is no longer blocked it is eligible and wakes the vCPU at once.
        rig.write(0, 0x80, 0);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::WakeNow(0));
    }

    #[test]
    fn a_vector_blocked_by_an_in_service_vector_is_not_waited_for() {
        let mut rig = Rig::new(1);
        let apic = rig.apic(0);
        apic.request_fixed_edge(0x50).unwrap();
        assert_eq!(apic.acknowledge(), Some(0x50)); // in service, no EOI
        rig.arm(0, 0x45, 400);
        apic.request_fixed_edge(0x45).unwrap();
        assert_eq!(apic.eligible(), None);
        let views = rig.halted(1, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AllIdle);
    }

    #[test]
    fn wait_for_sipi_and_shutdown_are_never_picked() {
        let mut rig = Rig::new(3);
        rig.arm(0, 0x40, 100);
        rig.arm(1, 0x41, 200);
        rig.apic(0).request_fixed_edge(0x60).unwrap();
        let skipped = [
            rig.view(0, SchedActivity::WaitForSipi, IF_SET),
            rig.view(1, SchedActivity::Shutdown, IF_SET),
        ];
        // Neither a deadline nor a pending vector of such a vCPU counts; no board deadline either.
        assert_eq!(decide(0, false, &skipped, &[]), IdleDecision::AllIdle);
        assert_eq!(decide(0, false, &skipped, &[50]), IdleDecision::AllIdle);
        // With a halted vCPU present only its own deadline decides.
        let mixed = [
            skipped[0].clone(),
            skipped[1].clone(),
            rig.view(2, SchedActivity::Halted, IF_SET),
        ];
        assert_eq!(decide(0, false, &mixed, &[]), IdleDecision::AllIdle);
        rig.arm(2, 0x42, 900);
        assert_eq!(decide(0, false, &mixed, &[]), IdleDecision::AdvanceTo(900));
        assert_eq!(
            SchedActivity::from(od_isa::CpuActivity::Running),
            SchedActivity::Running
        );
        assert_eq!(
            SchedActivity::from(od_isa::CpuActivity::Halted),
            SchedActivity::Halted
        );
        assert_eq!(
            SchedActivity::from(od_isa::CpuActivity::Shutdown),
            SchedActivity::Shutdown
        );
        // S6-13: the CPU model's wait-for-SIPI activity maps to the same never-picked branch.
        assert_eq!(
            SchedActivity::from(od_isa::CpuActivity::WaitForSipi),
            SchedActivity::WaitForSipi
        );
    }

    #[test]
    fn a_running_vcpu_comes_before_everything_but_stop() {
        let mut rig = Rig::new(3);
        rig.arm(0, 0x40, 100);
        rig.apic(0).request_fixed_edge(0x60).unwrap();
        let views = [
            rig.view(0, SchedActivity::Halted, IF_SET),
            rig.view(1, SchedActivity::Running, IF_CLEAR),
            rig.view(2, SchedActivity::Running, IF_SET),
        ];
        assert_eq!(decide(0, false, &views, &[10]), IdleDecision::Runnable(1));
        assert_eq!(decide(0, true, &views, &[10]), IdleDecision::Stopped);
    }

    #[test]
    fn no_deadline_means_all_idle_and_the_clock_does_not_move() {
        let rig = Rig::new(2);
        let views = rig.halted(2, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AllIdle);
        assert_eq!(decide(0, false, &[], &[]), IdleDecision::AllIdle);
        assert_eq!(rig.now(), 0);
        // An unarmed timer is no deadline, and neither is an armed but masked one.
        let mut rig = Rig::new(2);
        rig.arm(0, 0x40, 300);
        assert_eq!(rig.apic(0).next_timer_deadline(), Some(300));
        rig.write(0, 0x320, 0x10000 | 0x40);
        assert_eq!(rig.apic(0).next_timer_deadline(), None);
        let views = rig.halted(2, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AllIdle);
        assert_eq!(rig.now(), 0);
    }

    #[test]
    fn a_deadline_not_after_now_is_no_progress() {
        let mut rig = Rig::new(2);
        rig.arm(1, 0x41, 300);
        let views = rig.halted(2, IF_SET);
        assert_eq!(decide(300, false, &views, &[]), IdleDecision::NoProgress);
        assert_eq!(decide(301, false, &views, &[]), IdleDecision::NoProgress);
        assert_eq!(
            decide(299, false, &views, &[]),
            IdleDecision::AdvanceTo(300)
        );
        // A stale board deadline is no progress too, even next to a later timer.
        assert_eq!(decide(0, false, &views, &[0]), IdleDecision::NoProgress);
        // `apply` refuses it without touching anything.
        let mut called = false;
        let result = apply(
            300,
            300,
            |_| {
                called = true;
                Ok::<(), ()>(())
            },
            &views,
        );
        assert_eq!(result, Err(ApplyError::NoProgress));
        assert!(!called);
    }

    #[test]
    fn stop_leaves_the_time_and_the_queues_alone() {
        let mut rig = Rig::new(2);
        rig.arm(0, 0x40, 300);
        let views = rig.halted(2, IF_SET);
        let before = state(&rig, 2, 0x40);
        assert_eq!(decide(0, true, &views, &[5]), IdleDecision::Stopped);
        assert_eq!(rig.now(), 0);
        assert_eq!(state(&rig, 2, 0x40), before);
    }

    #[test]
    fn a_clock_overflow_changes_neither_the_clock_nor_the_queues() {
        // With a TSC frequency of u64::MAX the clock cannot pass 1_000_000_000 ns.
        let mut rig = Rig::with_tsc(2, u64::MAX);
        rig.arm(0, 0x40, 2_000_000_000);
        rig.arm(1, 0x41, 2_000_000_000);
        let views = rig.halted(2, IF_SET);
        let decision = decide(0, false, &views, &[]);
        assert_eq!(decision, IdleDecision::AdvanceTo(2_000_000_000));
        let before = state(&rig, 2, 0x40);
        let before_tsc = rig.clock.snapshot();
        let result = apply(0, 2_000_000_000, rig.advance(), &views);
        assert_eq!(
            result,
            Err(ApplyError::Advance(VirtualClockError::TscOverflow))
        );
        assert_eq!(rig.clock.snapshot(), before_tsc);
        assert_eq!(rig.now(), 0);
        assert_eq!(state(&rig, 2, 0x40), before);
        assert_eq!(state(&rig, 2, 0x41)[1].1, (false, false));
    }

    #[test]
    fn a_failed_dispatch_keeps_the_clock_and_stops_before_the_next_vcpu() {
        let mut rig = Rig::new(3);
        for cpu in 0..3 {
            rig.arm(cpu, 0x40 + cpu as u64, 500);
        }
        let views = rig.halted(3, IF_SET);
        assert_eq!(decide(0, false, &views, &[]), IdleDecision::AdvanceTo(500));
        // The public disable resets the timer, so it cannot make a dispatch fail; the LAPIC of
        // vCPU 1 is put in the refusing state directly, with its timer still armed.
        rig.apic(1).0.lock().unwrap().cpus[1].local.global_enabled = false;
        let cpu1_before = rig.apic(1).0.lock().unwrap().cpus[1].clone();
        let cpu2_before = rig.apic(2).0.lock().unwrap().cpus[2].clone();
        let result = apply(0, 500, rig.advance(), &views);
        assert_eq!(
            result,
            Err(ApplyError::Dispatch {
                cpu: 1,
                error: DeviceError::UnsupportedConfiguration
            })
        );
        // The clock has advanced; vCPU 0 was dispatched.
        assert_eq!(rig.now(), 500);
        assert_eq!(rig.apic(0).next_timer_deadline(), None);
        assert_eq!(rig.apic(0).vector_state(0x40), (true, false));
        // vCPU 1 keeps its armed timer and request state; vCPU 2 was not processed.
        assert_eq!(rig.apic(1).0.lock().unwrap().cpus[1], cpu1_before);
        assert_eq!(rig.apic(1).next_timer_deadline(), Some(500));
        assert_eq!(rig.apic(2).0.lock().unwrap().cpus[2], cpu2_before);
        assert_eq!(rig.apic(2).next_timer_deadline(), Some(500));
        assert_eq!(rig.apic(2).vector_state(0x42), (false, false));
    }

    #[test]
    fn the_same_inputs_give_the_same_result() {
        let run = || {
            let mut rig = Rig::new(3);
            rig.arm(2, 0x42, 400);
            rig.arm(1, 0x41, 400);
            rig.arm(0, 0x40, 900);
            let views = rig.halted(3, IF_SET);
            let first = decide(0, false, &views, &[1_000, 450]);
            let fired = apply(0, 400, rig.advance(), &views).unwrap();
            let second = decide(rig.now(), false, &views, &[1_000, 450]);
            (first, fired, second, rig.now(), state(&rig, 3, 0x41))
        };
        let a = run();
        assert_eq!(a, run());
        assert_eq!(a.0, IdleDecision::AdvanceTo(400));
        assert_eq!(a.1, [false, true, true]);
        assert_eq!(a.2, IdleDecision::WakeNow(1));
    }

    #[test]
    fn a_board_deadline_is_independent_of_the_timers() {
        let mut rig = Rig::new(2);
        rig.arm(1, 0x41, 900);
        let views = rig.halted(2, IF_SET);
        assert_eq!(
            decide(0, false, &views, &[250, 600]),
            IdleDecision::AdvanceTo(250)
        );
        let fired = apply(0, 250, rig.advance(), &views).unwrap();
        // Nothing is due at the board deadline: no timer fired, the timer is untouched.
        assert_eq!(fired, [false, false]);
        assert_eq!(rig.apic(1).next_timer_deadline(), Some(900));
        assert_eq!(rig.now(), 250);
    }

    // The arbiter on one vCPU against the real `InterruptPump::idle_advance`.

    struct HaltedBackend {
        clock: VirtualClockHandle,
        state: od_isa::LongModeState,
    }

    impl PumpBackend for HaltedBackend {
        fn state(&self) -> &od_isa::LongModeState {
            &self.state
        }
        fn is_waiting(&self) -> bool {
            true
        }
        fn virtual_time_ns(&self) -> Option<u64> {
            Some(self.clock.snapshot().now_ns())
        }
        fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), PumpRejection> {
            self.clock.advance_standalone(delta_ns).map_err(|error| {
                PumpRejection::TimerAdvance(crate::diagnostic_vcpu::DiagnosticHaltWaitError::Clock(
                    error,
                ))
            })
        }
        fn stop(&mut self) {}
        fn peek_ram_qword(&mut self, _: u64) -> Option<u64> {
            None
        }
        fn step(&mut self, _: BackendEnv<'_>) -> BackendStep {
            BackendStep::new(BackendFlow::Idle)
        }
    }

    #[derive(Clone, Copy)]
    enum Scenario {
        Timer,
        TimerWithIfClear,
        NoTimer,
        TimerBlockedByTpr,
    }

    fn single(scenario: Scenario) -> (Rig, bool) {
        let mut rig = Rig::new(1);
        let if_set = !matches!(scenario, Scenario::TimerWithIfClear);
        if !matches!(scenario, Scenario::NoTimer) {
            rig.arm(0, 0x40, 1_234);
        }
        if matches!(scenario, Scenario::TimerBlockedByTpr) {
            rig.write(0, 0x80, 0xf0);
            rig.apic(0).request_fixed_edge(0x40).unwrap();
        }
        (rig, if_set)
    }

    #[test]
    fn on_one_vcpu_the_decision_matches_the_pump_idle_advance() {
        let mut advanced = 0;
        for scenario in [
            Scenario::Timer,
            Scenario::TimerWithIfClear,
            Scenario::NoTimer,
            Scenario::TimerBlockedByTpr,
        ] {
            // The pump: one step into idle; it either advances or reports an idle halt.
            let (pump_rig, if_set) = single(scenario);
            let state = od_isa::LongModeState {
                rflags: 2 | (u64::from(if_set) << 9),
                ..od_isa::LongModeState::default()
            };
            let backend = HaltedBackend {
                clock: pump_rig.clock.clone(),
                state,
            };
            let mut pump = InterruptPump::with_backend(backend, pump_rig.apic(0));
            let result = pump.run(1, || false);

            // The arbiter on an identical rig.
            let (arb_rig, if_set) = single(scenario);
            let views = [arb_rig.view(0, SchedActivity::Halted, if_set)];
            let decision = decide(0, false, &views, &[]);
            match decision {
                IdleDecision::AdvanceTo(to) => {
                    assert_eq!(result.exit, PumpExit::Budget);
                    advanced += 1;
                    apply(0, to, arb_rig.advance(), &views).unwrap();
                }
                IdleDecision::AllIdle => assert_eq!(result.exit, PumpExit::IdleHalt),
                other => panic!("unexpected decision {other:?}"),
            }
            // Same clock, same timer queue, same IRR/ISR of the timer vector.
            assert_eq!(arb_rig.now(), pump_rig.now());
            assert_eq!(
                arb_rig.apic(0).next_timer_deadline(),
                pump_rig.apic(0).next_timer_deadline()
            );
            assert_eq!(
                arb_rig.apic(0).vector_state(0x40),
                pump_rig.apic(0).vector_state(0x40)
            );
        }
        // Only the plain timer scenario advances the clock; the other three idle-halt.
        assert_eq!(advanced, 1);
    }
}
