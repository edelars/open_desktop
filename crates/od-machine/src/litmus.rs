//! S6-19b: the public face of the litmus driver of S6-19 (`litmus_interleaving`): load a case from
//! the S6-05 fixture text, run it under one policy, get a plain report.
//!
//! The scheduler (`SmpScheduler`, `QuantumPolicy`) is not exposed: [`LitmusPolicy`] names the four
//! ways a caller can explore (every order-preserving schedule, a range of seeds, a fixed quantum,
//! one explicit schedule) and [`run_litmus`] turns them into runs of the one driver. Every run is
//! on one host thread, one vCPU per litmus thread created directly in long mode on a flat identity
//! RAM (SMP_CONTRACT 4.4, litmus only; this is not an AP start). The memory model under test is
//! "SC with every instruction indivisible" (`interleaving_instruction_atomic`), not x86-TSO and
//! not a parallel run.
//!
//! The result types are plain structs without `serde`; the JSON text is made by the caller. The
//! only JSON in this crate is the reading of the fixture files (`serde_json::Value`, no derive).
//!
//! A form the interpreter does not execute (before S6-20 the corpus `LOCK` forms) is a verdict
//! [`LitmusVerdict::Unsupported`] with the instruction and its bytes, never an `Equal`.

use crate::litmus_interleaving::{
    Case, EXHAUSTIVE_THRESHOLD, Exploration, Refusal, Seen, Witness, compare, random_schedule,
    try_explore_exhaustive, try_explore_seeds, try_run_case,
};
use crate::smp_scheduler::{Finished, QuantumPolicy};
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::path::Path;

/// The default threshold of complete schedules up to which an exhaustive run is accepted
/// (`docs/testing/SMP.md`).
pub const LITMUS_EXHAUSTIVE_THRESHOLD: u128 = EXHAUSTIVE_THRESHOLD;
/// The most seeds one seed run accepts.
pub const LITMUS_MAX_SEEDS: u64 = 1_000_000;

/// A litmus case loaded from fixture text; opaque, run it with [`run_litmus`].
#[derive(Clone, Debug)]
pub struct LitmusCase(Case);

impl LitmusCase {
    /// A finite fixture of the corpus (`sb.json`, `lock-xadd.json`, ...) from its JSON text.
    pub fn from_fixture_json(source: &str) -> Result<Self, LitmusError> {
        Case::parse_fixture(source)
            .map(Self)
            .map_err(LitmusError::Load)
    }

    /// One counter of `counters-formula.json` (`LOCK-INC`, `UNLOCKED-INC`, `THREE-INSN-COUNTER`)
    /// for `threads` threads and `rounds` rounds, from the JSON text of that file.
    pub fn from_formula_json(
        source: &str,
        id: &str,
        threads: u64,
        rounds: u64,
    ) -> Result<Self, LitmusError> {
        Case::parse_formula(source, id, threads, rounds)
            .map(Self)
            .map_err(LitmusError::Load)
    }

    /// The reading of a fixture file; a file that cannot be read is [`LitmusError::Load`].
    pub fn from_fixture_file(path: &Path) -> Result<Self, LitmusError> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| LitmusError::Load(format!("{}: {error}", path.display())))?;
        Self::from_fixture_json(&source)
            .map_err(|error| LitmusError::Load(format!("{}: {error}", path.display())))
    }

    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn vcpus(&self) -> usize {
        self.0.threads.len()
    }

    /// Scheduler steps of each thread. A fixture loaded through this loader gives every
    /// instruction exactly one step (`REP` included: it is not cut into slices here); only the
    /// tests' own `REP` case builder cuts a `REP` of a known count into slices of 1024 iterations.
    pub fn steps_per_thread(&self) -> Vec<usize> {
        self.0.steps_per_thread()
    }

    /// `(Σ n_i)! / Π n_i!`, `None` when it does not fit in 128 bits.
    pub fn schedule_count(&self) -> Option<u128> {
        self.0.schedule_count()
    }
}

/// How [`run_litmus`] explores a case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LitmusPolicy {
    /// Every order-preserving schedule of all steps. Refused when there are more than
    /// `max_schedules` of them. The observed set must EQUAL the expected set.
    Exhaustive { max_schedules: u128 },
    /// One run per seed: the seed gives a random order of steps (SplitMix64). The observed set
    /// must be a SUBSET of the expected set.
    Seeds(RangeInclusive<u64>),
    /// One run, every turn is this many steps (at least 1).
    Fixed(u64),
    /// One run of an explicit schedule (a vCPU per step); it must hold exactly the steps of each
    /// thread, see [`LitmusCase::steps_per_thread`].
    Schedule(Vec<usize>),
}

/// The first run that reached an outcome.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LitmusWitness {
    Schedule(Vec<usize>),
    Seed(u64),
    Quantum(u64),
}

/// One observed outcome: the values of the observed fields, in the order of
/// [`LitmusReport::observe`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitmusOutcome {
    pub values: Vec<u64>,
    /// Runs that ended in this outcome.
    pub runs: u64,
    pub first: LitmusWitness,
    /// The outcome is in the expected set of the fixture.
    pub expected: bool,
}

/// The instruction the interpreter does not execute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitmusUnsupported {
    pub cpu: usize,
    pub rip: u64,
    pub asm: String,
    pub bytes: Vec<u8>,
    /// The opcode byte the interpreter reported (the byte after `0F` for a two-byte opcode).
    pub opcode: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LitmusVerdict {
    /// The observed set equals the expected set.
    Equal,
    /// An observed outcome is not in the expected set: a defect, never adjusted.
    Forbidden,
    /// No forbidden outcome, but an expected outcome was not reached by this exploration. In an
    /// exhaustive run this is a defect (the sets must be equal); in a seed or single run it is
    /// only coverage.
    NotCovered,
    /// The interpreter does not execute a form of the case; nothing was explored.
    Unsupported,
    /// A thread did not end, or an instruction was cut: the run does not count.
    Incomplete,
}

impl LitmusVerdict {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Equal => "equal",
            Self::Forbidden => "forbidden",
            Self::NotCovered => "not_covered",
            Self::Unsupported => "unsupported",
            Self::Incomplete => "incomplete",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitmusReport {
    pub id: String,
    pub vcpus: usize,
    pub steps_per_thread: Vec<usize>,
    /// The `asm` of every instruction of every thread.
    pub threads: Vec<Vec<String>>,
    /// `cpuN.reg` / `mem.cell` names of the values of an outcome.
    pub observe: Vec<String>,
    pub complete_schedules: Option<u128>,
    pub runs: u64,
    /// Observed outcomes in the order of their values.
    pub histogram: Vec<LitmusOutcome>,
    /// The `interleaving_instruction_atomic` set of the fixture.
    pub expected: Vec<Vec<u64>>,
    pub forbidden: Vec<Vec<u64>>,
    pub not_covered: Vec<Vec<u64>>,
    /// The sequentially consistent set of the fixture (instructions may be cut), for the record.
    pub sc: Vec<Vec<u64>>,
    /// The outcome(s) the fixture marks as interesting (empty for formula cases).
    pub interesting: Vec<Vec<u64>>,
    /// Allowed by x86-TSO and not by this mode (for the record; empty for formula cases).
    pub x86_tso_only: Vec<Vec<u64>>,
    /// Calls of the interpreter that ended inside an instruction (must be 0).
    pub instructions_cut: u64,
    pub rep_slices: u64,
    pub incomplete_runs: u64,
    pub verdict: LitmusVerdict,
    pub unsupported: Option<LitmusUnsupported>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LitmusError {
    /// The fixture could not be read or is not a case the driver runs.
    Load(String),
    /// The exhaustive run would exceed `max_schedules` (or the count does not fit in 128 bits).
    TooManySchedules {
        count: Option<u128>,
        max_schedules: u128,
    },
    /// The policy is not usable with this case.
    Policy(String),
    /// The guest left the way a litmus cannot (a fault, an unreached stop address).
    Guest { cpu: usize, rip: u64, exit: String },
}

impl std::fmt::Display for LitmusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Load(detail) => write!(f, "load: {detail}"),
            Self::TooManySchedules {
                count,
                max_schedules,
            } => match count {
                Some(count) => write!(
                    f,
                    "{count} complete schedules, more than the limit {max_schedules}"
                ),
                None => write!(
                    f,
                    "the number of complete schedules does not fit in 128 bits, limit {max_schedules}"
                ),
            },
            Self::Policy(detail) => write!(f, "policy: {detail}"),
            Self::Guest { cpu, rip, exit } => {
                write!(f, "guest: cpu{cpu} at {rip:#x} ended with {exit}")
            }
        }
    }
}

impl std::error::Error for LitmusError {}

fn sets(set: &BTreeSet<Vec<u64>>) -> Vec<Vec<u64>> {
    set.iter().cloned().collect()
}

fn witness(first: &Witness, quantum: Option<u64>) -> LitmusWitness {
    match (first, quantum) {
        (_, Some(quantum)) => LitmusWitness::Quantum(quantum),
        (Witness::Schedule(list), None) => LitmusWitness::Schedule(list.clone()),
        (Witness::Seed(seed), None) => LitmusWitness::Seed(*seed),
    }
}

fn outcome(case: &Case, values: &[u64], seen: &Seen, quantum: Option<u64>) -> LitmusOutcome {
    LitmusOutcome {
        values: values.to_vec(),
        runs: seen.runs,
        first: witness(&seen.first, quantum),
        expected: case.expected.contains(values),
    }
}

fn refusal_error(refusal: Refusal) -> LitmusError {
    match refusal {
        Refusal::Fault { cpu, rip, exit } => LitmusError::Guest { cpu, rip, exit },
        // The probe run found no refused form: a later one is not expected, and is a guest error.
        Refusal::Unsupported(unsupported) => LitmusError::Guest {
            cpu: unsupported.cpu,
            rip: unsupported.rip,
            exit: format!("unsupported {}", unsupported.asm),
        },
    }
}

/// The bytes of the instruction at `rip` of thread `cpu`.
fn instruction_bytes(case: &Case, cpu: usize, rip: u64) -> Vec<u8> {
    let Some(thread) = case.threads.get(cpu) else {
        return Vec::new();
    };
    let mut at = thread.code_start;
    for instruction in &thread.instructions {
        if at == rip {
            return instruction.bytes.clone();
        }
        at += instruction.bytes.len() as u64;
    }
    Vec::new()
}

fn check_schedule(case: &Case, schedule: &[usize]) -> Result<(), LitmusError> {
    let lengths = case.steps_per_thread();
    let mut counts = vec![0usize; lengths.len()];
    for &cpu in schedule {
        match counts.get_mut(cpu) {
            Some(count) => *count += 1,
            None => {
                return Err(LitmusError::Policy(format!(
                    "the schedule names cpu{cpu}, the case has {} vCPUs",
                    lengths.len()
                )));
            }
        }
    }
    if counts != lengths {
        return Err(LitmusError::Policy(format!(
            "the schedule has {counts:?} steps per thread, the case needs {lengths:?}"
        )));
    }
    Ok(())
}

/// Runs `case` under `policy` and compares what it reached with the expected set of the fixture.
///
/// `Ok` with [`LitmusVerdict::Unsupported`] when the interpreter refuses a form of the case (the
/// refused instruction is in [`LitmusReport::unsupported`], no run is counted). `Err` for a
/// policy the case does not accept or a guest that leaves the way a litmus cannot.
pub fn run_litmus(case: &LitmusCase, policy: LitmusPolicy) -> Result<LitmusReport, LitmusError> {
    let case = &case.0;
    let count = case.schedule_count();
    let mut quantum = None;
    let mut forced: Option<Vec<usize>> = None;
    match &policy {
        LitmusPolicy::Exhaustive { max_schedules } => {
            if count.is_none_or(|count| count > *max_schedules) {
                return Err(LitmusError::TooManySchedules {
                    count,
                    max_schedules: *max_schedules,
                });
            }
        }
        LitmusPolicy::Seeds(seeds) => {
            // `end - start` cannot overflow once `start <= end`.
            if seeds.is_empty() || seeds.end() - seeds.start() >= LITMUS_MAX_SEEDS {
                return Err(LitmusError::Policy(format!(
                    "seeds must be a non-empty range of at most {LITMUS_MAX_SEEDS} seeds"
                )));
            }
        }
        LitmusPolicy::Fixed(steps) => {
            if *steps == 0 {
                return Err(LitmusError::Policy("the quantum must be at least 1".into()));
            }
            quantum = Some(*steps);
        }
        LitmusPolicy::Schedule(schedule) => {
            check_schedule(case, schedule)?;
            forced = Some(schedule.clone());
        }
    }

    let mut report = LitmusReport {
        id: case.id.clone(),
        vcpus: case.threads.len(),
        steps_per_thread: case.steps_per_thread(),
        threads: case
            .threads
            .iter()
            .map(|thread| thread.instructions.iter().map(|i| i.asm.clone()).collect())
            .collect(),
        observe: case.observe_names.clone(),
        complete_schedules: count,
        runs: 0,
        histogram: Vec::new(),
        expected: sets(&case.expected),
        forbidden: Vec::new(),
        not_covered: Vec::new(),
        sc: sets(&case.sc),
        interesting: sets(&case.interesting),
        x86_tso_only: sets(&case.x86_tso.difference(&case.expected).cloned().collect()),
        instructions_cut: 0,
        rep_slices: 0,
        incomplete_runs: 0,
        verdict: LitmusVerdict::Unsupported,
        unsupported: None,
    };

    // One round-robin probe of one step per turn reaches every instruction of every thread.
    match try_run_case(case, QuantumPolicy::Fixed(1)) {
        Ok(_) => {}
        Err(Refusal::Unsupported(unsupported)) => {
            report.unsupported = Some(LitmusUnsupported {
                cpu: unsupported.cpu,
                rip: unsupported.rip,
                bytes: instruction_bytes(case, unsupported.cpu, unsupported.rip),
                asm: unsupported.asm,
                opcode: unsupported.opcode,
            });
            return Ok(report);
        }
        Err(fault) => return Err(refusal_error(fault)),
    }

    let exploration = match (&policy, forced, quantum) {
        (LitmusPolicy::Exhaustive { .. }, _, _) => try_explore_exhaustive(case),
        (LitmusPolicy::Seeds(seeds), _, _) => try_explore_seeds(case, seeds.clone()),
        (_, Some(schedule), _) => single_run(
            case,
            QuantumPolicy::Schedule(schedule.clone()),
            Witness::Schedule(schedule),
            Finished::ScheduleEnd,
        ),
        (_, None, Some(steps)) => single_run(
            case,
            QuantumPolicy::Fixed(steps),
            Witness::Seed(0),
            Finished::AllIdle,
        ),
        (_, None, None) => unreachable!("a policy is one of the four"),
    }
    .map_err(refusal_error)?;

    let verdict = compare(case, &exploration);
    report.runs = exploration.runs;
    report.histogram = exploration
        .outcomes
        .iter()
        .map(|(values, seen)| outcome(case, values, seen, quantum))
        .collect();
    report.forbidden = sets(&verdict.forbidden);
    report.not_covered = sets(&verdict.not_covered);
    report.instructions_cut = exploration.cut_instructions;
    report.rep_slices = exploration.rep_slices;
    report.incomplete_runs = exploration.incomplete_runs;
    // An unfinished (or cut) run has no trustworthy outcome, so `incomplete` ranks above
    // `forbidden`: such runs are not counted as a forbidden result.
    report.verdict = if exploration.incomplete_runs > 0 || exploration.cut_instructions > 0 {
        LitmusVerdict::Incomplete
    } else if !verdict.forbidden.is_empty() {
        LitmusVerdict::Forbidden
    } else if !verdict.not_covered.is_empty() {
        LitmusVerdict::NotCovered
    } else {
        LitmusVerdict::Equal
    };
    Ok(report)
}

fn single_run(
    case: &Case,
    policy: QuantumPolicy,
    first: Witness,
    wanted: Finished,
) -> Result<Exploration, Refusal> {
    let mut exploration = Exploration::new();
    let run = try_run_case(case, policy)?;
    exploration.add(run, || first, wanted);
    Ok(exploration)
}

/// The random order of steps a seed stands for (the schedule the seed run uses), so that a caller
/// can replay a witness as an explicit [`LitmusPolicy::Schedule`].
pub fn litmus_seed_schedule(case: &LitmusCase, seed: u64) -> Vec<usize> {
    random_schedule(&case.0.steps_per_thread(), seed)
}
