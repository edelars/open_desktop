use std::fs;
use std::io::{self, Write};
use std::process::ExitCode;

use od_isa::{CpuState, ExecutionResult, interpret};
use od_testkit::HexU64;
use od_testkit::case::{Fixture, LoadedFixture};
use od_testkit::compare::{
    CompareError, ComparisonReport, compare_results as compare_result_values,
};
use od_testkit::generate::generate_fixture;
use od_testkit::minimize::{MinimizeError, minimize_fixture as minimize_fixture_value};
use od_testkit::result::{
    Backend, BuildMetadata, CompletionStatus, CpuSnapshot, ExitReason, GprSnapshot,
    MachineMetadata, Observation, RESULT_SCHEMA_VERSION, RunResult,
};

const EXIT_MISMATCH: u8 = 2;
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_UNSUPPORTED: u8 = 4;
const EXIT_HARNESS_FAILURE: u8 = 5;

pub fn convert_fixture_v1(args: &[String]) -> ExitCode {
    if args.len() != 2 {
        return invalid_input("usage: od-runner --convert-fixture-v1 CASE_V1.json CASE_V2.json");
    }
    let conversion = (|| -> Result<(), String> {
        let bytes = fs::read(&args[0]).map_err(|e| e.to_string())?;
        let input = Fixture::from_json(&bytes).map_err(|e| e.to_string())?;
        let output = od_testkit::v2::Fixture::convert_v1(&input).map_err(|e| e.to_string())?;
        let bytes = output.to_json().map_err(|e| e.to_string())?;
        // Explicit output and create_new preserve both historical and user files.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[1])
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
        println!(
            "converted_fixture_sha256={}",
            od_testkit::sha256_hex(&bytes)
        );
        Ok(())
    })();
    match conversion {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => invalid_input(&error),
    }
}

pub fn generate_corpus(args: &[String]) -> ExitCode {
    if args.len() != 3 {
        return invalid_input("usage: od-runner --generate-corpus SEED COUNT OUTPUT_DIR");
    }
    let seed = match parse_u64(&args[0]) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error),
    };
    let count = match args[1].parse::<u32>() {
        Ok(value) if (1..=100_000).contains(&value) => value,
        _ => return invalid_input("COUNT must be an integer from 1 to 100000"),
    };
    let directory = std::path::Path::new(&args[2]);
    if let Err(error) = fs::create_dir(directory) {
        return invalid_input(&format!("could not create output directory: {error}"));
    }
    let mut corpus = Vec::new();
    for sequence in 0..count {
        let fixture = generate_fixture(seed, sequence);
        let bytes = match fixture.to_json() {
            Ok(bytes) => bytes,
            Err(error) => return invalid_input(&format!("generated fixture is invalid: {error}")),
        };
        corpus.extend_from_slice(&bytes);
        let path = directory.join(format!("{}.case.json", fixture.case_id));
        if let Err(error) = fs::write(path, bytes) {
            return invalid_input(&format!("could not write generated fixture: {error}"));
        }
    }
    println!(
        "generated {count} fixtures with seed 0x{seed:016x}; corpus_sha256={}",
        od_testkit::sha256_hex(&corpus)
    );
    ExitCode::SUCCESS
}

pub fn minimize_fixture(args: &[String]) -> ExitCode {
    let Some(input_path) = args.first() else {
        return invalid_input(
            "usage: od-runner --minimize CASE.json --output REDUCED.json [--max-attempts N]",
        );
    };
    let mut output_path = None;
    let mut max_attempts = 1_000usize;
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--output" => {
                index += 1;
                output_path = args.get(index).map(String::as_str);
                if output_path.is_none() {
                    return invalid_input("missing value for --output");
                }
            }
            "--max-attempts" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --max-attempts");
                };
                max_attempts = match value.parse::<usize>() {
                    Ok(value) if (2..=100_000).contains(&value) => value,
                    _ => return invalid_input("--max-attempts must be between 2 and 100000"),
                };
            }
            option => return invalid_input(&format!("unexpected minimizer argument '{option}'")),
        }
        index += 1;
    }
    let Some(output_path) = output_path else {
        return invalid_input("--minimize requires --output REDUCED.json");
    };
    let input = match fs::read(input_path) {
        Ok(input) => input,
        Err(error) => return invalid_input(&format!("could not read fixture: {error}")),
    };
    let loaded = match Fixture::from_json(&input) {
        Ok(loaded) => loaded,
        Err(error) => return invalid_input(&format!("invalid fixture: {error}")),
    };
    let result = minimize_fixture_value(&loaded.fixture, max_attempts, local_mismatch_class);
    let result = match result {
        Ok(result) => result,
        Err(MinimizeError::NoInitialMismatch) => {
            eprintln!("no stable interpreter/JIT mismatch exists for this fixture");
            return ExitCode::from(EXIT_MISMATCH);
        }
        Err(MinimizeError::FlakyInitialResult { .. }) => {
            eprintln!("initial mismatch was not stable across two executions");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
        Err(error) => return invalid_input(&format!("cannot minimize fixture: {error:?}")),
    };
    let bytes = match result.fixture.to_json() {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input(&format!("minimized fixture is invalid: {error}")),
    };
    if let Err(error) = fs::write(output_path, bytes) {
        return invalid_input(&format!("could not write minimized fixture: {error}"));
    }
    println!(
        "minimized {}: {} -> {} instructions in {} attempts; mismatch={}",
        result.fixture.case_id,
        result.original_instruction_count,
        result.minimized_instruction_count,
        result.attempts,
        result.mismatch_class
    );
    ExitCode::SUCCESS
}

fn local_mismatch_class(fixture: &Fixture) -> Option<String> {
    let bytes = fixture.to_json().ok()?;
    let loaded = Fixture::from_json(&bytes).ok()?;
    let interpreter = execute_fixture(&loaded, Backend::Interpreter).ok()?;
    let jit = execute_fixture(&loaded, Backend::Jit).ok()?;
    let report = compare_result_values(&loaded, &interpreter, &jit).ok()?;
    report
        .differences
        .first()
        .map(|difference| difference.field.clone())
}

fn parse_u64(input: &str) -> Result<u64, String> {
    if let Some(hex) = input.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)
            .map_err(|_| "SEED must be a decimal or 0x-prefixed u64".to_string())
    } else {
        input
            .parse()
            .map_err(|_| "SEED must be a decimal or 0x-prefixed u64".to_string())
    }
}

pub fn run_fixture(args: &[String]) -> ExitCode {
    let mut fixture_path = None;
    let mut engine = None;
    let mut json = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--json" => json = true,
            "--engine" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --engine");
                };
                engine = Some(value.as_str());
            }
            value if fixture_path.is_none() => fixture_path = Some(value),
            value => return invalid_input(&format!("unexpected fixture argument '{value}'")),
        }
        index += 1;
    }
    if !json {
        return invalid_input("--fixture requires --json");
    }
    let Some(fixture_path) = fixture_path else {
        return invalid_input(
            "usage: od-runner --fixture CASE.json --engine interpreter|jit --json",
        );
    };
    let backend = match engine {
        Some("interpreter") => Backend::Interpreter,
        Some("jit") => Backend::Jit,
        Some(other) => return invalid_input(&format!("unknown fixture engine '{other}'")),
        None => return invalid_input("--fixture requires --engine interpreter|jit"),
    };

    let input = match fs::read(fixture_path) {
        Ok(input) => input,
        Err(error) => return invalid_input(&format!("could not read fixture: {error}")),
    };
    match fixture_schema_version(&input) {
        Ok(2) => return run_fixture_v2(&input, backend),
        Ok(1) => {}
        Ok(version) => return invalid_input(&format!("unsupported fixture schema {version}")),
        Err(error) => return invalid_input(&error),
    }
    let loaded = match Fixture::from_json(&input) {
        Ok(loaded) => loaded,
        Err(error) => return invalid_input(&format!("invalid fixture: {error}")),
    };
    let result = match execute_fixture(&loaded, backend) {
        Ok(result) => result,
        Err(ExecutionFailure::Unsupported(reason)) => {
            return emit_result(
                unsupported_result(&loaded, backend, reason),
                EXIT_UNSUPPORTED,
            );
        }
        Err(ExecutionFailure::InvalidInput(reason)) => {
            return emit_result(
                invalid_fixture_result(&loaded, backend, reason),
                EXIT_INVALID_INPUT,
            );
        }
        Err(ExecutionFailure::Harness(reason)) => {
            eprintln!("harness failure: {reason}");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
    };
    emit_result(result, 0)
}

fn fixture_schema_version(bytes: &[u8]) -> Result<u64, String> {
    if bytes.len() > od_testkit::v2::MAX_FIXTURE_JSON_BYTES {
        return Err("fixture JSON exceeds the v2 size limit".into());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    value
        .get("schema_version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "fixture requires an unsigned integer schema_version".into())
}

fn run_fixture_v2(bytes: &[u8], backend: Backend) -> ExitCode {
    use od_testkit::v2;
    let loaded = match v2::Fixture::from_json(bytes) {
        Ok(value) => value,
        Err(error) => return invalid_input(&format!("invalid v2 fixture: {error}")),
    };
    let result = if backend == Backend::Interpreter {
        let mut memory = match od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture) {
            Ok(value) => value,
            Err(error) => return invalid_input(&error.to_string()),
        };
        execute_fixture_v2(&loaded, &mut memory)
    } else {
        v2::RunResult {
            schema_version: v2::SCHEMA_VERSION,
            case_id: loaded.fixture.case_id.clone(),
            input_sha256: loaded.input_sha256.clone(),
            profile: loaded.fixture.profile,
            backend,
            outcome: v2::Outcome::Unsupported {
                reason: "schema v2 execution is implemented only for the interpreter".into(),
            },
            instructions_executed: unavailable("engine was not executed"),
            snapshot: unavailable("engine was not executed"),
            build: build_metadata(),
            machine: machine_metadata(),
        }
    };
    let exit = match result.outcome {
        v2::Outcome::Unsupported { .. } => EXIT_UNSUPPORTED,
        v2::Outcome::TimedOut { .. }
        | v2::Outcome::Signaled { .. }
        | v2::Outcome::HarnessError { .. } => EXIT_HARNESS_FAILURE,
        _ => 0,
    };
    match result.to_json() {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(exit),
            Err(error) => {
                eprintln!("harness failure: could not write v2 result: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize v2 result: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn execute_fixture_v2(
    loaded: &od_testkit::v2::LoadedFixture,
    memory: &mut od_testkit::memory::FlatMemory,
) -> od_testkit::v2::RunResult {
    use od_isa::execute::{RunConfig, RunExit, run};
    use od_testkit::v2;
    use std::time::{Duration, Instant};

    let mut state = loaded
        .fixture
        .initial_state
        .to_cpu_for(loaded.fixture.profile)
        .expect("validated fixture state converts to CPU state");
    let start = Instant::now();
    let limit = Duration::from_millis(u64::from(loaded.fixture.wall_timeout_ms));
    let mut timed_out = false;
    // A cooperative deadline is checked at the same instruction boundaries as
    // STOP, before budget/fetch. It cannot interrupt a stalled backend call.
    let execution = run(
        &mut state,
        memory,
        RunConfig {
            instruction_budget: loaded.fixture.instruction_budget,
            stop_rip: loaded.fixture.stop_rip.map(HexU64::get),
            context: od_isa::memory::AccessContext::default(),
        },
        || {
            timed_out = start.elapsed() >= limit;
            timed_out
        },
    );
    let exception = match &execution.exit {
        RunExit::Fault(fault) => Some(od_testkit::result::ExceptionSnapshot {
            vector: fault.vector,
            error_code: fault.error_code.map_or_else(
                || unavailable("exception has no architectural error code"),
                |code| Observation::Available {
                    value: HexU64(u64::from(code)),
                },
            ),
            faulting_rip: HexU64(state.rip),
            fault_address: fault.linear_address.map_or_else(
                || unavailable("exception has no reported linear address"),
                |address| Observation::Available {
                    value: HexU64(address),
                },
            ),
        }),
        _ => None,
    };
    let outcome = match execution.exit {
        RunExit::StopAddress => v2::Outcome::StopAddress {},
        RunExit::InstructionBudgetExhausted => v2::Outcome::InstructionBudgetExhausted {},
        RunExit::RepIterationBudgetExhausted => v2::Outcome::RepIterationBudgetExhausted {},
        RunExit::StopRequested if timed_out => v2::Outcome::TimedOut {
            reason: "cooperative wall-time deadline reached at an instruction boundary".into(),
        },
        RunExit::StopRequested => v2::Outcome::StopRequested {},
        RunExit::Fault(_) => v2::Outcome::Exception {},
        RunExit::Unsupported { opcode } => v2::Outcome::Unsupported {
            reason: format!(
                "interpreter does not support opcode 0x{opcode:02x} at RIP 0x{:016x}",
                state.rip
            ),
        },
        RunExit::MemoryError(error) => v2::Outcome::HarnessError {
            reason: format!("fixture memory backend: {error:?}"),
        },
    };
    v2::RunResult {
        schema_version: v2::SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend: Backend::Interpreter,
        outcome,
        instructions_executed: Observation::Available {
            value: HexU64(execution.instructions_executed),
        },
        snapshot: Observation::Available {
            value: v2::Snapshot {
                state: v2::IntegerState::from_cpu_for(&state, loaded.fixture.profile),
                // The interpreter tracks architectural flag definedness as
                // instructions retire, including undefined outputs such as AF
                // after logical operations.
                rflags_defined: HexU64(execution.rflags_defined),
                memory: memory.snapshot(),
                exception: Observation::Available { value: exception },
            },
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

pub fn compare_results(args: &[String]) -> ExitCode {
    let Some(left_path) = args.first() else {
        return invalid_input(
            "usage: od-runner --compare-results RESULT_A.json RESULT_B.json --case CASE.json --json",
        );
    };
    let Some(right_path) = args.get(1) else {
        return invalid_input("--compare-results requires two result paths");
    };
    let mut case_path = None;
    let mut json = false;
    let mut index = 2;
    while index < args.len() {
        match args[index].as_str() {
            "--case" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return invalid_input("missing value for --case");
                };
                case_path = Some(value.as_str());
            }
            "--json" => json = true,
            value => return invalid_input(&format!("unexpected compare argument '{value}'")),
        }
        index += 1;
    }
    if !json {
        return invalid_input("--compare-results requires --json");
    }
    let Some(case_path) = case_path else {
        return invalid_input("--compare-results requires --case CASE.json");
    };

    let case_bytes = match fs::read(case_path) {
        Ok(bytes) => bytes,
        Err(error) => return invalid_input(&format!("could not read case: {error}")),
    };
    match fixture_schema_version(&case_bytes) {
        Ok(2) => return compare_results_v2(&case_bytes, left_path, right_path),
        Ok(1) => {}
        Ok(version) => return invalid_input(&format!("unsupported fixture schema {version}")),
        Err(error) => return invalid_input(&error),
    }
    let fixture = match Fixture::from_json(&case_bytes) {
        Ok(fixture) => fixture,
        Err(error) => return invalid_input(&format!("invalid case: {error}")),
    };
    let left = match read_result(left_path) {
        Ok(result) => result,
        Err(error) => return invalid_input(&format!("invalid left result: {error}")),
    };
    let right = match read_result(right_path) {
        Ok(result) => result,
        Err(error) => return invalid_input(&format!("invalid right result: {error}")),
    };
    let report = match compare_result_values(&fixture, &left, &right) {
        Ok(report) => report,
        Err(CompareError::Incomplete {
            status: CompletionStatus::Unsupported,
            ..
        }) => {
            eprintln!("comparison unsupported: one result has unsupported status");
            return ExitCode::from(EXIT_UNSUPPORTED);
        }
        Err(CompareError::Incomplete { status, .. }) => {
            eprintln!("harness failure: result cannot be compared because it ended as {status:?}");
            return ExitCode::from(EXIT_HARNESS_FAILURE);
        }
        Err(error) => return invalid_input(&format!("results are not comparable: {error:?}")),
    };
    let mismatch = !report.matches();
    emit_report(report, if mismatch { EXIT_MISMATCH } else { 0 })
}

fn read_result(path: &str) -> Result<RunResult, String> {
    let bytes = fs::read(path).map_err(|error| error.to_string())?;
    RunResult::from_json(&bytes).map_err(|error| error.to_string())
}

fn compare_results_v2(case: &[u8], left_path: &str, right_path: &str) -> ExitCode {
    use od_testkit::v2;
    let loaded = match v2::Fixture::from_json(case) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error.to_string()),
    };
    let read = |path| -> Result<v2::RunResult, String> {
        let raw = fs::read(path).map_err(|e| e.to_string())?;
        v2::RunResult::from_json(&raw).map_err(|e| e.to_string())
    };
    let (left, right) = match (read(left_path), read(right_path)) {
        (Ok(left), Ok(right)) => (left, right),
        (Err(error), _) | (_, Err(error)) => return invalid_input(&error),
    };
    for result in [&left, &right] {
        if let Err(error) = result.validate_for(&loaded) {
            return invalid_input(&error.to_string());
        }
    }
    for result in [&left, &right] {
        match result.outcome {
            v2::Outcome::Unsupported { .. } => return ExitCode::from(EXIT_UNSUPPORTED),
            v2::Outcome::TimedOut { .. }
            | v2::Outcome::Signaled { .. }
            | v2::Outcome::HarnessError { .. } => return ExitCode::from(EXIT_HARNESS_FAILURE),
            _ => {}
        }
    }
    let differences = match v2::compare(&loaded, &left, &right) {
        Ok(value) => value,
        Err(error) => return invalid_input(&error.to_string()),
    };
    println!(
        "{}",
        serde_json::json!({"schema_version":2,"case_id":loaded.fixture.case_id,"input_sha256":loaded.input_sha256,"matches":differences.is_empty(),"differences":differences})
    );
    ExitCode::from(if differences.is_empty() {
        0
    } else {
        EXIT_MISMATCH
    })
}

enum ExecutionFailure {
    InvalidInput(String),
    Unsupported(String),
    Harness(String),
}

fn interpreter_failure(error: od_isa::DecodeErrorOrExecute) -> ExecutionFailure {
    let reason = format!("interpreter: {error:?}");
    if matches!(
        error,
        od_isa::DecodeErrorOrExecute::Execute(od_isa::ExecuteError::UnexpectedCoreExit { .. })
    ) {
        ExecutionFailure::Harness(reason)
    } else {
        ExecutionFailure::Unsupported(reason)
    }
}

fn execute_fixture(
    loaded: &LoadedFixture,
    backend: Backend,
) -> Result<RunResult, ExecutionFailure> {
    let fixture = &loaded.fixture;
    let code = fixture
        .program
        .bytes()
        .map_err(|error| ExecutionFailure::InvalidInput(error.to_string()))?;
    let mut state = initial_state(fixture).map_err(ExecutionFailure::InvalidInput)?;
    let execution = match backend {
        Backend::Interpreter => {
            interpret(&code, &mut state, fixture.instruction_budget).map_err(interpreter_failure)?
        }
        Backend::Jit => {
            od_isa::decode(&code).map_err(|error| {
                ExecutionFailure::Unsupported(format!("JIT cannot decode fixture: {error:?}"))
            })?;
            execute_jit(&code, &mut state, fixture.instruction_budget)?
        }
        Backend::NativeX86 => {
            return Err(ExecutionFailure::Unsupported(
                "native x86 execution is implemented in a later stage".to_string(),
            ));
        }
    };
    Ok(completed_result(loaded, backend, execution))
}

fn initial_state(fixture: &od_testkit::case::Fixture) -> Result<CpuState, String> {
    let initial = &fixture.initial_state;
    let rip = usize::try_from(initial.rip.get())
        .map_err(|_| "fixture RIP does not fit host usize".to_string())?;
    Ok(CpuState {
        gpr: [
            initial.rax.get(),
            initial.rcx.get(),
            initial.rdx.get(),
            initial.rbx.get(),
        ],
        rip,
        rflags: initial.rflags.get(),
    })
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn execute_jit(
    code: &[u8],
    state: &mut CpuState,
    budget: u64,
) -> Result<ExecutionResult, ExecutionFailure> {
    super::execute_legacy_jit(code, state, budget).map_err(ExecutionFailure::Harness)
}

#[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
fn execute_jit(
    _code: &[u8],
    _state: &mut CpuState,
    _budget: u64,
) -> Result<ExecutionResult, ExecutionFailure> {
    Err(ExecutionFailure::Unsupported(
        "ARM64 JIT execution requires an aarch64 macOS host".to_string(),
    ))
}

fn completed_result(
    loaded: &LoadedFixture,
    backend: Backend,
    execution: ExecutionResult,
) -> RunResult {
    let state = execution.state;
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend,
        status: CompletionStatus::Completed,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: Observation::Available {
            value: match execution.reason {
                od_isa::ExitReason::ReachedCodeEnd => ExitReason::ReachedCodeEnd,
                od_isa::ExitReason::InstructionBudgetExhausted => {
                    ExitReason::InstructionBudgetExhausted
                }
                od_isa::ExitReason::StopRequested => ExitReason::StopRequested,
            },
        },
        instructions_executed: Observation::Available {
            value: HexU64(execution.instructions_executed),
        },
        snapshot: CpuSnapshot {
            gpr: Observation::Available {
                value: GprSnapshot {
                    rax: HexU64(state.gpr[0]),
                    rcx: HexU64(state.gpr[1]),
                    rdx: HexU64(state.gpr[2]),
                    rbx: HexU64(state.gpr[3]),
                },
            },
            rip: Observation::Available {
                value: HexU64(state.rip as u64),
            },
            rflags: Observation::Available {
                value: HexU64(state.rflags),
            },
            memory: unavailable("S0-09 register-only profile does not capture memory"),
            exception: unavailable("S0-09 profile does not capture exceptions"),
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unsupported_result(loaded: &LoadedFixture, backend: Backend, reason: String) -> RunResult {
    unavailable_result(loaded, backend, CompletionStatus::Unsupported, reason)
}

fn invalid_fixture_result(loaded: &LoadedFixture, backend: Backend, reason: String) -> RunResult {
    unavailable_result(loaded, backend, CompletionStatus::InvalidInput, reason)
}

fn unavailable_result(
    loaded: &LoadedFixture,
    backend: Backend,
    status: CompletionStatus,
    reason: String,
) -> RunResult {
    RunResult {
        schema_version: RESULT_SCHEMA_VERSION,
        case_id: loaded.fixture.case_id.clone(),
        input_sha256: loaded.input_sha256.clone(),
        profile: loaded.fixture.profile,
        backend,
        status,
        expected_exit: loaded.fixture.expected_exit,
        exit_reason: unavailable(&reason),
        instructions_executed: unavailable(&reason),
        snapshot: CpuSnapshot {
            gpr: unavailable(&reason),
            rip: unavailable(&reason),
            rflags: unavailable(&reason),
            memory: unavailable(&reason),
            exception: unavailable(&reason),
        },
        build: build_metadata(),
        machine: machine_metadata(),
    }
}

fn unavailable<T>(reason: &str) -> Observation<T> {
    Observation::Unavailable {
        reason: reason.to_string(),
    }
}

fn build_metadata() -> BuildMetadata {
    BuildMetadata {
        revision: option_env!("OPEN_DESKTOP_GIT_REVISION")
            .unwrap_or("unavailable")
            .to_string(),
        target: build_target(),
        profile: if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
        .to_string(),
        compiler: option_env!("RUSTC_VERSION")
            .unwrap_or("rustc version unavailable at build time")
            .to_string(),
    }
}

fn build_target() -> String {
    let arch = std::env::consts::ARCH;
    match (arch, std::env::consts::OS) {
        ("aarch64", "macos") => "aarch64-apple-darwin".to_string(),
        ("x86_64", "macos") => "x86_64-apple-darwin".to_string(),
        ("x86_64", "linux") => "x86_64-unknown-linux-gnu".to_string(),
        ("aarch64", "linux") => "aarch64-unknown-linux-gnu".to_string(),
        _ => format!("{arch}-unknown-{}", std::env::consts::OS),
    }
}

fn machine_metadata() -> MachineMetadata {
    MachineMetadata {
        host_os: std::env::consts::OS.to_string(),
        host_arch: std::env::consts::ARCH.to_string(),
        cpu_model: None,
        kernel: None,
        page_size: None,
    }
}

fn emit_result(result: RunResult, code: u8) -> ExitCode {
    match result.to_json() {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(code),
            Err(error) => {
                eprintln!("harness failure: could not write JSON result: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize result: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn emit_report(report: ComparisonReport, code: u8) -> ExitCode {
    match serde_json::to_vec(&report) {
        Ok(json) => match write_json(&json) {
            Ok(()) => ExitCode::from(code),
            Err(error) => {
                eprintln!("harness failure: could not write JSON report: {error}");
                ExitCode::from(EXIT_HARNESS_FAILURE)
            }
        },
        Err(error) => {
            eprintln!("harness failure: could not serialize comparison: {error}");
            ExitCode::from(EXIT_HARNESS_FAILURE)
        }
    }
}

fn write_json(json: &[u8]) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    stdout.write_all(json)?;
    stdout.write_all(b"\n")
}

fn invalid_input(message: &str) -> ExitCode {
    eprintln!("invalid input: {message}");
    ExitCode::from(EXIT_INVALID_INPUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_testkit::v2;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Files(PathBuf);

    impl Files {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "od-runner-cli-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&root).unwrap();
            Self(root)
        }

        fn path(&self, name: &str) -> String {
            self.0.join(name).to_str().unwrap().to_string()
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn old() -> LoadedFixture {
        Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/s0_09_mov_imm64.case.json"
        ))
        .unwrap()
    }

    #[test]
    fn interpreter_internal_invariant_failure_is_a_harness_error() {
        use od_isa::{DecodeErrorOrExecute, ExecuteError};
        assert!(matches!(
            interpreter_failure(DecodeErrorOrExecute::Execute(
                ExecuteError::UnexpectedCoreExit {
                    exit: od_isa::execute::RunExit::InstructionBudgetExhausted,
                }
            )),
            ExecutionFailure::Harness(_)
        ));
        assert!(matches!(
            interpreter_failure(DecodeErrorOrExecute::Execute(
                ExecuteError::InvalidInstructionPointer { rip: 1 }
            )),
            ExecutionFailure::Unsupported(_)
        ));
    }

    #[test]
    fn real_v2_execution_projects_injected_page_fault_and_backend_error_distinctly() {
        use od_isa::memory::{AccessKind, ArchitecturalFault, MemoryError};
        let input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap();
        for error in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(6),
                linear_address: Some(0x1000),
            }),
            MemoryError::Backend("injected".into()),
        ] {
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&input.fixture).unwrap();
            let before = memory.snapshot();
            memory.inject_once(0x1000, AccessKind::Write, error.clone());
            let result = execute_fixture_v2(&input, &mut memory);
            result.validate_for(&input).unwrap();
            let Observation::Available { value: observed } = &result.snapshot else {
                panic!("observed interpreter state");
            };
            assert_eq!(observed.memory, before);
            assert_eq!(observed.state.gpr[0], HexU64(12));
            assert_eq!(observed.state.rip, HexU64(0x4007));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(2) }
            );
            if matches!(error, MemoryError::Architectural(_)) {
                assert!(matches!(result.outcome, v2::Outcome::Exception {}));
                let Observation::Available { value: Some(fault) } = &observed.exception else {
                    panic!("observed page fault");
                };
                assert_eq!(fault.vector, 14);
                assert_eq!(
                    fault.error_code,
                    Observation::Available { value: HexU64(6) }
                );
                assert_eq!(
                    fault.fault_address,
                    Observation::Available {
                        value: HexU64(0x1000)
                    }
                );
                assert_eq!(fault.faulting_rip, HexU64(0x4007));
            } else {
                assert!(matches!(result.outcome, v2::Outcome::HarnessError { .. }));
                assert_eq!(observed.exception, Observation::Available { value: None });
            }
        }
    }

    // Synthetic snapshots exercise CLI plumbing and validation only, not CPU execution.
    fn synthetic(input: &v2::LoadedFixture) -> v2::RunResult {
        let mut state = input.fixture.initial_state.clone();
        state.rip = input.fixture.stop_rip.unwrap();
        v2::RunResult {
            schema_version: 2,
            case_id: input.fixture.case_id.clone(),
            input_sha256: input.input_sha256.clone(),
            profile: input.fixture.profile,
            backend: Backend::Interpreter,
            outcome: v2::Outcome::StopAddress {},
            instructions_executed: Observation::Unavailable {
                reason: "synthetic infrastructure test".into(),
            },
            snapshot: Observation::Available {
                value: v2::Snapshot {
                    state,
                    rflags_defined: HexU64(v2::INTEGER_FLAGS),
                    memory: od_testkit::memory::FlatMemory::from_fixture(&input.fixture)
                        .unwrap()
                        .snapshot(),
                    exception: Observation::Available { value: None },
                },
            },
            build: build_metadata(),
            machine: machine_metadata(),
        }
    }

    fn compare(files: &Files, case: &[u8], left: &[u8], right: &[u8]) -> ExitCode {
        for (name, bytes) in [("case", case), ("left", left), ("right", right)] {
            fs::write(files.path(name), bytes).unwrap();
        }
        compare_results(&[
            files.path("left"),
            files.path("right"),
            "--case".into(),
            files.path("case"),
            "--json".into(),
        ])
    }

    #[test]
    fn conversion_preserves_input_and_refuses_existing_output() {
        let files = Files::new();
        let bytes = old().fixture.to_json().unwrap();
        fs::write(files.path("v1"), &bytes).unwrap();
        assert_eq!(
            convert_fixture_v1(&[files.path("v1"), files.path("v2")]),
            ExitCode::SUCCESS
        );
        let output = fs::read(files.path("v2")).unwrap();
        let converted = v2::Fixture::from_json(&output).unwrap();
        assert_eq!(
            converted.fixture.converted_from_v1_sha256,
            Some(od_testkit::sha256_hex(&bytes))
        );
        assert_ne!(converted.input_sha256, od_testkit::sha256_hex(&bytes));
        for target in ["v1", "v2"] {
            assert_eq!(
                convert_fixture_v1(&[files.path("v1"), files.path(target)]),
                ExitCode::from(EXIT_INVALID_INPUT)
            );
        }
        assert_eq!(fs::read(files.path("v1")).unwrap(), bytes);
        assert_eq!(fs::read(files.path("v2")).unwrap(), output);
        assert_eq!(
            convert_fixture_v1(&[files.path("v2"), files.path("unused")]),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        assert!(!files.0.join("unused").exists());
    }

    #[test]
    fn comparison_dispatches_schemas_and_reports_mismatch_or_invalid_binding() {
        let files = Files::new();
        let v1 = old();
        let legacy = execute_fixture(&v1, Backend::Interpreter)
            .unwrap_or_else(|_| panic!("legacy fixture execution"));
        let legacy_bytes = legacy.to_json().unwrap();
        // Preserve exact source bytes: result digests bind to these bytes, not reserialization.
        let v1_bytes = include_bytes!("../../../tests/fixtures/x86/s0_09_mov_imm64.case.json");
        assert_eq!(
            compare(&files, v1_bytes, &legacy_bytes, &legacy_bytes),
            ExitCode::SUCCESS
        );
        let case = v2::Fixture::convert_v1(&v1).unwrap().to_json().unwrap();
        let loaded = v2::Fixture::from_json(&case).unwrap();
        let baseline = synthetic(&loaded);
        let baseline_bytes = baseline.to_json().unwrap();
        assert_eq!(
            compare(&files, &case, &baseline_bytes, &baseline_bytes),
            ExitCode::SUCCESS
        );
        let mut changed = baseline.clone();
        if let Observation::Available { value } = &mut changed.snapshot {
            value.state.gpr[15].0 ^= 1;
        }
        assert_eq!(
            compare(&files, &case, &baseline_bytes, &changed.to_json().unwrap()),
            ExitCode::from(EXIT_MISMATCH)
        );
        assert_eq!(
            compare(&files, &case, &legacy_bytes, &baseline_bytes),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        let mut different_whitespace = case.clone();
        different_whitespace.push(b'\n');
        assert_eq!(
            compare(
                &files,
                &different_whitespace,
                &baseline_bytes,
                &baseline_bytes
            ),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
        let mut unknown: serde_json::Value = serde_json::from_slice(&case).unwrap();
        unknown["schema_version"] = serde_json::json!(3);
        assert_eq!(
            compare(
                &files,
                &serde_json::to_vec(&unknown).unwrap(),
                &baseline_bytes,
                &baseline_bytes
            ),
            ExitCode::from(EXIT_INVALID_INPUT)
        );
    }

    #[test]
    fn v2_comparison_validates_both_bindings_before_returning_status() {
        let files = Files::new();
        let case = v2::Fixture::convert_v1(&old()).unwrap().to_json().unwrap();
        let loaded = v2::Fixture::from_json(&case).unwrap();
        let baseline = synthetic(&loaded);
        for (outcome, exit) in [
            (
                v2::Outcome::Unsupported {
                    reason: "test".into(),
                },
                EXIT_UNSUPPORTED,
            ),
            (
                v2::Outcome::TimedOut {
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
            (
                v2::Outcome::Signaled {
                    signal: 11,
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
            (
                v2::Outcome::HarnessError {
                    reason: "test".into(),
                },
                EXIT_HARNESS_FAILURE,
            ),
        ] {
            let mut incomplete = baseline.clone();
            incomplete.outcome = outcome;
            incomplete.snapshot = unavailable("synthetic test");
            let incomplete = incomplete.to_json().unwrap();
            let good = baseline.to_json().unwrap();
            for (left, right) in [(&incomplete, &good), (&good, &incomplete)] {
                assert_eq!(compare(&files, &case, left, right), ExitCode::from(exit));
            }
            for kind in ["digest", "layout"] {
                let mut invalid = baseline.clone();
                if kind == "digest" {
                    invalid.input_sha256 = "0".repeat(64);
                } else if let Observation::Available { value } = &mut invalid.snapshot {
                    value.memory[0].guest_address.0 += 1;
                }
                let invalid = invalid.to_json().unwrap();
                for (left, right) in [(&incomplete, &invalid), (&invalid, &incomplete)] {
                    assert_eq!(
                        compare(&files, &case, left, right),
                        ExitCode::from(EXIT_INVALID_INPUT)
                    );
                }
            }
        }
    }

    #[test]
    fn fs_base_profile_cli_result_contains_observed_base_and_alias_load() {
        let input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_22a_fs_base_qword_load_alias.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&input.fixture).unwrap();
        let result = execute_fixture_v2(&input, &mut memory);
        result.validate_for(&input).unwrap();
        let json = result.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(
            value["snapshot"]["value"]["state"]["fs_base"],
            "0xffff800000000000"
        );
        assert_eq!(
            value["snapshot"]["value"]["state"]["gpr"][0],
            "0x8877665544332211"
        );
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(1) }
        );
    }

    #[test]
    fn x87_fadd_fixture_runner_executes_all_register_sources_and_rejects_bad_tag() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        for source in 0u8..=7 {
            let mut input = v2::Fixture::from_json(include_bytes!(
                "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
            ))
            .unwrap()
            .fixture;
            input.case_id = format!("local_fadd_st0_st{source}");
            input.profile = v2::Profile::LongX87StateV1;
            input.instruction_budget = 1;
            input.initial_state.rip = HexU64(0x4000);
            input.stop_rip = Some(HexU64(0x4002));
            input.expected_exit = v2::ExpectedExit::StopAddress;
            let code = input
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x4000);
            code.length = 2;
            code.initial_bytes_hex = format!("d8c{source:x}");
            code.comparison_mask_hex = "ffff".into();

            let top = if source == 7 || source == 1 {
                7
            } else {
                (source + 2) & 7
            };
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0400;
            x87.tag_word = 0;
            let one = normal(0x8000_0000_0000_0000, 0x3fff);
            x87.registers[usize::from(top)] = one;
            x87.registers[usize::from((top + source) & 7)] = one;
            input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

            let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let result = execute_fixture_v2(&loaded, &mut memory);
            result.validate_for(&loaded).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "source {source}, TOP {top}: {:?}",
                result.outcome
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(1) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("x87 FADD result must be observable");
            };
            let observed_x87 = value.state.x87.unwrap();
            assert_eq!(
                observed_x87.registers[usize::from(top)],
                "00000000000000800040"
            );
            assert_eq!((observed_x87.status_word.0 >> 11) & 7, u64::from(top));
            assert_eq!(observed_x87.status_word.0 & (1 << 9), 0);
            assert_eq!(observed_x87.instruction_pointer, HexU64(0x4000));
        }

        let mut rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        rejected.profile = v2::Profile::LongX87StateV1;
        rejected.instruction_budget = 1;
        rejected.initial_state.rip = HexU64(0x4000);
        rejected.stop_rip = Some(HexU64(0x4002));
        rejected.expected_exit = v2::ExpectedExit::StopAddress;
        let code = rejected
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x4000);
        code.length = 2;
        code.initial_bytes_hex = "d8c7".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0;
        x87.registers[0] = normal(0x8000_0000_0000_0000, 0x3fff);
        x87.registers[7] = normal(0x8000_0000_0000_0000, 0x3fff);
        x87.tag_word |= 3 << (7 * 2);
        rejected.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&rejected.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        let Observation::Available { value } = result.snapshot else {
            panic!("rejected x87 state must remain observable");
        };
        assert_eq!(value.state.x87.unwrap(), v2::X87Snapshot::from(&x87));
    }

    #[test]
    fn x87_faddp_fixture_runner_covers_every_index_and_top() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        let one = normal(0x8000_0000_0000_0000, 0x3fff);
        let two = "00000000000000800040";
        for top in 0u8..=7 {
            for destination in 0u8..=7 {
                let old_top = top;
                let physical_destination = top.wrapping_add(destination) & 7;
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_faddp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5000);
                input.stop_rip = Some(HexU64(0x5002));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5000);
                code.length = 2;
                code.initial_bytes_hex = format!("dec{destination:x}");
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4500 | 0x0200;
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [physical as u8; 10];
                }
                x87.registers[usize::from(old_top)] = one;
                x87.registers[usize::from(physical_destination)] = one;
                x87.set_tag(old_top, od_isa::state::X87Tag::Valid);
                x87.set_tag(physical_destination, od_isa::state::X87Tag::Valid);
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(
                    matches!(result.outcome, v2::Outcome::StopAddress {}),
                    "ST({destination}), TOP={top}: {:?}",
                    result.outcome
                );
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FADDP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[usize::from(physical_destination)] =
                    normal(0x8000_0000_0000_0000, 0x4000);
                expected.set_tag(old_top, od_isa::state::X87Tag::Empty);
                let new_top = old_top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5000;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
                assert_eq!(observed.registers[usize::from(physical_destination)], two);
            }
        }
    }

    #[test]
    fn x87_fmulp_fixture_runner_covers_every_index_top_and_square_alias() {
        fn normal(significand: u64, exponent: u16) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        }

        let one_and_half = normal(0xc000_0000_0000_0000, 0x3fff);
        let two = normal(0x8000_0000_0000_0000, 0x4000);
        let three = normal(0xc000_0000_0000_0000, 0x4000);
        let two_and_quarter = normal(0x9000_0000_0000_0000, 0x4000);
        for top in 0u8..=7 {
            for destination in 0u8..=7 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fmulp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5300);
                input.stop_rip = Some(HexU64(0x5302));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5300);
                code.length = 2;
                code.initial_bytes_hex = format!("de{:x}", 0xc8 | destination);
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [0x20 + physical as u8; 10];
                }
                x87.registers[old_top] = one_and_half;
                x87.set_tag(top, od_isa::state::X87Tag::Valid);
                if destination != 0 {
                    x87.registers[physical_destination] = two;
                    x87.set_tag(physical_destination as u8, od_isa::state::X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(
                    matches!(result.outcome, v2::Outcome::StopAddress {}),
                    "ST({destination}), TOP={top}: {:?}",
                    result.outcome
                );
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FMULP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] = if destination == 0 {
                    two_and_quarter
                } else {
                    three
                };
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5300;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        // The fixture schema accepts DE C8 with an explicit unsupported exit
        // and no stop address. This instance rejects atomically because the
        // finite product is inexact; the i=0 exact square succeeds above.
        let mut input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        input.case_id = "local_fmulp_i0_inexact_rejection".into();
        input.profile = v2::Profile::LongX87StateV1;
        input.instruction_budget = 1;
        input.initial_state.rip = HexU64(0x5400);
        input.stop_rip = None;
        input.expected_exit = v2::ExpectedExit::Unsupported;
        let code = input
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x5400);
        code.length = 2;
        code.initial_bytes_hex = "dec8".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = normal(0x8000_0000_0000_0001, 0x3fff);
        x87.set_tag(0, od_isa::state::X87Tag::Valid);
        input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("unsupported x87 square rejection must be observable");
        };
        assert_eq!(value.state, loaded.fixture.initial_state);
    }

    #[test]
    fn x87_fsubrp_fixture_runner_covers_every_index_top_and_alias_rejection() {
        let normal = |significand: u64, exponent: u16| {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..].copy_from_slice(&exponent.to_le_bytes());
            bytes
        };
        let one = normal(0x8000_0000_0000_0000, 0x3fff);
        let one_and_half = normal(0xc000_0000_0000_0000, 0x3fff);
        let half = normal(0x8000_0000_0000_0000, 0x3ffe);
        for top in 0u8..8 {
            for destination in 1u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fsubrp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x7100);
                input.stop_rip = Some(HexU64(0x7102));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x7100);
                code.length = 2;
                code.initial_bytes_hex = format!("dee{destination:x}");
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word =
                    u16::MAX & !(3 << (old_top * 2)) & !(3 << (physical_destination * 2));
                for physical in 0..8 {
                    x87.registers[physical] = [physical as u8; 10];
                }
                x87.registers[old_top] = one_and_half;
                x87.registers[physical_destination] = one;
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FSUBRP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] = half;
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x7100;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        for top in 0u8..8 {
            let mut input = v2::Fixture::from_json(include_bytes!(
                "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
            ))
            .unwrap()
            .fixture;
            input.case_id = format!("local_fsubrp_alias_top{top}");
            input.profile = v2::Profile::LongX87StateV1;
            input.instruction_budget = 1;
            input.initial_state.rip = HexU64(0x7200);
            input.stop_rip = None;
            input.expected_exit = v2::ExpectedExit::Unsupported;
            let code = input
                .memory
                .iter_mut()
                .find(|region| region.region_id == "code")
                .unwrap();
            code.guest_address = HexU64(0x7200);
            code.length = 2;
            code.initial_bytes_hex = "dee0".into();
            code.comparison_mask_hex = "ffff".into();
            let mut x87 = od_isa::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = one;
            input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
            let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
            let result = execute_fixture_v2(&loaded, &mut memory);
            result.validate_for(&loaded).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("unsupported x87 alias remains observable");
            };
            assert_eq!(value.state, loaded.fixture.initial_state);
        }
    }

    #[test]
    fn x87_fdivp_fixture_runner_covers_every_index_top_and_inexact_rejection() {
        fn normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
            let mut bytes = [0; 10];
            bytes[..8].copy_from_slice(&significand.to_le_bytes());
            bytes[8..]
                .copy_from_slice(&(exponent | if negative { 0x8000 } else { 0 }).to_le_bytes());
            bytes
        }
        let fifteen = normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = normal(0x8000_0000_0000_0000, 0x3fff, false);

        for top in 0u8..8 {
            for destination in 0u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + destination) & 7);
                let mut input = v2::Fixture::from_json(include_bytes!(
                    "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
                ))
                .unwrap()
                .fixture;
                input.case_id = format!("local_fdivp_st{destination}_top{top}");
                input.profile = v2::Profile::LongX87StateV1;
                input.instruction_budget = 1;
                input.initial_state.rip = HexU64(0x5500);
                input.stop_rip = Some(HexU64(0x5502));
                input.expected_exit = v2::ExpectedExit::StopAddress;
                let code = input
                    .memory
                    .iter_mut()
                    .find(|region| region.region_id == "code")
                    .unwrap();
                code.guest_address = HexU64(0x5500);
                code.length = 2;
                code.initial_bytes_hex = format!("de{:x}", 0xf8 | destination);
                code.comparison_mask_hex = "ffff".into();

                let mut x87 = od_isa::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [0x30 + physical as u8; 10];
                }
                if destination == 0 {
                    x87.registers[old_top] = normal(0xd123_4567_89ab_cdef, 0x4000, top & 1 != 0);
                } else {
                    x87.registers[physical_destination] = fifteen;
                    x87.set_tag((top + destination) & 7, od_isa::state::X87Tag::Valid);
                    x87.registers[old_top] = three;
                }
                x87.set_tag(top, od_isa::state::X87Tag::Valid);
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));

                let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
                let mut memory =
                    od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
                let result = execute_fixture_v2(&loaded, &mut memory);
                result.validate_for(&loaded).unwrap();
                assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
                assert_eq!(
                    result.instructions_executed,
                    Observation::Available { value: HexU64(1) }
                );
                let Observation::Available { value } = result.snapshot else {
                    panic!("x87 FDIVP result must be observable");
                };
                let observed = value.state.x87.unwrap();
                let mut expected = x87;
                expected.registers[physical_destination] =
                    if destination == 0 { one } else { five };
                expected.set_tag(top, od_isa::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected.status_word =
                    (expected.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
                expected.instruction_pointer = 0x5500;
                assert_eq!(observed, v2::X87Snapshot::from(&expected));
            }
        }

        let mut input = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/mov_load_add_store.case.json"
        ))
        .unwrap()
        .fixture;
        input.case_id = "local_fdivp_inexact_rejection".into();
        input.profile = v2::Profile::LongX87StateV1;
        input.instruction_budget = 1;
        input.initial_state.rip = HexU64(0x5600);
        input.stop_rip = None;
        input.expected_exit = v2::ExpectedExit::Unsupported;
        let code = input
            .memory
            .iter_mut()
            .find(|region| region.region_id == "code")
            .unwrap();
        code.guest_address = HexU64(0x5600);
        code.length = 2;
        code.initial_bytes_hex = "def9".into();
        code.comparison_mask_hex = "ffff".into();
        let mut x87 = od_isa::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = normal(0xc000_0000_0000_0000, 0x4000, false);
        x87.registers[1] = normal(0x8000_0000_0000_0000, 0x3fff, false);
        input.initial_state.x87 = Some(v2::X87Snapshot::from(&x87));
        let loaded = v2::Fixture::from_json(&input.to_json().unwrap()).unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&loaded.fixture).unwrap();
        let result = execute_fixture_v2(&loaded, &mut memory);
        result.validate_for(&loaded).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("inexact FDIVP state must remain observable");
        };
        assert_eq!(value.state, loaded.fixture.initial_state);
    }

    #[test]
    fn x87_fucom_literal_fixtures_run_success_and_atomic_rhs_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOM literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4000));
        assert_eq!(after.tag_word, before.tag_word);
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x4000);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ag_fucom_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOM rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn x87_fucomp_literal_fixtures_run_compare_pop_and_atomic_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMP literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x4800));
        assert_eq!(after.tag_word, HexU64(0xfff3));
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x4800);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ah_fucomp_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMP rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn x87_fucompp_literal_fixtures_run_compare_double_pop_and_atomic_rejection() {
        let success = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_equal_fnstsw.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&success.fixture).unwrap();
        let result = execute_fixture_v2(&success, &mut memory);
        result.validate_for(&success).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(2) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMPP literal success fixture must be observable");
        };
        let before = success.fixture.initial_state.x87.as_ref().unwrap();
        let after = value.state.x87.as_ref().unwrap();
        assert_eq!(after.status_word, HexU64(0x5000));
        assert_eq!(after.tag_word, HexU64(0xffff));
        assert_eq!(after.registers, before.registers);
        assert_eq!(value.state.gpr[0].get() & 0xffff, 0x5000);

        let rejected = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_30ai_fucompp_invalid_rhs_tag.case.json"
        ))
        .unwrap();
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&rejected.fixture).unwrap();
        let result = execute_fixture_v2(&rejected, &mut memory);
        result.validate_for(&rejected).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("FUCOMPP rejected fixture must preserve an observable state");
        };
        assert_eq!(value.state, rejected.fixture.initial_state);
    }

    #[test]
    fn fistp_m32int_literal_fixtures_store_and_reject_without_commit() {
        for (name, bytes, expected_rax) in [
            (
                "s2_31b_fistp_m32int_success.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
            ),
            (
                "s2_31b_fistp_m32int_fnstsw.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4f20,
            ),
            (
                "s2_31c_fist_m32int_success.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
            ),
            (
                "s2_31c_fist_m32int_fnstsw.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4720,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(if name.contains("fnstsw") { 2 } else { 1 })
                },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            let before = fixture.fixture.initial_state.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            let is_fist = name.contains("31c");
            let expected_fsw = if is_fist { 0x4720 } else { 0x4f20 };
            let expected_tags = if is_fist { 0xfffc } else { 0xffff };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, HexU64(expected_tags), "{name}");
            assert_eq!(after.registers, before.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let left = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_left")
                .unwrap();
            let right = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            assert_eq!(left.bytes_hex, "02", "{name}");
            assert_eq!(right.bytes_hex, "000000eeff11223344556677889900", "{name}");
        }

        for (name, bytes) in [
            (
                "s2_31b_fistp_m32int_invalid_tag.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_invalid_tag.case.json"
                )
                .as_slice(),
            ),
            (
                "s2_31b_fistp_m32int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31b_fistp_m32int_overflow.case.json"
                )
                .as_slice(),
            ),
            (
                "s2_31c_fist_m32int_invalid_tag.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_invalid_tag.case.json"
                )
                .as_slice(),
            ),
            (
                "s2_31c_fist_m32int_overflow.case.json",
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_overflow.case.json"
                )
                .as_slice(),
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} must have an observable state");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
        }

        let mut nearby_form: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
        ))
        .unwrap();
        nearby_form["memory"][0]["initial_bytes_hex"] = serde_json::Value::String("db11".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&nearby_form).unwrap()).is_err(),
            "the literal schema must reject nearby DB ModR/M forms"
        );

        let mut exact_min_must_execute: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31c_fist_m32int_success.case.json"
        ))
        .unwrap();
        exact_min_must_execute["initial_state"]["x87"]["registers"][0] =
            serde_json::Value::String("00000000000000801ec0".into());
        exact_min_must_execute["stop_rip"] = serde_json::Value::Null;
        exact_min_must_execute["expected_exit"] = serde_json::Value::String("unsupported".into());
        assert!(
            v2::Fixture::from_json(&serde_json::to_vec(&exact_min_must_execute).unwrap()).is_err(),
            "the schema must not classify exact negative 2^31 as an overflow rejection"
        );
    }

    #[test]
    fn fstp_m64fp_literal_fixtures_store_then_pop_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31l_fstp_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31l fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31l fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], "000000000000f83f");
        }
    }

    #[test]
    fn fstp_m80fp_literal_fixtures_store_raw_binary80_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31q fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31q fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4d00));
            assert_eq!(after.tag_word, HexU64(0xffff));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..28], "00000000000000c0ff3f");
        }

        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31q_fstp_m80fp.case.json"
        ))
        .unwrap();
        for bytes in ["db39", "48db38", "9bdb38", "db38dfe1"] {
            let mut nearby = template.clone();
            let code = &mut nearby["memory"][0];
            code["initial_bytes_hex"] = serde_json::Value::String(bytes.into());
            code["length"] = serde_json::Value::from(bytes.len() / 2);
            code["comparison_mask_hex"] = serde_json::Value::String("ff".repeat(bytes.len() / 2));
            nearby["stop_rip"] = serde_json::Value::String(format!(
                "0x{:016x}",
                0x0010_0000 + (bytes.len() / 2) as u64
            ));
            assert!(
                v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap()).is_err(),
                "the S2-31q schema must reject {bytes}"
            );
        }
    }

    #[test]
    fn fld_m32fp_literal_fixtures_expand_binary32_values_and_reject_bad_class() {
        for (bytes, expected_rax, expected_raw, expected_tag_word, retired, expected_status) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000000000000",
                0x7ffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31s_fld_m32fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                "00000000000000000080",
                0x7ffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_infinity.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "0000000000000080ff7f",
                0xbffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31af_fld_m32fp_negative_infinity_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                "0000000000000080ffff",
                0xbffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_qnan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000000c0ff7f",
                0xbffc,
                1,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ah_fld_m32fp_negative_qnan_payload_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                "00000000004523c1ffff",
                0xbffc,
                2,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_snan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                "00000000000100c0ff7f",
                0xbffc,
                1,
                0x7d01,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31aj_fld_m32fp_negative_snan_payload_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d01,
                "00000000004523e1ffff",
                0xbffc,
                2,
                0x7d01,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ah fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31s fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(expected_status));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], expected_raw);
            for physical in 0..7 {
                assert_eq!(
                    after.registers[physical],
                    fixture
                        .fixture
                        .initial_state
                        .x87
                        .as_ref()
                        .unwrap()
                        .registers[physical]
                );
            }
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap()
                    .bytes_hex,
                fixture.fixture.memory[1].initial_bytes_hex
            );
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "canary")
                    .unwrap()
                    .bytes_hex,
                "a5"
            );
        }

        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json"
        ))
        .expect("S2-31ak binary32 invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state");
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let canary = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "canary")
            .unwrap();
        assert_eq!(canary.bytes_hex, "a5");

        let invalid_json: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m32fp_invalid_status.case.json"
        ))
        .unwrap();
        for source in ["0000803f", "0100807f", "0000c07f", "ffffbf7f"] {
            let mut source_class_irrelevant = invalid_json.clone();
            source_class_irrelevant["memory"][1]["initial_bytes_hex"] =
                serde_json::Value::String(source.into());
            v2::Fixture::from_json(&serde_json::to_vec(&source_class_irrelevant).unwrap())
                .unwrap_or_else(|error| panic!("environment gate should admit {source}: {error}"));
        }
        let mut clean_snan = invalid_json;
        clean_snan["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000004500".into());
        clean_snan["memory"][1]["initial_bytes_hex"] = serde_json::Value::String("0100807f".into());
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&clean_snan).unwrap()).is_err());
    }

    #[test]
    fn fld_m64fp_literal_fixtures_expand_binary64_and_reject_bad_class() {
        for (bytes, retired, raw, expected_tag_word, expected_status) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp.case.json")
                    .as_slice(),
                1,
                "00000000000000000000",
                0x7ffc,
                0x7d00,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                2,
                "00000000000000000000",
                0x7ffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_infinity.case.json")
                    .as_slice(),
                1,
                "0000000000000080ff7f",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ae_fld_m64fp_negative_infinity_fnstsw.case.json")
                    .as_slice(),
                2,
                "0000000000000080ffff",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_qnan.case.json")
                    .as_slice(),
                1,
                "00000000000000c0ff7f",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ag_fld_m64fp_negative_qnan_payload_fnstsw.case.json")
                    .as_slice(),
                2,
                "00e0d5c4b3a291c0ffff",
                0xbffc,
                0x7d00,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_snan.case.json")
                    .as_slice(),
                1,
                "00080000000000c0ff7f",
                0xbffc,
                0x7d01,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ai_fld_m64fp_negative_snan_payload_fnstsw.case.json")
                    .as_slice(),
                2,
                "00f8ffffffffffffffff",
                0xbffc,
                0x7d01,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31ae fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31u fixture must produce observable state")
            };
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(
                value.state.gpr[0],
                HexU64(if retired == 1 {
                    0x0030_0004
                } else {
                    0x0030_0000 | expected_status
                })
            );
            assert_eq!(after.status_word, HexU64(expected_status));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], raw);
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(stack.bytes_hex, fixture.fixture.memory[1].initial_bytes_hex);
            let canary = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "canary")
                .unwrap();
            assert_eq!(canary.bytes_hex, "a5");
        }
        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json"
        ))
        .expect("invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state")
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let mut clean_environment = invalid.fixture.clone();
        clean_environment
            .initial_state
            .x87
            .as_mut()
            .unwrap()
            .status_word = HexU64(0x4500);
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&clean_environment).unwrap()).is_err());
        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31t_fld_m64fp.case.json"
        ))
        .unwrap();
        for bytes in ["dd01", "dd08", "48dd00", "9bdd00", "67dd00", "dd00dfe1"] {
            let mut nearby = template.clone();
            nearby["memory"][0]["initial_bytes_hex"] = serde_json::Value::String(bytes.into());
            nearby["memory"][0]["length"] = serde_json::Value::from(bytes.len() / 2);
            nearby["memory"][0]["comparison_mask_hex"] =
                serde_json::Value::String("ff".repeat(bytes.len() / 2));
            nearby["stop_rip"] = serde_json::Value::String(format!(
                "0x{:016x}",
                0x0010_0000 + (bytes.len() / 2) as u64
            ));
            assert!(
                v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap()).is_err(),
                "S2-31t schema must reject {bytes}"
            );
        }
        let mut rejected_subnormal: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31ak_fld_m64fp_invalid_status.case.json"
        ))
        .unwrap();
        rejected_subnormal["initial_state"]["x87"]["status_word"] =
            serde_json::Value::String("0x0000000000004500".into());
        rejected_subnormal["memory"][1]["initial_bytes_hex"] =
            serde_json::Value::String("0100000000000000".into());
        assert!(v2::Fixture::from_json(&serde_json::to_vec(&rejected_subnormal).unwrap()).is_err());
    }

    #[test]
    fn fld_m80fp_literal_fixtures_load_raw_binary80_and_reject_bad_class() {
        for (bytes, expected_rax, retired, expected_raw, expected_tag_word) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "00000000000000000000",
                0x7ffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "00000000000000000080",
                0x7ffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31x_fld_m80fp_subnormal.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "01000000000000000000",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31x_fld_m80fp_largest_negative_subnormal_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffff7f0080",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31aa_fld_m80fp_infinity.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "0000000000000080ff7f",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31aa_fld_m80fp_negative_infinity_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "0000000000000080ffff",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31ab_fld_m80fp_qnan.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "35120000000000c0ff7f",
                0xbffc,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31ab_fld_m80fp_indefinite_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "00000000000000c0ffff",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ac_fld_m80fp_snan.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "0100000000000080ff7f",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ac_fld_m80fp_snan_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffffbfffff",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ad_fld_m80fp_pseudo_denormal.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
                "00000000000000800000",
                0xbffc,
            ),
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31ad_fld_m80fp_negative_max_pseudo_denormal_fnstsw.case.json")
                    .as_slice(),
                0x0000_0000_0030_7d00,
                2,
                "ffffffffffffffff0080",
                0xbffc,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("FLD m80fp fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
            panic!("FLD m80fp fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x7d00));
            assert_eq!(after.tag_word, HexU64(expected_tag_word));
            assert_eq!(
                (after.tag_word.get() >> 14) & 3,
                if expected_tag_word == 0xbffc { 0b10 } else { 0b01 }
            );
            assert_eq!(after.status_word.get() & 0x0012, 0, "no new DE/UE");
            assert_eq!(after.status_word.get() & 0x0001, 0, "no IE");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(after.registers[7], expected_raw);
            for physical in 0..7 {
                assert_eq!(
                    after.registers[physical],
                    fixture
                        .fixture
                        .initial_state
                        .x87
                        .as_ref()
                        .unwrap()
                        .registers[physical]
                );
            }
            let regions = memory.snapshot();
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "stack")
                    .unwrap()
                    .bytes_hex,
                expected_raw
            );
            assert_eq!(
                regions
                    .iter()
                    .find(|region| region.region_id == "canary")
                    .unwrap()
                    .bytes_hex,
                "a5"
            );
        }

        let invalid = v2::Fixture::from_json(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_invalid_class.case.json"
        ))
        .expect("invalid-status fixture schema");
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&invalid.fixture).unwrap();
        let result = execute_fixture_v2(&invalid, &mut memory);
        result.validate_for(&invalid).unwrap();
        assert!(matches!(result.outcome, v2::Outcome::Unsupported { .. }));
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) }
        );
        let Observation::Available { value } = result.snapshot else {
            panic!("invalid-status rejection must preserve an observable state");
        };
        assert_eq!(value.state, invalid.fixture.initial_state);
        let stack = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "stack")
            .unwrap();
        assert_eq!(stack.bytes_hex, invalid.fixture.memory[1].initial_bytes_hex);
        let canary = memory
            .snapshot()
            .into_iter()
            .find(|region| region.region_id == "canary")
            .unwrap();
        assert_eq!(canary.bytes_hex, "a5");

        let template: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/x86/stage2/s2_31r_fld_m80fp_invalid_class.case.json"
        ))
        .unwrap();
        for (raw, should_be_admitted) in [
            ("01000000000000000000", false),
            ("01000000000000000080", false),
            ("00000000000000000000", false),
            ("00000000000000000080", false),
            ("00000000000000800000", false),
            ("00000000000000800080", false),
            ("ffffffffffffff7f0000", false),
            ("0000000000000080003f", false),
            ("0000000000000080ff7f", false),
            ("0000000000000080ffff", false),
            ("00000000000000c0ff7f", false),
            ("00000000000000c0ffff", false),
            ("0100000000000080ffff", false),
            ("0100000000000080ff7f", false),
            ("0100000000000040ff7f", false),
            ("0100000000000040ffff", false),
            ("5555555555555595ff7f", false),
            ("5555555555555595ffff", false),
            ("ffffffffffffffbfff7f", false),
            ("ffffffffffffffbfffff", false),
            ("0100000000000000ff7f", false),
            ("0100000000000000ffff", false),
            ("0000000000000000ff7f", true),
            ("0000000000000000ffff", false),
        ] {
            let mut nearby = template.clone();
            nearby["memory"][1]["initial_bytes_hex"] = serde_json::Value::String(raw.into());
            let parsed = v2::Fixture::from_json(&serde_json::to_vec(&nearby).unwrap());
            assert_eq!(
                parsed.is_ok(),
                should_be_admitted,
                "S2-31ac schema admission for raw binary80 {raw}"
            );
        }
    }

    #[test]
    fn fst_m32fp_signed_zero_fixtures_preserve_or_pop_and_observe_fsw() {
        for (name, bytes, expected_rax, expected_status, expected_tag_word, raw_zero) in [
            (
                "positive FST zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4500,
                0xfffd,
                "00000000000000000000",
            ),
            (
                "negative FST zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fst_m32fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4500,
                0x4500,
                0xfffd,
                "00000000000000000080",
            ),
            (
                "positive FSTP zero",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_positive_zero.case.json").as_slice(),
                0x0030_0004,
                0x4d00,
                0xffff,
                "00000000000000000000",
            ),
            (
                "negative FSTP zero with FNSTSW",
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31al_fstp_m32fp_negative_zero_fnstsw.case.json").as_slice(),
                0x0030_4d00,
                0x4d00,
                0xffff,
                "00000000000000000080",
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes)
                .unwrap_or_else(|error| panic!("{name}: invalid fixture schema: {error}"));
            let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}), "{name}");
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(fixture.fixture.instruction_budget) },
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name}: expected observable CPU state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax), "{name}");
            assert_eq!(value.state.rip, fixture.fixture.stop_rip.unwrap(), "{name}");
            let x87 = value.state.x87.as_ref().unwrap();
            assert_eq!(x87.status_word, HexU64(expected_status), "{name}");
            assert_eq!(x87.tag_word, HexU64(expected_tag_word), "{name}");
            assert_eq!(x87.registers[0], raw_zero, "{name}");
            assert_eq!(x87.instruction_pointer, HexU64(0x0010_0000), "{name}");
            assert_eq!(x87.data_pointer, HexU64(0x0030_0004), "{name}");
            let stack = memory.snapshot().into_iter().find(|region| region.region_id == "stack").unwrap();
            let expected_memory = if raw_zero.ends_with("80") {
                "00000080"
            } else {
                "00000000"
            };
            assert_eq!(&stack.bytes_hex[8..16], expected_memory, "{name}");
        }
    }

    #[test]
    fn fst_m64fp_literal_fixtures_store_without_pop_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!("../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp.case.json")
                    .as_slice(),
                0x0000_0000_0030_0004,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31m_fst_m64fp_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_4500,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31m fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31m fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x4500));
            assert_eq!(after.tag_word, HexU64(0xfffc));
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0004));
            assert_eq!(
                after.registers,
                fixture
                    .fixture
                    .initial_state
                    .x87
                    .as_ref()
                    .unwrap()
                    .registers
            );
            let stack = memory
                .snapshot()
                .into_iter()
                .find(|region| region.region_id == "stack")
                .unwrap();
            assert_eq!(&stack.bytes_hex[8..24], "000000000000f83f");
        }
    }

    #[test]
    fn fild_m32_literal_fixtures_push_to_st7_and_compose_with_fnstsw() {
        for (bytes, expected_rax, retired) in [
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31n_fild_m32int_success.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_0001,
                1,
            ),
            (
                include_bytes!(
                    "../../../tests/fixtures/x86/stage2/s2_31n_fild_m32int_fnstsw.case.json"
                )
                .as_slice(),
                0x0000_0000_0030_7d00,
                2,
            ),
        ] {
            let fixture = v2::Fixture::from_json(bytes).expect("S2-31n fixture schema");
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(matches!(result.outcome, v2::Outcome::StopAddress {}));
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(retired)
                }
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("S2-31n fixture must produce an observable state");
            };
            assert_eq!(value.state.gpr[0], HexU64(expected_rax));
            let after = value.state.x87.as_ref().unwrap();
            assert_eq!(after.status_word, HexU64(0x7d00));
            assert_eq!(after.tag_word, HexU64(0x3ffc));
            assert_eq!(after.registers[7], "0000000000000080ff3f");
            assert_eq!(after.instruction_pointer, HexU64(0x0010_0000));
            assert_eq!(after.data_pointer, HexU64(0x0030_0001));
            let memory_after = memory.snapshot();
            let source_left = memory_after
                .iter()
                .find(|region| region.region_id == "data_left")
                .unwrap();
            assert_eq!(source_left.bytes_hex, "01");
        }
    }

    #[test]
    fn fistp_m16int_literal_fixtures_cover_rounding_edges_and_rejections() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let successful = [
            ("rc0_positive", 2_i16, true, true),
            ("rc0_negative", -2, true, true),
            ("rc1_positive", 1, true, false),
            ("rc1_negative", -2, true, true),
            ("rc2_positive", 2, true, true),
            ("rc2_negative", -1, true, false),
            ("rc3_positive", 1, true, false),
            ("rc3_negative", -1, true, false),
            ("fnstsw_tie", -2, true, false),
            ("exact_max", i16::MAX, false, false),
            ("exact_min", i16::MIN, false, false),
            ("rounded_max", 32767, true, false),
            ("rounded_min", i16::MIN, true, false),
        ];
        for (suffix, expected, inexact, incremented) in successful {
            let name = format!("s2_31d_fistp_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(if suffix == "fnstsw_tie" { 2 } else { 1 })
                },
                "{name}"
            );
            let left = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_left")
                .unwrap();
            let right = memory
                .snapshot()
                .into_iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!("{:02x}{}", bytes[1], "ccddeeff11223344556677889900"),
                "{name}"
            );
            assert_ne!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let before_x87 = before.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            let expected_fsw = (before_x87.status_word.get() & !(7 << 11 | 1 << 9))
                | (1 << 11)
                | if inexact { 0x20 } else { 0 }
                | if incremented { 0x200 } else { 0 };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word.get() & 3, 3, "{name}");
            assert_eq!(after.registers, before_x87.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let mut expected_state = before;
            expected_state.rip = HexU64(if suffix == "fnstsw_tie" {
                0x100004
            } else {
                0x100002
            });
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(expected_fsw);
            expected_x87.tag_word = HexU64((expected_x87.tag_word.get() & !3) | 3);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            if suffix == "fnstsw_tie" {
                expected_state.gpr[0] =
                    HexU64((expected_state.gpr[0].get() & !0xffff) | u64::from(expected_fsw));
            }
            assert_eq!(value.state, expected_state, "{name}: full CPU state");
        }

        for suffix in [
            "overflow",
            "invalid_tag",
            "invalid_status",
            "invalid_control",
            "invalid_exponent",
        ] {
            let name = format!("s2_31d_fistp_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
        }
    }

    #[test]
    fn fist_m16int_literal_fixtures_cover_no_pop_fnstsw_and_rejections() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        let successful = [
            ("rc0_positive", 2_i16, true, true),
            ("fnstsw_tie", -2, true, false),
            ("exact_max", i16::MAX, false, false),
            ("exact_min", i16::MIN, false, false),
        ];
        for (suffix, expected, inexact, incremented) in successful {
            let name = format!("s2_31e_fist_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            let instruction_count = if suffix == "fnstsw_tie" { 2 } else { 1 };
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(instruction_count)
                },
                "{name}"
            );
            let regions = memory.snapshot();
            let left = regions.iter().find(|r| r.region_id == "data_left").unwrap();
            let right = regions
                .iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!("{:02x}{}", bytes[1], "ccddeeff11223344556677889900"),
                "{name}"
            );
            assert_ne!(regions, initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let before_x87 = before.x87.as_ref().unwrap();
            let after = value.state.x87.as_ref().unwrap();
            let expected_fsw = (before_x87.status_word.get() & !(1 << 9))
                | if inexact { 0x20 } else { 0 }
                | if incremented { 0x200 } else { 0 };
            assert_eq!(after.status_word, HexU64(expected_fsw), "{name}");
            assert_eq!(after.tag_word, before_x87.tag_word, "{name}");
            assert_eq!(after.registers, before_x87.registers, "{name}");
            assert_eq!(after.instruction_pointer, HexU64(0x100000), "{name}");
            assert_eq!(after.data_pointer, HexU64(0x300001), "{name}");
            let mut expected_state = before;
            expected_state.rip = HexU64(0x100000 + 2 * instruction_count);
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(expected_fsw);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            if suffix == "fnstsw_tie" {
                expected_state.gpr[0] =
                    HexU64((expected_state.gpr[0].get() & !0xffff) | u64::from(expected_fsw));
            }
            assert_eq!(value.state, expected_state, "{name}: complete CPU state");
        }

        for suffix in [
            "overflow",
            "invalid_tag",
            "invalid_status",
            "invalid_control",
            "invalid_exponent",
        ] {
            let name = format!("s2_31e_fist_m16int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let initial_memory = memory.snapshot();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::Unsupported { .. }),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available { value: HexU64(0) },
                "{name}"
            );
            assert_eq!(memory.snapshot(), initial_memory, "{name}");
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
        }
    }

    #[test]
    fn fistp_m64int_literal_fixtures_cover_qword_boundaries_and_overflow() {
        let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/stage2");
        for (suffix, expected, count, rax_after) in [
            (
                "exact_max",
                0x7fff_ffff_ffff_ffff_u64,
                1_u64,
                0x0000_0000_0030_0001_u64,
            ),
            (
                "negative_min_fnstsw",
                0x8000_0000_0000_0000_u64,
                2_u64,
                0x0000_0000_0030_4d00_u64,
            ),
        ] {
            let name = format!("s2_31f_fistp_m64int_{suffix}.case.json");
            let bytes = std::fs::read(directory.join(&name)).unwrap();
            let fixture = v2::Fixture::from_json(&bytes)
                .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
            let before = fixture.fixture.initial_state.clone();
            let mut memory =
                od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
            let result = execute_fixture_v2(&fixture, &mut memory);
            result.validate_for(&fixture).unwrap();
            assert!(
                matches!(result.outcome, v2::Outcome::StopAddress {}),
                "{name}"
            );
            assert_eq!(
                result.instructions_executed,
                Observation::Available {
                    value: HexU64(count)
                },
                "{name}"
            );
            let regions = memory.snapshot();
            let left = regions.iter().find(|r| r.region_id == "data_left").unwrap();
            let right = regions
                .iter()
                .find(|r| r.region_id == "data_right")
                .unwrap();
            let bytes = expected.to_le_bytes();
            assert_eq!(left.bytes_hex, format!("{:02x}", bytes[0]), "{name}");
            assert_eq!(
                right.bytes_hex,
                format!(
                    "{}3344556677889900",
                    bytes[1..]
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>()
                ),
                "{name}"
            );
            let Observation::Available { value } = result.snapshot else {
                panic!("{name} snapshot unavailable");
            };
            let mut expected_state = before;
            expected_state.rip = HexU64(0x100000 + 2 * count);
            expected_state.gpr[0] = HexU64(rax_after);
            let expected_x87 = expected_state.x87.as_mut().unwrap();
            expected_x87.status_word = HexU64(0x4d00);
            expected_x87.tag_word = HexU64(0xffff);
            expected_x87.instruction_pointer = HexU64(0x100000);
            expected_x87.data_pointer = HexU64(0x300001);
            assert_eq!(value.state, expected_state, "{name}: full CPU state");
        }

        let name = "s2_31f_fistp_m64int_positive_overflow.case.json";
        let bytes = std::fs::read(directory.join(name)).unwrap();
        let fixture = v2::Fixture::from_json(&bytes)
            .unwrap_or_else(|error| panic!("{name} failed schema validation: {error}"));
        let mut memory = od_testkit::memory::FlatMemory::from_fixture(&fixture.fixture).unwrap();
        let initial_memory = memory.snapshot();
        let result = execute_fixture_v2(&fixture, &mut memory);
        result.validate_for(&fixture).unwrap();
        assert!(
            matches!(result.outcome, v2::Outcome::Unsupported { .. }),
            "{name}"
        );
        assert_eq!(
            result.instructions_executed,
            Observation::Available { value: HexU64(0) },
            "{name}"
        );
        assert_eq!(memory.snapshot(), initial_memory, "{name}");
        let Observation::Available { value } = result.snapshot else {
            panic!("{name} snapshot unavailable");
        };
        assert_eq!(value.state, fixture.fixture.initial_state, "{name}");
    }
}
