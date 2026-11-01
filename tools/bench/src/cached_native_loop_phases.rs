//! S3-31b attribution for the fixed S3-36f NativeRegisterV1 register loops.
//!
//! Observed phase durations are nested inside the observed full-call duration.
//! The residual and ordinary/observed full-call delta are reported as
//! observations, never subtracted as a correction.

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
    SingleEntryBlockCache,
};
use od_jit::phase_observation::{PhaseRecorder, PhaseTotal};
use od_jit::runtime::RuntimeExitTag;
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use serde::Serialize;

use crate::{HostMetadata, MAX_RUN_TIME, cached_native_loop};

const PROFILE_ID: &str = "s3-31b-native-register-v1-cached-loop-phase-attribution-v1";
const PAIRS: usize = 20;
const CALIBRATION_CALLS: u64 = 64;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_CALLS: u64 = 1 << 27;
const SOURCE: &[u8] = include_bytes!("cached_native_loop_phases.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/dispatch.rs");
const OBSERVER_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/phase_observation.rs");
const BLOCK_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block.rs");
const HOST_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");

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
    fn from(s: &LongModeState) -> Self {
        Self {
            gpr: s.gpr,
            rip: s.rip,
            rflags: s.rflags,
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
struct PhaseObservation {
    phase: String,
    calls: u64,
    elapsed_ns: u128,
}
#[derive(Clone, Serialize)]
struct Sample {
    pair: usize,
    order: &'static str,
    seed: u64,
    calls: u64,
    guest_instructions: u64,
    retired: u64,
    elapsed_ns: u128,
    phase_totals: Vec<PhaseObservation>,
    phase_sum_ns: u128,
    residual_ns: Option<i128>,
    cache: CacheCounts,
}
#[derive(Serialize)]
struct Pair {
    pair: usize,
    order: &'static str,
    seed: u64,
    ordinary: Sample,
    observed: Sample,
    observed_minus_ordinary_elapsed_ns: i128,
    observed_over_ordinary_ratio: f64,
}
#[derive(Serialize)]
struct Restart {
    attempt: usize,
    previous_calls: u64,
    next_calls: u64,
    discarded_pairs: Vec<Pair>,
}
#[derive(Serialize)]
struct StratumPreflight {
    id: &'static str,
    code_hex: String,
    code_sha256: String,
    seed: u64,
    iterations: u64,
    budget: u64,
    interpreter: StateSnapshot,
    ordinary: StateSnapshot,
    observed: StateSnapshot,
    ordinary_retired: u64,
    observed_retired: u64,
    resident_entry_count: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    final_cache_counts: CacheCounts,
    hit_only: bool,
    phase_counts_valid: bool,
    final_phase_counts: Vec<PhaseObservation>,
    zero_work_phase_counts: Vec<PhaseObservation>,
    stop_phase_counts: Vec<PhaseObservation>,
    event_phase_counts: Vec<PhaseObservation>,
    pre_admission_error_phase_counts: Vec<PhaseObservation>,
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
    resident_entry_count: usize,
    resident_code_bytes: usize,
    resident_logical_bytes: usize,
    cold_first_dispatch_ns: u128,
    ordinary_warmup_calls_per_side: u64,
    observed_warmup_calls_per_side: u64,
    calibration_calls_per_side: u64,
    calibration_ordinary: Sample,
    calibration_observed: Sample,
    retained_pairs: Vec<Pair>,
    discarded_restarts: Vec<Restart>,
}
#[derive(Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    measure_host: HostMetadata,
    source_fingerprints: Vec<(&'static str, String)>,
    ordinary_timed_scope: &'static str,
    observed_timed_scope: &'static str,
    phase_boundary_contract: &'static str,
    residual_contract: &'static str,
    strata: Vec<StratumReport>,
    total_elapsed_ns: u128,
}

struct Prepared {
    spec: cached_native_loop::Stratum,
    bytes: Vec<u8>,
    admission: NativeRegisterV1Admission,
    initial: LongModeState,
    oracle: LongModeState,
    context: ExecutionContextSnapshot,
    dependencies: [CodeDependencySpan; 1],
    budget: u64,
}

pub fn preflight() -> Result<PreflightReport, Box<dyn std::error::Error>> {
    preflight_native().map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

pub fn run(process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    run_native(process_run).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}

fn prepare(spec: cached_native_loop::Stratum) -> Result<Prepared, BenchError> {
    let bytes = spec.code.to_vec();
    let view = CodeView::new(cached_native_loop::BASE_RIP, &bytes)
        .map_err(|e| err(format!("{} CodeView: {e:?}", spec.id)))?;
    let admission = NativeRegisterV1Admission::from_code(view)
        .map_err(|e| err(format!("{} admission: {e:?}", spec.id)))?;
    let budget = spec.iterations * spec.instructions_per_iteration;
    let initial = cached_native_loop::initial_state(spec.seed);
    let fetch_bytes = cached_native_loop::padded_code(spec.code);
    let mut oracle = initial.clone();
    let mut memory = crate::dispatch_cache::FetchMemory(&fetch_bytes);
    let result = interpreter_run(
        &mut oracle,
        &mut memory,
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
        admission,
        initial,
        oracle,
        context,
        dependencies,
        budget,
    })
}

fn preflight_native() -> Result<PreflightReport, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err(
            "cached-native-loop-phases preflight requires a release build",
        ));
    }
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
    if cfg!(debug_assertions) {
        return Err(err("cached-native-loop-phases requires a release build"));
    }
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
        ordinary_timed_scope: "summed full ordinary cached NativeRegisterV1 dispatcher call durations; state reset and CodeView construction are outside each interval",
        observed_timed_scope: "summed full explicit-observer cached NativeRegisterV1 dispatcher call durations; observer begin/end overhead remains inside each interval",
        phase_boundary_contract: "BlockBuildAndAdmission wraps block building; KeySnapshotAndLookup wraps minimum cache-size/capacity, TranslationKey/snapshot/first lookup and counter update; HostExecutionBoundary wraps only backend.execute_cached_block. Publication is separately reported and has zero calls in the resident warm profile. Repeated snapshot validation, code-view matching, run guards, stop/event/budget/fallback handling, retirement aggregation, and post-execution validation remain outside phases. The error-path preflight exercises only pre-admission code mismatch; errors arising inside a measured phase are untested.",
        residual_contract: "only observed samples carry residual_ns: observed full-call elapsed_ns minus the sum of non-overlapping phase durations (signed). observed_minus_ordinary_elapsed_ns and observed_over_ordinary_ratio are net paired full-path differences that include instrumentation and timing/path noise; they are not isolated observer cost or timing corrections",
        strata,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

fn check_case(case: &Prepared) -> Result<StratumPreflight, BenchError> {
    let mut cache = new_cache()?;
    let mut recorder = PhaseRecorder::default();
    let mut state = case.initial.clone();
    let cold = ordinary_call(case, &mut state, case.budget, &mut cache, None, None)?;
    validate_full(case, &state, cold)?;
    let initial_counts = cache_counts(&cache);
    if initial_counts.misses != 1
        || initial_counts.compiles != 1
        || initial_counts.publications != 1
        || initial_counts.replacements != 0
        || initial_counts.hits != case.spec.iterations - 1
        || cache.resident_entries() != 1
        || cache.resident_code_bytes() == 0
        || cache.resident_logical_bytes() == 0
    {
        return Err(err(format!(
            "{} failed exactly-one resident prewarm: {initial_counts:?}",
            case.spec.id
        )));
    }

    let before_ordinary = cache_counts(&cache);
    state.clone_from(&case.initial);
    let ordinary = ordinary_call(case, &mut state, case.budget, &mut cache, None, None)?;
    validate_full(case, &state, ordinary)?;
    let ordinary_counts = cache_counts(&cache);
    validate_hit_delta(case, before_ordinary, ordinary_counts, case.spec.iterations)?;
    let ordinary_state = state.clone();

    let before_observed = cache_counts(&cache);
    state.clone_from(&case.initial);
    recorder.clear();
    let observed = observed_call(
        case,
        &mut state,
        case.budget,
        &mut cache,
        &mut recorder,
        None,
        None,
    )?;
    validate_full(case, &state, observed)?;
    let observed_state = state.clone();
    validate_hit_delta(
        case,
        before_observed,
        cache_counts(&cache),
        case.spec.iterations,
    )?;
    let final_phases = phase_observations(&recorder);
    validate_warm_phase_counts(case, &final_phases, 1)?;
    if ordinary_state != case.oracle
        || observed_state != case.oracle
        || ordinary_state != observed_state
    {
        return Err(err(format!(
            "{} ordinary/observed/interpreter full-state parity failed",
            case.spec.id
        )));
    }
    let zero_work_phase_counts =
        check_early_exit(case, &mut cache, &mut recorder, EarlyExit::Zero)?;
    let stop_phase_counts = check_early_exit(case, &mut cache, &mut recorder, EarlyExit::Stop)?;
    let event_phase_counts = check_early_exit(case, &mut cache, &mut recorder, EarlyExit::Event)?;
    let pre_admission_error_phase_counts =
        check_pre_admission_error(case, &mut cache, &mut recorder)?;
    let counts = cache_counts(&cache);
    let expected_hits = case.spec.iterations * 2 + case.spec.iterations.saturating_sub(1);
    if counts.hits != expected_hits
        || counts.misses != 1
        || counts.compiles != 1
        || counts.publications != 1
        || counts.replacements != 0
    {
        return Err(err(format!(
            "{} final preflight cache counters mismatch: {counts:?}",
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
        interpreter: StateSnapshot::from(&case.oracle),
        ordinary: StateSnapshot::from(&ordinary_state),
        observed: StateSnapshot::from(&observed_state),
        ordinary_retired: ordinary.retired_native + ordinary.retired_interpreter,
        observed_retired: observed.retired_native + observed.retired_interpreter,
        resident_entry_count: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        final_cache_counts: counts,
        hit_only: true,
        phase_counts_valid: true,
        final_phase_counts: final_phases,
        zero_work_phase_counts,
        stop_phase_counts,
        event_phase_counts,
        pre_admission_error_phase_counts,
    })
}

#[derive(Clone, Copy)]
enum EarlyExit {
    Zero,
    Stop,
    Event,
}

fn check_early_exit(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    recorder: &mut PhaseRecorder,
    kind: EarlyExit,
) -> Result<Vec<PhaseObservation>, BenchError> {
    recorder.clear();
    let before = cache_counts(cache);
    let mut state = case.initial.clone();
    let event = AtomicBool::new(matches!(kind, EarlyExit::Event));
    let (budget, stop_rip, expected_exit) = match kind {
        EarlyExit::Zero => (0, None, DispatchExit::BudgetExhausted),
        EarlyExit::Stop => (
            case.budget,
            Some(cached_native_loop::BASE_RIP),
            DispatchExit::Runtime(RuntimeExitTag::StopAddress),
        ),
        EarlyExit::Event => (case.budget, None, DispatchExit::EventPending),
    };
    let expected_remaining = if matches!(kind, EarlyExit::Zero) {
        0
    } else {
        case.budget
    };
    let mut ordinary_state = case.initial.clone();
    let ordinary = ordinary_call(
        case,
        &mut ordinary_state,
        budget,
        cache,
        stop_rip,
        Some(&event),
    )?;
    let result = observed_call(
        case,
        &mut state,
        budget,
        cache,
        recorder,
        stop_rip,
        Some(&event),
    )?;
    if result != ordinary
        || result.exit != expected_exit
        || result.retired_native != 0
        || result.retired_interpreter != 0
        || result.blocks_executed != 0
        || result.remaining_budget != expected_remaining
        || state != case.initial
        || ordinary_state != case.initial
    {
        return Err(err(format!(
            "{} {:?} early exit did not preserve state/accounting",
            case.spec.id,
            kind_name(kind)
        )));
    }
    validate_zero_phase_counts(recorder, kind_name(kind))?;
    if cache_counts(cache) != before {
        return Err(err(format!(
            "{} {:?} early exit touched resident cache counters",
            case.spec.id,
            kind_name(kind)
        )));
    }
    Ok(phase_observations(recorder))
}

fn check_pre_admission_error(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    recorder: &mut PhaseRecorder,
) -> Result<Vec<PhaseObservation>, BenchError> {
    recorder.clear();
    let before = cache_counts(cache);
    let mut wrong_bytes = case.bytes.clone();
    wrong_bytes[0] ^= 1;
    let mut state = case.initial.clone();
    let stop = AtomicU8::new(0);
    let ordinary_view = CodeView::new(cached_native_loop::BASE_RIP, &wrong_bytes)
        .map_err(|e| err(format!("ordinary error-path view: {e:?}")))?;
    let ordinary_error = od_jit::dispatch::run_cached_native_register_v1_controlled(
        ordinary_view,
        &case.admission,
        case.context,
        &case.dependencies,
        &mut state,
        case.budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
    );
    if ordinary_error.is_ok() || state != case.initial {
        return Err(err(format!(
            "{} ordinary admission error path unexpectedly succeeded or mutated state",
            case.spec.id
        )));
    }
    let view = CodeView::new(cached_native_loop::BASE_RIP, &wrong_bytes)
        .map_err(|e| err(format!("observed error-path view: {e:?}")))?;
    let error = od_jit::dispatch::run_cached_native_register_v1_controlled_observed(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        &mut state,
        case.budget,
        DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        },
        cache,
        &mut PublishedBlockExecutor,
        recorder,
    );
    if ordinary_error != error || state != case.initial {
        return Err(err(format!(
            "{} ordinary/observed pre-admission error results differ or mutate state",
            case.spec.id
        )));
    }
    validate_zero_phase_counts(recorder, "pre-admission error")?;
    if cache_counts(cache) != before {
        return Err(err(format!(
            "{} admission error path touched resident cache counters",
            case.spec.id
        )));
    }
    Ok(phase_observations(recorder))
}

fn measure_stratum(case: &Prepared, run_started: Instant) -> Result<StratumReport, BenchError> {
    let mut cache = new_cache()?;
    let mut state = case.initial.clone();
    let cold_started = Instant::now();
    let cold = ordinary_call(case, &mut state, case.budget, &mut cache, None, None)?;
    let cold_first_dispatch_ns = cold_started.elapsed().as_nanos();
    validate_full(case, &state, cold)?;
    for _ in 0..32 {
        state.clone_from(&case.initial);
        let result = ordinary_call(case, &mut state, case.budget, &mut cache, None, None)?;
        validate_full(case, &state, result)?;
    }
    for _ in 0..32 {
        state.clone_from(&case.initial);
        let mut recorder = PhaseRecorder::default();
        let result = observed_call(
            case,
            &mut state,
            case.budget,
            &mut cache,
            &mut recorder,
            None,
            None,
        )?;
        validate_full(case, &state, result)?;
        validate_warm_phase_counts(case, &phase_observations(&recorder), 1)?;
    }
    let calibration_ordinary = sample(
        case,
        &mut cache,
        CALIBRATION_CALLS,
        false,
        0,
        "calibration_ordinary_first",
    )?;
    let calibration_observed = sample(
        case,
        &mut cache,
        CALIBRATION_CALLS,
        true,
        0,
        "calibration_observed_second",
    )?;
    let mut calls = CALIBRATION_CALLS.max(calls_for(
        calibration_ordinary.elapsed_ns,
        calibration_observed.elapsed_ns,
    )?);
    if calls > MAX_CALLS {
        return Err(err(format!(
            "{} calibration exceeds sample-call safety cap",
            case.spec.id
        )));
    }
    let mut restarts = Vec::new();
    let mut retained = Vec::new();
    for attempt in 0..6 {
        let mut pairs = Vec::with_capacity(PAIRS);
        let mut shortest = u128::MAX;
        for pair_idx in 0..PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err(
                    "30 minute benchmark run limit exceeded during phase samples",
                ));
            }
            let ordinary_first = pair_idx % 2 == 0;
            let (ordinary, observed, order) = if ordinary_first {
                let o = sample(case, &mut cache, calls, false, pair_idx, "ordinary_first")?;
                let p = sample(case, &mut cache, calls, true, pair_idx, "observed_second")?;
                (o, p, "ordinary_first")
            } else {
                let p = sample(case, &mut cache, calls, true, pair_idx, "observed_first")?;
                let o = sample(case, &mut cache, calls, false, pair_idx, "ordinary_second")?;
                (o, p, "observed_first")
            };
            shortest = shortest.min(ordinary.elapsed_ns).min(observed.elapsed_ns);
            pairs.push(make_pair(
                pair_idx,
                order,
                case.spec.seed,
                ordinary,
                observed,
            ));
        }
        if shortest >= MIN_SAMPLE.as_nanos() {
            retained = pairs;
            break;
        }
        let next = calls_for_sample(
            calls,
            pairs
                .iter()
                .flat_map(|p| [p.ordinary.elapsed_ns, p.observed.elapsed_ns])
                .min()
                .unwrap_or(0),
        )?;
        if next <= calls || next > MAX_CALLS {
            return Err(err(format!(
                "{} could not reach the per-side sample floor",
                case.spec.id
            )));
        }
        restarts.push(Restart {
            attempt,
            previous_calls: calls,
            next_calls: next,
            discarded_pairs: pairs,
        });
        calls = next;
    }
    if retained.len() != PAIRS {
        return Err(err(format!(
            "{} did not retain 20 pairs above 200 ms",
            case.spec.id
        )));
    }
    let final_counts = cache_counts(&cache);
    if final_counts.misses != 1
        || final_counts.compiles != 1
        || final_counts.publications != 1
        || final_counts.replacements != 0
    {
        return Err(err(format!(
            "{} warm phase samples were not resident: {final_counts:?}",
            case.spec.id
        )));
    }
    Ok(StratumReport {
        id: case.spec.id,
        code_hex: hex(&case.bytes),
        code_sha256: od_testkit::sha256_hex(&case.bytes),
        seed: case.spec.seed,
        iterations: case.spec.iterations,
        guest_work_per_call: case.budget,
        resident_entry_count: cache.resident_entries(),
        resident_code_bytes: cache.resident_code_bytes(),
        resident_logical_bytes: cache.resident_logical_bytes(),
        cold_first_dispatch_ns,
        ordinary_warmup_calls_per_side: 32,
        observed_warmup_calls_per_side: 32,
        calibration_calls_per_side: CALIBRATION_CALLS,
        calibration_ordinary,
        calibration_observed,
        retained_pairs: retained,
        discarded_restarts: restarts,
    })
}

fn sample(
    case: &Prepared,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    calls: u64,
    observed: bool,
    pair: usize,
    order: &'static str,
) -> Result<Sample, BenchError> {
    let before = cache_counts(cache);
    let mut recorder = PhaseRecorder::default();
    let mut state = case.initial.clone();
    let mut elapsed_ns = 0u128;
    let mut retired = 0u64;
    for _ in 0..calls {
        state.clone_from(&case.initial);
        let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
            .map_err(|e| err(format!("{} CodeView: {e:?}", case.spec.id)))?;
        let stop = AtomicU8::new(0);
        let start = Instant::now();
        let result = if observed {
            od_jit::dispatch::run_cached_native_register_v1_controlled_observed(
                view,
                &case.admission,
                case.context,
                &case.dependencies,
                &mut state,
                case.budget,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: None,
                },
                cache,
                &mut PublishedBlockExecutor,
                &mut recorder,
            )
        } else {
            od_jit::dispatch::run_cached_native_register_v1_controlled(
                view,
                &case.admission,
                case.context,
                &case.dependencies,
                &mut state,
                case.budget,
                DispatchControl {
                    stop_rip: None,
                    stop_requested: &stop,
                    event_requested: None,
                },
                cache,
                &mut PublishedBlockExecutor,
            )
        }
        .map_err(|e| err(format!("{} sample dispatch: {e:?}", case.spec.id)))?;
        elapsed_ns += start.elapsed().as_nanos();
        validate_full(case, &state, result)?;
        retired = retired
            .checked_add(result.retired_native + result.retired_interpreter)
            .ok_or_else(|| err("phase sample retirement overflow"))?;
    }
    let after = cache_counts(cache);
    validate_hit_delta(case, before, after, calls * case.spec.iterations)?;
    let phases = if observed {
        phase_observations(&recorder)
    } else {
        Vec::new()
    };
    if observed {
        validate_warm_phase_counts(case, &phases, calls)?;
    }
    let phase_sum_ns = phases.iter().map(|phase| phase.elapsed_ns).sum();
    Ok(Sample {
        pair,
        order,
        seed: case.spec.seed,
        calls,
        guest_instructions: calls * case.budget,
        retired,
        elapsed_ns,
        phase_sum_ns,
        residual_ns: if observed {
            Some(elapsed_ns as i128 - phase_sum_ns as i128)
        } else {
            None
        },
        phase_totals: phases,
        cache: CacheCounts {
            hits: after.hits - before.hits,
            misses: after.misses - before.misses,
            compiles: after.compiles - before.compiles,
            publications: after.publications - before.publications,
            replacements: after.replacements - before.replacements,
        },
    })
}

fn new_cache() -> Result<SingleEntryBlockCache<PublishedCode>, BenchError> {
    SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
        .map_err(|e| err(format!("capacity-one cache setup: {e:?}")))
}

fn ordinary_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    stop_rip: Option<u64>,
    event: Option<&AtomicBool>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|e| err(format!("{} ordinary CodeView: {e:?}", case.spec.id)))?;
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
            event_requested: event,
        },
        cache,
        &mut PublishedBlockExecutor,
    )
    .map_err(|e| err(format!("{} ordinary dispatch: {e:?}", case.spec.id)))
}

fn observed_call(
    case: &Prepared,
    state: &mut LongModeState,
    budget: u64,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    recorder: &mut PhaseRecorder,
    stop_rip: Option<u64>,
    event: Option<&AtomicBool>,
) -> Result<FallbackDispatchResult, BenchError> {
    let view = CodeView::new(cached_native_loop::BASE_RIP, &case.bytes)
        .map_err(|e| err(format!("{} observed CodeView: {e:?}", case.spec.id)))?;
    let stop = AtomicU8::new(0);
    od_jit::dispatch::run_cached_native_register_v1_controlled_observed(
        view,
        &case.admission,
        case.context,
        &case.dependencies,
        state,
        budget,
        DispatchControl {
            stop_rip,
            stop_requested: &stop,
            event_requested: event,
        },
        cache,
        &mut PublishedBlockExecutor,
        recorder,
    )
    .map_err(|e| err(format!("{} observed dispatch: {e:?}", case.spec.id)))
}

fn validate_full(
    case: &Prepared,
    state: &LongModeState,
    result: FallbackDispatchResult,
) -> Result<(), BenchError> {
    if result.exit != DispatchExit::BudgetExhausted
        || result.retired_native != case.budget
        || result.retired_interpreter != 0
        || result.remaining_budget != 0
        || result.blocks_executed != case.spec.iterations
        || *state != case.oracle
    {
        return Err(err(format!(
            "{} full dispatch parity/accounting failed: {:?}, native/interpreter retired {}/{}, remaining {}, blocks {}, state_equal {}",
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

fn cache_counts(cache: &SingleEntryBlockCache<PublishedCode>) -> CacheCounts {
    let c = cache.counters();
    CacheCounts {
        hits: c.hits,
        misses: c.misses,
        compiles: c.successful_compiles,
        publications: c.successful_publications,
        replacements: c.replacements,
    }
}

fn validate_hit_delta(
    case: &Prepared,
    before: CacheCounts,
    after: CacheCounts,
    expected_hits: u64,
) -> Result<(), BenchError> {
    let delta = CacheCounts {
        hits: after.hits - before.hits,
        misses: after.misses - before.misses,
        compiles: after.compiles - before.compiles,
        publications: after.publications - before.publications,
        replacements: after.replacements - before.replacements,
    };
    if delta.hits != expected_hits
        || delta.misses != 0
        || delta.compiles != 0
        || delta.publications != 0
        || delta.replacements != 0
    {
        return Err(err(format!(
            "{} resident path was not hit-only: {delta:?}",
            case.spec.id
        )));
    }
    Ok(())
}

fn phase_observations(recorder: &PhaseRecorder) -> Vec<PhaseObservation> {
    recorder
        .totals()
        .into_iter()
        .map(|p: PhaseTotal| PhaseObservation {
            phase: format!("{:?}", p.phase),
            calls: p.calls,
            elapsed_ns: p.elapsed_ns,
        })
        .collect()
}

fn validate_warm_phase_counts(
    case: &Prepared,
    phases: &[PhaseObservation],
    calls: u64,
) -> Result<(), BenchError> {
    let expected = calls * case.spec.iterations;
    for p in phases {
        let wanted = match p.phase.as_str() {
            "BlockBuildAndAdmission" | "KeySnapshotAndLookup" | "HostExecutionBoundary" => expected,
            _ => 0,
        };
        if p.calls != wanted {
            return Err(err(format!(
                "{} phase {} count {}, expected {wanted}",
                case.spec.id, p.phase, p.calls
            )));
        }
    }
    if phases.len() != 7 {
        return Err(err(format!(
            "{} observer reported {} phases, expected 7",
            case.spec.id,
            phases.len()
        )));
    }
    Ok(())
}

fn validate_zero_phase_counts(recorder: &PhaseRecorder, label: &str) -> Result<(), BenchError> {
    if recorder
        .totals()
        .iter()
        .any(|p| p.calls != 0 || p.elapsed_ns != 0)
    {
        return Err(err(format!("{label} early exit recorded nonzero phases")));
    }
    Ok(())
}

fn calls_for(ordinary_ns: u128, observed_ns: u128) -> Result<u64, BenchError> {
    let shortest = ordinary_ns.min(observed_ns);
    if shortest == 0 {
        return Err(err("zero-duration paired calibration"));
    }
    let per_call = shortest.div_ceil(u128::from(CALIBRATION_CALLS));
    let calls = MIN_SAMPLE
        .as_nanos()
        .div_ceil(per_call)
        .saturating_mul(6)
        .div_ceil(5)
        .max(u128::from(CALIBRATION_CALLS));
    u64::try_from(calls).map_err(|_| err("calibrated call count overflow"))
}

fn calls_for_sample(current_calls: u64, shortest_elapsed: u128) -> Result<u64, BenchError> {
    if shortest_elapsed == 0 {
        return Err(err("zero-duration sample during recalibration"));
    }
    let calls = u128::from(current_calls)
        .saturating_mul(MIN_SAMPLE.as_nanos())
        .div_ceil(shortest_elapsed)
        .saturating_mul(6)
        .div_ceil(5);
    u64::try_from(calls).map_err(|_| err("sample restart call count overflow"))
}

fn make_pair(
    pair: usize,
    order: &'static str,
    seed: u64,
    ordinary: Sample,
    observed: Sample,
) -> Pair {
    let overhead = observed.elapsed_ns as i128 - ordinary.elapsed_ns as i128;
    Pair {
        pair,
        order,
        seed,
        observed_minus_ordinary_elapsed_ns: overhead,
        observed_over_ordinary_ratio: observed.elapsed_ns as f64 / ordinary.elapsed_ns as f64,
        ordinary,
        observed,
    }
}

fn kind_name(kind: EarlyExit) -> &'static str {
    match kind {
        EarlyExit::Zero => "zero_work",
        EarlyExit::Stop => "stop",
        EarlyExit::Event => "event",
    }
}

fn fingerprints() -> Vec<(&'static str, String)> {
    [
        ("tools/bench/src/cached_native_loop_phases.rs", SOURCE),
        (
            "tools/bench/src/cached_native_loop.rs",
            include_bytes!("cached_native_loop.rs").as_slice(),
        ),
        ("tools/bench/src/main.rs", CLI_SOURCE),
        ("tools/bench/src/lib.rs", LIB_SOURCE),
        ("crates/od-jit/src/dispatch.rs", DISPATCH_SOURCE),
        ("crates/od-jit/src/block.rs", BLOCK_SOURCE),
        ("crates/od-host/src/lib.rs", HOST_SOURCE),
        ("crates/od-jit/src/phase_observation.rs", OBSERVER_SOURCE),
    ]
    .into_iter()
    .map(|(path, bytes)| (path, od_testkit::sha256_hex(bytes)))
    .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
