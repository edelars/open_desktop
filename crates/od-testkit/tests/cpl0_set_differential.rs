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
use od_isa::long_decode::DecodedBlock;
use od_isa::predecoded_block::{
    FastBlockRole, MAX_BLOCK_INSTRUCTIONS, fast_block_role, predecode_block,
};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CliProfile, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, OperandSize, PortIo, PrivilegeLevel, StiShadow,
};
use od_testkit::generate::generate_fixture;
use od_testkit::memory_corpus::{
    MEMORY_CORPUS_RAM_LEN, MEMORY_CORPUS_SEQUENCES, S3_34A_SEEDS, generate_memory_corpus_input,
};
use std::collections::HashMap;
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
    check_blocks(&case, label);
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
    let plain = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
    for set in [plain, plain.with_fast_path()] {
        let reference = one_instruction_reference(case, &set, label);
        for refuse_every in [None, Some(1), Some(3), Some(7)] {
            let mut state = admitted(&case.state);
            let mut memory = case.memory.clone();
            let (exit, retired, defined, asked) =
                run_batched_over(case, &set, &mut state, &mut memory, refuse_every, label);
            let variant = format!("{label}: batched, refusing every {refuse_every:?}");
            assert_eq!(exit, reference.exit, "{variant}");
            assert_eq!(retired, reference.retired, "{variant}");
            assert_eq!(defined, reference.defined, "{variant}");
            assert_eq!(state, reference.state, "{variant}");
            assert_eq!(memory, reference.memory, "{variant}");
            // Asked before every instruction but the first of each call, never at the budget.
            if refuse_every.is_none() {
                assert!(asked <= retired, "{variant}");
            }
        }
    }
    BATCHED_CASES.fetch_add(1, Ordering::Relaxed);
}

/// What a run of one-instruction calls of the set leaves (the reference of the batched checks).
struct Reference {
    exit: Cpl0ProfileSetExit<()>,
    retired: u64,
    defined: u64,
    state: LongModeState,
    memory: Memory,
}

fn admitted(state: &LongModeState) -> LongModeState {
    let mut state = state.clone();
    state.cr0 = Some(0x8001_0033);
    state.cr4 = Some(0x20);
    state.activity = Some(CpuActivity::Running);
    state.sti_shadow = Some(StiShadow::Clear);
    state
}

/// The case as the board loop runs it without batches: one-instruction calls of `set`, each with
/// its own admission, until an exit other than the budget or the budget of the case.
fn one_instruction_reference(case: &Case, set: &Cpl0ProfileSet<'_>, label: &str) -> Reference {
    let budget = config(case).instruction_budget;
    let mut state = admitted(&case.state);
    let mut memory = case.memory.clone();
    let (mut retired, mut defined) = (0, od_isa::INTEGER_FLAGS);
    let exit = loop {
        let result = run_with_cpl0_profile_set_and_pio(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                ..config(case)
            },
            defined,
            set,
            None,
            &mut NoPort,
            || false,
        )
        .unwrap_or_else(|error| panic!("{label}: admission failed: {error:?}"));
        retired += result.instructions_executed;
        defined = result.rflags_defined;
        if result.exit != Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
            || result.instructions_executed == 0
            || retired == budget
        {
            break result.exit;
        }
    };
    Reference {
        exit,
        retired,
        defined,
        state,
        memory,
    }
}

/// PERF-31 (ADR-0018, variant (a), prototype): the corpus memory with a predecoded block cache of
/// its own, by the rule of the board's: a block is built with `predecode_block` from the bytes
/// (their fetch has no effect here), keyed by its RIP and the generation of its 4 KiB page, and
/// every write raises the generation of each page it touches; `block_holds` compares the
/// generation of the active block's page. `ignore_writes` is the mutation that drops the
/// invalidation (the test of it must then see a stale block run); `ignore_page_fence` (PERF-33,
/// E5) the mutation that builds blocks without the page fence of `predecode_block`.
struct BlockMemory {
    inner: Memory,
    generations: HashMap<u64, u64>,
    blocks: HashMap<u64, (u64, Option<DecodedBlock>)>,
    active: Option<(u64, u64)>,
    retired_from_blocks: u64,
    holds_failed: u64,
    ignore_writes: bool,
    ignore_page_fence: bool,
}

/// PERF-33 (E5): the mutation of the page fence: the loop of `predecode_block` (the end of the page
/// of `rip`, the length limit, the role of each instruction) without its fence, so an instruction
/// that needs a byte of the next page is taken.
fn predecode_block_without_page_fence(
    code: &mut Memory,
    rip: u64,
    context: AccessContext,
) -> Vec<od_isa::long_decode::Instruction> {
    let mut block = Vec::new();
    let mut at = rip;
    while block.len() < MAX_BLOCK_INSTRUCTIONS && at >> 12 == rip >> 12 {
        let Ok(instruction) = od_isa::long_decode::decode(&mut *code, at, context) else {
            break;
        };
        let Some(role) = fast_block_role(&instruction) else {
            break;
        };
        block.push(instruction);
        at = at.wrapping_add(u64::from(instruction.length));
        if role == FastBlockRole::Terminator {
            break;
        }
    }
    block
}

impl BlockMemory {
    fn new(inner: Memory) -> Self {
        Self {
            inner,
            generations: HashMap::new(),
            blocks: HashMap::new(),
            active: None,
            retired_from_blocks: 0,
            holds_failed: 0,
            ignore_writes: false,
            ignore_page_fence: false,
        }
    }

    fn generation(&self, page: u64) -> u64 {
        self.generations.get(&page).copied().unwrap_or(0)
    }

    fn wrote(&mut self, address: u64, len: usize) {
        if self.ignore_writes || len == 0 {
            return;
        }
        let last = address.wrapping_add(len as u64 - 1);
        for page in address >> 12..=last >> 12 {
            *self.generations.entry(page).or_default() += 1;
        }
    }
}

impl GuestMemory for BlockMemory {
    fn fetch(&mut self, address: u64, context: AccessContext) -> Result<u8, MemoryError> {
        self.inner.fetch(address, context)
    }

    fn cached_block(&mut self, rip: u64, context: AccessContext) -> Option<DecodedBlock> {
        self.active = None;
        let page = rip >> 12;
        let generation = self.generation(page);
        let block = match self.blocks.get(&rip) {
            Some((held, block)) if *held == generation => block.clone(),
            _ => {
                let built = if self.ignore_page_fence {
                    predecode_block_without_page_fence(&mut self.inner, rip, context)
                } else {
                    predecode_block(&mut self.inner, rip, context, MAX_BLOCK_INSTRUCTIONS)
                };
                let block = (!built.is_empty()).then(|| DecodedBlock::from(built));
                self.blocks.insert(rip, (generation, block.clone()));
                block
            }
        }?;
        self.active = Some((page, generation));
        Some(block)
    }

    fn block_holds(&mut self) -> bool {
        let holds = self
            .active
            .is_some_and(|(page, generation)| self.generation(page) == generation);
        if !holds && self.active.take().is_some() {
            self.holds_failed += 1;
        }
        holds
    }

    fn block_retired(&mut self, _: usize) {
        self.retired_from_blocks += 1;
    }

    fn read(
        &mut self,
        address: u64,
        context: AccessContext,
        out: &mut [u8],
    ) -> Result<(), MemoryError> {
        self.inner.read(address, context, out)
    }

    fn write(
        &mut self,
        address: u64,
        context: AccessContext,
        data: &[u8],
    ) -> Result<(), MemoryError> {
        self.inner.write(address, context, data)?;
        self.wrote(address, data.len());
        Ok(())
    }

    fn atomic_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        value: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_exchange_integer(address, context, size, value)
    }

    fn atomic_fetch_add_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        value: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_fetch_add_integer(address, context, size, value)
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        address: u64,
        context: AccessContext,
        size: OperandSize,
        expected: u64,
        replacement: u64,
    ) -> Result<u64, MemoryError> {
        self.inner
            .atomic_compare_exchange_integer(address, context, size, expected, replacement)
    }

    fn mfence(&mut self, context: AccessContext) -> Result<(), MemoryError> {
        self.inner.mfence(context)
    }
}

/// Batched calls of `set` over `memory`, as `check_batched` makes them (the boundary check
/// refuses at every `refuse_every`-th boundary it is asked at), until an exit other than the
/// refusal. Returns the exit, the retired count, the definedness and how often the check was asked.
fn run_batched_over<M: GuestMemory>(
    case: &Case,
    set: &Cpl0ProfileSet<'_>,
    state: &mut LongModeState,
    memory: &mut M,
    refuse_every: Option<u64>,
    label: &str,
) -> (Cpl0ProfileSetExit<()>, u64, u64, u64) {
    let budget = config(case).instruction_budget;
    let (mut retired, mut defined, mut asked) = (0, od_isa::INTEGER_FLAGS, 0u64);
    let exit = loop {
        let result = run_with_cpl0_profile_set_and_pio_batched(
            state,
            memory,
            RunConfig {
                instruction_budget: budget - retired,
                ..config(case)
            },
            defined,
            set,
            None,
            &mut NoPort,
            || false,
            &mut |_: &LongModeState, _: &mut M| {
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
    (exit, retired, defined, asked)
}

/// PERF-31 (ADR-0018, variant (a), prototype): the case through batched calls of the set with the
/// fast path and the block cache prototype over [`BlockMemory`] must end exactly like the
/// one-instruction calls of the set with the fast path (exit, count, definedness, state, memory),
/// whatever the boundary check refuses.
fn check_blocks(case: &Case, label: &str) {
    let fast = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_fast_path();
    let reference = one_instruction_reference(case, &fast, label);
    let blocks = fast.with_block_cache_prototype();
    for refuse_every in [None, Some(1), Some(3), Some(7)] {
        let mut state = admitted(&case.state);
        let mut memory = BlockMemory::new(case.memory.clone());
        let (exit, retired, defined, _) =
            run_batched_over(case, &blocks, &mut state, &mut memory, refuse_every, label);
        let variant = format!("{label}: blocks, refusing every {refuse_every:?}");
        assert_eq!(exit, reference.exit, "{variant}");
        assert_eq!(retired, reference.retired, "{variant}");
        assert_eq!(defined, reference.defined, "{variant}");
        assert_eq!(state, reference.state, "{variant}");
        assert_eq!(memory.inner, reference.memory, "{variant}");
        assert!(memory.retired_from_blocks <= retired, "{variant}");
        BLOCK_RETIRED.fetch_add(memory.retired_from_blocks, Ordering::Relaxed);
    }
    BLOCK_CASES.fetch_add(1, Ordering::Relaxed);
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
    // PERF-31: and through the batched entry with predecoded blocks; some instructions ran from them.
    println!(
        "{corpus}: block runs so far matched one-instruction calls on {} cases, {} instructions retired from blocks",
        BLOCK_CASES.load(Ordering::Relaxed),
        BLOCK_RETIRED.load(Ordering::Relaxed)
    );
    assert!(BLOCK_CASES.load(Ordering::Relaxed) > 0);
    assert!(BLOCK_RETIRED.load(Ordering::Relaxed) > 0);
}

static FAST_STEPS_TAKEN: AtomicU64 = AtomicU64::new(0);
static BATCHED_CASES: AtomicU64 = AtomicU64::new(0);
static BLOCK_CASES: AtomicU64 = AtomicU64::new(0);
static BLOCK_RETIRED: AtomicU64 = AtomicU64::new(0);
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

/// PERF-31 (ADR-0018, variant (a), prototype): self-modifying code inside one block. The first
/// instruction of the block (`MOV BYTE [RIP+1], 0x22`) patches the low byte of the immediate of the
/// next one (`MOV EAX, 0x1111`). The block was decoded before the store; the store raises the
/// generation of the page, the block no longer holds and the patched instruction is decoded again
/// on the general path, exactly as one-instruction calls run it. The mutation (writes do not raise
/// the generation) must run the stale decode and be seen.
#[test]
fn a_store_into_the_running_block_ends_it_and_the_new_bytes_run() {
    let mut program = vec![0xC6, 0x05, 0x01, 0x00, 0x00, 0x00, 0x22]; // MOV BYTE [RIP+1], 0x22
    program.extend([0xB8, 0x11, 0x11, 0x00, 0x00]); // MOV EAX, 0x1111
    program.extend([0x01, 0xC3]); // ADD EBX, EAX
    let stop_rip = 0x1000 + program.len() as u64;
    let case = Case {
        state: LongModeState {
            rip: 0x1000,
            rflags: 2,
            ..LongModeState::default()
        },
        memory: Memory {
            regions: vec![(0x1000, program)],
        },
        stop_rip,
    };
    let fast = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_fast_path();
    let reference = one_instruction_reference(&case, &fast, "smc");
    assert_eq!(
        reference.exit,
        Cpl0ProfileSetExit::Run(RunExit::StopAddress)
    );
    assert_eq!(reference.state.gpr[0], 0x1122);
    assert_eq!(reference.state.gpr[3], 0x1122);
    let blocks = fast.with_block_cache_prototype();
    let run = |ignore_writes: bool| {
        let mut state = admitted(&case.state);
        let mut memory = BlockMemory::new(case.memory.clone());
        memory.ignore_writes = ignore_writes;
        let (exit, retired, defined, _) =
            run_batched_over(&case, &blocks, &mut state, &mut memory, None, "smc");
        (exit, retired, defined, state, memory)
    };
    let (exit, retired, defined, state, memory) = run(false);
    assert_eq!(exit, reference.exit);
    assert_eq!((retired, defined), (reference.retired, reference.defined));
    assert_eq!(state, reference.state);
    assert_eq!(memory.inner, reference.memory);
    // The store ran from the block; the block then no longer held, and a new block was built
    // from the patched bytes (served from it: MOV EAX and ADD).
    assert_eq!(memory.holds_failed, 1);
    assert_eq!(memory.retired_from_blocks, 3);
    // The mutation: the stale `MOV EAX, 0x1111` runs from the block.
    let (_, _, _, stale, _) = run(true);
    assert_eq!(stale.gpr[0], 0x1111, "the mutation must be visible");
    assert_ne!(stale, reference.state);
}

/// PERF-33 (E5, step 3): the case of `a_store_into_the_running_block_ends_it_and_the_new_bytes_run`
/// through `check_blocks`: the batched calls with blocks match one-instruction calls whether the
/// boundary check lets every boundary pass or refuses every 1st, 3rd or 7th one.
#[test]
fn a_store_into_the_running_block_matches_at_every_refused_boundary() {
    let mut program = vec![0xC6, 0x05, 0x01, 0x00, 0x00, 0x00, 0x22]; // MOV BYTE [RIP+1], 0x22
    program.extend([0xB8, 0x11, 0x11, 0x00, 0x00]); // MOV EAX, 0x1111
    program.extend([0x01, 0xC3]); // ADD EBX, EAX
    let stop_rip = 0x1000 + program.len() as u64;
    let case = Case {
        state: LongModeState {
            rip: 0x1000,
            rflags: 2,
            ..LongModeState::default()
        },
        memory: Memory {
            regions: vec![(0x1000, program)],
        },
        stop_rip,
    };
    check_blocks(&case, "smc at every boundary");
}

/// PERF-33 (E5, step 3): an instruction over the end of a page. `MOV BYTE [RIP+9], 0x99` at 0x1FF0
/// writes the first byte of the next page, the last byte of the immediate of `MOV EDX, 0x44332211`
/// at 0x1FFC. The block of the first page stops before the crossing instruction (the page fence),
/// so the store, a write to the other page, does not touch the block, and the crossing instruction
/// is decoded on the general path with the new byte: `check_blocks` matches one-instruction calls at
/// every refused boundary. The mutation (no page fence) puts the crossing instruction into the
/// block, decoded before the store, and its stale immediate runs.
#[test]
fn a_block_stops_before_an_instruction_over_the_page_end_at_every_refused_boundary() {
    let mut program = vec![0xC6, 0x05, 0x09, 0x00, 0x00, 0x00, 0x99]; // 0x1FF0 MOV BYTE [RIP+9], 0x99
    program.extend([0xB8, 0x11, 0x00, 0x00, 0x00]); // 0x1FF7 MOV EAX, 0x11
    program.extend([0xBA, 0x11, 0x22, 0x33, 0x44]); // 0x1FFC MOV EDX, 0x44332211 (0x44 at 0x2000)
    program.extend([0x01, 0xD3]); // 0x2001 ADD EBX, EDX
    let stop_rip = 0x1FF0 + program.len() as u64;
    let case = Case {
        state: LongModeState {
            rip: 0x1FF0,
            rflags: 2,
            ..LongModeState::default()
        },
        memory: Memory {
            regions: vec![(0x1FF0, program)],
        },
        stop_rip,
    };
    check_blocks(&case, "page end");
    let fast = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_fast_path();
    let reference = one_instruction_reference(&case, &fast, "page end");
    assert_eq!(
        reference.exit,
        Cpl0ProfileSetExit::Run(RunExit::StopAddress)
    );
    assert_eq!(reference.state.gpr[2], 0x9933_2211);
    let blocks = fast.with_block_cache_prototype();
    let run = |ignore_page_fence: bool| {
        let mut state = admitted(&case.state);
        let mut memory = BlockMemory::new(case.memory.clone());
        memory.ignore_page_fence = ignore_page_fence;
        run_batched_over(&case, &blocks, &mut state, &mut memory, None, "page end");
        (state, memory)
    };
    let (state, memory) = run(false);
    assert_eq!(state, reference.state);
    // The store and MOV EAX ran from the block of the first page; ADD from the second page's.
    assert_eq!(memory.retired_from_blocks, 3);
    let (stale, _) = run(true);
    assert_eq!(stale.gpr[2], 0x4433_2211, "the mutation must be visible");
}
