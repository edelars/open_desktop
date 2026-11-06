//! Full cached-call comparison for the frozen S3-34a checked-memory subset.
//!
//! This child module deliberately reuses the warm profile's corpus, reset
//! fixtures, oracle and event validation. Each sequence owns one capacity-one
//! cache and mapping; the mapping is dropped before the next sequence starts.

use std::num::NonZeroU64;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::time::Instant;

use od_jit::memory_cache::{
    MemoryExecutionContextGeneration, MemoryPairCache, MemoryPairCacheCounters, MemoryPairProfile,
};
use od_jit::translation::{
    AddressSpaceId, BackingId, BackingVersion, CodeDependencySpan, CodeIdentity, MappingId,
    MappingVersion,
};
use serde::Serialize;

use super::{InterpreterFixture, cold, select_sequence, sequence_name};
use crate::HostMetadata;

const PROFILE_ID: &str = "s3-36e-two-site-memory-full-cache-call-v1";
const PREFLIGHT_TRIALS: usize = 2;
const WARMUP_SWEEPS: u32 = 1;
const SAMPLE_PAIRS: usize = cold::MAX_SAMPLE_PAIRS;
const MAX_REPETITIONS: u32 = cold::MAX_REPETITIONS;
const MIN_SIDE_TIME_NS: u128 = cold::MIN_SIDE_TIME.as_nanos();
const MAX_RUN_TIME_NS: u128 = cold::MAX_RUN_TIME.as_nanos();
const CHECK_EVERY_CALLS: usize = 64;

const CACHE_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/memory_cache.rs");
const HOST_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");
const JIT_MEMORY_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/memory_load_v2.rs");
const JIT_TRANSLATION_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/translation.rs");
const JIT_DISPATCH_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/memory_dispatch.rs");
const JIT_BLOCK_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/block.rs");
const ISA_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/execute.rs");
const ISA_STATE_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/state.rs");
const JIT_RUNTIME_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/runtime.rs");
const TESTKIT_SOURCE: &[u8] = include_bytes!("../../../crates/od-testkit/src/memory_corpus.rs");
const WARM_SOURCE: &[u8] = include_bytes!("two_site_memory_warm.rs");
const COLD_SOURCE: &[u8] = include_bytes!("two_site_memory_cold.rs");
const MODULE_SOURCE: &[u8] = include_bytes!("two_site_memory_cached.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const SOURCE_ENTRIES: [(&str, &[u8]); 15] = [
    ("tools/bench/src/two_site_memory_cached.rs", MODULE_SOURCE),
    ("tools/bench/src/two_site_memory_warm.rs", WARM_SOURCE),
    ("tools/bench/src/two_site_memory_cold.rs", COLD_SOURCE),
    ("tools/bench/src/main.rs", CLI_SOURCE),
    ("tools/bench/src/lib.rs", LIB_SOURCE),
    ("crates/od-jit/src/memory_cache.rs", CACHE_SOURCE),
    ("crates/od-host/src/lib.rs", HOST_SOURCE),
    ("crates/od-jit/src/memory_load_v2.rs", JIT_MEMORY_SOURCE),
    ("crates/od-jit/src/translation.rs", JIT_TRANSLATION_SOURCE),
    ("crates/od-jit/src/memory_dispatch.rs", JIT_DISPATCH_SOURCE),
    ("crates/od-jit/src/block.rs", JIT_BLOCK_SOURCE),
    ("crates/od-jit/src/runtime.rs", JIT_RUNTIME_SOURCE),
    ("crates/od-isa/src/execute.rs", ISA_SOURCE),
    ("crates/od-isa/src/state.rs", ISA_STATE_SOURCE),
    ("crates/od-testkit/src/memory_corpus.rs", TESTKIT_SOURCE),
];

#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    timing_started: bool,
    input_digest: String,
    source_sha256: String,
    source_files: Vec<&'static str>,
    measure_host: HostMetadata,
    unique_cases: usize,
    repetitions_per_case_backend: usize,
    interpreter_executions: usize,
    native_executions: usize,
    parity_trials: usize,
    sequences: Vec<SequencePreflight>,
}

#[derive(Clone, Debug, Serialize)]
struct SequencePreflight {
    sequence: &'static str,
    input_count: usize,
    interpreter_executions: usize,
    native_executions: usize,
    parity_trials: usize,
    counters: CounterSnapshot,
    counters_delta: CounterDelta,
    resident_entries: usize,
    resident_logical_bytes_high_water: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    status: &'static str,
    stop_reason: Option<String>,
    calibration_only: bool,
    timing_started: bool,
    input_digest: String,
    source_sha256: String,
    source_files: Vec<&'static str>,
    journal_path: String,
    measure_host: HostMetadata,
    unique_cases: usize,
    preflight_repetitions_per_case_backend: usize,
    interpreter_preflight_executions: usize,
    native_preflight_executions: usize,
    parity_trials: usize,
    calibration_floor_passed: bool,
    sample_floor_passed: bool,
    minimum_side_time_ns: u128,
    sample_floor_policy: &'static str,
    timer_boundaries: &'static str,
    sequences: Vec<SequenceReport>,
    total_elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct SequenceReport {
    sequence: &'static str,
    input_count: usize,
    setup_preflight_interpreter_executions: usize,
    setup_preflight_native_executions: usize,
    setup_preflight_parity_trials: usize,
    setup_preflight_counters: CounterSnapshot,
    setup_preflight_counter_delta: CounterDelta,
    warmup_sweeps_per_backend: Option<u32>,
    warmup_complete: bool,
    warmup_interpreter_executions: usize,
    warmup_native_executions: usize,
    warmup_native_counter_delta: CounterDelta,
    repetitions_per_side: u32,
    calibration_floor_passed: bool,
    calibration_attempts: Vec<CalibrationAttempt>,
    calibration_counter_delta: CounterDelta,
    sample_pairs: Vec<SamplePair>,
    sample_floor_passed: bool,
    scored_counter_delta: CounterDelta,
    final_counters: CounterSnapshot,
    resident_entries_high_water: usize,
    resident_logical_bytes_high_water: usize,
    cache_drop_ns: u128,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CounterSnapshot {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    replacements: u64,
    resident_entries_high_water: usize,
    resident_logical_bytes_high_water: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
struct CounterDelta {
    hits: u64,
    misses: u64,
    successful_compiles: u64,
    successful_publications: u64,
    replacements: u64,
}

#[derive(Clone, Debug, Serialize)]
struct CalibrationAttempt {
    record_type: &'static str,
    sequence: &'static str,
    attempt_index: usize,
    repetitions: u32,
    native_total_ns: u128,
    interpreter_total_ns: u128,
    native_wall_elapsed_ns: u128,
    interpreter_wall_elapsed_ns: u128,
    interpreter_executions: usize,
    native_executions: usize,
    native_cache_delta: CounterDelta,
    native_floor_met: bool,
    interpreter_floor_met: bool,
}

#[derive(Clone, Debug, Serialize)]
struct SamplePair {
    record_type: &'static str,
    sequence: &'static str,
    pair_index: usize,
    interpreter_first: bool,
    repetitions: u32,
    native_total_ns: u128,
    interpreter_total_ns: u128,
    native_wall_elapsed_ns: u128,
    interpreter_wall_elapsed_ns: u128,
    native_over_interpreter: Option<f64>,
    interpreter_executions: usize,
    native_executions: usize,
    native_cache_delta: CounterDelta,
    native_floor_met: bool,
    interpreter_floor_met: bool,
}

#[derive(Clone, Debug)]
struct SideMeasurement {
    total_ns: u128,
    wall_elapsed_ns: u128,
    executions: usize,
    native_cache_delta: CounterDelta,
}

#[derive(Clone, Debug)]
struct SideFailure {
    reason: &'static str,
    wall_elapsed_ns: u128,
    timed_ns: u128,
    completed_executions: usize,
    native_cache_delta: CounterDelta,
}

#[derive(Clone, Debug, Serialize)]
struct PartialAttempt {
    record_type: &'static str,
    sequence: &'static str,
    phase: &'static str,
    side: &'static str,
    repetitions: u32,
    wall_elapsed_ns: u128,
    completed_timed_calls_ns: u128,
    completed_executions: usize,
    native_cache_delta: CounterDelta,
    completed_other_side: Option<CompletedSide>,
    stop_reason: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct CompletedSide {
    side: &'static str,
    timed_calls_ns: u128,
    wall_elapsed_ns: u128,
    completed_executions: usize,
    native_cache_delta: CounterDelta,
}

#[derive(Clone, Debug, Serialize)]
struct CompletedSideRecord {
    record_type: &'static str,
    sequence: &'static str,
    phase: &'static str,
    repetitions: u32,
    result: CompletedSide,
}

#[derive(Clone, Debug, Serialize)]
struct TerminalRecord {
    record_type: &'static str,
    candidate_status_before_final_sync: &'static str,
    stop_reason: Option<String>,
    elapsed_before_final_sync_ns: u128,
    sequence_count: usize,
}

pub fn preflight(process_run: u8) -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = process_run;
        Err("two-site-memory-cache requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    preflight_native(process_run).map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn run(process_run: u8, journal_path: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    run_profile(process_run, journal_path, false)
}

pub fn calibration_only(
    process_run: u8,
    journal_path: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    run_profile(process_run, journal_path, true)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_native(process_run: u8) -> Result<PreflightReport, cold::BenchError> {
    cold::validate_process_run(process_run)?;
    let (inputs, input_digest) = cold::generate_inputs()?;
    let mut sequences = Vec::with_capacity(2);
    let mut interpreter_executions = 0usize;
    let mut native_executions = 0usize;
    let mut parity_trials = 0usize;
    for sequence in od_testkit::memory_corpus::S3_34A_SEQUENCES {
        let cases = select_sequence(&inputs, sequence)?;
        let mut cache = MemoryPairCache::<od_host::PublishedMemoryPairHandle>::new();
        let mut backend = od_host::CachedPublishedMemoryPairBackend;
        let before = snapshot(cache.counters());
        let (_, per_interpreter, per_native, per_parity) =
            preflight_sequence(sequence, &cases, &mut cache, &mut backend)?;
        let after = snapshot(cache.counters());
        let delta = counter_delta(before, after).map_err(cold::err)?;
        validate_preflight_counters(delta, cases.len() * PREFLIGHT_TRIALS).map_err(cold::err)?;
        let resident_entries = cache.resident_entries();
        let resident_logical_bytes_high_water = after.resident_logical_bytes_high_water;
        cache.clear();
        drop(cache);
        interpreter_executions += per_interpreter;
        native_executions += per_native;
        parity_trials += per_parity;
        sequences.push(SequencePreflight {
            sequence: sequence_name(sequence),
            input_count: cases.len(),
            interpreter_executions: per_interpreter,
            native_executions: per_native,
            parity_trials: per_parity,
            counters: after,
            counters_delta: delta,
            resident_entries,
            resident_logical_bytes_high_water,
        });
    }
    let expected_per_backend = 2 * 3 * 400 * PREFLIGHT_TRIALS;
    if interpreter_executions != expected_per_backend
        || native_executions != expected_per_backend
        || parity_trials != expected_per_backend
    {
        return Err(cold::err(
            "cached preflight execution count differs from 2x3x400x2",
        ));
    }
    Ok(PreflightReport {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        timing_started: false,
        input_digest,
        source_sha256: source_sha256(),
        source_files: source_files().to_vec(),
        measure_host: HostMetadata::current(),
        unique_cases: 2 * 3 * 400,
        repetitions_per_case_backend: PREFLIGHT_TRIALS,
        interpreter_executions,
        native_executions,
        parity_trials,
        sequences,
    })
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn run_profile(
    process_run: u8,
    journal_path: &Path,
    calibration_only: bool,
) -> Result<Report, Box<dyn std::error::Error>> {
    let _ = (process_run, journal_path, calibration_only);
    Err("two-site-memory-cache requires native Apple Silicon macOS execution".into())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_profile(
    process_run: u8,
    journal_path: &Path,
    calibration_only: bool,
) -> Result<Report, Box<dyn std::error::Error>> {
    use std::fs::OpenOptions;
    use std::io::Write;

    cold::validate_process_run(process_run)?;
    let started = Instant::now();
    let (inputs, input_digest) = cold::generate_inputs()?;
    if let Some(parent) = journal_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(journal_path)?;
    let mut journal = std::io::BufWriter::new(file);
    let mut sequence_reports = Vec::with_capacity(2);
    let mut stop_reason = None::<String>;

    for sequence in od_testkit::memory_corpus::S3_34A_SEQUENCES {
        if let Err(error) = check_deadline(started) {
            stop_reason = Some(error);
            break;
        }
        let cases = select_sequence(&inputs, sequence)?;
        let mut cache = MemoryPairCache::<od_host::PublishedMemoryPairHandle>::new();
        let mut backend = od_host::CachedPublishedMemoryPairBackend;
        let preflight_start = snapshot(cache.counters());
        let (oracles, interpreter_executions, native_executions, parity_trials) =
            preflight_sequence(sequence, &cases, &mut cache, &mut backend)?;
        let preflight_end = snapshot(cache.counters());
        let preflight_delta = counter_delta(preflight_start, preflight_end).map_err(cold::err)?;
        validate_preflight_counters(preflight_delta, cases.len() * PREFLIGHT_TRIALS)
            .map_err(cold::err)?;

        let (warmup_interpreter, warmup_native, warmup_native_delta, warmup_complete, warmup_stop) =
            warmup_sequence(
                sequence,
                &cases,
                &oracles,
                &mut cache,
                &mut backend,
                started,
                &mut journal,
            )?;
        let mut sequence_stop = warmup_stop;
        let (
            repetitions,
            calibration_attempts,
            calibration_floor_passed,
            calibration_delta,
            calibration_stop,
        ) = if sequence_stop.is_none() {
            calibrate_sequence(
                sequence,
                &cases,
                &oracles,
                &mut cache,
                &mut backend,
                started,
                &mut journal,
                process_run,
            )?
        } else {
            (
                0,
                Vec::new(),
                false,
                CounterDelta::default(),
                sequence_stop.clone(),
            )
        };
        if sequence_stop.is_none() {
            sequence_stop = calibration_stop;
        }

        let mut sample_pairs = Vec::with_capacity(if calibration_only { 0 } else { SAMPLE_PAIRS });
        let mut scored_delta = CounterDelta::default();
        let mut sample_floor_passed = !calibration_only && calibration_floor_passed;
        if !calibration_only && calibration_floor_passed && sequence_stop.is_none() {
            for pair_index in 0..SAMPLE_PAIRS {
                if let Err(reason) = check_deadline(started) {
                    sequence_stop = Some(reason);
                    sample_floor_passed = false;
                    break;
                }
                let interpreter_first = (pair_index + usize::from(process_run % 2 == 0)) % 2 == 0;
                let mut native = None;
                let mut interpreter = None;
                let mut failure = None;
                let mut failed_side = "";
                if interpreter_first {
                    match measure_interpreter(&cases, &oracles, repetitions, started) {
                        Ok(value) => interpreter = Some(value),
                        Err(value) => {
                            failed_side = "interpreter";
                            failure = Some(value);
                        }
                    }
                    if failure.is_none() {
                        match measure_cached(
                            sequence,
                            &cases,
                            &oracles,
                            &mut cache,
                            &mut backend,
                            repetitions,
                            started,
                        ) {
                            Ok(value) => {
                                add_delta(&mut scored_delta, value.native_cache_delta)
                                    .map_err(cold::err)?;
                                journal_completed_cached_side(
                                    &mut journal,
                                    sequence,
                                    "scored",
                                    repetitions,
                                    &value,
                                )?;
                                native = Some(value);
                            }
                            Err(value) => {
                                add_delta(&mut scored_delta, value.native_cache_delta)
                                    .map_err(cold::err)?;
                                failed_side = "cached";
                                failure = Some(value);
                            }
                        }
                    }
                } else {
                    match measure_cached(
                        sequence,
                        &cases,
                        &oracles,
                        &mut cache,
                        &mut backend,
                        repetitions,
                        started,
                    ) {
                        Ok(value) => {
                            add_delta(&mut scored_delta, value.native_cache_delta)
                                .map_err(cold::err)?;
                            journal_completed_cached_side(
                                &mut journal,
                                sequence,
                                "scored",
                                repetitions,
                                &value,
                            )?;
                            native = Some(value);
                        }
                        Err(value) => {
                            add_delta(&mut scored_delta, value.native_cache_delta)
                                .map_err(cold::err)?;
                            failed_side = "cached";
                            failure = Some(value);
                        }
                    }
                    if failure.is_none() {
                        match measure_interpreter(&cases, &oracles, repetitions, started) {
                            Ok(value) => interpreter = Some(value),
                            Err(value) => {
                                failed_side = "interpreter";
                                failure = Some(value);
                            }
                        }
                    }
                }
                if let Some(failure) = failure {
                    sequence_stop = Some(failure.reason.to_owned());
                    sample_floor_passed = false;
                    let completed_other = if failed_side == "cached" {
                        interpreter
                            .as_ref()
                            .map(|side| completed_side("interpreter", side))
                    } else {
                        native.as_ref().map(|side| completed_side("cached", side))
                    };
                    write_partial(
                        &mut journal,
                        sequence,
                        "scored",
                        failed_side,
                        repetitions,
                        failure,
                        completed_other,
                    )?;
                    break;
                }
                let (native, interpreter) = (
                    native.expect("completed cached side"),
                    interpreter.expect("completed interpreter side"),
                );
                let expected_executions = cases.len() * repetitions as usize;
                if native.executions != expected_executions
                    || interpreter.executions != expected_executions
                    || !validate_hit_only_sweeps(
                        native.native_cache_delta,
                        cases.len(),
                        repetitions,
                    )
                {
                    return Err(Box::new(cold::err(
                        "scored cache counter delta is not hit-only",
                    )));
                }
                let native_floor_met = native.total_ns >= MIN_SIDE_TIME_NS;
                let interpreter_floor_met = interpreter.total_ns >= MIN_SIDE_TIME_NS;
                sample_floor_passed &= native_floor_met && interpreter_floor_met;
                let pair = SamplePair {
                    record_type: "sample_pair",
                    sequence: sequence_name(sequence),
                    pair_index,
                    interpreter_first,
                    repetitions,
                    native_total_ns: native.total_ns,
                    interpreter_total_ns: interpreter.total_ns,
                    native_wall_elapsed_ns: native.wall_elapsed_ns,
                    interpreter_wall_elapsed_ns: interpreter.wall_elapsed_ns,
                    native_over_interpreter: (interpreter.total_ns != 0)
                        .then(|| native.total_ns as f64 / interpreter.total_ns as f64),
                    interpreter_executions: interpreter.executions,
                    native_executions: native.executions,
                    native_cache_delta: native.native_cache_delta,
                    native_floor_met,
                    interpreter_floor_met,
                };
                cold::append_durable_journal_record(&mut journal, &pair)?;
                sample_pairs.push(pair);
                if let Err(reason) = check_deadline(started) {
                    sequence_stop = Some(reason);
                    sample_floor_passed = false;
                    break;
                }
            }
        }

        let final_counters = snapshot(cache.counters());
        let resident_entries_high_water = final_counters.resident_entries_high_water;
        let resident_logical_bytes_high_water = final_counters.resident_logical_bytes_high_water;
        let cache_drop_start = Instant::now();
        cache.clear();
        drop(cache);
        let cache_drop_ns = cache_drop_start.elapsed().as_nanos();
        let (warmup_sweeps, warmup_complete) = warmup_summary(
            warmup_interpreter,
            warmup_native,
            cases.len(),
            warmup_complete,
        );
        if stop_reason.is_none() {
            stop_reason = sequence_stop.clone();
        }
        sequence_reports.push(SequenceReport {
            sequence: sequence_name(sequence),
            input_count: cases.len(),
            setup_preflight_interpreter_executions: interpreter_executions,
            setup_preflight_native_executions: native_executions,
            setup_preflight_parity_trials: parity_trials,
            setup_preflight_counters: preflight_end,
            setup_preflight_counter_delta: preflight_delta,
            warmup_sweeps_per_backend: warmup_sweeps,
            warmup_complete,
            warmup_interpreter_executions: warmup_interpreter,
            warmup_native_executions: warmup_native,
            warmup_native_counter_delta: warmup_native_delta,
            repetitions_per_side: repetitions,
            calibration_floor_passed,
            calibration_attempts,
            calibration_counter_delta: calibration_delta,
            sample_pairs,
            sample_floor_passed,
            scored_counter_delta: scored_delta,
            final_counters,
            resident_entries_high_water,
            resident_logical_bytes_high_water,
            cache_drop_ns,
        });
        if stop_reason.is_some() {
            break;
        }
    }

    let measure_host = HostMetadata::current();
    let source_sha256 = source_sha256();
    journal.flush()?;
    journal.get_ref().sync_data()?;
    drop(inputs);
    let calibration_floor_passed = sequence_reports.len() == 2
        && sequence_reports
            .iter()
            .all(|sequence| sequence.calibration_floor_passed);
    let sample_floor_passed = !calibration_only
        && sequence_reports.len() == 2
        && sequence_reports.iter().all(|sequence| {
            sequence.sample_pairs.len() == SAMPLE_PAIRS && sequence.sample_floor_passed
        });
    let (candidate_status, candidate_stop_reason) = classify_status(
        calibration_only,
        calibration_floor_passed,
        sample_floor_passed,
        stop_reason.clone(),
        started.elapsed().as_nanos(),
    );
    let terminal = TerminalRecord {
        record_type: "terminal_candidate",
        candidate_status_before_final_sync: candidate_status,
        stop_reason: candidate_stop_reason,
        elapsed_before_final_sync_ns: started.elapsed().as_nanos(),
        sequence_count: sequence_reports.len(),
    };
    cold::append_durable_journal_record(&mut journal, &terminal)?;
    drop(journal);
    let total_elapsed_ns = started.elapsed().as_nanos();
    let (status, final_stop_reason, sample_floor_passed) = classify_final_status(
        calibration_only,
        calibration_floor_passed,
        sample_floor_passed,
        stop_reason,
        total_elapsed_ns,
    );
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        status,
        stop_reason: final_stop_reason,
        calibration_only,
        timing_started: true,
        input_digest,
        source_sha256,
        source_files: source_files().to_vec(),
        journal_path: journal_path.display().to_string(),
        measure_host,
        unique_cases: 2 * 3 * 400,
        preflight_repetitions_per_case_backend: PREFLIGHT_TRIALS,
        interpreter_preflight_executions: sequence_reports
            .iter()
            .map(|sequence| sequence.setup_preflight_interpreter_executions)
            .sum(),
        native_preflight_executions: sequence_reports
            .iter()
            .map(|sequence| sequence.setup_preflight_native_executions)
            .sum(),
        parity_trials: sequence_reports
            .iter()
            .map(|sequence| sequence.setup_preflight_parity_trials)
            .sum(),
        calibration_floor_passed,
        sample_floor_passed,
        minimum_side_time_ns: MIN_SIDE_TIME_NS,
        sample_floor_policy: "Retain all 20 pairs independently per sequence. Each side must meet 200ms; any short or incomplete side makes the full process diagnostic-only. No trimming or cross-sequence aggregate.",
        timer_boundaries: "Each native timed call is one complete MemoryPairCache::run, including profile/context/source/dependency/key validation, cache lookup and counters, and typed host execution. State/RAM/callback reset, counter snapshots and per-result full state/RAM/ordered event/exit validation are outside each call timer. One resident capacity-one cache/mapping is retained per sequence, then cleared/dropped before publishing the next sequence.",
        sequences: sequence_reports,
        total_elapsed_ns,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn preflight_sequence(
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    cases: &[&cold::GeneratedCase],
    cache: &mut MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut od_host::CachedPublishedMemoryPairBackend,
) -> Result<(Vec<cold::Outcome>, usize, usize, usize), cold::BenchError> {
    if cases.len() != 1200 {
        return Err(cold::err("cached sequence must contain exactly 1200 cases"));
    }
    let profile = memory_profile(sequence);
    let mut oracles = Vec::with_capacity(cases.len());
    let mut interpreter_executions = 0usize;
    let mut native_executions = 0usize;
    let mut parity_trials = 0usize;
    for case in cases {
        let mut first_interpreter = None;
        let mut first_native = None;
        for trial in 0..PREFLIGHT_TRIALS {
            let bytes = case.input.sequence.bytes();
            let mut interpreter = InterpreterFixture::new(case, &bytes);
            let (interpreter_outcome, _) = interpreter.execute();
            interpreter_executions += 1;
            let mut native = CachedFixture::new(case, &bytes);
            let (native_outcome, _) = native.execute(profile, cache, backend)?;
            native_executions += 1;
            if !cached_pair_matches(&interpreter_outcome, &native_outcome, &case.input) {
                return Err(cold::err(format!(
                    "cached full-state/RAM/event/exit parity failed for {sequence:?}, seed={}, index={}: interpreter={interpreter_outcome:?}, cache={native_outcome:?}",
                    case.seed, case.index
                )));
            }
            parity_trials += 1;
            if trial == 0 {
                first_interpreter = Some(interpreter_outcome);
                first_native = Some(native_outcome);
            } else {
                let expected_interpreter = first_interpreter
                    .as_ref()
                    .expect("first interpreter trial retained");
                let expected_native = first_native.as_ref().expect("first native trial retained");
                if !cold::repeat_outcomes_equal(expected_interpreter, &interpreter_outcome)
                    || !cold::repeat_outcomes_equal(expected_native, &native_outcome)
                {
                    return Err(cold::err(format!(
                        "cached preflight repeat mismatch for {:?}, seed={}, index={}",
                        sequence, case.seed, case.index
                    )));
                }
            }
        }
        oracles.push(first_interpreter.expect("two interpreter trials"));
    }
    let expected = cases.len() * PREFLIGHT_TRIALS;
    if interpreter_executions != expected
        || native_executions != expected
        || parity_trials != expected
    {
        return Err(cold::err("cached per-sequence preflight counts mismatch"));
    }
    Ok((
        oracles,
        interpreter_executions,
        native_executions,
        parity_trials,
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
struct CachedFixture<'a> {
    input: &'a cold::GeneratedCase,
    state: od_isa::LongModeState,
    memory: cold::FixtureMemory<'a>,
    stop_requested: AtomicU8,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl<'a> CachedFixture<'a> {
    fn new(input: &'a cold::GeneratedCase, code: &'a [u8; 6]) -> Self {
        Self {
            input,
            state: cold::make_state(&input.input),
            memory: cold::FixtureMemory {
                code,
                ram: input.input.ram,
                memory_events: [None; 2],
                memory_event_count: 0,
            },
            stop_requested: AtomicU8::new(0),
        }
    }

    fn execute(
        &mut self,
        profile: MemoryPairProfile,
        cache: &mut MemoryPairCache<od_host::PublishedMemoryPairHandle>,
        backend: &mut od_host::CachedPublishedMemoryPairBackend,
    ) -> Result<(cold::Outcome, u128), cold::BenchError> {
        self.state = cold::make_state(&self.input.input);
        self.memory.ram = self.input.input.ram;
        self.memory.memory_events = [None; 2];
        self.memory.memory_event_count = 0;
        self.stop_requested
            .store(0, std::sync::atomic::Ordering::Relaxed);
        let bytes = self.input.input.sequence.bytes();
        let code =
            od_jit::block::CodeView::new(cold::ENTRY_RIP, &bytes).map_err(cold::bench_error)?;
        let dependencies = [CodeDependencySpan {
            start_rip: cold::ENTRY_RIP,
            end_rip: cold::ENTRY_RIP + 6,
            identity: CodeIdentity {
                address_space: AddressSpaceId::new(NonZeroU64::new(1).unwrap()),
                mapping: MappingId::new(NonZeroU64::new(1).unwrap()),
                mapping_version: MappingVersion::new(NonZeroU64::new(1).unwrap()),
                backing: BackingId::new(NonZeroU64::new(1).unwrap()),
                backing_version: BackingVersion::new(NonZeroU64::new(1).unwrap()),
                executable: true,
            },
        }];
        let context_generation = MemoryExecutionContextGeneration::from_u64(1)
            .ok_or_else(|| cold::err("cache context generation must be nonzero"))?;
        let start = Instant::now();
        // SAFETY: the fixture's immutable six-byte code and private synchronous
        // ordinary RAM are disjoint; no MMIO, aliasing, DMA, concurrency,
        // re-entry or unwind is possible in this benchmark call.
        let result = unsafe {
            cache.run(
                profile,
                context_generation,
                code,
                &dependencies,
                &mut self.state,
                &mut self.memory,
                od_isa::memory::AccessContext::default(),
                cold::INSTRUCTION_BUDGET,
                None,
                &self.stop_requested,
                None,
                backend,
            )
        }
        .map_err(cold::bench_error)?;
        let elapsed_ns = start.elapsed().as_nanos();
        std::hint::black_box((&self.state, &self.memory.ram, &result));
        Ok((
            cold::Outcome {
                state: self.state.clone(),
                ram: self.memory.ram,
                exit: format!("{:?}", result.exit),
                retired: result.retired_instructions,
                remaining_budget: result.remaining_budget,
                memory_events: self.memory.memory_events,
                memory_event_count: self.memory.memory_event_count,
            },
            elapsed_ns,
        ))
    }
}

fn memory_profile(sequence: od_testkit::memory_corpus::MemoryCorpusSequence) -> MemoryPairProfile {
    use od_testkit::memory_corpus::MemoryCorpusSequence;
    match sequence {
        MemoryCorpusSequence::StoreThenLoad => MemoryPairProfile::StoreThenLoad,
        MemoryCorpusSequence::LoadThenStore => MemoryPairProfile::LoadThenStore,
        MemoryCorpusSequence::LoadThenLoad => unreachable!("third sequence is excluded"),
        MemoryCorpusSequence::StoreThenStore => {
            unreachable!("S3-26f is excluded from this benchmark")
        }
    }
}

fn snapshot(counters: MemoryPairCacheCounters) -> CounterSnapshot {
    CounterSnapshot {
        hits: counters.hits,
        misses: counters.misses,
        successful_compiles: counters.successful_compiles,
        successful_publications: counters.successful_publications,
        replacements: counters.replacements,
        resident_entries_high_water: counters.resident_entries_high_water,
        resident_logical_bytes_high_water: counters.resident_logical_bytes_high_water,
    }
}

fn counter_delta(before: CounterSnapshot, after: CounterSnapshot) -> Result<CounterDelta, String> {
    Ok(CounterDelta {
        hits: after
            .hits
            .checked_sub(before.hits)
            .ok_or("cache hit counter regressed")?,
        misses: after
            .misses
            .checked_sub(before.misses)
            .ok_or("cache miss counter regressed")?,
        successful_compiles: after
            .successful_compiles
            .checked_sub(before.successful_compiles)
            .ok_or("cache compile counter regressed")?,
        successful_publications: after
            .successful_publications
            .checked_sub(before.successful_publications)
            .ok_or("cache publication counter regressed")?,
        replacements: after
            .replacements
            .checked_sub(before.replacements)
            .ok_or("cache replacement counter regressed")?,
    })
}

fn add_delta(total: &mut CounterDelta, addition: CounterDelta) -> Result<(), String> {
    total.hits = total
        .hits
        .checked_add(addition.hits)
        .ok_or("cache delta overflow")?;
    total.misses = total
        .misses
        .checked_add(addition.misses)
        .ok_or("cache delta overflow")?;
    total.successful_compiles = total
        .successful_compiles
        .checked_add(addition.successful_compiles)
        .ok_or("cache delta overflow")?;
    total.successful_publications = total
        .successful_publications
        .checked_add(addition.successful_publications)
        .ok_or("cache delta overflow")?;
    total.replacements = total
        .replacements
        .checked_add(addition.replacements)
        .ok_or("cache delta overflow")?;
    Ok(())
}

fn completed_side(side: &'static str, measurement: &SideMeasurement) -> CompletedSide {
    CompletedSide {
        side,
        timed_calls_ns: measurement.total_ns,
        wall_elapsed_ns: measurement.wall_elapsed_ns,
        completed_executions: measurement.executions,
        native_cache_delta: measurement.native_cache_delta,
    }
}

fn partial_attempt_record(
    sequence: &'static str,
    phase: &'static str,
    side: &'static str,
    repetitions: u32,
    failure: &SideFailure,
    completed_other_side: Option<CompletedSide>,
) -> PartialAttempt {
    PartialAttempt {
        record_type: "partial_measurement_attempt",
        sequence,
        phase,
        side,
        repetitions,
        wall_elapsed_ns: failure.wall_elapsed_ns,
        completed_timed_calls_ns: failure.timed_ns,
        completed_executions: failure.completed_executions,
        native_cache_delta: failure.native_cache_delta,
        completed_other_side,
        stop_reason: failure.reason,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn journal_completed_cached_side(
    journal: &mut std::io::BufWriter<std::fs::File>,
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    phase: &'static str,
    repetitions: u32,
    measurement: &SideMeasurement,
) -> Result<(), cold::BenchError> {
    let record = CompletedSideRecord {
        record_type: "completed_cached_side",
        sequence: sequence_name(sequence),
        phase,
        repetitions,
        result: completed_side("cached", measurement),
    };
    cold::append_durable_journal_record(journal, &record)
}

fn only_hits(delta: CounterDelta) -> bool {
    delta.misses == 0
        && delta.successful_compiles == 0
        && delta.successful_publications == 0
        && delta.replacements == 0
}

fn validate_hit_only_sweeps(delta: CounterDelta, cases_per_sweep: usize, repetitions: u32) -> bool {
    let expected_hits = cases_per_sweep.checked_mul(repetitions as usize);
    expected_hits.is_some_and(|expected| delta.hits == expected as u64 && only_hits(delta))
}

fn validate_preflight_counters(delta: CounterDelta, executions: usize) -> Result<(), String> {
    if executions == 0
        || delta.hits as usize != executions - 1
        || delta.misses != 1
        || delta.successful_compiles != 1
        || delta.successful_publications != 1
        || delta.replacements != 0
    {
        return Err(format!(
            "setup/preflight cache counters mismatch: {delta:?}, executions={executions}"
        ));
    }
    Ok(())
}

fn cached_pair_matches(
    expected: &cold::Outcome,
    actual: &cold::Outcome,
    input: &od_testkit::memory_corpus::MemoryCorpusInput,
) -> bool {
    let code_end = cold::ENTRY_RIP + 6;
    let Some(expected_events) =
        cold::expected_memory_events(input.sequence, input.gpr[3], input.gpr[2])
    else {
        return false;
    };
    let interpreter_budget_end = expected.exit == "InstructionBudgetExhausted"
        && expected.state.rip == code_end
        && expected.retired == 2
        && expected.remaining_budget == 0;
    let full_parity = expected.state == actual.state
        && expected.ram == actual.ram
        && expected.retired == actual.retired
        && expected.remaining_budget == actual.remaining_budget
        && expected.memory_event_count == 2
        && actual.memory_event_count == 2
        && expected.memory_events == expected_events
        && actual.memory_events == expected_events
        && actual.state.rip == code_end
        && actual.retired == 2
        && actual.remaining_budget == 0;
    let raw_boundary_exit = format!(
        "{:?}",
        od_jit::memory_dispatch::BoundedMemoryExit::BoundaryReached { rip: code_end }
    );
    let raw_exit_matches = actual.exit == "Complete"
        || actual.exit == "BudgetExhausted"
        || actual.exit == raw_boundary_exit;
    interpreter_budget_end && full_parity && raw_exit_matches
}

fn warmup_summary(
    interpreter_executions: usize,
    native_executions: usize,
    expected: usize,
    complete: bool,
) -> (Option<u32>, bool) {
    let complete = complete && interpreter_executions == expected && native_executions == expected;
    (complete.then_some(WARMUP_SWEEPS), complete)
}

fn classify_status(
    calibration_only: bool,
    calibration_passed: bool,
    sample_passed: bool,
    mut stop_reason: Option<String>,
    total_elapsed_ns: u128,
) -> (&'static str, Option<String>) {
    if total_elapsed_ns >= MAX_RUN_TIME_NS {
        stop_reason = Some("overall_wall_time_ceiling_after_final_sync".into());
    }
    let status = if stop_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("overall_wall_time_ceiling"))
    {
        "diagnostic_only_overall_deadline"
    } else if stop_reason.is_some() {
        "diagnostic_only_incomplete"
    } else if !calibration_passed {
        "diagnostic_only_calibration_floor_failed"
    } else if calibration_only {
        "calibration_only_unscored"
    } else if sample_passed {
        "scored"
    } else {
        "diagnostic_only_sample_floor_failed"
    };
    (status, stop_reason)
}

fn classify_final_status(
    calibration_only: bool,
    calibration_passed: bool,
    sample_passed: bool,
    stop_reason: Option<String>,
    total_elapsed_ns: u128,
) -> (&'static str, Option<String>, bool) {
    let (status, stop_reason) = classify_status(
        calibration_only,
        calibration_passed,
        sample_passed,
        stop_reason,
        total_elapsed_ns,
    );
    (status, stop_reason, sample_passed && status == "scored")
}

fn source_sha256() -> String {
    let sources = SOURCE_ENTRIES;
    let mut bytes = Vec::new();
    for (path, source) in sources {
        bytes.extend_from_slice(&(path.len() as u64).to_le_bytes());
        bytes.extend_from_slice(path.as_bytes());
        bytes.extend_from_slice(&(source.len() as u64).to_le_bytes());
        bytes.extend_from_slice(source);
    }
    od_testkit::sha256_hex(&bytes)
}

fn source_files() -> Vec<&'static str> {
    SOURCE_ENTRIES.iter().map(|(path, _)| *path).collect()
}

#[cfg(test)]
mod tests {
    use super::{
        CounterDelta, MAX_REPETITIONS, MAX_RUN_TIME_NS, PREFLIGHT_TRIALS, SAMPLE_PAIRS,
        cached_pair_matches, classify_status, only_hits, source_files, validate_hit_only_sweeps,
        validate_preflight_counters, warmup_summary,
    };
    use crate::two_site_memory_cold::{ENTRY_RIP, MemoryEvent, MemoryOperation, Outcome};
    use od_testkit::memory_corpus::{
        MEMORY_CORPUS_RAM_LEN, MemoryCorpusInput, MemoryCorpusSequence,
    };

    #[test]
    fn frozen_profile_and_counter_phase_gates_are_exact() {
        assert_eq!(MAX_REPETITIONS, 2048);
        assert_eq!(SAMPLE_PAIRS, 20);
        assert_eq!(PREFLIGHT_TRIALS, 2);
        assert!(
            validate_preflight_counters(
                CounterDelta {
                    hits: 2399,
                    misses: 1,
                    successful_compiles: 1,
                    successful_publications: 1,
                    replacements: 0,
                },
                2400,
            )
            .is_ok()
        );
        assert!(validate_preflight_counters(CounterDelta::default(), 2400).is_err());
        assert!(only_hits(CounterDelta {
            hits: 1200,
            ..CounterDelta::default()
        }));
        assert!(!only_hits(CounterDelta {
            hits: 1199,
            misses: 1,
            ..CounterDelta::default()
        }));
        assert!(validate_hit_only_sweeps(
            CounterDelta {
                hits: 1200,
                ..CounterDelta::default()
            },
            1200,
            1,
        ));
        assert!(validate_hit_only_sweeps(
            CounterDelta {
                hits: 2400,
                ..CounterDelta::default()
            },
            1200,
            2,
        ));
        for invalid in [
            CounterDelta {
                hits: 1199,
                ..CounterDelta::default()
            },
            CounterDelta {
                hits: 1200,
                misses: 1,
                ..CounterDelta::default()
            },
            CounterDelta {
                hits: 1200,
                successful_compiles: 1,
                ..CounterDelta::default()
            },
            CounterDelta {
                hits: 1200,
                successful_publications: 1,
                ..CounterDelta::default()
            },
            CounterDelta {
                hits: 1200,
                replacements: 1,
                ..CounterDelta::default()
            },
        ] {
            assert!(!validate_hit_only_sweeps(invalid, 1200, 1));
        }
    }

    #[test]
    fn partial_warmup_and_calibration_only_are_never_scored() {
        assert_eq!(warmup_summary(64, 0, 1200, false), (None, false));
        assert_eq!(warmup_summary(1200, 1200, 1200, true), (Some(1), true));
        assert_eq!(
            classify_status(true, true, false, None, 1),
            ("calibration_only_unscored", None)
        );
        assert_eq!(
            classify_status(false, true, true, None, MAX_RUN_TIME_NS),
            (
                "diagnostic_only_overall_deadline",
                Some("overall_wall_time_ceiling_after_final_sync".into())
            )
        );
        assert_eq!(
            classify_status(false, true, false, Some("deadline".into()), 1).0,
            "diagnostic_only_incomplete"
        );
    }

    #[test]
    fn fingerprint_names_bind_warm_cold_cache_and_runtime_sources() {
        let files = source_files();
        assert_eq!(files.len(), 15);
        assert_eq!(files[0], "tools/bench/src/two_site_memory_cached.rs");
        assert!(files.contains(&"tools/bench/src/two_site_memory_warm.rs"));
        assert!(files.contains(&"tools/bench/src/two_site_memory_cold.rs"));
        assert!(files.contains(&"crates/od-jit/src/memory_cache.rs"));
        assert!(files.contains(&"crates/od-host/src/lib.rs"));
        assert!(files.contains(&"crates/od-testkit/src/memory_corpus.rs"));
    }

    #[test]
    fn boundary_exit_normalizes_only_after_exact_budget_end_parity() {
        let mut gpr = [0; 16];
        gpr[3] = 24;
        let input = MemoryCorpusInput {
            sequence: MemoryCorpusSequence::StoreThenLoad,
            gpr,
            flags: 2,
            first_address: 24,
            second_address: 32,
            ram: [0; MEMORY_CORPUS_RAM_LEN],
        };
        let events = [
            Some(MemoryEvent {
                operation: MemoryOperation::Write,
                address: 24,
                width: 8,
            }),
            Some(MemoryEvent {
                operation: MemoryOperation::Read,
                address: 24,
                width: 8,
            }),
        ];
        let expected = Outcome {
            state: od_isa::LongModeState {
                cr3: None,
                rip: ENTRY_RIP + 6,
                ..od_isa::LongModeState::default()
            },
            ram: [0; MEMORY_CORPUS_RAM_LEN],
            exit: "InstructionBudgetExhausted".into(),
            retired: 2,
            remaining_budget: 0,
            memory_events: events,
            memory_event_count: 2,
        };
        let mut boundary = expected.clone();
        boundary.exit = format!(
            "{:?}",
            od_jit::memory_dispatch::BoundedMemoryExit::BoundaryReached { rip: ENTRY_RIP + 6 }
        );
        assert!(cached_pair_matches(&expected, &boundary, &input));

        let mut wrong = boundary.clone();
        wrong.state.rip += 1;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.retired = 1;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.remaining_budget = 1;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.state.rflags ^= 1;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.memory_events[0] = None;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.memory_event_count = 1;
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.exit = "EventPending".into();
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong = boundary.clone();
        wrong.exit = "Failed".into();
        assert!(!cached_pair_matches(&expected, &wrong, &input));
        let mut wrong_expected = expected.clone();
        wrong_expected.exit = "InstructionLimitReached".into();
        assert!(!cached_pair_matches(&wrong_expected, &boundary, &input));
        let mut wrong = boundary;
        wrong.exit = format!(
            "{:?}",
            od_jit::memory_dispatch::BoundedMemoryExit::BoundaryReached { rip: ENTRY_RIP + 5 }
        );
        assert!(!cached_pair_matches(&expected, &wrong, &input));
    }

    #[test]
    fn completed_cached_side_is_retained_when_interpreter_side_fails() {
        use super::{
            SideFailure, SideMeasurement, add_delta, completed_side, partial_attempt_record,
        };

        let native = SideMeasurement {
            total_ns: 55_000,
            wall_elapsed_ns: 61_000,
            executions: 1200,
            native_cache_delta: CounterDelta {
                hits: 1200,
                ..CounterDelta::default()
            },
        };
        let mut phase_delta = CounterDelta::default();
        add_delta(&mut phase_delta, native.native_cache_delta).unwrap();

        let failure = SideFailure {
            reason: "interpreter_failure",
            wall_elapsed_ns: 83_000,
            timed_ns: 2_000,
            completed_executions: 7,
            native_cache_delta: CounterDelta::default(),
        };
        let completed = completed_side("cached", &native);
        let record = partial_attempt_record(
            "StoreThenLoad",
            "scored",
            "interpreter",
            1,
            &failure,
            Some(completed),
        );

        assert_eq!(phase_delta.hits, 1200);
        assert_eq!(record.completed_executions, 7);
        assert_eq!(record.completed_timed_calls_ns, 2_000);
        assert_eq!(record.wall_elapsed_ns, 83_000);
        let completed_side = record.completed_other_side.as_ref().unwrap();
        assert_eq!(completed_side.side, "cached");
        assert_eq!(completed_side.timed_calls_ns, 55_000);
        assert_eq!(completed_side.wall_elapsed_ns, 61_000);
        assert_eq!(completed_side.completed_executions, 1200);
        assert_eq!(completed_side.native_cache_delta.hits, 1200);
        let serialized = serde_json::to_value(record).unwrap();
        assert_eq!(
            serialized["completed_other_side"]["native_cache_delta"]["hits"],
            1200
        );
        assert_eq!(serialized["wall_elapsed_ns"], 83_000);
        assert_eq!(serialized["completed_timed_calls_ns"], 2_000);
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn check_deadline(started: Instant) -> Result<(), String> {
    if started.elapsed().as_nanos() >= MAX_RUN_TIME_NS {
        Err("overall_wall_time_ceiling_during_sequence".into())
    } else {
        Ok(())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_interpreter(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    repetitions: u32,
    started: Instant,
) -> Result<SideMeasurement, SideFailure> {
    let start = Instant::now();
    let mut executions = 0usize;
    let mut total_ns = 0u128;
    for _ in 0..repetitions {
        for (case_index, case) in cases.iter().enumerate() {
            if started.elapsed().as_nanos() >= MAX_RUN_TIME_NS {
                return Err(SideFailure {
                    reason: "overall_wall_time_ceiling_during_interpreter_sweep",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: CounterDelta::default(),
                });
            }
            let bytes = case.input.sequence.bytes();
            let mut interpreter = InterpreterFixture::new(case, &bytes);
            let (outcome, elapsed_ns) = interpreter.execute();
            total_ns += elapsed_ns;
            executions += 1;
            if !cold::repeat_outcomes_equal(&oracles[case_index], &outcome) {
                return Err(SideFailure {
                    reason: "interpreter_result_changed_after_preflight",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: CounterDelta::default(),
                });
            }
            if executions % CHECK_EVERY_CALLS == 0
                && started.elapsed().as_nanos() >= MAX_RUN_TIME_NS
            {
                return Err(SideFailure {
                    reason: "overall_wall_time_ceiling_during_interpreter_sweep",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: CounterDelta::default(),
                });
            }
        }
    }
    Ok(SideMeasurement {
        total_ns,
        wall_elapsed_ns: start.elapsed().as_nanos(),
        executions,
        native_cache_delta: CounterDelta::default(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_cached(
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    cache: &mut MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut od_host::CachedPublishedMemoryPairBackend,
    repetitions: u32,
    started: Instant,
) -> Result<SideMeasurement, SideFailure> {
    let start = Instant::now();
    let counter_start = snapshot(cache.counters());
    let mut executions = 0usize;
    let mut total_ns = 0u128;
    for _ in 0..repetitions {
        for (case_index, case) in cases.iter().enumerate() {
            if started.elapsed().as_nanos() >= MAX_RUN_TIME_NS {
                let delta =
                    counter_delta(counter_start, snapshot(cache.counters())).unwrap_or_default();
                return Err(SideFailure {
                    reason: "overall_wall_time_ceiling_during_cached_sweep",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: delta,
                });
            }
            let bytes = case.input.sequence.bytes();
            let mut native = CachedFixture::new(case, &bytes);
            let (outcome, elapsed_ns) =
                match native.execute(memory_profile(sequence), cache, backend) {
                    Ok(value) => value,
                    Err(_) => {
                        let delta = counter_delta(counter_start, snapshot(cache.counters()))
                            .unwrap_or_default();
                        return Err(SideFailure {
                            reason: "cached_execution_error",
                            wall_elapsed_ns: start.elapsed().as_nanos(),
                            timed_ns: total_ns,
                            completed_executions: executions,
                            native_cache_delta: delta,
                        });
                    }
                };
            total_ns += elapsed_ns;
            executions += 1;
            if !cached_pair_matches(&oracles[case_index], &outcome, &case.input) {
                let delta =
                    counter_delta(counter_start, snapshot(cache.counters())).unwrap_or_default();
                return Err(SideFailure {
                    reason: "cached_result_changed_after_preflight",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: delta,
                });
            }
            if executions % CHECK_EVERY_CALLS == 0
                && started.elapsed().as_nanos() >= MAX_RUN_TIME_NS
            {
                let delta =
                    counter_delta(counter_start, snapshot(cache.counters())).unwrap_or_default();
                return Err(SideFailure {
                    reason: "overall_wall_time_ceiling_during_cached_sweep",
                    wall_elapsed_ns: start.elapsed().as_nanos(),
                    timed_ns: total_ns,
                    completed_executions: executions,
                    native_cache_delta: delta,
                });
            }
        }
    }
    let delta = counter_delta(counter_start, snapshot(cache.counters())).unwrap_or_default();
    let expected = executions as u64;
    if !validate_hit_only_sweeps(delta, cases.len(), repetitions) || delta.hits != expected {
        return Err(SideFailure {
            reason: "cached_measurement_counter_mismatch",
            wall_elapsed_ns: start.elapsed().as_nanos(),
            timed_ns: total_ns,
            completed_executions: executions,
            native_cache_delta: delta,
        });
    }
    Ok(SideMeasurement {
        total_ns,
        wall_elapsed_ns: start.elapsed().as_nanos(),
        executions,
        native_cache_delta: delta,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn warmup_sequence(
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    cache: &mut MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut od_host::CachedPublishedMemoryPairBackend,
    started: Instant,
    journal: &mut std::io::BufWriter<std::fs::File>,
) -> Result<(usize, usize, CounterDelta, bool, Option<String>), cold::BenchError> {
    let interpreter = match measure_interpreter(cases, oracles, WARMUP_SWEEPS, started) {
        Ok(value) => value,
        Err(failure) => {
            let reason = failure.reason.to_owned();
            let completed = failure.completed_executions;
            write_partial(
                journal,
                sequence,
                "warmup",
                "interpreter",
                WARMUP_SWEEPS,
                failure,
                None,
            )?;
            return Ok((completed, 0, CounterDelta::default(), false, Some(reason)));
        }
    };
    let native = match measure_cached(
        sequence,
        cases,
        oracles,
        cache,
        backend,
        WARMUP_SWEEPS,
        started,
    ) {
        Ok(value) => value,
        Err(failure) => {
            let reason = failure.reason.to_owned();
            let completed = failure.completed_executions;
            let cache_delta = failure.native_cache_delta;
            write_partial(
                journal,
                sequence,
                "warmup",
                "cached",
                WARMUP_SWEEPS,
                failure,
                Some(completed_side("interpreter", &interpreter)),
            )?;
            return Ok((
                interpreter.executions,
                completed,
                cache_delta,
                false,
                Some(reason),
            ));
        }
    };
    let complete = interpreter.executions == cases.len()
        && native.executions == cases.len()
        && native.native_cache_delta.hits as usize == cases.len()
        && only_hits(native.native_cache_delta);
    if !complete {
        return Err(cold::err("warmup was not exactly one full hit-only sweep"));
    }
    Ok((
        interpreter.executions,
        native.executions,
        native.native_cache_delta,
        true,
        None,
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn calibrate_sequence(
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    cache: &mut MemoryPairCache<od_host::PublishedMemoryPairHandle>,
    backend: &mut od_host::CachedPublishedMemoryPairBackend,
    started: Instant,
    journal: &mut std::io::BufWriter<std::fs::File>,
    process_run: u8,
) -> Result<
    (
        u32,
        Vec<CalibrationAttempt>,
        bool,
        CounterDelta,
        Option<String>,
    ),
    cold::BenchError,
> {
    let mut repetitions = 1u32;
    let mut attempts = Vec::new();
    let mut phase_delta = CounterDelta::default();
    loop {
        let interpreter_first = (attempts.len() + usize::from(process_run % 2 == 0)) % 2 == 0;
        let mut native = None;
        let mut interpreter = None;
        let mut failure = None;
        let mut failed_side = "";
        if interpreter_first {
            match measure_interpreter(cases, oracles, repetitions, started) {
                Ok(value) => interpreter = Some(value),
                Err(value) => {
                    failed_side = "interpreter";
                    failure = Some(value);
                }
            }
            if failure.is_none() {
                match measure_cached(
                    sequence,
                    cases,
                    oracles,
                    cache,
                    backend,
                    repetitions,
                    started,
                ) {
                    Ok(value) => {
                        add_delta(&mut phase_delta, value.native_cache_delta).map_err(cold::err)?;
                        journal_completed_cached_side(
                            journal,
                            sequence,
                            "calibration",
                            repetitions,
                            &value,
                        )?;
                        native = Some(value);
                    }
                    Err(value) => {
                        add_delta(&mut phase_delta, value.native_cache_delta).map_err(cold::err)?;
                        failed_side = "cached";
                        failure = Some(value);
                    }
                }
            }
        } else {
            match measure_cached(
                sequence,
                cases,
                oracles,
                cache,
                backend,
                repetitions,
                started,
            ) {
                Ok(value) => {
                    add_delta(&mut phase_delta, value.native_cache_delta).map_err(cold::err)?;
                    journal_completed_cached_side(
                        journal,
                        sequence,
                        "calibration",
                        repetitions,
                        &value,
                    )?;
                    native = Some(value);
                }
                Err(value) => {
                    add_delta(&mut phase_delta, value.native_cache_delta).map_err(cold::err)?;
                    failed_side = "cached";
                    failure = Some(value);
                }
            }
            if failure.is_none() {
                match measure_interpreter(cases, oracles, repetitions, started) {
                    Ok(value) => interpreter = Some(value),
                    Err(value) => {
                        failed_side = "interpreter";
                        failure = Some(value);
                    }
                }
            }
        }
        if let Some(failure) = failure {
            let reason = failure.reason.to_owned();
            let completed_other = if failed_side == "cached" {
                interpreter
                    .as_ref()
                    .map(|side| completed_side("interpreter", side))
            } else {
                native.as_ref().map(|side| completed_side("cached", side))
            };
            write_partial(
                journal,
                sequence,
                "calibration",
                failed_side,
                repetitions,
                failure,
                completed_other,
            )?;
            return Ok((repetitions, attempts, false, phase_delta, Some(reason)));
        }
        let (native, interpreter) = (
            native.expect("completed cached side"),
            interpreter.expect("completed interpreter side"),
        );
        let expected = cases.len() * repetitions as usize;
        let attempt = CalibrationAttempt {
            record_type: "calibration_attempt",
            sequence: sequence_name(sequence),
            attempt_index: attempts.len(),
            repetitions,
            native_total_ns: native.total_ns,
            interpreter_total_ns: interpreter.total_ns,
            native_wall_elapsed_ns: native.wall_elapsed_ns,
            interpreter_wall_elapsed_ns: interpreter.wall_elapsed_ns,
            interpreter_executions: interpreter.executions,
            native_executions: native.executions,
            native_cache_delta: native.native_cache_delta,
            native_floor_met: native.total_ns >= MIN_SIDE_TIME_NS,
            interpreter_floor_met: interpreter.total_ns >= MIN_SIDE_TIME_NS,
        };
        if native.executions != expected || interpreter.executions != expected {
            return Err(cold::err(
                "calibration execution count does not equal full sweeps",
            ));
        }
        cold::append_durable_journal_record(journal, &attempt)?;
        attempts.push(attempt);
        if let Err(reason) = check_deadline(started) {
            return Ok((repetitions, attempts, false, phase_delta, Some(reason)));
        }
        if attempts
            .last()
            .is_some_and(|attempt| attempt.native_floor_met && attempt.interpreter_floor_met)
        {
            return Ok((repetitions, attempts, true, phase_delta, None));
        }
        let Some(next) = repetitions
            .checked_mul(2)
            .filter(|next| *next <= MAX_REPETITIONS)
        else {
            return Ok((
                repetitions,
                attempts,
                false,
                phase_delta,
                Some("maximum_repetitions".into()),
            ));
        };
        repetitions = next;
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn write_partial(
    journal: &mut std::io::BufWriter<std::fs::File>,
    sequence: od_testkit::memory_corpus::MemoryCorpusSequence,
    phase: &'static str,
    side: &'static str,
    repetitions: u32,
    failure: SideFailure,
    completed_other_side: Option<CompletedSide>,
) -> Result<(), cold::BenchError> {
    let record = partial_attempt_record(
        sequence_name(sequence),
        phase,
        side,
        repetitions,
        &failure,
        completed_other_side,
    );
    cold::append_durable_journal_record(journal, &record)
}
