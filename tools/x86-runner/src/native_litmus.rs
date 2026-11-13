//! Separate, corpus-scoped native runner for Stage 6 litmus tests.
//!
//! This deliberately does not share or widen the ordinary fixture admission
//! path. Its input is the project-authored `od.smp-litmus.v1` corpus and its
//! instruction boundaries are checked by `od-isa` before any code is mapped.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::{
    Arc, Barrier, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use crate::litmus_policy::{
    decode_build_metadata, decode_one, execution_id, operation_writes_rcx, parse_u32,
    required_output_path, select_affinity_by_topology, valid_corpus_inventory,
    validate_decoded_litmus_instruction, validate_histogram_total, validate_low_mapping,
    validate_observation_contract,
};
use od_isa::long_decode::Operation;
use od_testkit::sha256_hex;
use serde_json::{Value, json};

const MAX_ITERATIONS: u64 = 10_000_000;
const PROCESS_REPETITIONS: usize = 3;
const MAX_FIXTURE_BYTES: usize = 1024 * 1024;
const MAX_THREADS: usize = 4;
const MAX_CELLS: usize = 16;
const MAX_INSTRUCTIONS_PER_THREAD: usize = 16;
const CPU_LIMIT_SECONDS: u64 = 30;
const WALL_LIMIT_SECONDS: u64 = 30;
const EXIT_INVALID: u8 = 3;
const EXIT_UNSUPPORTED: u8 = 4;
const EXIT_FAILURE: u8 = 5;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Gprs {
    rax: u64,
    rcx: u64,
    rdx: u64,
    rbx: u64,
    out_rax: u64,
    out_rcx: u64,
    out_rdx: u64,
    out_rbx: u64,
}

core::arch::global_asm!(
    ".text",
    ".global od_litmus_call",
    ".type od_litmus_call,@function",
    "od_litmus_call:",
    "push rbx",
    "push r12",
    "push r13",
    "push r14",
    "push r15",
    "mov r12, rdi",
    "mov r13, rsi",
    "mov rax, [r13]",
    "mov rcx, [r13 + 8]",
    "mov rdx, [r13 + 16]",
    "mov rbx, [r13 + 24]",
    "call r12",
    "mov [r13 + 32], rax",
    "mov [r13 + 40], rcx",
    "mov [r13 + 48], rdx",
    "mov [r13 + 56], rbx",
    "pop r15",
    "pop r14",
    "pop r13",
    "pop r12",
    "pop rbx",
    "ret",
    ".size od_litmus_call, .-od_litmus_call",
);

unsafe extern "C" {
    fn od_litmus_call(code: *const u8, gprs: *mut Gprs);
}

#[derive(Clone)]
struct Cell {
    name: String,
    original: u32,
    address: u32,
}

#[derive(Clone)]
struct Code {
    cpu: usize,
    original: Vec<u8>,
    executed: Vec<u8>,
    initial: Gprs,
    original_hash: String,
    executed_hash: String,
    mappings: Vec<Value>,
}

struct Prepared {
    id: String,
    observe: Vec<String>,
    cells: Vec<Cell>,
    code: Vec<Code>,
    expected: BTreeSet<Vec<u64>>,
    fixture_hash: String,
    relocation_base: *mut u8,
    relocation_len: usize,
    executable: Vec<*mut u8>,
    executable_lens: Vec<usize>,
    machine: Value,
    limits: Value,
}

impl Drop for Prepared {
    fn drop(&mut self) {
        unsafe {
            if !self.relocation_base.is_null() {
                libc::munmap(self.relocation_base.cast(), self.relocation_len);
            }
            for (base, len) in self.executable.iter().zip(&self.executable_lens) {
                libc::munmap((*base).cast(), *len);
            }
        }
    }
}

pub(crate) fn dispatch(args: &[String]) -> Option<ExitCode> {
    if args.first().map(String::as_str) == Some("--native-litmus-child") {
        return Some(child_cli(&args[1..]));
    }
    if args.first().map(String::as_str) == Some("--litmus-corpus") {
        return Some(parent_cli(&args[1..]));
    }
    None
}

#[derive(Debug)]
struct ChildFailure {
    reason: String,
    status: String,
    stdout: String,
    stderr: String,
    stdout_bytes_hex: String,
    stderr_bytes_hex: String,
}

impl std::fmt::Display for ChildFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} (child status {})", self.reason, self.status)
    }
}

impl ChildFailure {
    fn from_output(
        reason: impl Into<String>,
        status: impl Into<String>,
        output: &crate::supervisor::ProcessOutcome,
    ) -> Self {
        Self {
            reason: reason.into(),
            status: status.into(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            stdout_bytes_hex: hex(&output.stdout),
            stderr_bytes_hex: hex(&output.stderr),
        }
    }
}

fn parent_cli(args: &[String]) -> ExitCode {
    let output_path = match required_output_path(args) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if args.len() != 10
        || args[1] != "--iterations"
        || args[3] != "--counter-n"
        || args[5] != "--counter-k"
        || args[7] != "--json"
        || args[8] != "--output"
    {
        eprintln!(
            "usage: od-x86-runner --litmus-corpus DIR --iterations N --counter-n N --counter-k K --json"
        );
        return ExitCode::from(EXIT_INVALID);
    }
    let directory = &args[0];
    let iterations = args[2].parse::<u64>();
    let Ok(iterations) = iterations else {
        eprintln!("invalid litmus iteration count");
        return ExitCode::from(EXIT_INVALID);
    };
    if iterations == 0 || iterations > MAX_ITERATIONS {
        eprintln!("iterations must be in 1..={MAX_ITERATIONS}");
        return ExitCode::from(EXIT_INVALID);
    }
    let (Ok(counter_n), Ok(counter_k)) = (args[4].parse::<usize>(), args[6].parse::<u64>()) else {
        eprintln!("counter parameters must be unsigned integers");
        return ExitCode::from(EXIT_INVALID);
    };
    if !(2..=4).contains(&counter_n)
        || counter_k == 0
        || counter_k > 4096
        || counter_n as u64 * counter_k > u32::MAX as u64
    {
        eprintln!("counter parameters require 2 <= N <= 4, 1 <= K <= 4096, and N*K <= 2^32-1");
        return ExitCode::from(EXIT_INVALID);
    }
    if let Err(error) = persist_progress(&output_path, &[], None, None) {
        eprintln!("native litmus output path is not writable: {error}");
        return ExitCode::from(EXIT_FAILURE);
    }
    let root = Path::new(directory);
    let mut fixtures = match fs::read_dir(root) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>(),
        Err(error) => {
            eprintln!("could not read litmus corpus: {error}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    fixtures.sort();
    let mut reports = Vec::new();
    let mut formula_count = 0usize;
    let mut source_fixture_hashes = Vec::new();
    let mut seen_finite_ids = BTreeSet::new();
    let mut seen_formula_ids = BTreeSet::new();
    for path in fixtures {
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("could not read {}: {error}", path.display());
                return ExitCode::from(EXIT_INVALID);
            }
        };
        if bytes.len() > MAX_FIXTURE_BYTES {
            eprintln!(
                "fixture exceeds {MAX_FIXTURE_BYTES} byte limit: {}",
                path.display()
            );
            return ExitCode::from(EXIT_INVALID);
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("invalid litmus JSON {}: {error}", path.display());
                return ExitCode::from(EXIT_INVALID);
            }
        };
        if value.get("schema").and_then(Value::as_str) != Some("od.smp-litmus.v1") {
            continue;
        }
        source_fixture_hashes.push(sha256_hex(&bytes));
        if value.get("kind").and_then(Value::as_str) == Some("parameterized_counter_group")
            || value.get("cases").is_some()
        {
            let Some(cases) = value.get("cases").and_then(Value::as_array) else {
                eprintln!("formula group has no cases: {}", path.display());
                return ExitCode::from(EXIT_INVALID);
            };
            for case in cases {
                formula_count += 1;
                let Some(case_id) = case.get("id").and_then(Value::as_str) else {
                    eprintln!("formula case has no id: {}", path.display());
                    return ExitCode::from(EXIT_INVALID);
                };
                if !seen_formula_ids.insert(case_id.to_string()) {
                    eprintln!("duplicate formula case id {case_id}");
                    return ExitCode::from(EXIT_INVALID);
                }
                let generated = match expand_formula(case, counter_n, counter_k) {
                    Ok(generated) => generated,
                    Err(error) => {
                        eprintln!("unsupported formula case: {error}");
                        return ExitCode::from(EXIT_UNSUPPORTED);
                    }
                };
                let result = match run_repetitions(
                    &generated,
                    &path,
                    iterations,
                    case.get("id").and_then(Value::as_str),
                    counter_n,
                    counter_k,
                    &output_path,
                    &reports,
                ) {
                    Ok(result) => result,
                    Err(error) => {
                        eprintln!("native litmus failed for {}: {error}", path.display());
                        return ExitCode::from(EXIT_FAILURE);
                    }
                };
                reports.push(result);
                if let Err(error) = persist_progress(&output_path, &reports, None, None) {
                    eprintln!("could not persist native litmus progress: {error}");
                    return ExitCode::from(EXIT_FAILURE);
                }
            }
        } else {
            let Some(case_id) = value.get("id").and_then(Value::as_str) else {
                eprintln!("fixture has no id: {}", path.display());
                return ExitCode::from(EXIT_INVALID);
            };
            if !seen_finite_ids.insert(case_id.to_string()) {
                eprintln!("duplicate finite fixture id {case_id}");
                return ExitCode::from(EXIT_INVALID);
            }
            let result = match run_repetitions(
                &value,
                &path,
                iterations,
                None,
                counter_n,
                counter_k,
                &output_path,
                &reports,
            ) {
                Ok(result) => result,
                Err(error) => {
                    eprintln!("native litmus failed for {}: {error}", path.display());
                    return ExitCode::from(EXIT_FAILURE);
                }
            };
            reports.push(result);
            if let Err(error) = persist_progress(&output_path, &reports, None, None) {
                eprintln!("could not persist native litmus progress: {error}");
                return ExitCode::from(EXIT_FAILURE);
            }
        }
    }
    if !valid_corpus_inventory(
        &seen_finite_ids,
        &seen_formula_ids,
        reports.len(),
        formula_count,
    ) {
        eprintln!("litmus corpus is incomplete (formula cases: {formula_count})");
        return ExitCode::from(EXIT_INVALID);
    }
    source_fixture_hashes.sort();
    let final_report = json!({
        "schema": "od.smp-native-corpus-result.v1",
        "iterations_per_fixture": iterations,
        "counter_parameters": {"N": counter_n, "K": counter_k},
        "process_repetitions": PROCESS_REPETITIONS,
        "corpus_sha256": sha256_hex(source_fixture_hashes.join("\n").as_bytes()),
        "fixture_count": reports.len(),
        "cpu_limit_seconds_per_fixture": CPU_LIMIT_SECONDS,
        "wall_timeout_seconds_per_fixture": WALL_LIMIT_SECONDS,
        "results": reports,
    });
    if let Err(error) = write_json_atomic(&output_path, &final_report) {
        eprintln!("could not persist final native litmus result: {error}");
        return ExitCode::from(EXIT_FAILURE);
    }
    println!("{final_report}");
    ExitCode::SUCCESS
}

fn run_one(
    fixture: &Value,
    path: &Path,
    iterations: u64,
    formula_case: Option<&str>,
    counter_n: usize,
    counter_k: u64,
) -> Result<Value, ChildFailure> {
    let executable = std::env::current_exe().map_err(|error| ChildFailure {
        reason: format!("could not locate runner executable: {error}"),
        status: "not_spawned".into(),
        stdout: String::new(),
        stderr: String::new(),
        stdout_bytes_hex: String::new(),
        stderr_bytes_hex: String::new(),
    })?;
    let mut command = Command::new(executable);
    command
        .arg("--native-litmus-child")
        .arg(path)
        .arg(iterations.to_string());
    if let Some(case_id) = formula_case {
        command
            .arg(case_id)
            .arg(counter_n.to_string())
            .arg(counter_k.to_string());
    }
    let limits = crate::supervisor::ProcessLimits {
        wall_timeout: Duration::from_secs(WALL_LIMIT_SECONDS),
        cpu_seconds: CPU_LIMIT_SECONDS,
        address_space_bytes: 256 * 1024 * 1024,
        stdout_bytes: od_testkit::MAX_RESULT_JSON_BYTES,
        stderr_bytes: 64 * 1024,
    };
    let output =
        crate::supervisor::run_supervised(&mut command, limits).map_err(|error| ChildFailure {
            reason: format!("supervisor failed: {error}"),
            status: "supervisor_error".into(),
            stdout: String::new(),
            stderr: error.to_string(),
            stdout_bytes_hex: String::new(),
            stderr_bytes_hex: String::new(),
        })?;
    match &output.termination {
        crate::supervisor::Termination::Exited { code: Some(0) } => {
            let result: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
                ChildFailure::from_output(
                    format!("invalid child JSON: {error}"),
                    "exited(0)",
                    &output,
                )
            })?;
            let fixture_bytes = fs::read(path).map_err(|error| {
                ChildFailure::from_output(
                    format!("could not reread fixture: {error}"),
                    "exited(0)",
                    &output,
                )
            })?;
            if result.get("fixture_sha256").and_then(Value::as_str)
                != Some(sha256_hex(&fixture_bytes).as_str())
            {
                return Err(ChildFailure::from_output(
                    "native child returned a fixture hash mismatch",
                    "exited(0)",
                    &output,
                ));
            }
            if result.get("case_id").and_then(Value::as_str)
                != fixture.get("id").and_then(Value::as_str)
                && fixture.get("id").and_then(Value::as_str).is_some()
            {
                return Err(ChildFailure::from_output(
                    "native child returned an unexpected test id",
                    "exited(0)",
                    &output,
                ));
            }
            if result.get("outcome_check").and_then(Value::as_str)
                != Some("all_observed_outcomes_allowed")
            {
                return Err(ChildFailure::from_output(
                    "forbidden x86-TSO outcome was observed",
                    "exited(0)",
                    &output,
                ));
            }
            Ok(result)
        }
        termination => {
            let status = format!("{termination:?}");
            let reason = match &termination {
                crate::supervisor::Termination::TimedOut => "child exceeded wall timeout".into(),
                crate::supervisor::Termination::Signaled { signal } => {
                    format!("child terminated by signal {signal}")
                }
                crate::supervisor::Termination::OutputLimitExceeded { stream } => {
                    format!("child exceeded {stream:?} output limit")
                }
                crate::supervisor::Termination::Exited { code } => {
                    format!("child exited with status {code:?}")
                }
            };
            Err(ChildFailure::from_output(reason, status, &output))
        }
    }
}

fn run_repetitions(
    fixture: &Value,
    path: &Path,
    iterations: u64,
    formula_case: Option<&str>,
    counter_n: usize,
    counter_k: u64,
    output_path: &Path,
    completed_results: &[Value],
) -> Result<Value, String> {
    let kind = if formula_case.is_some() {
        "formula"
    } else {
        "finite"
    };
    let id = fixture
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let execution_id = execution_id(kind, id, counter_n, counter_k);
    let mut runs = Vec::with_capacity(PROCESS_REPETITIONS);
    let mut combined = BTreeMap::<String, u64>::new();
    let mut total_elapsed = 0u128;
    let mut total_rounds = 0u64;
    for process_run in 1..=PROCESS_REPETITIONS {
        let result = match run_one(
            fixture,
            path,
            iterations,
            formula_case,
            counter_n,
            counter_k,
        ) {
            Ok(result) => result,
            Err(error) => {
                let message = error.to_string();
                if let Err(storage_error) = persist_progress(
                    output_path,
                    completed_results,
                    Some(json!({
                        "execution_id": execution_id,
                        "source_fixture_id": id,
                        "completed_process_runs": runs,
                    })),
                    Some(json!({
                        "reason": error.reason,
                        "child_status": error.status,
                        "stdout": error.stdout,
                        "stderr": error.stderr,
                        "stdout_bytes_hex": error.stdout_bytes_hex,
                        "stderr_bytes_hex": error.stderr_bytes_hex,
                        "process_run": process_run,
                    })),
                ) {
                    return Err(format!("{message}; progress write failed: {storage_error}"));
                }
                return Err(message);
            }
        };
        let histogram = match result.get("histogram").and_then(Value::as_object) {
            Some(histogram) => histogram,
            None => {
                let error = "native result omitted histogram".to_string();
                if let Err(storage_error) = persist_progress(
                    output_path,
                    completed_results,
                    Some(json!({
                        "execution_id": execution_id,
                        "source_fixture_id": id,
                        "completed_process_runs": runs,
                        "invalid_child_result": result,
                    })),
                    Some(json!({"failure": error, "process_run": process_run})),
                ) {
                    return Err(format!("{error}; progress write failed: {storage_error}"));
                }
                return Err(error);
            }
        };
        for (outcome, count) in histogram {
            let Some(count) = count.as_u64() else {
                let error = "histogram count is not an integer".to_string();
                if let Err(storage_error) = persist_progress(
                    output_path,
                    completed_results,
                    Some(json!({
                        "execution_id": execution_id,
                        "source_fixture_id": id,
                        "completed_process_runs": runs,
                        "invalid_child_result": result,
                    })),
                    Some(json!({"failure": error, "process_run": process_run})),
                ) {
                    return Err(format!("{error}; progress write failed: {storage_error}"));
                }
                return Err(error);
            };
            *combined.entry(outcome.clone()).or_default() += count;
        }
        total_elapsed += result
            .get("elapsed_ns")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u128;
        total_rounds += result
            .get("iterations_observed")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        runs.push(json!({"process_run": process_run, "result": result}));
        persist_progress(
            output_path,
            completed_results,
            Some(json!({
                "execution_id": execution_id,
                "source_fixture_id": id,
                "completed_process_runs": runs,
            })),
            None,
        )?;
    }
    if total_rounds != iterations * PROCESS_REPETITIONS as u64 {
        let error = format!(
            "combined histogram sum {total_rounds} differs from requested process total {}",
            iterations * PROCESS_REPETITIONS as u64
        );
        if let Err(storage_error) = persist_progress(
            output_path,
            completed_results,
            Some(json!({
                "execution_id": execution_id,
                "source_fixture_id": id,
                "completed_process_runs": runs,
            })),
            Some(json!({"failure": error})),
        ) {
            return Err(format!("{error}; progress write failed: {storage_error}"));
        }
        return Err(error);
    }
    Ok(json!({
        "execution_id": execution_id,
        "execution_kind": kind,
        "source_fixture_id": id,
        "iterations_per_process": iterations,
        "process_repetitions": PROCESS_REPETITIONS,
        "iterations_total": total_rounds,
        "elapsed_ns_total": total_elapsed,
        "combined_histogram": combined,
        "process_runs": runs,
    }))
}

fn write_json_atomic(path: &Path, value: &Value) -> Result<(), String> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    fs::write(&temp, bytes).map_err(|error| error.to_string())?;
    fs::rename(&temp, path).map_err(|error| {
        let _ = fs::remove_file(&temp);
        error.to_string()
    })
}

fn persist_progress(
    path: &Path,
    results: &[Value],
    partial: Option<Value>,
    failure: Option<Value>,
) -> Result<(), String> {
    let value = json!({
        "schema": "od.smp-native-corpus-progress.v1",
        "status": if failure.is_some() { "failed" } else { "running" },
        "completed_case_count": results.len(),
        "results": results,
        "in_progress_case": partial,
        "failure": failure,
    });
    write_json_atomic(path, &value)
}

fn child_cli(args: &[String]) -> ExitCode {
    if args.len() != 2 && args.len() != 5 {
        eprintln!(
            "native litmus child requires fixture path, iterations, and optional formula case id/N/K"
        );
        return ExitCode::from(EXIT_INVALID);
    }
    let Ok(iterations) = args[1].parse::<u64>() else {
        return ExitCode::from(EXIT_INVALID);
    };
    if iterations == 0 || iterations > MAX_ITERATIONS {
        return ExitCode::from(EXIT_INVALID);
    }
    let path = PathBuf::from(&args[0]);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("could not read litmus fixture: {error}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    if bytes.len() > MAX_FIXTURE_BYTES {
        eprintln!("litmus fixture exceeds {MAX_FIXTURE_BYTES} byte limit");
        return ExitCode::from(EXIT_INVALID);
    }
    let fixture: Value = match serde_json::from_slice(&bytes) {
        Ok(fixture) => fixture,
        Err(error) => {
            eprintln!("invalid litmus fixture: {error}");
            return ExitCode::from(EXIT_INVALID);
        }
    };
    let expanded;
    let selected = if args.len() == 5 {
        let case = fixture
            .get("cases")
            .and_then(Value::as_array)
            .and_then(|cases| {
                cases
                    .iter()
                    .find(|case| case.get("id").and_then(Value::as_str) == Some(args[2].as_str()))
            });
        let Some(case) = case else {
            eprintln!("unknown formula case id");
            return ExitCode::from(EXIT_INVALID);
        };
        let (Ok(counter_n), Ok(counter_k)) = (args[3].parse::<usize>(), args[4].parse::<u64>())
        else {
            eprintln!("invalid formula parameters");
            return ExitCode::from(EXIT_INVALID);
        };
        if !(2..=4).contains(&counter_n) || counter_k == 0 || counter_k > 4096 {
            eprintln!("formula parameters out of range");
            return ExitCode::from(EXIT_INVALID);
        }
        expanded = match expand_formula(case, counter_n, counter_k) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("unsupported formula: {error}");
                return ExitCode::from(EXIT_UNSUPPORTED);
            }
        };
        &expanded
    } else {
        &fixture
    };
    match execute(selected, &bytes, iterations) {
        Ok(result) => {
            println!("{result}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("litmus execution rejected: {error}");
            ExitCode::from(EXIT_UNSUPPORTED)
        }
    }
}

fn execute(fixture: &Value, raw: &[u8], iterations: u64) -> Result<Value, String> {
    let mut prepared = prepare(fixture, raw)?;
    let allowed = allowed_cpus()?;
    let topology = cpu_topology(&allowed)?;
    let affinity = select_affinity_by_topology(&allowed, prepared.code.len(), &topology)?;
    prepared.machine = machine_metadata(&affinity, allowed.len())?;
    let build = build_metadata();
    prepared.limits = json!({"cpu_seconds": CPU_LIMIT_SECONDS, "wall_seconds": WALL_LIMIT_SECONDS, "max_iterations": MAX_ITERATIONS});
    let histogram = Arc::new(Mutex::new(BTreeMap::<String, u64>::new()));
    let outputs = Arc::new(Mutex::new(vec![[0u64; 4]; prepared.code.len()]));
    let barrier = Arc::new(Barrier::new(prepared.code.len()));
    let aborted = Arc::new(AtomicBool::new(false));
    let setup_barrier = Arc::new(Barrier::new(prepared.code.len()));
    let setup_error = Arc::new(Mutex::new(None::<String>));
    let started = Instant::now();
    let cells_ptr = prepared.relocation_base as usize;
    let cells = prepared.cells.clone();
    let observe = prepared.observe.clone();
    let expected = prepared.expected.clone();
    let code_ptrs = prepared.executable.clone();
    let mut workers = Vec::new();
    for index in 0..prepared.code.len() {
        let code = Arc::new(prepared.code[index].clone());
        let barrier = Arc::clone(&barrier);
        let outputs = Arc::clone(&outputs);
        let histogram = Arc::clone(&histogram);
        let aborted = Arc::clone(&aborted);
        let setup_barrier = Arc::clone(&setup_barrier);
        let setup_error = Arc::clone(&setup_error);
        let cells = cells.clone();
        let observe = observe.clone();
        let expected = expected.clone();
        let affinity_cpu = affinity[index];
        let code_ptr = code_ptrs[index] as usize;
        let fixture_id = prepared.id.clone();
        workers.push(
            thread::Builder::new()
                .name(format!("od-litmus-{index}"))
                .spawn(move || -> Result<(), String> {
                    if let Err(error) = pin_current_thread(affinity_cpu) {
                        *setup_error
                            .lock()
                            .map_err(|_| "litmus setup lock poisoned")? = Some(error);
                    }
                    setup_barrier.wait();
                    if setup_error
                        .lock()
                        .map_err(|_| "litmus setup lock poisoned")?
                        .is_some()
                    {
                        return Ok(());
                    }
                    for _ in 0..iterations {
                        barrier.wait();
                        if aborted.load(Ordering::Acquire) {
                            break;
                        }
                        let mut regs = code.initial;
                        // SAFETY: code_ptr is an RX mapping containing the
                        // validated fixture bytes followed by a single RET.
                        unsafe {
                            od_litmus_call(code_ptr as *const u8, &mut regs);
                        }
                        outputs.lock().map_err(|_| "litmus output lock poisoned")?[index] = [
                            regs.out_rax as u32 as u64,
                            regs.out_rbx as u32 as u64,
                            regs.out_rcx,
                            regs.out_rdx as u32 as u64,
                        ];
                        barrier.wait();
                        if index == 0 {
                            let mut vector = Vec::with_capacity(observe.len());
                            let register_values = outputs
                                .lock()
                                .map_err(|_| "litmus output lock poisoned")?
                                .clone();
                            for field in &observe {
                                if let Some((cpu, reg)) = parse_register_observation(field) {
                                    let values = register_values.get(cpu).ok_or_else(|| {
                                        format!("observation CPU index out of range in {field}")
                                    })?;
                                    vector.push(match reg {
                                        "eax" => values[0],
                                        "ebx" => values[1],
                                        "ecx" => values[2],
                                        "edx" => values[3],
                                        _ => {
                                            return Err(format!(
                                                "unsupported observed register {field}"
                                            ));
                                        }
                                    });
                                } else if let Some(name) = field.strip_prefix("mem.") {
                                    let cell =
                                        cells.iter().find(|cell| cell.name == name).ok_or_else(
                                            || format!("observation names missing cell {name}"),
                                        )?;
                                    // SAFETY: the shared mapping is aligned RAM
                                    // and all guest execution completed at barrier.
                                    vector.push(unsafe {
                                        (&*((cells_ptr as *mut u8)
                                            .add((cell.address - cells[0].address) as usize)
                                            as *const std::sync::atomic::AtomicU32))
                                            .load(Ordering::Relaxed)
                                            as u64
                                    });
                                } else {
                                    return Err(format!("unsupported observation {field}"));
                                }
                            }
                            let key = vector
                                .iter()
                                .map(u64::to_string)
                                .collect::<Vec<_>>()
                                .join(",");
                            {
                                let mut histogram = histogram
                                    .lock()
                                    .map_err(|_| "litmus histogram lock poisoned")?;
                                *histogram.entry(key).or_default() += 1;
                            }
                            if !expected.is_empty() && !expected.contains(&vector) {
                                aborted.store(true, Ordering::Release);
                            }
                            for cell in &cells {
                                // SAFETY: after the end-of-round barrier no
                                // worker is executing guest code. Worker 0 resets.
                                unsafe {
                                    (&*((cells_ptr as *mut u8)
                                        .add((cell.address - cells[0].address) as usize)
                                        as *const std::sync::atomic::AtomicU32))
                                        .store(cell.original, Ordering::Relaxed);
                                }
                            }
                        }
                        barrier.wait();
                        if aborted.load(Ordering::Acquire) {
                            break;
                        }
                    }
                    let _ = fixture_id;
                    Ok(())
                })
                .map_err(|e| e.to_string())?,
        );
    }
    for worker in workers {
        worker
            .join()
            .map_err(|_| "litmus worker panicked".to_string())??;
    }
    if let Some(error) = setup_error
        .lock()
        .map_err(|_| "litmus setup lock poisoned")?
        .clone()
    {
        return Err(error);
    }
    let elapsed = started.elapsed();
    let histogram = histogram
        .lock()
        .map_err(|_| "litmus histogram lock poisoned")?
        .clone();
    let observed: Vec<Vec<u64>> = histogram
        .keys()
        .map(|key| {
            if key.is_empty() {
                vec![]
            } else {
                key.split(',')
                    .map(|part| part.parse().unwrap_or(0))
                    .collect()
            }
        })
        .collect();
    let observed_rounds = histogram.values().sum::<u64>();
    if !aborted.load(Ordering::Acquire) {
        validate_histogram_total(&histogram, iterations)?;
    }
    let relocation = prepared
        .code
        .iter()
        .map(|code| {
            json!({
                "cpu": code.cpu,
                "original_bytes_hex": hex(&code.original),
                "original_sha256": code.original_hash,
                "executed_bytes_hex": hex(&code.executed),
                "executed_sha256": code.executed_hash,
                "patches": code.mappings,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "schema": "od.smp-native-result.v1",
        "case_id": prepared.id,
        "fixture_sha256": prepared.fixture_hash,
        "mode": "linux_native_threaded",
        "build": build,
        "seed": null,
        "iterations_requested": iterations,
        "iterations_observed": observed_rounds,
        "elapsed_ns": elapsed.as_nanos(),
        "rounds_per_second": if elapsed.as_secs_f64() > 0.0 { observed_rounds as f64 / elapsed.as_secs_f64() } else { 0.0 },
        "histogram": histogram,
        "observed_outcomes": observed,
        "expected_x86_tso_outcomes": prepared.expected,
        "formula_model_oracles": fixture.get("formula_model_oracles"),
        "formula_parameters": fixture.get("formula_parameters"),
        "outcome_check": if aborted.load(Ordering::Acquire) { "forbidden_outcome_observed" } else { "all_observed_outcomes_allowed" },
        "machine": prepared.machine,
        "supervision": prepared.limits,
        "relocation": {"method": "declared absolute disp32 operands relocated to MAP_32BIT shared cells", "records": relocation},
    }))
}

fn prepare(fixture: &Value, raw: &[u8]) -> Result<Prepared, String> {
    if fixture.get("schema").and_then(Value::as_str) != Some("od.smp-litmus.v1") {
        return Err("schema must be od.smp-litmus.v1".into());
    }
    let id = fixture
        .get("id")
        .and_then(Value::as_str)
        .ok_or("missing fixture id")?
        .to_string();
    if !matches!(
        id.as_str(),
        "2+2W"
            | "IRIW"
            | "LB"
            | "LOCK-CMPXCHG-MUTEX"
            | "LOCK-XADD"
            | "MP"
            | "SB-LOCK-LOCAL"
            | "SB-LOCK-XADD"
            | "SB-LOCK"
            | "SB-MFENCE"
            | "SB-XCHG"
            | "SB"
            | "STORE-FORWARDING"
            | "UNLOCKED-INC"
            | "XCHG-MUTEX"
            | "LOCK-INC"
            | "THREE-INSN-COUNTER"
    ) {
        return Err(format!(
            "fixture id {id} is outside the S6-05 corpus admission"
        ));
    }
    let observe = fixture
        .get("observe")
        .and_then(|v| v.get("fields"))
        .and_then(Value::as_array)
        .ok_or("missing observe.fields")?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or("non-string observation field")
        })
        .collect::<Result<Vec<_>, _>>()?;
    let expected = fixture
        .get("outcomes")
        .and_then(|o| o.get("x86_tso"))
        .and_then(|o| o.get("values"))
        .and_then(Value::as_array)
        .ok_or("missing complete x86_tso outcome set")?
        .iter()
        .map(|values| {
            values
                .as_array()
                .ok_or("invalid expected outcome")?
                .iter()
                .map(|v| v.as_u64().ok_or("invalid expected value"))
                .collect::<Result<Vec<_>, _>>()
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    if expected.is_empty() {
        return Err("complete x86_tso outcome set must not be empty".into());
    }
    let raw_cells = fixture
        .get("memory")
        .and_then(|v| v.get("cells"))
        .and_then(Value::as_array)
        .ok_or("missing memory.cells")?;
    if raw_cells.is_empty() || raw_cells.len() > MAX_CELLS {
        return Err(format!("memory.cells must contain 1..={MAX_CELLS} entries"));
    }
    let mut cells = raw_cells
        .iter()
        .map(|cell| {
            if cell.get("size_bits").and_then(Value::as_u64) != Some(32)
                || cell.get("kind").and_then(Value::as_str) != Some("write_back_ram")
                || !cell.get("alias").is_some_and(Value::is_null)
            {
                return Err("litmus cells must be independent 32-bit write-back RAM".to_string());
            }
            let name = cell
                .get("name")
                .and_then(Value::as_str)
                .ok_or("cell missing name")?
                .to_string();
            let address = parse_u32(
                cell.get("address")
                    .and_then(Value::as_str)
                    .ok_or("cell missing address")?,
            )?;
            let original_value = cell
                .get("initial")
                .and_then(Value::as_u64)
                .ok_or("cell missing initial value")?;
            if original_value > u32::MAX as u64 || address & 3 != 0 {
                return Err("32-bit cell values must fit u32 and addresses must be aligned".into());
            }
            let original = original_value as u32;
            Ok(Cell {
                name,
                original,
                address,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if cells
        .iter()
        .map(|cell| cell.name.as_str())
        .collect::<BTreeSet<_>>()
        .len()
        != cells.len()
    {
        return Err("memory cell names must be unique".into());
    }
    cells.sort_by_key(|cell| cell.address);
    if cells.is_empty()
        || cells
            .windows(2)
            .any(|pair| pair[0].address == pair[1].address)
    {
        return Err("memory cells must have unique addresses".into());
    }
    let min_addr = cells[0].address;
    let max_addr = cells.iter().map(|cell| cell.address).max().unwrap();
    let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
    let map_len = page.max(((max_addr - min_addr) as usize + 4 + page - 1) & !(page - 1));
    // SAFETY: mmap creates an isolated shared anonymous RAM region owned by Prepared.
    let relocation_base = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            map_len,
            libc::PROT_READ | libc::PROT_WRITE,
            libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_32BIT,
            -1,
            0,
        )
    };
    if relocation_base == libc::MAP_FAILED {
        return Err(format!(
            "MAP_32BIT relocation failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    let base = relocation_base as usize;
    if validate_low_mapping(base, map_len).is_err() {
        unsafe {
            libc::munmap(relocation_base, map_len);
        }
        return Err("relocation mapping is outside positive signed disp32 reach".into());
    }
    for cell in &mut cells {
        cell.address = (base + (cell.address - min_addr) as usize) as u32;
        // SAFETY: mapped writable data page; AtomicU32 has the required alignment.
        unsafe {
            (&*((relocation_base.cast::<u8>()).add((cell.address as usize) - base)
                as *const std::sync::atomic::AtomicU32))
                .store(cell.original, Ordering::Relaxed);
        }
    }
    let raw_threads = fixture
        .get("threads")
        .and_then(Value::as_array)
        .ok_or("missing threads array")?;
    if raw_threads.len() < 2 || raw_threads.len() > MAX_THREADS {
        return Err(format!("threads must contain 2..={MAX_THREADS} entries"));
    }
    if let Some(parameters) = fixture.get("formula_parameters") {
        let n = parameters
            .get("N")
            .and_then(Value::as_u64)
            .ok_or("formula N must be an integer")?;
        let k = parameters
            .get("K")
            .and_then(Value::as_u64)
            .ok_or("formula K must be an integer")?;
        if n != raw_threads.len() as u64
            || !(2..=4).contains(&n)
            || !(1..=4096).contains(&k)
            || n.checked_mul(k).is_none_or(|total| total > u32::MAX as u64)
        {
            return Err(
                "formula N/K parameters do not match thread count or admitted bounds".into(),
            );
        }
    }
    let cell_names = cells.iter().map(|cell| cell.name.clone()).collect();
    validate_observation_contract(&observe, &expected, &cell_names, raw_threads.len())?;
    let mut code = Vec::new();
    for raw_thread in raw_threads {
        let cpu = raw_thread
            .get("cpu")
            .and_then(Value::as_u64)
            .ok_or("thread missing cpu")? as usize;
        let instructions = raw_thread
            .get("instructions")
            .and_then(Value::as_array)
            .ok_or("thread missing instructions")?;
        if instructions.is_empty() || instructions.len() > MAX_INSTRUCTIONS_PER_THREAD {
            return Err(format!(
                "CPU {cpu} instruction count is outside supported bounds"
            ));
        }
        let mut original = Vec::new();
        let mut executed = Vec::new();
        let mut mappings = Vec::new();
        let mut loop_displacement = None;
        let mut loop_body_writes_rcx = false;
        for instruction in instructions {
            let bytes = parse_hex(
                instruction
                    .get("bytes")
                    .and_then(Value::as_str)
                    .ok_or("instruction missing bytes")?,
            )?;
            let length = instruction
                .get("length")
                .and_then(Value::as_u64)
                .ok_or("instruction missing length")? as usize;
            if bytes.len() != length {
                return Err(format!("declared byte length mismatch on CPU {cpu}"));
            }
            let decoded = decode_one(&bytes)?;
            if usize::from(decoded.length) != bytes.len() {
                return Err(format!("od-isa decoded length mismatch on CPU {cpu}"));
            }
            validate_decoded_litmus_instruction(instruction, decoded)?;
            if let Operation::Loop { displacement } = decoded.operation {
                if loop_displacement.is_some()
                    || !std::ptr::eq(instruction, instructions.last().unwrap())
                {
                    return Err("LOOP must appear once as the final litmus instruction".into());
                }
                loop_displacement = Some(displacement);
            } else if loop_displacement.is_none() && operation_writes_rcx(decoded.operation) {
                loop_body_writes_rcx = true;
            }
            let mut relocated = bytes.clone();
            if let Some(address_text) = instruction
                .get("operands")
                .and_then(|o| o.get("address"))
                .and_then(Value::as_str)
            {
                let from = parse_u32(address_text)?;
                let cell = cells
                    .iter()
                    .find(|cell| {
                        let original_address = raw_cells
                            .iter()
                            .find(|c| {
                                c.get("name").and_then(Value::as_str) == Some(cell.name.as_str())
                            })
                            .and_then(|c| c.get("address"))
                            .and_then(Value::as_str)
                            .and_then(|a| parse_u32(a).ok());
                        original_address == Some(from)
                    })
                    .ok_or_else(|| {
                        format!("operand address {address_text} does not name a declared cell")
                    })?;
                let offsets = absolute_disp32_offsets(&bytes, from);
                if offsets.len() != 1 {
                    return Err(format!(
                        "operand {address_text} has {} validated disp32 fields",
                        offsets.len()
                    ));
                }
                let offset = offsets[0];
                let new_address =
                    (relocation_base as usize + (cell.address - (base as u32)) as usize) as i32;
                relocated[offset..offset + 4].copy_from_slice(&new_address.to_le_bytes());
                mappings.push(json!({"instruction_offset": original.len(), "disp32_offset": offset, "original_address": format!("0x{from:08x}"), "executed_address": format!("0x{:08x}", new_address as u32)}));
            }
            original.extend_from_slice(&bytes);
            executed.extend_from_slice(&relocated);
        }
        if let Some(displacement) = loop_displacement {
            if displacement as isize != -(original.len() as isize) {
                return Err(
                    "formula LOOP must target the first instruction in its code region".into(),
                );
            }
            if loop_body_writes_rcx {
                return Err("formula LOOP body must not write RCX".into());
            }
        }
        let formula_k = fixture
            .get("formula_parameters")
            .and_then(|parameters| parameters.get("K"))
            .and_then(Value::as_u64);
        if loop_displacement.is_some() != formula_k.is_some() {
            return Err("LOOP is admitted only for parameterized formula fixtures".into());
        }
        let initial = read_initial_gprs(
            raw_thread
                .get("initial_registers")
                .ok_or("thread missing initial registers")?,
        )?;
        if formula_k.is_some_and(|k| initial.rcx != k) {
            return Err("formula thread RCX must start at the declared K".into());
        }
        code.push(Code {
            cpu,
            original_hash: sha256_hex(&original),
            executed_hash: sha256_hex(&executed),
            original,
            executed,
            initial,
            mappings,
        });
    }
    if code.len() < 2
        || code
            .iter()
            .map(|thread| thread.cpu)
            .collect::<BTreeSet<_>>()
            .len()
            != code.len()
    {
        unsafe {
            libc::munmap(relocation_base, map_len);
        }
        return Err("litmus requires at least two threads with unique CPU ids".into());
    }
    code.sort_by_key(|thread| thread.cpu);
    if code
        .iter()
        .enumerate()
        .any(|(index, thread)| thread.cpu != index)
    {
        unsafe {
            libc::munmap(relocation_base, map_len);
        }
        return Err("litmus CPU ids must be contiguous from zero".into());
    }
    let mut executable = Vec::new();
    let mut executable_lens = Vec::new();
    for thread in &code {
        let bytes_len = thread
            .executed
            .len()
            .checked_add(1)
            .ok_or("code size overflow")?;
        let len = (bytes_len + page - 1) & !(page - 1);
        // SAFETY: anonymous mapping is filled before changing it to RX.
        let mapping = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if mapping == libc::MAP_FAILED {
            return Err(format!(
                "code mmap failed: {}",
                std::io::Error::last_os_error()
            ));
        }
        unsafe {
            std::ptr::copy_nonoverlapping(
                thread.executed.as_ptr(),
                mapping.cast(),
                thread.executed.len(),
            );
            *mapping.cast::<u8>().add(thread.executed.len()) = 0xc3;
            if libc::mprotect(mapping, len, libc::PROT_READ | libc::PROT_EXEC) != 0 {
                libc::munmap(mapping, len);
                return Err(format!(
                    "code mprotect failed: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        executable.push(mapping.cast::<u8>());
        executable_lens.push(len);
    }
    Ok(Prepared {
        id,
        observe,
        cells,
        code,
        expected,
        fixture_hash: sha256_hex(raw),
        relocation_base: relocation_base.cast::<u8>(),
        relocation_len: map_len,
        executable,
        executable_lens,
        machine: Value::Null,
        limits: Value::Null,
    })
}

fn absolute_disp32_offsets(bytes: &[u8], address: u32) -> Vec<usize> {
    let needle = address.to_le_bytes();
    bytes
        .windows(4)
        .enumerate()
        .filter_map(|(offset, window)| {
            let valid_modrm_sib =
                offset >= 2 && bytes[offset - 2] & 0xc7 == 0x04 && bytes[offset - 1] & 0x07 == 0x05;
            (valid_modrm_sib && window == needle).then_some(offset)
        })
        .collect()
}

fn read_initial_gprs(registers: &Value) -> Result<Gprs, String> {
    let object = registers
        .as_object()
        .ok_or("initial_registers must be an object")?;
    if object.values().any(|value| value.as_u64().is_none()) {
        return Err("initial_registers values must be unsigned integers".into());
    }
    if object.keys().any(|name| {
        !matches!(
            name.as_str(),
            "rax" | "eax" | "rcx" | "ecx" | "rdx" | "edx" | "rbx" | "ebx"
        )
    }) {
        return Err("initial_registers contains an unsupported register".into());
    }
    let value = |wide: &str, narrow: &str| -> Result<u64, String> {
        let wide_value = registers.get(wide).and_then(Value::as_u64);
        let narrow_value = registers.get(narrow).and_then(Value::as_u64);
        if wide_value.zip(narrow_value).is_some_and(|(a, b)| a != b) {
            return Err(format!(
                "conflicting initial values for {wide} and {narrow}"
            ));
        }
        Ok(wide_value.or(narrow_value).unwrap_or(0))
    };
    Ok(Gprs {
        rax: value("rax", "eax")?,
        rcx: value("rcx", "ecx")?,
        rdx: value("rdx", "edx")?,
        rbx: value("rbx", "ebx")?,
        ..Gprs::default()
    })
}

fn parse_register_observation(field: &str) -> Option<(usize, &str)> {
    let rest = field.strip_prefix("cpu")?;
    let (cpu, reg) = rest.split_once('.')?;
    Some((cpu.parse().ok()?, reg))
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    text.split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).map_err(|_| format!("invalid byte {byte}")))
        .collect()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn allowed_cpus() -> Result<Vec<usize>, String> {
    // SAFETY: zeroed cpu_set_t is the documented input/output structure for sched_getaffinity.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    let result =
        unsafe { libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) };
    if result != 0 {
        return Err(format!(
            "sched_getaffinity failed: {}",
            std::io::Error::last_os_error()
        ));
    }
    Ok((0..libc::CPU_SETSIZE as usize)
        .filter(|cpu| unsafe { libc::CPU_ISSET(*cpu, &set) })
        .collect())
}

fn pin_current_thread(cpu: usize) -> Result<(), String> {
    // SAFETY: CPU_ZERO initializes the mask and CPU_SET selects one in-range id.
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    unsafe {
        libc::CPU_ZERO(&mut set);
        libc::CPU_SET(cpu, &mut set);
    }
    let result = unsafe {
        libc::pthread_setaffinity_np(
            libc::pthread_self(),
            std::mem::size_of::<libc::cpu_set_t>(),
            &set,
        )
    };
    if result != 0 {
        return Err(format!(
            "could not pin worker to logical CPU {cpu}: {}",
            std::io::Error::from_raw_os_error(result)
        ));
    }
    Ok(())
}

fn cpu_topology(cpus: &[usize]) -> Result<BTreeMap<usize, (String, String)>, String> {
    cpus.iter()
        .map(|cpu| {
            let base = PathBuf::from(format!("/sys/devices/system/cpu/cpu{cpu}/topology"));
            let package = fs::read_to_string(base.join("physical_package_id"))
                .map_err(|e| format!("CPU {cpu} package topology unavailable: {e}"))?
                .trim()
                .to_string();
            let core = fs::read_to_string(base.join("core_id"))
                .map_err(|e| format!("CPU {cpu} core topology unavailable: {e}"))?
                .trim()
                .to_string();
            Ok((*cpu, (package, core)))
        })
        .collect()
}

fn machine_metadata(cpus: &[usize], allowed_logical_cpu_count: usize) -> Result<Value, String> {
    let cpuinfo = fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    let model = cpuinfo
        .lines()
        .find_map(|line| {
            line.strip_prefix("model name\t: ")
                .or_else(|| line.strip_prefix("Hardware\t: "))
        })
        .unwrap_or("unknown");
    let mut records = Vec::new();
    let mut cores = BTreeSet::new();
    let mut packages = BTreeSet::new();
    for cpu in cpus {
        let base = PathBuf::from(format!("/sys/devices/system/cpu/cpu{cpu}/topology"));
        let read_num = |name: &str| -> String {
            fs::read_to_string(base.join(name))
                .unwrap_or_else(|_| "unknown".into())
                .trim()
                .to_string()
        };
        let package = read_num("physical_package_id");
        let core = read_num("core_id");
        if package == "unknown" || core == "unknown" {
            return Err(format!("CPU topology unavailable for logical CPU {cpu}"));
        }
        packages.insert(package.clone());
        cores.insert(format!("{package}:{core}"));
        records.push(json!({"logical_cpu": cpu, "package_id": package, "core_id": core, "thread_siblings_list": read_num("thread_siblings_list")}));
    }
    let online_cpus =
        parse_cpu_list(&fs::read_to_string("/sys/devices/system/cpu/online").unwrap_or_default());
    let mut host_cores = BTreeSet::new();
    let mut host_packages = BTreeSet::new();
    for cpu in &online_cpus {
        let base = PathBuf::from(format!("/sys/devices/system/cpu/cpu{cpu}/topology"));
        let package = fs::read_to_string(base.join("physical_package_id"))
            .unwrap_or_default()
            .trim()
            .to_string();
        let core = fs::read_to_string(base.join("core_id"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !package.is_empty() && !core.is_empty() {
            host_packages.insert(package.clone());
            host_cores.insert(format!("{package}:{core}"));
        }
    }
    if online_cpus.is_empty() || host_cores.is_empty() || host_packages.is_empty() {
        return Err("host CPU topology metadata is unavailable".into());
    }
    Ok(json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu_model": model,
        "host_online_logical_cpu_count": online_cpus.len(),
        "host_physical_package_count": host_packages.len(),
        "host_physical_core_count": host_cores.len(),
        "host_smt_threads_per_core_average": if host_cores.len() > 0 { online_cpus.len() as f64 / host_cores.len() as f64 } else { 0.0 },
        "allowed_logical_cpu_count": allowed_logical_cpu_count,
        "worker_logical_cpu_count": cpus.len(),
        "worker_affinity_policy": "one logical CPU per physical core first; use distinct allowed SMT siblings only when worker count exceeds physical-core count",
        "worker_smt_sharing": cores.len() < cpus.len(),
        "worker_physical_package_count": packages.len(),
        "worker_physical_core_count": cores.len(),
        "worker_smt_threads_per_core_average": if cores.len() > 0 { cpus.len() as f64 / cores.len() as f64 } else { 0.0 },
        "workers": records
    }))
}

fn parse_cpu_list(value: &str) -> Vec<usize> {
    let mut cpus = BTreeSet::new();
    for part in value.trim().split(',') {
        if let Some((start, end)) = part.split_once('-') {
            if let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>()) {
                cpus.extend(start..=end);
            }
        } else if let Ok(cpu) = part.parse::<usize>() {
            cpus.insert(cpu);
        }
    }
    cpus.into_iter().collect()
}

fn build_metadata() -> Value {
    let executable = std::env::current_exe().ok();
    let executable_sha256 = executable
        .as_ref()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| sha256_hex(&bytes));
    json!({
        "runner_version": env!("CARGO_PKG_VERSION"),
        "executable_path": executable.map(|path| path.display().to_string()),
        "executable_sha256": executable_sha256,
        "source_revision": env!("OD_X86_BUILD_SOURCE_REVISION"),
        "source_sha256": env!("OD_X86_BUILD_SOURCE_SHA256"),
        "source_dirty_at_build": env!("OD_X86_BUILD_SOURCE_DIRTY"),
        "dirty_paths_at_build": decode_build_metadata(env!("OD_X86_BUILD_DIRTY_PATHS_HEX")),
        "rustc_version": decode_build_metadata(env!("OD_X86_BUILD_RUSTC_HEX")),
        "build_profile": env!("OD_X86_BUILD_PROFILE"),
        "target": env!("OD_X86_BUILD_TARGET"),
        "rustflags_source": env!("OD_X86_BUILD_RUSTFLAGS_SOURCE"),
        "extra_rustflags": decode_build_metadata(env!("OD_X86_BUILD_RUSTFLAGS_HEX")),
        "cargo_profile_defaults": "Cargo profile defaults for the recorded build_profile; see workspace Cargo.toml"
    })
}

fn expand_formula(case: &Value, n: usize, k: u64) -> Result<Value, String> {
    let template = case
        .get("thread_template")
        .ok_or("formula missing thread_template")?;
    let cells = case
        .get("initial_memory")
        .and_then(|v| v.get("cells"))
        .cloned()
        .ok_or("formula missing initial_memory.cells")?;
    let instructions = template
        .get("instructions")
        .and_then(Value::as_array)
        .ok_or("formula missing instructions")?;
    let mut threads = Vec::new();
    for cpu in 0..n {
        let mut expanded = Vec::new();
        for instruction in instructions {
            let mut item = instruction.clone();
            if let Some(bytes) = item.get("bytes").and_then(Value::as_str) {
                item["bytes"] = Value::String(bytes.to_string());
            }
            expanded.push(item);
        }
        threads.push(json!({"cpu": cpu, "initial_registers": {"eax": 0, "ebx": 0, "ecx": k, "edx": 0, "rcx": k}, "instructions": expanded}));
    }
    let counter_total = n as u64 * k;
    if counter_total > u32::MAX as u64 {
        return Err("formula N*K exceeds the guest counter width".into());
    }
    let formula_oracles = case
        .get("formula_oracle")
        .and_then(Value::as_object)
        .ok_or("formula case missing formula_oracle")?;
    let mut evaluated_oracles = serde_json::Map::new();
    for model in ["x86_tso", "sc", "interleaving_instruction_atomic"] {
        let oracle = formula_oracles
            .get(model)
            .ok_or("formula model oracle missing")?;
        let kind = oracle
            .get("kind")
            .and_then(Value::as_str)
            .ok_or("formula oracle kind missing")?;
        let evaluated = match kind {
            "exact_singleton" => json!({"kind": kind, "complete": true, "values": [counter_total]}),
            "exact_integer_range" => {
                let minimum_expr = oracle
                    .get("minimum")
                    .and_then(Value::as_str)
                    .ok_or("formula minimum missing")?;
                let minimum = if minimum_expr == "1 if K == 1 else 2" {
                    if k == 1 { 1 } else { 2 }
                } else {
                    minimum_expr
                        .parse::<u64>()
                        .map_err(|_| "unsupported formula minimum expression")?
                };
                json!({"kind": kind, "complete": true, "minimum": minimum, "maximum": counter_total, "step": 1, "lost_updates_possible": oracle.get("lost_updates_possible").and_then(Value::as_bool).unwrap_or(false)})
            }
            _ => return Err(format!("unsupported formula oracle kind {kind}")),
        };
        evaluated_oracles.insert(model.to_string(), evaluated);
    }
    let tso = evaluated_oracles
        .get("x86_tso")
        .ok_or("x86_tso oracle missing")?;
    let expected = if tso.get("kind").and_then(Value::as_str) == Some("exact_singleton") {
        vec![vec![counter_total]]
    } else {
        let minimum = tso
            .get("minimum")
            .and_then(Value::as_u64)
            .ok_or("x86_tso minimum missing")?;
        (minimum..=counter_total).map(|v| vec![v]).collect()
    };
    let mut synthetic = json!({"schema":"od.smp-litmus.v1", "id":case.get("id").and_then(Value::as_str).unwrap_or("counter").to_string(), "observe":case.get("observe").cloned().ok_or("formula missing observe")?, "memory":{"cells":cells}, "threads":threads, "formula_model_oracles": evaluated_oracles, "formula_parameters":{"N": n, "K": k}});
    synthetic["outcomes"] = json!({"x86_tso":{"values":expected}});
    Ok(synthetic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_instruction(op: &str, operands: Value, bytes: &[u8]) -> Result<(), String> {
        let decoded = decode_one(bytes)?;
        let instruction = json!({"op": op, "operands": operands});
        validate_decoded_litmus_instruction(&instruction, decoded)
    }

    #[test]
    fn exact_decoded_admission_rejects_address_and_operand_escapes() {
        let address = json!({"address":"0x00001000", "destination":"eax"});
        // SIB index=RAX means this is [disp32 + RAX], not the declared cell.
        let indexed = [0x8b, 0x04, 0x05, 0x00, 0x10, 0x00, 0x00];
        assert!(decode_one(&indexed).is_ok());
        assert!(check_instruction("load", address.clone(), &indexed).is_err());

        // ModRM.reg=4 writes ESP/RSP, outside the preserved and observed GPR set.
        let rsp_destination = [0x8b, 0x24, 0x25, 0x00, 0x10, 0x00, 0x00];
        assert!(decode_one(&rsp_destination).is_ok());
        assert!(check_instruction("load", address, &rsp_destination).is_err());

        // FF /6 is PUSH r/m32, not a locked increment/RMW.
        let push = [0xff, 0x34, 0x25, 0x00, 0x10, 0x00, 0x00];
        assert!(decode_one(&push).is_ok());
        assert!(check_instruction("locked_rmw", json!({"address":"0x00001000"}), &push).is_err());
        // FF /1 is DEC, not INC.
        let dec = [0xff, 0x0c, 0x25, 0x00, 0x10, 0x00, 0x00];
        assert!(decode_one(&dec).is_ok());
        assert!(check_instruction("rmw", json!({"address":"0x00001000"}), &dec).is_err());

        // Correct address form but the decoded destination disagrees with metadata.
        let ebx_load = [0x8b, 0x1c, 0x25, 0x00, 0x10, 0x00, 0x00];
        assert!(
            check_instruction(
                "load",
                json!({"address":"0x00001000", "destination":"eax"}),
                &ebx_load
            )
            .is_err()
        );
        // Extra declared register operand cannot make a unary load valid.
        let eax_load = [0x8b, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00];
        assert!(
            check_instruction(
                "load",
                json!({"address":"0x00001000", "destination":"eax", "register":"ebx"}),
                &eax_load
            )
            .is_err()
        );
        assert!(check_instruction("mfence", json!({}), &[0x0f, 0xae, 0xf0]).is_ok());
        assert!(
            check_instruction(
                "load",
                json!({"address":"0x00001000", "destination":"eax"}),
                &[0x0f, 0x05]
            )
            .is_err()
        );
    }

    #[test]
    fn disp32_relocation_requires_one_declared_absolute_memory_operand() {
        let bytes = [
            0xc7, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
        ];
        assert_eq!(absolute_disp32_offsets(&bytes, 0x1000), vec![3]);
        assert!(absolute_disp32_offsets(&bytes, 0x1004).is_empty());
        let ambiguous = [
            0x04, 0x25, 0x00, 0x10, 0x00, 0x00, 0x04, 0x25, 0x00, 0x10, 0x00, 0x00,
        ];
        assert_eq!(absolute_disp32_offsets(&ambiguous, 0x1000).len(), 2);
    }

    #[test]
    fn low_mapping_rejects_addresses_outside_positive_disp32_reach() {
        assert!(validate_low_mapping(0x1000, 4096).is_ok());
        assert!(validate_low_mapping(i32::MAX as usize, 4096).is_err());
        assert!(validate_low_mapping((i32::MAX as usize) - 32, 64).is_err());
    }

    #[test]
    fn formula_oracles_keep_locked_and_unlocked_inc_distinct_at_n2_k2() {
        let group: Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/guest/stage6/litmus/counters-formula.json"
        ))
        .unwrap();
        let cases = group["cases"].as_array().unwrap();
        let unlocked = cases
            .iter()
            .find(|case| case["id"] == "UNLOCKED-INC")
            .unwrap();
        let expanded = expand_formula(unlocked, 2, 2).unwrap();
        assert_eq!(expanded["formula_parameters"], json!({"N":2,"K":2}));
        assert_eq!(expanded["formula_model_oracles"]["x86_tso"]["minimum"], 2);
        assert_eq!(expanded["formula_model_oracles"]["x86_tso"]["maximum"], 4);
        assert_eq!(
            expanded["formula_model_oracles"]["interleaving_instruction_atomic"]["values"][0],
            4
        );

        let locked = cases.iter().find(|case| case["id"] == "LOCK-INC").unwrap();
        let expanded = expand_formula(locked, 2, 2).unwrap();
        assert_eq!(expanded["formula_model_oracles"]["x86_tso"]["values"][0], 4);
    }

    #[test]
    fn cpu_list_parser_handles_linux_ranges_and_singletons() {
        assert_eq!(parse_cpu_list("0-2,5,7-8"), vec![0, 1, 2, 5, 7, 8]);
    }
}
