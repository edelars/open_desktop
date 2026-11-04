//! S3-31c full-call A/B for the opt-in RUN-local prepared-key reuse candidate.
//!
//! The profile uses the exact frozen S3-36f kernels and strata. It reports
//! measured samples only; it does not imply a production speedup or policy
//! change.

use std::num::NonZeroU64;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::time::{Duration, Instant};

use od_host::{PublishedBlockExecutor, PublishedCode};
use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::AccessContext;
use od_jit::block::{CodeView, NativeRegisterV1Admission};
use od_jit::dispatch::{
    DispatchControl, DispatchExit, FallbackDispatchResult, MAX_SINGLE_ENTRY_CACHE_BYTES,
    PreparedKeyReuseCounts, SingleEntryBlockCache,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use serde::Serialize;

use crate::{HostMetadata, MAX_RUN_TIME, cached_native_loop};

const PROFILE_ID: &str = "s3-31c-native-register-v1-run-local-prepared-key-reuse-v1";
const PAIRS: usize = 20;
const CALIBRATION_CALLS: u64 = 64;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_CALLS: u64 = 1 << 27;
const WARMUP_CALLS: u64 = 32;
const SOURCE: &[u8] = include_bytes!("cached_native_loop_reuse.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/dispatch.rs");
const BLOCK_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block.rs");
const HOST_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");
const FIXTURE_SOURCE: &[u8] = include_bytes!("cached_native_loop.rs");

#[derive(Debug)]
struct BenchError(String);
impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BenchError {}
fn err(message: impl Into<String>) -> BenchError {
    BenchError(message.into())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct StateSnapshot {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
}
impl From<&LongModeState> for StateSnapshot {
    fn from(state: &LongModeState) -> Self {
        Self {
            gpr: state.gpr,
            rip: state.rip,
            rflags: state.rflags,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct CacheCounts {
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    replacements: u64,
}

#[derive(Clone, Serialize)]
struct Observation {
    state: StateSnapshot,
    exit: String,
    retired: u64,
    remaining_budget: u64,
    blocks_executed: u64,
}

#[derive(Serialize)]
struct PartialControl {
    id: &'static str,
    budget: u64,
    stop_rip: Option<u64>,
    initial_state: StateSnapshot,
    interpreter: Observation,
    baseline: Observation,
    candidate: Observation,
    candidate_reuse_counts: ReuseCounts,
}

#[derive(Clone, Serialize)]
struct Sample {
    pair: usize,
    order: String,
    seed: u64,
    calls: u64,
    guest_instructions: u64,
    retired: u64,
    elapsed_ns: u128,
    cache_delta: Option<CacheCounts>,
}

#[derive(Serialize)]
struct Pair {
    pair: usize,
    order: String,
    seed: u64,
    baseline: Sample,
    candidate: Sample,
    interpreter: Sample,
    candidate_over_baseline_ratio: f64,
    candidate_over_interpreter_ratio: f64,
}

#[derive(Serialize)]
struct StratumPreflight {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    seed: u64,
    iterations: u64,
    budget: u64,
    interpreter: Observation,
    baseline: Observation,
    candidate: Observation,
    reuse_counts: ReuseCounts,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    final_cache_counts: CacheCounts,
    hit_only: bool,
    zero_work_checked: bool,
    entry_stop_checked: bool,
    entry_event_checked: bool,
    partial_controls_checked: bool,
    partial_controls: Vec<PartialControl>,
    dependency_replacement_checked: bool,
    error_path_contract: &'static str,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
struct ReuseCounts {
    prepared_keys: u64,
    reused_keys: u64,
}

#[derive(Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    measure_host: HostMetadata,
    source_fingerprints: Vec<(&'static str, String)>,
    timing_started: bool,
    strata: Vec<StratumPreflight>,
}

#[derive(Serialize)]
struct StratumReport {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    seed: u64,
    iterations: u64,
    guest_work_per_call: u64,
    resident_entries: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    warmup_calls_per_side: u64,
    calibrated_calls_per_side: u64,
    calibration: Pair,
    retained_pairs: Vec<Pair>,
    discarded_restarts: Vec<Restart>,
}

#[derive(Serialize)]
struct Restart {
    attempt: usize,
    previous_calls: u64,
    next_calls: u64,
    discarded_pairs: Vec<Pair>,
}

#[derive(Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    measure_host: HostMetadata,
    source_fingerprints: Vec<(&'static str, String)>,
    timed_scope: &'static str,
    sampling_protocol: &'static str,
    ratio_contract: &'static str,
    strata: Vec<StratumReport>,
    total_elapsed_ns: u128,
}

struct Prepared {
    spec: cached_native_loop::Stratum,
    bytes: Vec<u8>,
    padded_fetch_bytes: Vec<u8>,
    admission: NativeRegisterV1Admission,
    initial: LongModeState,
    oracle: LongModeState,
    context: ExecutionContextSnapshot,
    dependencies: [CodeDependencySpan; 1],
    budget: u64,
    interpreter_exit: String,
    interpreter_retired: u64,
}

pub fn preflight() -> Result<PreflightReport, Box<dyn std::error::Error>> {
    preflight_native().map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    run_native(process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

fn prepare(spec: cached_native_loop::Stratum) -> Result<Prepared, BenchError> {
    let bytes = spec.code.to_vec();
    let view = CodeView::new(cached_native_loop::BASE_RIP, &bytes)
        .map_err(|error| err(format!("{} CodeView: {error:?}", spec.id)))?;
    let admission = NativeRegisterV1Admission::from_code(view)
        .map_err(|error| err(format!("{} admission: {error:?}", spec.id)))?;
    let budget = spec.iterations * spec.instructions_per_iteration;
    let initial = cached_native_loop::initial_state(spec.seed);
    let padded_fetch_bytes = cached_native_loop::padded_code(spec.code);
    let mut oracle = initial.clone();
    let mut fetch = crate::dispatch_cache::FetchMemory(&padded_fetch_bytes);
    let result = interpreter_run(
        &mut oracle,
        &mut fetch,
        RunConfig {
            instruction_budget: budget,
            stop_rip: None,
            context: AccessContext::default(),
        },
        || false,
    );
    if result.exit != RunExit::InstructionBudgetExhausted
        || result.instructions_executed != budget
        || oracle.rip != cached_native_loop::BASE_RIP
        || oracle.rflags & 0x40 != 0
    {
        return Err(err(format!(
            "{} interpreter oracle failed: {:?}, retired {}, RIP={:#x}",
            spec.id, result.exit, result.instructions_executed, oracle.rip
        )));
    }
    let context = ExecutionContextSnapshot::native_register_v1_fixture(
        ExecutionContextId::from_u64(1).expect("nonzero benchmark context"),
    );
    let dependencies = [CodeDependencySpan {
        start_rip: cached_native_loop::BASE_RIP,
        end_rip: view.code_end(),
        identity: CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
            backing: BackingId::new(NonZeroU64::new(1).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
            executable: true,
        },
    }];
    Ok(Prepared {
        spec,
        bytes,
        padded_fetch_bytes,
        admission,
        initial,
        oracle,
        context,
        dependencies,
        budget,
        interpreter_exit: format!("{:?}", result.exit),
        interpreter_retired: result.instructions_executed,
    })
}

fn preflight_native() -> Result<PreflightReport, BenchError> {
    require_release_host()?;
    let mut strata = Vec::new();
    for spec in cached_native_loop::strata() {
        strata.push(check_case(&prepare(spec)?)?);
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        measure_host: HostMetadata::current(),
        source_fingerprints: fingerprints(),
        timing_started: false,
        strata,
    })
}

fn run_native(process_run: u8) -> Result<Report, BenchError> {
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be in 1..=3"));
    }
    require_release_host()?;
    let started = Instant::now();
    let mut strata = Vec::new();
    for spec in cached_native_loop::strata() {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute benchmark run limit exceeded"));
        }
        let case = prepare(spec)?;
        check_case(&case)?;
        strata.push(measure_stratum(&case, started)?);
    }
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        measure_host: HostMetadata::current(),
        source_fingerprints: fingerprints(),
        timed_scope: "full synchronous interpreter, ordinary cached dispatcher, or prepared-key candidate call; state reset and CodeView/control setup are outside each interval; candidate memo setup/use/drop are inside the dispatcher call",
        sampling_protocol: "each sample side has at least 200 ms measured elapsed time; 20 retained paired samples per stratum; baseline/candidate/interpreter order rotates across all six permutations; calibration begins at 64 calls per side and doubles on restart; raw samples preserve order, seed, calls and elapsed nanoseconds",
        ratio_contract: "candidate_over_baseline_ratio and candidate_over_interpreter_ratio are elapsed candidate / elapsed comparator; below 1 is faster; no speedup is inferred without the raw repeated ranges and cross-process comparison",
        strata,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

fn check_case(case: &Prepared) -> Result<StratumPreflight, BenchError> {
    let mut cache = new_cache()?;
    let mut state = case.initial.clone();
    let cold = ordinary_call(case, &mut state, case.budget, &mut cache)?;
    validate_full(case, &state, cold)?;
    let baseline_state = state.clone();
    let baseline_retired = cold.retired_native + cold.retired_interpreter;
    let baseline_exit = format!("{:?}", cold.exit);
    let baseline_blocks = cold.blocks_executed;

    state.clone_from(&case.initial);
    let before_candidate = cache_counts(&cache);
    let mut reuse_counts = PreparedKeyReuseCounts::default();
    let candidate =
        counted_candidate_call(case, &mut state, case.budget, &mut cache, &mut reuse_counts)?;
    validate_full(case, &state, candidate)?;
    let candidate_retired = candidate.retired_native + candidate.retired_interpreter;
    let candidate_exit = format!("{:?}", candidate.exit);
    let candidate_blocks = candidate.blocks_executed;
    if state != baseline_state
        || reuse_counts.prepared_keys != 1
        || reuse_counts.reused_keys != case.spec.iterations.saturating_sub(1)
    {
        return Err(err(format!(
            "{} candidate parity or reuse-count mismatch: counts={reuse_counts:?}",
            case.spec.id
        )));
    }
    let candidate_state = state.clone();
    let after_candidate = cache_counts(&cache);
    let candidate_delta = delta(before_candidate, after_candidate);
    if candidate_delta.hits != case.spec.iterations
        || candidate_delta.misses != 0
        || candidate_delta.compiles != 0
        || candidate_delta.publications != 0
        || candidate_delta.replacements != 0
    {
        return Err(err(format!(
            "{} preflight candidate was not warm hit-only: {candidate_delta:?}",
            case.spec.id
        )));
    }

    state.clone_from(&case.initial);
    let zero_work = candidate_call(case, &mut state, 0, &mut cache)?;
    if zero_work.exit != DispatchExit::BudgetExhausted
        || zero_work.retired_native != 0
        || zero_work.retired_interpreter != 0
        || zero_work.remaining_budget != 0
        || zero_work.blocks_executed != 0
        || state != case.initial
    {
        return Err(err(format!("{} zero-work control failed", case.spec.id)));
    }
    let stop = AtomicU8::new(1);
    state.clone_from(&case.initial);
    let stopped = call_with_control(case, &mut state, case.budget, &mut cache, &stop)?;
    if stopped.exit != DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopRequested)
        || stopped.retired_native != 0
        || stopped.retired_interpreter != 0
        || stopped.remaining_budget != case.budget
        || stopped.blocks_executed != 0
        || state != case.initial
    {
        return Err(err(format!("{} entry-stop control failed", case.spec.id)));
    }
    if cache_counts(&cache) != after_candidate {
        return Err(err(format!(
            "{} no-work/entry-stop changed cache counters",
            case.spec.id
        )));
    }

    let event = AtomicBool::new(true);
    state.clone_from(&case.initial);
    let event_result = call_with_event(case, &mut state, case.budget, &mut cache, &event)?;
    if event_result.exit != DispatchExit::EventPending
        || event_result.retired_native != 0
        || event_result.retired_interpreter != 0
        || event_result.remaining_budget != case.budget
        || event_result.blocks_executed != 0
        || state != case.initial
    {
        return Err(err(format!("{} entry-event control failed", case.spec.id)));
    }
    if cache_counts(&cache) != after_candidate {
        return Err(err(format!(
            "{} entry-event changed cache counters",
            case.spec.id
        )));
    }

    let partial_controls = check_partial_controls(case, &mut cache)?;

    let mut changed_dependencies = case.dependencies.clone();
    changed_dependencies[0].identity.mapping_version =
        MappingVersion::new(NonZeroU64::new(2).unwrap());
    state.clone_from(&case.initial);
    let before_invalidation = cache_counts(&cache);
    let mut invalidation_counts = PreparedKeyReuseCounts::default();
    let invalidated = counted_candidate_call_with_dependencies(
        case,
        &mut state,
        case.budget,
        &mut cache,
        &changed_dependencies,
        &mut invalidation_counts,
        None,
    )?;
    validate_full(case, &state, invalidated)?;
    let invalidation_delta = delta(before_invalidation, cache_counts(&cache));
    let expected_prepared = u64::from(case.spec.iterations > 1) + 1;
    if invalidation_delta.hits != case.spec.iterations.saturating_sub(1)
        || invalidation_delta.misses != 1
        || invalidation_delta.compiles != 1
        || invalidation_delta.publications != 1
        || invalidation_delta.replacements != 1
        || invalidation_counts.prepared_keys != expected_prepared
        || invalidation_counts.reused_keys != case.spec.iterations.saturating_sub(expected_prepared)
    {
        return Err(err(format!(
            "{} dependency replacement did not clear/rebuild the memo: delta={invalidation_delta:?}, counts={invalidation_counts:?}",
            case.spec.id
        )));
    }
    let dependency_replacement_checked = true;

    let final_counts = cache_counts(&cache);
    if final_counts.misses < 2
        || final_counts.compiles < 2
        || final_counts.publications < 2
        || final_counts.hits == 0
        || cache.resident_entries() != 1
        || cache.resident_code_bytes() == 0
        || cache.resident_logical_bytes() == 0
    {
        return Err(err(format!(
            "{} final resident cache state invalid",
            case.spec.id
        )));
    }
    Ok(StratumPreflight {
        id: case.spec.id,
        code_hex: hex(&case.bytes),
        code_sha256: od_testkit::sha256_hex(&case.bytes),
        seed: case.spec.seed,
        iterations: case.spec.iterations,
        budget: case.budget,
        interpreter: observation(
            &case.oracle,
            case.interpreter_exit.clone(),
            case.interpreter_retired,
            case.budget.saturating_sub(case.interpreter_retired),
            0,
        ),
        baseline: observation(
            &baseline_state,
            baseline_exit,
            baseline_retired,
            cold.remaining_budget,
            baseline_blocks,
        ),
        candidate: observation(
            &candidate_state,
            candidate_exit,
            candidate_retired,
            candidate.remaining_budget,
            candidate_blocks,
        ),
        reuse_counts: ReuseCounts {
            prepared_keys: reuse_counts.prepared_keys,
            reused_keys: reuse_counts.reused_keys,
        },
        resident_entries: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        final_cache_counts: final_counts,
        hit_only: candidate_delta.hits == case.spec.iterations
            && candidate_delta.misses == 0
            && candidate_delta.compiles == 0
            && candidate_delta.publications == 0
            && candidate_delta.replacements == 0,
        zero_work_checked: true,
        entry_stop_checked: true,
        entry_event_checked: true,
        partial_controls_checked: !partial_controls.is_empty(),
        partial_controls,
        dependency_replacement_checked,
        error_path_contract: "the benchmark host backend has no deterministic failure injection; executor-error removal is covered by the od-jit unit test",
    })
}

fn check_partial_controls(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
) -> Result<Vec<PartialControl>, BenchError> {
    let instructions_per_iteration = case.spec.instructions_per_iteration;
    let branch_rip = cached_native_loop::BASE_RIP + case.bytes.len() as u64 - 2;
    let mut scenarios = vec![
        (
            "later_iteration_after_lea",
            instructions_per_iteration + 1,
            None,
            case.initial.clone(),
        ),
        (
            "later_iteration_after_xchg",
            instructions_per_iteration + 2,
            None,
            case.initial.clone(),
        ),
        (
            "later_iteration_before_jnz",
            instructions_per_iteration * 2 - 1,
            None,
            case.initial.clone(),
        ),
        (
            "stop_address_before_xchg",
            case.budget,
            Some(cached_native_loop::BASE_RIP + 4),
            case.initial.clone(),
        ),
        (
            "stop_address_at_jnz",
            case.budget,
            Some(branch_rip),
            case.initial.clone(),
        ),
    ];
    let mut wrapping_initial = case.initial.clone();
    wrapping_initial.gpr[0] = u64::MAX - 1;
    wrapping_initial.gpr[3] = u64::MAX;
    wrapping_initial.gpr[1] = 1;
    wrapping_initial.rflags &= !0x40;
    if wrapping_initial.gpr[3].wrapping_add(wrapping_initial.gpr[1]) != 0 {
        return Err(err(format!(
            "{} wrapping operand setup failed",
            case.spec.id
        )));
    }
    scenarios.push((
        "wrapping_operand_later_iteration",
        instructions_per_iteration * 2 + 1,
        None,
        wrapping_initial,
    ));

    scenarios
        .into_iter()
        .map(|(id, budget, stop_rip, initial)| {
            run_partial_control(case, cache, id, budget, stop_rip, initial)
        })
        .collect()
}

fn run_partial_control(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    id: &'static str,
    budget: u64,
    stop_rip: Option<u64>,
    initial: LongModeState,
) -> Result<PartialControl, BenchError> {
    let mut interpreter_state = initial.clone();
    let mut memory = crate::dispatch_cache::FetchMemory(&case.padded_fetch_bytes);
    let interpreter = interpreter_run(
        &mut interpreter_state,
        &mut memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip,
            context: AccessContext::default(),
        },
        || false,
    );
    let expected_native_exit = match &interpreter.exit {
        RunExit::InstructionBudgetExhausted => DispatchExit::BudgetExhausted,
        RunExit::StopAddress => DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopAddress),
        exit => {
            return Err(err(format!(
                "{} {id} interpreter control exited unexpectedly: {exit:?}",
                case.spec.id
            )));
        }
    };
    let expected_remaining = budget.saturating_sub(interpreter.instructions_executed);
    let before_baseline = cache_counts(cache);
    let mut baseline_state = initial.clone();
    let baseline = ordinary_call_with_control(case, &mut baseline_state, budget, cache, stop_rip)?;
    validate_partial_parity(
        case,
        id,
        &interpreter_state,
        interpreter.instructions_executed,
        expected_native_exit,
        expected_remaining,
        &baseline_state,
        baseline,
    )?;
    let baseline_delta = delta(before_baseline, cache_counts(cache));
    if baseline_delta.hits != baseline.blocks_executed
        || baseline_delta.misses != 0
        || baseline_delta.compiles != 0
        || baseline_delta.publications != 0
        || baseline_delta.replacements != 0
    {
        return Err(err(format!(
            "{} {id} baseline control was not resident hit-only: {baseline_delta:?}",
            case.spec.id
        )));
    }

    let before_candidate = cache_counts(cache);
    let mut candidate_state = initial.clone();
    let mut candidate_counts = PreparedKeyReuseCounts::default();
    let candidate = counted_candidate_call_with_control(
        case,
        &mut candidate_state,
        budget,
        cache,
        stop_rip,
        &mut candidate_counts,
    )?;
    validate_partial_parity(
        case,
        id,
        &interpreter_state,
        interpreter.instructions_executed,
        expected_native_exit,
        expected_remaining,
        &candidate_state,
        candidate,
    )?;
    if candidate_state != baseline_state || candidate != baseline {
        return Err(err(format!(
            "{} {id} baseline/candidate partial-control parity failed",
            case.spec.id
        )));
    }
    let candidate_delta = delta(before_candidate, cache_counts(cache));
    if candidate_delta.hits != candidate.blocks_executed
        || candidate_delta.misses != 0
        || candidate_delta.compiles != 0
        || candidate_delta.publications != 0
        || candidate_delta.replacements != 0
        || candidate_counts.prepared_keys != 1
        || candidate_counts.reused_keys != candidate.blocks_executed.saturating_sub(1)
    {
        return Err(err(format!(
            "{} {id} candidate partial-control memo/cache evidence failed: delta={candidate_delta:?}, reuse={candidate_counts:?}",
            case.spec.id
        )));
    }
    Ok(PartialControl {
        id,
        budget,
        stop_rip,
        initial_state: StateSnapshot::from(&initial),
        interpreter: observation(
            &interpreter_state,
            format!("{:?}", interpreter.exit),
            interpreter.instructions_executed,
            expected_remaining,
            0,
        ),
        baseline: observation(
            &baseline_state,
            format!("{:?}", baseline.exit),
            baseline.retired_native + baseline.retired_interpreter,
            baseline.remaining_budget,
            baseline.blocks_executed,
        ),
        candidate: observation(
            &candidate_state,
            format!("{:?}", candidate.exit),
            candidate.retired_native + candidate.retired_interpreter,
            candidate.remaining_budget,
            candidate.blocks_executed,
        ),
        candidate_reuse_counts: ReuseCounts {
            prepared_keys: candidate_counts.prepared_keys,
            reused_keys: candidate_counts.reused_keys,
        },
    })
}

fn validate_partial_parity(
    case: &Prepared,
    scenario: &str,
    interpreter_state: &LongModeState,
    interpreter_retired: u64,
    expected_exit: DispatchExit,
    expected_remaining: u64,
    native_state: &LongModeState,
    native: FallbackDispatchResult,
) -> Result<(), BenchError> {
    let native_retired = native.retired_native + native.retired_interpreter;
    if native.exit != expected_exit
        || native_state != interpreter_state
        || native_retired != interpreter_retired
        || native.remaining_budget != expected_remaining
    {
        return Err(err(format!(
            "{} {scenario} parity mismatch: exit={:?}/{expected_exit:?}, retired={native_retired}/{interpreter_retired}, remaining={}/{expected_remaining}, full_state_equal={}",
            case.spec.id,
            native.exit,
            native.remaining_budget,
            native_state == interpreter_state
        )));
    }
    Ok(())
}

fn measure_stratum(case: &Prepared, run_started: Instant) -> Result<StratumReport, BenchError> {
    let mut cache = new_cache()?;
    let mut state = case.initial.clone();
    let _ = ordinary_call(case, &mut state, case.budget, &mut cache)?;
    let resident_entries = cache.resident_entries();
    let resident_code_bytes = cache.resident_code_bytes();
    let resident_logical_bytes = cache.resident_logical_bytes();
    for _ in 0..WARMUP_CALLS {
        let mut state = case.initial.clone();
        let result = ordinary_call(case, &mut state, case.budget, &mut cache)?;
        validate_full(case, &state, result)?;
        state.clone_from(&case.initial);
        let result = candidate_call(case, &mut state, case.budget, &mut cache)?;
        validate_full(case, &state, result)?;
        state.clone_from(&case.initial);
        let mut memory = crate::dispatch_cache::FetchMemory(&case.padded_fetch_bytes);
        let result = interpreter_run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: case.budget,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        if result.exit != RunExit::InstructionBudgetExhausted
            || result.instructions_executed != case.budget
            || state != case.oracle
        {
            return Err(err(format!("{} interpreter warmup mismatch", case.spec.id)));
        }
    }
    let mut calls = CALIBRATION_CALLS;
    let calibration = loop {
        let pair = sample_triple(case, &mut cache, calls, 0)?;
        if all_minimum(&pair) {
            break pair;
        }
        if calls >= MAX_CALLS {
            return Err(err(format!(
                "{} failed sample minimum within call bound",
                case.spec.id
            )));
        }
        calls = calls.saturating_mul(2).min(MAX_CALLS);
    };
    let mut restarts = Vec::new();
    let mut retained = Vec::new();
    for attempt in 1..=3 {
        let mut attempt_pairs = Vec::new();
        for pair_index in 0..PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err("30 minute benchmark run limit exceeded"));
            }
            attempt_pairs.push(sample_triple(
                case,
                &mut cache,
                calls,
                pair_index + (attempt - 1) * PAIRS,
            )?);
        }
        if attempt_pairs.iter().all(all_minimum) {
            retained = attempt_pairs;
            break;
        }
        if attempt == 3 || calls == MAX_CALLS {
            return Err(err(format!(
                "{} sample minimum failed after 3 attempts",
                case.spec.id
            )));
        }
        let next_calls = calls.saturating_mul(2).min(MAX_CALLS);
        restarts.push(Restart {
            attempt,
            previous_calls: calls,
            next_calls,
            discarded_pairs: attempt_pairs,
        });
        calls = next_calls;
    }
    Ok(StratumReport {
        id: case.spec.id,
        code_hex: hex(&case.bytes),
        code_sha256: od_testkit::sha256_hex(&case.bytes),
        seed: case.spec.seed,
        iterations: case.spec.iterations,
        guest_work_per_call: case.budget,
        resident_entries,
        resident_code_bytes,
        resident_logical_bytes,
        warmup_calls_per_side: WARMUP_CALLS,
        calibrated_calls_per_side: calls,
        calibration,
        retained_pairs: retained,
        discarded_restarts: restarts,
    })
}

fn sample_triple(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    calls: u64,
    pair: usize,
) -> Result<Pair, BenchError> {
    let permutations = [
        [0usize, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];
    let order = permutations[pair % permutations.len()];
    let order_name = order
        .iter()
        .map(|value| ["B", "C", "I"][*value])
        .collect::<String>();
    let mut samples: [Option<Sample>; 3] = [None, None, None];
    for side in order {
        samples[side] = Some(sample_side(case, cache, calls, pair, &order_name, side)?);
    }
    let baseline = samples[0].take().unwrap();
    let candidate = samples[1].take().unwrap();
    let interpreter = samples[2].take().unwrap();
    Ok(Pair {
        pair,
        order: order_name,
        seed: case.spec.seed,
        candidate_over_baseline_ratio: candidate.elapsed_ns as f64 / baseline.elapsed_ns as f64,
        candidate_over_interpreter_ratio: candidate.elapsed_ns as f64
            / interpreter.elapsed_ns as f64,
        baseline,
        candidate,
        interpreter,
    })
}

fn sample_side(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    calls: u64,
    pair: usize,
    order: &str,
    side: usize,
) -> Result<Sample, BenchError> {
    let before = cache_counts(cache);
    let mut retired = 0u64;
    let mut elapsed_ns = 0u128;
    for _ in 0..calls {
        let mut state = case.initial.clone();
        let view = if side == 2 {
            None
        } else {
            Some(
                CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
                    .map_err(|error| err(format!("{} timed CodeView: {error:?}", case.spec.id)))?,
            )
        };
        let stop = AtomicU8::new(0);
        let mut memory = crate::dispatch_cache::FetchMemory(&case.padded_fetch_bytes);
        let call_started = Instant::now();
        let (native_result, interpreter_result) = match side {
            0 => {
                let result = timed_dispatch(
                    case,
                    view.expect("native side has a code view"),
                    &mut state,
                    case.budget,
                    cache,
                    &stop,
                    false,
                )?;
                (Some(result), None)
            }
            1 => {
                let result = timed_dispatch(
                    case,
                    view.expect("native side has a code view"),
                    &mut state,
                    case.budget,
                    cache,
                    &stop,
                    true,
                )?;
                (Some(result), None)
            }
            _ => (
                None,
                Some(interpreter_run(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: case.budget,
                        stop_rip: None,
                        context: AccessContext::default(),
                    },
                    || false,
                )),
            ),
        };
        elapsed_ns = elapsed_ns
            .checked_add(call_started.elapsed().as_nanos())
            .ok_or_else(|| err("sample elapsed counter overflow"))?;
        let actual_retired = if let Some(result) = native_result {
            validate_full(case, &state, result)?;
            result.retired_native + result.retired_interpreter
        } else {
            let result = interpreter_result.expect("interpreter side has a result");
            if result.exit != RunExit::InstructionBudgetExhausted
                || result.instructions_executed != case.budget
                || state != case.oracle
            {
                return Err(err(format!(
                    "{} timed interpreter mismatch: exit={:?}, retired={}, state_equal={}",
                    case.spec.id,
                    result.exit,
                    result.instructions_executed,
                    state == case.oracle
                )));
            }
            result.instructions_executed
        };
        retired = retired
            .checked_add(actual_retired)
            .ok_or_else(|| err("sample retired counter overflow"))?;
    }
    let after = cache_counts(cache);
    let cache_delta = if side == 2 {
        None
    } else {
        Some(delta(before, after))
    };
    if side != 2 {
        let actual = cache_delta.unwrap();
        if actual.hits != calls.saturating_mul(case.spec.iterations)
            || actual.misses != 0
            || actual.compiles != 0
            || actual.publications != 0
            || actual.replacements != 0
        {
            return Err(err(format!(
                "{} timed side was not hit-only: {actual:?}",
                case.spec.id
            )));
        }
    }
    Ok(Sample {
        pair,
        order: order.to_owned(),
        seed: case.spec.seed,
        calls,
        guest_instructions: calls * case.budget,
        retired,
        elapsed_ns,
        cache_delta,
    })
}

fn validate_full(
    case: &Prepared,
    state: &LongModeState,
    result: FallbackDispatchResult,
) -> Result<(), BenchError> {
    let retired = result.retired_native + result.retired_interpreter;
    if result.exit != DispatchExit::BudgetExhausted
        || result.retired_interpreter != 0
        || retired != case.budget
        || result.remaining_budget != 0
        || result.blocks_executed != case.spec.iterations
        || *state != case.oracle
    {
        return Err(err(format!(
            "{} full-state/accounting mismatch: exit={:?} native={} interp={} remaining={} blocks={} state_equal={}",
            case.spec.id,
            result.exit,
            result.retired_native,
            result.retired_interpreter,
            result.remaining_budget,
            result.blocks_executed,
            *state == case.oracle
        )));
    }
    Ok(())
}

fn observation(
    state: &LongModeState,
    exit: String,
    retired: u64,
    remaining_budget: u64,
    blocks_executed: u64,
) -> Observation {
    Observation {
        state: StateSnapshot::from(state),
        exit,
        retired,
        remaining_budget,
        blocks_executed,
    }
}

fn ordinary_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
) -> Result<FallbackDispatchResult, BenchError> {
    dispatch_call(case, state, budget, cache, false)
}

fn ordinary_call_with_control(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    stop_rip: Option<u64>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} partial CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
    )
    .map_err(|error| {
        err(format!(
            "{} partial ordinary dispatch: {error:?}",
            case.spec.id
        ))
    })
}

fn candidate_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} candidate CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
    )
    .map_err(|error| err(format!("{} candidate dispatch: {error:?}", case.spec.id)))
}

fn counted_candidate_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    counts: &mut PreparedKeyReuseCounts,
) -> Result<FallbackDispatchResult, BenchError> {
    counted_candidate_call_with_control(case, state, budget, cache, None, counts)
}

fn counted_candidate_call_with_control(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    stop_rip: Option<u64>,
    counts: &mut PreparedKeyReuseCounts,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} counted CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
        counts,
    )
    .map_err(|error| {
        err(format!(
            "{} counted candidate dispatch: {error:?}",
            case.spec.id
        ))
    })
}

fn dispatch_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    candidate: bool,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: &stop,
        event_requested: None,
    };
    let result = if candidate {
        od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled(
            view,
            &case.admission,
            case.context,
            &case.dependencies,
            state,
            budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    } else {
        od_jit::dispatch::run_cached_native_register_v1_controlled(
            view,
            &case.admission,
            case.context,
            &case.dependencies,
            state,
            budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    };
    result.map_err(|error| err(format!("{} cached dispatch: {error:?}", case.spec.id)))
}

fn timed_dispatch(
    case: &Prepared,
    view: CodeView<'_>,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    stop: &AtomicU8,
    candidate: bool,
) -> Result<FallbackDispatchResult, BenchError> {
    let control = DispatchControl {
        stop_rip: None,
        stop_requested: stop,
        event_requested: None,
    };
    let result = if candidate {
        od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled(
            view,
            &case.admission,
            case.context,
            &case.dependencies,
            state,
            budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    } else {
        od_jit::dispatch::run_cached_native_register_v1_controlled(
            view,
            &case.admission,
            case.context,
            &case.dependencies,
            state,
            budget,
            control,
            cache,
            &mut PublishedBlockExecutor,
        )
    };
    result.map_err(|error| err(format!("{} timed dispatch: {error:?}", case.spec.id)))
}

fn call_with_event(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    event: &AtomicBool,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} event CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: Some(event),
        },
        cache,
        &mut PublishedBlockExecutor,
    )
    .map_err(|error| err(format!("{} event dispatch: {error:?}", case.spec.id)))
}

fn counted_candidate_call_with_dependencies(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    dependencies: &[CodeDependencySpan],
    counts: &mut PreparedKeyReuseCounts,
    stop_rip: Option<u64>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} invalidation CodeView: {error:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled_counted(
        view,
        &case.admission,
        case.context,
        dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
        counts,
    )
    .map_err(|error| err(format!("{} invalidation dispatch: {error:?}", case.spec.id)))
}

fn call_with_control(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    stop: &AtomicU8,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|error| err(format!("{} stop CodeView: {error:?}", case.spec.id)))?;
    od_jit::dispatch::run_cached_native_register_v1_prepared_key_candidate_controlled(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
    )
    .map_err(|error| err(format!("{} stop dispatch: {error:?}", case.spec.id)))
}

fn new_cache() -> Result<SingleEntryBlockCache<PublishedCode>, BenchError> {
    SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
        .map_err(|error| err(format!("cache setup: {error:?}")))
}

fn cache_counts(cache: &SingleEntryBlockCache<PublishedCode>) -> CacheCounts {
    let counters = cache.counters();
    CacheCounts {
        hits: counters.hits,
        misses: counters.misses,
        compiles: counters.successful_compiles,
        publications: counters.successful_publications,
        replacements: counters.replacements,
    }
}

fn delta(before: CacheCounts, after: CacheCounts) -> CacheCounts {
    CacheCounts {
        hits: after.hits - before.hits,
        misses: after.misses - before.misses,
        compiles: after.compiles - before.compiles,
        publications: after.publications - before.publications,
        replacements: after.replacements - before.replacements,
    }
}

fn all_minimum(pair: &Pair) -> bool {
    [
        pair.baseline.elapsed_ns,
        pair.candidate.elapsed_ns,
        pair.interpreter.elapsed_ns,
    ]
    .into_iter()
    .all(|elapsed| elapsed >= MIN_SAMPLE.as_nanos())
}

fn fingerprints() -> Vec<(&'static str, String)> {
    [
        ("profile", SOURCE),
        ("cli", CLI_SOURCE),
        ("lib", LIB_SOURCE),
        ("dispatch", DISPATCH_SOURCE),
        ("block", BLOCK_SOURCE),
        ("host", HOST_SOURCE),
        ("frozen_fixture", FIXTURE_SOURCE),
    ]
    .into_iter()
    .map(|(name, bytes)| (name, od_testkit::sha256_hex(bytes)))
    .collect()
}

fn require_release_host() -> Result<(), BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cached-native-loop-reuse requires a release build"));
    }
    if !cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Err(err(
            "cached-native-loop-reuse requires native Apple Silicon macOS execution",
        ));
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(DIGITS[(byte >> 4) as usize] as char);
        output.push(DIGITS[(byte & 0xf) as usize] as char);
    }
    output
}
