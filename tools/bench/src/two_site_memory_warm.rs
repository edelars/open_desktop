//! Resident direct-entry comparison for the frozen S3-34a checked-memory subset.
//!
//! One published artifact is retained for one sequence at a time. Compile and
//! publication are reported separately; warm sample timers surround only the
//! direct native entry or interpreter call.

use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

use od_testkit::memory_corpus::{MemoryCorpusSequence, S3_34A_SEQUENCES};
use serde::Serialize;

use crate::HostMetadata;
use crate::two_site_memory_cold as cold;

#[path = "two_site_memory_cached.rs"]
mod cached;

const PROFILE_ID: &str = "s3-36d-two-site-memory-warm-direct-v1";
const PREFLIGHT_TRIALS: usize = 2;
const WARMUP_SWEEPS: u32 = 1;
const MIN_SIDE_TIME_NS: u128 = cold::MIN_SIDE_TIME.as_nanos();
const MAX_REPETITIONS: u32 = cold::MAX_REPETITIONS;
const SAMPLE_PAIRS: usize = cold::MAX_SAMPLE_PAIRS;
const MAX_RUN_TIME_NS: u128 = cold::MAX_RUN_TIME.as_nanos();
const WARM_SOURCE: &[u8] = include_bytes!("two_site_memory_warm.rs");
const COLD_SOURCE: &[u8] = include_bytes!("two_site_memory_cold.rs");
const CLI_SOURCE: &[u8] = include_bytes!("main.rs");
const LIB_SOURCE: &[u8] = include_bytes!("lib.rs");
const JIT_SOURCE: &[u8] = include_bytes!("../../../crates/od-jit/src/memory_load_v2.rs");
const HOST_SOURCE: &[u8] = include_bytes!("../../../crates/od-host/src/lib.rs");
const ISA_SOURCE: &[u8] = include_bytes!("../../../crates/od-isa/src/execute.rs");
const TESTKIT_SOURCE: &[u8] = include_bytes!("../../../crates/od-testkit/src/memory_corpus.rs");

#[derive(Clone, Debug, Serialize)]
pub struct PreflightReport {
    schema: u32,
    profile_id: &'static str,
    process_run: u8,
    timing_started: bool,
    input_digest: String,
    source_sha256: String,
    measure_host: HostMetadata,
    preflight_trials_per_input_backend: usize,
    interpreter_executions: usize,
    native_executions: usize,
    parity_trials: usize,
    sequences: Vec<SequencePreflight>,
}

#[derive(Clone, Debug, Serialize)]
struct SequencePreflight {
    sequence: &'static str,
    input_count: usize,
    compile_count: u32,
    compile_ns: u128,
    publish_count: u32,
    publish_ns: u128,
    ir_drop_count: u32,
    ir_drop_ns: u128,
    resident_mapping_high_water: u32,
    published_drop_count: u32,
    published_drop_ns: u128,
    interpreter_executions: usize,
    native_executions: usize,
    parity_trials: usize,
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
    journal_path: String,
    measure_host: HostMetadata,
    preflight_trials_per_input_backend: usize,
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
    compile_count: u32,
    compile_ns: u128,
    publish_count: u32,
    publish_ns: u128,
    ir_drop_count: u32,
    ir_drop_ns: u128,
    resident_mapping_high_water: u32,
    preflight_interpreter_executions: usize,
    preflight_native_executions: usize,
    preflight_parity_trials: usize,
    warmup_sweeps_per_backend: Option<u32>,
    warmup_complete: bool,
    warmup_interpreter_executions: usize,
    warmup_native_executions: usize,
    warmup_interpreter_ns: Option<u128>,
    warmup_native_ns: Option<u128>,
    repetitions_per_side: u32,
    calibration_floor_passed: bool,
    calibration_attempts: Vec<CalibrationAttempt>,
    sample_pairs: Vec<SamplePair>,
    sample_floor_passed: bool,
    published_drop_count: u32,
    published_drop_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct CalibrationAttempt {
    record_type: &'static str,
    sequence: &'static str,
    repetitions: u32,
    native_total_ns: u128,
    interpreter_total_ns: u128,
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
    native_over_interpreter: f64,
    native_floor_met: bool,
    interpreter_floor_met: bool,
}

struct WarmupProgress {
    interpreter_ns: Option<u128>,
    native_ns: Option<u128>,
    interpreter_executions: usize,
    native_executions: usize,
    stop_reason: Option<String>,
}

struct InterpreterFixture<'a> {
    input: &'a cold::GeneratedCase,
    state: od_isa::LongModeState,
    memory: cold::FixtureMemory<'a>,
}

impl<'a> InterpreterFixture<'a> {
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
        }
    }

    fn reset(&mut self) {
        self.state = cold::make_state(&self.input.input);
        self.memory.ram = self.input.input.ram;
        self.memory.memory_events = [None; 2];
        self.memory.memory_event_count = 0;
    }

    fn execute(&mut self) -> (cold::Outcome, u128) {
        self.reset();
        let start = Instant::now();
        let result = cold::execute_interpreter(&mut self.state, &mut self.memory);
        let elapsed_ns = start.elapsed().as_nanos();
        black_box((&self.state, &self.memory.ram, &result));
        (
            cold::outcome_from_interpreter(&self.state, &self.memory, result),
            elapsed_ns,
        )
    }
}

struct NativeFixture<'a> {
    input: &'a cold::GeneratedCase,
    state: od_isa::LongModeState,
    memory: cold::FixtureMemory<'a>,
    stop_requested: AtomicU8,
}

impl<'a> NativeFixture<'a> {
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
        published: &od_host::PublishedCode,
    ) -> Result<(cold::Outcome, u128), cold::BenchError> {
        self.state = cold::make_state(&self.input.input);
        self.memory.ram = self.input.input.ram;
        self.memory.memory_events = [None; 2];
        self.memory.memory_event_count = 0;
        self.stop_requested.store(0, Ordering::Relaxed);
        let start = Instant::now();
        let result = cold::execute_published(
            published,
            &mut self.state,
            &mut self.memory,
            &self.stop_requested,
        )?;
        let elapsed_ns = start.elapsed().as_nanos();
        black_box((&self.state, &self.memory.ram, &result));
        Ok((
            cold::outcome_from_native(&self.state, &self.memory, result),
            elapsed_ns,
        ))
    }
}

pub fn preflight(process_run: u8) -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = process_run;
        Err("two-site-memory-warm requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        cold::validate_process_run(process_run)?;
        let (inputs, input_digest) = cold::generate_inputs()?;
        let mut sequences = Vec::with_capacity(S3_34A_SEQUENCES.len());
        let mut interpreter_executions = 0usize;
        let mut native_executions = 0usize;
        let mut parity_trials = 0usize;
        for sequence in S3_34A_SEQUENCES {
            let cases = select_sequence(&inputs, sequence)?;
            let (published, compile_ns, publish_ns, ir_drop_ns) = compile_publish(sequence)?;
            let (sequence_interpreter, sequence_native, sequence_parity) =
                resident_preflight(&cases, &published)?;
            let published_drop_start = Instant::now();
            drop(published);
            let published_drop_ns = published_drop_start.elapsed().as_nanos();
            interpreter_executions += sequence_interpreter;
            native_executions += sequence_native;
            parity_trials += sequence_parity;
            sequences.push(SequencePreflight {
                sequence: sequence_name(sequence),
                input_count: cases.len(),
                compile_count: 1,
                compile_ns,
                publish_count: 1,
                publish_ns,
                ir_drop_count: 1,
                ir_drop_ns,
                resident_mapping_high_water: 1,
                published_drop_count: 1,
                published_drop_ns,
                interpreter_executions: sequence_interpreter,
                native_executions: sequence_native,
                parity_trials: sequence_parity,
            });
        }
        Ok(PreflightReport {
            schema: 1,
            profile_id: PROFILE_ID,
            process_run,
            timing_started: false,
            input_digest,
            source_sha256: source_sha256(),
            measure_host: HostMetadata::current(),
            preflight_trials_per_input_backend: PREFLIGHT_TRIALS,
            interpreter_executions,
            native_executions,
            parity_trials,
            sequences,
        })
    }
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

pub fn cached_preflight(
    process_run: u8,
) -> Result<cached::PreflightReport, Box<dyn std::error::Error>> {
    cached::preflight(process_run)
}

pub fn cached_run(
    process_run: u8,
    journal_path: &Path,
) -> Result<cached::Report, Box<dyn std::error::Error>> {
    cached::run(process_run, journal_path)
}

pub fn cached_calibration_only(
    process_run: u8,
    journal_path: &Path,
) -> Result<cached::Report, Box<dyn std::error::Error>> {
    cached::calibration_only(process_run, journal_path)
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
    let mut sequence_results = Vec::with_capacity(S3_34A_SEQUENCES.len());
    let mut stop_reason = None;
    for sequence in S3_34A_SEQUENCES {
        if let Err(error) = cold::check_run_bound(started) {
            stop_reason = error.terminal_reason.map(str::to_owned);
            if stop_reason.is_some() {
                break;
            }
            return Err(Box::new(error));
        }
        let cases = select_sequence(&inputs, sequence)?;
        let (published, compile_ns, publish_ns, ir_drop_ns) = compile_publish(sequence)?;
        let (oracles, preflight_interpreter, preflight_native, preflight_parity) =
            resident_preflight_with_oracles(&cases, &published)?;
        let warmup = warmup_sequence(&cases, &oracles, &published, started)?;
        let mut sequence_stop = warmup.stop_reason.clone();
        let calibration = if sequence_stop.is_none() {
            calibrate_sequence(
                sequence,
                &cases,
                &oracles,
                &published,
                started,
                &mut journal,
                process_run,
            )?
        } else {
            (0, Vec::new(), false, sequence_stop.clone())
        };
        let (repetitions, calibration_attempts, calibration_floor_passed, calibration_stop) =
            calibration;
        if sequence_stop.is_none() {
            sequence_stop = calibration_stop;
        }
        let mut sample_pairs = Vec::with_capacity(if calibration_only { 0 } else { SAMPLE_PAIRS });
        let mut sample_floor_passed = !calibration_only && calibration_floor_passed;
        if !calibration_only && calibration_floor_passed && sequence_stop.is_none() {
            for pair_index in 0..SAMPLE_PAIRS {
                let interpreter_first = (pair_index + usize::from(process_run % 2 == 0)) % 2 == 0;
                let measured = if interpreter_first {
                    measure_interpreter(&cases, &oracles, repetitions, started).and_then(
                        |interpreter| {
                            measure_native(&cases, &oracles, &published, repetitions, started)
                                .map(|native| (native, interpreter))
                        },
                    )
                } else {
                    measure_native(&cases, &oracles, &published, repetitions, started).and_then(
                        |native| {
                            measure_interpreter(&cases, &oracles, repetitions, started)
                                .map(|interpreter| (native, interpreter))
                        },
                    )
                };
                let (native_ns, interpreter_ns) = match measured {
                    Ok(pair) => pair,
                    Err(error) if error.terminal_reason.is_some() => {
                        sequence_stop = error.terminal_reason.map(str::to_owned);
                        sample_floor_passed = false;
                        break;
                    }
                    Err(error) => return Err(Box::new(error)),
                };
                let native_floor_met = native_ns >= MIN_SIDE_TIME_NS;
                let interpreter_floor_met = interpreter_ns >= MIN_SIDE_TIME_NS;
                sample_floor_passed &= native_floor_met && interpreter_floor_met;
                let pair = SamplePair {
                    record_type: "sample_pair",
                    sequence: sequence_name(sequence),
                    pair_index,
                    interpreter_first,
                    repetitions,
                    native_total_ns: native_ns,
                    interpreter_total_ns: interpreter_ns,
                    native_over_interpreter: native_ns as f64 / interpreter_ns as f64,
                    native_floor_met,
                    interpreter_floor_met,
                };
                cold::append_durable_journal_record(&mut journal, &pair)?;
                sample_pairs.push(pair);
                if let Err(error) = cold::check_run_bound(started) {
                    if let Some(reason) = error.terminal_reason {
                        sequence_stop = Some(reason.to_owned());
                        sample_floor_passed = false;
                        break;
                    }
                    return Err(Box::new(error));
                }
            }
        }
        let published_drop_count = 1;
        let published_drop_start = Instant::now();
        drop(published);
        let published_drop_ns = published_drop_start.elapsed().as_nanos();
        if stop_reason.is_none() {
            stop_reason = sequence_stop.clone();
        }
        let (warmup_sweeps, warmup_complete) = warmup_summary(
            warmup.native_executions,
            warmup.interpreter_executions,
            cases.len(),
        );
        let sequence_result = SequenceReport {
            sequence: sequence_name(sequence),
            input_count: cases.len(),
            compile_count: 1,
            compile_ns,
            publish_count: 1,
            publish_ns,
            ir_drop_count: 1,
            ir_drop_ns,
            resident_mapping_high_water: 1,
            preflight_interpreter_executions: preflight_interpreter,
            preflight_native_executions: preflight_native,
            preflight_parity_trials: preflight_parity,
            warmup_sweeps_per_backend: warmup_sweeps,
            warmup_complete,
            warmup_interpreter_executions: warmup.interpreter_executions,
            warmup_native_executions: warmup.native_executions,
            warmup_interpreter_ns: warmup.interpreter_ns,
            warmup_native_ns: warmup.native_ns,
            repetitions_per_side: repetitions,
            calibration_floor_passed,
            calibration_attempts,
            sample_pairs,
            sample_floor_passed,
            published_drop_count,
            published_drop_ns,
        };
        sequence_results.push(sequence_result);
        if stop_reason.is_some() {
            break;
        }
    }
    let measure_host = HostMetadata::current();
    let source_sha256 = source_sha256();
    journal.flush()?;
    journal.get_ref().sync_data()?;
    drop(journal);
    let total_elapsed_ns = started.elapsed().as_nanos();
    let all_calibration_passed = sequence_results.len() == S3_34A_SEQUENCES.len()
        && sequence_results
            .iter()
            .all(|sequence| sequence.calibration_floor_passed);
    let all_samples_passed = !calibration_only
        && sequence_results.len() == S3_34A_SEQUENCES.len()
        && sequence_results.iter().all(|sequence| {
            sequence.sample_pairs.len() == SAMPLE_PAIRS && sequence.sample_floor_passed
        });
    let (status, stop_reason, sample_floor_passed) = classify_status(
        calibration_only,
        all_calibration_passed,
        all_samples_passed,
        stop_reason,
        total_elapsed_ns,
    );
    Ok(Report {
        schema: 1,
        profile_id: PROFILE_ID,
        process_run,
        status,
        stop_reason,
        calibration_only,
        timing_started: true,
        input_digest,
        source_sha256,
        journal_path: journal_path.display().to_string(),
        measure_host,
        preflight_trials_per_input_backend: PREFLIGHT_TRIALS,
        interpreter_preflight_executions: sequence_results
            .iter()
            .map(|sequence| sequence.preflight_interpreter_executions)
            .sum(),
        native_preflight_executions: sequence_results
            .iter()
            .map(|sequence| sequence.preflight_native_executions)
            .sum(),
        parity_trials: sequence_results
            .iter()
            .map(|sequence| sequence.preflight_parity_trials)
            .sum(),
        calibration_floor_passed: all_calibration_passed,
        sample_floor_passed,
        minimum_side_time_ns: MIN_SIDE_TIME_NS,
        sample_floor_policy: "Retain all 20 raw pairs separately per sequence. Any side below 200ms makes the process diagnostic-only; no partial scoring or trimming.",
        timer_boundaries: "Each native warm sample times only the direct typed execute call for one input; each interpreter sample times only od_isa::execute::run. State/RAM/callback reset and validation are outside timers. Each sequence owns one compile/publish and one resident mapping, dropped before the next sequence is compiled.",
        sequences: sequence_results,
        total_elapsed_ns,
    })
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn run_profile(
    process_run: u8,
    journal_path: &Path,
    calibration_only: bool,
) -> Result<Report, Box<dyn std::error::Error>> {
    let _ = (process_run, journal_path, calibration_only);
    Err("two-site-memory-warm requires native Apple Silicon macOS execution".into())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn select_sequence<'a>(
    inputs: &'a [cold::GeneratedCase],
    sequence: MemoryCorpusSequence,
) -> Result<Vec<&'a cold::GeneratedCase>, cold::BenchError> {
    if !S3_34A_SEQUENCES.contains(&sequence) {
        return Err(cold::err("sequence is outside the frozen S3-34a subset"));
    }
    let selected: Vec<_> = inputs
        .iter()
        .filter(|input| input.input.sequence == sequence)
        .collect();
    if selected.len() != 3 * 400
        || selected
            .iter()
            .any(|input| input.input.sequence != sequence)
    {
        return Err(cold::err(format!(
            "frozen sequence count mismatch for {:?}: {}",
            sequence,
            selected.len()
        )));
    }
    Ok(selected)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn compile_publish(
    sequence: MemoryCorpusSequence,
) -> Result<(od_host::PublishedCode, u128, u128, u128), cold::BenchError> {
    let bytes = sequence.bytes();
    let code = od_jit::block::CodeView::new(cold::ENTRY_RIP, &bytes).map_err(cold::bench_error)?;
    let compile_start = Instant::now();
    let block = od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(code, cold::ENTRY_RIP)
        .map_err(cold::bench_error)?;
    let compile_ns = compile_start.elapsed().as_nanos();
    let publish_start = Instant::now();
    let published =
        od_host::PublishedCode::publish_ir_memory_two_site_v2(&block).map_err(cold::bench_error)?;
    let publish_ns = publish_start.elapsed().as_nanos();
    let ir_drop_start = Instant::now();
    drop(block);
    let ir_drop_ns = ir_drop_start.elapsed().as_nanos();
    Ok((published, compile_ns, publish_ns, ir_drop_ns))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn resident_preflight_with_oracles(
    cases: &[&cold::GeneratedCase],
    published: &od_host::PublishedCode,
) -> Result<(Vec<cold::Outcome>, usize, usize, usize), cold::BenchError> {
    let sequence = cases
        .first()
        .map(|case| case.input.sequence)
        .ok_or_else(|| cold::err("empty frozen sequence"))?;
    let bytes = sequence.bytes();
    let mut oracles = Vec::with_capacity(cases.len());
    let mut interpreter_executions = 0usize;
    let mut native_executions = 0usize;
    let mut parity_trials = 0usize;
    for case in cases {
        let mut first_interpreter = None;
        let mut first_native = None;
        for trial in 0..PREFLIGHT_TRIALS {
            let mut interpreter = InterpreterFixture::new(case, &bytes);
            let (interpreter_outcome, _) = interpreter.execute();
            interpreter_executions += 1;
            let mut native = NativeFixture::new(case, &bytes);
            let (native_outcome, _) = native.execute(published)?;
            native_executions += 1;
            cold::validate_pair(
                &interpreter_outcome,
                &native_outcome,
                &case.input,
                case.seed,
                case.index,
                true,
            )?;
            parity_trials += 1;
            if trial == 0 {
                first_interpreter = Some(interpreter_outcome);
                first_native = Some(native_outcome);
            } else {
                let expected_interpreter = first_interpreter
                    .as_ref()
                    .expect("first interpreter preflight result retained");
                let expected_native = first_native
                    .as_ref()
                    .expect("first native preflight result retained");
                if !cold::repeat_outcomes_equal(expected_interpreter, &interpreter_outcome)
                    || !cold::repeat_outcomes_equal(expected_native, &native_outcome)
                {
                    return Err(cold::err(format!(
                        "resident preflight repeat mismatch for {:?}, seed={}, index={}",
                        case.input.sequence, case.seed, case.index
                    )));
                }
            }
        }
        oracles.push(first_interpreter.expect("exactly two preflight trials"));
    }
    let expected = 1200 * PREFLIGHT_TRIALS;
    if interpreter_executions != expected
        || native_executions != expected
        || parity_trials != expected
    {
        return Err(cold::err(format!(
            "resident preflight counts differ: interpreter={interpreter_executions}, native={native_executions}, parity={parity_trials}, expected={expected}"
        )));
    }
    Ok((
        oracles,
        interpreter_executions,
        native_executions,
        parity_trials,
    ))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn resident_preflight(
    cases: &[&cold::GeneratedCase],
    published: &od_host::PublishedCode,
) -> Result<(usize, usize, usize), cold::BenchError> {
    let (_, interpreter, native, parity) = resident_preflight_with_oracles(cases, published)?;
    Ok((interpreter, native, parity))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn warmup_sequence(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    published: &od_host::PublishedCode,
    started: Instant,
) -> Result<WarmupProgress, cold::BenchError> {
    let (native_ns, native_executions) =
        match measure_native_with_count(cases, oracles, published, WARMUP_SWEEPS, started) {
            Ok(progress) => progress,
            Err((error, executions)) if error.terminal_reason.is_some() => {
                return Ok(WarmupProgress {
                    interpreter_ns: None,
                    native_ns: None,
                    interpreter_executions: 0,
                    native_executions: executions,
                    stop_reason: error.terminal_reason.map(str::to_owned),
                });
            }
            Err((error, _)) => return Err(error),
        };
    let (interpreter_ns, interpreter_executions) =
        match measure_interpreter_with_count(cases, oracles, WARMUP_SWEEPS, started) {
            Ok(progress) => progress,
            Err((error, executions)) if error.terminal_reason.is_some() => {
                return Ok(WarmupProgress {
                    interpreter_ns: None,
                    native_ns: Some(native_ns),
                    interpreter_executions: executions,
                    native_executions,
                    stop_reason: error.terminal_reason.map(str::to_owned),
                });
            }
            Err((error, _)) => return Err(error),
        };
    Ok(WarmupProgress {
        interpreter_ns: Some(interpreter_ns),
        native_ns: Some(native_ns),
        interpreter_executions,
        native_executions,
        stop_reason: None,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_native(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    published: &od_host::PublishedCode,
    repetitions: u32,
    started: Instant,
) -> Result<u128, cold::BenchError> {
    measure_native_with_count(cases, oracles, published, repetitions, started)
        .map(|(elapsed, _)| elapsed)
        .map_err(|(error, _)| error)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_native_with_count(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    published: &od_host::PublishedCode,
    repetitions: u32,
    started: Instant,
) -> Result<(u128, usize), (cold::BenchError, usize)> {
    measure_side_with_count(cases, oracles, repetitions, started, true, |case| {
        let bytes = case.input.sequence.bytes();
        let mut fixture = NativeFixture::new(case, &bytes);
        fixture
            .execute(published)
            .map(|(outcome, elapsed)| (elapsed, outcome))
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_interpreter(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    repetitions: u32,
    started: Instant,
) -> Result<u128, cold::BenchError> {
    measure_interpreter_with_count(cases, oracles, repetitions, started)
        .map(|(elapsed, _)| elapsed)
        .map_err(|(error, _)| error)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_interpreter_with_count(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    repetitions: u32,
    started: Instant,
) -> Result<(u128, usize), (cold::BenchError, usize)> {
    measure_side_with_count(cases, oracles, repetitions, started, false, |case| {
        let bytes = case.input.sequence.bytes();
        let mut fixture = InterpreterFixture::new(case, &bytes);
        let (outcome, elapsed) = fixture.execute();
        Ok((elapsed, outcome))
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_side_with_count<F>(
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    repetitions: u32,
    started: Instant,
    native: bool,
    mut execute: F,
) -> Result<(u128, usize), (cold::BenchError, usize)>
where
    F: FnMut(&cold::GeneratedCase) -> Result<(u128, cold::Outcome), cold::BenchError>,
{
    if cases.len() != oracles.len() {
        return Err((cold::err("warm result/oracle case counts differ"), 0));
    }
    let mut total_ns = 0u128;
    let mut completed = 0usize;
    for _ in 0..repetitions {
        for (index, (case, oracle)) in cases.iter().zip(oracles).enumerate() {
            if index % 64 == 0 {
                cold::check_run_bound(started).map_err(|error| (error, completed))?;
            }
            let (elapsed_ns, outcome) = execute(case).map_err(|error| (error, completed))?;
            cold::validate_pair(oracle, &outcome, &case.input, case.seed, case.index, native)
                .map_err(|error| (error, completed))?;
            total_ns = total_ns.saturating_add(elapsed_ns);
            completed += 1;
        }
    }
    Ok((total_ns, completed))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn calibrate_sequence(
    sequence: MemoryCorpusSequence,
    cases: &[&cold::GeneratedCase],
    oracles: &[cold::Outcome],
    published: &od_host::PublishedCode,
    started: Instant,
    journal: &mut std::io::BufWriter<std::fs::File>,
    process_run: u8,
) -> Result<(u32, Vec<CalibrationAttempt>, bool, Option<String>), cold::BenchError> {
    let mut repetitions = 1;
    let mut attempts = Vec::new();
    loop {
        let measured = if attempts.len() % 2 == usize::from(process_run % 2 == 0) {
            measure_interpreter(cases, oracles, repetitions, started).and_then(|interpreter| {
                measure_native(cases, oracles, published, repetitions, started)
                    .map(|native| (native, interpreter))
            })
        } else {
            measure_native(cases, oracles, published, repetitions, started).and_then(|native| {
                measure_interpreter(cases, oracles, repetitions, started)
                    .map(|interpreter| (native, interpreter))
            })
        };
        let pair = match measured {
            Ok(pair) => pair,
            Err(error) if error.terminal_reason.is_some() => {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_during_calibration".into()),
                ));
            }
            Err(error) => return Err(error),
        };
        let attempt = CalibrationAttempt {
            record_type: "calibration_attempt",
            sequence: sequence_name(sequence),
            repetitions,
            native_total_ns: pair.0,
            interpreter_total_ns: pair.1,
            native_floor_met: pair.0 >= MIN_SIDE_TIME_NS,
            interpreter_floor_met: pair.1 >= MIN_SIDE_TIME_NS,
        };
        cold::append_durable_journal_record(journal, &attempt)?;
        attempts.push(attempt);
        if let Err(error) = cold::check_run_bound(started) {
            if error.terminal_reason.is_some() {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_after_calibration".into()),
                ));
            }
            return Err(error);
        }
        if cold::calibration_attempt_meets_floor(pair.0, pair.1) {
            return Ok((repetitions, attempts, true, None));
        }
        let Some(next) = repetitions.checked_mul(2).filter(|n| *n <= MAX_REPETITIONS) else {
            return Ok((
                repetitions,
                attempts,
                false,
                Some("maximum_repetitions".into()),
            ));
        };
        repetitions = next;
    }
}

fn classify_status(
    calibration_only: bool,
    calibration_passed: bool,
    sample_passed: bool,
    mut stop_reason: Option<String>,
    elapsed_ns: u128,
) -> (&'static str, Option<String>, bool) {
    if elapsed_ns >= MAX_RUN_TIME_NS {
        stop_reason = Some("overall_wall_time_ceiling_after_final_sync".into());
    }
    let status = if stop_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("overall_wall_time_ceiling"))
    {
        "diagnostic_only_overall_deadline"
    } else if stop_reason.as_deref() == Some("maximum_repetitions") {
        "diagnostic_only_calibration_repetition_limit"
    } else if calibration_only && calibration_passed {
        "calibration_only_unscored"
    } else if !calibration_passed {
        "diagnostic_only_calibration_floor_failed"
    } else if sample_passed {
        "scored"
    } else {
        "diagnostic_only_sample_floor_failed"
    };
    (
        status,
        stop_reason,
        sample_passed && elapsed_ns < MAX_RUN_TIME_NS,
    )
}

fn warmup_summary(
    native_executions: usize,
    interpreter_executions: usize,
    expected_executions_per_backend: usize,
) -> (Option<u32>, bool) {
    let complete = expected_executions_per_backend != 0
        && native_executions == expected_executions_per_backend
        && interpreter_executions == expected_executions_per_backend;
    (complete.then_some(WARMUP_SWEEPS), complete)
}

fn sequence_name(sequence: MemoryCorpusSequence) -> &'static str {
    match sequence {
        MemoryCorpusSequence::StoreThenLoad => "store_then_load",
        MemoryCorpusSequence::LoadThenStore => "load_then_store",
        MemoryCorpusSequence::LoadThenLoad => "load_then_load_excluded",
    }
}

fn source_sha256() -> String {
    let mut bytes = Vec::new();
    for source in [
        WARM_SOURCE,
        COLD_SOURCE,
        CLI_SOURCE,
        LIB_SOURCE,
        JIT_SOURCE,
        HOST_SOURCE,
        ISA_SOURCE,
        TESTKIT_SOURCE,
    ] {
        bytes.extend_from_slice(&(source.len() as u64).to_le_bytes());
        bytes.extend_from_slice(source);
    }
    od_testkit::sha256_hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_REPETITIONS, MAX_RUN_TIME_NS, PREFLIGHT_TRIALS, SAMPLE_PAIRS, WARMUP_SWEEPS,
        classify_status, sequence_name, warmup_summary,
    };
    use od_testkit::memory_corpus::{MemoryCorpusSequence, S3_34A_SEQUENCES};

    #[test]
    fn sequence_strata_are_the_frozen_two_and_never_aggregate() {
        assert_eq!(S3_34A_SEQUENCES.len(), 2);
        assert_eq!(sequence_name(S3_34A_SEQUENCES[0]), "store_then_load");
        assert_eq!(sequence_name(S3_34A_SEQUENCES[1]), "load_then_store");
        assert_eq!(
            sequence_name(MemoryCorpusSequence::LoadThenLoad),
            "load_then_load_excluded"
        );
        assert_eq!(SAMPLE_PAIRS, 20);
        assert_eq!(PREFLIGHT_TRIALS, 2);
        assert_eq!(WARMUP_SWEEPS, 1);
        assert_eq!(MAX_REPETITIONS, 2048);
    }

    #[test]
    fn calibration_only_never_reports_scored_samples() {
        let (status, stop, sample_passed) = classify_status(true, true, false, None, 1);
        assert_eq!(status, "calibration_only_unscored");
        assert_eq!(stop, None);
        assert!(!sample_passed);
    }

    #[test]
    fn short_sample_or_wall_limit_is_diagnostic() {
        let (status, _, sample_passed) = classify_status(false, true, false, None, 1);
        assert_eq!(status, "diagnostic_only_sample_floor_failed");
        assert!(!sample_passed);
        let (status, stop, sample_passed) =
            classify_status(false, true, true, None, MAX_RUN_TIME_NS);
        assert_eq!(status, "diagnostic_only_overall_deadline");
        assert_eq!(
            stop.as_deref(),
            Some("overall_wall_time_ceiling_after_final_sync")
        );
        assert!(!sample_passed);
    }

    #[test]
    fn interrupted_warmup_does_not_claim_a_complete_sweep() {
        assert_eq!(warmup_summary(17, 0, 1200), (None, false));
        assert_eq!(warmup_summary(1200, 1200, 1200), (Some(1), true));
    }
}
