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

enum ExecutionFailure {
    InvalidInput(String),
    Unsupported(String),
    Harness(String),
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
        Backend::Interpreter => interpret(&code, &mut state, fixture.instruction_budget)
            .map_err(|error| ExecutionFailure::Unsupported(format!("interpreter: {error:?}")))?,
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
