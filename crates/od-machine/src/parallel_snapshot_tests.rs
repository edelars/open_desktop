//! S6-35 on the machine fixture of the S6-34 tests (two vCPUs, the Local APIC timers armed): the
//! state of the processors on a machine that the parallel mode has stopped, and what the restore
//! does to the timers and to the pending requests.
//!
//! The boot processor runs a loop with IF = 1 and takes a periodic LAPIC timer (vector `0x31`,
//! handler counts into `COUNTER_0`); the application processor sleeps in `HLT` with IF = 1 and takes
//! a one-shot LAPIC timer (vector `0x30`, handler counts into `COUNTER_1`). The controller stops the
//! run at a moment drawn from the seed, posts one IPI to the stopped, sleeping AP (it stays pending
//! in its IRR), and takes the state. The run ends, the CPU complex is reset to power-on in the same
//! machine (RAM, devices and the clock untouched), the state is put back, and the machine goes on
//! in the interleaved mode. Virtual time is absolute and stands over all of it, so the one-shot
//! must fire exactly once (the IPI and the timer: two entries of the AP handler), the periodic timer
//! exactly once per period (counter plus the request still in flight = the periods elapsed), and
//! no pending request may be lost. This is the state of the processors in one machine, not a
//! snapshot of the machine (`S9-24`/`S9-25`).

use crate::firmware_board::vcpu_quantum::fixture::*;
use crate::firmware_board::vcpu_quantum::run_vcpu_set_quantum_shared;
use crate::parallel_run::{ParallelConfig, ParallelFinish, QuantumCall, run_threads};
use crate::smp_scheduler::{
    BoardSmpHost, Executed, QuantumPolicy, SmpScheduler, StopAck, StopRequest,
};
use crate::vm_snapshot::{SnapshotError, VmSnapshot};
use crate::{AccessWidth, FirmwareBoardError, GuestPhysicalAddress, LOCAL_APIC_MMIO_BASE};
use crate::{LoadedFirmwareBoard, parallel_run::StopProof};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// One count of the LAPIC timer is 10 ns (100 MHz, divider 1).
const NS_PER_COUNT: u64 = 10;
const PERIOD_NS: u64 = 25_000;
const ONE_SHOT_NS: u64 = 300_000;
const PERIODIC_LVT: u64 = (1 << 17) | TO_BSP as u64;
const WAIT: Duration = Duration::from_secs(60);
const BSP_EFER: u64 = 0xD01;

fn parallel_quantum(call: QuantumCall<'_, '_>) -> Result<Executed, FirmwareBoardError> {
    with_set(|set| {
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
        )
    })
}

fn interleaved_quantum(
    board: &mut LoadedFirmwareBoard,
    cpu: usize,
    budget: u64,
    stop: &mut dyn FnMut() -> bool,
) -> Result<Executed, FirmwareBoardError> {
    with_set(|set| board.run_vcpu_set_quantum(cpu, budget, stop, set))
}

fn lapic_write(board: &mut LoadedFirmwareBoard, cpu: usize, offset: u64, value: u64) {
    board
        .bus_mut()
        .write_mmio_shared(
            cpu,
            GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
            AccessWidth::Dword,
            value,
        )
        .expect("the register accepts the write");
}

/// SVR enabled, then the LVT timer (`lvt`), the divider 1 and the initial count of `ns`.
fn arm(board: &mut LoadedFirmwareBoard, cpu: usize, lvt: u64, ns: u64) {
    lapic_write(board, cpu, 0xF0, 0x1FF);
    lapic_write(board, cpu, 0x320, lvt);
    lapic_write(board, cpu, 0x3E0, 0xB);
    lapic_write(board, cpu, 0x380, ns / NS_PER_COUNT);
}

fn bsp_state() -> od_isa::LongModeState {
    long_state(CODE_0, STACK_0, TABLES_0, true)
}

/// The boot processor spins (IF = 1, handler of `0x31` counts and ends the interrupt), the AP
/// sleeps (IF = 1, handler of `0x30` counts), both Local APIC timers armed.
fn board_with_timers() -> LoadedFirmwareBoard {
    let mut board = board_without_timers();
    arm(&mut board, 0, PERIODIC_LVT, PERIOD_NS);
    arm(&mut board, 1, u64::from(TO_AP), ONE_SHOT_NS);
    board
}

fn board_without_timers() -> LoadedFirmwareBoard {
    let mut board = board(2);
    system(&mut board, false);
    let mut handler_0 = count_and_eoi(COUNTER_0);
    handler_0.extend([0x48, 0xCF]);
    patch(&mut board, HANDLER_0, &handler_0);
    let mut spin = enable_lapic();
    spin.extend([0xEB, 0xFE]);
    patch(&mut board, CODE_0, &spin);
    patch(&mut board, CODE_1, &ap_program(false));
    let cr3_0 = tables(&mut board, TABLES_0, 0x60_0000, 0);
    let cr3_1 = tables(&mut board, TABLES_1, 0x80_0000, 0);
    install(
        &mut board,
        0,
        long_state(CODE_0, STACK_0, cr3_0, true),
        BSP_EFER,
    );
    install(
        &mut board,
        1,
        long_state(CODE_1, STACK_1, cr3_1, true),
        BSP_EFER,
    );
    board
}

fn deadlines(board: &LoadedFirmwareBoard) -> Vec<Option<u64>> {
    (0..2)
        .map(|cpu| board.local_apic_of(cpu).unwrap().next_timer_deadline())
        .collect()
}

fn pending(board: &LoadedFirmwareBoard, cpu: usize) -> Vec<(u8, bool, bool)> {
    let apic = board.local_apic_of(cpu).unwrap();
    (0..=255u8)
        .filter_map(|vector| {
            let (irr, isr) = apic.vector_state(vector);
            (irr || isr).then_some((vector, irr, isr))
        })
        .collect()
}

fn cell(control: &crate::parallel_run::ParallelControl<'_, '_>, at: u64) -> u32 {
    control
        .bus()
        .physical_memory()
        .load_u32(GuestPhysicalAddress::new(at))
        .unwrap()
}

/// What the stop of one round found.
#[derive(Debug, Default)]
struct Round {
    /// The one-shot was still to come and the AP's IRR held the IPI (always, by the test).
    now_at_stop: u64,
    fires_before_the_stop: u32,
    ap_asleep: bool,
}

fn round(seed: u64) -> Round {
    let quantum = [1u64, 7, 64][(seed % 3) as usize];
    let stop_at = 5_000 + seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) % 95_000;
    let mut board = board_with_timers();
    let lapic = board.local_apic();
    let sender = board.local_apic_of(0).unwrap();
    let mut scheduler =
        SmpScheduler::new(2, QuantumPolicy::seeded(seed), StopRequest::new()).unwrap();
    let config = ParallelConfig {
        quantum,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(20),
        end_when_idle: true,
        pre_sleep: None,
        barrier_window: None,
        standing_window: None,
    };
    let (report, (snapshot, stopped)) =
        run_threads(&mut board, config, &parallel_quantum, |control| {
            assert!(
                control.wait_until(WAIT, || control.now_ns() >= stop_at),
                "seed {seed}: the machine does not run"
            );
            // No stop in force: nothing is read, whatever proof is shown.
            let unproven = StopProof {
                acks: vec![StopAck::Boundary; 2],
                now_ns: control.now_ns(),
            };
            assert_eq!(
                VmSnapshot::take_parallel(control, &lapic, &unproven, &scheduler),
                Err(SnapshotError::NotStopped),
                "seed {seed}"
            );
            // A proof of a stop that was resumed is not a proof: nothing is read with it either.
            let first = control.stop_all(Duration::from_secs(10)).expect("stop");
            control.resume();
            assert_eq!(
                VmSnapshot::take_parallel(control, &lapic, &first, &scheduler),
                Err(SnapshotError::NotStopped),
                "seed {seed}"
            );
            std::thread::sleep(Duration::from_micros(300));
            let proof = control.stop_all(Duration::from_secs(10)).expect("stop");
            assert!(
                proof.now_ns < ONE_SHOT_NS,
                "seed {seed}: the stop came after the one-shot ({} ns)",
                proof.now_ns
            );
            if first.now_ns != proof.now_ns {
                assert_eq!(
                    VmSnapshot::take_parallel(control, &lapic, &first, &scheduler),
                    Err(SnapshotError::StaleSafePoint),
                    "seed {seed}: the proof of an earlier stop"
                );
            }
            // A request that arrives at the stopped, sleeping AP waits in its IRR (S6-34, step 4).
            sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
            let (counter_0, counter_1) = (cell(control, COUNTER_0), cell(control, COUNTER_1));
            let snapshot = VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler)
                .unwrap_or_else(|error| panic!("seed {seed}: {error:?}"));
            assert_eq!(snapshot.now_ns(), proof.now_ns);
            // The AP has not taken the IPI, and the clock stood while we looked.
            assert_eq!(cell(control, COUNTER_1), counter_1);
            assert_eq!(control.now_ns(), proof.now_ns);
            (snapshot, (counter_0, counter_1, proof.now_ns, proof.acks))
        })
        .expect("the board splits");
    assert_eq!(
        report.finished,
        ParallelFinish::Stopped,
        "seed {seed}: {report:?}"
    );
    assert_eq!(report.threads_started, report.threads_exited);
    let (counter_0, counter_1, now_at_stop, acks) = stopped;
    assert_eq!(
        counter_1, 0,
        "seed {seed}: the AP took nothing before the IPI"
    );
    // The machine stands as the controller took it: no vCPU, Local APIC or clock moved when the
    // threads ended.
    for (cpu, vcpu) in snapshot.vcpus().iter().enumerate() {
        assert_eq!(board.vcpu(cpu).unwrap(), vcpu, "seed {seed}: vCPU {cpu}");
    }
    assert_eq!(&board.local_apic().snapshot_pair(), snapshot.apic());
    assert_eq!(board.clock().now_ns(), now_at_stop);

    // The stop of the interleaved scheduler on the same machine finds what the parallel stop found.
    let point = {
        let mut host = BoardSmpHost::new(&mut board, interleaved_quantum).unwrap();
        scheduler.stop_all(&mut host, 8).unwrap()
    };
    let at_rest = VmSnapshot::take(&board, &scheduler, &point).unwrap();
    assert_eq!(at_rest.vcpus(), snapshot.vcpus(), "seed {seed}");
    assert_eq!(at_rest.apic(), snapshot.apic(), "seed {seed}");
    // The timers, before: the one-shot at its absolute deadline, the periodic one on its phase.
    let before = (deadlines(&board), pending(&board, 0), pending(&board, 1));
    assert_eq!(before.0[1], Some(ONE_SHOT_NS), "seed {seed}");
    let periodic = before.0[0].expect("the periodic timer is armed");
    assert!(
        periodic > now_at_stop && periodic.is_multiple_of(PERIOD_NS),
        "seed {seed}: next {periodic}, now {now_at_stop}"
    );
    assert!(
        before.2.contains(&(TO_AP, true, false)),
        "seed {seed}: the IPI waits in the IRR of the AP: {:?}",
        before.2
    );
    // The reset empties the queues and the requests, RAM and the clock stay ...
    scheduler.reset();
    board.reset_cpu_complex(bsp_state(), BSP_EFER).unwrap();
    assert_eq!(deadlines(&board), vec![None, None], "seed {seed}");
    assert!(pending(&board, 1).is_empty());
    assert_eq!(
        board.vcpu(1).unwrap().activity(),
        od_isa::CpuActivity::WaitForSipi
    );
    assert_eq!(board.clock().now_ns(), now_at_stop);
    assert_eq!(dword(&mut board, COUNTER_1), counter_1);
    // ... and the restore brings every timer and every request back, the same ones.
    snapshot
        .restore(&mut board, &mut scheduler, &point)
        .unwrap_or_else(|error| panic!("seed {seed}: {error:?}"));
    assert_eq!(
        (deadlines(&board), pending(&board, 0), pending(&board, 1)),
        before,
        "seed {seed}: timers and requests after the restore"
    );
    assert_eq!(&board.local_apic().snapshot_pair(), snapshot.apic());
    for (cpu, vcpu) in snapshot.vcpus().iter().enumerate() {
        assert_eq!(board.vcpu(cpu).unwrap(), vcpu);
    }
    // Go on in the interleaved mode (the seed is the one the snapshot carries) past the one-shot
    // and three more periods.
    scheduler.resume();
    let target = ONE_SHOT_NS + 3 * PERIOD_NS;
    {
        let mut host = BoardSmpHost::new(&mut board, interleaved_quantum).unwrap();
        let finished = scheduler.run(&mut host, target - now_at_stop).unwrap();
        assert_eq!(
            finished,
            crate::smp_scheduler::Finished::BudgetExhausted,
            "seed {seed}"
        );
    }
    let end = board.clock().now_ns();
    assert!(end >= target, "seed {seed}: the clock stopped at {end}");
    // The IPI and the one-shot: two entries of the AP handler, no more. The one-shot is spent.
    assert_eq!(dword(&mut board, COUNTER_1), 2, "seed {seed}");
    assert_eq!(deadlines(&board)[1], None, "seed {seed}");
    // The periodic timer fired once for every period that elapsed, before the stop, at the stop and
    // after the restore: handled entries plus the request still in flight. At most one is in flight
    // (a request can stand in the IRR and the ISR at the same moment; the run is short against the
    // period of the timer).
    let in_flight = u32::from(
        pending(&board, 0)
            .iter()
            .any(|&(vector, _, _)| vector == TO_BSP),
    );
    assert_eq!(
        u64::from(dword(&mut board, COUNTER_0) + in_flight),
        end / PERIOD_NS,
        "seed {seed}: periods elapsed by {end} ns (stop at {now_at_stop})"
    );
    assert!(acks.len() == 2);
    Round {
        now_at_stop,
        fires_before_the_stop: counter_0,
        ap_asleep: acks[1] == StopAck::Asleep(crate::idle_arbiter::SchedActivity::Halted),
    }
}

fn rounds(count: u64) -> (u64, u64, u64) {
    let (mut with_fires, mut asleep, mut max_now) = (0, 0, 0);
    for seed in 0..count {
        let found = round(seed);
        with_fires += u64::from(found.fires_before_the_stop > 0);
        asleep += u64::from(found.ap_asleep);
        max_now = max_now.max(found.now_at_stop);
    }
    (with_fires, asleep, max_now)
}

#[test]
fn timers_and_pending_requests_survive_a_stop_of_the_parallel_mode_a_reset_and_a_restore() {
    let (with_fires, asleep, _) = rounds(6);
    eprintln!(
        "timers across the stop: {with_fires} of 6 stops after a periodic fire, {asleep} with the AP asleep"
    );
    assert!(with_fires >= 1 && asleep >= 1);
}

/// 50 rounds by default (`OD_S6_SNAP_TIMER_RUNS`), seeds `0..rounds`.
#[test]
#[ignore = "long: OD_S6_SNAP_TIMER_RUNS=500 cargo test --release -p od-machine --offline --lib -- --ignored parallel_snapshot_tests::timers_across_the_stop_many_runs --nocapture"]
fn timers_across_the_stop_many_runs() {
    let count = std::env::var("OD_S6_SNAP_TIMER_RUNS")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(50);
    let started = std::time::Instant::now();
    let (with_fires, asleep, max_now) = rounds(count);
    eprintln!(
        "timers across the stop: {count} rounds, {:.1} s, {with_fires} stops after a periodic fire, {asleep} with the AP asleep, latest stop at {max_now} ns",
        started.elapsed().as_secs_f64()
    );
    assert!(with_fires >= count / 2 && asleep >= count / 2);
}

/// A request that arrives at the stopped, sleeping AP kicks its thread, which goes through its
/// barrier again and lowers its `safe` flag for a moment (S6-34 review, m2). The snapshot taken at
/// that very moment must still be taken, whole and right, every time: 300 stops, an IPI posted
/// to the AP right after each, the state taken at once, the run resumed, the IPI handled.
#[test]
fn a_kick_of_the_stopped_sleeping_ap_does_not_spoil_the_snapshot() {
    let mut board = board_without_timers();
    let lapic = board.local_apic();
    let sender = board.local_apic_of(0).unwrap();
    let scheduler = SmpScheduler::new(2, QuantumPolicy::Fixed(16), StopRequest::new()).unwrap();
    let config = ParallelConfig {
        quantum: 8,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(20),
        end_when_idle: true,
        pre_sleep: None,
        barrier_window: None,
        standing_window: None,
    };
    let (report, taken) = run_threads(&mut board, config, &parallel_quantum, |control| {
        assert!(control.wait_until(WAIT, || control.activity(1)
            == crate::idle_arbiter::SchedActivity::Halted));
        for round in 1..=300u32 {
            let proof = control.stop_all(Duration::from_secs(10)).expect("stop");
            sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
            let snapshot = VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler)
                .unwrap_or_else(|error| panic!("round {round}: {error:?}"));
            assert_eq!(
                snapshot.apic().cpus[1].local.irr[1] >> 16 & 1,
                1,
                "round {round}"
            );
            assert_eq!(cell(control, COUNTER_1), round - 1, "round {round}");
            control.resume();
            assert!(
                control.wait_until(WAIT, || cell(control, COUNTER_1) >= round),
                "round {round}: the IPI was lost"
            );
        }
        true
    })
    .expect("the board splits");
    assert!(taken);
    assert_eq!(report.threads_started, report.threads_exited);
    assert_eq!(dword(&mut board, COUNTER_1), 300);
}

// The state of the injection of the test below (a plain `fn` hook cannot capture).
static HOLD_IN_WINDOW: AtomicBool = AtomicBool::new(false);
static HELD_IN_WINDOW: AtomicBool = AtomicBool::new(false);
static RELEASE_WINDOW: AtomicBool = AtomicBool::new(false);
static WINDOW_INJECTION: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn hold_vcpu_1_in_the_barrier_window(cpu: usize) {
    if cpu == 1 && HOLD_IN_WINDOW.load(Ordering::SeqCst) {
        HELD_IN_WINDOW.store(true, Ordering::SeqCst);
        while !RELEASE_WINDOW.load(Ordering::SeqCst) {
            std::hint::spin_loop();
        }
        HELD_IN_WINDOW.store(false, Ordering::SeqCst);
    }
}

/// S6-36, conditions 1 and 2 (the window of the barrier, S6-34 review m2). The AP sleeps in `HLT`
/// and the machine is stopped, so its `safe` flag is up. An IPI posted to it kicks its thread out
/// of the sleep and to the barrier. The injection of the barrier (between the lowering of `safe`
/// and its raising again) holds the thread there if it ever gets there: with the old barrier it
/// does, `safe` stays down while it is held, and `take_parallel` cannot read the vCPU (stale safe
/// point); with the barrier of S6-36 the thread finds `safe` up and the stop in force, lowers
/// nothing, never reaches the injection, and the snapshot is taken at once, whole and right.
/// Twenty rounds, each bounded; the mutation that restores the old order fails the first one.
#[test]
fn a_kick_of_the_stopped_ap_never_lowers_safe_in_the_barrier_and_the_snapshot_is_taken() {
    let _injection = WINDOW_INJECTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut board = board_without_timers();
    let lapic = board.local_apic();
    let sender = board.local_apic_of(0).unwrap();
    let scheduler = SmpScheduler::new(2, QuantumPolicy::Fixed(16), StopRequest::new()).unwrap();
    let config = ParallelConfig {
        quantum: 8,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(20),
        end_when_idle: true,
        pre_sleep: None,
        barrier_window: Some(hold_vcpu_1_in_the_barrier_window),
        standing_window: None,
    };
    HOLD_IN_WINDOW.store(false, Ordering::SeqCst);
    RELEASE_WINDOW.store(false, Ordering::SeqCst);
    HELD_IN_WINDOW.store(false, Ordering::SeqCst);
    let (report, held_rounds) = run_threads(&mut board, config, &parallel_quantum, |control| {
        assert!(control.wait_until(WAIT, || control.activity(1)
            == crate::idle_arbiter::SchedActivity::Halted));
        let mut held_rounds = Vec::new();
        for round in 1..=20u32 {
            let proof = control.stop_all(Duration::from_secs(10)).expect("stop");
            RELEASE_WINDOW.store(false, Ordering::SeqCst);
            HELD_IN_WINDOW.store(false, Ordering::SeqCst);
            HOLD_IN_WINDOW.store(true, Ordering::SeqCst);
            sender.write_icr(0, 1, FIXED | u32::from(TO_AP)).unwrap();
            // Give the thread the time to come to the barrier: the old one is held in its window.
            let until = std::time::Instant::now() + Duration::from_millis(100);
            while !HELD_IN_WINDOW.load(Ordering::SeqCst) && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(1));
            }
            let held = HELD_IN_WINDOW.load(Ordering::SeqCst);
            let taken = VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler);
            // Let the thread go before anything is asserted, so that the run can end.
            HOLD_IN_WINDOW.store(false, Ordering::SeqCst);
            RELEASE_WINDOW.store(true, Ordering::SeqCst);
            if held {
                held_rounds.push((round, taken.is_ok()));
            } else {
                let snapshot = taken.unwrap_or_else(|error| panic!("round {round}: {error:?}"));
                assert_eq!(
                    snapshot.apic().cpus[1].local.irr[1] >> 16 & 1,
                    1,
                    "round {round}"
                );
                assert_eq!(cell(control, COUNTER_1), round - 1, "round {round}");
            }
            control.resume();
            assert!(
                control.wait_until(WAIT, || cell(control, COUNTER_1) >= round),
                "round {round}: the IPI was lost"
            );
            assert!(
                control.wait_until(WAIT, || control.activity(1)
                    == crate::idle_arbiter::SchedActivity::Halted),
                "round {round}: the AP does not go back to sleep"
            );
        }
        held_rounds
    })
    .expect("the board splits");
    HOLD_IN_WINDOW.store(false, Ordering::SeqCst);
    RELEASE_WINDOW.store(true, Ordering::SeqCst);
    assert_eq!(report.threads_started, report.threads_exited);
    assert!(
        held_rounds.is_empty(),
        "the thread of a stopped, sleeping AP lowered `safe` in its barrier in rounds {held_rounds:?} (round, snapshot taken)"
    );
    assert_eq!(dword(&mut board, COUNTER_1), 20);
}

// The state of the first injection (before the lowering of `safe`) of the test below.
static HOLD_BEFORE_LOWERING: AtomicBool = AtomicBool::new(false);
static HELD_BEFORE_LOWERING: AtomicBool = AtomicBool::new(false);
static RELEASE_BEFORE_LOWERING: AtomicBool = AtomicBool::new(false);

fn hold_vcpu_1_before_it_lowers_safe(cpu: usize) {
    if cpu == 1 && HOLD_BEFORE_LOWERING.load(Ordering::SeqCst) {
        HELD_BEFORE_LOWERING.store(true, Ordering::SeqCst);
        while !RELEASE_BEFORE_LOWERING.load(Ordering::SeqCst) {
            std::hint::spin_loop();
        }
        HELD_BEFORE_LOWERING.store(false, Ordering::SeqCst);
    }
}

fn wait_flag(flag: &AtomicBool, what: &str) {
    let until = std::time::Instant::now() + Duration::from_secs(10);
    while !flag.load(Ordering::SeqCst) {
        assert!(std::time::Instant::now() < until, "{what}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// S6-36 review, M-1: the residual window of the barrier. A thread woken by the kick of a resume
/// reads "not standing" (the stop is not set yet) and is held there; the controller sets the stop
/// and gets its proof (the thread's `safe` is still up from the earlier stop); the thread is let
/// go, lowers `safe`, and is held again before it reads the stop and raises `safe`. The reader
/// finds `safe` of the vCPU down under a stop whose proof was given: with a short bound it gets
/// `SafeFlagTimeout` (neither `StaleSafePoint` nor `NotStopped`, and the stop stays in force);
/// with the default wait it waits until the thread is let go and takes the snapshot, whole and
/// right. Without the wait (the code of the first version of S6-36) the second call gives a
/// false refusal.
#[test]
fn a_thread_between_its_two_stores_in_the_barrier_is_waited_for_and_not_refused() {
    let _injection = WINDOW_INJECTION
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut board = board_without_timers();
    let lapic = board.local_apic();
    let scheduler = SmpScheduler::new(2, QuantumPolicy::Fixed(16), StopRequest::new()).unwrap();
    let config = ParallelConfig {
        quantum: 8,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(20),
        end_when_idle: true,
        pre_sleep: None,
        barrier_window: Some(hold_vcpu_1_in_the_barrier_window),
        standing_window: Some(hold_vcpu_1_before_it_lowers_safe),
    };
    for flag in [
        &HOLD_IN_WINDOW,
        &HELD_IN_WINDOW,
        &RELEASE_WINDOW,
        &HOLD_BEFORE_LOWERING,
        &HELD_BEFORE_LOWERING,
        &RELEASE_BEFORE_LOWERING,
    ] {
        flag.store(false, Ordering::SeqCst);
    }
    let (report, outcome) = run_threads(&mut board, config, &parallel_quantum, |control| {
        assert!(control.wait_until(WAIT, || control.activity(1)
            == crate::idle_arbiter::SchedActivity::Halted));
        // An earlier stop, so that `safe` of the AP is up when the resume comes.
        control
            .stop_all(Duration::from_secs(10))
            .expect("first stop");
        HOLD_BEFORE_LOWERING.store(true, Ordering::SeqCst);
        control.resume();
        wait_flag(&HELD_BEFORE_LOWERING, "the AP did not come to the barrier");
        // The thread of the AP has read "not standing" and holds. The stop is set and its proof
        // given on the `safe` of the earlier stop.
        let proof = control
            .stop_all(Duration::from_secs(10))
            .expect("second stop");
        // Let it lower `safe`; it is held again between the lowering and the raising.
        HOLD_IN_WINDOW.store(true, Ordering::SeqCst);
        HOLD_BEFORE_LOWERING.store(false, Ordering::SeqCst);
        RELEASE_BEFORE_LOWERING.store(true, Ordering::SeqCst);
        wait_flag(
            &HELD_IN_WINDOW,
            "the AP did not come to the second injection",
        );
        let short = VmSnapshot::take_parallel_within(
            control,
            &lapic,
            &proof,
            &scheduler,
            Duration::from_millis(100),
        );
        let still_stopped = control.stopped();
        // Release the thread from a helper after a pause; the default wait must outlast it.
        let taken = std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(150));
                HOLD_IN_WINDOW.store(false, Ordering::SeqCst);
                RELEASE_WINDOW.store(true, Ordering::SeqCst);
            });
            VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler)
        });
        HOLD_IN_WINDOW.store(false, Ordering::SeqCst);
        RELEASE_WINDOW.store(true, Ordering::SeqCst);
        control.resume();
        (short, still_stopped, taken)
    })
    .expect("the board splits");
    for flag in [&HOLD_IN_WINDOW, &HOLD_BEFORE_LOWERING] {
        flag.store(false, Ordering::SeqCst);
    }
    RELEASE_WINDOW.store(true, Ordering::SeqCst);
    RELEASE_BEFORE_LOWERING.store(true, Ordering::SeqCst);
    assert_eq!(report.threads_started, report.threads_exited);
    let (short, still_stopped, taken) = outcome;
    assert_eq!(short, Err(SnapshotError::SafeFlagTimeout { cpu: 1 }));
    assert!(still_stopped, "the timeout must leave the stop in force");
    let snapshot = taken.expect("the wait outlasts the window: the snapshot is taken");
    assert_eq!(snapshot.vcpus().len(), 2);
}

/// A proof with another number of acknowledgements than the run has threads is refused as a stale
/// safe point, before any vCPU is looked at: one more must not index past the vCPUs (review of
/// S6-35, m-1), one less is not a whole snapshot. Nothing changes in the machine, and the true
/// proof of the same stop still gives the snapshot.
#[test]
fn a_proof_with_another_number_of_acknowledgements_is_stale_and_changes_nothing() {
    let mut board = board_without_timers();
    let lapic = board.local_apic();
    let scheduler = SmpScheduler::new(2, QuantumPolicy::Fixed(16), StopRequest::new()).unwrap();
    let config = ParallelConfig {
        quantum: 8,
        budget: u64::MAX,
        split_timeout: Duration::from_secs(20),
        end_when_idle: true,
        pre_sleep: None,
        barrier_window: None,
        standing_window: None,
    };
    let (report, taken) = run_threads(&mut board, config, &parallel_quantum, |control| {
        assert!(control.wait_until(WAIT, || control.activity(1)
            == crate::idle_arbiter::SchedActivity::Halted));
        let proof = control.stop_all(Duration::from_secs(10)).expect("stop");
        assert_eq!(control.vcpu_count(), 2);
        let (retired, counter_1) = (control.retired(), cell(control, COUNTER_1));
        let mut more = StopProof {
            acks: proof.acks.clone(),
            now_ns: proof.now_ns,
        };
        more.acks.push(StopAck::Boundary);
        let fewer = StopProof {
            acks: proof.acks[..1].to_vec(),
            now_ns: proof.now_ns,
        };
        let none = StopProof {
            acks: Vec::new(),
            now_ns: proof.now_ns,
        };
        for refused in [&more, &fewer, &none] {
            assert_eq!(
                VmSnapshot::take_parallel(control, &lapic, refused, &scheduler),
                Err(SnapshotError::StaleSafePoint),
                "{} acknowledgements",
                refused.acks.len()
            );
        }
        assert!(control.stopped());
        assert_eq!(control.retired(), retired);
        assert_eq!(cell(control, COUNTER_1), counter_1);
        assert_eq!(control.now_ns(), proof.now_ns);
        let snapshot = VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler)
            .expect("the true proof of the same stop still serves");
        assert_eq!(snapshot.vcpus().len(), 2);
        control.resume();
        true
    })
    .expect("the board splits");
    assert!(taken);
    assert_eq!(report.threads_started, report.threads_exited);
}
