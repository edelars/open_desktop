//! S6-42b: the JIT host loop on `&MachineBus` (A7 of S6-40) and on the vCPU threads of
//! `ParallelControl` (S6-34).
//!
//! The tests of S6-42 put the bus behind a `Mutex` because the paged JIT path and the shared
//! interpreter took `&mut MachineBus`. Here no lock of the bus exists anywhere: the threads hold
//! `&MachineBus`, RAM is `PhysicalMemory` by `&self` (parallel mode: `SeqCst` accesses, `Release`
//! page generations), and the interpreter fallback takes the device lock for one port access at a
//! time. `run_paged_host_loop_shared` is the loop; `od_machine::run_host_threads` runs the threads.

use super::*;
use crate::host_threads::JitHostQuantum;
use crate::jit_region::SharedJitRegion;
use od_machine::{
    ExecutionMode, HostParallelConfig, HostParallelFinish, host_install_vcpu, host_parallel_board,
    run_host_threads,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Barrier, Mutex};
use std::time::{Duration, Instant};

const A_CODE_RIP: u64 = 0x0040_2000;
const A_CODE_PHYSICAL: u64 = 0xa000;
/// `mov [rbx],rax`, the native store form, padded to a qword.
const FORM_STORE: [u8; 8] = [0x48, 0x89, 0x03, 0, 0, 0, 0, 0];
const WAIT: Duration = Duration::from_secs(60);

fn qword(bytes: [u8; 8]) -> u64 {
    u64::from_le_bytes(bytes)
}

/// `put_qword` and the code of vCPU A at linear 0x402000 (physical 0xa000), on top of the tables
/// of `populate_tables`.
fn add_code_a(bus: &mut MachineBus, code_a: &[u8]) {
    // PT entry of linear 0x402000 -> physical 0xa000 (present, writable, user).
    put_qword(bus, 0x6010, A_CODE_PHYSICAL | 0x7);
    bus.patch_physical_ram(GuestPhysicalAddress::new(A_CODE_PHYSICAL), code_a)
        .unwrap();
}

// ---------------------------------------------------------------------------
// The shared loop equals the interpreter (and so the exclusive loop).
// ---------------------------------------------------------------------------

/// The S3-38o mixed program through `run_paged_host_loop_shared`, budgets 0 to 13 and 103: the
/// registers, the RAM, the committed writes and the counters equal the pure interpreter's.
#[test]
fn s6_42b_the_shared_loop_equals_the_interpreter_on_the_mixed_program() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(15, 2).unwrap();
    for budget in (0..=13).chain([103]) {
        let reference_run = reference(&LOOP_ALIAS, &loop_state(), budget, true);
        let mut session = PagedFetchedLoadSession::with_region_part(region.take_part().unwrap());
        let control = PagedHostLoopControl::new();
        let mut bus = make_bus(&LOOP_JIT_STORE);
        bus.set_execution_mode(ExecutionMode::Parallel);
        let mut state = loop_state();
        let mut rflags_defined = RFLAGS_DEFINED;
        let mut writes = Vec::new();
        let report = run_paged_host_loop_shared(
            &mut session,
            &control,
            config(),
            &mut state,
            &bus,
            0,
            &mut rflags_defined,
            budget,
            |range| writes.push((range.start().get(), range.len())),
            || false,
        );
        assert_eq!(
            report.exit,
            PagedHostLoopExit::BudgetExhausted,
            "budget {budget}"
        );
        assert_eq!(report.retired_total(), reference_run.retired, "{budget}");
        assert!(report.budget_is_consistent(), "budget {budget}");
        bus.set_execution_mode(ExecutionMode::Interleaved);
        assert_eq!(
            observe(&state, &mut bus, true),
            reference_run.observation,
            "budget {budget}"
        );
        assert_eq!(writes, reference_run.writes, "budget {budget}");
        if budget == 103 {
            assert_eq!(
                (report.retired_jit, report.retired_interpreter),
                (52, 51),
                "both paths ran"
            );
        }
        drop(session);
    }
}

/// The run's stop poll ends the call on the safe point like a raised STOP flag: no step after it.
#[test]
fn s6_42b_the_external_stop_poll_ends_the_call_before_the_next_step() {
    let _guard = jit_guard();
    let mut session = new_session();
    let control = PagedHostLoopControl::new();
    let bus = make_bus(&LOOP_JIT_STORE);
    let mut state = loop_state();
    let mut rflags_defined = RFLAGS_DEFINED;
    let mut polls = 0u32;
    let report = run_paged_host_loop_shared(
        &mut session,
        &control,
        config(),
        &mut state,
        &bus,
        0,
        &mut rflags_defined,
        1_000_000,
        |_| {},
        || {
            polls += 1;
            polls > 7
        },
    );
    assert_eq!(report.exit, PagedHostLoopExit::StopRequested);
    // Seven polls said "go": seven steps ran, none after the true one.
    assert_eq!(report.retired_total(), 7);
    assert!(report.budget_is_consistent());
}

// ---------------------------------------------------------------------------
// D1 without a lock: A changes the code, B runs it, the bus is shared by reference.
// ---------------------------------------------------------------------------

/// `mov rax,[rbx+disp8]` padded to a qword: the instruction vCPU A writes over B's code.
fn load_code(k: u64) -> [u8; 8] {
    let disp = (8 * k as i64 - 128) as i8 as u8;
    [0x48, 0x8b, 0x43, disp, 0, 0, 0, 0]
}

const SLOT_BASE: u64 = 0xC0DE_0000;
const WRITES: u64 = 31;

/// Result of one run of the lock-free pair.
struct LockFreeRun {
    steps: u64,
    distinct_seen: u64,
    final_k: u64,
    jit_steps_b: u64,
    fallback_b: u64,
    b_admissions: u64,
}

/// vCPU A (thread, own session and part) rewrites the instruction at B's `CODE_RIP` 31 times; each
/// write is a native `mov [rbx],rax` of 8 bytes and is published with `Release` after it committed.
/// vCPU B (thread, own session and part) runs one step at a time over the same `&MachineBus`:
/// every step first reads the published count with `Acquire` (`seen`), then runs the instruction
/// at `CODE_RIP`, which loads one of 32 distinct qwords, so RAX names the instruction that ran.
/// D1: after the `Acquire` read of `seen`, the first step already runs an instruction at least as
/// new as write `seen`; the instruction never goes back to an older one. There is no `Mutex` of
/// the bus.
fn run_lock_free_pair(lockstep: bool) -> LockFreeRun {
    let region = SharedJitRegion::new(2, 4).unwrap();
    let part_b = region.take_part().unwrap();
    let part_a = region.take_part().unwrap();
    let mut bus = make_bus(&load_code(0));
    add_code_a(&mut bus, &FORM_STORE);
    for k in 0..=WRITES {
        put_qword(&mut bus, DATA_QWORD + 8 * k, SLOT_BASE + k);
    }
    bus.set_execution_mode(ExecutionMode::Parallel);
    let published = AtomicU64::new(0);
    let b_steps = AtomicU64::new(0);
    let start = Barrier::new(2);

    let b_result = std::thread::scope(|scope| {
        let (bus, published, b_steps, start) = (&bus, &published, &b_steps, &start);
        let b = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_b);
            let control = PagedHostLoopControl::new();
            let mut state = loop_state();
            state.gpr[Register::Rbx as usize] = DATA_LINEAR + 128;
            let mut rflags_defined = RFLAGS_DEFINED;
            let (mut steps, mut distinct_seen, mut last_k) = (0u64, 0u64, 0u64);
            let (mut jit_steps, mut fallback) = (0u64, 0u64);
            start.wait();
            loop {
                let seen = published.load(Ordering::Acquire);
                state.rip = CODE_RIP;
                state.gpr[Register::Rax as usize] = 0;
                let report = run_paged_host_loop_shared(
                    &mut session,
                    &control,
                    config(),
                    &mut state,
                    bus,
                    1,
                    &mut rflags_defined,
                    1,
                    |_| {},
                    || false,
                );
                assert_eq!(report.exit, PagedHostLoopExit::BudgetExhausted);
                jit_steps += report.steps_jit;
                fallback += report.steps_interpreter;
                steps += 1;
                b_steps.store(steps, Ordering::Release);
                let k = state.gpr[Register::Rax as usize]
                    .checked_sub(SLOT_BASE)
                    .filter(|k| *k <= WRITES)
                    .unwrap_or_else(|| {
                        panic!(
                            "step {steps}: RAX {:#x} names no instruction",
                            state.gpr[Register::Rax as usize]
                        )
                    });
                assert!(
                    k >= seen,
                    "D1: after the Acquire read of {seen} published writes the step ran the \
                     instruction of write {k}"
                );
                assert!(
                    k >= last_k,
                    "the instruction went back from {last_k} to {k}"
                );
                distinct_seen += u64::from(k != last_k);
                last_k = k;
                if seen == WRITES && k == WRITES {
                    break;
                }
            }
            (
                steps,
                distinct_seen,
                last_k,
                jit_steps,
                fallback,
                session.admissions(),
            )
        });
        let a = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_a);
            let control = PagedHostLoopControl::new();
            let mut state = make_state(0, CODE_RIP, 0);
            let mut rflags_defined = RFLAGS_DEFINED;
            start.wait();
            for k in 1..=WRITES {
                state.rip = A_CODE_RIP;
                state.gpr[Register::Rax as usize] = qword(load_code(k));
                let report = run_paged_host_loop_shared(
                    &mut session,
                    &control,
                    config(),
                    &mut state,
                    bus,
                    0,
                    &mut rflags_defined,
                    1,
                    |_| {},
                    || false,
                );
                assert_eq!(report.exit, PagedHostLoopExit::BudgetExhausted);
                assert_eq!(report.steps_jit, 1, "A's store ran natively");
                // The write committed (its generation was raised with Release): publish it.
                published.store(k, Ordering::Release);
                if lockstep {
                    // Let B take at least two steps (and so see write k) before write k + 1.
                    let target = b_steps.load(Ordering::Acquire) + 2;
                    let deadline = Instant::now() + WAIT;
                    while b_steps.load(Ordering::Acquire) < target {
                        assert!(Instant::now() < deadline, "B does not step");
                        std::hint::spin_loop();
                    }
                }
            }
        });
        a.join().unwrap();
        b.join().unwrap()
    });
    drop(region);
    let (steps, distinct_seen, final_k, jit_steps_b, fallback_b, b_admissions) = b_result;
    LockFreeRun {
        steps,
        distinct_seen,
        final_k,
        jit_steps_b,
        fallback_b,
        b_admissions,
    }
}

#[test]
fn s6_42b_b_runs_the_new_code_on_the_first_step_after_the_acquire_read_without_a_bus_lock() {
    let _guard = jit_guard();
    let mut total_steps = 0;
    for lockstep in [false, true] {
        let run = run_lock_free_pair(lockstep);
        assert_eq!(run.final_k, WRITES);
        assert_eq!(run.fallback_b, 0, "B ran native steps only");
        assert_eq!(run.jit_steps_b, run.steps, "every step of B was native");
        assert!(run.distinct_seen >= 1);
        if lockstep {
            assert_eq!(run.distinct_seen, WRITES, "B saw every instruction A wrote");
        }
        // A block is reused while the page is unchanged and compiled again after a write: the
        // admissions of B are fewer than its steps and at least one per instruction B ran.
        assert!(run.b_admissions >= run.distinct_seen);
        total_steps += run.steps;
        eprintln!(
            "S6-42b D1 (no bus lock, lockstep {lockstep}): B ran {} native steps, saw {} of {} \
             instructions, {} admissions, fallback 0%",
            run.steps, run.distinct_seen, WRITES, run.b_admissions
        );
    }
    assert!(total_steps > 0);
}

// ---------------------------------------------------------------------------
// The vCPU threads of ParallelControl.
// ---------------------------------------------------------------------------

/// vCPU B's loop: `mov [rbx],rax` (native store), `push r12`, `ret`; the new code (written by vCPU
/// A) loads in place of the store; the alias is the store the interpreter implements.
const B_OLD: [u8; 6] = [0x48, 0x89, 0x03, 0x41, 0x54, 0xc3];
const B_OLD_ALIAS: [u8; 6] = [0x48, 0x89, 0x0b, 0x41, 0x54, 0xc3];
const B_NEW: [u8; 8] = [0x48, 0x8b, 0x03, 0x41, 0x54, 0xc3, 0, 0];
/// vCPU A: `mov [rbx],rax; hlt`.
const A_PROGRAM: [u8; 4] = [0x48, 0x89, 0x03, 0xf4];
const MARKER: u64 = 0xBEEF_0000_0000_BEEF;

fn b_state() -> LongModeState {
    let mut state = loop_state();
    state.gpr[Register::Rax as usize] = INITIAL_DATA;
    state
}

/// vCPU A asleep in `HLT` (IF = 0) with the registers of its store.
fn a_halted_state() -> LongModeState {
    let mut state = make_state(qword(B_NEW), CODE_RIP, 0);
    state.rip = A_CODE_RIP;
    state.rflags = 2;
    state.activity = Some(od_isa::state::CpuActivity::Halted);
    state
}

fn board_ram(control: &od_machine::HostParallelControl<'_, '_>) -> Vec<u8> {
    let mut bytes = vec![0u8; RAM_BYTES as usize];
    control
        .bus()
        .atomic_inline_memory()
        .load_bytes(GuestPhysicalAddress::new(0), &mut bytes)
        .unwrap();
    bytes
}

fn data_qword(control: &od_machine::HostParallelControl<'_, '_>) -> u64 {
    control
        .bus()
        .atomic_inline_memory()
        .load_u64(GuestPhysicalAddress::new(DATA_QWORD))
        .unwrap()
}

/// The pure interpreter at the same retired count: B's loop is private (its own data cell, its own
/// stack), so the state of B at any stop is determined by the number of instructions it retired.
fn oracle(retired: u64) -> Observation {
    let mut bus = make_bus(&B_OLD_ALIAS);
    add_code_a(&mut bus, &A_PROGRAM);
    let mut state = b_state();
    let result = od_machine::run_paged_probe(
        &mut state,
        &mut bus,
        CR3,
        EFER,
        context(),
        retired,
        RFLAGS_DEFINED,
        || false,
        |_| {},
    );
    assert_eq!(result.instructions_executed, retired);
    observe(&state, &mut bus, true)
}

/// Both vCPU threads of `ParallelControl` run the JIT host loop over the board's `&MachineBus`:
/// `stop_all` acknowledges on safe points (nothing retires while stopped, B's state equals the
/// interpreter's at the retired count), vCPU A (woken by the controller during a stop) writes new
/// code over B's loop with a native store while B runs natively, and B then runs the new code: the
/// controller's marker in B's data cell, which the old store would have overwritten, reaches RAX.
#[test]
fn s6_42b_parallel_control_stops_jit_vcpus_and_a_changes_code_b_executes() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 4).unwrap();
    let parts = vec![
        Some(region.take_part().unwrap()),
        Some(region.take_part().unwrap()),
    ];
    let quantum = JitHostQuantum::new(parts, vec![config(), config()], RFLAGS_DEFINED);
    let mut board = host_parallel_board(2).unwrap();
    populate_tables(board.bus_mut(), &B_OLD);
    add_code_a(board.bus_mut(), &A_PROGRAM);
    assert!(host_install_vcpu(&mut board, 0, b_state()));
    assert!(host_install_vcpu(&mut board, 1, a_halted_state()));

    let seen = Mutex::new(Vec::<String>::new());
    let (report, ()) = run_host_threads(
        &mut board,
        HostParallelConfig {
            // A quantum far longer than a stop may wait: every stop lands inside a call and
            // ends it through the run's stop poll.
            quantum: 4096,
            budget: u64::MAX,
            end_when_idle: false,
        },
        &|call| quantum.run(call),
        &|_| quantum.thread_exit(),
        |control| {
            // Phase 1: B runs natively; A sleeps.
            assert!(
                control.wait_until(WAIT, || {
                    control.retired()[0] >= 3000 && quantum.stats(0).steps_jit >= 500
                }),
                "B does not run: {:?}",
                quantum.stats(0)
            );
            let proof = control.stop_all(WAIT).expect("every vCPU acknowledges");
            assert_eq!(proof.acknowledged, 2);
            let at_stop = control.retired();
            let b_stopped = control.with_state(0, |state| state.clone()).unwrap();
            let ram = board_ram(control);
            // S1: while the stop stands nothing retires and nothing is written.
            std::thread::sleep(Duration::from_millis(40));
            assert_eq!(control.retired(), at_stop, "nothing retires while stopped");
            assert_eq!(board_ram(control), ram, "RAM does not change while stopped");
            assert_eq!(
                control.with_state(0, |state| state.clone()).unwrap(),
                b_stopped
            );
            // B's state equals the interpreter's after the same number of instructions.
            let expected = oracle(at_stop[0]);
            let mut actual = Observation {
                gpr: b_stopped.gpr,
                rip: b_stopped.rip,
                rflags: b_stopped.rflags & RFLAGS_DEFINED,
                ram: Vec::new(),
            };
            for address in (0..RAM_BYTES).step_by(8) {
                if (CODE_PHYSICAL..CODE_PHYSICAL + 0x1000).contains(&address) {
                    continue;
                }
                let at = address as usize;
                actual
                    .ram
                    .push(u64::from_le_bytes(ram[at..at + 8].try_into().unwrap()));
            }
            assert_same(&actual, &expected, "vCPU B at the first stop");
            seen.lock()
                .unwrap()
                .push(format!("stop 1: B retired {} (= interpreter)", at_stop[0]));

            // Phase 2: wake A during the stop. It writes the new code over B's loop (a native
            // store into the page B executes from), halts again; B runs on.
            control
                .with_state(1, |state| {
                    state.activity = Some(od_isa::state::CpuActivity::Running)
                })
                .unwrap();
            control.resume();
            assert!(
                control.wait_until(WAIT, || control.retired()[1] >= 2 && control.halted(1)),
                "A does not finish its store"
            );
            // Phase 3: stop again, put the marker in B's data cell, run B on the new code.
            control.stop_all(WAIT).expect("second stop");
            let new_code = control
                .bus()
                .atomic_inline_memory()
                .load_u64(GuestPhysicalAddress::new(CODE_PHYSICAL))
                .unwrap();
            assert_eq!(new_code, qword(B_NEW), "A's store reached B's code page");
            control
                .bus()
                .atomic_inline_memory()
                .store_u64(GuestPhysicalAddress::new(DATA_QWORD), MARKER, |_| {})
                .unwrap();
            let before = (control.retired()[0], quantum.stats(0));
            control.resume();
            assert!(
                control.wait_until(WAIT, || control.retired()[0] >= before.0 + 600),
                "B does not run after the change"
            );
            control.stop_all(WAIT).expect("third stop");
            let rax = control
                .with_state(0, |state| state.gpr[Register::Rax as usize])
                .unwrap();
            // The old code (store) would have written its RAX over the marker; the new code
            // (load) read it: the marker is in RAX and still in the cell.
            assert_eq!(rax, MARKER, "B runs the new code (RAX)");
            assert_eq!(data_qword(control), MARKER, "B stores nothing any more");
            let after = quantum.stats(0);
            assert!(after.steps_jit > before.1.steps_jit, "B stayed native");
            seen.lock().unwrap().push(format!(
                "B native steps {} -> {}, interpreter steps {}, admissions {} -> {}",
                before.1.steps_jit,
                after.steps_jit,
                after.steps_interpreter,
                before.1.admissions,
                after.admissions
            ));
            control.resume();
        },
    )
    .expect("the board runs");
    assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
    assert_eq!(report.threads_started, 2);
    assert_eq!(report.threads_exited, 2, "every thread is joined");
    let (b, a) = (quantum.stats(0), quantum.stats(1));
    assert!(b.steps_jit > 0 && a.steps_jit >= 1, "{b:?} {a:?}");
    assert!(b.stop_exits >= 3, "B left its loop on the stop poll: {b:?}");
    for line in seen.lock().unwrap().iter() {
        eprintln!("S6-42b ParallelControl: {line}");
    }
    eprintln!(
        "S6-42b ParallelControl: B native {} interpreter {} (fallback {}%), A native {}",
        b.steps_jit,
        b.steps_interpreter,
        b.steps_interpreter * 100 / (b.steps_jit + b.steps_interpreter),
        a.steps_jit
    );
    drop(quantum);
    drop(region);
    // The threads released their leases themselves: a new region is available at once.
    drop(SharedJitRegion::new(1, 1).expect("the mapping is free after the run"));
}

/// M1 of the review of S6-42b: the session and the part of the region of a vCPU thread are dropped
/// on that thread before `run_host_threads` returns (the exit hook), not by a `thread_local!`
/// destructor that `thread::scope` does not wait for. So a region made right after a run, with the
/// parent's own handles dropped, never meets the previous one still live
/// (`MappingAlreadyLive`), run after run.
#[test]
fn s6_42b_the_thread_exit_releases_the_region_before_the_run_returns() {
    let _guard = jit_guard();
    // Per exit hook: did the thread hold a session before the hook, and does it after.
    let held_before = AtomicU64::new(0);
    let held_after = AtomicU64::new(0);
    for round in 0..24 {
        let region = SharedJitRegion::new(2, 2)
            .unwrap_or_else(|error| panic!("round {round}: the region is not free: {error:?}"));
        let parts = vec![
            Some(region.take_part().unwrap()),
            Some(region.take_part().unwrap()),
        ];
        let quantum = JitHostQuantum::new(parts, vec![config(), config()], RFLAGS_DEFINED);
        let mut board = host_parallel_board(2).unwrap();
        populate_tables(board.bus_mut(), &B_OLD);
        add_code_a(board.bus_mut(), &A_PROGRAM);
        assert!(host_install_vcpu(&mut board, 0, b_state()));
        assert!(host_install_vcpu(&mut board, 1, a_halted_state()));
        let (report, ()) = run_host_threads(
            &mut board,
            HostParallelConfig {
                quantum: 256,
                budget: u64::MAX,
                end_when_idle: false,
            },
            &|call| quantum.run(call),
            &|_| {
                if crate::host_threads::thread_slot_occupied() {
                    held_before.fetch_add(1, Ordering::SeqCst);
                }
                quantum.thread_exit();
                if crate::host_threads::thread_slot_occupied() {
                    held_after.fetch_add(1, Ordering::SeqCst);
                }
            },
            |control| {
                assert!(
                    control.wait_until(WAIT, || quantum.stats(0).steps_jit >= 8),
                    "round {round}: B does not run natively: {:?}",
                    quantum.stats(0)
                );
            },
        )
        .expect("the board runs");
        assert_eq!(report.finished, HostParallelFinish::Stopped, "{report:?}");
        assert_eq!(report.threads_exited, 2);
        drop(quantum);
        drop(region);
        // The next region (and its mapping) is available at once.
        let next = SharedJitRegion::new(1, 1)
            .unwrap_or_else(|error| panic!("round {round}: the mapping is still live: {error:?}"));
        drop(next);
    }
    // The thread of B held its session when the hook ran, in every round, and the hook released
    // it on that thread (a destructor would have found it still there at the end of the hook).
    assert!(held_before.load(Ordering::SeqCst) >= 24);
    assert_eq!(held_after.load(Ordering::SeqCst), 0);
}
