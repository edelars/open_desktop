//! Native end-to-end dispatch/cache comparison. Fixture loading, full decode
//! admission, and interpreter oracles happen before timing.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use od_host::{PublishedArenaBackend, PublishedArenaBlock, PublishedBlockExecutor};
use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_jit::block::{AdmittedInstructionBoundaries, CodeView};
use od_jit::dispatch::{
    CacheEvictionPolicy, DispatchExit, MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
    MAX_MULTI_ENTRY_CACHE_METADATA_BYTES, MAX_SINGLE_ENTRY_CACHE_BYTES, MultiEntryBlockCache,
    SingleEntryBlockCache, run as dispatch_run, run_cached, run_multi_cached,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, MappingId,
    MappingVersion,
};
use od_testkit::case::{ExpectedExit, Fixture};
use serde::Serialize;

use crate::{
    HostMetadata, MAX_INPUT_JSON_BYTES, MAX_INPUTS, MAX_RUN_TIME, MAX_TOTAL_TRACE_VISITS,
    MAX_VISITS_PER_INPUT, RecordedTrace,
};

const PAIRS: usize = 20;
const FIRST_RUN_PAIRS: usize = 20;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const ARENA_SLOTS: usize = 16;
const MAX_CASE_LIST_BYTES: usize = 4 * 1024 * 1024;
const FROZEN_CASE_LIST_SHA256: &str =
    "b9276b52556b3692552de074e83cd8aef0c777f29861bd19ab09c188e41615d3";
const FROZEN_TRACE_SHA256: &str =
    "bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3";

#[derive(Debug)]
pub struct BenchError(String);
impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for BenchError {}
fn err(message: impl Into<String>) -> BenchError {
    BenchError(message.into())
}

struct Case {
    id: String,
    bytes: Vec<u8>,
    admission: AdmittedInstructionBoundaries,
    initial: LongModeState,
    expected: LongModeState,
    budget: u64,
    exit: DispatchExit,
    retired: u64,
    remaining: u64,
    flags_mask: u64,
    identity: CodeIdentity,
    entry_count: usize,
    unique_entries: usize,
}

#[derive(Clone, Debug, Serialize)]
struct Scenario {
    name: String,
    input_indices: Vec<usize>,
    warmup_sweeps: usize,
    minimum_measured_sweeps: usize,
    cold_each_sweep: bool,
    selected_case_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum VariantName {
    UncachedPublishedBlockExecutor,
    SingleEntryArena16,
    MultiFifo16,
    MultiLru16,
}

enum Variant {
    Uncached(PublishedBlockExecutor),
    Single {
        backend: PublishedArenaBackend,
        cache: SingleEntryBlockCache<PublishedArenaBlock>,
    },
    Fifo {
        backend: PublishedArenaBackend,
        cache: MultiEntryBlockCache<PublishedArenaBlock>,
    },
    Lru {
        backend: PublishedArenaBackend,
        cache: MultiEntryBlockCache<PublishedArenaBlock>,
    },
}

impl Variant {
    fn new(name: VariantName) -> Result<Self, BenchError> {
        match name {
            VariantName::UncachedPublishedBlockExecutor => {
                Ok(Self::Uncached(PublishedBlockExecutor))
            }
            VariantName::SingleEntryArena16 => Ok(Self::Single {
                backend: PublishedArenaBackend::new(ARENA_SLOTS)
                    .map_err(|e| err(format!("arena: {e:?}")))?,
                cache: SingleEntryBlockCache::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
                    .map_err(|e| err(format!("cache: {e:?}")))?,
            }),
            VariantName::MultiFifo16 | VariantName::MultiLru16 => {
                let backend = PublishedArenaBackend::new(ARENA_SLOTS)
                    .map_err(|e| err(format!("arena: {e:?}")))?;
                let policy = if matches!(name, VariantName::MultiLru16) {
                    CacheEvictionPolicy::Lru
                } else {
                    CacheEvictionPolicy::Fifo
                };
                let cache = MultiEntryBlockCache::with_policy(
                    ARENA_SLOTS,
                    MAX_MULTI_ENTRY_CACHE_CODE_BYTES,
                    MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
                    policy,
                )
                .map_err(|e| err(format!("cache: {e:?}")))?;
                if cache.max_entries() > backend.capacity() {
                    return Err(err("cache exceeds arena capacity"));
                }
                if policy == CacheEvictionPolicy::Lru {
                    Ok(Self::Lru { backend, cache })
                } else {
                    Ok(Self::Fifo { backend, cache })
                }
            }
        }
    }
    fn call(
        &mut self,
        case: &Case,
        state: &mut LongModeState,
    ) -> Result<od_jit::dispatch::DispatchResult, BenchError> {
        let code = CodeView::new(0, &case.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
        let dep = [CodeDependencySpan {
            start_rip: 0,
            end_rip: code.code_end(),
            identity: case.identity,
        }];
        match self {
            Self::Uncached(executor) => {
                dispatch_run(code, &case.admission, state, case.budget, executor)
                    .map_err(|e| err(format!("dispatch: {e:?}")))
            }
            Self::Single { backend, cache } => run_cached(
                code,
                &case.admission,
                crate::benchmark_execution_context(),
                &dep,
                state,
                case.budget,
                cache,
                backend,
            )
            .map_err(|e| err(format!("single cache: {e:?}"))),
            Self::Fifo { backend, cache } | Self::Lru { backend, cache } => run_multi_cached(
                code,
                &case.admission,
                crate::benchmark_execution_context(),
                &dep,
                state,
                case.budget,
                cache,
                backend,
            )
            .map_err(|e| err(format!("multi cache: {e:?}"))),
        }
    }
    fn counters(&self) -> Counters {
        match self {
            Self::Uncached(_) => Counters {
                cache_lookup_metrics_available: false,
                ..Counters::default()
            },
            Self::Single { backend, cache } => {
                let c = cache.counters();
                Counters {
                    hits: c.hits,
                    misses: c.misses,
                    compiles: c.successful_compiles,
                    publications: c.successful_publications,
                    evictions: c.replacements,
                    cache_lookup_metrics_available: true,
                    resident: c.resident_entries_high_water,
                    code_bytes: c.resident_code_bytes_high_water,
                    logical_bytes: c.resident_logical_metadata_bytes_high_water,
                    arena_capacity: backend.capacity(),
                    arena_free: backend
                        .capacity()
                        .saturating_sub(c.resident_entries_high_water),
                    arena_reserved_bytes: backend.reserved_mapping_bytes(),
                    ..Counters::default()
                }
            }
            Self::Fifo { backend, cache } | Self::Lru { backend, cache } => {
                let c = cache.counters();
                Counters {
                    hits: c.hits,
                    misses: c.misses,
                    compiles: c.successful_compiles,
                    publications: c.successful_publications,
                    evictions: c.evictions,
                    cache_lookup_metrics_available: true,
                    resident: c.resident_entries_high_water,
                    code_bytes: c.resident_code_bytes_high_water,
                    logical_bytes: c.resident_logical_metadata_bytes_high_water,
                    arena_capacity: backend.capacity(),
                    arena_free: backend
                        .capacity()
                        .saturating_sub(c.resident_entries_high_water),
                    arena_reserved_bytes: backend.reserved_mapping_bytes(),
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct Counters {
    cache_lookup_metrics_available: bool,
    hits: u64,
    misses: u64,
    compiles: u64,
    publications: u64,
    evictions: u64,
    #[serde(rename = "resident_entries_high_water")]
    resident: usize,
    #[serde(rename = "resident_code_bytes_high_water")]
    code_bytes: usize,
    #[serde(rename = "resident_logical_metadata_bytes_high_water")]
    logical_bytes: usize,
    arena_capacity: usize,
    #[serde(rename = "arena_available_slots_minimum_observed")]
    arena_free: usize,
    arena_reserved_bytes: usize,
}
#[derive(Clone, Debug, Serialize)]
struct Sample {
    pair: usize,
    order: usize,
    elapsed_ns: u128,
    sweeps: usize,
    calls: u64,
    native_block_executions: u64,
    retired_guest_instructions: u64,
    counters: Counters,
}
#[derive(Clone, Debug, Serialize)]
struct VariantResult {
    variant: VariantName,
    samples: Vec<Sample>,
    median_ns: u128,
    min_ns: u128,
    max_ns: u128,
    iqr_ns: u128,
}
#[derive(Clone, Debug, Serialize)]
struct ScenarioResult {
    scenario: Scenario,
    calibration_sweeps_by_variant: Vec<usize>,
    common_repetitions_per_sample: usize,
    variants: Vec<VariantResult>,
    paired_ratios_to_uncached: Vec<Vec<f64>>,
}
#[derive(Serialize)]
struct Report {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    visits: usize,
    timing_scope: String,
    excluded_from_timing: String,
    arena_slots: usize,
    arena_slot_bytes: usize,
    total_elapsed_ns: u128,
    scenarios: Vec<ScenarioResult>,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum FirstRunVariantName {
    Interpreter,
    Fifo16ColdBlockDispatcher,
    Fifo16WarmBlockDispatcher,
}

#[derive(Clone, Debug, Serialize)]
struct FirstRunSample {
    pair: usize,
    order: usize,
    sweeps: usize,
    calls: u64,
    retired_guest_instructions: u64,
    native_block_executions: u64,
    initialization_ns: u128,
    execution_ns: u128,
    elapsed_ns: u128,
    cache_hits: u64,
    cache_misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    arena_reserved_bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct FirstRunVariantResult {
    variant: FirstRunVariantName,
    samples: Vec<FirstRunSample>,
    median_ns_per_sweep: u128,
    min_ns_per_sweep: u128,
    max_ns_per_sweep: u128,
    iqr_ns_per_sweep: u128,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct FirstRunSeriesRestart {
    failed_pair: usize,
    attempted_sweeps: usize,
    fifo_elapsed_ns: u128,
    interpreter_elapsed_ns: u128,
    next_sweeps: usize,
}

#[derive(Serialize)]
struct FirstRunReport {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    visits: usize,
    calibration_sweeps: Vec<usize>,
    paired_calibration_attempts: usize,
    measured_series_restarts: Vec<FirstRunSeriesRestart>,
    measured_pairs: usize,
    common_sweeps_per_sample: usize,
    timing_scope: String,
    excluded_from_timing: String,
    variants: Vec<FirstRunVariantResult>,
    paired_interpreter_over_fifo_ratios: Vec<f64>,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
struct WarmRunReport {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    visits: usize,
    selected_cases: usize,
    selected_entry_budget: usize,
    selected_case_ids: Vec<String>,
    cache_capacity_entries: usize,
    warmup_dispatch_elapsed_ns: u128,
    warmup_cache_hits: u64,
    warmup_cache_misses: u64,
    warmup_successful_compiles: u64,
    warmup_successful_publications: u64,
    warmup_evictions: u64,
    calibration_sweeps: Vec<usize>,
    paired_calibration_attempts: usize,
    measured_series_restarts: Vec<FirstRunSeriesRestart>,
    measured_pairs: usize,
    common_sweeps_per_sample: usize,
    timing_scope: String,
    excluded_from_timing: String,
    variants: Vec<FirstRunVariantResult>,
    paired_interpreter_over_fifo_ratios: Vec<f64>,
    total_elapsed_ns: u128,
}

pub fn run(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("dispatch-cache measurement requires a release build"));
    }
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be 1..=3"));
    }
    let started = Instant::now();
    let list = read_bounded(case_list, MAX_CASE_LIST_BYTES, "case list")?;
    let trace_raw = read_bounded(trace_path, crate::MAX_LOGICAL_BYTES, "trace")?;
    if od_testkit::sha256_hex(&list) != FROZEN_CASE_LIST_SHA256
        || od_testkit::sha256_hex(&trace_raw) != FROZEN_TRACE_SHA256
    {
        return Err(err(
            "case list or trace does not match the frozen S3-17 corpus",
        ));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.visits.len() != 1_322 || trace.visits.len() > MAX_TOTAL_TRACE_VISITS {
        return Err(err("frozen trace visit count mismatch"));
    }
    let cases = prepare_cases(&list, &trace, started)?;
    let plans = plans(&cases)?;
    if plans.len() > crate::MAX_SCENARIOS {
        return Err(err("scenario count exceeds configured bound"));
    }
    let names = [
        VariantName::UncachedPublishedBlockExecutor,
        VariantName::SingleEntryArena16,
        VariantName::MultiFifo16,
        VariantName::MultiLru16,
    ];
    let mut output = Vec::new();
    for (si, scenario) in plans.into_iter().enumerate() {
        let mut calibrated = Vec::with_capacity(names.len());
        for name in names {
            calibrated.push(calibrate_sweeps(name, &scenario, &cases, started)?);
        }
        let repetitions = calibrated
            .iter()
            .copied()
            .max()
            .unwrap_or(1)
            .max(scenario.minimum_measured_sweeps);
        let mut samples_by_variant: Vec<Vec<Sample>> = (0..names.len())
            .map(|_| Vec::with_capacity(PAIRS))
            .collect();
        for pair in 0..PAIRS {
            for order in 0..names.len() {
                let vi = (order + pair + si) % names.len();
                let mut engine = Variant::new(names[vi])?;
                for _ in 0..scenario.warmup_sweeps {
                    let _ = sweep(&mut engine, &cases, &scenario.input_indices, started)?;
                }
                let (time, calls, native_blocks, retired, high) = run_fixed_sweeps(
                    &mut engine,
                    names[vi],
                    &scenario,
                    &cases,
                    started,
                    repetitions,
                )?;
                let expected_calls = u64::try_from(scenario.input_indices.len())
                    .ok()
                    .and_then(|count| count.checked_mul(repetitions as u64))
                    .ok_or_else(|| err("sample operation count overflow"))?;
                if calls != expected_calls {
                    return Err(err("paired sample operation count mismatch"));
                }
                if time < MIN_SAMPLE {
                    return Err(err(format!(
                        "timed sample below 200ms for {}",
                        scenario.name
                    )));
                }
                samples_by_variant[vi].push(Sample {
                    pair,
                    order,
                    elapsed_ns: time.as_nanos(),
                    sweeps: repetitions,
                    calls,
                    native_block_executions: native_blocks,
                    retired_guest_instructions: retired,
                    counters: high,
                });
            }
        }
        let variant_results: Vec<VariantResult> = names
            .iter()
            .copied()
            .zip(samples_by_variant)
            .map(|(name, samples)| summarize(name, samples))
            .collect();
        let baseline = variant_results
            .iter()
            .find(|r| matches!(r.variant, VariantName::UncachedPublishedBlockExecutor))
            .unwrap();
        let ratios = variant_results
            .iter()
            .map(|r| {
                r.samples
                    .iter()
                    .zip(&baseline.samples)
                    .map(|(sample, base)| sample.elapsed_ns as f64 / base.elapsed_ns as f64)
                    .collect()
            })
            .collect();
        output.push(ScenarioResult {
            scenario,
            calibration_sweeps_by_variant: calibrated,
            common_repetitions_per_sample: repetitions,
            variants: variant_results,
            paired_ratios_to_uncached: ratios,
        });
    }
    let total_elapsed_ns = started.elapsed().as_nanos();
    let report=Report{schema:1,process_run,case_list_sha256:od_testkit::sha256_hex(&list),trace_sha256:od_testkit::sha256_hex(&trace_raw),capture_host:trace.capture_host.clone(),measure_host:HostMetadata::current(),inputs:trace.inputs.len(),visits:trace.visits.len(),timing_scope:"dispatch-call intervals accumulated; state reset and result validation outside timer; cache retained between warm measured sweeps".into(),excluded_from_timing:"fixture/trace parsing, full decoder admission, interpreter oracle, arena/cache construction and destruction, state clone/reset, and result validation".into(),arena_slots:ARENA_SLOTS,arena_slot_bytes:od_host::ARENA_SLOT_BYTES,total_elapsed_ns,scenarios:output};
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

/// Measures one complete register-only corpus pass from an empty FIFO block
/// cache against the interpreter on the same frozen fixtures. Each measured
/// JIT sweep owns a new arena/cache; runtime teardown and harness state/reset
/// validation are intentionally outside the timed intervals.
pub fn run_cold_first_execution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cold-first-run measurement requires a release build"));
    }
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be 1..=3"));
    }
    let started = Instant::now();
    let list = read_bounded(case_list, MAX_CASE_LIST_BYTES, "case list")?;
    let trace_raw = read_bounded(trace_path, crate::MAX_LOGICAL_BYTES, "trace")?;
    if od_testkit::sha256_hex(&list) != FROZEN_CASE_LIST_SHA256
        || od_testkit::sha256_hex(&trace_raw) != FROZEN_TRACE_SHA256
    {
        return Err(err(
            "case list or trace does not match the frozen S3-17 corpus",
        ));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.visits.len() != 1_322 || trace.visits.len() > MAX_TOTAL_TRACE_VISITS {
        return Err(err("frozen trace visit count mismatch"));
    }
    let cases = prepare_cases(&list, &trace, started)?;
    if cases.len() != MAX_INPUTS {
        return Err(err("cold-first-run requires the complete frozen corpus"));
    }

    // Native preflight is deliberately outside timing and the fresh engine is
    // dropped before calibration, so it cannot leave a compiled block behind.
    let _ = cold_fifo_sweep(&cases, started)?;
    let mut calibration = [Duration::ZERO; 2];
    let mut calibration_sweeps = [0usize; 2];
    while calibration.iter().any(|elapsed| *elapsed < MIN_SAMPLE) {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during cold calibration"));
        }
        if calibration[0] < MIN_SAMPLE {
            let sweep = cold_fifo_sweep(&cases, started)?;
            calibration[0] = calibration[0]
                .saturating_add(sweep.initialization)
                .saturating_add(sweep.execution);
            calibration_sweeps[0] += 1;
        }
        if calibration[1] < MIN_SAMPLE {
            let sweep = interpreter_sweep(&cases, started)?;
            calibration[1] = calibration[1].saturating_add(sweep.execution);
            calibration_sweeps[1] += 1;
        }
    }
    let initial_repetitions = calibration_sweeps.into_iter().max().unwrap_or(1).max(1);
    let (repetitions, paired_calibration_attempts) =
        calibrate_common_first_run_sweeps(&cases, started, initial_repetitions)?;

    let measured = collect_first_run_series(repetitions, FIRST_RUN_PAIRS, |pair, sweeps| {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during cold samples"));
        }
        let fifo_first = pair % 2 == 0;
        let (fifo_order, interpreter_order) = if fifo_first { (0, 1) } else { (1, 0) };
        if fifo_first {
            Ok((
                measure_cold_fifo_sample(pair, fifo_order, sweeps, &cases, started)?,
                measure_interpreter_sample(pair, interpreter_order, sweeps, &cases, started)?,
            ))
        } else {
            let interpreter =
                measure_interpreter_sample(pair, interpreter_order, sweeps, &cases, started)?;
            let fifo = measure_cold_fifo_sample(pair, fifo_order, sweeps, &cases, started)?;
            Ok((fifo, interpreter))
        }
    })?;

    let fifo_result = summarize_first_run(
        FirstRunVariantName::Fifo16ColdBlockDispatcher,
        measured.fifo_samples,
    );
    let interpreter_result = summarize_first_run(
        FirstRunVariantName::Interpreter,
        measured.interpreter_samples,
    );
    let fifo_by_pair: std::collections::HashMap<usize, u128> = fifo_result
        .samples
        .iter()
        .map(|sample| (sample.pair, sample.elapsed_ns / sample.sweeps as u128))
        .collect();
    let ratios = interpreter_result
        .samples
        .iter()
        .map(|sample| {
            let interpreter = sample.elapsed_ns as f64 / sample.sweeps as f64;
            let fifo = fifo_by_pair[&sample.pair] as f64;
            interpreter / fifo
        })
        .collect();
    let report = FirstRunReport {
        schema: 2,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host.clone(),
        measure_host: HostMetadata::current(),
        inputs: trace.inputs.len(),
        visits: trace.visits.len(),
        calibration_sweeps: calibration_sweeps.to_vec(),
        paired_calibration_attempts,
        measured_series_restarts: measured.restarts,
        measured_pairs: FIRST_RUN_PAIRS,
        common_sweeps_per_sample: measured.common_sweeps,
        timing_scope: "per-sample sum of fresh FIFO arena/cache initialization plus each corpus dispatch-call interval; interpreter-call intervals for the same case order; each JIT sweep starts with an empty cache; cache may be reused within a corpus sweep".into(),
        excluded_from_timing: "fixture/trace parsing and admission, interpreter oracle preparation, per-case state clone/reset, post-call result validation, runtime teardown, and process launch".into(),
        variants: vec![fifo_result, interpreter_result],
        paired_interpreter_over_fifo_ratios: ratios,
        total_elapsed_ns: started.elapsed().as_nanos(),
    };
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

/// Measures repeated execution of a deterministic resident subset of the frozen
/// register-only corpus. The full corpus exceeds FIFO16 capacity and is not a
/// valid no-compile warm workload.
pub fn run_warm_first_execution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("warm-first-run measurement requires a release build"));
    }
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be 1..=3"));
    }
    let started = Instant::now();
    let list = read_bounded(case_list, MAX_CASE_LIST_BYTES, "case list")?;
    let trace_raw = read_bounded(trace_path, crate::MAX_LOGICAL_BYTES, "trace")?;
    if od_testkit::sha256_hex(&list) != FROZEN_CASE_LIST_SHA256
        || od_testkit::sha256_hex(&trace_raw) != FROZEN_TRACE_SHA256
    {
        return Err(err(
            "case list or trace does not match the frozen S3-17 corpus",
        ));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.visits.len() != 1_322 || trace.visits.len() > MAX_TOTAL_TRACE_VISITS {
        return Err(err("frozen trace visit count mismatch"));
    }
    let all_cases = prepare_cases(&list, &trace, started)?;
    if all_cases.len() != MAX_INPUTS {
        return Err(err("warm-first-run requires the complete frozen corpus"));
    }
    let (selected_case_count, selected_entry_budget) = select_resident_case_count(&all_cases)?;
    let cases = &all_cases[..selected_case_count];

    // Warm the sole FIFO runtime once; the same runtime remains alive through
    // calibration and all measured samples. This work is recorded but untimed.
    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    let warmup = warm_fifo_sweep(&mut engine, &cases, started)?;
    let warmup_counters = warmup.counters;
    if warmup_counters.misses == 0
        || warmup_counters.compiles == 0
        || warmup_counters.compiles != warmup_counters.misses
        || warmup_counters.publications != warmup_counters.compiles
        || warmup_counters.evictions != 0
        || warmup_counters.resident > ARENA_SLOTS
    {
        return Err(err(
            "FIFO warmup did not establish the expected resident cache",
        ));
    }

    let mut calibration_elapsed = [Duration::ZERO; 2];
    let mut calibration_sweeps = [0usize; 2];
    let mut calibration_order = 0usize;
    while calibration_elapsed
        .iter()
        .any(|elapsed| *elapsed < MIN_SAMPLE)
    {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during warm calibration"));
        }
        let fifo_first = calibration_order % 2 == 0;
        calibration_order = calibration_order.saturating_add(1);
        for variant_index in if fifo_first { [0, 1] } else { [1, 0] } {
            if calibration_elapsed[variant_index] >= MIN_SAMPLE {
                continue;
            }
            if variant_index == 0 {
                let sample = measure_warm_fifo_sample(0, 0, 1, &cases, &mut engine, started)?;
                calibration_elapsed[0] = calibration_elapsed[0].saturating_add(
                    Duration::from_nanos(u64::try_from(sample.elapsed_ns).unwrap_or(u64::MAX)),
                );
            } else {
                let sample = measure_interpreter_sample(0, 1, 1, &cases, started)?;
                calibration_elapsed[1] = calibration_elapsed[1].saturating_add(
                    Duration::from_nanos(u64::try_from(sample.elapsed_ns).unwrap_or(u64::MAX)),
                );
            }
            calibration_sweeps[variant_index] = calibration_sweeps[variant_index]
                .checked_add(1)
                .ok_or_else(|| err("warm calibration sweep count overflow"))?;
        }
    }
    let initial_repetitions = calibration_sweeps.into_iter().max().unwrap_or(1).max(1);
    let (repetitions, paired_calibration_attempts) =
        calibrate_common_warm_sweeps(&cases, started, initial_repetitions, &mut engine)?;

    let measured = collect_first_run_series(repetitions, FIRST_RUN_PAIRS, |pair, sweeps| {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during warm samples"));
        }
        let fifo_first = pair % 2 == 0;
        let (fifo_order, interpreter_order) = if fifo_first { (0, 1) } else { (1, 0) };
        if fifo_first {
            Ok((
                measure_warm_fifo_sample(pair, fifo_order, sweeps, &cases, &mut engine, started)?,
                measure_interpreter_sample(pair, interpreter_order, sweeps, &cases, started)?,
            ))
        } else {
            let interpreter =
                measure_interpreter_sample(pair, interpreter_order, sweeps, &cases, started)?;
            let fifo =
                measure_warm_fifo_sample(pair, fifo_order, sweeps, &cases, &mut engine, started)?;
            Ok((fifo, interpreter))
        }
    })?;

    let fifo_result = summarize_first_run(
        FirstRunVariantName::Fifo16WarmBlockDispatcher,
        measured.fifo_samples,
    );
    let interpreter_result = summarize_first_run(
        FirstRunVariantName::Interpreter,
        measured.interpreter_samples,
    );
    let fifo_by_pair: std::collections::HashMap<usize, u128> = fifo_result
        .samples
        .iter()
        .map(|sample| (sample.pair, sample.elapsed_ns))
        .collect();
    let ratios = interpreter_result
        .samples
        .iter()
        .map(|sample| {
            // Both variants use the same calibrated sweep count per pair, so
            // the raw elapsed ratio avoids integer normalization truncation.
            let interpreter = sample.elapsed_ns as f64;
            let fifo = fifo_by_pair[&sample.pair] as f64;
            interpreter / fifo
        })
        .collect();
    let report = WarmRunReport {
        schema: 1,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host.clone(),
        measure_host: HostMetadata::current(),
        inputs: trace.inputs.len(),
        visits: trace.visits.len(),
        selected_cases: cases.len(),
        selected_entry_budget,
        selected_case_ids: cases.iter().map(|case| case.id.clone()).collect(),
        cache_capacity_entries: ARENA_SLOTS,
        warmup_dispatch_elapsed_ns: warmup.execution.as_nanos(),
        warmup_cache_hits: warmup_counters.hits,
        warmup_cache_misses: warmup_counters.misses,
        warmup_successful_compiles: warmup_counters.compiles,
        warmup_successful_publications: warmup_counters.publications,
        warmup_evictions: warmup_counters.evictions,
        calibration_sweeps: calibration_sweeps.to_vec(),
        paired_calibration_attempts,
        measured_series_restarts: measured.restarts,
        measured_pairs: FIRST_RUN_PAIRS,
        common_sweeps_per_sample: measured.common_sweeps,
        timing_scope: "per-sample sum of FIFO dispatch-call intervals on one prewarmed, resident FIFO16 runtime and interpreter-call intervals over the same selected cases; per-case state cloning/reset and output validation occur outside timed call intervals".into(),
        excluded_from_timing: "fixture/trace parsing and admission, initial FIFO cache warmup, interpreter oracle preparation, per-case state clone/reset, post-call result validation, runtime initialization/teardown, and process launch".into(),
        variants: vec![fifo_result, interpreter_result],
        paired_interpreter_over_fifo_ratios: ratios,
        total_elapsed_ns: started.elapsed().as_nanos(),
    };
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

fn select_resident_case_count(all_cases: &[Case]) -> Result<(usize, usize), BenchError> {
    let mut selected = 0usize;
    let mut entries = 0usize;
    for case in all_cases {
        if case.unique_entries > ARENA_SLOTS {
            if selected == 0 {
                return Err(err("first frozen case exceeds FIFO16 capacity"));
            }
            break;
        }
        let Some(next_entries) = entries.checked_add(case.unique_entries) else {
            return Err(err("selected warm entry count overflow"));
        };
        if next_entries > ARENA_SLOTS {
            break;
        }
        entries = next_entries;
        selected += 1;
    }
    if selected < 2 || entries == 0 {
        return Err(err(
            "frozen corpus has too few cases for a resident warm slice",
        ));
    }
    Ok((selected, entries))
}

struct WarmSweep {
    execution: Duration,
    calls: u64,
    retired: u64,
    native_blocks: u64,
    counters: Counters,
}

fn warm_fifo_sweep(
    engine: &mut Variant,
    cases: &[Case],
    started: Instant,
) -> Result<WarmSweep, BenchError> {
    let mut execution = Duration::ZERO;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    for case in cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during warm FIFO sweep"));
        }
        let mut state = case.initial.clone();
        let call_started = Instant::now();
        let result = engine.call(case, &mut state)?;
        execution = execution.saturating_add(call_started.elapsed());
        validate_dispatch_result(case, &state, &result)?;
        retired = retired
            .checked_add(result.retired_instructions)
            .ok_or_else(|| err("warm FIFO retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(result.blocks_executed)
            .ok_or_else(|| err("warm FIFO native block count overflow"))?;
    }
    Ok(WarmSweep {
        execution,
        calls: cases.len() as u64,
        retired,
        native_blocks,
        counters: engine.counters(),
    })
}

fn measure_warm_fifo_sample(
    pair: usize,
    order: usize,
    repetitions: usize,
    cases: &[Case],
    engine: &mut Variant,
    started: Instant,
) -> Result<FirstRunSample, BenchError> {
    let before = engine.counters();
    let mut execution = Duration::ZERO;
    let mut calls = 0u64;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    for _ in 0..repetitions {
        let sweep = warm_fifo_sweep(engine, cases, started)?;
        execution = execution.saturating_add(sweep.execution);
        calls = calls
            .checked_add(sweep.calls)
            .ok_or_else(|| err("warm FIFO call count overflow"))?;
        retired = retired
            .checked_add(sweep.retired)
            .ok_or_else(|| err("warm FIFO retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(sweep.native_blocks)
            .ok_or_else(|| err("warm FIFO native block count overflow"))?;
    }
    let after = engine.counters();
    let hits = after.hits.saturating_sub(before.hits);
    let misses = after.misses.saturating_sub(before.misses);
    let compiles = after.compiles.saturating_sub(before.compiles);
    let publications = after.publications.saturating_sub(before.publications);
    let evictions = after.evictions.saturating_sub(before.evictions);
    if hits == 0 || misses != 0 || compiles != 0 || publications != 0 || evictions != 0 {
        return Err(err(
            "warm FIFO sample was not a resident cache-hit-only pass",
        ));
    }
    Ok(FirstRunSample {
        pair,
        order,
        sweeps: repetitions,
        calls,
        retired_guest_instructions: retired,
        native_block_executions: native_blocks,
        initialization_ns: 0,
        execution_ns: execution.as_nanos(),
        elapsed_ns: execution.as_nanos(),
        cache_hits: hits,
        cache_misses: misses,
        successful_compiles: compiles,
        successful_publications: publications,
        evictions,
        resident_entries_high_water: after.resident,
        resident_code_bytes_high_water: after.code_bytes,
        resident_logical_metadata_bytes_high_water: after.logical_bytes,
        arena_reserved_bytes: after.arena_reserved_bytes,
    })
}

fn calibrate_common_warm_sweeps(
    cases: &[Case],
    started: Instant,
    mut repetitions: usize,
    engine: &mut Variant,
) -> Result<(usize, usize), BenchError> {
    let mut attempts = 0usize;
    loop {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err(
                "30 minute run limit exceeded during warm paired calibration",
            ));
        }
        let (fifo, interpreter) = if attempts % 2 == 0 {
            (
                measure_warm_fifo_sample(0, 0, repetitions, cases, engine, started)?,
                measure_interpreter_sample(0, 1, repetitions, cases, started)?,
            )
        } else {
            let interpreter = measure_interpreter_sample(0, 1, repetitions, cases, started)?;
            let fifo = measure_warm_fifo_sample(0, 0, repetitions, cases, engine, started)?;
            (fifo, interpreter)
        };
        attempts = attempts.saturating_add(1);
        let next = next_paired_sample_sweeps(repetitions, fifo.elapsed_ns, interpreter.elapsed_ns)?;
        match next {
            Some(next) => repetitions = next,
            None => return Ok((repetitions, attempts)),
        }
    }
}

struct FirstRunSeriesSamples {
    fifo_samples: Vec<FirstRunSample>,
    interpreter_samples: Vec<FirstRunSample>,
    restarts: Vec<FirstRunSeriesRestart>,
    common_sweeps: usize,
}

fn collect_first_run_series<F>(
    mut repetitions: usize,
    pair_count: usize,
    mut measure_pair: F,
) -> Result<FirstRunSeriesSamples, BenchError>
where
    F: FnMut(usize, usize) -> Result<(FirstRunSample, FirstRunSample), BenchError>,
{
    let mut restarts = Vec::new();
    loop {
        let mut fifo_samples = Vec::with_capacity(pair_count);
        let mut interpreter_samples = Vec::with_capacity(pair_count);
        let mut retry_sweeps = None;
        for pair in 0..pair_count {
            let (fifo, interpreter) = measure_pair(pair, repetitions)?;
            if let Some(next_sweeps) =
                next_paired_sample_sweeps(repetitions, fifo.elapsed_ns, interpreter.elapsed_ns)?
            {
                restarts.push(FirstRunSeriesRestart {
                    failed_pair: pair,
                    attempted_sweeps: repetitions,
                    fifo_elapsed_ns: fifo.elapsed_ns,
                    interpreter_elapsed_ns: interpreter.elapsed_ns,
                    next_sweeps,
                });
                retry_sweeps = Some(next_sweeps);
                break;
            }
            fifo_samples.push(fifo);
            interpreter_samples.push(interpreter);
        }
        if let Some(next_sweeps) = retry_sweeps {
            repetitions = next_sweeps;
            continue;
        }
        return Ok(FirstRunSeriesSamples {
            fifo_samples,
            interpreter_samples,
            restarts,
            common_sweeps: repetitions,
        });
    }
}

fn next_paired_sample_sweeps(
    current_sweeps: usize,
    fifo_elapsed_ns: u128,
    interpreter_elapsed_ns: u128,
) -> Result<Option<usize>, BenchError> {
    let minimum_ns = MIN_SAMPLE.as_nanos();
    if fifo_elapsed_ns >= minimum_ns && interpreter_elapsed_ns >= minimum_ns {
        return Ok(None);
    }
    let current = current_sweeps.max(1) as u128;
    let target_work_ns = current
        .checked_mul(minimum_ns)
        .ok_or_else(|| err("paired sample calibration multiplication overflow"))?;
    let elapsed_ns = fifo_elapsed_ns.min(interpreter_elapsed_ns).max(1);
    let quotient = target_work_ns / elapsed_ns;
    let estimated = quotient
        .checked_add(u128::from(target_work_ns % elapsed_ns != 0))
        .ok_or_else(|| err("paired sample calibration division overflow"))?;
    let next = estimated.max(
        current
            .checked_add(1)
            .ok_or_else(|| err("paired sample sweep count overflow"))?,
    );
    usize::try_from(next)
        .map(Some)
        .map_err(|_| err("paired sample sweep count exceeds platform limit"))
}

fn calibrate_common_first_run_sweeps(
    cases: &[Case],
    started: Instant,
    mut repetitions: usize,
) -> Result<(usize, usize), BenchError> {
    let mut attempts = 0usize;
    loop {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err(
                "30 minute run limit exceeded during paired calibration",
            ));
        }
        let (fifo, interpreter) = if attempts % 2 == 0 {
            (
                measure_cold_fifo_sample(0, 0, repetitions, cases, started)?,
                measure_interpreter_sample(0, 1, repetitions, cases, started)?,
            )
        } else {
            let interpreter = measure_interpreter_sample(0, 1, repetitions, cases, started)?;
            let fifo = measure_cold_fifo_sample(0, 0, repetitions, cases, started)?;
            (fifo, interpreter)
        };
        attempts = attempts.saturating_add(1);
        let next = next_paired_sample_sweeps(repetitions, fifo.elapsed_ns, interpreter.elapsed_ns)?;
        match next {
            Some(next) => repetitions = next,
            None => return Ok((repetitions, attempts)),
        }
    }
}

struct ColdSweep {
    initialization: Duration,
    execution: Duration,
    calls: u64,
    retired: u64,
    native_blocks: u64,
    counters: Counters,
}

fn cold_fifo_sweep(cases: &[Case], started: Instant) -> Result<ColdSweep, BenchError> {
    let init_started = Instant::now();
    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    let initialization = init_started.elapsed();
    let mut execution = Duration::ZERO;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    for case in cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during cold FIFO sweep"));
        }
        let mut state = case.initial.clone();
        let call_started = Instant::now();
        let result = engine.call(case, &mut state)?;
        execution = execution.saturating_add(call_started.elapsed());
        validate_dispatch_result(case, &state, &result)?;
        retired = retired
            .checked_add(result.retired_instructions)
            .ok_or_else(|| err("retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(result.blocks_executed)
            .ok_or_else(|| err("native block count overflow"))?;
    }
    let counters = engine.counters();
    drop(engine); // Runtime teardown is explicitly outside the measured intervals.
    Ok(ColdSweep {
        initialization,
        execution,
        calls: cases.len() as u64,
        retired,
        native_blocks,
        counters,
    })
}

fn interpreter_sweep(cases: &[Case], started: Instant) -> Result<ColdSweep, BenchError> {
    let mut execution = Duration::ZERO;
    let mut retired = 0u64;
    for case in cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during interpreter sweep"));
        }
        let mut state = case.initial.clone();
        let mut memory = FetchMemory(&case.bytes);
        let call_started = Instant::now();
        let result = interpreter_run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: case.budget,
                stop_rip: Some(case.bytes.len() as u64),
                context: AccessContext::default(),
            },
            || false,
        );
        execution = execution.saturating_add(call_started.elapsed());
        validate_interpreter_result(case, &state, &result)?;
        retired = retired
            .checked_add(result.instructions_executed)
            .ok_or_else(|| err("interpreter retirement count overflow"))?;
    }
    Ok(ColdSweep {
        initialization: Duration::ZERO,
        execution,
        calls: cases.len() as u64,
        retired,
        native_blocks: 0,
        counters: Counters::default(),
    })
}

fn measure_cold_fifo_sample(
    pair: usize,
    order: usize,
    repetitions: usize,
    cases: &[Case],
    started: Instant,
) -> Result<FirstRunSample, BenchError> {
    let mut initialization = Duration::ZERO;
    let mut execution = Duration::ZERO;
    let mut calls = 0u64;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    let mut counters = Counters {
        arena_free: usize::MAX,
        ..Counters::default()
    };
    for _ in 0..repetitions {
        let sweep = cold_fifo_sweep(cases, started)?;
        initialization = initialization.saturating_add(sweep.initialization);
        execution = execution.saturating_add(sweep.execution);
        calls = calls
            .checked_add(sweep.calls)
            .ok_or_else(|| err("cold FIFO call count overflow"))?;
        retired = retired
            .checked_add(sweep.retired)
            .ok_or_else(|| err("cold FIFO retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(sweep.native_blocks)
            .ok_or_else(|| err("cold FIFO native block count overflow"))?;
        add_sample_counters(&mut counters, sweep.counters)?;
    }
    let elapsed = initialization.saturating_add(execution);
    Ok(FirstRunSample {
        pair,
        order,
        sweeps: repetitions,
        calls,
        retired_guest_instructions: retired,
        native_block_executions: native_blocks,
        initialization_ns: initialization.as_nanos(),
        execution_ns: execution.as_nanos(),
        elapsed_ns: elapsed.as_nanos(),
        cache_hits: counters.hits,
        cache_misses: counters.misses,
        successful_compiles: counters.compiles,
        successful_publications: counters.publications,
        evictions: counters.evictions,
        resident_entries_high_water: counters.resident,
        resident_code_bytes_high_water: counters.code_bytes,
        resident_logical_metadata_bytes_high_water: counters.logical_bytes,
        arena_reserved_bytes: counters.arena_reserved_bytes,
    })
}

fn measure_interpreter_sample(
    pair: usize,
    order: usize,
    repetitions: usize,
    cases: &[Case],
    started: Instant,
) -> Result<FirstRunSample, BenchError> {
    let mut execution = Duration::ZERO;
    let mut calls = 0u64;
    let mut retired = 0u64;
    for _ in 0..repetitions {
        let sweep = interpreter_sweep(cases, started)?;
        execution = execution.saturating_add(sweep.execution);
        calls = calls
            .checked_add(sweep.calls)
            .ok_or_else(|| err("interpreter call count overflow"))?;
        retired = retired
            .checked_add(sweep.retired)
            .ok_or_else(|| err("interpreter retirement count overflow"))?;
    }
    Ok(FirstRunSample {
        pair,
        order,
        sweeps: repetitions,
        calls,
        retired_guest_instructions: retired,
        native_block_executions: 0,
        initialization_ns: 0,
        execution_ns: execution.as_nanos(),
        elapsed_ns: execution.as_nanos(),
        cache_hits: 0,
        cache_misses: 0,
        successful_compiles: 0,
        successful_publications: 0,
        evictions: 0,
        resident_entries_high_water: 0,
        resident_code_bytes_high_water: 0,
        resident_logical_metadata_bytes_high_water: 0,
        arena_reserved_bytes: 0,
    })
}

fn add_sample_counters(total: &mut Counters, current: Counters) -> Result<(), BenchError> {
    total.hits = total
        .hits
        .checked_add(current.hits)
        .ok_or_else(|| err("cache hit counter overflow"))?;
    total.misses = total
        .misses
        .checked_add(current.misses)
        .ok_or_else(|| err("cache miss counter overflow"))?;
    total.compiles = total
        .compiles
        .checked_add(current.compiles)
        .ok_or_else(|| err("compile counter overflow"))?;
    total.publications = total
        .publications
        .checked_add(current.publications)
        .ok_or_else(|| err("publication counter overflow"))?;
    total.evictions = total
        .evictions
        .checked_add(current.evictions)
        .ok_or_else(|| err("eviction counter overflow"))?;
    total.resident = total.resident.max(current.resident);
    total.code_bytes = total.code_bytes.max(current.code_bytes);
    total.logical_bytes = total.logical_bytes.max(current.logical_bytes);
    total.arena_capacity = total.arena_capacity.max(current.arena_capacity);
    total.arena_free = total.arena_free.min(current.arena_free);
    total.arena_reserved_bytes = total.arena_reserved_bytes.max(current.arena_reserved_bytes);
    total.cache_lookup_metrics_available |= current.cache_lookup_metrics_available;
    Ok(())
}

fn summarize_first_run(
    variant: FirstRunVariantName,
    mut samples: Vec<FirstRunSample>,
) -> FirstRunVariantResult {
    let mut per_sweep = samples
        .iter()
        .map(|sample| sample.elapsed_ns / sample.sweeps as u128)
        .collect::<Vec<_>>();
    per_sweep.sort_unstable();
    let median = if per_sweep.len() % 2 == 0 {
        (per_sweep[per_sweep.len() / 2 - 1] + per_sweep[per_sweep.len() / 2]) / 2
    } else {
        per_sweep[per_sweep.len() / 2]
    };
    let iqr = per_sweep[per_sweep.len() * 3 / 4].saturating_sub(per_sweep[per_sweep.len() / 4]);
    samples.sort_by_key(|sample| sample.pair);
    FirstRunVariantResult {
        variant,
        samples,
        median_ns_per_sweep: median,
        min_ns_per_sweep: per_sweep[0],
        max_ns_per_sweep: *per_sweep.last().unwrap(),
        iqr_ns_per_sweep: iqr,
    }
}

fn validate_dispatch_result(
    case: &Case,
    state: &LongModeState,
    result: &od_jit::dispatch::DispatchResult,
) -> Result<(), BenchError> {
    if state.gpr != case.expected.gpr
        || state.rip != case.expected.rip
        || state.rflags & case.flags_mask != case.expected.rflags & case.flags_mask
        || result.exit != case.exit
        || result.retired_instructions != case.retired
        || result.remaining_budget != case.remaining
        || result.blocks_executed as usize != case.entry_count
    {
        return Err(err(format!("native result mismatch for {}", case.id)));
    }
    Ok(())
}

fn validate_interpreter_result(
    case: &Case,
    state: &LongModeState,
    result: &od_isa::execute::RunResult,
) -> Result<(), BenchError> {
    let expected_exit = matches!(
        (&case.exit, &result.exit),
        (DispatchExit::CodeEnd, RunExit::StopAddress)
            | (
                DispatchExit::BudgetExhausted,
                RunExit::InstructionBudgetExhausted
            )
    );
    if !expected_exit
        || state.gpr != case.expected.gpr
        || state.rip != case.expected.rip
        || state.rflags & case.flags_mask != case.expected.rflags & case.flags_mask
        || result.instructions_executed != case.retired
        || case.budget.saturating_sub(result.instructions_executed) != case.remaining
    {
        return Err(err(format!("interpreter result mismatch for {}", case.id)));
    }
    Ok(())
}

fn prepare_cases(
    list: &[u8],
    trace: &RecordedTrace,
    started: Instant,
) -> Result<Vec<Case>, BenchError> {
    if trace.schema != 2 || trace.inputs.len() != MAX_INPUTS {
        return Err(err(format!(
            "expected frozen schema 2 corpus with {MAX_INPUTS} inputs"
        )));
    }
    let text = std::str::from_utf8(list).map_err(|e| err(e.to_string()))?;
    let paths: Vec<PathBuf> = text
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(PathBuf::from)
        .collect();
    if paths.len() != trace.inputs.len() {
        return Err(err("case list and trace length differ"));
    }
    let mut entries = vec![Vec::new(); paths.len()];
    for v in &trace.visits {
        entries
            .get_mut(v.input_index)
            .ok_or_else(|| err("trace visit index out of range"))?
            .push(v.entry_rip);
    }
    if entries
        .iter()
        .any(|visits| visits.len() > MAX_VISITS_PER_INPUT)
    {
        return Err(err("per-input visit limit exceeded"));
    }
    let mut result = Vec::with_capacity(paths.len());
    let mut fixture_bytes = 0usize;
    for (i, path) in paths.iter().enumerate() {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("preparation time limit exceeded"));
        }
        let raw = read_bounded(path, MAX_INPUT_JSON_BYTES, "fixture JSON")?;
        fixture_bytes = fixture_bytes
            .checked_add(raw.len())
            .ok_or_else(|| err("fixture size overflow"))?;
        if fixture_bytes > crate::MAX_LOGICAL_BYTES {
            return Err(err("fixtures exceed 128 MiB limit"));
        }
        if raw.len() > MAX_INPUT_JSON_BYTES {
            return Err(err("fixture exceeds size bound"));
        }
        let loaded = Fixture::from_json(&raw)
            .map_err(|e| err(format!("fixture {}: {e:?}", path.display())))?;
        let f = loaded.fixture;
        let bytes = f
            .program
            .bytes()
            .map_err(|e| err(format!("program: {e:?}")))?;
        if bytes.len() > crate::MAX_CODE_BYTES_PER_INPUT {
            return Err(err("fixture code exceeds per-input bound"));
        }
        let t = &trace.inputs[i];
        let hex = bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let iid = i as u64 + 1;
        if f.case_id != t.case_id
            || loaded.input_sha256 != t.fixture_sha256
            || hex != t.code_hex
            || od_testkit::sha256_hex(&bytes) != t.program_sha256
            || t.base_rip != 0
            || t.address_space_id != 1
            || t.mapping_id != iid
            || t.mapping_version != 1
            || t.backing_id != iid
            || t.backing_version != 1
        {
            return Err(err(format!("frozen trace mismatch at {i}")));
        }
        let view = CodeView::new(0, &bytes).map_err(|e| err(format!("view: {e:?}")))?;
        let decoded = od_isa::decode(&bytes).map_err(|e| err(format!("decode: {e:?}")))?;
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(view, &decoded)
            .map_err(|e| err(format!("admission: {e:?}")))?;
        let mut initial = LongModeState::default();
        initial.gpr[..4].copy_from_slice(&[
            f.initial_state.rax.get(),
            f.initial_state.rcx.get(),
            f.initial_state.rdx.get(),
            f.initial_state.rbx.get(),
        ]);
        initial.rip = f.initial_state.rip.get();
        initial.rflags = f.initial_state.rflags.get();
        let mut boundaries = std::collections::HashSet::new();
        let mut boundary = view.base_rip();
        boundaries.insert(boundary);
        for instruction in &decoded {
            let length = u64::try_from(instruction.length)
                .map_err(|_| err("instruction length overflow"))?;
            boundary = boundary
                .checked_add(length)
                .ok_or_else(|| err("decoded boundary overflow"))?;
            boundaries.insert(boundary);
        }
        if boundary != view.code_end()
            || !boundaries.contains(&initial.rip)
            || entries[i].iter().any(|entry| !boundaries.contains(entry))
        {
            return Err(err(format!("entry boundary mismatch for {}", f.case_id)));
        }
        let mut expected = initial.clone();
        let mut mem = FetchMemory(&bytes);
        let interp = interpreter_run(
            &mut expected,
            &mut mem,
            RunConfig {
                instruction_budget: f.instruction_budget,
                stop_rip: Some(view.code_end()),
                context: AccessContext::default(),
            },
            || false,
        );
        if interp.instructions_executed > f.instruction_budget {
            return Err(err("interpreter retired beyond budget"));
        }
        let exit = match (f.expected_exit, interp.exit) {
            (ExpectedExit::ReachedCodeEnd, RunExit::StopAddress) => DispatchExit::CodeEnd,
            (ExpectedExit::InstructionBudgetExhausted, RunExit::InstructionBudgetExhausted) => {
                DispatchExit::BudgetExhausted
            }
            _ => return Err(err(format!("fixture oracle exit mismatch: {}", f.case_id))),
        };
        let ident = CodeIdentity {
            address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
            mapping: MappingId::new(NonZeroU64::new(iid).unwrap()),
            mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
            backing: BackingId::new(NonZeroU64::new(iid).unwrap()),
            backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
            executable: true,
        };
        result.push(Case {
            id: f.case_id,
            bytes,
            admission,
            initial,
            expected,
            budget: f.instruction_budget,
            exit,
            retired: interp.instructions_executed,
            remaining: f.instruction_budget - interp.instructions_executed,
            flags_mask: f.comparison.rflags_mask.get(),
            identity: ident,
            entry_count: entries[i].len(),
            unique_entries: entries[i]
                .iter()
                .copied()
                .collect::<std::collections::HashSet<_>>()
                .len(),
        });
    }
    Ok(result)
}

fn read_bounded(path: &Path, max_bytes: usize, label: &str) -> Result<Vec<u8>, BenchError> {
    use std::io::Read;
    let metadata = std::fs::metadata(path).map_err(|e| err(format!("{label}: {e}")))?;
    if metadata.len() > max_bytes as u64 {
        return Err(err(format!("{label} exceeds {max_bytes} byte limit")));
    }
    let limit = u64::try_from(max_bytes)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| err("bounded read limit overflow"))?;
    let file = std::fs::File::open(path).map_err(|e| err(format!("{label}: {e}")))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit)
        .read_to_end(&mut bytes)
        .map_err(|e| err(format!("{label}: {e}")))?;
    if bytes.len() > max_bytes {
        return Err(err(format!("{label} exceeds {max_bytes} byte limit")));
    }
    Ok(bytes)
}

fn plans(cases: &[Case]) -> Result<Vec<Scenario>, BenchError> {
    let singles: Vec<usize> = cases
        .iter()
        .enumerate()
        .filter_map(|(i, c)| (c.entry_count == 1).then_some(i))
        .collect();
    if singles.len() < 17 {
        return Err(err("need 17 one-block cases"));
    }
    let mut ps = vec![
        make_plan(
            "corpus-cold-sweep",
            (0..cases.len()).collect(),
            0,
            1,
            true,
            cases,
        ),
        make_plan(
            "corpus-repeat-2",
            (0..cases.len()).collect(),
            1,
            2,
            false,
            cases,
        ),
        make_plan(
            "corpus-repeat-4",
            (0..cases.len()).collect(),
            1,
            4,
            false,
            cases,
        ),
    ];
    for n in [1, 8, 16] {
        ps.push(make_plan(
            &format!("hot-single-block-{n}"),
            singles[..n].to_vec(),
            1,
            1,
            false,
            cases,
        ));
    }
    let multi = cases
        .iter()
        .position(|c| (2..=16).contains(&c.unique_entries))
        .ok_or_else(|| err("no recorded multi-block input with <=16 visits"))?;
    ps.push(make_plan(
        "recorded-multiblock",
        vec![multi],
        1,
        1,
        false,
        cases,
    ));
    ps.push(make_plan(
        "capacity-edge-17",
        singles[..17].to_vec(),
        1,
        1,
        false,
        cases,
    ));
    for n in [8, 16] {
        let mut seq = Vec::new();
        for _ in 0..8 {
            seq.extend_from_slice(&singles[..n]);
            seq.push(singles[0]);
            seq.push(singles[16]);
            seq.extend_from_slice(&singles[..n]);
        }
        ps.push(make_plan(
            &format!("hot-{n}-plus-cold-insertion"),
            seq,
            1,
            1,
            false,
            cases,
        ));
    }
    Ok(ps)
}
fn make_plan(
    name: &str,
    input_indices: Vec<usize>,
    warmup_sweeps: usize,
    minimum_measured_sweeps: usize,
    cold_each_sweep: bool,
    cases: &[Case],
) -> Scenario {
    let selected_case_ids = input_indices
        .iter()
        .filter_map(|i| cases.get(*i).map(|c| c.id.clone()))
        .collect();
    Scenario {
        name: name.into(),
        input_indices,
        warmup_sweeps,
        minimum_measured_sweeps,
        cold_each_sweep,
        selected_case_ids,
    }
}

fn calibrate_sweeps(
    name: VariantName,
    scenario: &Scenario,
    cases: &[Case],
    started: Instant,
) -> Result<usize, BenchError> {
    let mut engine = Variant::new(name)?;
    for _ in 0..scenario.warmup_sweeps {
        let _ = sweep(&mut engine, cases, &scenario.input_indices, started)?;
    }
    let mut elapsed = Duration::ZERO;
    let mut sweeps = 0usize;
    while elapsed < Duration::from_millis(300) || sweeps < scenario.minimum_measured_sweeps {
        let (time, _, _, _, _) = sweep(&mut engine, cases, &scenario.input_indices, started)?;
        elapsed = elapsed.saturating_add(time);
        sweeps += 1;
        if scenario.cold_each_sweep
            && (elapsed < Duration::from_millis(300) || sweeps < scenario.minimum_measured_sweeps)
        {
            replace_variant(&mut engine, name)?;
        }
    }
    Ok(sweeps)
}

fn run_fixed_sweeps(
    engine: &mut Variant,
    name: VariantName,
    scenario: &Scenario,
    cases: &[Case],
    started: Instant,
    repetitions: usize,
) -> Result<(Duration, u64, u64, u64, Counters), BenchError> {
    let mut elapsed = Duration::ZERO;
    let mut calls = 0u64;
    let mut native_blocks = 0u64;
    let mut retired = 0u64;
    let mut high = engine.counters();
    let mut counter_totals = Counters::default();
    let mut previous = engine.counters();
    for sweep_index in 0..repetitions {
        if scenario.cold_each_sweep && sweep_index != 0 {
            replace_variant(engine, name)?;
            previous = engine.counters();
        }
        let (time, count, blocks, retired_count, sweep_high) =
            sweep(engine, cases, &scenario.input_indices, started)?;
        elapsed = elapsed.saturating_add(time);
        calls = calls
            .checked_add(count)
            .ok_or_else(|| err("call count overflow"))?;
        native_blocks = native_blocks
            .checked_add(blocks)
            .ok_or_else(|| err("block count overflow"))?;
        retired = retired
            .checked_add(retired_count)
            .ok_or_else(|| err("retired count overflow"))?;
        let counters = engine.counters();
        counter_totals.hits = counter_totals
            .hits
            .checked_add(counters.hits - previous.hits)
            .ok_or_else(|| err("hit count overflow"))?;
        counter_totals.misses = counter_totals
            .misses
            .checked_add(counters.misses - previous.misses)
            .ok_or_else(|| err("miss count overflow"))?;
        counter_totals.compiles = counter_totals
            .compiles
            .checked_add(counters.compiles - previous.compiles)
            .ok_or_else(|| err("compile count overflow"))?;
        counter_totals.publications = counter_totals
            .publications
            .checked_add(counters.publications - previous.publications)
            .ok_or_else(|| err("publication count overflow"))?;
        counter_totals.evictions = counter_totals
            .evictions
            .checked_add(counters.evictions - previous.evictions)
            .ok_or_else(|| err("eviction count overflow"))?;
        if matches!(engine, Variant::Uncached(_)) {
            counter_totals.compiles = counter_totals
                .compiles
                .checked_add(blocks)
                .ok_or_else(|| err("compile count overflow"))?;
            counter_totals.publications = counter_totals
                .publications
                .checked_add(blocks)
                .ok_or_else(|| err("publication count overflow"))?;
        }
        previous = counters;
        high.resident = high.resident.max(sweep_high.resident);
        high.code_bytes = high.code_bytes.max(sweep_high.code_bytes);
        high.logical_bytes = high.logical_bytes.max(sweep_high.logical_bytes);
        high.arena_capacity = high.arena_capacity.max(sweep_high.arena_capacity);
        high.arena_free = high.arena_free.min(sweep_high.arena_free);
        high.arena_reserved_bytes = high
            .arena_reserved_bytes
            .max(sweep_high.arena_reserved_bytes);
    }
    high.hits = counter_totals.hits;
    high.misses = counter_totals.misses;
    high.compiles = counter_totals.compiles;
    high.publications = counter_totals.publications;
    high.evictions = counter_totals.evictions;
    Ok((elapsed, calls, native_blocks, retired, high))
}

fn replace_variant(engine: &mut Variant, name: VariantName) -> Result<(), BenchError> {
    let old = std::mem::replace(engine, Variant::Uncached(PublishedBlockExecutor));
    drop(old);
    *engine = Variant::new(name)?;
    Ok(())
}

fn sweep(
    engine: &mut Variant,
    cases: &[Case],
    indices: &[usize],
    started: Instant,
) -> Result<(Duration, u64, u64, u64, Counters), BenchError> {
    let mut elapsed = Duration::ZERO;
    let mut blocks = 0u64;
    let mut retired = 0u64;
    let mut high = engine.counters();
    for &i in indices {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded"));
        }
        let c = cases.get(i).ok_or_else(|| err("scenario index invalid"))?;
        let mut state = c.initial.clone();
        let tick = Instant::now();
        let r = engine.call(c, &mut state)?;
        elapsed = elapsed.saturating_add(tick.elapsed());
        blocks = blocks
            .checked_add(r.blocks_executed)
            .ok_or_else(|| err("block count overflow"))?;
        retired = retired
            .checked_add(r.retired_instructions)
            .ok_or_else(|| err("retired count overflow"))?;
        let counters = engine.counters();
        high.resident = high.resident.max(counters.resident);
        high.code_bytes = high.code_bytes.max(counters.code_bytes);
        high.logical_bytes = high.logical_bytes.max(counters.logical_bytes);
        high.arena_capacity = high.arena_capacity.max(counters.arena_capacity);
        high.arena_free = high.arena_free.min(counters.arena_free);
        high.arena_reserved_bytes = high.arena_reserved_bytes.max(counters.arena_reserved_bytes);
        if state.gpr != c.expected.gpr
            || state.rip != c.expected.rip
            || state.rflags & c.flags_mask != c.expected.rflags & c.flags_mask
            || r.exit != c.exit
            || r.retired_instructions != c.retired
            || r.remaining_budget != c.remaining
            || r.blocks_executed as usize != c.entry_count
        {
            return Err(err(format!("native result mismatch for {}", c.id)));
        }
    }
    Ok((elapsed, indices.len() as u64, blocks, retired, high))
}

fn summarize(variant: VariantName, mut samples: Vec<Sample>) -> VariantResult {
    let mut v = samples.iter().map(|s| s.elapsed_ns).collect::<Vec<_>>();
    v.sort_unstable();
    let median = if v.len() % 2 == 0 {
        (v[v.len() / 2 - 1] as u128 + v[v.len() / 2] as u128) / 2
    } else {
        v[v.len() / 2]
    };
    let iqr = v[v.len() * 3 / 4].saturating_sub(v[v.len() / 4]);
    samples.sort_by_key(|s| s.pair);
    VariantResult {
        variant,
        samples,
        median_ns: median,
        min_ns: v[0],
        max_ns: *v.last().unwrap(),
        iqr_ns: iqr,
    }
}

struct FetchMemory<'a>(&'a [u8]);
impl GuestMemory for FetchMemory<'_> {
    fn fetch(&mut self, a: u64, _: AccessContext) -> Result<u8, MemoryError> {
        usize::try_from(a)
            .ok()
            .and_then(|i| self.0.get(i).copied())
            .ok_or(MemoryError::Unmapped {
                address: a,
                access: od_isa::memory::AccessKind::Fetch,
            })
    }
    fn read(&mut self, a: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!("read disabled at {a:#x}")))
    }
    fn write(&mut self, a: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Backend(format!("write disabled at {a:#x}")))
    }
    fn atomic_exchange_integer(
        &mut self,
        a: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!("atomic disabled at {a:#x}")))
    }
    fn atomic_fetch_add_integer(
        &mut self,
        a: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "atomic add disabled at {a:#x}"
        )))
    }
    fn atomic_compare_exchange_integer(
        &mut self,
        a: u64,
        _: AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(MemoryError::Backend(format!(
            "atomic compare-exchange disabled at {a:#x}"
        )))
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Err(MemoryError::Backend("mfence disabled".into()))
    }
}

pub fn write_atomic(path: &Path, value: &serde_json::Value) -> Result<(), BenchError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|e| err(e.to_string()))?;
    let temp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|e| err(e.to_string()))?;
    bytes.push(b'\n');
    use std::io::Write;
    let result = (|| {
        let mut file = std::fs::File::create(&temp).map_err(|e| err(e.to_string()))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| err(e.to_string()))?;
        std::fs::rename(&temp, path).map_err(|e| err(e.to_string()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static NATIVE_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_case(id: &str, entry_count: usize, unique_entries: usize) -> Case {
        let bytes = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let code = CodeView::new(0, &bytes).unwrap();
        let decoded = od_isa::decode(&bytes).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(code, &decoded).unwrap();
        Case {
            id: id.into(),
            bytes: bytes.to_vec(),
            admission,
            initial: LongModeState::default(),
            expected: LongModeState::default(),
            budget: 1,
            exit: DispatchExit::BudgetExhausted,
            retired: 1,
            remaining: 0,
            flags_mask: u64::MAX,
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
                mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
                mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
                backing: BackingId::new(NonZeroU64::new(1).unwrap()),
                backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
                executable: true,
            },
            entry_count,
            unique_entries,
        }
    }

    #[test]
    fn plan_records_stable_hot_and_multi_block_selections() {
        let mut cases: Vec<_> = (0..17)
            .map(|i| test_case(&format!("hot-{i}"), 1, 1))
            .collect();
        cases.push(test_case("multi", 4, 3));
        let scenarios = plans(&cases).unwrap();
        assert_eq!(scenarios.len(), 10);
        assert_eq!(
            scenarios[0].input_indices,
            (0..cases.len()).collect::<Vec<_>>()
        );
        assert_eq!(scenarios[3].input_indices, vec![0]);
        assert_eq!(scenarios[4].input_indices, (0..8).collect::<Vec<_>>());
        assert_eq!(scenarios[5].input_indices, (0..16).collect::<Vec<_>>());
        assert_eq!(scenarios[6].input_indices, vec![17]);
        assert_eq!(scenarios[7].input_indices, (0..17).collect::<Vec<_>>());
        assert_eq!(scenarios[8].input_indices[9], 16);
        assert_eq!(scenarios[9].input_indices[17], 16);
    }

    #[test]
    fn atomic_output_writes_complete_json() {
        let path =
            std::env::temp_dir().join(format!("od-bench-{}-atomic.json", std::process::id()));
        let value = serde_json::json!({"schema": 1, "ok": true});
        write_atomic(&path, &value).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written, value);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn cold_variant_reset_releases_arena_before_reallocation() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let mut engine = Variant::new(VariantName::MultiFifo16).unwrap();
        replace_variant(&mut engine, VariantName::MultiLru16).unwrap();
        assert!(matches!(&engine, Variant::Lru { .. }));
        replace_variant(&mut engine, VariantName::SingleEntryArena16).unwrap();
        assert!(matches!(&engine, Variant::Single { .. }));
    }

    #[test]
    fn first_run_summary_normalizes_each_batch_by_cold_sweeps() {
        let sample = |pair, sweeps, elapsed_ns| FirstRunSample {
            pair,
            order: pair % 2,
            sweeps,
            calls: 0,
            retired_guest_instructions: 0,
            native_block_executions: 0,
            initialization_ns: 0,
            execution_ns: elapsed_ns,
            elapsed_ns,
            cache_hits: 0,
            cache_misses: 0,
            successful_compiles: 0,
            successful_publications: 0,
            evictions: 0,
            resident_entries_high_water: 0,
            resident_code_bytes_high_water: 0,
            resident_logical_metadata_bytes_high_water: 0,
            arena_reserved_bytes: 0,
        };
        let result = summarize_first_run(
            FirstRunVariantName::Interpreter,
            vec![
                sample(0, 1, 100),
                sample(1, 2, 400),
                sample(2, 4, 1_200),
                sample(3, 5, 2_000),
            ],
        );
        assert_eq!(result.min_ns_per_sweep, 100);
        assert_eq!(result.median_ns_per_sweep, 250);
        assert_eq!(result.max_ns_per_sweep, 400);
        assert_eq!(result.iqr_ns_per_sweep, 200);
    }

    #[test]
    fn measured_sample_drift_discards_partial_series_and_restarts_at_common_count() {
        let minimum_ns = MIN_SAMPLE.as_nanos();
        let mut series_attempt = 0usize;
        let measured = collect_first_run_series(1, FIRST_RUN_PAIRS, |pair, sweeps| {
            if pair == 0 {
                series_attempt += 1;
            }
            let (fifo_elapsed_ns, interpreter_elapsed_ns) = if series_attempt == 1 {
                if pair == 0 {
                    (minimum_ns + 100, minimum_ns + 100)
                } else {
                    (minimum_ns + 100, minimum_ns * 19 / 20)
                }
            } else {
                (minimum_ns * sweeps as u128, minimum_ns * 2 * sweeps as u128)
            };
            let sample = |order, elapsed_ns| FirstRunSample {
                pair,
                order,
                sweeps,
                calls: 0,
                retired_guest_instructions: 0,
                native_block_executions: 0,
                initialization_ns: 0,
                execution_ns: elapsed_ns,
                elapsed_ns,
                cache_hits: 0,
                cache_misses: 0,
                successful_compiles: 0,
                successful_publications: 0,
                evictions: 0,
                resident_entries_high_water: 0,
                resident_code_bytes_high_water: 0,
                resident_logical_metadata_bytes_high_water: 0,
                arena_reserved_bytes: 0,
            };
            Ok((
                sample(0, fifo_elapsed_ns),
                sample(1, interpreter_elapsed_ns),
            ))
        })
        .unwrap();

        assert_eq!(series_attempt, 2);
        assert_eq!(measured.common_sweeps, 2);
        assert_eq!(measured.fifo_samples.len(), FIRST_RUN_PAIRS);
        assert_eq!(measured.interpreter_samples.len(), FIRST_RUN_PAIRS);
        assert_eq!(
            measured.restarts,
            vec![FirstRunSeriesRestart {
                failed_pair: 1,
                attempted_sweeps: 1,
                fifo_elapsed_ns: minimum_ns + 100,
                interpreter_elapsed_ns: minimum_ns * 19 / 20,
                next_sweeps: 2,
            }]
        );
        for (pair, (fifo, interpreter)) in measured
            .fifo_samples
            .iter()
            .zip(&measured.interpreter_samples)
            .enumerate()
        {
            assert_eq!(fifo.pair, pair);
            assert_eq!(interpreter.pair, pair);
            assert_eq!(fifo.sweeps, 2);
            assert_eq!(interpreter.sweeps, 2);
            assert!(fifo.elapsed_ns >= minimum_ns);
            assert!(interpreter.elapsed_ns >= minimum_ns);
        }
        // Pair zero from the failed first attempt was discarded.
        assert_eq!(measured.fifo_samples[0].elapsed_ns, minimum_ns * 2);
    }

    #[test]
    fn paired_sample_sweep_count_overflow_fails_closed() {
        assert!(next_paired_sample_sweeps(usize::MAX, 1, MIN_SAMPLE.as_nanos()).is_err());
    }

    fn native_mov_case() -> Case {
        let bytes = [0x48, 0xb8, 0x2a, 0, 0, 0, 0, 0, 0, 0];
        let code = CodeView::new(0, &bytes).unwrap();
        let decoded = od_isa::decode(&bytes).unwrap();
        let admission = AdmittedInstructionBoundaries::from_fixture_decode(code, &decoded).unwrap();
        let initial = LongModeState::default();
        let mut expected = initial.clone();
        let mut mem = FetchMemory(&bytes);
        let oracle = interpreter_run(
            &mut expected,
            &mut mem,
            RunConfig {
                instruction_budget: 2,
                stop_rip: Some(code.code_end()),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(oracle.exit, RunExit::StopAddress);
        assert_eq!(oracle.instructions_executed, 1);
        Case {
            id: "native-mov-smoke".into(),
            bytes: bytes.to_vec(),
            admission,
            initial,
            expected,
            budget: 2,
            exit: DispatchExit::CodeEnd,
            retired: 1,
            remaining: 1,
            flags_mask: u64::MAX,
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
                mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
                mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
                backing: BackingId::new(NonZeroU64::new(1).unwrap()),
                backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
                executable: true,
            },
            entry_count: 1,
            unique_entries: 1,
        }
    }

    #[test]
    fn native_sweep_matches_oracle_and_reports_cold_warm_counters_for_all_variants() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let case = native_mov_case();
        let cases = [case];
        let started = Instant::now();
        for name in [
            VariantName::UncachedPublishedBlockExecutor,
            VariantName::SingleEntryArena16,
            VariantName::MultiFifo16,
            VariantName::MultiLru16,
        ] {
            for cold_each_sweep in [false, true] {
                let scenario = Scenario {
                    name: format!("native-smoke-{name:?}-{cold_each_sweep}"),
                    input_indices: vec![0],
                    warmup_sweeps: 0,
                    minimum_measured_sweeps: 0,
                    cold_each_sweep,
                    selected_case_ids: vec![cases[0].id.clone()],
                };
                let mut engine = Variant::new(name).unwrap();
                let (_, calls, blocks, retired, counters) =
                    run_fixed_sweeps(&mut engine, name, &scenario, &cases, started, 2).unwrap();
                assert_eq!((calls, blocks, retired), (2, 2, 2));
                let expected_publications = if cold_each_sweep
                    || matches!(name, VariantName::UncachedPublishedBlockExecutor)
                {
                    2
                } else {
                    1
                };
                assert_eq!(counters.compiles, expected_publications);
                assert_eq!(counters.publications, expected_publications);
                if matches!(name, VariantName::UncachedPublishedBlockExecutor) {
                    assert!(!counters.cache_lookup_metrics_available);
                    assert_eq!(counters.hits, 0);
                    assert_eq!(counters.misses, 0);
                } else if cold_each_sweep {
                    assert_eq!(counters.hits, 0);
                    assert_eq!(counters.misses, 2);
                    assert_eq!(counters.resident, 1);
                } else {
                    assert_eq!(counters.hits, 1);
                    assert_eq!(counters.misses, 1);
                    assert_eq!(counters.resident, 1);
                }
            }
        }
    }

    #[test]
    fn bounded_reader_rejects_oversized_sparse_input_before_loading() {
        let path = std::env::temp_dir().join(format!("od-bench-{}-oversized", std::process::id()));
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(33).unwrap();
        drop(file);
        assert!(read_bounded(&path, 32, "test input").is_err());
        std::fs::remove_file(path).unwrap();
    }
}
