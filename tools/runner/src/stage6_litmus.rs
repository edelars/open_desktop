//! `--stage6-litmus` (S6-19b): the S6-05 litmus corpus on our machine in the interleaving mode
//! (S6-19: one vCPU per litmus thread, created directly in long mode on a flat RAM, SMP_CONTRACT
//! 4.4, litmus only; "SC with every instruction indivisible", not x86-TSO, not a parallel run).
//!
//! ```text
//! od-runner --stage6-litmus (--fixture FILE | --corpus DIR |
//!                            --formula FILE --counter ID --threads N --rounds K)
//!           [--mode exhaustive|seed] [--seeds A..=B] [--budget SCHEDULES]
//!           [--quantum Q | --schedule CPU:STEPS,...]
//! ```
//!
//! `--fixture` is one finite fixture of `tests/fixtures/guest/stage6/litmus`, `--corpus` every
//! `*.json` of that directory but `counters-formula.json` (in file-name order), `--formula` with
//! `--counter` one counter of that file for N threads and K rounds. The default is
//! `--mode exhaustive` with at most `--budget` complete schedules (1,000,000); `--mode seed` runs
//! the seeds `--seeds` (default `0..=9999`) as random orders of steps; `--quantum Q` and
//! `--schedule` are one run each (one case only).
//!
//! `--parallel` (S6-36) runs the cases in the PARALLEL mode instead (one host thread per vCPU, the
//! real interpreter and RAM path; see `od_machine::run_litmus_parallel`):
//!
//! ```text
//! od-runner --stage6-litmus (--fixture FILE | --corpus DIR | --formula FILE --counter ID --threads N --rounds K)
//!           --parallel --vcpus 2|3|4 --iterations ROUNDS
//! ```
//!
//! A case that needs more concurrent threads than `--vcpus` is `not_applicable` (IRIW on 2 vCPUs),
//! never run on fewer. The observed set must be a subset of the x86-TSO set of the fixture; the
//! parallel mode is not deterministic, so the output carries the elapsed time and is not
//! byte-repeatable. Exit 5 on a `forbidden` outcome.
//!
//! One JSON object on stdout, the same arguments give the same bytes (not with `--parallel`). Exit codes: 0 every case is
//! `equal`, or `unsupported` (a form the interpreter does not run: the verdict says so, it is not
//! `equal`), or `not_covered` in a seed or single run; 3 invalid arguments (or an exhaustive run
//! above the budget); 4 a fixture could not be loaded; 5 a `forbidden` outcome, an `incomplete`
//! run, `not_covered` in an exhaustive run (the sets must be equal) or an error of the guest.

use od_machine::{
    BUILD_NONATOMIC_LOCK_TEST, BUILD_WEAK_HOST_ORDERING, LITMUS_EXHAUSTIVE_THRESHOLD, LitmusCase,
    LitmusError, LitmusParallelReport, LitmusParallelVerdict, LitmusPolicy, LitmusReport,
    LitmusVerdict, LitmusWitness, run_litmus, run_litmus_parallel,
};
use od_testkit::sha256_hex;
use serde_json::{Value, json};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const SCHEMA: &str = "od.stage6-litmus-run.v1";
const EXIT_INVALID_INPUT: u8 = 3;
const EXIT_LOADER: u8 = 4;
const EXIT_FAILED: u8 = 5;
const DEFAULT_SEEDS: RangeInclusive<u64> = 0..=9_999;
const SCHEDULE_MAX_STEPS: u64 = 10_000_000;
const FORMULA_FILE_NAME: &str = "counters-formula.json";

enum Source {
    Fixture(PathBuf),
    Corpus(PathBuf),
    Counter {
        formula: PathBuf,
        id: String,
        threads: u64,
        rounds: u64,
    },
}

#[derive(Clone)]
enum Mode {
    Exhaustive {
        budget: u128,
    },
    Seeds(RangeInclusive<u64>),
    Fixed(u64),
    Schedule(Vec<usize>),
    /// S6-36: the parallel mode, `vcpus` vCPUs, `iterations` rounds per case.
    Parallel {
        vcpus: usize,
        iterations: u64,
    },
}

struct Options {
    source: Source,
    mode: Mode,
}

pub fn run(args: &[String]) -> ExitCode {
    let (result, code) = match run_inner(args) {
        Ok(result) | Err(result) => result,
    };
    println!("{result}");
    ExitCode::from(code)
}

fn parse_number(value: &str) -> Result<u64, String> {
    // `from_str_radix` and `parse` accept a leading `+`; this grammar does not.
    let (digits, radix) = match value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        Some(hex) => (hex, 16),
        None => (value, 10),
    };
    if digits.starts_with('+') {
        return Err(format!("invalid number '{value}'"));
    }
    u64::from_str_radix(digits, radix).map_err(|_| format!("invalid number '{value}'"))
}

fn parse_seeds(value: &str) -> Result<RangeInclusive<u64>, String> {
    let (first, last) = value
        .split_once("..=")
        .ok_or_else(|| format!("--seeds expects A..=B, got '{value}'"))?;
    let (first, last) = (parse_number(first)?, parse_number(last)?);
    if first > last {
        return Err(format!("--seeds {value}: the range is empty"));
    }
    Ok(first..=last)
}

fn parse_schedule(value: &str) -> Result<Vec<usize>, String> {
    let mut list = Vec::new();
    for run in value.split(',') {
        let (cpu, steps) = run
            .split_once(':')
            .ok_or_else(|| format!("--schedule expects CPU:STEPS runs, got '{run}'"))?;
        let cpu = usize::try_from(parse_number(cpu)?)
            .map_err(|_| format!("vCPU out of range '{cpu}'"))?;
        let steps = parse_number(steps)?;
        // One entry per step (8 bytes each); the sum is checked without overflow.
        if steps == 0 || steps > SCHEDULE_MAX_STEPS - list.len() as u64 {
            return Err(format!(
                "--schedule run '{run}': steps 1.., at most {SCHEDULE_MAX_STEPS} in all"
            ));
        }
        list.extend(std::iter::repeat_n(cpu, steps as usize));
    }
    Ok(list)
}

fn once<T>(slot: &mut Option<T>, option: &str, value: T) -> Result<(), String> {
    if slot.replace(value).is_some() {
        Err(format!("{option} may be specified only once"))
    } else {
        Ok(())
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let parallel = args.iter().any(|argument| argument == "--parallel");
    let filtered: Vec<String>;
    let args = if parallel {
        filtered = args
            .iter()
            .filter(|argument| *argument != "--parallel")
            .cloned()
            .collect();
        &filtered[..]
    } else {
        args
    };
    let mut vcpus = None;
    let mut iterations = None;
    let mut fixture = None;
    let mut corpus = None;
    let mut formula = None;
    let mut counter = None;
    let mut threads = None;
    let mut rounds = None;
    let mut mode: Option<String> = None;
    let mut seeds = None;
    let mut budget = None;
    let mut quantum = None;
    let mut schedule = None;
    let mut index = 1;
    while index < args.len() {
        let option = args[index].as_str();
        let value = args
            .get(index + 1)
            .ok_or_else(|| format!("missing value for {option}"))?;
        match option {
            "--fixture" => once(&mut fixture, option, PathBuf::from(value))?,
            "--corpus" => once(&mut corpus, option, PathBuf::from(value))?,
            "--formula" => once(&mut formula, option, PathBuf::from(value))?,
            "--counter" => once(&mut counter, option, value.clone())?,
            "--threads" => once(&mut threads, option, parse_number(value)?)?,
            "--rounds" => once(&mut rounds, option, parse_number(value)?)?,
            "--mode" => {
                if !matches!(value.as_str(), "exhaustive" | "seed") {
                    return Err("--mode must be exhaustive or seed".into());
                }
                once(&mut mode, option, value.clone())?;
            }
            "--seeds" => once(&mut seeds, option, parse_seeds(value)?)?,
            "--budget" => {
                let parsed = parse_number(value)?;
                if parsed == 0 {
                    return Err("--budget must be at least 1".into());
                }
                once(&mut budget, option, u128::from(parsed))?;
            }
            "--quantum" => {
                let steps = parse_number(value)?;
                if steps == 0 {
                    return Err("--quantum must be at least 1".into());
                }
                once(&mut quantum, option, steps)?;
            }
            "--schedule" => once(&mut schedule, option, parse_schedule(value)?)?,
            "--vcpus" => once(&mut vcpus, option, parse_number(value)?)?,
            "--iterations" => once(&mut iterations, option, parse_number(value)?)?,
            other => return Err(format!("unexpected option '{other}'")),
        }
        index += 2;
    }
    let source = match (fixture, corpus, formula) {
        (Some(path), None, None) => Source::Fixture(path),
        (None, Some(path), None) => Source::Corpus(path),
        (None, None, Some(formula)) => Source::Counter {
            formula,
            id: counter
                .take()
                .ok_or("--formula needs --counter, --threads and --rounds")?,
            threads: threads
                .take()
                .ok_or("--formula needs --counter, --threads and --rounds")?,
            rounds: rounds
                .take()
                .ok_or("--formula needs --counter, --threads and --rounds")?,
        },
        (None, None, None) => {
            return Err("one of --fixture, --corpus, --formula is required".into());
        }
        _ => return Err("only one of --fixture, --corpus, --formula is accepted".into()),
    };
    if counter.is_some() || threads.is_some() || rounds.is_some() {
        return Err("--counter, --threads and --rounds go with --formula only".into());
    }
    if !parallel && (vcpus.is_some() || iterations.is_some()) {
        return Err("--vcpus and --iterations go with --parallel only".into());
    }
    if parallel {
        if mode.is_some()
            || seeds.is_some()
            || budget.is_some()
            || quantum.is_some()
            || schedule.is_some()
        {
            return Err(
                "--parallel takes --vcpus and --iterations only (no --mode, --seeds, --budget, --quantum, --schedule)"
                    .into(),
            );
        }
        let vcpus = vcpus.ok_or("--parallel needs --vcpus")?;
        let iterations = iterations.ok_or("--parallel needs --iterations")?;
        if iterations == 0 {
            return Err("--iterations must be at least 1".into());
        }
        let vcpus = usize::try_from(vcpus).map_err(|_| "--vcpus out of range".to_string())?;
        return Ok(Options {
            source,
            mode: Mode::Parallel { vcpus, iterations },
        });
    }
    let single = quantum.is_some() || schedule.is_some();
    if usize::from(quantum.is_some())
        + usize::from(schedule.is_some())
        + usize::from(mode.is_some())
        > 1
        || (single && (seeds.is_some() || budget.is_some()))
    {
        return Err("--quantum, --schedule and --mode are alternatives".into());
    }
    if single && matches!(source, Source::Corpus(_)) {
        return Err("--quantum and --schedule run one case: use --fixture or --formula".into());
    }
    let mode = if let Some(steps) = quantum {
        Mode::Fixed(steps)
    } else if let Some(list) = schedule {
        Mode::Schedule(list)
    } else if mode.as_deref() == Some("seed") {
        if budget.is_some() {
            return Err("--budget is the limit of --mode exhaustive".into());
        }
        Mode::Seeds(seeds.unwrap_or(DEFAULT_SEEDS))
    } else {
        if seeds.is_some() {
            return Err("--seeds goes with --mode seed".into());
        }
        Mode::Exhaustive {
            budget: budget.unwrap_or(LITMUS_EXHAUSTIVE_THRESHOLD),
        }
    };
    Ok(Options { source, mode })
}

fn failure(status: &str, kind: &str, detail: String) -> Value {
    json!({
        "schema": SCHEMA,
        "status": status,
        "error": {"kind": kind, "detail": detail},
    })
}

fn policy(mode: &Mode) -> LitmusPolicy {
    match mode {
        Mode::Exhaustive { budget } => LitmusPolicy::Exhaustive {
            max_schedules: *budget,
        },
        Mode::Seeds(seeds) => LitmusPolicy::Seeds(seeds.clone()),
        Mode::Fixed(steps) => LitmusPolicy::Fixed(*steps),
        Mode::Schedule(list) => LitmusPolicy::Schedule(list.clone()),
        Mode::Parallel { .. } => unreachable!("the parallel mode has no exploration policy"),
    }
}

fn mode_json(mode: &Mode) -> Value {
    match mode {
        Mode::Exhaustive { budget } => {
            json!({"mode": "exhaustive", "max_schedules": budget.to_string()})
        }
        Mode::Seeds(seeds) => json!({
            "mode": "seed",
            "seeds": [seeds.start(), seeds.end()],
            "seed_policy": "random order of steps: per step a thread drawn by SplitMix64(seed) among the threads with steps left, run as an explicit schedule",
        }),
        Mode::Fixed(steps) => json!({"mode": "fixed_quantum", "quantum": steps}),
        Mode::Schedule(list) => json!({"mode": "schedule", "steps": list.len()}),
        Mode::Parallel { vcpus, iterations } => {
            json!({"mode": "parallel", "vcpus": vcpus, "iterations_per_case": iterations})
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn witness_json(first: &LitmusWitness) -> Value {
    match first {
        LitmusWitness::Schedule(list) => {
            json!({"schedule": list.iter().map(ToString::to_string).collect::<String>()})
        }
        LitmusWitness::Seed(seed) => json!({"seed": seed}),
        LitmusWitness::Quantum(steps) => json!({"quantum": steps}),
    }
}

fn case_json(source: &str, fixture_sha256: &str, report: &LitmusReport) -> Value {
    let runs = report.runs.max(1) as f64;
    let histogram: Vec<Value> = report
        .histogram
        .iter()
        .map(|entry| {
            json!({
                "outcome": entry.values,
                "runs": entry.runs,
                "frequency": entry.runs as f64 / runs,
                "first": witness_json(&entry.first),
                "in_expected_set": entry.expected,
            })
        })
        .collect();
    let unsupported = report.unsupported.as_ref().map(|u| {
        json!({
            "instruction": u.asm,
            "bytes": hex(&u.bytes),
            "cpu": u.cpu,
            "rip": format!("{:#x}", u.rip),
            "opcode": format!("{:#04x}", u.opcode),
            "reason": format!(
                "the interpreter does not execute `{}` (bytes {}, opcode {:#04x}) on cpu{} at {:#x}: a gap of the interpreter, not a result of the litmus; nothing was explored",
                u.asm,
                hex(&u.bytes),
                u.opcode,
                u.cpu,
                u.rip
            ),
        })
    });
    json!({
        "id": report.id,
        "source": source,
        "fixture_sha256": fixture_sha256,
        "vcpus": report.vcpus,
        "threads": report.threads,
        "steps_per_thread": report.steps_per_thread,
        "complete_schedules": report.complete_schedules.map(|n| n.to_string()),
        "runs": report.runs,
        "observe": report.observe,
        "histogram": histogram,
        "expected_interleaving_instruction_atomic": report.expected,
        "forbidden_outcomes": report.forbidden,
        "not_covered_by_exploration": report.not_covered,
        "x86_tso_only_outcomes": report.x86_tso_only,
        "instructions_cut": report.instructions_cut,
        "rep_slices": report.rep_slices,
        "incomplete_runs": report.incomplete_runs,
        "verdict": report.verdict.name(),
        "unsupported": unsupported,
    })
}

fn load_error(detail: impl std::fmt::Display) -> (Value, u8) {
    (
        failure("loader_error", "loader", detail.to_string()),
        EXIT_LOADER,
    )
}

fn corpus_files(dir: &Path) -> Result<Vec<PathBuf>, (Value, u8)> {
    let entries = std::fs::read_dir(dir)
        .map_err(|error| load_error(format!("{}: {error}", dir.display())))?;
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .file_name()
                    .is_some_and(|name| name != FORMULA_FILE_NAME)
        })
        .collect();
    files.sort();
    if files.is_empty() {
        return Err(load_error(format!("{}: no fixture files", dir.display())));
    }
    Ok(files)
}

/// `(source name, SHA-256 of the source file, case)`.
type LoadedCase = (String, String, LitmusCase);

fn load_cases(source: &Source) -> Result<Vec<LoadedCase>, (Value, u8)> {
    let loaded = |name: String, path: &Path, result: Result<LitmusCase, LitmusError>| {
        let case = result.map_err(load_error)?;
        let bytes = std::fs::read(path)
            .map_err(|error| load_error(format!("{}: {error}", path.display())))?;
        Ok((name, sha256_hex(&bytes), case))
    };
    match source {
        Source::Fixture(path) => Ok(vec![loaded(
            path.display().to_string(),
            path,
            LitmusCase::from_fixture_file(path),
        )?]),
        Source::Corpus(dir) => corpus_files(dir)?
            .into_iter()
            .map(|path| {
                let name = path.file_name().map_or_else(
                    || path.display().to_string(),
                    |name| name.to_string_lossy().into_owned(),
                );
                loaded(name, &path, LitmusCase::from_fixture_file(&path))
            })
            .collect(),
        Source::Counter {
            formula,
            id,
            threads,
            rounds,
        } => {
            let text = std::fs::read_to_string(formula)
                .map_err(|error| load_error(format!("{}: {error}", formula.display())))?;
            Ok(vec![loaded(
                formula.display().to_string(),
                formula,
                LitmusCase::from_formula_json(&text, id, *threads, *rounds),
            )?])
        }
    }
}

fn run_inner(args: &[String]) -> Result<(Value, u8), (Value, u8)> {
    let invalid = |detail: String| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    };
    let options = parse_args(args).map_err(invalid)?;
    let cases = load_cases(&options.source)?;
    if let Mode::Parallel { vcpus, iterations } = options.mode {
        return run_parallel(&cases, vcpus, iterations);
    }
    let exhaustive = matches!(options.mode, Mode::Exhaustive { .. });
    let mut records = Vec::new();
    let mut counts = [0u64; 5];
    let mut failed = false;
    for (source, sha256, case) in &cases {
        let report = match run_litmus(case, policy(&options.mode)) {
            Ok(report) => report,
            Err(error @ (LitmusError::TooManySchedules { .. } | LitmusError::Policy(_))) => {
                return Err(invalid(format!("{}: {error}", case.id())));
            }
            Err(error @ LitmusError::Load(_)) => return Err(load_error(error)),
            Err(error @ LitmusError::Guest { .. }) => {
                return Err((
                    failure("guest_error", "guest", format!("{}: {error}", case.id())),
                    EXIT_FAILED,
                ));
            }
        };
        let slot = match report.verdict {
            LitmusVerdict::Equal => 0,
            LitmusVerdict::Forbidden => 1,
            LitmusVerdict::NotCovered => 2,
            LitmusVerdict::Unsupported => 3,
            LitmusVerdict::Incomplete => 4,
        };
        counts[slot] += 1;
        failed |= matches!(
            report.verdict,
            LitmusVerdict::Forbidden | LitmusVerdict::Incomplete
        ) || (exhaustive && report.verdict == LitmusVerdict::NotCovered);
        records.push(case_json(source, sha256, &report));
    }
    let result = json!({
        "schema": SCHEMA,
        "status": if failed { "failed" } else { "ok" },
        "harness": "one vCPU per litmus thread, created directly in long mode on a flat identity RAM (SMP_CONTRACT 4.4, litmus only): not an AP start",
        "memory_model": "SC with every instruction indivisible (interleaving_instruction_atomic); not x86-TSO, not a parallel run",
        "difference_from_x86": "INC [x] without LOCK is indivisible here (x86 can lose the update); SB both-zero is not reachable (it is x86-TSO only)",
        "exploration": mode_json(&options.mode),
        "summary": {
            "cases": records.len(),
            "equal": counts[0],
            "forbidden": counts[1],
            "not_covered": counts[2],
            "unsupported": counts[3],
            "incomplete": counts[4],
        },
        "cases": records,
    });
    if failed {
        Err((result, EXIT_FAILED))
    } else {
        Ok((result, 0))
    }
}

fn parallel_case_json(source: &str, fixture_sha256: &str, report: &LitmusParallelReport) -> Value {
    let rounds = report.rounds.max(1) as f64;
    let histogram: Vec<Value> = report
        .histogram
        .iter()
        .map(|entry| {
            json!({
                "outcome": entry.values,
                "rounds": entry.rounds,
                "frequency": entry.rounds as f64 / rounds,
                "in_x86_tso_set": entry.in_x86_tso,
                "first_round": entry.first_round,
                "first_placement_thread_to_vcpu": entry.first_placement,
            })
        })
        .collect();
    let seconds = report.elapsed_ns as f64 / 1e9;
    json!({
        "id": report.id,
        "source": source,
        "fixture_sha256": fixture_sha256,
        "status": "run",
        "concurrent_threads": report.threads,
        "vcpus": report.vcpus,
        "placements": report.placements,
        "rounds": report.rounds,
        "observe": report.observe,
        "histogram": histogram,
        "x86_tso_set": report.x86_tso,
        "sc_set": report.sc,
        "interesting_outcomes": report.interesting,
        "forbidden_outcomes": report.forbidden,
        "outside_sc": report.outside_sc,
        "x86_tso_outcomes_not_observed": report.not_observed,
        "late_barrier_rounds": report.late_barrier_rounds,
        "retired_per_vcpu": report.retired,
        "elapsed_s": seconds,
        "rounds_per_s": report.rounds as f64 / seconds.max(1e-9),
        "verdict": report.verdict.name(),
    })
}

/// S6-36: the parallel mode (see the module documentation).
fn run_parallel(
    cases: &[LoadedCase],
    vcpus: usize,
    iterations: u64,
) -> Result<(Value, u8), (Value, u8)> {
    let invalid = |detail: String| {
        (
            failure("invalid_input", "arguments", detail),
            EXIT_INVALID_INPUT,
        )
    };
    let mut records = Vec::new();
    let (mut allowed, mut forbidden, mut not_applicable) = (0u64, 0u64, 0u64);
    for (source, sha256, case) in cases {
        if case.concurrent_threads() > vcpus {
            not_applicable += 1;
            records.push(json!({
                "id": case.id(),
                "source": source,
                "fixture_sha256": sha256,
                "status": "not_applicable",
                "concurrent_threads": case.concurrent_threads(),
                "vcpus": vcpus,
                "reason": format!(
                    "the case needs {} concurrent threads, at least {} vCPUs: it is not reported for {} vCPUs",
                    case.concurrent_threads(),
                    case.concurrent_threads(),
                    vcpus
                ),
            }));
            continue;
        }
        let report = match run_litmus_parallel(case, vcpus, iterations) {
            Ok(report) => report,
            Err(error @ LitmusError::Policy(_)) => {
                return Err(invalid(format!("{}: {error}", case.id())));
            }
            Err(error @ LitmusError::Load(_)) => return Err(load_error(error)),
            Err(error) => {
                return Err((
                    failure("guest_error", "guest", format!("{}: {error}", case.id())),
                    EXIT_FAILED,
                ));
            }
        };
        match report.verdict {
            LitmusParallelVerdict::Allowed => allowed += 1,
            LitmusParallelVerdict::Forbidden => forbidden += 1,
        }
        records.push(parallel_case_json(source, sha256, &report));
    }
    let result = json!({
        "schema": "od.stage6-litmus-parallel-run.v1",
        "status": if forbidden > 0 { "failed" } else { "ok" },
        "harness": "one host thread per vCPU (S6-34), the real interpreter and RAM path, one identity 2 MiB table set, vCPUs created directly in long mode (SMP_CONTRACT 4.4, litmus only); a round is one quantum call per thread, the start barrier is crossed in the first poll of the stop test, thread-to-vCPU placement cycles through all injective maps",
        "reference": "x86-TSO set of the fixture (formula for the counters), read from the fixture file; the observed set must be a subset",
        "memory_order": "parallel mode: ordinary accesses SeqCst, LOCK as host atomics, host fences for guest fences",
        "build": {
            "weak_host_ordering": BUILD_WEAK_HOST_ORDERING,
            "nonatomic_lock_test": BUILD_NONATOMIC_LOCK_TEST,
        },
        "exploration": mode_json(&Mode::Parallel { vcpus, iterations }),
        "summary": {
            "cases": records.len(),
            "allowed_only": allowed,
            "forbidden": forbidden,
            "not_applicable": not_applicable,
        },
        "cases": records,
    });
    if forbidden > 0 {
        Err((result, EXIT_FAILED))
    } else {
        Ok((result, 0))
    }
}
