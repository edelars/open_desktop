//! Differential check for the combined CPL0 profile set (S2-45a): on the
//! integer, branch, memory and stack inputs of the existing corpora, a state
//! without control registers run through `run` must match the same state with
//! CR0/CR4 modeled run through the set, up to the fields the set requires.
//!
//! Limits: no corpus contains CLI (0xfa), so the plain set and the set with
//! CLI execute the same instructions here and CLI is not covered
//! differentially; it is covered only by the targeted tests in
//! `crates/od-isa/tests/cpl0_profile_set.rs`. Only the BASE CR0/CR4 values are
//! used because `run` takes a state without control registers; values inside
//! the policy but not BASE (EM/TS, OSFXSR/OSXMMEXCPT) have no `run` baseline
//! and are covered by the targeted tests as well.

use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit, run,
    run_with_cpl0_profile_set, run_with_cpl0_profile_set_and_pio,
    run_with_cpl0_profile_set_and_pio_batched,
};
use od_isa::fast_path_check::{self, FastPathMode};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CliProfile, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, StiShadow,
};
use od_testkit::generate::generate_fixture;
use od_testkit::memory_corpus::{
    MEMORY_CORPUS_RAM_LEN, MEMORY_CORPUS_SEQUENCES, S3_34A_SEEDS, generate_memory_corpus_input,
};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Debug, Eq, PartialEq)]
struct Memory {
    regions: Vec<(u64, Vec<u8>)>,
}

impl Memory {
    fn slot(&self, address: u64, len: usize) -> Option<(usize, usize)> {
        self.regions
            .iter()
            .enumerate()
            .find_map(|(index, (base, bytes))| {
                let offset = address.checked_sub(*base)? as usize;
                (offset.checked_add(len)? <= bytes.len()).then_some((index, offset))
            })
    }
}

impl GuestMemory for Memory {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        let (index, offset) = self.slot(address, 1).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Fetch,
        })?;
        Ok(self.regions[index].1[offset])
    }

    fn read(&mut self, address: u64, _: AccessContext, out: &mut [u8]) -> Result<(), MemoryError> {
        let (index, offset) = self.slot(address, out.len()).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })?;
        out.copy_from_slice(&self.regions[index].1[offset..offset + out.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, data: &[u8]) -> Result<(), MemoryError> {
        let (index, offset) = self
            .slot(address, data.len())
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })?;
        self.regions[index].1[offset..offset + data.len()].copy_from_slice(data);
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        unreachable!()
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        unreachable!()
    }
}

struct Case {
    state: LongModeState,
    memory: Memory,
    stop_rip: u64,
}

fn config(case: &Case) -> RunConfig {
    RunConfig {
        instruction_budget: 64,
        stop_rip: Some(case.stop_rip),
        context: AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        },
    }
}

#[derive(Default)]
struct Tally {
    cases: usize,
    stopped_at_end: usize,
    other_exits: usize,
}

/// Run the case through `run` and through the set (with and without CLI) and
/// require identical observable results.
fn check(case: Case, tally: &mut Tally, label: &str) {
    let cli = CliProfile::NonPviLong64V1;
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let with_cli = plain.with_cli(&cli);
    // PERF-23: the fast path of the step, enabled; the result must be `run`'s, like the others.
    let fast = plain.with_fast_path();

    let mut base_state = case.state.clone();
    let mut base_memory = case.memory.clone();
    let base = run(&mut base_state, &mut base_memory, config(&case), || false);

    for set in [&plain, &with_cli, &fast] {
        let taken_before = fast_path_check::fast_steps_taken();
        let mut state = case.state.clone();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(0x20);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        let mut memory = case.memory.clone();
        let result = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(&case),
            od_isa::INTEGER_FLAGS,
            set,
            None,
            || false,
        )
        .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
        assert_eq!(
            result.exit,
            Cpl0ProfileSetExit::Run(base.exit.clone()),
            "{label}"
        );
        assert_eq!(
            result.instructions_executed, base.instructions_executed,
            "{label}"
        );
        assert_eq!(result.rflags_defined, base.rflags_defined, "{label}");
        // The set requires CR0/CR4, activity and the STI shadow; they must
        // come back unchanged and everything else must match `run`.
        assert_eq!(state.cr0, Some(0x8001_0033), "{label}");
        assert_eq!(state.cr4, Some(0x20), "{label}");
        assert_eq!(state.activity, Some(CpuActivity::Running), "{label}");
        assert_eq!(state.sti_shadow, Some(StiShadow::Clear), "{label}");
        state.cr0 = None;
        state.cr4 = None;
        state.activity = None;
        state.sti_shadow = None;
        assert_eq!(state, base_state, "{label}");
        assert_eq!(memory, base_memory, "{label}");
        // Only the set with the fast path takes steps through it (debug builds count them).
        let taken = fast_path_check::fast_steps_taken() - taken_before;
        if std::ptr::eq(set, &fast) {
            FAST_STEPS_TAKEN.fetch_add(taken, Ordering::Relaxed);
        } else {
            assert_eq!(taken, 0, "{label}");
        }
    }
    check_fast_path(&case, label);
    check_batched(&case, label);
    tally.cases += 1;
    if base.exit == od_isa::execute::RunExit::StopAddress {
        tally.stopped_at_end += 1;
    } else {
        tally.other_exits += 1;
    }
}

/// PERF-22: the same case through the set with the fast-path check (ADR-0017, section 5). The
/// checked run must end exactly like the plain set run (exit, count, definedness, state, memory),
/// with the empty fast path of the tree (nothing taken) and with the general path as its own fast
/// path (every instruction compared: all CPU state, the outcome and the ordered memory calls,
/// memory errors included). No divergence in either.
fn check_fast_path(case: &Case, label: &str) {
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    let admitted = |state: &LongModeState| {
        let mut state = state.clone();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(0x20);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        state
    };
    let mut base_state = admitted(&case.state);
    let mut base_memory = case.memory.clone();
    let base = run_with_cpl0_profile_set(
        &mut base_state,
        &mut base_memory,
        config(case),
        od_isa::INTEGER_FLAGS,
        &plain,
        None,
        || false,
    )
    .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
    for mode in [FastPathMode::Tree, FastPathMode::GeneralPath] {
        fast_path_check::set_mode(mode);
        fast_path_check::reset();
        let mut state = admitted(&case.state);
        let mut memory = case.memory.clone();
        let checked = run_with_cpl0_profile_set(
            &mut state,
            &mut memory,
            config(case),
            od_isa::INTEGER_FLAGS,
            &plain.with_fast_path_verify(),
            None,
            || false,
        )
        .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
        fast_path_check::set_mode(FastPathMode::Tree);
        let counters = fast_path_check::counters();
        assert_eq!(
            fast_path_check::take_divergence(),
            None,
            "{label}: {mode:?}"
        );
        assert_eq!(counters.divergences, 0, "{label}: {mode:?}");
        assert_eq!(checked, base, "{label}: {mode:?}");
        assert_eq!(state, base_state, "{label}: {mode:?}");
        assert_eq!(memory, base_memory, "{label}: {mode:?}");
        // Every retired instruction was either compared or refused by the admission of the fast
        // path of the tree (PERF-23: register ALU, group G2); a step that ends in a memory error
        // or a fault retires nothing but is still compared.
        match mode {
            FastPathMode::Tree => {
                assert!(
                    counters.compared + counters.refused_by_admission >= base.instructions_executed,
                    "{label}"
                );
                TREE_COMPARED.fetch_add(counters.compared, Ordering::Relaxed);
            }
            _ => {
                assert!(counters.compared >= base.instructions_executed, "{label}");
                assert_eq!(counters.refused_by_admission, 0, "{label}");
                FAST_PATH_COMPARED.fetch_add(counters.compared, Ordering::Relaxed);
                FAST_PATH_CALLS.fetch_add(counters.memory_calls_compared, Ordering::Relaxed);
            }
        }
    }
}

/// The corpora have no port instruction; the sets have no port member either.
struct NoPort;

impl PortIo for NoPort {
    type Error = ();

    fn read_u8(&mut self, _: u16) -> Result<u8, ()> {
        unreachable!()
    }

    fn write_u8(&mut self, _: u16, _: u8) -> Result<(), ()> {
        unreachable!()
    }
}

/// PERF-25: the case as the board loop runs it, through one-instruction calls of the set (each with
/// its own admission), and through batched calls (`run_with_cpl0_profile_set_and_pio_batched`) whose
/// boundary check agrees always, or refuses at every 1st, 3rd or 7th boundary it is asked at (the
/// caller then goes on with a new call). The exit that ends the run, the retired count, the
/// definedness, the state and the memory must be the same, with and without the fast path.
fn check_batched(case: &Case, label: &str) {
    let admitted = |state: &LongModeState| {
        let mut state = state.clone();
        state.cr0 = Some(0x8001_0033);
        state.cr4 = Some(0x20);
        state.activity = Some(CpuActivity::Running);
        state.sti_shadow = Some(StiShadow::Clear);
        state
    };
    let budget = config(case).instruction_budget;
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for set in [plain, plain.with_fast_path()] {
        let mut reference_state = admitted(&case.state);
        let mut reference_memory = case.memory.clone();
        let (mut reference_retired, mut reference_defined) = (0, od_isa::INTEGER_FLAGS);
        let reference_exit = loop {
            let result = run_with_cpl0_profile_set_and_pio(
                &mut reference_state,
                &mut reference_memory,
                RunConfig {
                    instruction_budget: 1,
                    ..config(case)
                },
                reference_defined,
                &set,
                None,
                &mut NoPort,
                || false,
            )
            .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
            reference_retired += result.instructions_executed;
            reference_defined = result.rflags_defined;
            if result.exit != Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                || result.instructions_executed == 0
                || reference_retired == budget
            {
                break result.exit;
            }
        };
        for refuse_every in [None, Some(1), Some(3), Some(7)] {
            let mut state = admitted(&case.state);
            let mut memory = case.memory.clone();
            let (mut retired, mut defined, mut asked) = (0, od_isa::INTEGER_FLAGS, 0u64);
            let exit = loop {
                let result = run_with_cpl0_profile_set_and_pio_batched(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: budget - retired,
                        ..config(case)
                    },
                    defined,
                    &set,
                    None,
                    &mut NoPort,
                    || false,
                    &mut |_: &LongModeState, _: &mut Memory| {
                        asked += 1;
                        refuse_every.is_none_or(|every| asked % every != 0)
                    },
                )
                .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
                retired += result.instructions_executed;
                defined = result.rflags_defined;
                if result.exit == Cpl0ProfileSetExit::Run(RunExit::StopRequested) {
                    assert!(result.instructions_executed > 0, "{label}");
                    continue;
                }
                break result.exit;
            };
            let variant = format!("{label}: batched, refusing every {refuse_every:?}");
            assert_eq!(exit, reference_exit, "{variant}");
            assert_eq!(retired, reference_retired, "{variant}");
            assert_eq!(defined, reference_defined, "{variant}");
            assert_eq!(state, reference_state, "{variant}");
            assert_eq!(memory, reference_memory, "{variant}");
            // Asked before every instruction but the first of each call, never at the budget.
            if refuse_every.is_none() {
                assert!(asked <= retired, "{variant}");
            }
        }
    }
    BATCHED_CASES.fetch_add(1, Ordering::Relaxed);
}

fn print_fast_path_totals(corpus: &str) {
    // The counters are process-wide here, so the three tests (parallel threads) share them; the
    // lines are informational and the per-case asserts above are the check.
    println!(
        "{corpus}: fast-path check so far {} instructions compared against the general path, {} memory calls",
        FAST_PATH_COMPARED.load(Ordering::Relaxed),
        FAST_PATH_CALLS.load(Ordering::Relaxed)
    );
    assert!(FAST_PATH_COMPARED.load(Ordering::Relaxed) > 0);
    // PERF-23: the fast path of the tree takes the register ALU forms of the corpora, with no
    // divergence (each case asserts that); it must have compared some.
    println!(
        "{corpus}: the tree fast path so far compared {} instructions",
        TREE_COMPARED.load(Ordering::Relaxed)
    );
    assert!(TREE_COMPARED.load(Ordering::Relaxed) > 0);
    // PERF-23: with the fast path on, the step itself takes them (counted in debug builds).
    println!(
        "{corpus}: the step fast path so far took {} instructions",
        FAST_STEPS_TAKEN.load(Ordering::Relaxed)
    );
    if fast_path_check::FAST_STEPS_COUNTED {
        assert!(FAST_STEPS_TAKEN.load(Ordering::Relaxed) > 0);
    }
    // PERF-25: every case also went through the batched entry.
    println!(
        "{corpus}: batched calls so far matched one-instruction calls on {} cases",
        BATCHED_CASES.load(Ordering::Relaxed)
    );
    assert!(BATCHED_CASES.load(Ordering::Relaxed) > 0);
}

static FAST_STEPS_TAKEN: AtomicU64 = AtomicU64::new(0);
static BATCHED_CASES: AtomicU64 = AtomicU64::new(0);
static FAST_PATH_COMPARED: AtomicU64 = AtomicU64::new(0);
static FAST_PATH_CALLS: AtomicU64 = AtomicU64::new(0);
static TREE_COMPARED: AtomicU64 = AtomicU64::new(0);

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[test]
fn integer_and_branch_corpus_matches_run() {
    let mut tally = Tally::default();
    for seed in [
        0x5332_3435_0000_0001u64,
        0x5332_3435_0000_0002,
        0x5332_3435_0000_0003,
    ] {
        for sequence in 0..400u32 {
            let fixture = generate_fixture(seed, sequence);
            let program = fixture.program.bytes().expect("generated program decodes");
            let mut state = LongModeState {
                rip: 0x1000,
                rflags: 2,
                ..LongModeState::default()
            };
            state.gpr[0] = fixture.initial_state.rax.0;
            state.gpr[1] = fixture.initial_state.rcx.0;
            state.gpr[2] = fixture.initial_state.rdx.0;
            state.gpr[3] = fixture.initial_state.rbx.0;
            let stop_rip = 0x1000 + program.len() as u64;
            check(
                Case {
                    state,
                    memory: Memory {
                        regions: vec![(0x1000, program)],
                    },
                    stop_rip,
                },
                &mut tally,
                &format!("integer seed={seed:#x} sequence={sequence}"),
            );
        }
    }
    assert_eq!(tally.cases, 1200);
    assert!(tally.stopped_at_end > 0);
    println!(
        "integer/branch: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
    print_fast_path_totals("integer/branch");
}

#[test]
fn memory_corpus_matches_run() {
    let mut tally = Tally::default();
    for seed in S3_34A_SEEDS {
        for sequence in MEMORY_CORPUS_SEQUENCES {
            for index in 0..120 {
                let input = generate_memory_corpus_input(seed, index, Some(sequence));
                let program = sequence.bytes().to_vec();
                let stop_rip = 0x9e00 + program.len() as u64;
                let mut state = LongModeState {
                    rip: 0x9e00,
                    rflags: 2 | input.flags,
                    gpr: input.gpr,
                    ..LongModeState::default()
                };
                state.rflags &= !(1 << 9);
                check(
                    Case {
                        state,
                        memory: Memory {
                            regions: vec![
                                (0, input.ram[..MEMORY_CORPUS_RAM_LEN].to_vec()),
                                (0x9e00, program),
                            ],
                        },
                        stop_rip,
                    },
                    &mut tally,
                    &format!("memory seed={seed:#x} {sequence:?} index={index}"),
                );
            }
        }
    }
    assert_eq!(tally.cases, 3 * 3 * 120);
    assert!(tally.stopped_at_end > 0);
    println!(
        "memory: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
    print_fast_path_totals("memory");
}

#[test]
fn stack_forms_match_run() {
    // PUSH r64, POP r64, PUSH imm8, CALL rel32 + POP, PUSH r8, POP r9.
    let programs: [&[u8]; 6] = [
        &[0x50, 0x59],
        &[0x6a, 0x05, 0x5a],
        &[0xe8, 0x00, 0x00, 0x00, 0x00, 0x58],
        &[0x41, 0x50, 0x41, 0x59],
        &[0x53, 0x53, 0x5e, 0x5f],
        &[0x50, 0xc3],
    ];
    let mut tally = Tally::default();
    let mut random = SplitMix(0x5332_3435_5354_4b01);
    for (index, program) in programs.iter().enumerate() {
        for round in 0..200 {
            let mut state = LongModeState {
                rip: 0x1000,
                rflags: 2 | (random.next() & 0x8d5),
                ..LongModeState::default()
            };
            state.rflags &= !(1 << 9);
            for value in &mut state.gpr {
                *value = random.next();
            }
            // Stack inside, at both edges of, and outside the 64-byte region.
            let rsp = match round % 4 {
                0 => 0x2000 + 8 * (random.next() % 8),
                1 => 0x2000 + 64,
                2 => 0x2000 + (random.next() % 64),
                _ => 0x2008,
            };
            state.gpr[4] = rsp;
            let mut stack = vec![0u8; 64];
            for chunk in stack.chunks_mut(8) {
                chunk.copy_from_slice(&random.next().to_le_bytes());
            }
            let stop_rip = 0x1000 + program.len() as u64;
            check(
                Case {
                    state,
                    memory: Memory {
                        regions: vec![(0x1000, program.to_vec()), (0x2000, stack)],
                    },
                    stop_rip,
                },
                &mut tally,
                &format!("stack program={index} round={round}"),
            );
        }
    }
    assert_eq!(tally.cases, programs.len() * 200);
    // The generator is seeded, so the split is exact: 884 cases reach the end
    // of the code and 316 end on a memory error from the 64-byte stack region.
    assert_eq!((tally.stopped_at_end, tally.other_exits), (884, 316));
    println!(
        "stack: {} cases, {} stopped at code end, {} other exits",
        tally.cases, tally.stopped_at_end, tally.other_exits
    );
    print_fast_path_totals("stack");
}
