//! Resident warm eager-dispatch comparison for frozen NOP-fallback inputs.

#[cfg(any(test, all(target_os = "macos", target_arch = "aarch64")))]
use std::collections::BTreeSet;
use std::path::Path;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::sync::atomic::AtomicU8;
use std::time::Duration;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use std::time::Instant;

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_host::PublishedCode;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_isa::LongModeState;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_isa::memory::AccessContext;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_jit::block::CodeView;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use od_jit::dispatch::{MAX_SINGLE_ENTRY_CACHE_BYTES, SingleEntryBlockCache};
use serde::Serialize;

use super::{CacheCounts, SourceFingerprint};
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use super::{
    CacheSnapshot, PreparedCase, cache_behavior, control, counter_delta, counts, dispatch, err,
    load_profile, snapshot, validate_cache_bounds, validate_result, validate_strategy,
};
use crate::HostMetadata;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::MAX_RUN_TIME;

const PROFILE_ID: &str = "s3-36c-resident-warm-native-nop-fallback-v1";
const CORPUS_SHA256: &str = "3fddc5c753f3d2884850468480ec68d13a6069d1aed16e8973e1acef56ffac1f";
const SAMPLE_PAIRS: usize = 20;
const MIN_SAMPLE: Duration = Duration::from_millis(200);
const MAX_REPETITIONS: u64 = 100_000_000;
const MAX_RESTARTS: usize = 8;
const MODULE_SOURCE: &[u8] = include_bytes!("warm_fallback.rs");

const FROZEN_IDENTITIES: [(u64, u32); 19] = [
    (5_995_193_176_011_309_057, 21),
    (5_995_193_176_011_309_057, 54),
    (5_995_193_176_011_309_057, 126),
    (5_995_193_176_011_309_057, 354),
    (5_995_193_176_011_309_058, 54),
    (5_995_193_176_011_309_058, 57),
    (5_995_193_176_011_309_058, 174),
    (5_995_193_176_011_309_058, 177),
    (5_995_193_176_011_309_058, 186),
    (5_995_193_176_011_309_058, 303),
    (5_995_193_176_011_309_058, 336),
    (5_995_193_176_011_309_058, 357),
    (5_995_193_176_011_309_059, 72),
    (5_995_193_176_011_309_059, 114),
    (5_995_193_176_011_309_059, 126),
    (5_995_193_176_011_309_059, 153),
    (5_995_193_176_011_309_059, 168),
    (5_995_193_176_011_309_059, 348),
    (5_995_193_176_011_309_059, 399),
];

#[derive(Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    corpus_path: String,
    corpus_sha256: String,
    inputs_digest: String,
    source_fingerprints: Vec<SourceFingerprint>,
    measure_host: HostMetadata,
    timing_started: bool,
    corpus_cases_validated: usize,
    stored_interpreter_observations_checked: usize,
    selected_case_count: usize,
    selected_cases: Vec<SelectedCase>,
    retirement_strata: Vec<StratumIdentity>,
}

#[derive(Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    corpus_path: String,
    corpus_sha256: String,
    inputs_digest: String,
    source_fingerprints: Vec<SourceFingerprint>,
    measure_host: HostMetadata,
    cargo_version: String,
    timed_scope: &'static str,
    excluded_from_timing: &'static str,
    cache_policy: &'static str,
    sampling_protocol: &'static str,
    case_count: usize,
    retirement_strata: Vec<StratumReport>,
    cases: Vec<WarmCaseReport>,
    total_elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct SelectedCase {
    seed: u64,
    index: u32,
    native_retired: u64,
    interpreter_retired: u64,
    budget: u64,
    input_digest: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
struct StratumIdentity {
    native_retired: u64,
    cases: usize,
}

#[derive(Serialize)]
struct StratumReport {
    native_retired: u64,
    cases: usize,
    median_of_case_ratio_medians: f64,
    case_ratio_iqr: f64,
    minimum_case_ratio_median: f64,
    maximum_case_ratio_median: f64,
}

#[derive(Serialize)]
struct WarmCaseReport {
    seed: u64,
    index: u32,
    input_digest: String,
    native_retired: u64,
    interpreter_retired: u64,
    total_retired: u64,
    budget: u64,
    exit: &'static str,
    eager_repeat_preflight: CacheCounts,
    prewarm_elapsed_ns: u128,
    prewarm_cache_counts: CacheCounts,
    warm_resident: ResidentSnapshot,
    calibration: CalibrationReport,
    measured_pairs: Vec<PairedSample>,
    restarts: Vec<SampleRestart>,
    ratio_summary: RatioSummary,
    timed_cache_counts: CacheCounts,
    final_cache: ResidentSnapshot,
}

#[derive(Clone, Copy, Serialize)]
struct ResidentSnapshot {
    entries: usize,
    code_bytes: usize,
    logical_bytes: usize,
    peak_entries: usize,
    peak_code_bytes: usize,
    peak_logical_bytes: usize,
}

#[derive(Serialize)]
struct CalibrationReport {
    cached_dispatch_elapsed_ns: u128,
    interpreter_elapsed_ns: u128,
    repetitions_per_sample: u64,
}

#[derive(Clone, Serialize)]
struct PairedSample {
    pair: usize,
    first: &'static str,
    repetitions: u64,
    cached_elapsed_ns: u128,
    interpreter_elapsed_ns: u128,
    cached_over_interpreter_ratio: f64,
}

#[derive(Serialize)]
struct SampleRestart {
    attempt: usize,
    trigger_pair: usize,
    shortest_elapsed_ns: u128,
    previous_repetitions: u64,
    next_repetitions: u64,
    discarded_pairs: Vec<PairedSample>,
}

#[derive(Serialize)]
struct RatioSummary {
    median: f64,
    iqr: f64,
    minimum: f64,
    maximum: f64,
}

pub fn preflight(path: &Path) -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = path;
        Err("warm-native-fallback requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    preflight_native(path).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn run(path: &Path, process_run: u8) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (path, process_run);
        Err("warm-native-fallback requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(path, process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_native(path: &Path) -> Result<PreflightReport, super::BenchError> {
    require_release_build()?;
    let (cases, corpus_sha256, inputs_digest) = load_selected(path)?;
    let mut selected = Vec::with_capacity(cases.len());
    let mut stratum_counts = [0usize; 2];
    for mut case in cases {
        case.eager_cache = Some(validate_strategy(&case, false)?);
        let native_retired = validate_eligibility(&case)?;
        stratum_counts[usize::from(native_retired == 3)] += 1;
        selected.push(selected_case(&case));
    }
    if stratum_counts != [10, 9] {
        return Err(err(format!(
            "fallback retirement strata changed: {stratum_counts:?} != [10, 9]"
        )));
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        corpus_path: path.display().to_string(),
        corpus_sha256,
        inputs_digest,
        source_fingerprints: source_fingerprints(),
        measure_host: HostMetadata::current(),
        timing_started: false,
        corpus_cases_validated: super::EXPECTED_CASES,
        stored_interpreter_observations_checked: super::EXPECTED_CASES,
        selected_case_count: selected.len(),
        selected_cases: selected,
        retirement_strata: vec![
            StratumIdentity {
                native_retired: 2,
                cases: stratum_counts[0],
            },
            StratumIdentity {
                native_retired: 3,
                cases: stratum_counts[1],
            },
        ],
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(path: &Path, process_run: u8) -> Result<Report, super::BenchError> {
    require_release_build()?;
    if !(1..=3).contains(&process_run) {
        return Err(err("process-run must be 1..=3"));
    }
    let started = Instant::now();
    let (mut cases, corpus_sha256, inputs_digest) = load_selected(path)?;
    for case in &mut cases {
        case.eager_cache = Some(validate_strategy(case, false)?);
        validate_eligibility(case)?;
    }
    let mut reports = Vec::with_capacity(cases.len());
    for case in &cases {
        if started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded"));
        }
        reports.push(measure_case(case, process_run, started)?);
    }
    let retirement_strata = [2u64, 3]
        .into_iter()
        .map(|native_retired| stratum_report(&reports, native_retired))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        corpus_path: path.display().to_string(),
        corpus_sha256,
        inputs_digest,
        source_fingerprints: source_fingerprints(),
        measure_host: HostMetadata::current(),
        cargo_version: super::command_text("cargo", &["--version"]),
        timed_scope: "sum of intervals immediately around one complete cached eager dispatcher call or one complete interpreter execution, repeated on each identical input; cache lookup and native/fallback dispatch are included",
        excluded_from_timing: "corpus validation, interpreter oracle recomputation, cache construction, one per-case native prewarm, state reset, CodeView/stop setup, result validation, counter checks, report serialization, and dropping the per-case executable mapping",
        cache_policy: "one capacity-one SingleEntryBlockCache is created and prewarmed for one input, retained across calibration and all sample pairs for that input, then dropped before the next input; each timed cached dispatch must add exactly one hit and no miss/compile/publication/replacement",
        sampling_protocol: "20 alternating interpreter/cached pairs per input; one shared calibrated repetition count per input; each retained sample has at least 200ms of summed timed calls on each side; a short pair restarts the full pair series with more repetitions; all states and results are reset/validated outside timed intervals",
        case_count: reports.len(),
        retirement_strata,
        cases: reports,
        total_elapsed_ns: started.elapsed().as_nanos(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn require_release_build() -> Result<(), super::BenchError> {
    if cfg!(debug_assertions) {
        Err(err("warm-native-fallback requires a release build"))
    } else {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn load_selected(path: &Path) -> Result<(Vec<PreparedCase>, String, String), super::BenchError> {
    let (all_cases, corpus_sha256, inputs_digest) = load_profile(path)?;
    if corpus_sha256 != CORPUS_SHA256 {
        return Err(err("corpus bytes differ from the frozen S3-34p corpus"));
    }
    let wanted: BTreeSet<_> = FROZEN_IDENTITIES.into_iter().collect();
    if wanted.len() != FROZEN_IDENTITIES.len() {
        return Err(err("frozen identity list contains duplicates"));
    }
    let selected: Vec<_> = all_cases
        .into_iter()
        .filter(|case| wanted.contains(&(case.input.seed, case.input.index)))
        .collect();
    let found: BTreeSet<_> = selected
        .iter()
        .map(|case| (case.input.seed, case.input.index))
        .collect();
    if found != wanted || selected.len() != FROZEN_IDENTITIES.len() {
        return Err(err(
            "frozen fallback input identities are missing or duplicated",
        ));
    }
    Ok((selected, corpus_sha256, inputs_digest))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn validate_eligibility(case: &PreparedCase) -> Result<u64, super::BenchError> {
    let behavior = cache_behavior(case, false)?;
    let expected_repeat = CacheCounts {
        hits: 1,
        ..CacheCounts::default()
    };
    let native_retired = case.eager_retired_native;
    let fallback_nop_precedes_final_rip = has_nop_at_budget_boundary(
        &case.input.code,
        case.input.guest_base_rip,
        case.expected_state.rip,
    );
    if case.expected_exit != "BudgetExhausted"
        || case.expected_retired != case.input.budget
        || case.eager_retired_interpreter != 1
        || !(native_retired == 2 || native_retired == 3)
        || case.input.budget != native_retired + 1
        || !fallback_nop_precedes_final_rip
        || behavior.repeat != expected_repeat
    {
        return Err(err(format!(
            "case {}:{} does not satisfy the frozen resident NOP-fallback profile: native={native_retired}, interpreter={}, budget={}, exit={}, repeat={:?}",
            case.input.seed,
            case.input.index,
            case.eager_retired_interpreter,
            case.input.budget,
            case.expected_exit,
            behavior.repeat
        )));
    }
    Ok(native_retired)
}

fn has_nop_at_budget_boundary(code: &[u8], base_rip: u64, final_rip: u64) -> bool {
    final_rip
        .checked_sub(base_rip)
        .and_then(|offset| usize::try_from(offset).ok())
        .filter(|offset| *offset > 0 && *offset <= code.len())
        .is_some_and(|offset| code[offset - 1] == 0x90)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn selected_case(case: &PreparedCase) -> SelectedCase {
    SelectedCase {
        seed: case.input.seed,
        index: case.input.index,
        native_retired: case.eager_retired_native,
        interpreter_retired: case.eager_retired_interpreter,
        budget: case.input.budget,
        input_digest: case.input.input_digest.clone(),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_case(
    case: &PreparedCase,
    process_run: u8,
    run_started: Instant,
) -> Result<WarmCaseReport, super::BenchError> {
    let behavior = cache_behavior(case, false)?;
    let mut cache = SingleEntryBlockCache::<PublishedCode>::new(1, MAX_SINGLE_ENTRY_CACHE_BYTES)
        .map_err(|error| err(format!("cache config: {error:?}")))?;
    let view = CodeView::new(case.input.guest_base_rip, &case.input.code)
        .map_err(|error| err(format!("CodeView: {error:?}")))?;
    let mut state = case.input_state();
    let stop = AtomicU8::new(u8::from(case.input.stop_requested));
    let (prewarm_result, prewarm_elapsed_ns) = dispatch(
        case,
        view,
        false,
        &mut state,
        &mut cache,
        control(case, &stop),
    )?;
    validate_result(
        case,
        false,
        &state,
        &prewarm_result.exit,
        prewarm_result.retired_native,
        prewarm_result.retired_interpreter,
        prewarm_result.remaining_budget,
    )?;
    let prewarm_snapshot = snapshot(&cache);
    validate_cache_bounds(prewarm_snapshot, case.input.index)?;
    let prewarm_counts = counts(prewarm_snapshot);
    if prewarm_counts != behavior.cold || prewarm_snapshot.resident_entries != 1 {
        return Err(err(format!(
            "case {} prewarm failed to establish its frozen resident entry: {prewarm_counts:?} != {:?}, resident={}",
            case.input.index, behavior.cold, prewarm_snapshot.resident_entries
        )));
    }
    state.clone_from(&case.input_state());

    let (cached_probe_ns, interpreter_probe_ns) =
        if (case.input.index + u32::from(process_run)) % 2 == 0 {
            let cached = measure_cached_once(case, view, &mut state, &mut cache, run_started)?;
            let interpreter = measure_interpreter_once(case, view, run_started)?;
            (cached, interpreter)
        } else {
            let interpreter = measure_interpreter_once(case, view, run_started)?;
            let cached = measure_cached_once(case, view, &mut state, &mut cache, run_started)?;
            (cached, interpreter)
        };
    let shortest_probe = cached_probe_ns.min(interpreter_probe_ns);
    if shortest_probe == 0 {
        return Err(err("warm calibration produced a zero-duration probe"));
    }
    let repetitions = repetitions_for_target(shortest_probe)?.max(1);
    if repetitions > MAX_REPETITIONS {
        return Err(err("warm calibration exceeds repetition limit"));
    }
    let mut calibration = CalibrationReport {
        cached_dispatch_elapsed_ns: cached_probe_ns,
        interpreter_elapsed_ns: interpreter_probe_ns,
        repetitions_per_sample: repetitions,
    };
    let mut restarts = Vec::new();
    let measured_pairs = collect_pairs(
        case,
        view,
        &mut state,
        &mut cache,
        repetitions,
        &mut restarts,
        run_started,
    )?;
    if let Some(sample) = measured_pairs.first() {
        calibration.repetitions_per_sample = sample.repetitions;
    }
    let ratios: Vec<_> = measured_pairs
        .iter()
        .map(|sample| sample.cached_over_interpreter_ratio)
        .collect();
    let ratio_summary = ratio_summary(&ratios)?;
    let final_snapshot = snapshot(&cache);
    validate_cache_bounds(final_snapshot, case.input.index)?;
    let timed_cache_counts = counter_delta(prewarm_snapshot, final_snapshot)?;
    let expected_timed_hits = 1u64
        .checked_add(
            measured_pairs
                .iter()
                .map(|sample| sample.repetitions)
                .sum::<u64>(),
        )
        .and_then(|hits| {
            restarts.iter().try_fold(hits, |sum, restart| {
                let discarded = restart
                    .discarded_pairs
                    .iter()
                    .map(|sample| sample.repetitions)
                    .sum::<u64>();
                sum.checked_add(discarded)
            })
        })
        .ok_or_else(|| err("warm hit count overflow"))?;
    let expected_counts = CacheCounts {
        hits: expected_timed_hits,
        ..CacheCounts::default()
    };
    if timed_cache_counts != expected_counts {
        return Err(err(format!(
            "case {} warm cache counters differ from expected hits: {timed_cache_counts:?} != {expected_counts:?}",
            case.input.index
        )));
    }
    Ok(WarmCaseReport {
        seed: case.input.seed,
        index: case.input.index,
        input_digest: case.input.input_digest.clone(),
        native_retired: case.eager_retired_native,
        interpreter_retired: case.eager_retired_interpreter,
        total_retired: case.expected_retired,
        budget: case.input.budget,
        exit: case.expected_exit,
        eager_repeat_preflight: behavior.repeat,
        prewarm_elapsed_ns,
        prewarm_cache_counts: prewarm_counts,
        warm_resident: resident_snapshot(prewarm_snapshot),
        calibration,
        measured_pairs,
        restarts,
        ratio_summary,
        timed_cache_counts,
        final_cache: resident_snapshot(final_snapshot),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn collect_pairs(
    case: &PreparedCase,
    view: CodeView<'_>,
    state: &mut LongModeState,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    mut repetitions: u64,
    restarts: &mut Vec<SampleRestart>,
    run_started: Instant,
) -> Result<Vec<PairedSample>, super::BenchError> {
    for attempt in 0..=MAX_RESTARTS {
        let mut pairs = Vec::with_capacity(SAMPLE_PAIRS);
        let mut short = None;
        for pair in 0..SAMPLE_PAIRS {
            if run_started.elapsed() > MAX_RUN_TIME {
                return Err(err("30 minute run limit exceeded during warm samples"));
            }
            let cached_first = pair % 2 == 0;
            let (cached_elapsed_ns, interpreter_elapsed_ns) = if cached_first {
                let cached =
                    measure_cached_batch(case, view, state, cache, repetitions, run_started)?;
                let interpreter = measure_interpreter_batch(case, view, repetitions, run_started)?;
                (cached, interpreter)
            } else {
                let interpreter = measure_interpreter_batch(case, view, repetitions, run_started)?;
                let cached =
                    measure_cached_batch(case, view, state, cache, repetitions, run_started)?;
                (cached, interpreter)
            };
            let ratio = cached_elapsed_ns as f64 / interpreter_elapsed_ns as f64;
            let sample = PairedSample {
                pair,
                first: if cached_first {
                    "cached"
                } else {
                    "interpreter"
                },
                repetitions,
                cached_elapsed_ns,
                interpreter_elapsed_ns,
                cached_over_interpreter_ratio: ratio,
            };
            let shortest = cached_elapsed_ns.min(interpreter_elapsed_ns);
            if shortest < MIN_SAMPLE.as_nanos() {
                short = Some((pair, shortest));
                pairs.push(sample);
                break;
            }
            pairs.push(sample);
        }
        if let Some((trigger_pair, shortest)) = short {
            if attempt == MAX_RESTARTS {
                return Err(err("could not reach 200 ms paired sample floor"));
            }
            let previous_repetitions = repetitions;
            let scaled = u128::from(repetitions)
                .checked_mul(MIN_SAMPLE.as_nanos())
                .ok_or_else(|| err("warm calibration overflow"))?
                .div_ceil(shortest)
                .saturating_mul(6)
                .div_ceil(5);
            repetitions = u64::try_from(scaled)
                .map_err(|_| err("warm calibration overflow"))?
                .max(repetitions.saturating_add(1));
            if repetitions > MAX_REPETITIONS {
                return Err(err("warm sample restart exceeds repetition limit"));
            }
            restarts.push(SampleRestart {
                attempt: attempt + 1,
                trigger_pair,
                shortest_elapsed_ns: shortest,
                previous_repetitions,
                next_repetitions: repetitions,
                discarded_pairs: pairs,
            });
            continue;
        }
        return Ok(pairs);
    }
    Err(err("unreachable warm sample calibration"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_cached_batch(
    case: &PreparedCase,
    view: CodeView<'_>,
    state: &mut LongModeState,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    repetitions: u64,
    run_started: Instant,
) -> Result<u128, super::BenchError> {
    let before = snapshot(cache);
    let mut elapsed = 0u128;
    for _ in 0..repetitions {
        if run_started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during cached batch"));
        }
        elapsed = elapsed
            .checked_add(measure_cached_once(case, view, state, cache, run_started)?)
            .ok_or_else(|| err("cached elapsed time overflow"))?;
    }
    let after = snapshot(cache);
    let delta = counter_delta(before, after)?;
    let expected = CacheCounts {
        hits: repetitions,
        ..CacheCounts::default()
    };
    if delta != expected {
        return Err(err(format!(
            "case {} timed cache delta differs from hit-only expectation: {delta:?} != {expected:?}",
            case.input.index
        )));
    }
    Ok(elapsed)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_cached_once(
    case: &PreparedCase,
    view: CodeView<'_>,
    state: &mut LongModeState,
    cache: &mut SingleEntryBlockCache<PublishedCode>,
    run_started: Instant,
) -> Result<u128, super::BenchError> {
    if run_started.elapsed() > MAX_RUN_TIME {
        return Err(err("30 minute run limit exceeded during cached call"));
    }
    state.clone_from(&case.input_state());
    let stop = AtomicU8::new(u8::from(case.input.stop_requested));
    let before = snapshot(cache);
    let (result, elapsed_ns) = dispatch(case, view, false, state, cache, control(case, &stop))?;
    validate_result(
        case,
        false,
        state,
        &result.exit,
        result.retired_native,
        result.retired_interpreter,
        result.remaining_budget,
    )?;
    let after = snapshot(cache);
    validate_cache_bounds(after, case.input.index)?;
    let delta = counter_delta(before, after)?;
    let expected = CacheCounts {
        hits: 1,
        ..CacheCounts::default()
    };
    if delta != expected {
        return Err(err(format!(
            "case {} cached dispatch did not remain a resident hit: {delta:?}",
            case.input.index
        )));
    }
    Ok(elapsed_ns)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_interpreter_batch(
    case: &PreparedCase,
    view: CodeView<'_>,
    repetitions: u64,
    run_started: Instant,
) -> Result<u128, super::BenchError> {
    let mut elapsed = 0u128;
    for _ in 0..repetitions {
        if run_started.elapsed() > MAX_RUN_TIME {
            return Err(err("30 minute run limit exceeded during interpreter batch"));
        }
        elapsed = elapsed
            .checked_add(measure_interpreter_once(case, view, run_started)?)
            .ok_or_else(|| err("interpreter elapsed time overflow"))?;
    }
    Ok(elapsed)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_interpreter_once(
    case: &PreparedCase,
    view: CodeView<'_>,
    run_started: Instant,
) -> Result<u128, super::BenchError> {
    let mut state = case.input_state();
    let mut memory = super::CodeMemory {
        base: case.input.guest_base_rip,
        bytes: &case.input.code,
    };
    let stop = AtomicU8::new(u8::from(case.input.stop_requested));
    let started = Instant::now();
    let mut retired = 0u64;
    let exit = loop {
        if case.input.stop_rip == Some(state.rip) {
            break "StopAddress";
        }
        if stop.load(std::sync::atomic::Ordering::Acquire) != 0 {
            break "StopRequested";
        }
        if state.rip == view.code_end() {
            break "CodeEnd";
        }
        if retired >= case.input.budget {
            break "BudgetExhausted";
        }
        match od_isa::execute::step(&mut state, &mut memory, AccessContext::default())
            .map_err(|error| err(format!("case {} interpreter: {error:?}", case.input.index)))?
        {
            od_isa::execute::StepOutcome::Advanced | od_isa::execute::StepOutcome::Branched => {
                retired += 1;
            }
            other => {
                return Err(err(format!(
                    "case {} interpreter returned unexpected outcome {other:?}",
                    case.input.index
                )));
            }
        }
    };
    let elapsed_ns = started.elapsed().as_nanos();
    std::hint::black_box(&state);
    if state != case.expected_state
        || exit != case.expected_exit
        || retired != case.expected_retired
        || case.input.budget.saturating_sub(retired)
            != case.input.budget.saturating_sub(case.expected_retired)
    {
        return Err(err(format!(
            "case {} timed interpreter result differs from the recomputed oracle",
            case.input.index
        )));
    }
    if run_started.elapsed() > MAX_RUN_TIME {
        return Err(err("30 minute run limit exceeded"));
    }
    Ok(elapsed_ns)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn resident_snapshot(snapshot: CacheSnapshot) -> ResidentSnapshot {
    ResidentSnapshot {
        entries: snapshot.resident_entries,
        code_bytes: snapshot.resident_code_bytes,
        logical_bytes: snapshot.resident_logical_bytes,
        peak_entries: snapshot.peak_resident_entries,
        peak_code_bytes: snapshot.max_resident_code_bytes,
        peak_logical_bytes: snapshot.max_resident_logical_bytes,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn repetitions_for_target(shortest_probe_ns: u128) -> Result<u64, super::BenchError> {
    if shortest_probe_ns == 0 {
        return Err(err("warm calibration duration is zero"));
    }
    let target_ns = MIN_SAMPLE
        .as_nanos()
        .checked_mul(6)
        .ok_or_else(|| err("warm calibration overflow"))?
        .div_ceil(5);
    u64::try_from(target_ns.div_ceil(shortest_probe_ns))
        .map_err(|_| err("warm calibration repetition count overflow"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn ratio_summary(values: &[f64]) -> Result<RatioSummary, super::BenchError> {
    if values.len() != SAMPLE_PAIRS || values.iter().any(|value| !value.is_finite()) {
        return Err(err("expected 20 finite per-case timing ratios"));
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    Ok(RatioSummary {
        median: sorted[sorted.len() / 2 - 1],
        iqr: sorted[14] - sorted[4],
        minimum: sorted[0],
        maximum: sorted[sorted.len() - 1],
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn stratum_report(
    cases: &[WarmCaseReport],
    native_retired: u64,
) -> Result<StratumReport, super::BenchError> {
    let mut values: Vec<_> = cases
        .iter()
        .filter(|case| case.native_retired == native_retired)
        .map(|case| case.ratio_summary.median)
        .collect();
    if values.is_empty() {
        return Err(err(format!(
            "missing {native_retired}-native fallback stratum"
        )));
    }
    values.sort_by(f64::total_cmp);
    Ok(StratumReport {
        native_retired,
        cases: values.len(),
        median_of_case_ratio_medians: values[(values.len() - 1) / 2],
        case_ratio_iqr: if values.len() > 1 {
            values[(3 * values.len()) / 4] - values[values.len() / 4]
        } else {
            0.0
        },
        minimum_case_ratio_median: values[0],
        maximum_case_ratio_median: values[values.len() - 1],
    })
}

fn source_fingerprints() -> Vec<SourceFingerprint> {
    let mut sources = super::source_fingerprints();
    sources.push(SourceFingerprint {
        path: "tools/bench/src/cached_native_flags_compare/warm_fallback.rs",
        sha256: od_testkit::sha256_hex(MODULE_SOURCE),
    });
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frozen_input_identities_are_unique_and_complete() {
        let identities: BTreeSet<_> = FROZEN_IDENTITIES.into_iter().collect();
        assert_eq!(identities.len(), 19);
        assert_eq!(FROZEN_IDENTITIES.len(), 19);
        assert_eq!(
            FROZEN_IDENTITIES
                .iter()
                .filter(|(seed, _)| *seed == 5_995_193_176_011_309_057)
                .count(),
            4
        );
        assert_eq!(
            FROZEN_IDENTITIES
                .iter()
                .filter(|(seed, _)| *seed == 5_995_193_176_011_309_058)
                .count(),
            8
        );
        assert_eq!(
            FROZEN_IDENTITIES
                .iter()
                .filter(|(seed, _)| *seed == 5_995_193_176_011_309_059)
                .count(),
            7
        );
    }

    #[test]
    fn fallback_nop_is_checked_at_final_rip_not_at_code_end() {
        let mut code = vec![0; 25];
        code[10] = 0x90;
        assert!(has_nop_at_budget_boundary(&code, 0x1000, 0x100b));
        assert!(!has_nop_at_budget_boundary(&code, 0x1000, 0x1000));
        assert!(!has_nop_at_budget_boundary(&code, 0x1000, 0x2000));
        code[10] = 0x91;
        assert!(!has_nop_at_budget_boundary(&code, 0x1000, 0x100b));
    }
}
