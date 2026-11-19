//! Native end-to-end dispatch/cache comparison. Fixture loading, full decode
//! admission, and interpreter oracles happen before timing.

use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

#[path = "dispatch_cache_stress.rs"]
mod lifecycle_stress;
#[cfg(target_os = "macos")]
use std::{
    io::Read,
    process::{Command, Stdio},
    thread,
};

use od_host::{PublishedArenaBackend, PublishedArenaBlock, PublishedBlockExecutor};
use od_isa::LongModeState;
use od_isa::execute::{RunConfig, RunExit, run as interpreter_run};
use od_isa::memory::{AccessContext, GuestMemory, MemoryError};
use od_jit::block::{AdmittedInstructionBoundaries, CodeView, NativeRegisterV1Admission};
use od_jit::dispatch::{
    CacheEvictionPolicy, CodeInvalidation, DispatchControl, DispatchExit,
    MAX_MULTI_ENTRY_CACHE_CODE_BYTES, MAX_MULTI_ENTRY_CACHE_METADATA_BYTES,
    MAX_SINGLE_ENTRY_CACHE_BYTES, MultiEntryBlockCache, RegisterFlagsMode, SingleEntryBlockCache,
    run as dispatch_run, run_cached, run_multi_cached, run_multi_cached_controlled,
    run_multi_cached_controlled_observed, run_multi_cached_native_register_v1_controlled,
};
use od_jit::phase_observation::{ObservedDispatchPhase, PhaseRecorder};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity,
    ExecutionContextId, ExecutionContextSnapshot, MappingId, MappingVersion,
};
use od_testkit::case::{ExpectedExit, Fixture};
use serde::Serialize;

use crate::{
    HostMetadata, MAX_INPUT_JSON_BYTES, MAX_INPUTS, MAX_RUN_TIME, MAX_TOTAL_TRACE_VISITS,
    MAX_VISITS_PER_INPUT, RecordedTrace,
};

const PAIRS: usize = 20;
const FIRST_RUN_PAIRS: usize = 20;
const CACHE_CHURN_SWEEPS: usize = 128;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const ARENA_SLOTS: usize = 16;
const MAX_CASE_LIST_BYTES: usize = 4 * 1024 * 1024;
const FROZEN_CASE_LIST_SHA256: &str =
    "b9276b52556b3692552de074e83cd8aef0c777f29861bd19ab09c188e41615d3";
const FROZEN_TRACE_SHA256: &str =
    "bf8d5efe0c52a47cbb4682f67ea73a82e684d463733aff3be60fed0b280bd5b3";
const BRANCH_HELPER_CASE_LIST_SHA256: &str =
    "dd590c6c8f8ae1a09f2192e47c1d019f7449c814212c87e06194bf7425dd0881";
const BRANCH_HELPER_TRACE_SHA256: &str =
    "272eb80c3283e71962f1bc2a2e6d847723164d48f56dd3970134a814df57d2c8";
const BRANCH_HELPER_INPUTS: usize = 5;
const BRANCH_HELPER_VISITS: usize = 5;
const WARM_DISPATCH_PHASE_MAX_RESTARTS: usize = 8;
const WARM_DISPATCH_PHASE_STRATA: [WarmDispatchPhaseStratumSpec; 2] = [
    WarmDispatchPhaseStratumSpec {
        name: "recorded_multiblock",
        input_index: 9,
        case_id: "gen_v1_000000005eedcafe_00000009",
        entry_rips: &[0, 15],
    },
    WarmDispatchPhaseStratumSpec {
        name: "self_loop",
        input_index: 1003,
        case_id: "s0_09_sub_jnz_loop",
        entry_rips: &[0, 0, 0],
    },
];

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
    native_admission: Option<NativeRegisterV1Admission>,
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
            Self::Fifo { backend, cache } | Self::Lru { backend, cache } => {
                if let Some(admission) = &case.native_admission {
                    let stop = AtomicU8::new(0);
                    let result = run_multi_cached_native_register_v1_controlled(
                        code,
                        admission,
                        ExecutionContextSnapshot::native_register_v1_fixture(
                            ExecutionContextId::from_u64(1).unwrap(),
                        ),
                        &dep,
                        state,
                        case.budget,
                        DispatchControl {
                            stop_rip: None,
                            stop_requested: &stop,
                            event_requested: None,
                        },
                        cache,
                        backend,
                        RegisterFlagsMode::Eager,
                    )
                    .map_err(|e| err(format!("generic multi cache: {e:?}")))?;
                    // This frozen, native-only workload must not silently turn
                    // into an interpreter-fallback benchmark.
                    if result.retired_interpreter != 0 {
                        return Err(err(format!("unexpected interpreter fallback: {}", case.id)));
                    }
                    Ok(od_jit::dispatch::DispatchResult {
                        exit: result.exit,
                        remaining_budget: result.remaining_budget,
                        retired_instructions: result.retired_native,
                        blocks_executed: result.blocks_executed,
                    })
                } else {
                    run_multi_cached(
                        code,
                        &case.admission,
                        crate::benchmark_execution_context(),
                        &dep,
                        state,
                        case.budget,
                        cache,
                        backend,
                    )
                    .map_err(|e| err(format!("multi cache: {e:?}")))
                }
            }
        }
    }

    fn call_observed(
        &mut self,
        case: &Case,
        state: &mut LongModeState,
        observer: &mut PhaseRecorder,
    ) -> Result<od_jit::dispatch::DispatchResult, BenchError> {
        let code = CodeView::new(0, &case.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
        let dep = [CodeDependencySpan {
            start_rip: 0,
            end_rip: code.code_end(),
            identity: case.identity,
        }];
        let stop = AtomicU8::new(0);
        let control = DispatchControl {
            stop_rip: None,
            stop_requested: &stop,
            event_requested: None,
        };
        match self {
            Self::Fifo { backend, cache } => run_multi_cached_controlled_observed(
                code,
                &case.admission,
                crate::benchmark_execution_context(),
                &dep,
                state,
                case.budget,
                control,
                cache,
                backend,
                observer,
            )
            .map_err(|e| err(format!("observed multi cache: {e:?}"))),
            _ => Err(err("phase observation requires the FIFO(16) dispatcher")),
        }
    }

    fn call_fifo_with_identity_and_context(
        &mut self,
        case: &Case,
        state: &mut LongModeState,
        identity: CodeIdentity,
        context: ExecutionContextSnapshot,
    ) -> Result<od_jit::dispatch::DispatchResult, BenchError> {
        self.call_fifo_controlled_with_identity_and_context(case, state, identity, context, None)
    }

    fn call_fifo_controlled_with_identity_and_context(
        &mut self,
        case: &Case,
        state: &mut LongModeState,
        identity: CodeIdentity,
        context: ExecutionContextSnapshot,
        stop_rip: Option<u64>,
    ) -> Result<od_jit::dispatch::DispatchResult, BenchError> {
        let code = CodeView::new(0, &case.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
        let dep = [CodeDependencySpan {
            start_rip: 0,
            end_rip: code.code_end(),
            identity,
        }];
        let stop_requested = AtomicU8::new(0);
        match self {
            Self::Fifo { backend, cache } => run_multi_cached_controlled(
                code,
                &case.admission,
                context,
                &dep,
                state,
                case.budget,
                DispatchControl {
                    stop_rip,
                    stop_requested: &stop_requested,
                    event_requested: None,
                },
                cache,
                backend,
            )
            .map_err(|e| err(format!("multi cache: {e:?}"))),
            _ => Err(err(
                "cache lifecycle requires the production FIFO(16) dispatcher",
            )),
        }
    }

    fn fifo_lifecycle_snapshot(&self) -> Result<LifecycleCacheSnapshot, BenchError> {
        match self {
            Self::Fifo { backend, cache } => {
                let counters = cache.counters();
                Ok(LifecycleCacheSnapshot {
                    hits: counters.hits,
                    misses: counters.misses,
                    successful_compiles: counters.successful_compiles,
                    successful_publications: counters.successful_publications,
                    evictions: counters.evictions,
                    resident_entries_current: cache.resident_entries(),
                    resident_code_bytes_current: counters.resident_code_bytes,
                    resident_logical_metadata_bytes_current: counters
                        .resident_logical_metadata_bytes,
                    resident_entries_high_water: counters.resident_entries_high_water,
                    resident_code_bytes_high_water: counters.resident_code_bytes_high_water,
                    resident_logical_metadata_bytes_high_water: counters
                        .resident_logical_metadata_bytes_high_water,
                    arena_capacity_slots: backend.capacity(),
                    arena_available_slots_actual: backend.available_slots(),
                    arena_reserved_mapping_bytes: backend.reserved_mapping_bytes(),
                })
            }
            _ => Err(err(
                "cache lifecycle requires the production FIFO(16) dispatcher",
            )),
        }
    }

    fn fifo_invalidate_backing(
        &mut self,
        backing: BackingId,
        current_version: BackingVersion,
    ) -> Result<usize, BenchError> {
        match self {
            Self::Fifo { cache, .. } => {
                Ok(cache.invalidate(CodeInvalidation::BackingContentChanged {
                    backing,
                    current_version,
                }))
            }
            _ => Err(err(
                "cache lifecycle requires the production FIFO(16) dispatcher",
            )),
        }
    }

    fn fifo_clear(&mut self) -> Result<(), BenchError> {
        match self {
            Self::Fifo { cache, .. } => {
                cache.clear();
                Ok(())
            }
            _ => Err(err(
                "cache lifecycle requires the production FIFO(16) dispatcher",
            )),
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

#[derive(Clone, Debug, Serialize)]
struct PipelinePhaseTotal {
    phase: String,
    calls: u64,
    elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct PipelinePhaseSample {
    pair: usize,
    order: usize,
    cold_sweeps: usize,
    calls: u64,
    retired_guest_instructions: u64,
    native_block_executions: u64,
    initialization_ns: u128,
    dispatch_elapsed_ns: u128,
    attributed_phase_ns: u128,
    other_dispatch_ns: u128,
    cache_hits: u64,
    cache_misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
    phase_totals: Vec<PipelinePhaseTotal>,
}

#[derive(Clone, Debug, Serialize)]
struct PipelinePhaseVariant {
    variant: String,
    samples: Vec<PipelinePhaseSample>,
}

#[derive(Clone, Debug, Serialize)]
struct PipelinePhasePreflight {
    full_state_exit_retirement_budget_parity: bool,
    ordinary_observed_dispatch_results_match: bool,
    ordinary_observed_cache_counters_match: bool,
    ordinary_calls: u64,
    observed_calls: u64,
    ordinary_retired_guest_instructions: u64,
    observed_retired_guest_instructions: u64,
    ordinary_native_block_executions: u64,
    observed_native_block_executions: u64,
    ordinary_cache_hits: u64,
    observed_cache_hits: u64,
    ordinary_cache_misses: u64,
    observed_cache_misses: u64,
    phase_event_counts_match_cache_counters: bool,
}

#[derive(Serialize)]
struct ColdPipelineReport {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    visits: usize,
    measured_pairs: usize,
    common_cold_sweeps_per_sample: usize,
    calibration_sweeps: usize,
    measured_series_restarts: Vec<PhaseSeriesRestart>,
    calibration_ordinary_dispatch_ns: u128,
    calibration_observed_dispatch_ns: u128,
    minimum_dispatch_sample_ns: u128,
    timing_scope: String,
    phase_boundary_contract: String,
    phase_overhead_interpretation: String,
    preflight: PipelinePhasePreflight,
    ordinary_variant: PipelinePhaseVariant,
    observed_variant: PipelinePhaseVariant,
    paired_observer_overhead_ratios: Vec<f64>,
    median_observer_overhead_ratio: f64,
    min_observer_overhead_ratio: f64,
    max_observer_overhead_ratio: f64,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
struct WarmRunReport {
    schema: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    workload_profile: Option<&'static str>,
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

#[derive(Clone, Copy)]
struct WarmDispatchPhaseStratumSpec {
    name: &'static str,
    input_index: usize,
    case_id: &'static str,
    entry_rips: &'static [u64],
}

#[derive(Serialize)]
struct WarmDispatchPhasesReport {
    schema: u32,
    profile: &'static str,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    trace_visits: usize,
    timing_scope: String,
    phase_boundary_contract: String,
    observer_effect_interpretation: String,
    cache_counter_interpretation: String,
    strata: Vec<WarmDispatchPhasesStratumReport>,
    total_elapsed_ns: u128,
}

#[derive(Serialize)]
struct WarmDispatchPhasesStratumReport {
    name: &'static str,
    case_index: usize,
    case_id: String,
    fixture_sha256: String,
    trace_entry_rips: Vec<u64>,
    unique_entry_count: usize,
    preflight: WarmDispatchPreflight,
    calibration_attempts: Vec<WarmDispatchCalibrationAttempt>,
    common_sweeps_per_sample: usize,
    measured_pairs: usize,
    measured_series_restarts: Vec<WarmDispatchSeriesRestart>,
    ordinary_variant: WarmDispatchVariantSamples,
    observed_variant: WarmDispatchVariantSamples,
    paired_observed_over_ordinary_call_ratios: Vec<f64>,
    median_observed_over_ordinary_call_ratio: f64,
    min_observed_over_ordinary_call_ratio: f64,
    max_observed_over_ordinary_call_ratio: f64,
}

#[derive(Serialize)]
struct WarmDispatchPreflight {
    full_state_matches_interpreter_oracle: bool,
    ordinary_observed_results_match: bool,
    ordinary_observed_normalized_state_matches: bool,
    ordinary_observed_warm_hit_deltas_match: bool,
    cold_prewarm_counters_before: Counters,
    cold_prewarm_counters_after: Counters,
    cold_prewarm_delta: WarmDispatchCacheDelta,
    ordinary_warm_counters_before: Counters,
    ordinary_warm_counters_after: Counters,
    ordinary_warm_delta: WarmDispatchCacheDelta,
    observed_warm_counters_before: Counters,
    observed_warm_counters_after: Counters,
    observed_warm_delta: WarmDispatchCacheDelta,
    observed_phase_counts_match_visits: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
struct WarmDispatchCacheDelta {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
}

#[derive(Serialize)]
struct WarmDispatchCalibrationAttempt {
    sweeps_per_side: usize,
    ordinary_dispatch_call_elapsed_ns: u128,
    observed_dispatch_call_elapsed_ns: u128,
}

#[derive(Serialize)]
struct WarmDispatchSeriesRestart {
    failed_pair: usize,
    attempted_sweeps: usize,
    ordinary_dispatch_call_elapsed_ns: u128,
    observed_dispatch_call_elapsed_ns: u128,
    next_sweeps: usize,
}

#[derive(Serialize)]
struct WarmDispatchVariantSamples {
    variant: &'static str,
    samples: Vec<WarmDispatchSample>,
}

#[derive(Serialize)]
struct WarmDispatchSample {
    pair: usize,
    order: usize,
    sweeps: usize,
    dispatch_calls: u64,
    retired_guest_instructions: u64,
    native_block_executions: u64,
    dispatch_call_elapsed_ns: u128,
    attributed_phase_ns: u128,
    unattributed_adapter_dispatcher_ns: u128,
    cache_delta: WarmDispatchCacheDelta,
    counters_before: Counters,
    counters_after: Counters,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    arena_capacity_slots: usize,
    arena_available_slots_from_high_water: usize,
    phase_totals: Vec<PipelinePhaseTotal>,
}

struct WarmDispatchSweep {
    elapsed_ns: u128,
    calls: u64,
    retired: u64,
    native_blocks: u64,
    cache_delta: WarmDispatchCacheDelta,
    counters_before: Counters,
    counters_after: Counters,
    phase_totals: Vec<PipelinePhaseTotal>,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct ChurnCounterDelta {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
}

#[derive(Serialize)]
struct ChurnSweepReport {
    sweep: usize,
    timed_dispatch_elapsed_ns: u128,
    input_calls: u64,
    native_block_executions: u64,
    retired_guest_instructions: u64,
    counter_delta: ChurnCounterDelta,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    arena_unoccupied_slots_at_resident_high_water: usize,
    arena_reserved_bytes: usize,
}

#[derive(Serialize)]
struct CacheChurnReport {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    inputs: usize,
    visits: usize,
    cache_capacity_entries: usize,
    fixed_sweeps: usize,
    timing_scope: String,
    excluded_from_timing: String,
    timed_dispatch_elapsed_ns: u128,
    input_calls: u64,
    native_block_executions: u64,
    retired_guest_instructions: u64,
    cumulative_counter_totals: ChurnCounterDelta,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    arena_unoccupied_slots_at_resident_high_water: usize,
    arena_reserved_bytes: usize,
    process_rss_before_runtime_creation: ProcessRssSnapshot,
    process_rss_after_sweeps_before_runtime_drop: ProcessRssSnapshot,
    process_rss_after_runtime_drop: ProcessRssSnapshot,
    teardown_or_independent_instance_lifecycle_measured: bool,
    sweeps: Vec<ChurnSweepReport>,
    total_elapsed_ns: u128,
}

#[derive(Clone, Copy, Debug, Serialize)]
struct LifecycleCacheSnapshot {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    evictions: u64,
    resident_entries_current: usize,
    resident_code_bytes_current: usize,
    resident_logical_metadata_bytes_current: usize,
    resident_entries_high_water: usize,
    resident_code_bytes_high_water: usize,
    resident_logical_metadata_bytes_high_water: usize,
    arena_capacity_slots: usize,
    arena_available_slots_actual: usize,
    arena_reserved_mapping_bytes: usize,
}

#[derive(Clone, Debug, Serialize)]
struct LifecycleExecution {
    dispatch_exit: String,
    blocks_executed: u64,
    retired_guest_instructions: u64,
    remaining_budget: u64,
    flags_mask: u64,
    state_debug: String,
}

#[derive(Clone, Debug, Serialize)]
struct LifecycleStep {
    index: usize,
    action: String,
    case_id: Option<String>,
    code_sha256: Option<String>,
    context_id: Option<u64>,
    mapping_id: Option<u64>,
    mapping_version: Option<u64>,
    backing_id: Option<u64>,
    backing_version: Option<u64>,
    invalidated_entries: Option<usize>,
    elapsed_ns: u128,
    cache_before: Option<LifecycleCacheSnapshot>,
    cache_after: Option<LifecycleCacheSnapshot>,
    execution: Option<LifecycleExecution>,
}

#[derive(Serialize)]
struct CacheLifecycleReport {
    schema: u32,
    process_run: u8,
    case_list_sha256: String,
    trace_sha256: String,
    capture_host: HostMetadata,
    measure_host: HostMetadata,
    cache_capacity_entries: usize,
    initial_fill_case_ids: Vec<String>,
    overflow_case_id: String,
    invalidation_target_case_id: String,
    changed_code_case_id: String,
    changed_code_sha256: String,
    original_mapping_id: u64,
    original_backing_id: u64,
    original_backing_version: u64,
    changed_backing_version: u64,
    changed_result_differs_under_same_initial_state: bool,
    context_generations: Vec<u64>,
    lifecycle_contract: String,
    limitations: Vec<String>,
    steps: Vec<LifecycleStep>,
    total_elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct ProcessRssSnapshot {
    rss_bytes: Option<u64>,
    method: &'static str,
    unavailable_reason: Option<String>,
}

/// Captures a process-wide RSS snapshot outside all timed dispatch intervals.
/// Cache attribution is not possible from this metric.
fn process_rss_snapshot() -> ProcessRssSnapshot {
    #[cfg(target_os = "macos")]
    {
        const TIMEOUT: Duration = Duration::from_secs(1);
        const METHOD: &str = "macOS /bin/ps -o rss= -p <pid>; KiB converted to bytes";
        let child = Command::new("/bin/ps")
            .args(["-o", "rss=", "-p"])
            .arg(std::process::id().to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("failed to start ps: {error}"));
        let result = child.and_then(|mut child| {
            let started = Instant::now();
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if started.elapsed() < TIMEOUT => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Ok(None) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err("ps timed out after 1 second".to_owned());
                    }
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break Err(format!("failed waiting for ps: {error}"));
                    }
                }
            }?;
            if !status.success() {
                return Err(format!("ps exited with status {status}"));
            }
            let mut stdout = String::new();
            child
                .stdout
                .take()
                .ok_or_else(|| "ps stdout was unavailable".to_owned())?
                .read_to_string(&mut stdout)
                .map_err(|error| format!("failed to read ps output: {error}"))?;
            let rss_bytes = stdout
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|kib| kib.checked_mul(1024))
                .ok_or_else(|| "ps returned an invalid RSS value".to_owned())?;
            Ok(rss_bytes)
        });
        match result {
            Ok(rss_bytes) => ProcessRssSnapshot {
                rss_bytes: Some(rss_bytes),
                method: METHOD,
                unavailable_reason: None,
            },
            Err(reason) => ProcessRssSnapshot {
                rss_bytes: None,
                method: METHOD,
                unavailable_reason: Some(reason),
            },
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        ProcessRssSnapshot {
            rss_bytes: None,
            method: "unsupported target; process RSS is not sampled",
            unavailable_reason: Some(
                "cache-churn RSS sampling is implemented for macOS only".into(),
            ),
        }
    }
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

/// Measures one complete S3-17 or S3-14 branch/helper corpus pass from an empty
/// FIFO block cache against the interpreter on the same frozen fixtures. Each
/// measured JIT sweep owns a new arena/cache. Runtime teardown and harness
/// state/reset validation are outside the timed intervals.
pub fn run_cold_first_execution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    run_cold_first_profile(case_list, trace_path, process_run, false)
}

/// S3-56: the frozen resident S3-14 corpus through the generic native path.
pub fn run_generic_cold_first_execution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    run_cold_first_profile(case_list, trace_path, process_run, true)
}

fn run_cold_first_profile(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
    generic: bool,
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
    let case_list_sha256 = od_testkit::sha256_hex(&list);
    let trace_sha256 = od_testkit::sha256_hex(&trace_raw);
    let branch_helper = generic || case_list_sha256 == BRANCH_HELPER_CASE_LIST_SHA256;
    let (list_hash, trace_hash, expected_inputs, expected_visits, label) = if branch_helper {
        (
            BRANCH_HELPER_CASE_LIST_SHA256,
            BRANCH_HELPER_TRACE_SHA256,
            BRANCH_HELPER_INPUTS,
            BRANCH_HELPER_VISITS,
            "S3-14 branch/helper",
        )
    } else {
        (
            FROZEN_CASE_LIST_SHA256,
            FROZEN_TRACE_SHA256,
            MAX_INPUTS,
            1_322,
            "S3-17",
        )
    };
    if case_list_sha256 != list_hash || trace_sha256 != trace_hash {
        return Err(err(format!(
            "case list or trace does not match the frozen {label} corpus"
        )));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.inputs.len() != expected_inputs
        || trace.visits.len() != expected_visits
        || trace.visits.len() > MAX_TOTAL_TRACE_VISITS
    {
        return Err(err("frozen trace visit count mismatch"));
    }
    let mut cases = prepare_cases(&list, &trace, started)?;
    if cases.len() != expected_inputs {
        return Err(err("cold-first-run requires the complete frozen corpus"));
    }
    if generic {
        prepare_generic_admissions(&mut cases)?;
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
        case_list_sha256,
        trace_sha256,
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
    annotate_generic_report(
        serde_json::to_value(report).map_err(|e| err(e.to_string()))?,
        generic,
        &cases,
    )
}

/// Attribute a cold FIFO16 dispatcher sweep to measured boundaries on the
/// actual cache/compiler path. The ordinary and observed variants execute the
/// same prepared cases through fresh runtimes and are compared before timing.
pub fn run_cold_pipeline_attribution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cold-phases measurement requires a release build"));
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
        return Err(err("cold-phases requires the complete frozen S3-17 corpus"));
    }

    // Every case is checked against the independently prepared interpreter
    // oracle inside each sweep; the fresh ordinary/observed paths must also
    // agree on aggregate retirement and cache behavior before calibration.
    let ordinary_preflight = pipeline_sweep(&cases, trace.visits.len(), started, false, true)?;
    let observed_preflight = pipeline_sweep(&cases, trace.visits.len(), started, true, true)?;
    let cache_counters_match =
        counters_match(ordinary_preflight.counters, observed_preflight.counters);
    let dispatch_results_match = ordinary_preflight.calls == observed_preflight.calls
        && ordinary_preflight.retired == observed_preflight.retired
        && ordinary_preflight.native_blocks == observed_preflight.native_blocks
        && ordinary_preflight.outcomes == observed_preflight.outcomes;
    if !cache_counters_match || !dispatch_results_match {
        return Err(err("ordinary and observed cold preflight diverged"));
    }
    validate_phase_counts(&observed_preflight, trace.visits.len())?;

    let mut calibration_sweeps = 0usize;
    let mut calibration_ordinary_ns = 0u128;
    let mut calibration_observed_ns = 0u128;
    while calibration_ordinary_ns < MIN_SAMPLE.as_nanos()
        || calibration_observed_ns < MIN_SAMPLE.as_nanos()
    {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err(
                "30 minute run limit exceeded during cold phase calibration",
            ));
        }
        if calibration_sweeps % 2 == 0 {
            let ordinary = pipeline_sweep(&cases, trace.visits.len(), started, false, false)?;
            let observed = pipeline_sweep(&cases, trace.visits.len(), started, true, false)?;
            calibration_ordinary_ns =
                calibration_ordinary_ns.saturating_add(ordinary.dispatch_elapsed.as_nanos());
            calibration_observed_ns =
                calibration_observed_ns.saturating_add(observed.dispatch_elapsed.as_nanos());
        } else {
            let observed = pipeline_sweep(&cases, trace.visits.len(), started, true, false)?;
            let ordinary = pipeline_sweep(&cases, trace.visits.len(), started, false, false)?;
            calibration_ordinary_ns =
                calibration_ordinary_ns.saturating_add(ordinary.dispatch_elapsed.as_nanos());
            calibration_observed_ns =
                calibration_observed_ns.saturating_add(observed.dispatch_elapsed.as_nanos());
        }
        calibration_sweeps = calibration_sweeps
            .checked_add(1)
            .ok_or_else(|| err("phase calibration sweep counter overflow"))?;
    }

    let mut common_sweeps = calibration_sweeps;
    let mut series_restarts = Vec::new();
    let (ordinary_samples, observed_samples) = loop {
        let mut ordinary_samples = Vec::with_capacity(FIRST_RUN_PAIRS);
        let mut observed_samples = Vec::with_capacity(FIRST_RUN_PAIRS);
        let mut short_pair = None;
        for pair in 0..FIRST_RUN_PAIRS {
            if started.elapsed() > MAX_RUN_TIME {
                return Err(err(
                    "30 minute run limit exceeded during cold phase samples",
                ));
            }
            let observed_first = (pair + usize::from(process_run)) % 2 != 0;
            let (ordinary, observed) = if observed_first {
                let observed = measure_pipeline_sample(
                    pair,
                    0,
                    common_sweeps,
                    &cases,
                    trace.visits.len(),
                    started,
                    true,
                )?;
                let ordinary = measure_pipeline_sample(
                    pair,
                    1,
                    common_sweeps,
                    &cases,
                    trace.visits.len(),
                    started,
                    false,
                )?;
                (ordinary, observed)
            } else {
                let ordinary = measure_pipeline_sample(
                    pair,
                    0,
                    common_sweeps,
                    &cases,
                    trace.visits.len(),
                    started,
                    false,
                )?;
                let observed = measure_pipeline_sample(
                    pair,
                    1,
                    common_sweeps,
                    &cases,
                    trace.visits.len(),
                    started,
                    true,
                )?;
                (ordinary, observed)
            };
            if ordinary.dispatch_elapsed_ns < MIN_SAMPLE.as_nanos()
                || observed.dispatch_elapsed_ns < MIN_SAMPLE.as_nanos()
            {
                let min_elapsed = ordinary
                    .dispatch_elapsed_ns
                    .min(observed.dispatch_elapsed_ns)
                    .max(1);
                let scaled = common_sweeps
                    .saturating_mul(
                        usize::try_from(MIN_SAMPLE.as_nanos().div_ceil(min_elapsed))
                            .unwrap_or(usize::MAX),
                    )
                    .saturating_add(1);
                let next_sweeps = common_sweeps.saturating_add(1).max(scaled);
                short_pair = Some(PhaseSeriesRestart {
                    failed_pair: pair,
                    attempted_sweeps: common_sweeps,
                    ordinary_dispatch_ns: ordinary.dispatch_elapsed_ns,
                    observed_dispatch_ns: observed.dispatch_elapsed_ns,
                    next_sweeps,
                });
                break;
            }
            ordinary_samples.push(ordinary);
            observed_samples.push(observed);
        }
        if let Some(restart) = short_pair {
            common_sweeps = restart.next_sweeps;
            series_restarts.push(restart);
            if series_restarts.len() > 8 {
                return Err(err(
                    "cold phase sample repeatedly fell below the 200ms floor",
                ));
            }
            continue;
        }
        break (ordinary_samples, observed_samples);
    };

    let paired_observer_overhead_ratios = ordinary_samples
        .iter()
        .zip(&observed_samples)
        .map(|(ordinary, observed)| {
            observed.dispatch_elapsed_ns as f64 / ordinary.dispatch_elapsed_ns as f64
        })
        .collect::<Vec<_>>();
    let mut sorted_ratios = paired_observer_overhead_ratios.clone();
    sorted_ratios.sort_by(f64::total_cmp);
    let median_observer_overhead_ratio = median_f64(&sorted_ratios);
    let min_observer_overhead_ratio = *sorted_ratios.first().unwrap_or(&1.0);
    let max_observer_overhead_ratio = *sorted_ratios.last().unwrap_or(&1.0);
    let preflight = PipelinePhasePreflight {
        full_state_exit_retirement_budget_parity: true,
        ordinary_observed_dispatch_results_match: dispatch_results_match,
        ordinary_observed_cache_counters_match: cache_counters_match,
        ordinary_calls: ordinary_preflight.calls,
        observed_calls: observed_preflight.calls,
        ordinary_retired_guest_instructions: ordinary_preflight.retired,
        observed_retired_guest_instructions: observed_preflight.retired,
        ordinary_native_block_executions: ordinary_preflight.native_blocks,
        observed_native_block_executions: observed_preflight.native_blocks,
        ordinary_cache_hits: ordinary_preflight.counters.hits,
        observed_cache_hits: observed_preflight.counters.hits,
        ordinary_cache_misses: ordinary_preflight.counters.misses,
        observed_cache_misses: observed_preflight.counters.misses,
        phase_event_counts_match_cache_counters: true,
    };
    let report = ColdPipelineReport {
        schema: 1,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host,
        measure_host: HostMetadata::current(),
        inputs: cases.len(),
        visits: trace.visits.len(),
        measured_pairs: FIRST_RUN_PAIRS,
        common_cold_sweeps_per_sample: common_sweeps,
        calibration_sweeps,
        measured_series_restarts: series_restarts,
        calibration_ordinary_dispatch_ns: calibration_ordinary_ns,
        calibration_observed_dispatch_ns: calibration_observed_ns,
        minimum_dispatch_sample_ns: MIN_SAMPLE.as_nanos(),
        timing_scope: "dispatch-call intervals across a complete S3-17 corpus sweep; each sweep constructs a fresh FIFO16 runtime/cache/arena; per-case state cloning and interpreter-result validation are outside each dispatch-call interval; arena teardown is outside intervals".into(),
        phase_boundary_contract: "real dispatcher boundaries: block decode/build and instruction-boundary admission; minimum-key sizing, TranslationKey/snapshot work and cache lookup; typed IR lowering; allocator build; ARM64 emission plus byte encoding/finalization; backend publication; host executor call. A same-RIP reuse records the previous-entry check instead of block build and hit accounting instead of key construction and lookup. Every phase is sequential and non-overlapping. Each observed phase duration spans the Instant sample in PhaseRecorder::begin through the clock read in PhaseRecorder::end, so it includes the wrapped operation and wrapper/callback transit between those samples.".into(),
        phase_overhead_interpretation: "ordinary/observed paired ratios measure total instrumentation overhead on identical cold sweeps. Phase durations include the operation and elapsed boundary transit between the begin/end clock samples; PhaseRecorder bookkeeping after the end clock read is excluded from that phase. other_dispatch_ns is measured dispatch total minus phase totals and includes control guards, cache install/accounting, result validation, and observer work outside measured phase intervals. Components are attribution within one observed pass, not independent end-to-end measurements and must not be summed with other benchmark reports.".into(),
        preflight,
        ordinary_variant: PipelinePhaseVariant {
            variant: "ordinary_noop_observer".into(),
            samples: ordinary_samples,
        },
        observed_variant: PipelinePhaseVariant {
            variant: "phase_observed".into(),
            samples: observed_samples,
        },
        paired_observer_overhead_ratios,
        median_observer_overhead_ratio,
        min_observer_overhead_ratio,
        max_observer_overhead_ratio,
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
    run_warm_profile(
        case_list,
        trace_path,
        process_run,
        None,
        FROZEN_CASE_LIST_SHA256,
        FROZEN_TRACE_SHA256,
        MAX_INPUTS,
        1_322,
        "S3-17",
        false,
    )
}

/// S3-56: warm execution of the same frozen five-case generic corpus as cold.
pub fn run_generic_warm_first_execution(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    run_warm_profile(
        case_list,
        trace_path,
        process_run,
        Some("s3-14-generic-register"),
        BRANCH_HELPER_CASE_LIST_SHA256,
        BRANCH_HELPER_TRACE_SHA256,
        BRANCH_HELPER_INPUTS,
        BRANCH_HELPER_VISITS,
        "S3-14 branch/helper",
        true,
    )
}

/// Measures the checked S3-14 branch/helper fixtures on a resident FIFO16
/// runtime. The exact fixture list and captured trace are frozen by digest.
pub fn run_warm_branch_helper(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    run_warm_profile(
        case_list,
        trace_path,
        process_run,
        Some("s3-14-branch-helper"),
        BRANCH_HELPER_CASE_LIST_SHA256,
        BRANCH_HELPER_TRACE_SHA256,
        BRANCH_HELPER_INPUTS,
        BRANCH_HELPER_VISITS,
        "S3-14 branch/helper",
        false,
    )
}

/// Compares ordinary and phase-observed dispatch calls on two explicitly
/// selected warmed strata from the accepted frozen S3-17 trace.
pub fn run_warm_dispatch_phases(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("warm-dispatch-phases requires a release build"));
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
    if trace.inputs.len() != MAX_INPUTS || trace.visits.len() != 1_322 {
        return Err(err("frozen S3-17 input or trace visit count mismatch"));
    }
    let cases = prepare_cases(&list, &trace, started)?;
    if cases.len() != MAX_INPUTS {
        return Err(err(
            "warm-dispatch-phases requires the full S3-17 case list",
        ));
    }

    let mut strata = Vec::with_capacity(WARM_DISPATCH_PHASE_STRATA.len());
    for spec in WARM_DISPATCH_PHASE_STRATA {
        let trace_entry_rips = trace
            .visits
            .iter()
            .filter(|visit| visit.input_index == spec.input_index)
            .map(|visit| visit.entry_rip)
            .collect::<Vec<_>>();
        if trace_entry_rips != spec.entry_rips {
            return Err(err(format!(
                "frozen {} trace entry sequence mismatch",
                spec.name
            )));
        }
        let case = cases
            .get(spec.input_index)
            .ok_or_else(|| err(format!("frozen {} case index is missing", spec.name)))?;
        let unique_entry_count = spec
            .entry_rips
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>()
            .len();
        if case.id != spec.case_id
            || case.entry_count != spec.entry_rips.len()
            || case.unique_entries != unique_entry_count
        {
            return Err(err(format!("frozen {} case identity mismatch", spec.name)));
        }
        strata.push(run_warm_dispatch_phase_stratum(
            spec,
            case,
            &trace.inputs[spec.input_index].fixture_sha256,
            trace_entry_rips,
            unique_entry_count,
            process_run,
            started,
        )?);
    }

    let report = WarmDispatchPhasesReport {
        schema: 1,
        profile: "s3-17-recorded-multiblock-and-self-loop",
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host,
        measure_host: HostMetadata::current(),
        inputs: cases.len(),
        trace_visits: trace.visits.len(),
        timing_scope: "sum of full Variant::call / Variant::call_observed adapter-plus-dispatcher intervals over one warmed FIFO(16) stratum; each interval includes CodeView, dependency and control/context setup performed by the adapter plus the complete dispatcher call. Case-state cloning/reset, full result/state validation, PhaseRecorder construction/reset and totals extraction are outside the interval. Ordinary and observed paths alternate on one shared warmed engine and arena; strata are never aggregated.".into(),
        phase_boundary_contract: "warm hit-only phase inventory: block_build_and_admission; key_snapshot_and_initial_lookup (minimum sizing, TranslationKey construction, initial snapshot validation, one cache.find and hit/miss update on a normal lookup; hit accounting only on same-RIP reuse); host_execution_boundary (backend/arena checks, host context, generated entry and result decoding, not pure guest execution). Lowering/allocation/emission/publication phase calls must be zero after warmup. Phase call counts are checked against the full expected visit/block counts for each completed deterministic dispatch; an early exit fails reconciliation. PhaseRecorder end-phase/index work before its elapsed clock read is inside that phase; duration/call accumulation after the clock read is outside. Block-build samples include the previous-entry check on same-RIP reuse. The unattributed_adapter_dispatcher_ns residual includes next-entry selection at continuation, cached snapshot and code-view revalidation, guards, accounting, other cache bookkeeping, adapter setup, and observer bookkeeping outside the phase clock samples.".into(),
        observer_effect_interpretation: "paired observed/ordinary call ratios are the net difference between two full adapter-plus-dispatcher call paths, including PhaseRecorder work and any difference between generic instantiations; they are not a pure observer-cost estimate. No pure generated-code host-execution time, dispatcher-only total lookup time, or linking benefit is isolated.".into(),
        cache_counter_interpretation: "hit/miss/compile/publication/eviction fields are per-sample deltas from cumulative counters. Resident entry/code/metadata values are high-water values; arena_available_slots_from_high_water is derived from capacity minus resident-entry high-water and is not live occupancy.".into(),
        strata,
        total_elapsed_ns: started.elapsed().as_nanos(),
    };
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

fn run_warm_dispatch_phase_stratum(
    spec: WarmDispatchPhaseStratumSpec,
    case: &Case,
    fixture_sha256: &str,
    trace_entry_rips: Vec<u64>,
    unique_entry_count: usize,
    process_run: u8,
    started: Instant,
) -> Result<WarmDispatchPhasesStratumReport, BenchError> {
    let mut engine = Variant::new(VariantName::MultiFifo16)?;

    let cold_prewarm_counters_before = engine.counters();
    let mut cold_state = case.initial.clone();
    let cold_result = engine.call(case, &mut cold_state)?;
    validate_dispatch_result(case, &cold_state, &cold_result)?;
    validate_full_dispatch_state(case, &cold_state)?;
    let cold_prewarm_counters_after = engine.counters();
    let cold_prewarm_delta =
        cache_counter_delta(cold_prewarm_counters_before, cold_prewarm_counters_after)?;
    validate_warm_prewarm_delta(case, unique_entry_count, cold_prewarm_delta, spec.name)?;

    let ordinary_warm_counters_before = engine.counters();
    let mut ordinary_state = case.initial.clone();
    let ordinary_result = engine.call(case, &mut ordinary_state)?;
    validate_dispatch_result(case, &ordinary_state, &ordinary_result)?;
    validate_full_dispatch_state(case, &ordinary_state)?;
    let ordinary_warm_counters_after = engine.counters();
    let ordinary_warm_delta =
        cache_counter_delta(ordinary_warm_counters_before, ordinary_warm_counters_after)?;
    validate_warm_hit_delta(case, ordinary_warm_delta, spec.name)?;

    let observed_warm_counters_before = engine.counters();
    let mut observed_state = case.initial.clone();
    let mut preflight_recorder = PhaseRecorder::default();
    let observed_result =
        engine.call_observed(case, &mut observed_state, &mut preflight_recorder)?;
    validate_dispatch_result(case, &observed_state, &observed_result)?;
    validate_full_dispatch_state(case, &observed_state)?;
    let observed_warm_counters_after = engine.counters();
    let observed_warm_delta =
        cache_counter_delta(observed_warm_counters_before, observed_warm_counters_after)?;
    validate_warm_hit_delta(case, observed_warm_delta, spec.name)?;
    let observed_warm_phases = collect_warm_phase_totals(&preflight_recorder);
    validate_warm_phase_counts(
        &observed_warm_phases,
        u64::try_from(case.entry_count).map_err(|_| err("warm phase visit count overflow"))?,
        0,
        observed_result.blocks_executed,
    )?;

    let normalized_states_match = normalized_case_state(case, &ordinary_state)
        == normalized_case_state(case, &observed_state);
    let results_match = ordinary_result == observed_result;
    let warm_hit_deltas_match = ordinary_warm_delta == observed_warm_delta;
    if !normalized_states_match || !results_match || !warm_hit_deltas_match {
        return Err(err(format!(
            "ordinary/observed warmed preflight mismatch for {}",
            spec.name
        )));
    }

    let mut calibration_attempts = Vec::new();
    let mut common_sweeps = 1usize;
    loop {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err(
                "30 minute run limit exceeded during warm phase calibration",
            ));
        }
        let observed_first = (calibration_attempts.len() + usize::from(process_run)) % 2 != 0;
        let (ordinary, observed) = if observed_first {
            let observed =
                run_warm_dispatch_sweep(&mut engine, case, common_sweeps, true, started)?;
            let ordinary =
                run_warm_dispatch_sweep(&mut engine, case, common_sweeps, false, started)?;
            (ordinary, observed)
        } else {
            let ordinary =
                run_warm_dispatch_sweep(&mut engine, case, common_sweeps, false, started)?;
            let observed =
                run_warm_dispatch_sweep(&mut engine, case, common_sweeps, true, started)?;
            (ordinary, observed)
        };
        calibration_attempts.push(WarmDispatchCalibrationAttempt {
            sweeps_per_side: common_sweeps,
            ordinary_dispatch_call_elapsed_ns: ordinary.elapsed_ns,
            observed_dispatch_call_elapsed_ns: observed.elapsed_ns,
        });
        if ordinary.elapsed_ns >= MIN_SAMPLE.as_nanos()
            && observed.elapsed_ns >= MIN_SAMPLE.as_nanos()
        {
            break;
        }
        common_sweeps =
            scale_warm_phase_sweeps(common_sweeps, ordinary.elapsed_ns.min(observed.elapsed_ns))?;
    }

    let mut measured_series_restarts = Vec::new();
    let (ordinary_samples, observed_samples) = loop {
        let mut ordinary_samples = Vec::with_capacity(FIRST_RUN_PAIRS);
        let mut observed_samples = Vec::with_capacity(FIRST_RUN_PAIRS);
        let mut short_pair = None;
        for pair in 0..FIRST_RUN_PAIRS {
            if started.elapsed() > MAX_RUN_TIME {
                return Err(err(
                    "30 minute run limit exceeded during warm phase samples",
                ));
            }
            let observed_first = (pair + usize::from(process_run)) % 2 != 0;
            let (ordinary, observed) = if observed_first {
                let observed = run_warm_dispatch_phase_sample(
                    pair,
                    0,
                    common_sweeps,
                    &mut engine,
                    case,
                    true,
                    started,
                )?;
                let ordinary = run_warm_dispatch_phase_sample(
                    pair,
                    1,
                    common_sweeps,
                    &mut engine,
                    case,
                    false,
                    started,
                )?;
                (ordinary, observed)
            } else {
                let ordinary = run_warm_dispatch_phase_sample(
                    pair,
                    0,
                    common_sweeps,
                    &mut engine,
                    case,
                    false,
                    started,
                )?;
                let observed = run_warm_dispatch_phase_sample(
                    pair,
                    1,
                    common_sweeps,
                    &mut engine,
                    case,
                    true,
                    started,
                )?;
                (ordinary, observed)
            };
            if ordinary.dispatch_call_elapsed_ns < MIN_SAMPLE.as_nanos()
                || observed.dispatch_call_elapsed_ns < MIN_SAMPLE.as_nanos()
            {
                short_pair = Some(WarmDispatchSeriesRestart {
                    failed_pair: pair,
                    attempted_sweeps: common_sweeps,
                    ordinary_dispatch_call_elapsed_ns: ordinary.dispatch_call_elapsed_ns,
                    observed_dispatch_call_elapsed_ns: observed.dispatch_call_elapsed_ns,
                    next_sweeps: scale_warm_phase_sweeps(
                        common_sweeps,
                        ordinary
                            .dispatch_call_elapsed_ns
                            .min(observed.dispatch_call_elapsed_ns),
                    )?,
                });
                break;
            }
            ordinary_samples.push(ordinary);
            observed_samples.push(observed);
        }
        if let Some(restart) = short_pair {
            common_sweeps = restart.next_sweeps;
            measured_series_restarts.push(restart);
            if measured_series_restarts.len() > WARM_DISPATCH_PHASE_MAX_RESTARTS {
                return Err(err(
                    "warm phase samples repeatedly fell below the 200ms floor",
                ));
            }
            continue;
        }
        break (ordinary_samples, observed_samples);
    };

    let mut ratios = ordinary_samples
        .iter()
        .zip(&observed_samples)
        .map(|(ordinary, observed)| {
            observed.dispatch_call_elapsed_ns as f64 / ordinary.dispatch_call_elapsed_ns as f64
        })
        .collect::<Vec<_>>();
    if ratios.len() != FIRST_RUN_PAIRS {
        return Err(err(
            "warm phase series did not retain the requested pair count",
        ));
    }
    let mut sorted_ratios = ratios.clone();
    sorted_ratios.sort_by(f64::total_cmp);
    let median_ratio = median_f64(&sorted_ratios);
    let min_ratio = *sorted_ratios.first().unwrap();
    let max_ratio = *sorted_ratios.last().unwrap();
    ratios.shrink_to_fit();

    Ok(WarmDispatchPhasesStratumReport {
        name: spec.name,
        case_index: spec.input_index,
        case_id: case.id.clone(),
        fixture_sha256: fixture_sha256.to_owned(),
        trace_entry_rips,
        unique_entry_count,
        preflight: WarmDispatchPreflight {
            full_state_matches_interpreter_oracle: true,
            ordinary_observed_results_match: results_match,
            ordinary_observed_normalized_state_matches: normalized_states_match,
            ordinary_observed_warm_hit_deltas_match: warm_hit_deltas_match,
            cold_prewarm_counters_before,
            cold_prewarm_counters_after,
            cold_prewarm_delta,
            ordinary_warm_counters_before,
            ordinary_warm_counters_after,
            ordinary_warm_delta,
            observed_warm_counters_before,
            observed_warm_counters_after,
            observed_warm_delta,
            observed_phase_counts_match_visits: true,
        },
        calibration_attempts,
        common_sweeps_per_sample: common_sweeps,
        measured_pairs: FIRST_RUN_PAIRS,
        measured_series_restarts,
        ordinary_variant: WarmDispatchVariantSamples {
            variant: "ordinary_noop_observer",
            samples: ordinary_samples,
        },
        observed_variant: WarmDispatchVariantSamples {
            variant: "phase_observed",
            samples: observed_samples,
        },
        paired_observed_over_ordinary_call_ratios: ratios,
        median_observed_over_ordinary_call_ratio: median_ratio,
        min_observed_over_ordinary_call_ratio: min_ratio,
        max_observed_over_ordinary_call_ratio: max_ratio,
    })
}

fn validate_warm_prewarm_delta(
    case: &Case,
    unique_entry_count: usize,
    delta: WarmDispatchCacheDelta,
    stratum: &str,
) -> Result<(), BenchError> {
    let expected_misses = u64::try_from(unique_entry_count)
        .map_err(|_| err("warm phase unique-entry count overflow"))?;
    let expected_hits = u64::try_from(case.entry_count)
        .ok()
        .and_then(|visits| visits.checked_sub(expected_misses))
        .ok_or_else(|| err("warm phase prewarm hit count underflow"))?;
    if delta.hits != expected_hits
        || delta.misses != expected_misses
        || delta.successful_compiles != expected_misses
        || delta.successful_publications != expected_misses
        || delta.evictions != 0
    {
        return Err(err(format!(
            "{} prewarm counters do not match the frozen entry trace",
            stratum
        )));
    }
    Ok(())
}

fn validate_warm_hit_delta(
    case: &Case,
    delta: WarmDispatchCacheDelta,
    stratum: &str,
) -> Result<(), BenchError> {
    let expected_hits =
        u64::try_from(case.entry_count).map_err(|_| err("warm phase visit count overflow"))?;
    if delta.hits != expected_hits
        || delta.misses != 0
        || delta.successful_compiles != 0
        || delta.successful_publications != 0
        || delta.evictions != 0
    {
        return Err(err(format!(
            "{} warm call was not hit-only with expected visits",
            stratum
        )));
    }
    Ok(())
}

fn normalized_case_state(case: &Case, state: &LongModeState) -> LongModeState {
    let mut normalized = state.clone();
    normalized.rflags &= case.flags_mask;
    normalized
}

fn cache_counter_delta(
    before: Counters,
    after: Counters,
) -> Result<WarmDispatchCacheDelta, BenchError> {
    if !before.cache_lookup_metrics_available || !after.cache_lookup_metrics_available {
        return Err(err("warm phase requires FIFO cache counters"));
    }
    Ok(WarmDispatchCacheDelta {
        hits: after
            .hits
            .checked_sub(before.hits)
            .ok_or_else(|| err("warm phase hit counter regressed"))?,
        misses: after
            .misses
            .checked_sub(before.misses)
            .ok_or_else(|| err("warm phase miss counter regressed"))?,
        successful_compiles: after
            .compiles
            .checked_sub(before.compiles)
            .ok_or_else(|| err("warm phase compile counter regressed"))?,
        successful_publications: after
            .publications
            .checked_sub(before.publications)
            .ok_or_else(|| err("warm phase publication counter regressed"))?,
        evictions: after
            .evictions
            .checked_sub(before.evictions)
            .ok_or_else(|| err("warm phase eviction counter regressed"))?,
    })
}

fn collect_warm_phase_totals(recorder: &PhaseRecorder) -> Vec<PipelinePhaseTotal> {
    recorder
        .totals()
        .into_iter()
        .map(|total| PipelinePhaseTotal {
            phase: warm_phase_name(total.phase).to_owned(),
            calls: total.calls,
            elapsed_ns: total.elapsed_ns,
        })
        .collect()
}

fn warm_phase_name(phase: ObservedDispatchPhase) -> &'static str {
    match phase {
        ObservedDispatchPhase::BlockBuildAndAdmission => "block_build_and_admission",
        ObservedDispatchPhase::KeySnapshotAndLookup => "key_snapshot_and_initial_lookup",
        ObservedDispatchPhase::IrLowering => "ir_lowering",
        ObservedDispatchPhase::Allocation => "allocation",
        ObservedDispatchPhase::EmissionAndFinalization => "emission_and_finalization",
        ObservedDispatchPhase::Publication => "publication",
        ObservedDispatchPhase::HostExecutionBoundary => "host_execution_boundary",
    }
}

fn validate_warm_phase_counts(
    phases: &[PipelinePhaseTotal],
    dispatcher_visits: u64,
    expected_compiles: u64,
    native_blocks: u64,
) -> Result<(), BenchError> {
    let phase_calls = |name: &str| -> u64 {
        phases
            .iter()
            .find(|phase| phase.phase == name)
            .map_or(0, |phase| phase.calls)
    };
    if phase_calls("block_build_and_admission") != dispatcher_visits
        || phase_calls("key_snapshot_and_initial_lookup") != dispatcher_visits
        || phase_calls("host_execution_boundary") != native_blocks
        || phase_calls("ir_lowering") != expected_compiles
        || phase_calls("allocation") != expected_compiles
        || phase_calls("emission_and_finalization") != expected_compiles
        || phase_calls("publication") != expected_compiles
    {
        return Err(err(
            "warm dispatch phase counts do not match visits/counters",
        ));
    }
    Ok(())
}

fn run_warm_dispatch_sweep(
    engine: &mut Variant,
    case: &Case,
    sweeps: usize,
    observed: bool,
    started: Instant,
) -> Result<WarmDispatchSweep, BenchError> {
    if sweeps == 0 {
        return Err(err("warm phase sweep count must be nonzero"));
    }
    let counters_before = engine.counters();
    let mut recorder = PhaseRecorder::default();
    let mut elapsed_ns = 0u128;
    let mut calls = 0u64;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    for _ in 0..sweeps {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during warm phase sweep"));
        }
        let mut state = case.initial.clone();
        let call_started = Instant::now();
        let result = if observed {
            engine.call_observed(case, &mut state, &mut recorder)?
        } else {
            engine.call(case, &mut state)?
        };
        elapsed_ns = elapsed_ns
            .checked_add(call_started.elapsed().as_nanos())
            .ok_or_else(|| err("warm phase dispatch elapsed overflow"))?;

        // Full parity work is deliberately outside each timed adapter call.
        validate_dispatch_result(case, &state, &result)?;
        validate_full_dispatch_state(case, &state)?;
        calls = calls
            .checked_add(1)
            .ok_or_else(|| err("warm phase call count overflow"))?;
        retired = retired
            .checked_add(result.retired_instructions)
            .ok_or_else(|| err("warm phase retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(result.blocks_executed)
            .ok_or_else(|| err("warm phase native-block count overflow"))?;
    }

    let counters_after = engine.counters();
    let cache_delta = cache_counter_delta(counters_before, counters_after)?;
    let visits_per_sweep =
        u64::try_from(case.entry_count).map_err(|_| err("warm phase visit count overflow"))?;
    let sweep_count = u64::try_from(sweeps).map_err(|_| err("warm phase sweep count overflow"))?;
    let expected_hits = visits_per_sweep
        .checked_mul(sweep_count)
        .ok_or_else(|| err("warm phase expected hit count overflow"))?;
    let expected_retired = case
        .retired
        .checked_mul(sweep_count)
        .ok_or_else(|| err("warm phase expected retirement count overflow"))?;
    let expected_native_blocks = visits_per_sweep
        .checked_mul(sweep_count)
        .ok_or_else(|| err("warm phase expected native-block count overflow"))?;
    if calls != sweep_count
        || retired != expected_retired
        || native_blocks != expected_native_blocks
        || cache_delta.hits != expected_hits
        || cache_delta.misses != 0
        || cache_delta.successful_compiles != 0
        || cache_delta.successful_publications != 0
        || cache_delta.evictions != 0
    {
        return Err(err(
            "warm dispatch sweep was not hit-only with expected work",
        ));
    }

    let phase_totals = if observed {
        let dispatcher_visits = visits_per_sweep
            .checked_mul(sweep_count)
            .ok_or_else(|| err("warm phase event count overflow"))?;
        let totals = collect_warm_phase_totals(&recorder);
        validate_warm_phase_counts(&totals, dispatcher_visits, 0, native_blocks)?;
        totals
    } else {
        Vec::new()
    };
    Ok(WarmDispatchSweep {
        elapsed_ns,
        calls,
        retired,
        native_blocks,
        cache_delta,
        counters_before,
        counters_after,
        phase_totals,
    })
}

fn run_warm_dispatch_phase_sample(
    pair: usize,
    order: usize,
    sweeps: usize,
    engine: &mut Variant,
    case: &Case,
    observed: bool,
    started: Instant,
) -> Result<WarmDispatchSample, BenchError> {
    let sweep = run_warm_dispatch_sweep(engine, case, sweeps, observed, started)?;
    let attributed_phase_ns = sweep
        .phase_totals
        .iter()
        .try_fold(0u128, |sum, phase| sum.checked_add(phase.elapsed_ns))
        .ok_or_else(|| err("warm phase duration sum overflow"))?;
    let unattributed_adapter_dispatcher_ns = sweep
        .elapsed_ns
        .checked_sub(attributed_phase_ns)
        .ok_or_else(|| err("warm phases exceed full adapter-plus-dispatcher elapsed"))?;
    Ok(WarmDispatchSample {
        pair,
        order,
        sweeps,
        dispatch_calls: sweep.calls,
        retired_guest_instructions: sweep.retired,
        native_block_executions: sweep.native_blocks,
        dispatch_call_elapsed_ns: sweep.elapsed_ns,
        attributed_phase_ns,
        unattributed_adapter_dispatcher_ns,
        cache_delta: sweep.cache_delta,
        counters_before: sweep.counters_before,
        counters_after: sweep.counters_after,
        resident_entries_high_water: sweep.counters_after.resident,
        resident_code_bytes_high_water: sweep.counters_after.code_bytes,
        resident_logical_metadata_bytes_high_water: sweep.counters_after.logical_bytes,
        arena_capacity_slots: sweep.counters_after.arena_capacity,
        arena_available_slots_from_high_water: sweep.counters_after.arena_free,
        phase_totals: sweep.phase_totals,
    })
}

fn scale_warm_phase_sweeps(current: usize, shortest_elapsed_ns: u128) -> Result<usize, BenchError> {
    let floor_ns = MIN_SAMPLE.as_nanos();
    let shortest_elapsed_ns = shortest_elapsed_ns.max(1);
    let multiplier = usize::try_from(floor_ns.div_ceil(shortest_elapsed_ns))
        .unwrap_or(usize::MAX)
        .max(2);
    current
        .checked_mul(multiplier)
        .and_then(|sweeps| sweeps.checked_add(1))
        .ok_or_else(|| err("warm phase calibration sweep count overflow"))
}

fn run_warm_profile(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
    workload_profile: Option<&'static str>,
    expected_case_list_sha256: &str,
    expected_trace_sha256: &str,
    expected_inputs: usize,
    expected_visits: usize,
    profile_label: &str,
    generic: bool,
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
    if od_testkit::sha256_hex(&list) != expected_case_list_sha256
        || od_testkit::sha256_hex(&trace_raw) != expected_trace_sha256
    {
        return Err(err(format!(
            "case list or trace does not match the frozen {profile_label} profile"
        )));
    }
    let trace: RecordedTrace =
        serde_json::from_slice(&trace_raw).map_err(|e| err(format!("trace JSON: {e}")))?;
    if trace.inputs.len() != expected_inputs
        || trace.visits.len() != expected_visits
        || trace.visits.len() > MAX_TOTAL_TRACE_VISITS
    {
        return Err(err(format!("frozen {profile_label} trace count mismatch")));
    }
    let mut all_cases = prepare_cases(&list, &trace, started)?;
    if all_cases.len() != expected_inputs {
        return Err(err(format!(
            "warm profile requires {expected_inputs} frozen inputs"
        )));
    }
    if generic {
        prepare_generic_admissions(&mut all_cases)?;
    }
    let (selected_case_count, selected_entry_budget) = select_resident_case_count(&all_cases)?;
    if generic && selected_case_count != expected_inputs {
        return Err(err("generic S3-14 corpus does not fit the resident cache"));
    }
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
        workload_profile,
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
    annotate_generic_report(
        serde_json::to_value(report).map_err(|e| err(e.to_string()))?,
        generic,
        cases,
    )
}

/// Records bounded capacity-churn counters for repeated execution of the
/// frozen register-only corpus on one retained FIFO16 runtime.
pub fn run_cache_churn(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cache-churn measurement requires a release build"));
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
        return Err(err("cache-churn requires the complete frozen corpus"));
    }

    let scenario = Scenario {
        name: "s3-17-frozen-corpus-cache-churn".into(),
        input_indices: (0..cases.len()).collect(),
        warmup_sweeps: 0,
        minimum_measured_sweeps: 0,
        cold_each_sweep: false,
        selected_case_ids: cases.iter().map(|case| case.id.clone()).collect(),
    };
    let process_rss_before_runtime_creation = process_rss_snapshot();
    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    if !engine.counters().cache_lookup_metrics_available {
        return Err(err("FIFO cache counters are unavailable"));
    }

    let mut timed_dispatch_elapsed = Duration::ZERO;
    let mut total_calls = 0u64;
    let mut total_native_blocks = 0u64;
    let mut total_retired = 0u64;
    let mut totals = ChurnCounterDelta::default();
    let mut resident_entries_high_water = 0usize;
    let mut resident_code_bytes_high_water = 0usize;
    let mut resident_logical_metadata_bytes_high_water = 0usize;
    let mut arena_unoccupied_slots_at_resident_high_water = ARENA_SLOTS;
    let mut arena_reserved_bytes = 0usize;
    let mut sweeps = Vec::with_capacity(CACHE_CHURN_SWEEPS);

    for index in 0..CACHE_CHURN_SWEEPS {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during cache churn"));
        }
        let before = engine.counters();
        let (elapsed, calls, native_blocks, retired, high) =
            sweep(&mut engine, &cases, &scenario.input_indices, started)?;
        let after = engine.counters();
        let counter_delta = churn_counter_delta(after, before)?;
        accumulate_churn_counters(&mut totals, counter_delta)?;
        timed_dispatch_elapsed = timed_dispatch_elapsed.saturating_add(elapsed);
        total_calls = total_calls
            .checked_add(calls)
            .ok_or_else(|| err("cache-churn call count overflow"))?;
        total_native_blocks = total_native_blocks
            .checked_add(native_blocks)
            .ok_or_else(|| err("cache-churn block count overflow"))?;
        total_retired = total_retired
            .checked_add(retired)
            .ok_or_else(|| err("cache-churn retirement count overflow"))?;
        resident_entries_high_water = resident_entries_high_water.max(high.resident);
        resident_code_bytes_high_water = resident_code_bytes_high_water.max(high.code_bytes);
        resident_logical_metadata_bytes_high_water =
            resident_logical_metadata_bytes_high_water.max(high.logical_bytes);
        let unoccupied_at_resident_high_water = ARENA_SLOTS.saturating_sub(high.resident);
        arena_unoccupied_slots_at_resident_high_water =
            arena_unoccupied_slots_at_resident_high_water.min(unoccupied_at_resident_high_water);
        arena_reserved_bytes = arena_reserved_bytes.max(high.arena_reserved_bytes);
        sweeps.push(ChurnSweepReport {
            sweep: index + 1,
            timed_dispatch_elapsed_ns: elapsed.as_nanos(),
            input_calls: calls,
            native_block_executions: native_blocks,
            retired_guest_instructions: retired,
            counter_delta,
            resident_entries_high_water: high.resident,
            resident_code_bytes_high_water: high.code_bytes,
            resident_logical_metadata_bytes_high_water: high.logical_bytes,
            arena_unoccupied_slots_at_resident_high_water: unoccupied_at_resident_high_water,
            arena_reserved_bytes: high.arena_reserved_bytes,
        });
    }

    let expected_calls = u64::try_from(cases.len())
        .ok()
        .and_then(|count| count.checked_mul(CACHE_CHURN_SWEEPS as u64))
        .ok_or_else(|| err("cache-churn expected call count overflow"))?;
    if total_calls != expected_calls
        || totals.evictions == 0
        || totals.misses != totals.successful_compiles
        || totals.successful_compiles != totals.successful_publications
        || resident_entries_high_water > ARENA_SLOTS
    {
        return Err(err(
            "cache-churn did not observe bounded FIFO eviction/recompile",
        ));
    }

    let process_rss_after_sweeps_before_runtime_drop = process_rss_snapshot();
    drop(engine);
    let process_rss_after_runtime_drop = process_rss_snapshot();

    let report = CacheChurnReport {
        schema: 2,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host.clone(),
        measure_host: HostMetadata::current(),
        inputs: trace.inputs.len(),
        visits: trace.visits.len(),
        cache_capacity_entries: ARENA_SLOTS,
        fixed_sweeps: CACHE_CHURN_SWEEPS,
        timing_scope: "sum of each production FIFO dispatch-call interval over complete frozen-corpus sweeps on one retained runtime/cache/arena; state cloning/reset and oracle validation occur outside timed calls".into(),
        excluded_from_timing: "fixture/trace parsing and admission, interpreter oracle preparation, per-case state clone/reset, post-call validation, runtime initialization/teardown, and process launch".into(),
        timed_dispatch_elapsed_ns: timed_dispatch_elapsed.as_nanos(),
        input_calls: total_calls,
        native_block_executions: total_native_blocks,
        retired_guest_instructions: total_retired,
        cumulative_counter_totals: totals,
        resident_entries_high_water,
        resident_code_bytes_high_water,
        resident_logical_metadata_bytes_high_water,
        arena_unoccupied_slots_at_resident_high_water,
        arena_reserved_bytes,
        process_rss_before_runtime_creation,
        process_rss_after_sweeps_before_runtime_drop,
        process_rss_after_runtime_drop,
        teardown_or_independent_instance_lifecycle_measured: false,
        sweeps,
        total_elapsed_ns: started.elapsed().as_nanos(),
    };
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

/// Records one bounded FIFO16 cache invalidation, replacement, clear, and
/// arena reopen sequence over admitted fixtures from the frozen S3-17 corpus.
/// This is a between-call cache-owner lifecycle experiment, not production SMC.
pub fn run_cache_lifecycle(
    case_list: &Path,
    trace_path: &Path,
    process_run: u8,
) -> Result<serde_json::Value, BenchError> {
    if cfg!(debug_assertions) {
        return Err(err("cache-lifecycle requires a release build"));
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
    if trace.inputs.len() != MAX_INPUTS
        || trace.visits.len() != 1_322
        || trace.visits.len() > MAX_TOTAL_TRACE_VISITS
    {
        return Err(err("frozen S3-17 trace count mismatch"));
    }
    let cases = prepare_cases(&list, &trace, started)?;
    if cases.len() != MAX_INPUTS {
        return Err(err("cache lifecycle requires the complete frozen corpus"));
    }

    let eligible: Vec<&Case> = cases
        .iter()
        .filter(|case| case.entry_count == 1 && case.unique_entries == 1 && case.initial.rip == 0)
        .collect();
    if eligible.len() < ARENA_SLOTS + 1 {
        return Err(err(format!(
            "cache lifecycle needs at least {} one-entry cases starting at RIP zero",
            ARENA_SLOTS + 1
        )));
    }

    // The target is inserted last, so one subsequent over-capacity insert
    // evicts an older filler rather than the entry selected for invalidation.
    let fillers = &eligible[..ARENA_SLOTS - 1];
    let target = eligible[ARENA_SLOTS - 1];
    let overflow = eligible[ARENA_SLOTS];
    let original_context = crate::benchmark_execution_context();
    let next_context_id = original_context
        .id
        .checked_next()
        .ok_or_else(|| err("execution context generation exhausted"))?;
    let reopened_context_id = next_context_id
        .checked_next()
        .ok_or_else(|| err("execution context generation exhausted"))?;
    let next_context = ExecutionContextSnapshot::s0_09_fixture(next_context_id);
    let reopened_context = ExecutionContextSnapshot::s0_09_fixture(reopened_context_id);

    let changed_version = target
        .identity
        .backing_version
        .checked_next()
        .ok_or_else(|| err("backing generation exhausted"))?;
    let mut changed_identity = target.identity;
    changed_identity.backing_version = changed_version;

    let mut changed_case = None;
    for candidate in cases
        .iter()
        .filter(|case| case.entry_count == 1 && case.unique_entries == 1 && case.initial.rip == 0)
    {
        if candidate.bytes == target.bytes {
            continue;
        }
        let candidate_case = lifecycle_case_with_program(
            candidate,
            &target.initial,
            target.budget,
            changed_identity,
            format!("{}-under-{}-new-backing", candidate.id, target.id),
        )?;
        if lifecycle_results_differ(target, &candidate_case) {
            changed_case = Some(candidate_case);
            break;
        }
    }
    let changed_case = changed_case.ok_or_else(|| {
        err("frozen S3-17 corpus has no admitted changed program with a distinct result")
    })?;

    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    let initial_snapshot = engine.fifo_lifecycle_snapshot()?;
    if initial_snapshot.resident_entries_current != 0
        || initial_snapshot.resident_code_bytes_current != 0
        || initial_snapshot.resident_logical_metadata_bytes_current != 0
        || initial_snapshot.arena_available_slots_actual != ARENA_SLOTS
    {
        return Err(err("fresh FIFO runtime did not start empty"));
    }

    let mut steps = Vec::new();
    for case in fillers.iter().copied().chain(std::iter::once(target)) {
        lifecycle_dispatch_step(
            &mut steps,
            &mut engine,
            case,
            case.identity,
            original_context,
            "fill_fifo_entry",
        )?;
    }
    let full_snapshot = engine.fifo_lifecycle_snapshot()?;
    if full_snapshot.resident_entries_current != ARENA_SLOTS
        || full_snapshot.arena_available_slots_actual != 0
        || full_snapshot.misses != ARENA_SLOTS as u64
        || full_snapshot.successful_compiles != ARENA_SLOTS as u64
        || full_snapshot.successful_publications != ARENA_SLOTS as u64
        || full_snapshot.evictions != 0
    {
        return Err(err("FIFO fill did not produce 16 cold resident entries"));
    }

    lifecycle_stop_boundary_step(&mut steps, &mut engine, target, original_context)?;

    lifecycle_dispatch_step(
        &mut steps,
        &mut engine,
        target,
        target.identity,
        original_context,
        "repeat_target_hit",
    )?;
    let hit_snapshot = engine.fifo_lifecycle_snapshot()?;
    if hit_snapshot.hits != 1
        || hit_snapshot.misses != full_snapshot.misses
        || hit_snapshot.successful_compiles != full_snapshot.successful_compiles
        || hit_snapshot.successful_publications != full_snapshot.successful_publications
        || hit_snapshot.evictions != full_snapshot.evictions
    {
        return Err(err("repeated target call was not a hit-only dispatch"));
    }

    lifecycle_dispatch_step(
        &mut steps,
        &mut engine,
        overflow,
        overflow.identity,
        original_context,
        "exceed_capacity_and_replace_oldest",
    )?;
    let churn_snapshot = engine.fifo_lifecycle_snapshot()?;
    if churn_snapshot.resident_entries_current != ARENA_SLOTS
        || churn_snapshot.arena_available_slots_actual != 0
        || churn_snapshot.evictions != 1
        || churn_snapshot.misses != full_snapshot.misses + 1
        || churn_snapshot.successful_compiles != full_snapshot.successful_compiles + 1
        || churn_snapshot.successful_publications != full_snapshot.successful_publications + 1
    {
        return Err(err(
            "over-capacity dispatch did not replace exactly one FIFO entry",
        ));
    }

    let before_invalidation = engine.fifo_lifecycle_snapshot()?;
    let invalidation_started = Instant::now();
    let invalidated_entries =
        engine.fifo_invalidate_backing(target.identity.backing, changed_version)?;
    let invalidation_elapsed = invalidation_started.elapsed();
    let after_invalidation = engine.fifo_lifecycle_snapshot()?;
    if invalidated_entries != 1
        || after_invalidation.resident_entries_current != ARENA_SLOTS - 1
        || after_invalidation.arena_available_slots_actual != 1
        || after_invalidation.resident_code_bytes_current
            >= before_invalidation.resident_code_bytes_current
        || after_invalidation.resident_logical_metadata_bytes_current
            >= before_invalidation.resident_logical_metadata_bytes_current
    {
        return Err(err(
            "targeted backing invalidation did not reclaim one resident entry",
        ));
    }
    steps.push(LifecycleStep {
        index: steps.len(),
        action: "invalidate_target_backing_between_calls".into(),
        case_id: Some(target.id.clone()),
        code_sha256: Some(od_testkit::sha256_hex(&target.bytes)),
        context_id: Some(original_context.id.get()),
        mapping_id: Some(target.identity.mapping.get()),
        mapping_version: Some(target.identity.mapping_version.get()),
        backing_id: Some(target.identity.backing.get()),
        backing_version: Some(changed_version.get()),
        invalidated_entries: Some(invalidated_entries),
        elapsed_ns: invalidation_elapsed.as_nanos(),
        cache_before: Some(before_invalidation),
        cache_after: Some(after_invalidation),
        execution: None,
    });

    lifecycle_dispatch_step(
        &mut steps,
        &mut engine,
        &changed_case,
        changed_identity,
        original_context,
        "dispatch_changed_code_after_backing_invalidation",
    )?;
    let changed_cold_snapshot = engine.fifo_lifecycle_snapshot()?;
    if changed_cold_snapshot.resident_entries_current != ARENA_SLOTS
        || changed_cold_snapshot.arena_available_slots_actual != 0
        || changed_cold_snapshot.misses != churn_snapshot.misses + 1
        || changed_cold_snapshot.successful_compiles != churn_snapshot.successful_compiles + 1
        || changed_cold_snapshot.successful_publications
            != churn_snapshot.successful_publications + 1
        || changed_cold_snapshot.evictions != churn_snapshot.evictions
    {
        return Err(err(
            "changed backing dispatch did not compile without eviction",
        ));
    }
    lifecycle_dispatch_step(
        &mut steps,
        &mut engine,
        &changed_case,
        changed_identity,
        original_context,
        "repeat_changed_code_hit",
    )?;
    let changed_hit_snapshot = engine.fifo_lifecycle_snapshot()?;
    if changed_hit_snapshot.hits != hit_snapshot.hits + 1
        || changed_hit_snapshot.misses != changed_cold_snapshot.misses
        || changed_hit_snapshot.successful_compiles != changed_cold_snapshot.successful_compiles
        || changed_hit_snapshot.successful_publications
            != changed_cold_snapshot.successful_publications
    {
        return Err(err("changed code repeat was not a hit-only dispatch"));
    }

    lifecycle_dispatch_step(
        &mut steps,
        &mut engine,
        &changed_case,
        changed_identity,
        next_context,
        "advance_context_generation_and_recompile",
    )?;
    let context_snapshot = engine.fifo_lifecycle_snapshot()?;
    if context_snapshot.resident_entries_current != 1
        || context_snapshot.arena_available_slots_actual != ARENA_SLOTS - 1
        || context_snapshot.misses != changed_hit_snapshot.misses + 1
        || context_snapshot.successful_compiles != changed_hit_snapshot.successful_compiles + 1
        || context_snapshot.successful_publications
            != changed_hit_snapshot.successful_publications + 1
        || context_snapshot.evictions != changed_hit_snapshot.evictions
    {
        return Err(err(
            "advancing execution context did not flush then recompile",
        ));
    }

    let before_clear = engine.fifo_lifecycle_snapshot()?;
    let clear_started = Instant::now();
    engine.fifo_clear()?;
    let clear_elapsed = clear_started.elapsed();
    let after_clear = engine.fifo_lifecycle_snapshot()?;
    if after_clear.resident_entries_current != 0
        || after_clear.resident_code_bytes_current != 0
        || after_clear.resident_logical_metadata_bytes_current != 0
        || after_clear.arena_available_slots_actual != ARENA_SLOTS
        || after_clear.resident_entries_high_water != ARENA_SLOTS
    {
        return Err(err(
            "clear did not return all slots and resident accounting to zero",
        ));
    }
    steps.push(LifecycleStep {
        index: steps.len(),
        action: "clear_cache_between_calls".into(),
        case_id: None,
        code_sha256: None,
        context_id: Some(next_context.id.get()),
        mapping_id: None,
        mapping_version: None,
        backing_id: None,
        backing_version: None,
        invalidated_entries: None,
        elapsed_ns: clear_elapsed.as_nanos(),
        cache_before: Some(before_clear),
        cache_after: Some(after_clear),
        execution: None,
    });

    let before_drop = engine.fifo_lifecycle_snapshot()?;
    let teardown_started = Instant::now();
    drop(engine);
    let teardown_elapsed = teardown_started.elapsed();
    steps.push(LifecycleStep {
        index: steps.len(),
        action: "drop_cache_and_arena_mapping_lease".into(),
        case_id: None,
        code_sha256: None,
        context_id: Some(next_context.id.get()),
        mapping_id: None,
        mapping_version: None,
        backing_id: None,
        backing_version: None,
        invalidated_entries: None,
        elapsed_ns: teardown_elapsed.as_nanos(),
        cache_before: Some(before_drop),
        cache_after: None,
        execution: None,
    });

    let reopen_started = Instant::now();
    let mut reopened = Variant::new(VariantName::MultiFifo16)?;
    let reopen_elapsed = reopen_started.elapsed();
    let reopened_before = reopened.fifo_lifecycle_snapshot()?;
    if reopened_before.arena_available_slots_actual != ARENA_SLOTS
        || reopened_before.resident_entries_current != 0
    {
        return Err(err("fresh arena did not reopen with all slots available"));
    }
    steps.push(LifecycleStep {
        index: steps.len(),
        action: "reopen_fresh_cache_and_arena".into(),
        case_id: None,
        code_sha256: None,
        context_id: Some(reopened_context.id.get()),
        mapping_id: None,
        mapping_version: None,
        backing_id: None,
        backing_version: None,
        invalidated_entries: None,
        elapsed_ns: reopen_elapsed.as_nanos(),
        cache_before: None,
        cache_after: Some(reopened_before),
        execution: None,
    });
    lifecycle_dispatch_step(
        &mut steps,
        &mut reopened,
        target,
        target.identity,
        reopened_context,
        "execute_after_reopen_with_new_context",
    )?;
    let reopened_after = reopened.fifo_lifecycle_snapshot()?;
    if reopened_after.resident_entries_current != 1
        || reopened_after.arena_available_slots_actual != ARENA_SLOTS - 1
        || reopened_after.misses != 1
        || reopened_after.successful_compiles != 1
        || reopened_after.successful_publications != 1
        || reopened_after.evictions != 0
    {
        return Err(err(
            "reopened runtime did not execute through a fresh cold entry",
        ));
    }
    drop(reopened);

    let report = CacheLifecycleReport {
        schema: 1,
        process_run,
        case_list_sha256: od_testkit::sha256_hex(&list),
        trace_sha256: od_testkit::sha256_hex(&trace_raw),
        capture_host: trace.capture_host.clone(),
        measure_host: HostMetadata::current(),
        cache_capacity_entries: ARENA_SLOTS,
        initial_fill_case_ids: fillers
            .iter()
            .map(|case| case.id.clone())
            .chain(std::iter::once(target.id.clone()))
            .collect(),
        overflow_case_id: overflow.id.clone(),
        invalidation_target_case_id: target.id.clone(),
        changed_code_case_id: changed_case.id.clone(),
        changed_code_sha256: od_testkit::sha256_hex(&changed_case.bytes),
        original_mapping_id: target.identity.mapping.get(),
        original_backing_id: target.identity.backing.get(),
        original_backing_version: target.identity.backing_version.get(),
        changed_backing_version: changed_version.get(),
        changed_result_differs_under_same_initial_state: true,
        context_generations: vec![
            original_context.id.get(),
            next_context.id.get(),
            reopened_context.id.get(),
        ],
        lifecycle_contract: "One synchronous FIFO16 runtime and one PublishedArenaBackend are active at a time. The exact sequence fills 16 entries, repeats the newest target as a hit, compiles a seventeenth key to evict the oldest filler, invalidates the target backing between dispatch calls, dispatches different admitted bytes under an advanced backing version, confirms the subsequent hit, advances execution-context generation to flush/recompile, clears the cache, drops the runtime/arena, then creates a fresh arena and executes with a new context.".into(),
        limitations: vec![
            "Backing invalidation is called directly between synchronous dispatcher invocations; this is not production owner notification or guest-write SMC.".into(),
            "STOP is not treated as an automatic cache clear; explicit clear and runtime drop are separate recorded steps.".into(),
            "No active-code SMC, guest memory fault, simultaneous arena, SMP, or independent concurrent runtime is exercised.".into(),
            "Recorded nanosecond values describe one bounded lifecycle schedule per process and make no performance or leak-free claim.".into(),
        ],
        steps,
        total_elapsed_ns: started.elapsed().as_nanos(),
    };
    serde_json::to_value(report).map_err(|e| err(e.to_string()))
}

/// Runs a bounded calibration with the exact deterministic S3-37d epoch
/// schedule and returns a fixed epoch count for scored stress processes.
pub fn calibrate_cache_lifecycle_stress(
    case_list: &Path,
    trace_path: &Path,
    output_path: &Path,
) -> Result<serde_json::Value, BenchError> {
    lifecycle_stress::calibrate(case_list, trace_path, output_path)
}

/// Runs the fixed-epoch FIFO16 lifecycle stress, persisting bounded progress
/// checkpoints to the requested output path while the process is active.
pub fn run_cache_lifecycle_stress(
    case_list: &Path,
    trace_path: &Path,
    calibration_path: &Path,
    process_run: u8,
    epochs: usize,
    output_path: &Path,
) -> Result<serde_json::Value, BenchError> {
    lifecycle_stress::run(
        case_list,
        trace_path,
        calibration_path,
        process_run,
        epochs,
        output_path,
    )
}

fn lifecycle_case_with_program(
    template: &Case,
    initial: &LongModeState,
    budget: u64,
    identity: CodeIdentity,
    id: String,
) -> Result<Case, BenchError> {
    let code = CodeView::new(0, &template.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
    if initial.rip != 0 {
        return Err(err("lifecycle versioned case must start at guest RIP zero"));
    }
    let mut expected = initial.clone();
    let mut memory = FetchMemory(&template.bytes);
    let result = interpreter_run(
        &mut expected,
        &mut memory,
        RunConfig {
            instruction_budget: budget,
            stop_rip: Some(code.code_end()),
            context: AccessContext::default(),
        },
        || false,
    );
    if result.instructions_executed > budget {
        return Err(err("lifecycle interpreter exceeded the instruction budget"));
    }
    let exit = match result.exit {
        RunExit::StopAddress => DispatchExit::CodeEnd,
        RunExit::InstructionBudgetExhausted => DispatchExit::BudgetExhausted,
        _ => {
            return Err(err(
                "lifecycle fixture produced an unexpected interpreter exit",
            ));
        }
    };
    let retired = result.instructions_executed;
    let remaining = budget
        .checked_sub(retired)
        .ok_or_else(|| err("lifecycle remaining budget underflow"))?;
    Ok(Case {
        id,
        bytes: template.bytes.clone(),
        admission: template.admission.clone(),
        native_admission: template.native_admission.clone(),
        initial: initial.clone(),
        expected,
        budget,
        exit,
        retired,
        remaining,
        flags_mask: template.flags_mask,
        identity,
        entry_count: template.entry_count,
        unique_entries: template.unique_entries,
    })
}

fn lifecycle_results_differ(left: &Case, right: &Case) -> bool {
    let mut left_state = left.expected.clone();
    let mut right_state = right.expected.clone();
    left_state.rflags &= left.flags_mask;
    right_state.rflags &= right.flags_mask;
    left_state != right_state
        || left.exit != right.exit
        || left.retired != right.retired
        || left.remaining != right.remaining
}

fn lifecycle_dispatch_step(
    steps: &mut Vec<LifecycleStep>,
    engine: &mut Variant,
    case: &Case,
    identity: CodeIdentity,
    context: ExecutionContextSnapshot,
    action: &str,
) -> Result<(), BenchError> {
    let cache_before = engine.fifo_lifecycle_snapshot()?;
    let mut state = case.initial.clone();
    let dispatch_started = Instant::now();
    let result = engine.call_fifo_with_identity_and_context(case, &mut state, identity, context)?;
    let elapsed = dispatch_started.elapsed();
    validate_full_dispatch_state(case, &state)?;
    validate_dispatch_result(case, &state, &result)?;
    let cache_after = engine.fifo_lifecycle_snapshot()?;
    steps.push(LifecycleStep {
        index: steps.len(),
        action: action.into(),
        case_id: Some(case.id.clone()),
        code_sha256: Some(od_testkit::sha256_hex(&case.bytes)),
        context_id: Some(context.id.get()),
        mapping_id: Some(identity.mapping.get()),
        mapping_version: Some(identity.mapping_version.get()),
        backing_id: Some(identity.backing.get()),
        backing_version: Some(identity.backing_version.get()),
        invalidated_entries: None,
        elapsed_ns: elapsed.as_nanos(),
        cache_before: Some(cache_before),
        cache_after: Some(cache_after),
        execution: Some(LifecycleExecution {
            dispatch_exit: format!("{:?}", result.exit),
            blocks_executed: result.blocks_executed,
            retired_guest_instructions: result.retired_instructions,
            remaining_budget: result.remaining_budget,
            flags_mask: case.flags_mask,
            state_debug: format!("{state:?}"),
        }),
    });
    Ok(())
}

fn lifecycle_stop_boundary_step(
    steps: &mut Vec<LifecycleStep>,
    engine: &mut Variant,
    case: &Case,
    context: ExecutionContextSnapshot,
) -> Result<(), BenchError> {
    let cache_before = engine.fifo_lifecycle_snapshot()?;
    let mut state = case.initial.clone();
    let before_state = state.clone();
    let stop_rip = state.rip;
    let dispatch_started = Instant::now();
    let result = engine.call_fifo_controlled_with_identity_and_context(
        case,
        &mut state,
        case.identity,
        context,
        Some(stop_rip),
    )?;
    let elapsed = dispatch_started.elapsed();
    let cache_after = engine.fifo_lifecycle_snapshot()?;
    if state != before_state
        || result.exit != DispatchExit::Runtime(od_jit::runtime::RuntimeExitTag::StopAddress)
        || result.blocks_executed != 0
        || result.retired_instructions != 0
        || result.remaining_budget != case.budget
        || cache_after.hits != cache_before.hits
        || cache_after.misses != cache_before.misses
        || cache_after.successful_compiles != cache_before.successful_compiles
        || cache_after.successful_publications != cache_before.successful_publications
        || cache_after.evictions != cache_before.evictions
        || cache_after.resident_entries_current != cache_before.resident_entries_current
        || cache_after.resident_code_bytes_current != cache_before.resident_code_bytes_current
        || cache_after.resident_logical_metadata_bytes_current
            != cache_before.resident_logical_metadata_bytes_current
        || cache_after.arena_available_slots_actual != cache_before.arena_available_slots_actual
    {
        return Err(err(
            "controlled stop-RIP/StopAddress boundary changed guest state or cache residency/counters",
        ));
    }
    steps.push(LifecycleStep {
        index: steps.len(),
        action:
            "controlled_stop_rip_at_current_guest_boundary_returns_stop_address_no_lookup_or_clear"
                .into(),
        case_id: Some(case.id.clone()),
        code_sha256: Some(od_testkit::sha256_hex(&case.bytes)),
        context_id: Some(context.id.get()),
        mapping_id: Some(case.identity.mapping.get()),
        mapping_version: Some(case.identity.mapping_version.get()),
        backing_id: Some(case.identity.backing.get()),
        backing_version: Some(case.identity.backing_version.get()),
        invalidated_entries: None,
        elapsed_ns: elapsed.as_nanos(),
        cache_before: Some(cache_before),
        cache_after: Some(cache_after),
        execution: Some(LifecycleExecution {
            dispatch_exit: format!("{:?}", result.exit),
            blocks_executed: result.blocks_executed,
            retired_guest_instructions: result.retired_instructions,
            remaining_budget: result.remaining_budget,
            flags_mask: case.flags_mask,
            state_debug: format!("{state:?}"),
        }),
    });
    Ok(())
}

fn churn_counter_delta(after: Counters, before: Counters) -> Result<ChurnCounterDelta, BenchError> {
    Ok(ChurnCounterDelta {
        hits: after
            .hits
            .checked_sub(before.hits)
            .ok_or_else(|| err("cache hit counter decreased"))?,
        misses: after
            .misses
            .checked_sub(before.misses)
            .ok_or_else(|| err("cache miss counter decreased"))?,
        successful_compiles: after
            .compiles
            .checked_sub(before.compiles)
            .ok_or_else(|| err("compile counter decreased"))?,
        successful_publications: after
            .publications
            .checked_sub(before.publications)
            .ok_or_else(|| err("publication counter decreased"))?,
        evictions: after
            .evictions
            .checked_sub(before.evictions)
            .ok_or_else(|| err("eviction counter decreased"))?,
    })
}

fn accumulate_churn_counters(
    total: &mut ChurnCounterDelta,
    delta: ChurnCounterDelta,
) -> Result<(), BenchError> {
    total.hits = total
        .hits
        .checked_add(delta.hits)
        .ok_or_else(|| err("cache-churn hit total overflow"))?;
    total.misses = total
        .misses
        .checked_add(delta.misses)
        .ok_or_else(|| err("cache-churn miss total overflow"))?;
    total.successful_compiles = total
        .successful_compiles
        .checked_add(delta.successful_compiles)
        .ok_or_else(|| err("cache-churn compile total overflow"))?;
    total.successful_publications = total
        .successful_publications
        .checked_add(delta.successful_publications)
        .ok_or_else(|| err("cache-churn publication total overflow"))?;
    total.evictions = total
        .evictions
        .checked_add(delta.evictions)
        .ok_or_else(|| err("cache-churn eviction total overflow"))?;
    Ok(())
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
        if case.native_admission.is_some() {
            validate_full_dispatch_state(case, &state)?;
        }
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

#[derive(Clone, Debug, Serialize)]
struct PhaseSeriesRestart {
    failed_pair: usize,
    attempted_sweeps: usize,
    ordinary_dispatch_ns: u128,
    observed_dispatch_ns: u128,
    next_sweeps: usize,
}

struct PipelineSweep {
    initialization: Duration,
    dispatch_elapsed: Duration,
    calls: u64,
    retired: u64,
    native_blocks: u64,
    counters: Counters,
    phases: Vec<PipelinePhaseTotal>,
    outcomes: Vec<PipelineOutcome>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PipelineOutcome {
    state: LongModeState,
    result: od_jit::dispatch::DispatchResult,
}

fn pipeline_sweep(
    cases: &[Case],
    expected_visits: usize,
    started: Instant,
    observed: bool,
    capture_outcomes: bool,
) -> Result<PipelineSweep, BenchError> {
    let initialization_started = Instant::now();
    let mut engine = Variant::new(VariantName::MultiFifo16)?;
    let initialization = initialization_started.elapsed();
    let mut dispatch_elapsed = Duration::ZERO;
    let mut calls = 0u64;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    let mut recorder = PhaseRecorder::default();
    let mut outcomes = Vec::with_capacity(if capture_outcomes { cases.len() } else { 0 });

    for case in cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err(
                "30 minute run limit exceeded during cold pipeline sweep",
            ));
        }
        let mut state = case.initial.clone();
        let call_started = Instant::now();
        let result = if observed {
            engine.call_observed(case, &mut state, &mut recorder)?
        } else {
            engine.call(case, &mut state)?
        };
        dispatch_elapsed = dispatch_elapsed.saturating_add(call_started.elapsed());
        validate_dispatch_result(case, &state, &result)?;
        validate_full_dispatch_state(case, &state)?;
        if capture_outcomes {
            outcomes.push(PipelineOutcome {
                state: state.clone(),
                result,
            });
        }
        calls = calls
            .checked_add(1)
            .ok_or_else(|| err("cold pipeline call count overflow"))?;
        retired = retired
            .checked_add(result.retired_instructions)
            .ok_or_else(|| err("cold pipeline retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(result.blocks_executed)
            .ok_or_else(|| err("cold pipeline block count overflow"))?;
    }

    let counters = engine.counters();
    let phases = recorder
        .totals()
        .into_iter()
        .map(|total| PipelinePhaseTotal {
            phase: phase_name(total.phase).to_owned(),
            calls: total.calls,
            elapsed_ns: total.elapsed_ns,
        })
        .collect::<Vec<_>>();
    let sweep = PipelineSweep {
        initialization,
        dispatch_elapsed,
        calls,
        retired,
        native_blocks,
        counters,
        phases,
        outcomes,
    };
    if observed {
        validate_phase_counts(&sweep, expected_visits)?;
    } else if sweep
        .phases
        .iter()
        .any(|phase| phase.calls != 0 || phase.elapsed_ns != 0)
    {
        return Err(err(
            "ordinary cold sweep unexpectedly recorded phase timings",
        ));
    }
    drop(engine);
    Ok(sweep)
}

fn validate_phase_counts(sweep: &PipelineSweep, expected_visits: usize) -> Result<(), BenchError> {
    let phase_calls = |name: &str| -> u64 {
        sweep
            .phases
            .iter()
            .find(|phase| phase.phase == name)
            .map_or(0, |phase| phase.calls)
    };
    let visits = u64::try_from(expected_visits).map_err(|_| err("visit count overflow"))?;
    let lookup_calls = sweep.counters.hits.saturating_add(sweep.counters.misses);
    if phase_calls("block_build_and_admission") != visits
        || phase_calls("key_snapshot_and_lookup") != visits
        || lookup_calls != visits
        || phase_calls("ir_lowering") != sweep.counters.compiles
        || phase_calls("allocation") != sweep.counters.compiles
        || phase_calls("emission_and_finalization") != sweep.counters.compiles
        || phase_calls("publication") != sweep.counters.publications
        || phase_calls("host_execution_boundary") != sweep.native_blocks
        || sweep.counters.compiles != sweep.counters.misses
        || sweep.counters.publications != sweep.counters.compiles
    {
        return Err(err(
            "phase event counts do not match cold FIFO16 cache counters",
        ));
    }
    let attributed = sweep
        .phases
        .iter()
        .try_fold(0u128, |sum, phase| sum.checked_add(phase.elapsed_ns))
        .ok_or_else(|| err("phase duration sum overflow"))?;
    if attributed > sweep.dispatch_elapsed.as_nanos() {
        return Err(err(
            "non-overlapping phase total exceeds dispatcher elapsed time",
        ));
    }
    Ok(())
}

fn phase_name(phase: ObservedDispatchPhase) -> &'static str {
    match phase {
        ObservedDispatchPhase::BlockBuildAndAdmission => "block_build_and_admission",
        ObservedDispatchPhase::KeySnapshotAndLookup => "key_snapshot_and_lookup",
        ObservedDispatchPhase::IrLowering => "ir_lowering",
        ObservedDispatchPhase::Allocation => "allocation",
        ObservedDispatchPhase::EmissionAndFinalization => "emission_and_finalization",
        ObservedDispatchPhase::Publication => "publication",
        ObservedDispatchPhase::HostExecutionBoundary => "host_execution_boundary",
    }
}

fn counters_match(left: Counters, right: Counters) -> bool {
    left.cache_lookup_metrics_available == right.cache_lookup_metrics_available
        && left.hits == right.hits
        && left.misses == right.misses
        && left.compiles == right.compiles
        && left.publications == right.publications
        && left.evictions == right.evictions
        && left.resident == right.resident
        && left.code_bytes == right.code_bytes
        && left.logical_bytes == right.logical_bytes
        && left.arena_capacity == right.arena_capacity
        && left.arena_free == right.arena_free
        && left.arena_reserved_bytes == right.arena_reserved_bytes
}

fn measure_pipeline_sample(
    pair: usize,
    order: usize,
    sweeps: usize,
    cases: &[Case],
    expected_visits: usize,
    started: Instant,
    observed: bool,
) -> Result<PipelinePhaseSample, BenchError> {
    let mut initialization_ns = 0u128;
    let mut dispatch_elapsed_ns = 0u128;
    let mut calls = 0u64;
    let mut retired = 0u64;
    let mut native_blocks = 0u64;
    let mut counters = Counters {
        arena_free: usize::MAX,
        ..Counters::default()
    };
    let mut phase_totals: Option<Vec<PipelinePhaseTotal>> = None;
    for _ in 0..sweeps {
        let sweep = pipeline_sweep(cases, expected_visits, started, observed, false)?;
        initialization_ns = initialization_ns.saturating_add(sweep.initialization.as_nanos());
        dispatch_elapsed_ns = dispatch_elapsed_ns.saturating_add(sweep.dispatch_elapsed.as_nanos());
        calls = calls
            .checked_add(sweep.calls)
            .ok_or_else(|| err("sample call count overflow"))?;
        retired = retired
            .checked_add(sweep.retired)
            .ok_or_else(|| err("sample retirement count overflow"))?;
        native_blocks = native_blocks
            .checked_add(sweep.native_blocks)
            .ok_or_else(|| err("sample native block count overflow"))?;
        add_sample_counters(&mut counters, sweep.counters)?;
        match &mut phase_totals {
            Some(totals) => add_phase_totals(totals, sweep.phases)?,
            None => phase_totals = Some(sweep.phases),
        }
    }
    let phase_totals = phase_totals.unwrap_or_default();
    let attributed_phase_ns = phase_totals
        .iter()
        .try_fold(0u128, |sum, phase| sum.checked_add(phase.elapsed_ns))
        .ok_or_else(|| err("sample phase duration sum overflow"))?;
    if attributed_phase_ns > dispatch_elapsed_ns {
        return Err(err("aggregated phase time exceeds dispatcher sample time"));
    }
    let expected_calls = u64::try_from(cases.len())
        .ok()
        .and_then(|count| count.checked_mul(sweeps as u64))
        .ok_or_else(|| err("sample expected-call count overflow"))?;
    if calls != expected_calls {
        return Err(err("ordinary/observed sweep call count mismatch"));
    }
    Ok(PipelinePhaseSample {
        pair,
        order,
        cold_sweeps: sweeps,
        calls,
        retired_guest_instructions: retired,
        native_block_executions: native_blocks,
        initialization_ns,
        dispatch_elapsed_ns,
        attributed_phase_ns,
        other_dispatch_ns: dispatch_elapsed_ns.saturating_sub(attributed_phase_ns),
        cache_hits: counters.hits,
        cache_misses: counters.misses,
        successful_compiles: counters.compiles,
        successful_publications: counters.publications,
        evictions: counters.evictions,
        phase_totals,
    })
}

fn add_phase_totals(
    accumulated: &mut [PipelinePhaseTotal],
    current: Vec<PipelinePhaseTotal>,
) -> Result<(), BenchError> {
    if accumulated.len() != current.len() {
        return Err(err("phase inventory changed within sample"));
    }
    for (accumulated, current) in accumulated.iter_mut().zip(current) {
        if accumulated.phase != current.phase {
            return Err(err("phase ordering changed within sample"));
        }
        accumulated.calls = accumulated
            .calls
            .checked_add(current.calls)
            .ok_or_else(|| err("phase call counter overflow"))?;
        accumulated.elapsed_ns = accumulated
            .elapsed_ns
            .checked_add(current.elapsed_ns)
            .ok_or_else(|| err("phase elapsed counter overflow"))?;
    }
    Ok(())
}

fn median_f64(sorted_values: &[f64]) -> f64 {
    match sorted_values.len() {
        0 => 1.0,
        len if len % 2 == 0 => (sorted_values[len / 2 - 1] + sorted_values[len / 2]) / 2.0,
        len => sorted_values[len / 2],
    }
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
        if case.native_admission.is_some() {
            validate_full_dispatch_state(case, &state)?;
        }
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

fn validate_full_dispatch_state(case: &Case, state: &LongModeState) -> Result<(), BenchError> {
    let mut expected = case.expected.clone();
    let mut actual = state.clone();
    expected.rflags &= case.flags_mask;
    actual.rflags &= case.flags_mask;
    if actual != expected {
        return Err(err(format!(
            "full native CPU state mismatch for {} outside undefined RFLAGS bits",
            case.id
        )));
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

// Prepare all admissions before timing and reject bare NOP, the only current
// NativeRegisterV1 interpreter handoff. This makes helper count zero a
// property of the measured workload, not an extrapolation to S3-55.
fn prepare_generic_admissions(cases: &mut [Case]) -> Result<(), BenchError> {
    for case in cases {
        let view = CodeView::new(0, &case.bytes).map_err(|e| err(format!("code view: {e:?}")))?;
        let admission = NativeRegisterV1Admission::from_code(view)
            .map_err(|e| err(format!("generic admission for {}: {e:?}", case.id)))?;
        let mut memory = FetchMemory(&case.bytes);
        let mut rip = 0u64;
        while rip < view.code_end() {
            let instruction =
                od_isa::long_decode::decode(&mut memory, rip, AccessContext::default())
                    .map_err(|e| err(format!("generic preflight decode for {}: {e:?}", case.id)))?;
            if matches!(instruction.operation, od_isa::long_decode::Operation::Nop) {
                return Err(err(format!(
                    "generic native-only workload contains NOP: {}",
                    case.id
                )));
            }
            rip = rip
                .checked_add(u64::from(instruction.length))
                .ok_or_else(|| err("generic preflight RIP overflow"))?;
            if rip > view.code_end() || instruction.length == 0 {
                return Err(err("generic preflight instruction range invalid"));
            }
        }
        case.native_admission = Some(admission);
    }
    Ok(())
}

fn annotate_generic_report(
    mut value: serde_json::Value,
    generic: bool,
    cases: &[Case],
) -> Result<serde_json::Value, BenchError> {
    if !generic {
        return Ok(value);
    }
    let object = value
        .as_object_mut()
        .ok_or_else(|| err("first-run report is not an object"))?;
    object.insert("generic_register".into(), serde_json::json!({
        "profile": "NativeRegisterV1",
        "flags_mode": "eager",
        "case_ids": cases.iter().map(|case| &case.id).collect::<Vec<_>>(),
        "fallback_helper_calls": 0,
        "fallback_helper_calls_basis": "preflight rejects bare NOP, the only NativeRegisterV1 interpreter handoff; each successful dispatch also verifies retired_interpreter == 0",
        "scope": "native-only S3-14 corpus; does not exercise S3-55 mixed denied-form fallback"
    }));
    let variants = object
        .get_mut("variants")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| err("first-run variants missing"))?;
    for variant in variants {
        let variant = variant
            .as_object_mut()
            .ok_or_else(|| err("first-run variant is not an object"))?;
        let is_interpreter =
            variant.get("variant").and_then(serde_json::Value::as_str) == Some("interpreter");
        let samples = variant
            .get_mut("samples")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| err("first-run samples missing"))?;
        for sample in samples {
            let sample = sample
                .as_object_mut()
                .ok_or_else(|| err("first-run sample is not an object"))?;
            let retired = sample
                .get("retired_guest_instructions")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| err("first-run retirement missing"))?;
            let hits = sample
                .get("cache_hits")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| err("first-run hits missing"))?;
            let misses = sample
                .get("cache_misses")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| err("first-run misses missing"))?;
            let lookups = hits
                .checked_add(misses)
                .ok_or_else(|| err("first-run lookup count overflow"))?;
            sample.insert(
                "retired_native".into(),
                serde_json::json!(if is_interpreter { 0 } else { retired }),
            );
            sample.insert(
                "interpreter_steps".into(),
                serde_json::json!(if is_interpreter { retired } else { 0 }),
            );
            sample.insert("fallback_helper_calls".into(), serde_json::json!(0));
            sample.insert(
                "cache_hit_rate".into(),
                if lookups == 0 {
                    serde_json::Value::Null
                } else {
                    serde_json::json!(hits as f64 / lookups as f64)
                },
            );
        }
    }
    Ok(value)
}

fn prepare_cases(
    list: &[u8],
    trace: &RecordedTrace,
    started: Instant,
) -> Result<Vec<Case>, BenchError> {
    if trace.schema != 2 || trace.inputs.is_empty() || trace.inputs.len() > MAX_INPUTS {
        return Err(err(format!(
            "expected nonempty frozen schema 2 corpus with at most {MAX_INPUTS} inputs"
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
            native_admission: None,
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

pub(crate) struct FetchMemory<'a>(pub(crate) &'a [u8]);
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
            native_admission: None,
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
            native_admission: None,
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
    fn s3_56_generic_cold_and_warm_preserve_outcomes_and_cache_counters() {
        let _guard = NATIVE_TEST_LOCK.lock().unwrap();
        let mut cases = [native_mov_case()];
        prepare_generic_admissions(&mut cases).unwrap();
        let started = Instant::now();
        let cold = cold_fifo_sweep(&cases, started).unwrap();
        assert_eq!(
            (cold.retired, cold.counters.hits, cold.counters.misses),
            (1, 0, 1)
        );
        assert_eq!(
            (
                cold.counters.compiles,
                cold.counters.publications,
                cold.counters.evictions
            ),
            (1, 1, 0)
        );

        let mut engine = Variant::new(VariantName::MultiFifo16).unwrap();
        let warmup = warm_fifo_sweep(&mut engine, &cases, started).unwrap();
        let sample = measure_warm_fifo_sample(0, 0, 2, &cases, &mut engine, started).unwrap();
        assert_eq!(
            (
                warmup.counters.misses,
                warmup.counters.compiles,
                warmup.counters.publications
            ),
            (1, 1, 1)
        );
        assert_eq!(
            (sample.retired_guest_instructions, sample.cache_hits),
            (2, 2)
        );
        assert_eq!(
            (
                sample.cache_misses,
                sample.successful_compiles,
                sample.successful_publications,
                sample.evictions
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn s3_56_native_only_preflight_rejects_interpreter_handoff() {
        let mut case = native_mov_case();
        case.bytes = vec![0x90];
        let error = prepare_generic_admissions(std::slice::from_mut(&mut case)).unwrap_err();
        assert!(error.to_string().contains("contains NOP"));
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
