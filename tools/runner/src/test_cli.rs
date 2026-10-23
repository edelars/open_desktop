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

    // The typed fixture has already been validated, so these lossless fields
    // can be copied without manufacturing a CPU state on conversion failure.
    let initial = &loaded.fixture.initial_state;
    let mut state = od_isa::LongModeState {
        gpr: initial.gpr.map(HexU64::get),
        rip: initial.rip.get(),
        rflags: initial.rflags.get(),
        fs_base: initial.fs_base.map_or(0, HexU64::get),
        gs_base: initial.gs_base.map_or(0, HexU64::get),
        fs_selector: initial
            .fs_selector
            .map_or(0, |selector| selector.get() as u16),
        gs_selector: initial
            .gs_selector
            .map_or(0, |selector| selector.get() as u16),
        gdtr: match (initial.gdtr_base, initial.gdtr_limit) {
            (Some(base), Some(limit)) => Some(od_isa::state::Gdtr {
                base: base.get(),
                limit: limit.get() as u16,
            }),
            _ => None,
        },
        fs_cache: match (
            initial.fs_cache_limit,
            initial.fs_cache_access,
            initial.fs_cache_flags,
        ) {
            (Some(limit), Some(access), Some(flags)) => Some(od_isa::FsDescriptorCache {
                limit: limit.get() as u32,
                access: access.get() as u8,
                flags: flags.get() as u8,
            }),
            _ => None,
        },
    };
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
}
