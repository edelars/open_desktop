use std::fs;
use std::hint::black_box;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use od_isa::{CpuState, ExecutionResult, interpret};

mod diagnostic;
mod firmware_boot;
mod stage4_alias_probe;
mod stage4_probe;
mod stage4_stack_probe;
mod stage4_store_load_probe;
mod stage4_uart_irq_probe;
mod test_cli;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Engine {
    Interpreter,
    Jit,
    Both,
}

struct Config {
    code: Vec<u8>,
    state: CpuState,
    budget: u64,
    engine: Engine,
    benchmark: bool,
    samples: usize,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--generate-corpus") => return test_cli::generate_corpus(&args[1..]),
        Some("--generate-native-xorpd-corpus") => {
            return test_cli::generate_native_xorpd_corpus(&args[1..]);
        }
        Some("--verify-native-xorpd-corpus") => {
            return test_cli::verify_native_xorpd_corpus(&args[1..]);
        }
        Some("--reduce-native-xorpd") => {
            return test_cli::reduce_native_xorpd(&args[1..]);
        }
        Some("--reduce-push-pop") => {
            return test_cli::reduce_push_pop(&args[1..]);
        }
        Some("--reduce-mov-memory") => {
            return test_cli::reduce_mov_memory(&args[1..]);
        }
        Some("--reduce-rep-fault") => {
            return test_cli::reduce_rep_fault(&args[1..]);
        }
        Some("--reduce-rep-stosb") => {
            return test_cli::reduce_rep_stosb(&args[1..]);
        }
        Some("--reduce-sqrtss-fault") => {
            return test_cli::reduce_sqrtss_fault(&args[1..]);
        }
        Some("--reduce-branch-nops") => {
            return test_cli::reduce_branch_nops(&args[1..]);
        }
        Some("--convert-fixture-v1") => return test_cli::convert_fixture_v1(&args[1..]),
        Some("--minimize") => return test_cli::minimize_fixture(&args[1..]),
        Some("--fixture") => return test_cli::run_fixture(&args[1..]),
        Some("--diagnostic") => return diagnostic::run(&args[1..]),
        Some("--diagnostic-replay") => return diagnostic::replay(&args[1..]),
        Some("--stage4-probe") | Some("--stage4-uart-echo-probe") => {
            return stage4_probe::run(&args);
        }
        Some("--stage4-stack-probe") => return stage4_stack_probe::run(&args),
        Some("--stage4-alias-probe") => return stage4_alias_probe::run(&args),
        Some("--stage4-store-load-probe") => return stage4_store_load_probe::run(&args),
        Some("--stage4-uart-irq-probe") => return stage4_uart_irq_probe::run(&args),
        Some("--compare-results") => return test_cli::compare_results(&args[1..]),
        Some("boot-firmware") => return firmware_boot::run(&args[1..]),
        _ => {}
    }
    if args.windows(2).any(|arguments| {
        arguments[0] == "--engine" && matches!(arguments[1].as_str(), "jit-block" | "jit-dispatch")
    }) {
        eprintln!("invalid input: --engine jit-block and jit-dispatch require --fixture v1 input");
        return ExitCode::from(3);
    }
    match parse_args(args.into_iter()) {
        Ok(None) => ExitCode::SUCCESS,
        Ok(Some(config)) => match run(config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("error: {error}\n\n{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Option<Config>, String> {
    let mut args = args.into_iter();
    let mut code = None;
    let mut state = CpuState::default();
    let mut budget = 100_000;
    let mut engine = Engine::Both;
    let mut benchmark = false;
    let mut samples = 20;
    let mut samples_set = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                return Ok(None);
            }
            "--hex" => set_code(&mut code, parse_hex(&next_value(&mut args, &arg)?)?)?,
            "--hex-file" => {
                let path = next_value(&mut args, &arg)?;
                let input = fs::read_to_string(&path)
                    .map_err(|error| format!("could not read hex fixture '{path}': {error}"))?;
                set_code(&mut code, parse_hex(&input)?)?;
            }
            "--rax" => state.gpr[0] = parse_integer(&next_value(&mut args, &arg)?)?,
            "--rcx" => state.gpr[1] = parse_integer(&next_value(&mut args, &arg)?)?,
            "--rdx" => state.gpr[2] = parse_integer(&next_value(&mut args, &arg)?)?,
            "--rbx" => state.gpr[3] = parse_integer(&next_value(&mut args, &arg)?)?,
            "--rip" => {
                state.rip = usize::try_from(parse_integer(&next_value(&mut args, &arg)?)?)
                    .map_err(|_| "RIP is larger than host usize".to_string())?
            }
            "--budget" => budget = parse_integer(&next_value(&mut args, &arg)?)?,
            "--benchmark" => benchmark = true,
            "--samples" => {
                samples = usize::try_from(parse_integer(&next_value(&mut args, &arg)?)?)
                    .map_err(|_| "sample count is larger than host usize".to_string())?;
                if !(20..=1000).contains(&samples) {
                    return Err("--samples must be between 20 and 1000".to_string());
                }
                samples_set = true;
            }
            "--engine" => {
                engine = match next_value(&mut args, &arg)?.as_str() {
                    "interpreter" => Engine::Interpreter,
                    "jit" => Engine::Jit,
                    "both" => Engine::Both,
                    other => return Err(format!("unknown engine '{other}'")),
                }
            }
            other => return Err(format!("unknown option '{other}'")),
        }
    }
    if samples_set && !benchmark {
        return Err("--samples requires --benchmark".to_string());
    }
    if benchmark && engine != Engine::Both {
        return Err("--benchmark compares both interpreter and JIT; omit --engine".to_string());
    }
    let code = code.ok_or_else(|| "either --hex or --hex-file is required".to_string())?;
    Ok(Some(Config {
        code,
        state,
        budget,
        engine,
        benchmark,
        samples,
    }))
}

fn set_code(code: &mut Option<Vec<u8>>, input: Vec<u8>) -> Result<(), String> {
    if code.is_some() {
        return Err("specify only one of --hex and --hex-file".to_string());
    }
    *code = Some(input);
    Ok(())
}

fn next_value(args: &mut impl Iterator<Item = String>, option: &str) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("missing value for {option}"))
}

fn parse_hex(input: &str) -> Result<Vec<u8>, String> {
    let compact: String = input
        .chars()
        .filter(|ch| !ch.is_ascii_whitespace() && *ch != '_')
        .collect();
    let digits = compact.strip_prefix("0x").unwrap_or(&compact);
    if digits.len() % 2 != 0 {
        return Err("hex byte string must contain an even number of digits".to_string());
    }
    (0..digits.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&digits[index..index + 2], 16)
                .map_err(|_| "invalid hex byte string".to_string())
        })
        .collect()
}

fn parse_integer(input: &str) -> Result<u64, String> {
    if let Some(value) = input.strip_prefix("0x") {
        u64::from_str_radix(value, 16).map_err(|_| format!("invalid integer '{input}'"))
    } else {
        input
            .parse()
            .map_err(|_| format!("invalid integer '{input}'"))
    }
}

fn run(config: Config) -> Result<(), String> {
    if config.benchmark {
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        return Err("JIT benchmark is unsupported on this host".to_string());
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        return run_benchmark(&config);
    }
    match config.engine {
        Engine::Interpreter => {
            let mut state = config.state;
            let result = interpret(&config.code, &mut state, config.budget)
                .map_err(|error| format!("interpreter failed: {error:?}"))?;
            print_result("interpreter", &result);
        }
        Engine::Jit => {
            let mut state = config.state;
            let result = execute_legacy_jit(&config.code, &mut state, config.budget)?;
            print_result("jit", &result);
        }
        Engine::Both => {
            let mut interpreter_state = config.state.clone();
            let interpreter = interpret(&config.code, &mut interpreter_state, config.budget)
                .map_err(|error| format!("interpreter failed: {error:?}"))?;
            let mut jit_state = config.state;
            let jit = execute_legacy_jit(&config.code, &mut jit_state, config.budget)?;
            print_result("interpreter", &interpreter);
            print_result("jit", &jit);
            if interpreter != jit {
                return Err("interpreter/JIT result mismatch".to_string());
            }
            println!("comparison: match");
        }
    }
    Ok(())
}

fn execute_legacy_jit(
    code: &[u8],
    state: &mut CpuState,
    budget: u64,
) -> Result<ExecutionResult, String> {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        od_host::execute_x86_fragment(code, state, budget)
            .map_err(|error| format!("JIT failed: {error:?}"))
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    {
        let _ = (code, state, budget);
        Err("ARM64 JIT execution requires an aarch64 macOS host".to_string())
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn run_benchmark(config: &Config) -> Result<(), String> {
    const TARGET_BATCH: Duration = Duration::from_millis(200);

    let mut interpreter_state = config.state.clone();
    let interpreter_reference = interpret(&config.code, &mut interpreter_state, config.budget)
        .map_err(|error| format!("interpreter failed: {error:?}"))?;
    let compiled = od_host::CompiledFragment::compile(&config.code)
        .map_err(|error| format!("JIT compile/publish failed: {error:?}"))?;
    let mut jit_state = config.state.clone();
    let jit_reference = compiled
        .execute(&mut jit_state, config.budget)
        .map_err(|error| format!("JIT execution failed: {error:?}"))?;
    if interpreter_reference != jit_reference {
        return Err("interpreter/JIT result mismatch before benchmark".to_string());
    }

    let decode_operations = calibrate_operations("decode", TARGET_BATCH, |count| {
        time_decode_batch(&config.code, count).map(|value| value.0)
    })?;
    let translation_operations = calibrate_operations("translation", TARGET_BATCH, |count| {
        time_translation_batch(&config.code, count).map(|value| value.0)
    })?;
    let compile_publish_operations =
        calibrate_operations("compile/publish/release", TARGET_BATCH, |count| {
            time_compile_publish_batch(&config.code, count).map(|value| value.0)
        })?;
    let interpreter_iterations =
        calibrate_operations("interpreter batch", TARGET_BATCH, |count| {
            time_interpreter_batch(config, count).map(|value| value.0)
        })?;
    let jit_iterations = calibrate_operations("reused JIT batch", TARGET_BATCH, |count| {
        time_jit_batch(config, &compiled, count).map(|value| value.0)
    })?;

    let mut decode_ns_per_op = Vec::with_capacity(config.samples);
    let mut translation_ns_per_op = Vec::with_capacity(config.samples);
    let mut compile_publish_ns_per_op = Vec::with_capacity(config.samples);
    let mut decode_batch_ns = Vec::with_capacity(config.samples);
    let mut translation_batch_ns = Vec::with_capacity(config.samples);
    let mut compile_publish_batch_ns = Vec::with_capacity(config.samples);
    let mut cold_orders = Vec::with_capacity(config.samples);
    for sample in 0..config.samples {
        let (decode, translation, compile_publish, order) = match sample % 3 {
            0 => {
                let decode = time_decode_batch(&config.code, decode_operations)?;
                let translation = time_translation_batch(&config.code, translation_operations)?;
                let compile_publish =
                    time_compile_publish_batch(&config.code, compile_publish_operations)?;
                (
                    decode,
                    translation,
                    compile_publish,
                    "decode_translation_publish",
                )
            }
            1 => {
                let translation = time_translation_batch(&config.code, translation_operations)?;
                let compile_publish =
                    time_compile_publish_batch(&config.code, compile_publish_operations)?;
                let decode = time_decode_batch(&config.code, decode_operations)?;
                (
                    decode,
                    translation,
                    compile_publish,
                    "translation_publish_decode",
                )
            }
            _ => {
                let compile_publish =
                    time_compile_publish_batch(&config.code, compile_publish_operations)?;
                let decode = time_decode_batch(&config.code, decode_operations)?;
                let translation = time_translation_batch(&config.code, translation_operations)?;
                (
                    decode,
                    translation,
                    compile_publish,
                    "publish_decode_translation",
                )
            }
        };
        decode_batch_ns.push(decode.0.as_nanos());
        decode_ns_per_op.push(decode.0.as_nanos() / u128::from(decode_operations));
        translation_batch_ns.push(translation.0.as_nanos());
        translation_ns_per_op.push(translation.0.as_nanos() / u128::from(translation_operations));
        compile_publish_batch_ns.push(compile_publish.0.as_nanos());
        compile_publish_ns_per_op
            .push(compile_publish.0.as_nanos() / u128::from(compile_publish_operations));
        cold_orders.push(order);
    }

    let mut interpreter_ns_per_op = Vec::with_capacity(config.samples);
    let mut jit_ns_per_op = Vec::with_capacity(config.samples);
    let mut interpreter_batch_ns = Vec::with_capacity(config.samples);
    let mut jit_batch_ns = Vec::with_capacity(config.samples);
    let mut orders = Vec::with_capacity(config.samples);
    for sample in 0..config.samples {
        let (interpreter, jit, order) = if sample % 2 == 0 {
            let interpreter = time_interpreter_batch(config, interpreter_iterations)?;
            let jit = time_jit_batch(config, &compiled, jit_iterations)?;
            (interpreter, jit, "interpreter_then_jit")
        } else {
            let jit = time_jit_batch(config, &compiled, jit_iterations)?;
            let interpreter = time_interpreter_batch(config, interpreter_iterations)?;
            (interpreter, jit, "jit_then_interpreter")
        };
        if interpreter.1 != interpreter_reference || jit.1 != jit_reference {
            return Err(format!(
                "result mismatch during benchmark sample {}",
                sample + 1
            ));
        }
        interpreter_batch_ns.push(interpreter.0.as_nanos());
        jit_batch_ns.push(jit.0.as_nanos());
        interpreter_ns_per_op.push(interpreter.0.as_nanos() / u128::from(interpreter_iterations));
        jit_ns_per_op.push(jit.0.as_nanos() / u128::from(jit_iterations));
        orders.push(order);
    }

    println!("format=od-runner-benchmark-v0");
    println!("host_os={}", std::env::consts::OS);
    println!("host_arch={}", std::env::consts::ARCH);
    println!(
        "build_profile={}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        }
    );
    println!(
        "code_hex={}",
        config
            .code
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    println!("initial_rax={}", config.state.gpr[0]);
    println!("initial_rcx={}", config.state.gpr[1]);
    println!("initial_rdx={}", config.state.gpr[2]);
    println!("initial_rbx={}", config.state.gpr[3]);
    println!("initial_rip={}", config.state.rip);
    println!("instruction_budget={}", config.budget);
    println!("correctness=interpreter_and_jit_match");
    println!("minimum_batch_ms={}", TARGET_BATCH.as_millis());
    println!("decode_operations_per_sample={decode_operations}");
    println!("translation_operations_per_sample={translation_operations}");
    println!("compile_publish_operations_per_sample={compile_publish_operations}");
    println!("interpreter_iterations_per_sample={interpreter_iterations}");
    println!("jit_iterations_per_sample={jit_iterations}");
    println!("samples={}", config.samples);
    println!(
        "sample,cold_order,decode_batch_ns,decode_ns_per_op,translation_batch_ns,translation_ns_per_op,compile_publish_batch_ns,compile_publish_ns_per_op,warm_order,interpreter_iterations,interpreter_batch_ns,interpreter_ns_per_op,jit_iterations,jit_reuse_batch_ns,jit_ns_per_op"
    );
    for sample in 0..config.samples {
        println!(
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            sample + 1,
            cold_orders[sample],
            decode_batch_ns[sample],
            decode_ns_per_op[sample],
            translation_batch_ns[sample],
            translation_ns_per_op[sample],
            compile_publish_batch_ns[sample],
            compile_publish_ns_per_op[sample],
            orders[sample],
            interpreter_iterations,
            interpreter_batch_ns[sample],
            interpreter_ns_per_op[sample],
            jit_iterations,
            jit_batch_ns[sample],
            jit_ns_per_op[sample],
        );
    }
    print_summary("decode_ns_per_op", &decode_ns_per_op);
    print_summary("translation_ns_per_op", &translation_ns_per_op);
    print_summary("compile_publish_ns_per_op", &compile_publish_ns_per_op);
    print_summary("interpreter_ns_per_op", &interpreter_ns_per_op);
    print_summary("jit_ns_per_op", &jit_ns_per_op);
    println!(
        "instructions_per_invocation={}",
        interpreter_reference.instructions_executed
    );
    Ok(())
}

fn calibrate_operations(
    label: &str,
    target: Duration,
    mut measure: impl FnMut(u64) -> Result<Duration, String>,
) -> Result<u64, String> {
    const MAX_OPERATIONS: u64 = 134_217_728;
    let mut operations = 1_u64;
    loop {
        if measure(operations)? >= target {
            return Ok(operations);
        }
        if operations >= MAX_OPERATIONS {
            return Err(format!(
                "{label} calibration did not reach {} ms within {MAX_OPERATIONS} operations",
                target.as_millis()
            ));
        }
        operations = operations.saturating_mul(2).min(MAX_OPERATIONS);
    }
}

fn time_decode_batch(code: &[u8], operations: u64) -> Result<(Duration, ()), String> {
    let start = Instant::now();
    for _ in 0..operations {
        let instructions =
            od_isa::decode(code).map_err(|error| format!("x86 decode failed: {error:?}"))?;
        black_box(instructions);
    }
    Ok((start.elapsed(), ()))
}

fn time_translation_batch(code: &[u8], operations: u64) -> Result<(Duration, ()), String> {
    let start = Instant::now();
    for _ in 0..operations {
        let output =
            od_jit::compile(code).map_err(|error| format!("JIT translation failed: {error:?}"))?;
        black_box(output);
    }
    Ok((start.elapsed(), ()))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn time_compile_publish_batch(code: &[u8], operations: u64) -> Result<(Duration, ()), String> {
    let start = Instant::now();
    for _ in 0..operations {
        let compiled = od_host::CompiledFragment::compile(code)
            .map_err(|error| format!("JIT compile/publish failed: {error:?}"))?;
        black_box(&compiled);
        drop(compiled);
    }
    Ok((start.elapsed(), ()))
}

fn time_interpreter_batch(
    config: &Config,
    iterations: u64,
) -> Result<(Duration, ExecutionResult), String> {
    let start = Instant::now();
    let mut last = None;
    for _ in 0..iterations {
        let mut state = config.state.clone();
        let result = interpret(&config.code, &mut state, config.budget)
            .map_err(|error| format!("interpreter failed: {error:?}"))?;
        black_box(&result);
        last = Some(result);
    }
    let elapsed = start.elapsed();
    let result = last.ok_or_else(|| "benchmark batch must run at least once".to_string())?;
    Ok((elapsed, result))
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
fn time_jit_batch(
    config: &Config,
    compiled: &od_host::CompiledFragment,
    iterations: u64,
) -> Result<(Duration, ExecutionResult), String> {
    let start = Instant::now();
    let mut last = None;
    for _ in 0..iterations {
        let mut state = config.state.clone();
        let result = compiled
            .execute(&mut state, config.budget)
            .map_err(|error| format!("JIT execution failed: {error:?}"))?;
        black_box(&result);
        last = Some(result);
    }
    let elapsed = start.elapsed();
    let result = last.ok_or_else(|| "benchmark batch must run at least once".to_string())?;
    Ok((elapsed, result))
}

fn print_summary(name: &str, values: &[u128]) {
    let mut ordered = values.to_vec();
    ordered.sort_unstable();
    let median = if ordered.len() % 2 == 0 {
        (ordered[ordered.len() / 2 - 1] + ordered[ordered.len() / 2]) / 2
    } else {
        ordered[ordered.len() / 2]
    };
    println!(
        "summary_{name}=median:{median},min:{},max:{}",
        ordered[0],
        ordered[ordered.len() - 1]
    );
}

fn print_result(engine: &str, result: &ExecutionResult) {
    println!(
        "{engine}: reason={:?} instructions={} rip=0x{:x} rax=0x{:016x} rcx=0x{:016x} rdx=0x{:016x} rbx=0x{:016x} rflags=0x{:016x}",
        result.reason,
        result.instructions_executed,
        result.state.rip,
        result.state.gpr[0],
        result.state.gpr[1],
        result.state.gpr[2],
        result.state.gpr[3],
        result.state.rflags,
    );
}

fn usage() -> &'static str {
    "od-runner (--hex HEX | --hex-file PATH) [--engine both|interpreter|jit] [--rax N] [--rcx N] [--rdx N] [--rbx N] [--rip N] [--budget N]\nNumbers may be decimal or 0x-prefixed hexadecimal. The default engine compares interpreter and JIT."
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn parses_hex_with_separators_and_prefixed_numbers() {
        let config = parse_args(["--hex", "0x48_b8_01 00", "--rax", "0xff"].map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(config.code, [0x48, 0xB8, 1, 0]);
        assert_eq!(config.state.gpr[0], 255);
    }

    #[test]
    fn rejects_odd_length_hex() {
        assert!(parse_hex("abc").is_err());
    }

    #[test]
    fn reads_a_repository_x86_fixture() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/s0_09_mov_imm64.hex");
        let config = parse_args(["--hex-file", path.to_str().unwrap()].map(str::to_string))
            .unwrap()
            .unwrap();
        assert_eq!(
            config.code,
            [0x48, 0xB8, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]
        );
    }

    #[test]
    fn rejects_multiple_code_sources() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/x86/s0_09_mov_imm64.hex");
        let result =
            parse_args(["--hex", "90", "--hex-file", path.to_str().unwrap()].map(str::to_string));
        assert!(result.is_err());
    }

    #[test]
    fn benchmark_mode_defaults_to_twenty_pairs_and_requires_both_engines() {
        let config = parse_args(["--hex", "48 2b c1 75 fb", "--benchmark"].map(str::to_string))
            .unwrap()
            .unwrap();
        assert!(config.benchmark);
        assert_eq!(config.samples, 20);
        assert_eq!(config.engine, Engine::Both);

        let result =
            parse_args(["--hex", "90", "--benchmark", "--samples", "2"].map(str::to_string));
        assert!(result.is_err());

        let result =
            parse_args(["--hex", "90", "--benchmark", "--engine", "jit"].map(str::to_string));
        assert!(result.is_err());
    }
}
