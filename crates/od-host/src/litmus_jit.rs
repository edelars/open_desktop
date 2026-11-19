//! S6-41e: the S6-19/S6-36 litmus corpus with the JIT in both modes.
//!
//! `od-machine` runs the litmus (S6-19 interleaved, S6-36 parallel) and does not depend on
//! `od-host`, so the JIT is connected here, on the host side, as a [`LitmusStepHook`]
//! ([`JitLitmusHook`]) that the two litmus drivers offer every instruction to before the
//! interpreter: [`run_litmus_jit_parallel`] is the S6-36 run (one host thread per vCPU, the start
//! barrier, the placements, the x86-TSO reference) and [`run_litmus_jit_interleaved`] the S6-19 run
//! (one thread, every order-preserving schedule) with the same hook. Nothing of the run, the
//! barriers or the reference sets differs from the interpreter-only runs; only the execution of the
//! instructions the hook takes does.
//!
//! **What the hook takes.** An instruction that [`od_jit::atomic_adapter::lower_atomic_instruction`]
//! translates (the `LOCK` forms, `XCHG`, `MFENCE`: the forms S6-41d admitted into a block) runs
//! through [`PagedFetchedLoadSession::execute_atomic`]: tracked fetch of the code window through
//! the guest page tables, a block of up to 16 consecutive atomic forms, the helper "translate and
//! check" with write intent, the arm64 sequence (host atomics in the parallel mode, `LDADDAL`-style
//! instructions with the memory-order mode of the bus). Every other instruction of a fixture (the
//! ordinary `MOV` loads and stores with an absolute address, the unlocked `INC`) stays with the
//! interpreter: the block profile of S6-41d does not admit ordinary memory effects and the forms of
//! `execute_forms` (S4-15) are `RBX`/`RIP`-relative, not the absolute `[disp32]` of the corpus.
//! The share is recorded in the report (`jit_retired`, `interpreter_retired`, the classification of
//! every instruction); a run in which the JIT executed no instruction is not counted
//! ([`JitLitmusCounters::counted`]).
//!
//! **Sessions.** The thread of a vCPU runs different litmus threads in different rounds (the
//! placements), and a session holds one block, so a session per vCPU would recompile in every round.
//! The hook keeps one session per pair (vCPU, litmus thread) on the vCPU's own thread (A1 of S6-40:
//! nothing of the session, the block or the handle crosses threads), each on its own part of the
//! process `MAP_JIT` region (`SharedJitRegion`, S6-42), and drops them on that thread in
//! [`LitmusStepHook::thread_exit`] before the run returns (A4, M1).
//!
//! **Not covered here (recorded in `JIT_CONTRACT.md`, S6-41e).** The page tables of the litmus board
//! are 4 KiB pages in the hooked bench (2 MiB leaves are not supported by `execute_atomic`; the
//! S6-36 bench is 2 MiB); the guest split `LOCK` gate is not handed to the host
//! quantum (S6-42c) and the corpus has no split operand; interrupts are not delivered (a litmus has
//! none).

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_jit::atomic_adapter::{AtomicAdapterError, lower_atomic_instruction};
use od_machine::{
    LitmusCase, LitmusError, LitmusInstruction, LitmusInterleavedReport, LitmusParallelReport,
    LitmusStep, LitmusStepHook, LitmusStepOutcome, run_litmus_interleaved_hooked,
    run_litmus_parallel_hooked,
};

use crate::atomic_inline::AtomicInlineExit;
use crate::jit_region::{JitRegionPart, SharedJitRegion};
use crate::{FetchedWindow, PagedFetchAdmission, PagedFetchedLoadSession};

/// The most litmus threads and vCPUs of the hooked runs (the parallel harness's bound).
const MAX_THREADS: usize = 4;
/// Slots per part of the region: a session holds one block and drops the old one before the
/// next (`execute_atomic`), two leave room for the replacement.
const SLOTS_PER_PART: usize = 2;

/// How the JIT hook treats one instruction of a fixture.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JitForm {
    /// Executed through `execute_atomic`.
    Jit,
    /// Left to the interpreter, with the reason.
    Interpreter(String),
}

/// One classified instruction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JitInstruction {
    pub thread: usize,
    pub index: usize,
    pub asm: String,
    pub bytes: Vec<u8>,
    pub form: JitForm,
}

fn classify(instruction: &LitmusInstruction) -> JitForm {
    let mut window = FetchedWindow(&instruction.bytes);
    let decoded = od_isa::long_decode::decode(&mut window, 0, AccessContext::default());
    match decoded {
        Err(error) => JitForm::Interpreter(format!("not decoded: {error:?}")),
        Ok(decoded) => match lower_atomic_instruction(&decoded, 0) {
            Ok(_) => JitForm::Jit,
            Err(AtomicAdapterError::NotAnAtomicForm) => JitForm::Interpreter(
                "not an atomic form: an ordinary load, store or read-modify-write; the block \
                 profile of S6-41d admits only LOCK forms, XCHG and fences, and the forms of \
                 execute_forms (S4-15) are RBX/RIP-relative, not the absolute [disp32] of the \
                 corpus"
                    .to_string(),
            ),
            Err(other) => JitForm::Interpreter(format!("refused by the atomic adapter: {other:?}")),
        },
    }
}

/// The classification of every instruction of `case` (no run).
pub fn jit_classification(case: &LitmusCase) -> Vec<JitInstruction> {
    case.program()
        .into_iter()
        .enumerate()
        .flat_map(|(thread, instructions)| {
            instructions
                .into_iter()
                .enumerate()
                .map(move |(index, instruction)| JitInstruction {
                    thread,
                    index,
                    form: classify(&instruction),
                    asm: instruction.asm,
                    bytes: instruction.bytes,
                })
        })
        .collect()
}

/// What the hook did, summed over all vCPUs after they finish the run.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JitLitmusCounters {
    /// Instructions executed through `execute_atomic`.
    pub jit_retired: u64,
    /// Calls of `execute_atomic`.
    pub jit_calls: u64,
    /// Blocks compiled and published (`PagedFetchAdmission::Admitted`) and blocks reused.
    pub blocks_admitted: u64,
    pub blocks_reused: u64,
    /// Calls of the block's helper "translate and check" (0 for fences only).
    pub helper_calls: u64,
    /// Offers the hook declined because the instruction is not a JIT form.
    pub ordinary_declined: u64,
    /// `execute_atomic` calls in which the helper declined the operand (the interpreter then
    /// executed the instruction).
    pub helper_declined: u64,
    /// Instructions of the fixture that are JIT forms, per round of the run, summed over threads.
    pub jit_forms_per_round: u64,
    /// Rounds (parallel) or runs (interleaved) of the report.
    pub rounds: u64,
}

impl JitLitmusCounters {
    /// Instructions the JIT forms of the fixture would take if nothing fell back.
    pub fn jit_expected(&self) -> u64 {
        self.jit_forms_per_round * self.rounds
    }

    /// JIT-form instructions that the interpreter executed (a helper decline).
    pub fn fallback(&self) -> u64 {
        self.jit_expected().saturating_sub(self.jit_retired)
    }

    /// The run is counted only if the JIT executed instructions and executed every JIT form of
    /// every round (no fallback): a run in which it did not execute the forms of the litmus is
    /// not a litmus with the JIT (S6-41e).
    pub fn counted(&self) -> bool {
        self.jit_retired > 0 && self.jit_calls > 0 && self.fallback() == 0
    }

    fn accumulate(&mut self, other: &Self) {
        self.jit_retired += other.jit_retired;
        self.jit_calls += other.jit_calls;
        self.blocks_admitted += other.blocks_admitted;
        self.blocks_reused += other.blocks_reused;
        self.helper_calls += other.helper_calls;
        self.ordinary_declined += other.ordinary_declined;
        self.helper_declined += other.helper_declined;
    }
}

struct SessionSlot {
    owner: u64,
    cpu: usize,
    thread: usize,
    session: Option<PagedFetchedLoadSession>,
    counters: JitLitmusCounters,
}

thread_local! {
    /// The sessions of the vCPUs this thread runs (one thread per vCPU in the parallel mode, the
    /// calling thread for all in the interleaved mode), emptied by `thread_exit` on this thread.
    static SESSIONS: RefCell<Vec<SessionSlot>> = const { RefCell::new(Vec::new()) };
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

fn ring0() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

/// The JIT as a litmus step hook. One value per run (it holds the parts of the region).
pub struct JitLitmusHook {
    owner: u64,
    threads: usize,
    /// `form[thread][index]`: whether the instruction is a JIT form.
    is_jit: Vec<Vec<bool>>,
    /// `parts[cpu * threads + thread]`.
    parts: Mutex<Vec<Option<JitRegionPart>>>,
    completed: Mutex<JitLitmusCounters>,
    #[cfg(test)]
    ram_probe: bool,
    #[cfg(test)]
    replace_mfence: bool,
}

impl JitLitmusHook {
    fn new(case: &LitmusCase, vcpus: usize, region: &SharedJitRegion) -> Result<Self, LitmusError> {
        let program = case.program();
        let threads = program.len();
        let is_jit = program
            .iter()
            .map(|instructions| {
                instructions
                    .iter()
                    .map(|instruction| classify(instruction) == JitForm::Jit)
                    .collect()
            })
            .collect();
        let mut parts = Vec::with_capacity(vcpus * threads);
        for _ in 0..vcpus * threads {
            parts.push(Some(region.take_part().map_err(|error| {
                LitmusError::Load(format!("JIT region part: {error:?}"))
            })?));
        }
        Ok(Self {
            owner: NEXT_OWNER.fetch_add(1, Ordering::Relaxed),
            threads,
            is_jit,
            parts: Mutex::new(parts),
            completed: Mutex::new(JitLitmusCounters::default()),
            #[cfg(test)]
            ram_probe: false,
            #[cfg(test)]
            replace_mfence: false,
        })
    }

    fn jit_forms_per_round(&self) -> u64 {
        self.is_jit
            .iter()
            .map(|thread| thread.iter().filter(|jit| **jit).count() as u64)
            .sum()
    }

    fn counters(&self, rounds: u64) -> JitLitmusCounters {
        let mut counters = self
            .completed
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        counters.jit_forms_per_round = self.jit_forms_per_round();
        counters.rounds = rounds;
        counters
    }
}

impl LitmusStepHook for JitLitmusHook {
    fn step(&self, step: LitmusStep<'_>) -> Result<LitmusStepOutcome, String> {
        let jit = self
            .is_jit
            .get(step.thread)
            .and_then(|thread| thread.get(step.index))
            .copied()
            .ok_or_else(|| {
                format!(
                    "no instruction {} of thread {} in the program",
                    step.index, step.thread
                )
            })?;
        SESSIONS.with(|cell| {
            let mut slots = cell.borrow_mut();
            let position = match slots.iter().position(|slot| {
                slot.owner == self.owner && slot.cpu == step.cpu && slot.thread == step.thread
            }) {
                Some(position) => position,
                None => {
                    slots.push(SessionSlot {
                        owner: self.owner,
                        cpu: step.cpu,
                        thread: step.thread,
                        session: None,
                        counters: JitLitmusCounters::default(),
                    });
                    slots.len() - 1
                }
            };
            let slot = &mut slots[position];
            if !jit {
                slot.counters.ordinary_declined += 1;
                return Ok(LitmusStepOutcome::Declined);
            }
            if slot.session.is_none() {
                let part = self
                    .parts
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .get_mut(step.cpu * self.threads + step.thread)
                    .and_then(Option::take)
                    .ok_or("no region part left for the session")?;
                let session = PagedFetchedLoadSession::with_region_part(part);
                #[cfg(test)]
                let session = PagedFetchedLoadSession {
                    replace_mfence_for_ram_probe: self.replace_mfence,
                    ..session
                };
                slot.session = Some(session);
            }
            let session = slot.session.as_mut().expect("the slot has a session");
            // The session refuses a CPU-owned CR3 (it takes the root as an argument): the litmus
            // vCPU carries its CR3 in the state, so it is lent to the call and put back.
            let saved_cr3 = step.state.cr3.take();
            let result = session.execute_atomic(
                step.state,
                step.bus,
                step.cr3,
                step.efer,
                ring0(),
                step.budget,
            );
            step.state.cr3 = saved_cr3;
            let report = result.map_err(|error| {
                format!(
                    "execute_atomic at cpu{} thread {} instruction {}: {:?}",
                    step.cpu, step.thread, step.index, error.kind
                )
            })?;
            #[cfg(test)]
            if self.ram_probe {
                assert!(report.fetch_committed_writes.is_empty());
            }
            let counters = &mut slot.counters;
            counters.jit_calls += 1;
            match report.admission {
                PagedFetchAdmission::Admitted => counters.blocks_admitted += 1,
                PagedFetchAdmission::Reused => counters.blocks_reused += 1,
            }
            counters.helper_calls += u64::from(report.run.helper_calls);
            let retired = report.run.retired;
            counters.jit_retired += retired;
            match report.run.exit {
                AtomicInlineExit::Complete | AtomicInlineExit::BudgetExhausted if retired > 0 => {
                    Ok(LitmusStepOutcome::Executed { retired })
                }
                AtomicInlineExit::Declined | AtomicInlineExit::BudgetExhausted => {
                    // The helper declined the operand (nothing was changed by that instruction):
                    // the interpreter executes it, and the run records the fallback.
                    counters.helper_declined += 1;
                    if retired > 0 {
                        Ok(LitmusStepOutcome::Executed { retired })
                    } else {
                        Ok(LitmusStepOutcome::Declined)
                    }
                }
                other => Err(format!(
                    "cpu{} thread {} instruction {}: the block ended with {other:?} after {retired} \
                     instruction(s) (a litmus has no fault and no stop inside a block)",
                    step.cpu, step.thread, step.index
                )),
            }
        })
    }

    fn thread_exit(&self, cpu: usize) {
        let dropped: Vec<SessionSlot> = SESSIONS
            .try_with(|cell| {
                let mut slots = cell.borrow_mut();
                let mut kept = Vec::new();
                let mut gone = Vec::new();
                for slot in slots.drain(..) {
                    if slot.owner == self.owner && slot.cpu == cpu {
                        gone.push(slot);
                    } else {
                        kept.push(slot);
                    }
                }
                *slots = kept;
                gone
            })
            .unwrap_or_default();
        // Publish only after this vCPU has left the measured rounds.
        {
            let mut completed = self.completed.lock().unwrap_or_else(|e| e.into_inner());
            for slot in &dropped {
                completed.accumulate(&slot.counters);
            }
        }
        // Dropped here, on the vCPU's own thread, outside the borrow.
        drop(dropped);
    }
}

/// The S6-36 parallel run with the JIT.
#[derive(Clone, Debug)]
pub struct JitLitmusParallel {
    pub litmus: LitmusParallelReport,
    pub jit: JitLitmusCounters,
    pub classification: Vec<JitInstruction>,
}

/// The S6-19 interleaved run with the JIT.
#[derive(Clone, Debug)]
pub struct JitLitmusInterleaved {
    pub litmus: LitmusInterleavedReport,
    pub jit: JitLitmusCounters,
    pub classification: Vec<JitInstruction>,
}

fn check_jit_forms(case: &LitmusCase) -> Result<Vec<JitInstruction>, LitmusError> {
    let classification = jit_classification(case);
    if !classification
        .iter()
        .any(|instruction| instruction.form == JitForm::Jit)
    {
        return Err(LitmusError::Policy(format!(
            "{}: no instruction of the fixture is a JIT form (LOCK, XCHG, fence): a run with the JIT \
             would execute none of them and is not counted",
            case.id()
        )));
    }
    Ok(classification)
}

fn region_for(vcpus: usize, threads: usize) -> Result<SharedJitRegion, LitmusError> {
    if vcpus == 0 || threads == 0 || vcpus > MAX_THREADS || threads > MAX_THREADS {
        return Err(LitmusError::Policy(format!(
            "the hooked litmus takes 1..={MAX_THREADS} vCPUs and threads"
        )));
    }
    SharedJitRegion::new(vcpus * threads, SLOTS_PER_PART)
        .map_err(|error| LitmusError::Load(format!("JIT region: {error:?}")))
}

/// Run `case` for `rounds` rounds in the parallel mode on `vcpus` vCPUs with the JIT hook (the run
/// of `od_machine::run_litmus_parallel`, with the atomic forms through `execute_atomic`). `Err`
/// for a fixture without a JIT form. The caller holds the process-wide JIT lock (one `MAP_JIT`
/// region per process).
pub fn run_litmus_jit_parallel(
    case: &LitmusCase,
    vcpus: usize,
    rounds: u64,
) -> Result<JitLitmusParallel, LitmusError> {
    let classification = check_jit_forms(case)?;
    let threads = case.vcpus();
    let region = region_for(vcpus, threads)?;
    let hook = JitLitmusHook::new(case, vcpus, &region)?;
    let litmus = run_litmus_parallel_hooked(case, vcpus, rounds, &hook)?;
    let jit = hook.counters(rounds);
    drop(hook);
    drop(region);
    Ok(JitLitmusParallel {
        litmus,
        jit,
        classification,
    })
}

/// Run `case` in the interleaved mode (S6-19: one vCPU per litmus thread, every order-preserving
/// schedule while there are at most `max_exhaustive`, else the schedules of `seeds`) with the JIT
/// hook.
pub fn run_litmus_jit_interleaved(
    case: &LitmusCase,
    max_exhaustive: u128,
    seeds: std::ops::RangeInclusive<u64>,
) -> Result<JitLitmusInterleaved, LitmusError> {
    let classification = check_jit_forms(case)?;
    let threads = case.vcpus();
    let region = region_for(threads, threads)?;
    let hook = JitLitmusHook::new(case, threads, &region)?;
    let litmus = run_litmus_interleaved_hooked(case, max_exhaustive, seeds, &hook)?;
    let jit = hook.counters(litmus.runs);
    drop(hook);
    drop(region);
    Ok(JitLitmusInterleaved {
        litmus,
        jit,
        classification,
    })
}

/// The fixtures of a corpus directory that have a JIT form, and those that do not, by id.
pub fn split_corpus_by_jit_form(cases: &[LitmusCase]) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut with = BTreeSet::new();
    let mut without = BTreeSet::new();
    for case in cases {
        let has = jit_classification(case)
            .iter()
            .any(|instruction| instruction.form == JitForm::Jit);
        if has {
            with.insert(case.id().to_string());
        } else {
            without.insert(case.id().to_string());
        }
    }
    (with, without)
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_machine::{LitmusInterleavedVerdict, LitmusParallelVerdict};
    use std::path::{Path, PathBuf};

    fn corpus() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/guest/stage6/litmus")
    }

    fn fixture(file: &str) -> LitmusCase {
        LitmusCase::from_fixture_file(&corpus().join(file)).expect("fixture")
    }

    fn counter(id: &str, threads: u64, rounds: u64) -> LitmusCase {
        let source = std::fs::read_to_string(corpus().join("counters-formula.json")).unwrap();
        LitmusCase::from_formula_json(&source, id, threads, rounds).expect("counter")
    }

    fn jit_lock() -> std::sync::MutexGuard<'static, ()> {
        crate::tests::JIT_TEST_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    fn ram_probe_mfence(
        vcpus: usize,
        rounds: u64,
        replace_mfence: bool,
    ) -> (LitmusParallelReport, JitLitmusCounters) {
        let case = fixture("sb-mfence.json");
        let region = region_for(vcpus, case.vcpus()).unwrap();
        let mut hook = JitLitmusHook::new(&case, vcpus, &region).unwrap();
        hook.ram_probe = true;
        hook.replace_mfence = replace_mfence;
        let report =
            od_machine::run_litmus_parallel_ram_probe(&case, vcpus, rounds, Some(&hook)).unwrap();
        let jit = hook.counters(rounds);
        assert_eq!(report.rounds, rounds);
        assert_eq!(report.late_barrier_rounds, 0);
        assert_eq!(report.hooked_retired, 2 * rounds);
        assert_eq!(report.interpreted_retired, 4 * rounds);
        assert_eq!(jit.jit_calls, 2 * rounds);
        assert_eq!(jit.jit_retired, 2 * rounds);
        assert_eq!(jit.ordinary_declined, 4 * rounds);
        assert_eq!(jit.helper_calls, 0);
        assert_eq!(jit.helper_declined, 0);
        assert!(jit.counted());
        (report, jit)
    }

    #[test]
    fn s6_41e_ram_probe_counts_guest_movs_and_emitted_mfence() {
        let _guard = jit_lock();
        for vcpus in [2, 4] {
            let (report, _) = ram_probe_mfence(vcpus, 100, false);
            assert_eq!(report.verdict, LitmusParallelVerdict::Allowed);
        }
    }

    #[test]
    #[ignore = "hardware sensitivity probe: run the release weak-host-ordering build"]
    fn s6_41e_ram_probe_weak_sb_and_mfence_mutation() {
        let _guard = jit_lock();
        assert!(
            od_machine::BUILD_WEAK_HOST_ORDERING,
            "enable od-machine/weak-host-ordering"
        );
        let rounds = std::env::var("OD_S6_SB_RAM_ROUNDS")
            .map_or(200_000, |value| value.parse().expect("round count"));
        let both_zero = |report: &LitmusParallelReport| {
            report
                .histogram
                .iter()
                .find(|entry| entry.values == [0, 0, 1, 1])
                .map_or(0, |entry| entry.rounds)
        };
        let mut sensitive = true;
        for vcpus in [2, 4] {
            for file in ["mp.json", "sb.json"] {
                let case = fixture(file);
                let report =
                    od_machine::run_litmus_parallel_ram_probe(&case, vcpus, rounds, None).unwrap();
                assert_eq!(report.rounds, rounds);
                assert_eq!(report.late_barrier_rounds, 0);
                assert_eq!(report.hooked_retired, 0);
                assert_eq!(report.interpreted_retired, 4 * rounds);
                let counts: Vec<_> = report
                    .histogram
                    .iter()
                    .map(|entry| (&entry.values, entry.rounds))
                    .collect();
                eprintln!("{vcpus} vCPU {file}: {counts:?}");
                if file == "sb.json" {
                    sensitive &= both_zero(&report) != 0;
                }
            }
            for (label, replace) in [("DMB ISH", false), ("NOP", true)] {
                let (report, jit) = ram_probe_mfence(vcpus, rounds, replace);
                let counts: Vec<_> = report
                    .histogram
                    .iter()
                    .map(|entry| (&entry.values, entry.rounds))
                    .collect();
                eprintln!("{vcpus} vCPU SB-MFENCE {label}: {counts:?}, JIT {jit:?}");
                if replace {
                    sensitive &= both_zero(&report) != 0;
                } else {
                    assert_eq!(report.verdict, LitmusParallelVerdict::Allowed);
                }
            }
        }
        assert!(
            sensitive,
            "inconclusive: the weak SB and NOP controls must both show (0,0)"
        );
    }

    /// The classification names which instructions the JIT takes and why the others stay with the
    /// interpreter; a fixture with no JIT form is refused (not counted).
    #[test]
    fn s6_41e_classification_and_refusal_of_a_fixture_without_a_jit_form() {
        let sb_lock = jit_classification(&fixture("sb-lock.json"));
        let forms: Vec<(usize, usize, bool)> = sb_lock
            .iter()
            .map(|i| (i.thread, i.index, i.form == JitForm::Jit))
            .collect();
        assert_eq!(
            forms,
            vec![
                (0, 0, false),
                (0, 1, true),
                (0, 2, false),
                (1, 0, false),
                (1, 1, true),
                (1, 2, false)
            ]
        );
        let reason = match &sb_lock[0].form {
            JitForm::Interpreter(reason) => reason.clone(),
            JitForm::Jit => unreachable!(),
        };
        assert!(reason.contains("not an atomic form"), "{reason}");
        for file in [
            "sb-mfence.json",
            "sb-xchg.json",
            "lock-xadd.json",
            "lock-cmpxchg-mutex.json",
            "xchg-mutex.json",
            "sb-lock-xadd.json",
            "sb-lock-local.json",
        ] {
            assert!(
                jit_classification(&fixture(file))
                    .iter()
                    .any(|i| i.form == JitForm::Jit),
                "{file}"
            );
        }
        for file in [
            "sb.json",
            "mp.json",
            "lb.json",
            "2+2w.json",
            "iriw.json",
            "store-forwarding.json",
            "unlocked-inc.json",
        ] {
            assert!(
                jit_classification(&fixture(file))
                    .iter()
                    .all(|i| matches!(i.form, JitForm::Interpreter(_))),
                "{file}"
            );
            let _guard = jit_lock();
            let error = run_litmus_jit_parallel(&fixture(file), 2, 10).unwrap_err();
            assert!(
                format!("{error:?}").contains("not counted"),
                "{file}: {error:?}"
            );
        }
    }

    /// SB-LOCK in the parallel mode through the JIT: the `LOCK INC` of both threads runs through
    /// `execute_atomic` in every round, no outcome outside x86-TSO.
    #[test]
    fn s6_41e_parallel_sb_lock_runs_its_lock_inc_through_the_jit() {
        let _guard = jit_lock();
        let run = run_litmus_jit_parallel(&fixture("sb-lock.json"), 2, 3_000).unwrap();
        eprintln!("{:?}\n{:?}", run.jit, run.litmus.histogram);
        assert_eq!(run.litmus.verdict, LitmusParallelVerdict::Allowed);
        assert!(run.litmus.forbidden.is_empty());
        assert_eq!(run.jit.jit_expected(), 2 * 3_000);
        assert_eq!(run.jit.jit_retired, 2 * 3_000);
        assert_eq!(run.jit.fallback(), 0);
        assert!(run.jit.counted());
        assert_eq!(run.litmus.hooked_retired, run.jit.jit_retired);
        assert_eq!(
            run.litmus.hooked_retired + run.litmus.interpreted_retired,
            6 * 3_000
        );
        assert!(run.jit.blocks_reused > run.jit.blocks_admitted);
    }

    /// The interleaved mode through the JIT: every schedule of SB-XCHG, the observed set equals the
    /// instruction-atomic set of the fixture.
    #[test]
    fn s6_41e_interleaved_sb_xchg_equals_the_expected_set_through_the_jit() {
        let _guard = jit_lock();
        let run = run_litmus_jit_interleaved(&fixture("sb-xchg.json"), 1_000_000, 0..=0).unwrap();
        eprintln!("{:?}\n{:?}", run.jit, run.litmus.histogram);
        assert_eq!(run.litmus.policy, "exhaustive");
        assert_eq!(run.litmus.runs, 20);
        assert_eq!(run.litmus.verdict, LitmusInterleavedVerdict::Equal);
        assert_eq!(run.litmus.incomplete_runs, 0);
        assert_eq!(run.jit.jit_retired, 2 * 20);
        assert!(run.jit.counted());
    }

    /// A multi-instruction block: the LOCK-INC counter (K = 2 per thread, two consecutive
    /// `LOCK INC` per thread) in both modes; the total is N * K in every run.
    #[test]
    fn s6_41e_lock_inc_counter_in_both_modes() {
        let _guard = jit_lock();
        let case = counter("LOCK-INC", 2, 2);
        let parallel = run_litmus_jit_parallel(&case, 2, 2_000).unwrap();
        assert_eq!(parallel.litmus.verdict, LitmusParallelVerdict::Allowed);
        assert!(parallel.jit.counted());
        assert_eq!(parallel.jit.jit_retired, 4 * 2_000);
        let interleaved = run_litmus_jit_interleaved(&case, 1_000_000, 0..=0).unwrap();
        assert_eq!(interleaved.litmus.verdict, LitmusInterleavedVerdict::Equal);
        assert_eq!(interleaved.litmus.runs, 6);
        assert!(interleaved.jit.counted());
    }
}

#[cfg(test)]
mod counter_tests {
    use super::*;

    /// A run in which the JIT executed nothing, or fewer instructions than its forms of every
    /// round, is not counted (S6-41e sensitivity).
    #[test]
    fn s6_41e_a_run_without_jit_execution_or_with_fallback_is_not_counted() {
        let full = JitLitmusCounters {
            jit_retired: 20,
            jit_calls: 20,
            jit_forms_per_round: 2,
            rounds: 10,
            ..Default::default()
        };
        assert!(full.counted());
        assert!(
            !JitLitmusCounters {
                jit_retired: 0,
                jit_calls: 0,
                ..full.clone()
            }
            .counted()
        );
        let fallback = JitLitmusCounters {
            jit_retired: 19,
            ..full.clone()
        };
        assert_eq!(fallback.fallback(), 1);
        assert!(!fallback.counted());
    }
}
