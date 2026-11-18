//! S6-42: several vCPU threads, each with its own session and its own part of the
//! process `MAP_JIT` region (JIT_CONTRACT S6-40: A1 to A4, D1, M1 to M6, S1, S3).
//!
//! The vCPU threads share one `MachineBus` behind a `Mutex`: the paged JIT path
//! and the shared interpreter take `&mut MachineBus`, and a shared-reference
//! variant of them does not exist yet (open, S6-42 report). A thread holds the
//! lock for one call of the loop (or one step), so steps of different vCPUs
//! interleave, and the RAM (with the page generations of `commit_write`) is one
//! object that every thread reads and writes. What runs truly in parallel is the
//! executable memory: slot writes, `ISB` and native calls of different parts
//! (`s6_42_threads_write_and_run_their_own_parts_concurrently`).

use super::*;
use crate::jit_region::{JitRegionError, JitRegionPart, SharedJitRegion};
use crate::{ARENA_SLOT_BYTES, ExecutableMemoryError, PublishedCode, map_jit_region_live};
use std::sync::atomic::AtomicU64;
use std::sync::{Barrier, Mutex, PoisonError, mpsc};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Compile-time ownership checks (A1): what must stay on its thread is `!Send`.
// ---------------------------------------------------------------------------

trait AmbiguousIfSend<A> {
    fn some_item() {}
}
impl<T: ?Sized> AmbiguousIfSend<()> for T {}
impl<T: ?Sized + Send> AmbiguousIfSend<u8> for T {}

/// Fails to compile when `$t` is `Send`.
macro_rules! assert_not_send {
    ($t:ty) => {
        let _ = <$t as AmbiguousIfSend<_>>::some_item;
    };
}

fn assert_send<T: Send>() {}
fn assert_sync<T: Sync>() {}

// ---------------------------------------------------------------------------
// Raw blocks for the region-level tests: `movz x0, #tag; ret`.
// ---------------------------------------------------------------------------

fn tiny(tag: u16) -> [u8; 8] {
    let mut bytes = [0u8; 8];
    bytes[..4].copy_from_slice(&(0xD280_0000u32 | (u32::from(tag) << 5)).to_le_bytes());
    bytes[4..].copy_from_slice(&0xD65F_03C0u32.to_le_bytes());
    bytes
}

fn call(code: &PublishedCode) -> u64 {
    // SAFETY: the bytes are `movz x0, #tag; ret`, published by this test into a
    // slot that stays owned by `code` for the whole call.
    let entry: extern "C" fn() -> u64 = unsafe { std::mem::transmute(code.as_ptr()) };
    entry()
}

fn slot_generation(code: &PublishedCode) -> u64 {
    match &code.region {
        crate::CodeRegion::Slot(lease) => lease.generation(),
        crate::CodeRegion::Owned(_) => panic!("not a slot block"),
    }
}

// ---------------------------------------------------------------------------
// The S3-38o tests again, on the slot backend (every session of them publishes
// into a slot of one shared region instead of a region per admission).
// ---------------------------------------------------------------------------

macro_rules! on_the_slot_backend {
    ($($name:ident => $test:ident),* $(,)?) => {
        $(
            #[test]
            fn $name() {
                with_slot_backend(super::$test);
            }
        )*
    };
}

on_the_slot_backend! {
    s6_42_slot_backend_mixed_program_with_a_guest_loop =>
        s3_38o_mixed_program_with_a_guest_loop_matches_the_interpreter_on_both_paths,
    s6_42_slot_backend_budgets_zero_to_n =>
        s3_38o_budgets_zero_to_n_on_the_mixed_program_equal_the_interpreter,
    s6_42_slot_backend_flag_before_the_reread =>
        s3_38o_flag_raised_before_the_reread_runs_no_step,
    s6_42_slot_backend_stop_after_the_reread =>
        s3_38o_stop_after_the_reread_is_a_normal_exit_without_replay,
    s6_42_slot_backend_stop_before_a_native_store =>
        s3_38o_stop_before_a_native_store_with_a_new_value_leaves_no_data_trace,
    s6_42_slot_backend_byte_without_a_flag =>
        s3_38o_a_byte_without_a_flag_is_harmless,
    s6_42_slot_backend_external_stop_from_another_thread =>
        s3_38o_external_stop_from_another_thread_in_an_endless_guest_loop,
    s6_42_slot_backend_event_and_stop_priority =>
        s3_38o_event_exits_on_the_safe_point_before_the_next_step_and_stop_wins,
    s6_42_slot_backend_code_page_write_by_the_interpreter =>
        s3_38o_code_page_write_by_the_interpreter_is_seen_by_the_next_lookup,
    s6_42_slot_backend_code_page_write_by_the_native_store =>
        s3_38o_code_page_write_by_the_native_store_is_seen_by_the_next_lookup,
    s6_42_slot_backend_guest_fault_of_the_native_form =>
        s3_38o_guest_fault_of_the_native_form_equals_the_interpreters,
}

// ---------------------------------------------------------------------------
// M1, M2, M6: one region, disjoint parts handed out once, the process lease.
// ---------------------------------------------------------------------------

#[test]
fn s6_42_region_parts_are_disjoint_handed_out_once_and_hold_the_process_lease() {
    let _guard = jit_guard();
    // A1: the executable handles stay on their thread; the owner and the part
    // before activation may cross threads (M1, M2).
    assert_not_send!(crate::PublishedCode);
    assert_not_send!(crate::jit_region::ActiveJitPart);
    assert_not_send!(PagedFetchedLoadSession);
    assert_not_send!(crate::PublishedArenaBackend);
    assert_sync::<SharedJitRegion>();
    assert_send::<SharedJitRegion>();
    assert_send::<JitRegionPart>();

    assert!(!map_jit_region_live());
    assert_eq!(
        SharedJitRegion::new(0, 1).err(),
        Some(JitRegionError::InvalidShape {
            parts: 0,
            slots_per_part: 1
        })
    );
    assert!(matches!(
        SharedJitRegion::new(1, 0),
        Err(JitRegionError::InvalidShape { .. })
    ));
    assert!(!map_jit_region_live(), "a refused shape maps nothing");

    let region = SharedJitRegion::new(3, 2).unwrap();
    assert!(map_jit_region_live());
    assert_eq!((region.parts(), region.slots_per_part()), (3, 2));

    // M6: while the region of the machine lives, every other owner is busy.
    assert_eq!(
        PublishedCode::publish(&tiny(1)).err(),
        Some(ExecutableMemoryError::MappingAlreadyLive)
    );
    assert!(matches!(
        crate::PublishedArenaBackend::new(1),
        Err(crate::PublishedArenaError::MappingAlreadyLive)
    ));
    assert_eq!(
        SharedJitRegion::new(1, 1).err(),
        Some(JitRegionError::Mapping(
            ExecutableMemoryError::MappingAlreadyLive
        ))
    );
    let mut plain = PagedFetchedLoadSession::new();
    let mut bus = make_bus(&LOOP_ALIAS);
    let mut state = loop_state();
    let refusal = plain
        .execute_forms(&mut state, &mut bus, CR3, EFER, context())
        .expect_err("a session without a part cannot map a second region");
    assert_eq!(
        refusal.kind,
        crate::PagedFetchedLoadErrorKind::Publish(ExecutableMemoryError::MappingAlreadyLive)
    );

    // M2: each part once, disjoint, inside the one mapping.
    let parts: Vec<JitRegionPart> = (0..3).map(|_| region.take_part().unwrap()).collect();
    assert_eq!(region.take_part().err(), Some(JitRegionError::NoPartLeft));
    let mut ranges: Vec<(usize, usize)> = parts.iter().map(JitRegionPart::address_range).collect();
    ranges.sort_unstable();
    for pair in ranges.windows(2) {
        assert!(
            pair[0].0 + pair[0].1 <= pair[1].0,
            "parts overlap: {pair:?}"
        );
    }
    assert_eq!(
        parts.iter().map(JitRegionPart::index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    for part in &parts {
        assert_eq!(part.slots(), 2);
    }

    // M1: the mapping lives until the owner and every part are gone.
    drop(region);
    assert!(map_jit_region_live(), "parts keep the mapping alive");
    drop(parts);
    assert!(!map_jit_region_live(), "the lease ends with the last part");
    drop(PublishedCode::publish(&tiny(1)).expect("the lease is free again"));
}

// ---------------------------------------------------------------------------
// A4: slots are released by their owner and reused with a new generation.
// ---------------------------------------------------------------------------

#[test]
fn s6_42_slot_is_released_by_the_owner_and_reused_with_a_new_generation() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(1, 2).unwrap();
    let part = region.take_part().unwrap().activate();
    let first = part.publish(&tiny(1)).unwrap();
    let second = part.publish(&tiny(2)).unwrap();
    assert_eq!((call(&first), call(&second)), (1, 2));
    assert_eq!(
        part.publish(&tiny(3)).err(),
        Some(ExecutableMemoryError::AllocationFailed),
        "a part never takes a slot of another part"
    );
    assert_eq!(part.stats().failed_allocations, 1);
    assert_eq!(
        part.publish(&[]).err(),
        Some(ExecutableMemoryError::InvalidBlockArtifact)
    );
    assert_eq!(
        part.publish(&vec![0u8; ARENA_SLOT_BYTES + 1]).err(),
        Some(ExecutableMemoryError::InvalidBlockArtifact)
    );

    let first_entry = first.as_ptr();
    let first_generation = slot_generation(&first);
    drop(first);
    let stats = part.stats();
    assert_eq!(
        (stats.live, stats.releases, stats.pending_release),
        (1, 1, 0)
    );

    // The freed slot is reused, with a generation that grew; the neighbour
    // slot kept running code the whole time.
    let third = part.publish(&tiny(3)).unwrap();
    assert_eq!(third.as_ptr(), first_entry);
    assert_eq!(slot_generation(&third), first_generation + 1);
    assert_eq!((call(&third), call(&second)), (3, 2));

    drop((second, third));
    part.safe_point();
    let stats = part.stats();
    assert_eq!(
        (stats.live, stats.pending_release, stats.max_slot_generation),
        (0, 0, 2)
    );
    assert_eq!((stats.allocations, stats.releases), (3, 3));
}

// ---------------------------------------------------------------------------
// M3: writes of one part run beside native calls of the other parts.
// ---------------------------------------------------------------------------

#[test]
fn s6_42_threads_write_and_run_their_own_parts_concurrently() {
    const THREADS: usize = 4;
    const ITERATIONS: u64 = 4000;
    let _guard = jit_guard();
    let region = SharedJitRegion::new(THREADS, 3).unwrap();
    let parts: Vec<JitRegionPart> = (0..THREADS).map(|_| region.take_part().unwrap()).collect();
    let start = Barrier::new(THREADS);
    let stats = std::thread::scope(|scope| {
        let handles: Vec<_> = parts
            .into_iter()
            .enumerate()
            .map(|(thread, part)| {
                let start = &start;
                scope.spawn(move || {
                    let part = part.activate();
                    let resident_tag = 0x7000 + thread as u16;
                    let resident = part.publish(&tiny(resident_tag)).unwrap();
                    start.wait();
                    for iteration in 0..ITERATIONS {
                        let tag = ((thread as u16) << 12) | (iteration as u16 & 0x0fff);
                        // Publication (a write window of this thread) while the
                        // other threads run their blocks.
                        let block = part.publish(&tiny(tag)).unwrap();
                        assert_eq!(call(&block), u64::from(tag), "thread {thread}");
                        assert_eq!(call(&resident), u64::from(resident_tag));
                        drop(block);
                    }
                    part.safe_point();
                    let stats = part.stats();
                    drop(resident);
                    (stats, part.stats())
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("no thread trapped"))
            .collect::<Vec<_>>()
    });
    for (thread, (with_resident, after)) in stats.into_iter().enumerate() {
        assert_eq!(with_resident.allocations, ITERATIONS + 1, "thread {thread}");
        assert_eq!(with_resident.releases, ITERATIONS, "thread {thread}");
        assert_eq!(with_resident.live, 1);
        assert_eq!(with_resident.pending_release, 0);
        // One churn slot reused every time: its generation counts the uses.
        assert_eq!(with_resident.max_slot_generation, ITERATIONS);
        assert_eq!((after.live, after.pending_release), (0, 0));
    }
    drop(region);
    assert!(!map_jit_region_live());
}

// ---------------------------------------------------------------------------
// The session on a slot, and a session without a part beside it (M6).
// ---------------------------------------------------------------------------

#[test]
fn s6_42_session_publishes_into_a_slot_and_a_plain_session_falls_back_to_the_interpreter() {
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 2).unwrap();
    let mut slot_session = PagedFetchedLoadSession::with_region_part(region.take_part().unwrap());
    let control = PagedHostLoopControl::new();
    let reference_run = reference(&LOOP_ALIAS, &loop_state(), 9, true);

    let mut bus = make_bus(&LOOP_ALIAS);
    let mut state = loop_state();
    let mut rflags_defined = RFLAGS_DEFINED;
    let run = run_loop(
        &mut slot_session,
        &control,
        &mut bus,
        &mut state,
        &mut rflags_defined,
        9,
    );
    assert_eq!(run.report.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(run.report.retired_total(), 9);
    assert!(
        run.report.retired_jit > 0,
        "the slot session ran native code"
    );
    assert_eq!(observe(&state, &mut bus, true), reference_run.observation);
    assert_eq!(run.writes, reference_run.writes);
    let stats = slot_session.region_part().unwrap().stats();
    assert_eq!(stats.allocations, slot_session.admissions());
    assert!(stats.live <= 1 && stats.pending_release == 0);
    assert!(map_jit_region_live());

    // M6: a session without a part, in the same process, cannot map a region of
    // its own; every native attempt is a refusal and the interpreter runs the
    // whole program with the same result.
    let mut plain = PagedFetchedLoadSession::new();
    let mut bus = make_bus(&LOOP_ALIAS);
    let mut state = loop_state();
    let mut rflags_defined = RFLAGS_DEFINED;
    let run = run_loop(
        &mut plain,
        &control,
        &mut bus,
        &mut state,
        &mut rflags_defined,
        9,
    );
    assert_eq!(run.report.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(run.report.retired_jit, 0);
    assert_eq!(run.report.retired_interpreter, 9);
    assert!(run.report.jit_refusals > 0);
    assert_eq!(plain.admissions(), 0);
    assert_eq!(observe(&state, &mut bus, true), reference_run.observation);
    assert_eq!(run.writes, reference_run.writes);

    // The session's reset releases its slot, not the region.
    assert!(slot_session.reset());
    assert_eq!(slot_session.region_part().unwrap().stats().live, 0);
    assert!(map_jit_region_live());
    drop(slot_session);
    drop(region);
    assert!(!map_jit_region_live());
}

// ---------------------------------------------------------------------------
// vCPU threads over one shared bus.
// ---------------------------------------------------------------------------

/// vCPU A's code (a native store `mov [rbx], rax`) lives at linear 0x402000.
const A_CODE_RIP: u64 = 0x0040_2000;
const A_CODE_PHYSICAL: u64 = 0xa000;
/// vCPU B's code at `CODE_RIP`: `mov rax,[rbx]; push r12; ret`, the loop of the
/// scripted test (two native steps and four interpreter steps per two turns).
const B_LOOP_OLD: [u8; 6] = [0x48, 0x8b, 0x03, 0x41, 0x54, 0xc3];
/// The same loop with the store in place of the load.
const B_LOOP_NEW: [u8; 8] = [0x48, 0x89, 0x03, 0x41, 0x54, 0xc3, 0, 0];
const B_LOOP_NEW_ALIAS: [u8; 8] = [0x48, 0x89, 0x0b, 0x41, 0x54, 0xc3, 0, 0];
/// The instruction forms alone, for the free-running tests.
const FORM_LOAD: [u8; 8] = [0x48, 0x8b, 0x03, 0, 0, 0, 0, 0];
const FORM_STORE: [u8; 8] = [0x48, 0x89, 0x03, 0, 0, 0, 0, 0];
const FORM_STORE_ALIAS: [u8; 8] = [0x48, 0x89, 0x0b, 0, 0, 0, 0, 0];

fn qword(bytes: [u8; 8]) -> u64 {
    u64::from_le_bytes(bytes)
}

/// The bus of the machine: B's code at `CODE_RIP`, A's code at `A_CODE_RIP` (the
/// same tables otherwise).
fn make_two_code_bus(code_b: &[u8], code_a: &[u8]) -> MachineBus {
    let mut bus = make_bus(code_b);
    // PT entry of linear 0x402000 -> physical 0xa000 (present, writable, user).
    put_qword(&mut bus, 0x6010, A_CODE_PHYSICAL | 0x7);
    bus.patch_physical_ram(GuestPhysicalAddress::new(A_CODE_PHYSICAL), code_a)
        .unwrap();
    bus
}

/// vCPU A: store RAX at linear `target` (`rbx`), code at `A_CODE_RIP`.
fn a_state(rax: u64, rcx: u64, target: u64) -> LongModeState {
    let mut state = make_state(rax, target, rcx);
    state.rip = A_CODE_RIP;
    state
}

fn lock_bus(bus: &Mutex<MachineBus>) -> std::sync::MutexGuard<'_, MachineBus> {
    bus.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers, RIP, flags and every RAM qword except the pages in `skip`.
fn observe_without(state: &LongModeState, bus: &mut MachineBus, skip_pages: &[u64]) -> Observation {
    Observation {
        gpr: state.gpr,
        rip: state.rip,
        rflags: state.rflags & RFLAGS_DEFINED,
        ram: (0..RAM_BYTES)
            .step_by(8)
            .filter(|address| !skip_pages.contains(&(address & !0xfff)))
            .map(|address| {
                bus.read_physical(GuestPhysicalAddress::new(address), AccessWidth::Qword)
                    .unwrap()
            })
            .collect(),
    }
}

struct Phase {
    report: PagedHostLoopReport,
    writes: Vec<(u64, u64)>,
}

fn run_phase(
    session: &mut PagedFetchedLoadSession,
    control: &PagedHostLoopControl,
    bus: &Mutex<MachineBus>,
    state: &mut LongModeState,
    rflags_defined: &mut u64,
    budget: u64,
) -> Phase {
    let mut bus = lock_bus(bus);
    let run = run_loop(session, control, &mut bus, state, rflags_defined, budget);
    Phase {
        report: run.report,
        writes: run.writes,
    }
}

/// Test (1) of the plan, scripted: vCPU A changes the code that vCPU B executes,
/// B stops executing the old code, and the two threads together equal the
/// interpreter at the same schedule.
#[test]
fn s6_42_vcpu_a_changes_code_vcpu_b_executes_it_and_leaves_the_old_code() {
    const STEPS: u64 = 6;
    let _guard = jit_guard();
    let region = SharedJitRegion::new(2, 2).unwrap();
    let part_b = region.take_part().unwrap();
    let part_a = region.take_part().unwrap();
    let bus = Mutex::new(make_two_code_bus(&B_LOOP_OLD, &FORM_STORE));
    let (to_a, from_b) = mpsc::channel::<u64>();
    let (to_b, from_a) = mpsc::channel::<u64>();

    let (b_result, a_result) = std::thread::scope(|scope| {
        let bus = &bus;
        let b = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_b);
            let control = PagedHostLoopControl::new();
            let mut state = loop_state();
            let mut rflags_defined = RFLAGS_DEFINED;
            let before = run_phase(
                &mut session,
                &control,
                bus,
                &mut state,
                &mut rflags_defined,
                STEPS,
            );
            let generation_before = lock_bus(bus)
                .code_page_generation(GuestPhysicalAddress::new(CODE_PHYSICAL))
                .unwrap();
            let admissions_before = session.admissions();
            to_a.send(generation_before).unwrap();
            // A's write is committed before it answers.
            let generation_after = from_a.recv().unwrap();
            let after = run_phase(
                &mut session,
                &control,
                bus,
                &mut state,
                &mut rflags_defined,
                STEPS,
            );
            let part_stats = session.region_part().unwrap().stats();
            let admissions = session.admissions();
            (
                before,
                after,
                state,
                generation_before,
                generation_after,
                admissions_before,
                admissions,
                part_stats,
            )
        });
        let a = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_a);
            let control = PagedHostLoopControl::new();
            let mut state = a_state(qword(B_LOOP_NEW), 0, CODE_RIP);
            let mut rflags_defined = RFLAGS_DEFINED;
            let _ = from_b.recv().unwrap();
            let phase = run_phase(
                &mut session,
                &control,
                bus,
                &mut state,
                &mut rflags_defined,
                1,
            );
            let generation = lock_bus(bus)
                .code_page_generation(GuestPhysicalAddress::new(CODE_PHYSICAL))
                .unwrap();
            to_b.send(generation).unwrap();
            (phase, state, session.admissions())
        });
        (b.join().unwrap(), a.join().unwrap())
    });
    let (
        before,
        after,
        b_state,
        generation_before,
        generation_after,
        admissions_before,
        admissions,
        part_stats,
    ) = b_result;
    let (a_phase, a_state_after, a_admissions) = a_result;

    // The oracle: the pure interpreter, one thread, the same schedule (B for six
    // steps, A for one, B for six). The JIT forms have their interpreter alias.
    let mut ref_bus = make_two_code_bus(&B_LOOP_OLD, &FORM_STORE_ALIAS);
    let mut ref_b = loop_state();
    let mut ref_a = a_state(0, qword(B_LOOP_NEW_ALIAS), CODE_RIP);
    let mut ref_b_writes = Vec::new();
    let mut ref_a_writes = Vec::new();
    let mut defined = RFLAGS_DEFINED;
    let quantum = |state: &mut LongModeState,
                   bus: &mut MachineBus,
                   budget: u64,
                   defined: &mut u64,
                   writes: &mut Vec<(u64, u64)>| {
        let result = od_machine::run_paged_probe(
            state,
            bus,
            CR3,
            EFER,
            context(),
            budget,
            *defined,
            || false,
            |range| writes.push((range.start().get(), range.len())),
        );
        *defined = result.rflags_defined;
        assert_eq!(result.instructions_executed, budget);
    };
    quantum(
        &mut ref_b,
        &mut ref_bus,
        STEPS,
        &mut defined,
        &mut ref_b_writes,
    );
    let mut a_defined = RFLAGS_DEFINED;
    quantum(
        &mut ref_a,
        &mut ref_bus,
        1,
        &mut a_defined,
        &mut ref_a_writes,
    );
    quantum(
        &mut ref_b,
        &mut ref_bus,
        STEPS,
        &mut defined,
        &mut ref_b_writes,
    );

    // A wrote into B's code page: the generation grew, B saw it.
    assert!(generation_after > generation_before);
    assert_eq!(a_phase.report.exit, PagedHostLoopExit::BudgetExhausted);
    assert_eq!(a_phase.report.steps_jit, 1, "A's store ran natively");
    assert_eq!(a_admissions, 1);

    // B before the write: native load, after: native store. Both phases ran their native steps natively.
    for (name, phase) in [("before", &before), ("after", &after)] {
        assert_eq!(
            phase.report.exit,
            PagedHostLoopExit::BudgetExhausted,
            "{name}"
        );
        assert_eq!(phase.report.steps_jit, 2, "{name}: native steps");
        assert_eq!(
            phase.report.steps_interpreter, 4,
            "{name}: interpreter steps"
        );
        assert_eq!(phase.report.jit_refusals, 4, "{name}");
        assert!(phase.report.budget_is_consistent(), "{name}");
    }
    // The old code only loads; the new code stores a qword into the data page
    // (the pushes write elsewhere in the page). Two native stores after the
    // write, none before: B left the old block.
    let data_store = (DATA_QWORD, 8);
    assert_eq!(
        before.writes.iter().filter(|w| **w == data_store).count(),
        0
    );
    assert_eq!(after.writes.iter().filter(|w| **w == data_store).count(), 2);
    // Every native step of B compiled its own block: 2 before, 2 after.
    assert_eq!((admissions_before, admissions), (2, 4));
    // A4: the blocks of B were released by B, slot after slot; none is left.
    assert_eq!(part_stats.allocations, 4);
    assert_eq!((part_stats.live, part_stats.pending_release), (0, 0));
    assert_eq!(part_stats.releases, 4);

    // The same registers, flags, RAM and write lists as the interpreter.
    {
        let mut bus = lock_bus(&bus);
        let skip = [CODE_PHYSICAL, A_CODE_PHYSICAL];
        assert_same(
            &observe_without(&b_state, &mut bus, &skip),
            &observe_without(&ref_b, &mut ref_bus, &skip),
            "vCPU B",
        );
        // A's value register differs by construction (RAX for the JIT store,
        // RCX for its interpreter alias); everything else must match.
        let mut jit_a = a_state_after.clone();
        let mut alias_a = ref_a.clone();
        for state in [&mut jit_a, &mut alias_a] {
            state.gpr[Register::Rax as usize] = 0;
            state.gpr[Register::Rcx as usize] = 0;
        }
        assert_same(
            &observe_without(&jit_a, &mut bus, &skip),
            &observe_without(&alias_a, &mut ref_bus, &skip),
            "vCPU A",
        );
        // The code page holds the new bytes in both runs.
        assert_eq!(
            bus.read_physical(GuestPhysicalAddress::new(CODE_PHYSICAL), AccessWidth::Qword)
                .unwrap(),
            qword(B_LOOP_NEW)
        );
    }
    let mut jit_b_writes = before.writes.clone();
    jit_b_writes.extend(after.writes.iter().copied());
    assert_eq!(jit_b_writes, ref_b_writes, "B's committed writes");
    assert_eq!(a_phase.writes, ref_a_writes, "A's committed writes");

    let steps = before.report.retired_total() + after.report.retired_total();
    let fallback = before.report.retired_interpreter + after.report.retired_interpreter;
    eprintln!(
        "S6-42 scripted: vCPU B retired {steps}, native {}, interpreter fallback {fallback} ({}%)",
        steps - fallback,
        fallback * 100 / steps
    );
    drop(region);
}

/// The result of the free-running pair of threads.
struct FreeRun {
    native_old: u64,
    native_new: u64,
    b_admissions: u64,
    b_stats: crate::jit_region::JitPartStats,
    a_writes: u64,
    a_raw_blocks: u64,
    a_admissions: u64,
    a_stats: crate::jit_region::JitPartStats,
}

/// vCPU A rewrites the instruction at `CODE_RIP` with its own native stores,
/// alternating load and store, while vCPU B runs it step after step. Both are
/// threads; the bus is behind a lock. For every step of B (native only, no
/// fallback) the executed bytes and the page generation the block was compiled
/// from must equal what the page holds at that moment (what the interpreter
/// would fetch), and the effect must be that of the instruction: a load puts
/// `[rbx]` into RAX, a store puts RAX into `[rbx]`. With `raw_churn` vCPU A also
/// publishes, runs and frees raw blocks in its own part, outside the bus lock,
/// while B runs native code.
fn run_free_running(writes: u64, raw_churn: bool) -> FreeRun {
    let region = SharedJitRegion::new(2, 4).unwrap();
    let part_b = region.take_part().unwrap();
    let part_a = region.take_part().unwrap();
    let bus = Mutex::new(make_two_code_bus(&FORM_LOAD, &FORM_STORE));
    let latest_generation = AtomicU64::new(0);
    let a_done = AtomicBool::new(false);
    let start = Barrier::new(2);

    let (b, a) = std::thread::scope(|scope| {
        let (bus, latest_generation, a_done, start) = (&bus, &latest_generation, &a_done, &start);
        let b = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_b);
            let stop_byte = AtomicU8::new(0);
            let mut state = loop_state();
            let (mut native_old, mut native_new, mut counter) = (0u64, 0u64, 0u64);
            start.wait();
            while !a_done.load(Ordering::Acquire) || counter < 64 {
                // The generation A published; a block compiled from an older
                // generation must never run after this read (S6-40, 2.3).
                let seen_generation = latest_generation.load(Ordering::Acquire);
                counter += 1;
                let mut bus = lock_bus(bus);
                let page_generation = bus
                    .code_page_generation(GuestPhysicalAddress::new(CODE_PHYSICAL))
                    .unwrap();
                let page_bytes = bus
                    .read_physical(GuestPhysicalAddress::new(CODE_PHYSICAL), AccessWidth::Qword)
                    .unwrap()
                    .to_le_bytes();
                state.rip = CODE_RIP;
                // A value that differs from the data qword, so that load and
                // store leave different traces.
                let rax_before = counter.wrapping_mul(0x0101_0000_0000_0001) | 1;
                state.gpr[Register::Rax as usize] = rax_before;
                let data_before = bus
                    .read_physical(GuestPhysicalAddress::new(DATA_QWORD), AccessWidth::Qword)
                    .unwrap();
                assert_ne!(data_before, rax_before);
                let report = session
                    .execute_forms_with_stop(&mut state, &mut bus, CR3, EFER, context(), &stop_byte)
                    .unwrap_or_else(|error| panic!("the native path refused: {error:?}"));
                assert_eq!(
                    report.fetched_code,
                    page_bytes[..report.fetched_code.len()],
                    "the block was built from bytes the page no longer holds"
                );
                assert_eq!(report.fetched_generation, page_generation);
                assert!(report.fetched_generation >= seen_generation);
                let result = report.execution.callback_result.as_ref().unwrap();
                assert_eq!(result.exit, MemoryOperationExecutionExitV2::Complete);
                assert_eq!(result.retired_instructions, 1);
                let data_after = bus
                    .read_physical(GuestPhysicalAddress::new(DATA_QWORD), AccessWidth::Qword)
                    .unwrap();
                match report.form {
                    crate::PagedFetchedForm::QwordLoadRbx => {
                        assert_eq!(page_bytes[..3], FORM_LOAD[..3]);
                        assert_eq!(state.gpr[Register::Rax as usize], data_before);
                        assert_eq!(data_after, data_before);
                        native_old += 1;
                    }
                    crate::PagedFetchedForm::QwordStoreRbx => {
                        assert_eq!(page_bytes[..3], FORM_STORE[..3]);
                        assert_eq!(data_after, rax_before);
                        assert_eq!(state.gpr[Register::Rax as usize], rax_before);
                        native_new += 1;
                    }
                    other => panic!("unexpected form {other:?}"),
                }
                drop(bus);
                std::thread::sleep(Duration::from_micros(20));
            }
            let admissions = session.admissions();
            session.safe_point();
            let stats = session.region_part().unwrap().stats();
            assert!(session.reset());
            let after_reset = session.region_part().unwrap().stats();
            assert_eq!((after_reset.live, after_reset.pending_release), (0, 0));
            (native_old, native_new, admissions, stats)
        });
        let a = scope.spawn(move || {
            let mut session = PagedFetchedLoadSession::with_region_part(part_a);
            let part = session.region_part().unwrap().clone();
            let stop_byte = AtomicU8::new(0);
            let mut raw_blocks = 0u64;
            start.wait();
            for write in 0..writes {
                if raw_churn {
                    // Beside B's native calls: A's write windows and `ISB`, and
                    // its release of slots, in A's own part.
                    for burst in 0..4u16 {
                        let tag = (((write as u16) << 2) | burst) & 0x3fff;
                        let block = part.publish(&tiny(tag)).unwrap();
                        assert_eq!(call(&block), u64::from(tag));
                        raw_blocks += 1;
                    }
                }
                let value = if write % 2 == 0 {
                    qword(FORM_STORE)
                } else {
                    qword(FORM_LOAD)
                };
                let mut bus = lock_bus(bus);
                let mut state = a_state(value, 0, CODE_RIP);
                let report = session
                    .execute_forms_with_stop(&mut state, &mut bus, CR3, EFER, context(), &stop_byte)
                    .unwrap_or_else(|error| panic!("vCPU A's store refused: {error:?}"));
                assert_eq!(report.form, crate::PagedFetchedForm::QwordStoreRbx);
                let result = report.execution.callback_result.as_ref().unwrap();
                assert_eq!(result.exit, MemoryOperationExecutionExitV2::Complete);
                let generation = bus
                    .code_page_generation(GuestPhysicalAddress::new(CODE_PHYSICAL))
                    .unwrap();
                latest_generation.store(generation, Ordering::Release);
                drop(bus);
                std::thread::sleep(Duration::from_micros(20));
            }
            a_done.store(true, Ordering::Release);
            session.safe_point();
            (raw_blocks, session.admissions(), part.stats())
        });
        (b.join().unwrap(), a.join().unwrap())
    });
    drop(region);
    FreeRun {
        native_old: b.0,
        native_new: b.1,
        b_admissions: b.2,
        b_stats: b.3,
        a_writes: writes,
        a_raw_blocks: a.0,
        a_admissions: a.1,
        a_stats: a.2,
    }
}

/// Test (1b): B never runs a block built from bytes or from a generation older
/// than the page holds, whatever the interleaving; B runs both versions of the
/// code natively (no fallback: a run in which the JIT did not execute the
/// changing code would not count).
#[test]
fn s6_42_free_running_vcpus_never_execute_stale_code() {
    let _guard = jit_guard();
    let run = run_free_running(300, false);
    assert_eq!(run.a_writes, 300);
    assert!(
        run.native_old > 0 && run.native_new > 0,
        "both versions ran natively"
    );
    assert!(run.native_old + run.native_new >= 64);
    // Each change of the page made B compile again: at least one block per
    // observed change, never one block across a change (checked per step).
    assert!(
        run.b_admissions >= 3,
        "B re-admitted {} times",
        run.b_admissions
    );
    assert_eq!(run.b_stats.allocations, run.b_admissions);
    eprintln!(
        "S6-42 free-running: vCPU B native steps {} (load {}, store {}), blocks compiled {}, interpreter fallback 0 of {} (0%)",
        run.native_old + run.native_new,
        run.native_old,
        run.native_new,
        run.b_admissions,
        run.native_old + run.native_new
    );
}

/// Test (3): release under load. vCPU B compiles, runs and releases a block
/// whenever the page changes (publish, invalidate, release, in a loop) while
/// vCPU A churns raw blocks in its own part and rewrites B's code. Test builds
/// poison a released slot with `BRK`, so executing a released block would trap.
#[test]
fn s6_42_release_under_load_never_runs_a_released_slot() {
    let _guard = jit_guard();
    let run = run_free_running(400, true);
    assert!(run.native_old > 0 && run.native_new > 0);
    // B: one slot taken per admission, released by B on its own safe points.
    let b = run.b_stats;
    assert_eq!(b.allocations, run.b_admissions);
    assert!(b.releases + b.live as u64 == b.allocations, "{b:?}");
    assert!(b.live <= 1 && b.pending_release == 0, "{b:?}");
    // The reused slot's generation counts every use: a handle of an earlier use
    // can never be mistaken for the current one.
    assert_eq!(b.max_slot_generation, b.allocations, "{b:?}");
    assert!(b.allocations >= 3);
    // A: its own churn and its one session block, all in its own part.
    let a = run.a_stats;
    assert_eq!(a.allocations, run.a_raw_blocks + run.a_admissions);
    assert_eq!(a.releases + a.live as u64, a.allocations, "{a:?}");
    assert!(a.live <= 1 && a.pending_release == 0, "{a:?}");
    assert_eq!(a.failed_allocations, 0);
    assert_eq!(b.failed_allocations, 0);
    eprintln!(
        "S6-42 churn: vCPU B {} blocks published and released, vCPU A {} raw blocks, max slot generation {}",
        b.allocations, run.a_raw_blocks, b.max_slot_generation
    );
}

// ---------------------------------------------------------------------------
// S1, S3: stop_all with JIT loops on every vCPU.
// ---------------------------------------------------------------------------

struct StopSnapshot {
    retired: u64,
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}

struct VcpuShared {
    control: PagedHostLoopControl,
    retired: AtomicU64,
    snapshot: Mutex<Option<StopSnapshot>>,
}

/// S6-42 test (5): several vCPU threads run the host loop over their own JIT
/// sessions; `stop_all` waits for every acknowledgement, and an acknowledgement
/// is given only on the safe point after the loop (after the native call) returned.
/// While the stop is in force no vCPU retires an instruction; the state each
/// vCPU stopped in equals the interpreter's at the same retired count; after
/// `resume` every vCPU goes on.
#[test]
fn s6_42_stop_all_with_jit_vcpus_acknowledges_on_safe_points_and_resumes() {
    const VCPUS: usize = 3;
    const ROUNDS: usize = 6;
    let _guard = jit_guard();
    let region = SharedJitRegion::new(VCPUS, 2).unwrap();
    let parts: Vec<JitRegionPart> = (0..VCPUS).map(|_| region.take_part().unwrap()).collect();
    let bus = Mutex::new(make_bus(&LOOP_JIT_STORE));
    let vcpus: Vec<VcpuShared> = (0..VCPUS)
        .map(|_| VcpuShared {
            control: PagedHostLoopControl::new(),
            retired: AtomicU64::new(0),
            snapshot: Mutex::new(None),
        })
        .collect();

    let totals = std::thread::scope(|scope| {
        let bus = &bus;
        let handles: Vec<_> = parts
            .into_iter()
            .zip(&vcpus)
            .map(|(part, vcpu)| {
                scope.spawn(move || {
                    let mut session = PagedFetchedLoadSession::with_region_part(part);
                    let mut state = loop_state();
                    let mut rflags_defined = RFLAGS_DEFINED;
                    let (mut total, mut jit, mut acknowledged) = (0u64, 0u64, 0u64);
                    loop {
                        let report = {
                            let mut bus = lock_bus(bus);
                            run_loop(
                                &mut session,
                                &vcpu.control,
                                &mut bus,
                                &mut state,
                                &mut rflags_defined,
                                8,
                            )
                            .report
                        };
                        assert!(report.budget_is_consistent());
                        total += report.retired_total();
                        jit += report.retired_jit;
                        vcpu.retired.store(total, Ordering::SeqCst);
                        match report.exit {
                            PagedHostLoopExit::BudgetExhausted => {}
                            PagedHostLoopExit::StopRequested => {
                                // The safe point: the loop returned, so the native
                                // call is over and its result accounted; the whole
                                // state is in `state`.
                                *vcpu.snapshot.lock().unwrap() = Some(StopSnapshot {
                                    retired: total,
                                    gpr: state.gpr,
                                    rip: state.rip,
                                    rflags: state.rflags & RFLAGS_DEFINED,
                                });
                                vcpu.control.acknowledge_safe();
                                acknowledged += 1;
                                while vcpu.control.stop_pending() {
                                    std::thread::yield_now();
                                }
                                vcpu.control.withdraw_safe();
                            }
                            PagedHostLoopExit::FinishRequested => break,
                            other => panic!("unexpected exit {other:?}"),
                        }
                    }
                    session.safe_point();
                    let stats = session.region_part().unwrap().stats();
                    (total, jit, acknowledged, stats)
                })
            })
            .collect();

        let group = PagedVcpuGroup::new(vcpus.iter().map(|vcpu| &vcpu.control).collect());
        let mut last = vec![0u64; VCPUS];
        for round in 0..ROUNDS {
            // Every vCPU made progress since the last round (the resume worked).
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while vcpus
                .iter()
                .zip(&last)
                .any(|(vcpu, last)| vcpu.retired.load(Ordering::SeqCst) <= *last)
            {
                assert!(
                    std::time::Instant::now() < deadline,
                    "round {round}: no progress"
                );
                std::thread::yield_now();
            }
            group
                .stop_all(Duration::from_secs(20))
                .unwrap_or_else(|timeout| {
                    panic!("round {round}: lost acknowledgement {timeout:?}")
                });
            // The proof of the stop: nothing retires while it is in force.
            let frozen: Vec<u64> = vcpus
                .iter()
                .map(|vcpu| vcpu.retired.load(Ordering::SeqCst))
                .collect();
            std::thread::sleep(Duration::from_millis(5));
            let still: Vec<u64> = vcpus
                .iter()
                .map(|vcpu| vcpu.retired.load(Ordering::SeqCst))
                .collect();
            assert_eq!(frozen, still, "round {round}: a vCPU ran during the stop");
            // The state of each stopped vCPU is the interpreter's at its count.
            for (index, vcpu) in vcpus.iter().enumerate() {
                let snapshot = vcpu.snapshot.lock().unwrap();
                let snapshot = snapshot.as_ref().expect("a stopped vCPU stored its state");
                assert_eq!(
                    snapshot.retired, frozen[index],
                    "round {round} vCPU {index}"
                );
                let expected = reference(&LOOP_ALIAS, &loop_state(), snapshot.retired, true);
                assert_eq!(
                    snapshot.gpr, expected.observation.gpr,
                    "round {round} vCPU {index}"
                );
                assert_eq!(
                    snapshot.rip, expected.observation.rip,
                    "round {round} vCPU {index}"
                );
                assert_eq!(
                    snapshot.rflags, expected.observation.rflags,
                    "round {round} vCPU {index}"
                );
            }
            last = frozen;
            group.resume();
        }
        for vcpu in &vcpus {
            vcpu.control.request_finish();
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("a vCPU thread failed"))
            .collect::<Vec<_>>()
    });

    let (mut retired, mut jit) = (0u64, 0u64);
    for (index, (total, native, acknowledged, stats)) in totals.iter().enumerate() {
        assert!(*native > 0, "vCPU {index} ran no native step");
        assert_eq!(
            *acknowledged, ROUNDS as u64,
            "vCPU {index}: one acknowledgement per stop, none lost"
        );
        assert_eq!(stats.pending_release, 0);
        assert!(stats.live <= 1);
        retired += total;
        jit += native;
    }
    eprintln!(
        "S6-42 stop_all: {VCPUS} vCPUs, {ROUNDS} stops, retired {retired}, native {jit}, interpreter fallback {} ({}%)",
        retired - jit,
        (retired - jit) * 100 / retired
    );
    drop(region);
}

/// `stop_all` does not report success for a vCPU that never acknowledges.
#[test]
fn s6_42_stop_all_names_the_vcpu_that_does_not_acknowledge() {
    let quiet = PagedHostLoopControl::new();
    let silent = PagedHostLoopControl::new();
    quiet.acknowledge_safe();
    let group = PagedVcpuGroup::new(vec![&quiet, &silent]);
    let timeout = group
        .stop_all(Duration::from_millis(30))
        .expect_err("vCPU 1 never acknowledges");
    assert_eq!(timeout.missing, vec![1]);
    // Both flags were raised and both bytes set (flag first, then byte).
    assert!(quiet.stop_pending() && silent.stop_pending());
    group.resume();
    assert!(!quiet.stop_pending() && !silent.stop_pending());
    // The acknowledgement is lowered by the thread, never by the controller.
    assert!(quiet.is_safe());
    quiet.withdraw_safe();
    assert!(!quiet.is_safe());
}
