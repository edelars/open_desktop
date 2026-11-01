//! Cold compile/publish/execute/drop comparison for the frozen S3-34a memory subset.
//!
//! This deliberately bypasses VMHost and dispatch/cache layers. Every native
//! input owns a fresh artifact whose complete lifecycle is inside its timed
//! interval; interpreter execution is measured over the same deterministic
//! inputs and validated against the same preflight oracle.

use std::hint::black_box;
use std::path::Path;
use std::sync::atomic::AtomicU8;
use std::time::{Duration, Instant};

use od_testkit::memory_corpus::{
    MEMORY_CORPUS_RAM_LEN, MemoryCorpusInput, MemoryCorpusSequence, S3_34A_CASES_PER_SEED,
    S3_34A_SEEDS, S3_34A_SEQUENCES, generate_memory_corpus_input,
};
use serde::Serialize;

use crate::HostMetadata;

const PROFILE_ID: &str = "s3-35c-two-site-memory-cold-direct-v1";
pub(super) const ENTRY_RIP: u64 = 0x9e00;
pub(super) const INSTRUCTION_BUDGET: u64 = 2;
pub(super) const EXPECTED_CASES: usize = 2 * 3 * 400;
pub(super) const PREFLIGHT_REPETITIONS_PER_CASE: usize = 2;
const FROZEN_INPUT_DIGEST: &str =
    "b09478b556aa91234d8c453f20e70f637222c5de2d7369d305ffe7c78caae540";
pub(super) const MIN_SIDE_TIME: Duration = Duration::from_millis(200);
pub(super) const MAX_REPETITIONS: u32 = 2048;
pub(super) const MAX_SAMPLE_PAIRS: usize = 20;
pub(super) const MAX_RUN_TIME: Duration = Duration::from_secs(30 * 60);
const CODE_SOURCE: &[u8] = include_bytes!("two_site_memory_cold.rs");
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
    case_count: usize,
    input_digest: String,
    source_sha256: String,
    measure_host: HostMetadata,
    preflight_parity_cases: usize,
    unique_cases: usize,
    repetitions_per_case: usize,
    interpreter_executions: usize,
    native_executions: usize,
    frozen_seed_values: [u64; 3],
    sequence_counts: [usize; 2],
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
    case_count: usize,
    input_digest: String,
    source_sha256: String,
    journal_path: String,
    measure_host: HostMetadata,
    preflight_parity_cases: usize,
    preflight_unique_cases: usize,
    preflight_repetitions_per_case: usize,
    interpreter_preflight_executions: usize,
    native_preflight_executions: usize,
    frozen_seed_values: [u64; 3],
    sequence_counts: [usize; 2],
    repetitions_per_side: u32,
    calibration_floor_passed: bool,
    calibration_attempts: Vec<CalibrationAttempt>,
    sample_pairs: Vec<SamplePair>,
    minimum_side_time_ns: u128,
    sample_floor_passed: bool,
    sample_floor_policy: &'static str,
    timer_boundaries: &'static str,
    total_elapsed_ns: u128,
}

#[derive(Clone, Debug, Serialize)]
struct CalibrationAttempt {
    record_type: &'static str,
    repetitions: u32,
    native_total_ns: u128,
    interpreter_total_ns: u128,
    native_floor_met: bool,
    interpreter_floor_met: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SamplePair {
    record_type: &'static str,
    pair_index: usize,
    interpreter_first: bool,
    repetitions: u32,
    native_total_ns: u128,
    interpreter_total_ns: u128,
    /// Native lifecycle time divided by interpreter execution time, computed
    /// from unrounded raw totals (common repetition count cancels).
    native_over_interpreter: f64,
    native_floor_met: bool,
    interpreter_floor_met: bool,
}

#[derive(Clone)]
pub(super) struct GeneratedCase {
    pub(super) seed: u64,
    pub(super) index: usize,
    pub(super) input: MemoryCorpusInput,
}

#[derive(Clone)]
struct Case {
    seed: u64,
    index: usize,
    input: MemoryCorpusInput,
    oracle: Outcome,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Outcome {
    pub(super) state: od_isa::LongModeState,
    pub(super) ram: [u8; MEMORY_CORPUS_RAM_LEN],
    pub(super) exit: String,
    pub(super) retired: u64,
    pub(super) remaining_budget: u64,
    pub(super) memory_events: [Option<MemoryEvent>; 2],
    pub(super) memory_event_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MemoryOperation {
    Read,
    Write,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MemoryEvent {
    pub(super) operation: MemoryOperation,
    pub(super) address: u64,
    pub(super) width: usize,
}

pub(super) struct FixtureMemory<'a> {
    pub(super) code: &'a [u8],
    pub(super) ram: [u8; MEMORY_CORPUS_RAM_LEN],
    pub(super) memory_events: [Option<MemoryEvent>; 2],
    pub(super) memory_event_count: usize,
}

impl od_isa::memory::GuestMemory for FixtureMemory<'_> {
    fn fetch(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
    ) -> Result<u8, od_isa::memory::MemoryError> {
        address
            .checked_sub(ENTRY_RIP)
            .and_then(|offset| self.code.get(offset as usize))
            .copied()
            .ok_or(od_isa::memory::MemoryError::Unmapped {
                address,
                access: od_isa::memory::AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
        destination: &mut [u8],
    ) -> Result<(), od_isa::memory::MemoryError> {
        let range =
            self.data_range(address, destination.len(), od_isa::memory::AccessKind::Read)?;
        self.ensure_event_slot(address, od_isa::memory::AccessKind::Read)?;
        destination.copy_from_slice(&self.ram[range]);
        self.record_event(MemoryOperation::Read, address, destination.len());
        Ok(())
    }

    fn write(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
        source: &[u8],
    ) -> Result<(), od_isa::memory::MemoryError> {
        let range = self.data_range(address, source.len(), od_isa::memory::AccessKind::Write)?;
        self.ensure_event_slot(address, od_isa::memory::AccessKind::Write)?;
        self.ram[range].copy_from_slice(source);
        self.record_event(MemoryOperation::Write, address, source.len());
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, od_isa::memory::MemoryError> {
        Err(self.unmapped(address, od_isa::memory::AccessKind::Read))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
        _: od_isa::OperandSize,
        _: u64,
    ) -> Result<u64, od_isa::memory::MemoryError> {
        Err(self.unmapped(address, od_isa::memory::AccessKind::Read))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        _: od_isa::memory::AccessContext,
        _: od_isa::OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, od_isa::memory::MemoryError> {
        Err(self.unmapped(address, od_isa::memory::AccessKind::Read))
    }

    fn mfence(
        &mut self,
        _: od_isa::memory::AccessContext,
    ) -> Result<(), od_isa::memory::MemoryError> {
        Ok(())
    }
}

impl FixtureMemory<'_> {
    fn ensure_event_slot(
        &self,
        address: u64,
        access: od_isa::memory::AccessKind,
    ) -> Result<(), od_isa::memory::MemoryError> {
        if self.memory_event_count < self.memory_events.len() {
            Ok(())
        } else {
            Err(self.unmapped(address, access))
        }
    }

    fn record_event(&mut self, operation: MemoryOperation, address: u64, width: usize) {
        self.memory_events[self.memory_event_count] = Some(MemoryEvent {
            operation,
            address,
            width,
        });
        self.memory_event_count += 1;
    }

    fn data_range(
        &self,
        address: u64,
        length: usize,
        access: od_isa::memory::AccessKind,
    ) -> Result<std::ops::Range<usize>, od_isa::memory::MemoryError> {
        let start = usize::try_from(address).map_err(|_| self.unmapped(address, access))?;
        let end = start
            .checked_add(length)
            .ok_or_else(|| self.unmapped(address, access))?;
        if end > self.ram.len() {
            return Err(self.unmapped(address, access));
        }
        Ok(start..end)
    }

    fn unmapped(
        &self,
        address: u64,
        access: od_isa::memory::AccessKind,
    ) -> od_isa::memory::MemoryError {
        od_isa::memory::MemoryError::Unmapped { address, access }
    }
}

pub fn preflight(process_run: u8) -> Result<PreflightReport, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = process_run;
        Err("two-site-memory-cold requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        validate_process_run(process_run)?;
        let (cases, input_digest, interpreter_executions, native_executions) =
            build_and_preflight()?;
        let sequence_counts = count_sequences(&cases);
        Ok(PreflightReport {
            schema: 1,
            profile_id: PROFILE_ID,
            process_run,
            timing_started: false,
            case_count: cases.len(),
            input_digest,
            source_sha256: source_sha256(),
            measure_host: HostMetadata::current(),
            preflight_parity_cases: cases.len() * PREFLIGHT_REPETITIONS_PER_CASE,
            unique_cases: cases.len(),
            repetitions_per_case: PREFLIGHT_REPETITIONS_PER_CASE,
            interpreter_executions,
            native_executions,
            frozen_seed_values: S3_34A_SEEDS,
            sequence_counts,
        })
    }
}

pub fn run(process_run: u8, journal_path: &Path) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (process_run, journal_path);
        Err("two-site-memory-cold requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(process_run, journal_path, false)
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

pub fn calibration_only(
    process_run: u8,
    journal_path: &Path,
) -> Result<Report, Box<dyn std::error::Error>> {
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (process_run, journal_path);
        Err("two-site-memory-cold requires native Apple Silicon macOS execution".into())
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    run_native(process_run, journal_path, true)
        .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_native(
    process_run: u8,
    journal_path: &Path,
    calibration_only: bool,
) -> Result<Report, BenchError> {
    use std::fs::OpenOptions;
    use std::io::Write;

    validate_process_run(process_run)?;
    let started = Instant::now();
    let (cases, input_digest, interpreter_preflight_executions, native_preflight_executions) =
        build_and_preflight()?;
    let sequence_counts = count_sequences(&cases);
    if let Some(parent) = journal_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(bench_error)?;
    }
    let journal_file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(journal_path)
        .map_err(bench_error)?;
    let mut journal = std::io::BufWriter::new(journal_file);
    let (repetitions, calibration_attempts, calibration_floor_passed, mut stop_reason) =
        calibrate(&cases, started, &mut journal)?;
    let mut sample_pairs = Vec::with_capacity(MAX_SAMPLE_PAIRS);
    let mut sample_floor_passed = !calibration_only && calibration_floor_passed;

    for pair_index in 0..if should_collect_sample_pairs(
        calibration_only,
        calibration_floor_passed,
        stop_reason.is_some(),
    ) {
        MAX_SAMPLE_PAIRS
    } else {
        0
    } {
        if let Err(error) = check_run_bound(started) {
            if let Some(reason) = error.terminal_reason {
                stop_reason = Some(reason.to_owned());
                sample_floor_passed = false;
                break;
            }
            return Err(error);
        }
        let interpreter_first = (pair_index + usize::from(process_run % 2 == 0)) % 2 == 0;
        let measured_pair = if interpreter_first {
            measure_sweeps(&cases, repetitions, false, started).and_then(|interpreter| {
                measure_sweeps(&cases, repetitions, true, started)
                    .map(|native| (native, interpreter))
            })
        } else {
            measure_sweeps(&cases, repetitions, true, started).and_then(|native| {
                measure_sweeps(&cases, repetitions, false, started)
                    .map(|interpreter| (native, interpreter))
            })
        };
        let (native_total_ns, interpreter_total_ns) = match measured_pair {
            Ok(pair) => pair,
            Err(error) if error.terminal_reason.is_some() => {
                stop_reason = Some(error.terminal_reason.unwrap().to_owned());
                sample_floor_passed = false;
                break;
            }
            Err(error) => return Err(error),
        };
        let native_floor_met = native_total_ns >= MIN_SIDE_TIME.as_nanos();
        let interpreter_floor_met = interpreter_total_ns >= MIN_SIDE_TIME.as_nanos();
        sample_floor_passed &= native_floor_met && interpreter_floor_met;
        let pair = SamplePair {
            record_type: "sample_pair",
            pair_index,
            interpreter_first,
            repetitions,
            native_total_ns,
            interpreter_total_ns,
            native_over_interpreter: native_total_ns as f64 / interpreter_total_ns as f64,
            native_floor_met,
            interpreter_floor_met,
        };
        append_durable_journal_record(&mut journal, &pair)?;
        sample_pairs.push(pair);
        if let Err(error) = check_run_bound(started) {
            if let Some(reason) = error.terminal_reason {
                stop_reason = Some(reason.to_owned());
                sample_floor_passed = false;
                break;
            }
            return Err(error);
        }
    }

    let measure_host = HostMetadata::current();
    let source_sha256 = source_sha256();
    journal.flush().map_err(bench_error)?;
    journal.get_ref().sync_data().map_err(bench_error)?;
    drop(journal);
    let case_count = cases.len();
    drop(cases);
    let total_elapsed_ns = started.elapsed().as_nanos();
    let (status, stop_reason, sample_floor_passed) = classify_final_status(
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
        stop_reason,
        calibration_only,
        timing_started: true,
        case_count,
        input_digest,
        source_sha256,
        journal_path: journal_path.display().to_string(),
        measure_host,
        preflight_parity_cases: case_count * PREFLIGHT_REPETITIONS_PER_CASE,
        preflight_unique_cases: case_count,
        preflight_repetitions_per_case: PREFLIGHT_REPETITIONS_PER_CASE,
        interpreter_preflight_executions,
        native_preflight_executions,
        frozen_seed_values: S3_34A_SEEDS,
        sequence_counts,
        repetitions_per_side: repetitions,
        calibration_floor_passed,
        calibration_attempts,
        sample_pairs,
        minimum_side_time_ns: MIN_SIDE_TIME.as_nanos(),
        sample_floor_passed,
        sample_floor_policy: "Retain all 20 raw pairs. Any side below 200ms classifies the entire process diagnostic-only; no partial scoring or trimming.",
        timer_boundaries: "Native per-input timer includes compile_ir_memory_two_site_block_v2, publish_ir_memory_two_site_v2, direct typed execute_ir_memory_two_site_v2, and explicit PublishedCode plus IR block drops. Interpreter timer includes only od_isa::execute::run. State/RAM reset and full-result validation are outside each timer. Each backend must report exactly two ordered 8-byte data callbacks at the sequence's expected addresses. Direct Complete or BudgetExhausted is normalized to interpreter InstructionBudgetExhausted only when all CPU state and RAM match, RIP is code_end, retired count is 2, and remaining budget is 0.",
        total_elapsed_ns,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
#[derive(Debug)]
pub(super) struct BenchError {
    pub(super) message: String,
    pub(super) terminal_reason: Option<&'static str>,
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl std::fmt::Display for BenchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl std::error::Error for BenchError {}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn err(message: impl Into<String>) -> BenchError {
    BenchError {
        message: message.into(),
        terminal_reason: None,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn bench_error(error: impl std::fmt::Debug) -> BenchError {
    err(format!("{error:?}"))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn append_durable_journal_record(
    journal: &mut std::io::BufWriter<std::fs::File>,
    record: &impl Serialize,
) -> Result<(), BenchError> {
    let sync_file = journal.get_ref().try_clone().map_err(bench_error)?;
    append_record_with_sync(journal, record, || sync_file.sync_data()).map_err(err)
}

fn append_record_with_sync<W, F>(
    writer: &mut W,
    record: &impl Serialize,
    synchronize: F,
) -> Result<(), String>
where
    W: std::io::Write,
    F: FnOnce() -> std::io::Result<()>,
{
    serde_json::to_writer(&mut *writer, record).map_err(|error| error.to_string())?;
    writer.write_all(b"\n").map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    synchronize().map_err(|error| error.to_string())
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn validate_process_run(process_run: u8) -> Result<(), BenchError> {
    if (1..=3).contains(&process_run) {
        Ok(())
    } else {
        Err(err("process-run must be 1..=3"))
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn generate_inputs() -> Result<(Vec<GeneratedCase>, String), BenchError> {
    let mut cases = Vec::with_capacity(EXPECTED_CASES);
    let mut digest = Vec::with_capacity(EXPECTED_CASES * 256);
    for sequence in S3_34A_SEQUENCES {
        for seed in S3_34A_SEEDS {
            for index in 0..S3_34A_CASES_PER_SEED {
                let input = generate_memory_corpus_input(seed, index, Some(sequence));
                digest.extend_from_slice(&sequence.bytes());
                digest.extend_from_slice(&seed.to_le_bytes());
                digest.extend_from_slice(&(index as u64).to_le_bytes());
                digest.extend_from_slice(&input.flags.to_le_bytes());
                digest.extend_from_slice(&input.first_address.to_le_bytes());
                digest.extend_from_slice(&input.second_address.to_le_bytes());
                for value in input.gpr {
                    digest.extend_from_slice(&value.to_le_bytes());
                }
                digest.extend_from_slice(&input.ram);
                if index != cases.len() % S3_34A_CASES_PER_SEED
                    || seed != S3_34A_SEEDS[(cases.len() / S3_34A_CASES_PER_SEED) % 3]
                    || sequence != S3_34A_SEQUENCES[cases.len() / (3 * S3_34A_CASES_PER_SEED)]
                {
                    return Err(err("frozen sequence/seed/index iteration order changed"));
                }
                cases.push(GeneratedCase { seed, index, input });
            }
        }
    }
    if cases.len() != EXPECTED_CASES
        || cases
            .iter()
            .any(|case| !S3_34A_SEQUENCES.contains(&case.input.sequence))
    {
        return Err(err(
            "generated corpus differs from exact frozen 2x3x400 subset",
        ));
    }
    let input_digest = od_testkit::sha256_hex(&digest);
    if input_digest != FROZEN_INPUT_DIGEST {
        return Err(err(format!(
            "frozen S3-34a input digest changed: {input_digest}"
        )));
    }
    Ok((cases, input_digest))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn build_and_preflight() -> Result<(Vec<Case>, String, usize, usize), BenchError> {
    let (generated, input_digest) = generate_inputs()?;
    let mut cases = Vec::with_capacity(generated.len());
    let mut interpreter_executions = 0usize;
    let mut native_executions = 0usize;
    for generated_case in generated {
        let GeneratedCase { seed, index, input } = generated_case;
        let mut oracle = None::<Outcome>;
        let mut first_native = None::<Outcome>;
        for repetition in 0..PREFLIGHT_REPETITIONS_PER_CASE {
            // Both functions rebuild CPU state and private RAM from the
            // immutable input, so every trial starts from a fresh reset.
            let interpreter = interpret(&input)?;
            interpreter_executions += 1;
            let native = native_once(&input)?;
            native_executions += 1;
            validate_pair(&interpreter, &native, &input, seed, index, true)?;
            if repetition == 0 {
                oracle = Some(interpreter);
                first_native = Some(native);
            } else {
                let first_interpreter = oracle
                    .as_ref()
                    .expect("first interpreter trial is retained");
                let first_native = first_native
                    .as_ref()
                    .expect("first native trial is retained");
                if !repeat_outcomes_equal(first_interpreter, &interpreter) {
                    return Err(err(format!(
                        "interpreter repeat mismatch for {:?}, seed={seed}, index={index}",
                        input.sequence
                    )));
                }
                if !repeat_outcomes_equal(first_native, &native) {
                    return Err(err(format!(
                        "native repeat mismatch for {:?}, seed={seed}, index={index}",
                        input.sequence
                    )));
                }
            }
        }
        cases.push(Case {
            seed,
            index,
            input,
            oracle: oracle.expect("exactly two interpreter trials were run"),
        });
    }
    let expected_backend_executions =
        preflight_execution_count(EXPECTED_CASES, PREFLIGHT_REPETITIONS_PER_CASE)
            .ok_or_else(|| err("preflight execution count overflow"))?;
    if interpreter_executions != expected_backend_executions
        || native_executions != expected_backend_executions
    {
        return Err(err(format!(
            "preflight backend execution counts differ from exact repeated matrix: interpreter={interpreter_executions}, native={native_executions}, expected={expected_backend_executions}"
        )));
    }
    Ok((
        cases,
        input_digest,
        interpreter_executions,
        native_executions,
    ))
}

pub(super) fn preflight_execution_count(
    unique_cases: usize,
    repetitions_per_case: usize,
) -> Option<usize> {
    unique_cases.checked_mul(repetitions_per_case)
}

pub(super) fn repeat_outcomes_equal(first: &Outcome, second: &Outcome) -> bool {
    first == second
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn make_state(input: &MemoryCorpusInput) -> od_isa::LongModeState {
    let mut state = od_isa::LongModeState {
        rip: ENTRY_RIP,
        rflags: input.flags | 2,
        ..od_isa::LongModeState::default()
    };
    state.gpr = input.gpr;
    state
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn interpret(input: &MemoryCorpusInput) -> Result<Outcome, BenchError> {
    let mut state = make_state(input);
    let bytes = input.sequence.bytes();
    let mut memory = FixtureMemory {
        code: &bytes,
        ram: input.ram,
        memory_events: [None; 2],
        memory_event_count: 0,
    };
    let result = execute_interpreter(&mut state, &mut memory);
    Ok(outcome_from_interpreter(&state, &memory, result))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn execute_interpreter(
    state: &mut od_isa::LongModeState,
    memory: &mut FixtureMemory<'_>,
) -> od_isa::execute::RunResult {
    od_isa::execute::run(
        state,
        memory,
        od_isa::execute::RunConfig {
            instruction_budget: INSTRUCTION_BUDGET,
            stop_rip: None,
            context: od_isa::memory::AccessContext::default(),
        },
        || false,
    )
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn outcome_from_interpreter(
    state: &od_isa::LongModeState,
    memory: &FixtureMemory<'_>,
    result: od_isa::execute::RunResult,
) -> Outcome {
    Outcome {
        state: state.clone(),
        ram: memory.ram,
        exit: format!("{:?}", result.exit),
        retired: result.instructions_executed,
        remaining_budget: INSTRUCTION_BUDGET.saturating_sub(result.instructions_executed),
        memory_events: memory.memory_events,
        memory_event_count: memory.memory_event_count,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn native_once(input: &MemoryCorpusInput) -> Result<Outcome, BenchError> {
    let bytes = input.sequence.bytes();
    let code = od_jit::block::CodeView::new(ENTRY_RIP, &bytes).map_err(bench_error)?;
    let block = od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(code, ENTRY_RIP)
        .map_err(bench_error)?;
    let published =
        od_host::PublishedCode::publish_ir_memory_two_site_v2(&block).map_err(bench_error)?;
    let mut state = make_state(input);
    let mut memory = FixtureMemory {
        code: &bytes,
        ram: input.ram,
        memory_events: [None; 2],
        memory_event_count: 0,
    };
    let stop_requested = AtomicU8::new(0);
    let result = execute_published(&published, &mut state, &mut memory, &stop_requested)?;
    let outcome = outcome_from_native(&state, &memory, result);
    drop(published);
    drop(block);
    Ok(outcome)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn execute_published(
    published: &od_host::PublishedCode,
    state: &mut od_isa::LongModeState,
    memory: &mut FixtureMemory<'_>,
    stop_requested: &AtomicU8,
) -> Result<od_host::MemoryOperationExecutionResultV2, BenchError> {
    // SAFETY: the data adapter is private synchronous ordinary RAM, disjoint
    // from the immutable six-byte code source, all-or-nothing on writes, and
    // accessed synchronously with exclusive state and memory borrows.
    let result = unsafe {
        published.execute_ir_memory_two_site_v2(
            state,
            memory,
            od_isa::memory::AccessContext::default(),
            INSTRUCTION_BUDGET,
            None,
            stop_requested,
        )
    }
    .map_err(bench_error)?;
    Ok(result)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn outcome_from_native(
    state: &od_isa::LongModeState,
    memory: &FixtureMemory<'_>,
    result: od_host::MemoryOperationExecutionResultV2,
) -> Outcome {
    Outcome {
        state: state.clone(),
        ram: memory.ram,
        exit: format!("{:?}", result.exit),
        retired: result.retired_instructions,
        remaining_budget: result.remaining_budget,
        memory_events: memory.memory_events,
        memory_event_count: memory.memory_event_count,
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn validate_pair(
    expected: &Outcome,
    actual: &Outcome,
    input: &MemoryCorpusInput,
    seed: u64,
    index: usize,
    native: bool,
) -> Result<(), BenchError> {
    let sequence = input.sequence;
    let expected_events = expected_memory_events(sequence, input.gpr[3], input.gpr[2])
        .ok_or_else(|| err("LoadThenLoad is outside the frozen S3-34a selection"))?;
    let exit_matches = if native {
        actual.exit == "Complete" || actual.exit == "BudgetExhausted"
    } else {
        actual.exit == expected.exit
    };
    if expected.state != actual.state
        || expected.ram != actual.ram
        || expected.retired != actual.retired
        || expected.remaining_budget != actual.remaining_budget
        || expected.retired != 2
        || expected.remaining_budget != 0
        || expected.state.rip != ENTRY_RIP + 6
        || expected.exit != "InstructionBudgetExhausted"
        || !exit_matches
        || expected.memory_event_count != 2
        || actual.memory_event_count != 2
        || expected.memory_events != expected_events
        || actual.memory_events != expected_events
        || actual.state.rip != ENTRY_RIP + 6
        || actual.retired != 2
        || actual.remaining_budget != 0
    {
        return Err(err(format!(
            "full-state/RAM/ordered-memory-event/exit parity failed for {sequence:?}, seed={seed}, index={index}: interpreter={expected:?}, actual={actual:?}"
        )));
    }
    Ok(())
}

pub(super) fn expected_memory_events(
    sequence: MemoryCorpusSequence,
    first_address: u64,
    second_address: u64,
) -> Option<[Option<MemoryEvent>; 2]> {
    Some(match sequence {
        MemoryCorpusSequence::StoreThenLoad => [
            Some(MemoryEvent {
                operation: MemoryOperation::Write,
                address: first_address,
                width: 8,
            }),
            Some(MemoryEvent {
                operation: MemoryOperation::Read,
                address: first_address,
                width: 8,
            }),
        ],
        MemoryCorpusSequence::LoadThenStore => [
            Some(MemoryEvent {
                operation: MemoryOperation::Read,
                address: first_address,
                width: 8,
            }),
            Some(MemoryEvent {
                operation: MemoryOperation::Write,
                address: second_address,
                width: 8,
            }),
        ],
        MemoryCorpusSequence::LoadThenLoad => return None,
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn count_sequences(cases: &[Case]) -> [usize; 2] {
    let mut counts = [0, 0];
    for case in cases {
        match case.input.sequence {
            MemoryCorpusSequence::StoreThenLoad => counts[0] += 1,
            MemoryCorpusSequence::LoadThenStore => counts[1] += 1,
            MemoryCorpusSequence::LoadThenLoad => unreachable!("excluded S3-34a sequence"),
        }
    }
    counts
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn source_sha256() -> String {
    let mut bytes = Vec::new();
    for source in [
        CODE_SOURCE,
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

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn calibrate(
    cases: &[Case],
    started: Instant,
    journal: &mut std::io::BufWriter<std::fs::File>,
) -> Result<(u32, Vec<CalibrationAttempt>, bool, Option<String>), BenchError> {
    let mut repetitions = 1;
    let mut attempts = Vec::new();
    loop {
        let pair = if attempts.len() % 2 == 0 {
            let Some(interpreter) = measure_until_deadline(cases, repetitions, false, started)?
            else {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_during_calibration".into()),
                ));
            };
            let Some(native) = measure_until_deadline(cases, repetitions, true, started)? else {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_during_calibration".into()),
                ));
            };
            (native, interpreter)
        } else {
            let Some(native) = measure_until_deadline(cases, repetitions, true, started)? else {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_during_calibration".into()),
                ));
            };
            let Some(interpreter) = measure_until_deadline(cases, repetitions, false, started)?
            else {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_during_calibration".into()),
                ));
            };
            (native, interpreter)
        };
        let (native, interpreter) = pair;
        let attempt = CalibrationAttempt {
            record_type: "calibration_attempt",
            repetitions,
            native_total_ns: native,
            interpreter_total_ns: interpreter,
            native_floor_met: native >= MIN_SIDE_TIME.as_nanos(),
            interpreter_floor_met: interpreter >= MIN_SIDE_TIME.as_nanos(),
        };
        append_durable_journal_record(journal, &attempt)?;
        attempts.push(attempt);
        if let Err(error) = check_run_bound(started) {
            if error.terminal_reason.is_some() {
                return Ok((
                    repetitions,
                    attempts,
                    false,
                    Some("overall_wall_time_ceiling_after_calibration_journal_sync".into()),
                ));
            }
            return Err(error);
        }
        if calibration_attempt_meets_floor(native, interpreter) {
            return Ok((repetitions, attempts, true, None));
        }
        let Some(next_repetitions) = next_calibration_repetitions(repetitions) else {
            return Ok((
                repetitions,
                attempts,
                false,
                Some("maximum_repetitions".into()),
            ));
        };
        repetitions = next_repetitions;
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_until_deadline(
    cases: &[Case],
    repetitions: u32,
    native: bool,
    started: Instant,
) -> Result<Option<u128>, BenchError> {
    match measure_sweeps(cases, repetitions, native, started) {
        Ok(total) => Ok(Some(total)),
        Err(error) if error.terminal_reason.is_some() => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn measure_sweeps(
    cases: &[Case],
    repetitions: u32,
    native: bool,
    started: Instant,
) -> Result<u128, BenchError> {
    let mut total_ns = 0u128;
    for _ in 0..repetitions {
        check_run_bound(started)?;
        for (case_index, case) in cases.iter().enumerate() {
            if case_index % 64 == 0 {
                check_run_bound(started)?;
            }
            let (elapsed, outcome) = if native {
                let bytes = case.input.sequence.bytes();
                let code = od_jit::block::CodeView::new(ENTRY_RIP, &bytes).map_err(bench_error)?;
                let mut state = make_state(&case.input);
                let mut memory = FixtureMemory {
                    code: &bytes,
                    ram: case.input.ram,
                    memory_events: [None; 2],
                    memory_event_count: 0,
                };
                let stop_requested = AtomicU8::new(0);
                let start = Instant::now();
                let block =
                    od_jit::memory_load_v2::compile_ir_memory_two_site_block_v2(code, ENTRY_RIP)
                        .map_err(bench_error)?;
                let published = od_host::PublishedCode::publish_ir_memory_two_site_v2(&block)
                    .map_err(bench_error)?;
                // SAFETY: private disjoint synchronous fixture RAM and pinned
                // published entry satisfy the direct typed entry contract.
                let result = unsafe {
                    published.execute_ir_memory_two_site_v2(
                        &mut state,
                        &mut memory,
                        od_isa::memory::AccessContext::default(),
                        INSTRUCTION_BUDGET,
                        None,
                        &stop_requested,
                    )
                }
                .map_err(bench_error)?;
                black_box((&state, &memory.ram, &result));
                drop(published);
                drop(block);
                let elapsed = start.elapsed().as_nanos();
                let outcome = Outcome {
                    state,
                    ram: memory.ram,
                    exit: format!("{:?}", result.exit),
                    retired: result.retired_instructions,
                    remaining_budget: result.remaining_budget,
                    memory_events: memory.memory_events,
                    memory_event_count: memory.memory_event_count,
                };
                (elapsed, outcome)
            } else {
                let mut state = make_state(&case.input);
                let bytes = case.input.sequence.bytes();
                let mut memory = FixtureMemory {
                    code: &bytes,
                    ram: case.input.ram,
                    memory_events: [None; 2],
                    memory_event_count: 0,
                };
                let start = Instant::now();
                let result = od_isa::execute::run(
                    &mut state,
                    &mut memory,
                    od_isa::execute::RunConfig {
                        instruction_budget: INSTRUCTION_BUDGET,
                        stop_rip: None,
                        context: od_isa::memory::AccessContext::default(),
                    },
                    || false,
                );
                black_box((&state, &memory.ram, &result));
                let elapsed = start.elapsed().as_nanos();
                let outcome = Outcome {
                    state,
                    ram: memory.ram,
                    exit: format!("{:?}", result.exit),
                    retired: result.instructions_executed,
                    remaining_budget: INSTRUCTION_BUDGET
                        .saturating_sub(result.instructions_executed),
                    memory_events: memory.memory_events,
                    memory_event_count: memory.memory_event_count,
                };
                (elapsed, outcome)
            };
            validate_pair(
                &case.oracle,
                &outcome,
                &case.input,
                case.seed,
                case.index,
                native,
            )?;
            total_ns = total_ns.saturating_add(elapsed);
        }
    }
    Ok(total_ns)
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub(super) fn check_run_bound(started: Instant) -> Result<(), BenchError> {
    if wall_limit_exceeded(started.elapsed().as_nanos()) {
        Err(BenchError {
            message: "30 minute total run bound exceeded".into(),
            terminal_reason: Some("overall_wall_time_ceiling"),
        })
    } else {
        Ok(())
    }
}

fn wall_limit_exceeded(elapsed_ns: u128) -> bool {
    elapsed_ns >= MAX_RUN_TIME.as_nanos()
}

fn next_calibration_repetitions(current: u32) -> Option<u32> {
    current
        .checked_mul(2)
        .filter(|next| *next <= MAX_REPETITIONS)
}

pub(super) fn calibration_attempt_meets_floor(
    native_total_ns: u128,
    interpreter_total_ns: u128,
) -> bool {
    native_total_ns >= MIN_SIDE_TIME.as_nanos() && interpreter_total_ns >= MIN_SIDE_TIME.as_nanos()
}

fn classify_final_status(
    calibration_only: bool,
    calibration_floor_passed: bool,
    mut sample_floor_passed: bool,
    mut stop_reason: Option<String>,
    final_elapsed_ns: u128,
) -> (&'static str, Option<String>, bool) {
    if wall_limit_exceeded(final_elapsed_ns) {
        stop_reason = Some("overall_wall_time_ceiling_after_final_sync".into());
        sample_floor_passed = false;
    }
    let status = if stop_reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("overall_wall_time_ceiling"))
    {
        "diagnostic_only_overall_deadline"
    } else if stop_reason.as_deref() == Some("maximum_repetitions") {
        "diagnostic_only_calibration_repetition_limit"
    } else if calibration_only && calibration_floor_passed {
        "calibration_only_unscored"
    } else if sample_floor_passed {
        "scored"
    } else if !calibration_floor_passed {
        "diagnostic_only_calibration_floor_failed"
    } else {
        "diagnostic_only_sample_floor_failed"
    };
    (status, stop_reason, sample_floor_passed)
}

fn should_collect_sample_pairs(
    calibration_only: bool,
    calibration_floor_passed: bool,
    stop_requested: bool,
) -> bool {
    !calibration_only && calibration_floor_passed && !stop_requested
}

#[cfg(test)]
mod tests {
    use super::{
        EXPECTED_CASES, FixtureMemory, MAX_REPETITIONS, MIN_SIDE_TIME, MemoryCorpusSequence,
        MemoryEvent, MemoryOperation, Outcome, PREFLIGHT_REPETITIONS_PER_CASE,
        append_record_with_sync, calibration_attempt_meets_floor, classify_final_status,
        expected_memory_events, next_calibration_repetitions, preflight_execution_count,
        repeat_outcomes_equal, should_collect_sample_pairs, wall_limit_exceeded,
    };
    use od_testkit::memory_corpus::{S3_34A_CASES_PER_SEED, S3_34A_SEEDS, S3_34A_SEQUENCES};

    #[test]
    fn frozen_s3_34a_selection_is_exactly_2400_inputs() {
        assert_eq!(
            S3_34A_SEQUENCES.len() * S3_34A_SEEDS.len() * S3_34A_CASES_PER_SEED,
            2400
        );
        assert_eq!(EXPECTED_CASES, 2400);
    }

    #[test]
    fn preflight_runs_two_trials_per_backend_and_rejects_repeat_mismatch() {
        assert_eq!(PREFLIGHT_REPETITIONS_PER_CASE, 2);
        assert_eq!(
            preflight_execution_count(2400, PREFLIGHT_REPETITIONS_PER_CASE),
            Some(4800)
        );
        assert_eq!(preflight_execution_count(usize::MAX, 2), None);

        let first = Outcome {
            state: od_isa::LongModeState::default(),
            ram: [0; 128],
            exit: "InstructionBudgetExhausted".into(),
            retired: 2,
            remaining_budget: 0,
            memory_events: expected_memory_events(MemoryCorpusSequence::StoreThenLoad, 24, 40)
                .unwrap(),
            memory_event_count: 2,
        };
        assert!(repeat_outcomes_equal(&first, &first.clone()));

        let mut changed_state = first.clone();
        changed_state.state.gpr[0] = 1;
        assert!(!repeat_outcomes_equal(&first, &changed_state));

        let mut changed_trace = first.clone();
        changed_trace.memory_events[0] = Some(MemoryEvent {
            operation: MemoryOperation::Read,
            address: 24,
            width: 8,
        });
        assert!(!repeat_outcomes_equal(&first, &changed_trace));
    }

    #[test]
    fn sample_floor_and_repetition_bound_are_fixed() {
        assert_eq!(MIN_SIDE_TIME.as_millis(), 200);
        assert_eq!(MAX_REPETITIONS, 2048);
    }

    #[test]
    fn calibration_candidate_cap_includes_1024_and_one_reserve_doubling() {
        let mut candidates = vec![1];
        while let Some(next) = next_calibration_repetitions(*candidates.last().unwrap()) {
            candidates.push(next);
        }
        assert_eq!(
            candidates,
            [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048]
        );
        assert_eq!(next_calibration_repetitions(2048), None);

        // Deterministic arithmetic from the conservative 1024-repetition
        // projection. This checks the floor predicate, not machine timing.
        assert!(calibration_attempt_meets_floor(22_544_416_256, 304_472_064));
        assert!(!calibration_attempt_meets_floor(
            11_272_208_128,
            152_236_032
        ));
    }

    #[test]
    fn exact_callback_sequences_match_the_two_frozen_forms() {
        assert_eq!(
            expected_memory_events(MemoryCorpusSequence::StoreThenLoad, 0x28, 0x58),
            Some([
                Some(MemoryEvent {
                    operation: MemoryOperation::Write,
                    address: 0x28,
                    width: 8,
                }),
                Some(MemoryEvent {
                    operation: MemoryOperation::Read,
                    address: 0x28,
                    width: 8,
                }),
            ])
        );
        assert_eq!(
            expected_memory_events(MemoryCorpusSequence::LoadThenStore, 0x28, 0x58),
            Some([
                Some(MemoryEvent {
                    operation: MemoryOperation::Read,
                    address: 0x28,
                    width: 8,
                }),
                Some(MemoryEvent {
                    operation: MemoryOperation::Write,
                    address: 0x58,
                    width: 8,
                }),
            ])
        );
        assert_eq!(
            expected_memory_events(MemoryCorpusSequence::LoadThenLoad, 0x28, 0x58),
            None
        );
    }

    #[test]
    fn final_wall_limit_is_inclusive() {
        let limit = std::time::Duration::from_secs(30 * 60).as_nanos();
        assert!(!wall_limit_exceeded(limit - 1));
        assert!(wall_limit_exceeded(limit));
        assert!(wall_limit_exceeded(limit + 1));
    }

    #[test]
    fn post_sync_deadline_overrides_a_previously_scored_status() {
        let limit = std::time::Duration::from_secs(30 * 60).as_nanos();
        assert_eq!(
            classify_final_status(false, true, true, None, limit - 1),
            ("scored", None, true)
        );
        for elapsed in [limit, limit + 1] {
            let (status, reason, floor_passed) =
                classify_final_status(false, true, true, None, elapsed);
            assert_eq!(status, "diagnostic_only_overall_deadline");
            assert_eq!(
                reason.as_deref(),
                Some("overall_wall_time_ceiling_after_final_sync")
            );
            assert!(!floor_passed);
        }
    }

    #[test]
    fn calibration_only_finishes_unscored_before_sample_path() {
        assert!(!should_collect_sample_pairs(true, true, false));
        assert!(!should_collect_sample_pairs(true, false, false));
        assert!(!should_collect_sample_pairs(true, true, true));
        assert!(should_collect_sample_pairs(false, true, false));
        assert!(!should_collect_sample_pairs(false, false, false));

        assert_eq!(
            classify_final_status(true, true, false, None, 0),
            ("calibration_only_unscored", None, false)
        );
        assert_eq!(
            classify_final_status(true, false, false, None, 0).0,
            "diagnostic_only_calibration_floor_failed"
        );
        assert_eq!(
            classify_final_status(true, false, false, Some("maximum_repetitions".into()), 0),
            (
                "diagnostic_only_calibration_repetition_limit",
                Some("maximum_repetitions".into()),
                false
            )
        );
        assert_eq!(
            classify_final_status(
                true,
                true,
                false,
                Some("overall_wall_time_ceiling_during_calibration".into()),
                0
            )
            .0,
            "diagnostic_only_overall_deadline"
        );
    }

    #[test]
    fn journal_sync_failure_is_returned_after_the_record_flush() {
        let mut bytes = Vec::new();
        let value = serde_json::json!({"record_type": "sample_pair"});
        let result = append_record_with_sync(&mut bytes, &value, || {
            Err(std::io::Error::other("injected sync failure"))
        });
        assert!(result.is_err());
        assert!(bytes.ends_with(b"\n"));
    }

    #[test]
    fn fixture_records_exact_data_callbacks_and_starts_each_case_empty() {
        let bytes = [0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b];
        let mut memory = FixtureMemory {
            code: &bytes,
            ram: [0; 128],
            memory_events: [None; 2],
            memory_event_count: 0,
        };
        let value = 0x0123_4567_89ab_cdefu64.to_le_bytes();
        od_isa::memory::GuestMemory::write(
            &mut memory,
            24,
            od_isa::memory::AccessContext::default(),
            &value,
        )
        .unwrap();
        let mut actual = [0; 8];
        od_isa::memory::GuestMemory::read(
            &mut memory,
            24,
            od_isa::memory::AccessContext::default(),
            &mut actual,
        )
        .unwrap();
        assert_eq!(actual, value);
        assert_eq!(
            memory.memory_events,
            expected_memory_events(MemoryCorpusSequence::StoreThenLoad, 24, 40).unwrap()
        );
        assert_eq!(memory.memory_event_count, 2);

        let next_case = FixtureMemory {
            code: &bytes,
            ram: [0; 128],
            memory_events: [None; 2],
            memory_event_count: 0,
        };
        assert_eq!(next_case.memory_event_count, 0);
        assert_eq!(next_case.memory_events, [None; 2]);
    }
}
