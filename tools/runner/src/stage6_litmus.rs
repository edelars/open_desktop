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
//! One JSON object on stdout, the same arguments give the same bytes. Exit codes: 0 every case is
//! `equal`, or `unsupported` (a form the interpreter does not run: the verdict says so, it is not
//! `equal`), or `not_covered` in a seed or single run; 3 invalid arguments (or an exhaustive run
//! above the budget); 4 a fixture could not be loaded; 5 a `forbidden` outcome, an `incomplete`
//! run, `not_covered` in an exhaustive run (the sets must be equal) or an error of the guest.

use od_machine::{
    LITMUS_EXHAUSTIVE_THRESHOLD, LitmusCase, LitmusError, LitmusPolicy, LitmusReport,
    LitmusVerdict, LitmusWitness, run_litmus,
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
    Exhaustive { budget: u128 },
    Seeds(RangeInclusive<u64>),
    Fixed(u64),
    Schedule(Vec<usize>),
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
