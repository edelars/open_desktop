//! S6-41e: the S6-19 litmus run in the interleaved mode (one host thread, one vCPU per litmus
//! thread, every instruction one scheduler step) on the machine's own bus, with a
//! [`LitmusStepHook`] that takes instructions before the interpreter (the JIT of `od-host`).
//!
//! The S6-19 driver (`litmus_interleaving`) runs on a flat RAM without a bus and cannot host the
//! JIT, whose blocks work on the bus's physical memory through the guest's page tables. This module
//! is the same exploration (every order-preserving schedule while there are at most
//! `max_exhaustive` of them, otherwise the random schedules of a seed range) on the machine of the
//! parallel litmus (`litmus_parallel::build_board`: one identity table set, every vCPU in long
//! mode, the Local APIC windows, the clock owner) in [`crate::ExecutionMode::Interleaved`]: the
//! scheduler of S6-15 ([`SmpScheduler`], [`QuantumPolicy::Schedule`]) over [`BoardSmpHost`], the
//! board's CPL0-set interpreter for what the hook declines.
//!
//! The expected set is the `interleaving_instruction_atomic` set of the fixture (read from the
//! fixture file, never from a run), exactly as in S6-19. The observed set must be a subset of it
//! (and, in an exhaustive run, equal to it); an outcome outside it is forbidden. The x86-TSO set is
//! reported too: an interleaved run cannot leave it either.

use crate::litmus::{LitmusCase, LitmusError};
use crate::litmus_interleaving::{Case, Field, Outcome, for_each_schedule, random_schedule};
use crate::litmus_parallel::{
    EFER, LitmusStep, LitmusStepHook, LitmusStepOutcome, TABLES, build_board, long_state,
};
use crate::smp_scheduler::{
    BoardSmpHost, Executed, Finished, QuantumPolicy, SmpScheduler, StopRequest,
};
use crate::{FirmwareBoardError, GuestPhysicalAddress, LoadedFirmwareBoard};
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1};
use od_isa::{CliProfile, CpuActivity, HaltProfile, StiProfile};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::RangeInclusive;
use std::time::Instant;

/// The most schedules a seed run accepts (as `LITMUS_MAX_SEEDS`).
const MAX_RUNS: u64 = 1_000_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LitmusInterleavedOutcome {
    pub values: Vec<u64>,
    /// Runs (schedules) that ended in this outcome.
    pub runs: u64,
    /// In the `interleaving_instruction_atomic` set of the fixture.
    pub expected: bool,
    /// In the x86-TSO set (fixture) or formula of the case.
    pub in_x86_tso: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LitmusInterleavedVerdict {
    /// The observed set equals the expected set.
    Equal,
    /// An observed outcome is not in the expected set: a defect.
    Forbidden,
    /// No forbidden outcome, but an expected outcome was not reached (a defect in an exhaustive
    /// run, not in a seed run).
    NotCovered,
    /// A run did not end on its schedule: its outcome is not trustworthy, so this ranks above the
    /// other verdicts (as in S6-19).
    Incomplete,
}

impl LitmusInterleavedVerdict {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Incomplete => "incomplete",
            Self::Equal => "equal",
            Self::Forbidden => "forbidden",
            Self::NotCovered => "not_covered",
        }
    }
}

#[derive(Clone, Debug)]
pub struct LitmusInterleavedReport {
    pub id: String,
    /// vCPUs of the machine (one per litmus thread).
    pub vcpus: usize,
    /// `"exhaustive"` or `"seeds"`.
    pub policy: &'static str,
    /// Runs made (schedules).
    pub runs: u64,
    pub observe: Vec<String>,
    pub histogram: Vec<LitmusInterleavedOutcome>,
    pub expected: Vec<Vec<u64>>,
    pub x86_tso: Vec<Vec<u64>>,
    pub forbidden: Vec<Vec<u64>>,
    pub not_covered: Vec<Vec<u64>>,
    /// Runs in which a thread did not end or the scheduler did not end on the schedule's end.
    pub incomplete_runs: u64,
    /// Instructions executed by the hook and by the interpreter, all runs.
    pub hooked_retired: u64,
    pub interpreted_retired: u64,
    pub elapsed_ns: u128,
    pub verdict: LitmusInterleavedVerdict,
}

fn guest(detail: String) -> LitmusError {
    LitmusError::Guest {
        cpu: 0,
        rip: 0,
        exit: detail,
    }
}

/// One machine for every run of the exploration.
struct Machine<'c> {
    case: &'c Case,
    board: LoadedFirmwareBoard,
    ends: Vec<u64>,
    totals: Vec<usize>,
    hooked_retired: u64,
    interpreted_retired: u64,
}

impl<'c> Machine<'c> {
    fn new(case: &'c Case) -> Result<Self, LitmusError> {
        let threads = case.threads.len();
        if !(2..=4).contains(&threads) {
            return Err(LitmusError::Policy(format!(
                "{}: the interleaved hooked run takes 2..=4 threads, the case has {threads}",
                case.id
            )));
        }
        let board = build_board(case, threads, true).map_err(LitmusError::Load)?;
        let ends = case
            .threads
            .iter()
            .map(|thread| {
                thread.code_start
                    + thread
                        .instructions
                        .iter()
                        .map(|i| i.bytes.len() as u64)
                        .sum::<u64>()
            })
            .collect();
        let totals = case
            .threads
            .iter()
            .map(|thread| thread.instructions.len())
            .collect();
        Ok(Self {
            case,
            board,
            ends,
            totals,
            hooked_retired: 0,
            interpreted_retired: 0,
        })
    }

    /// The initial registers and RIP of every thread on its own vCPU, the cells at their initial
    /// values.
    fn reset(&mut self) -> Result<(), LitmusError> {
        for (index, spec) in self.case.threads.iter().enumerate() {
            let vcpu = self
                .board
                .vcpu_mut(index)
                .ok_or_else(|| guest(format!("no vCPU {index}")))?;
            vcpu.state = long_state(spec.code_start);
            for (register, value) in &spec.registers {
                vcpu.state.gpr[*register as usize] = *value;
            }
            vcpu.state.rflags = 0x2;
            vcpu.skip_first_boundary = false;
            vcpu.state.activity = Some(CpuActivity::Running);
        }
        let memory = self.board.bus_mut().physical_memory();
        for (_, address, initial) in &self.case.cells {
            memory
                .store_u32(GuestPhysicalAddress::new(*address), *initial as u32, |_| {})
                .map_err(|error| guest(format!("resetting a cell: {error:?}")))?;
        }
        Ok(())
    }

    fn outcome(&mut self) -> Result<Outcome, LitmusError> {
        let mut values = Vec::with_capacity(self.case.observe.len());
        for field in &self.case.observe {
            match field {
                Field::Register { cpu, register } => {
                    let vcpu = self
                        .board
                        .vcpu(*cpu)
                        .ok_or_else(|| guest(format!("no vCPU {cpu}")))?;
                    values.push(vcpu.state.gpr[*register as usize] & 0xFFFF_FFFF);
                }
                Field::Cell { address } => {
                    let memory = self.board.bus_mut().physical_memory();
                    let value = memory
                        .load_u32(GuestPhysicalAddress::new(*address))
                        .map_err(|error| guest(format!("reading a cell: {error:?}")))?;
                    values.push(u64::from(value));
                }
            }
        }
        Ok(values)
    }

    /// One run under an explicit schedule: the outcome and whether the run ended as it must
    /// (every thread at its end, the scheduler on the end of the schedule).
    fn run(
        &mut self,
        schedule: &[usize],
        hook: &dyn LitmusStepHook,
    ) -> Result<(Outcome, bool), LitmusError> {
        self.reset()?;
        let threads = self.case.threads.len();
        let ends = self.ends.clone();
        let totals = self.totals.clone();
        let mut done = vec![0usize; threads];
        let mut hooked = 0u64;
        let mut interpreted = 0u64;
        let hook_failure: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let finished = {
            let mut host = BoardSmpHost::new(
                &mut self.board,
                |board: &mut LoadedFirmwareBoard,
                 cpu: usize,
                 _budget: u64,
                 stop: &mut dyn FnMut() -> bool| {
                    let rip = board.vcpu(cpu).map(|vcpu| vcpu.state.rip);
                    if rip == Some(ends[cpu]) {
                        // The thread has ended (a schedule never asks for this).
                        if let Some(vcpu) = board.vcpu_mut(cpu) {
                            vcpu.state.activity = Some(CpuActivity::Halted);
                        }
                        return Ok(Executed::default());
                    }
                    let mut state = board
                        .vcpu(cpu)
                        .ok_or(FirmwareBoardError::Config("litmus: no such vCPU"))?
                        .state
                        .clone();
                    let outcome = hook.step(LitmusStep {
                        cpu,
                        thread: cpu,
                        index: done[cpu],
                        // One instruction per scheduler step: S6-19 gives every instruction
                        // exactly one step.
                        budget: 1,
                        state: &mut state,
                        bus: board.bus_mut(),
                        cr3: TABLES,
                        efer: EFER,
                    });
                    match outcome {
                        Ok(LitmusStepOutcome::Executed { retired }) if retired > 1 => {
                            // The hook was given a budget of one instruction.
                            *hook_failure.borrow_mut() = Some(format!(
                                "litmus: the hook retired {retired} over a budget of 1"
                            ));
                            return Err(FirmwareBoardError::Config(
                                "litmus interleaved: the step hook exceeded its budget",
                            ));
                        }
                        Ok(LitmusStepOutcome::Executed { retired }) if retired > 0 => {
                            if let Some(vcpu) = board.vcpu_mut(cpu) {
                                vcpu.state = state;
                            }
                            done[cpu] += retired as usize;
                            hooked += retired;
                            return Ok(Executed {
                                retired,
                                partial_rep: false,
                                device_ns: 0,
                            });
                        }
                        Ok(_) => {}
                        Err(detail) => {
                            *hook_failure.borrow_mut() = Some(detail);
                            return Err(FirmwareBoardError::Config(
                                "litmus interleaved: the step hook failed",
                            ));
                        }
                    }
                    let cli = CliProfile::NonPviLong64V1;
                    let sti = StiProfile::NonPviLong64V1;
                    let halt = HaltProfile::Long64V1;
                    let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                        .with_cli(&cli)
                        .with_cpu_owned_cr3(MovCr3Long64V1)
                        .with_pio_u8()
                        .with_halt(&halt)
                        .with_sti(&sti);
                    let executed = board.run_vcpu_set_quantum(cpu, 1, stop, &set)?;
                    done[cpu] += executed.retired as usize;
                    interpreted += executed.retired;
                    Ok(executed)
                },
            )
            .map_err(|error| guest(format!("board host: {error:?}")))?;
            let mut scheduler = SmpScheduler::new(
                threads,
                QuantumPolicy::Schedule(schedule.to_vec()),
                StopRequest::new(),
            )
            .map_err(|error| guest(format!("scheduler: {error:?}")))?;
            match scheduler.run(&mut host, schedule.len() as u64 + 16) {
                Ok(finished) => finished,
                Err(error) => {
                    let detail = hook_failure.borrow_mut().take();
                    return Err(guest(match detail {
                        Some(detail) => format!("hook: {detail}"),
                        None => format!("run: {error:?}"),
                    }));
                }
            }
        };
        self.hooked_retired += hooked;
        self.interpreted_retired += interpreted;
        let all_ended = done.iter().zip(&totals).all(|(done, total)| done == total)
            && self
                .ends
                .iter()
                .enumerate()
                .all(|(cpu, end)| self.board.vcpu(cpu).is_some_and(|v| v.state.rip == *end));
        let ended_right = all_ended && finished == Finished::ScheduleEnd;
        Ok((self.outcome()?, ended_right))
    }
}

/// The verdict of a run: unfinished runs rank above everything else (S6-19).
fn verdict_of(
    incomplete_runs: u64,
    forbidden: bool,
    not_covered: bool,
) -> LitmusInterleavedVerdict {
    if incomplete_runs > 0 {
        LitmusInterleavedVerdict::Incomplete
    } else if forbidden {
        LitmusInterleavedVerdict::Forbidden
    } else if not_covered {
        LitmusInterleavedVerdict::NotCovered
    } else {
        LitmusInterleavedVerdict::Equal
    }
}

#[cfg(test)]
mod verdict_tests {
    use super::*;

    #[test]
    fn an_incomplete_run_ranks_above_every_other_verdict() {
        use LitmusInterleavedVerdict::*;
        assert_eq!(verdict_of(1, true, true), Incomplete);
        assert_eq!(verdict_of(1, false, false), Incomplete);
        assert_eq!(verdict_of(0, true, true), Forbidden);
        assert_eq!(verdict_of(0, false, true), NotCovered);
        assert_eq!(verdict_of(0, false, false), Equal);
    }
}

/// Run `case` in the interleaved mode with `hook`: every order-preserving schedule when there are at
/// most `max_exhaustive` of them (the report's `policy` is `"exhaustive"`), otherwise one run per
/// seed of `seeds` (`"seeds"`, at most 1 000 000 seeds).
pub fn run_litmus_interleaved_hooked(
    case: &LitmusCase,
    max_exhaustive: u128,
    seeds: RangeInclusive<u64>,
    hook: &dyn LitmusStepHook,
) -> Result<LitmusInterleavedReport, LitmusError> {
    let case = case.case();
    let started = Instant::now();
    let mut machine = Machine::new(case)?;
    let lengths: Vec<usize> = case.steps_per_thread();
    let exhaustive = case
        .schedule_count()
        .is_some_and(|count| count <= max_exhaustive);
    let mut histogram: BTreeMap<Outcome, u64> = BTreeMap::new();
    let mut runs = 0u64;
    let mut incomplete = 0u64;
    let mut failure: Option<LitmusError> = None;
    let mut one = |schedule: &[usize]| {
        if failure.is_some() {
            return;
        }
        match machine.run(schedule, hook) {
            Ok((outcome, ended)) => {
                runs += 1;
                incomplete += u64::from(!ended);
                *histogram.entry(outcome).or_insert(0) += 1;
            }
            Err(error) => failure = Some(error),
        }
    };
    if exhaustive {
        for_each_schedule(&lengths, &mut |schedule| one(schedule));
    } else {
        let count = seeds.end().saturating_sub(*seeds.start()).saturating_add(1);
        if count > MAX_RUNS {
            return Err(LitmusError::Policy(format!(
                "at most {MAX_RUNS} seeds, got {count}"
            )));
        }
        for seed in seeds {
            one(&random_schedule(&lengths, seed));
        }
    }
    drop(one);
    // The per-thread state of the hook is released once, on this thread, for every vCPU.
    for cpu in 0..case.threads.len() {
        hook.thread_exit(cpu);
    }
    if let Some(error) = failure {
        return Err(error);
    }
    let observed: BTreeSet<&Outcome> = histogram.keys().collect();
    let forbidden: Vec<Vec<u64>> = histogram
        .keys()
        .filter(|values| !case.expected.contains(*values))
        .cloned()
        .collect();
    let not_covered: Vec<Vec<u64>> = case
        .expected
        .iter()
        .filter(|values| !observed.contains(values))
        .cloned()
        .collect();
    let verdict = verdict_of(incomplete, !forbidden.is_empty(), !not_covered.is_empty());
    Ok(LitmusInterleavedReport {
        id: case.id.clone(),
        vcpus: case.threads.len(),
        policy: if exhaustive { "exhaustive" } else { "seeds" },
        runs,
        observe: case.observe_names.clone(),
        histogram: histogram
            .into_iter()
            .map(|(values, runs)| LitmusInterleavedOutcome {
                expected: case.expected.contains(&values),
                in_x86_tso: case.reference_tso.contains(&values),
                values,
                runs,
            })
            .collect(),
        expected: case.expected.iter().cloned().collect(),
        x86_tso: case.reference_tso.iter().cloned().collect(),
        forbidden,
        not_covered,
        incomplete_runs: incomplete,
        hooked_retired: machine.hooked_retired,
        interpreted_retired: machine.interpreted_retired,
        elapsed_ns: started.elapsed().as_nanos(),
        verdict,
    })
}
