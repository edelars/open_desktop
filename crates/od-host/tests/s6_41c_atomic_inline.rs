//! S6-41c: the inline memory emitter of atomic blocks (JIT_CONTRACT, S6-41c).
//!
//! The block of one atomic guest instruction translates its operand through the guest page tables
//! (`od_host::atomic_inline::locate_helper` over `od_machine::locate_paged_atomic_operand`), does
//! the host atomic operation on the host pointer of the RAM operand, raises the page generation
//! with a release and reports the committed write. Every test runs the interpreter (`od_isa`'s CPL0
//! profile set over the same adapter that the board uses) on a twin of the memory as the reference.
//!
//! Layout of the guest: 4-level tables at physical `0x1000..0x5fff`, code at linear `0x400000`
//! (frame `0x6000`), data at linear `0x800000` (page 0 frame `0x7000`, page 1 frame `0x8000`).
//!
//! Generated code is called through `PublishedCode` (one `MAP_JIT` region per process, so every
//! test takes `JIT_LOCK`).
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Barrier, Mutex, mpsc};

use od_host::atomic_inline::{
    AtomicInlineBlock, AtomicInlineEnv, AtomicInlineExit, AtomicInlineReport,
};
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
};
use od_isa::long_decode;
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, PageFaultRecord, PrivilegeLevel, StiShadow,
};
use od_jit::arm64::memory_order::MemoryOrderMode;
use od_jit::atomic_adapter::lower_atomic_instruction;
use od_jit::atomic_block::compile_atomic_block;
use od_jit::runtime::{AtomicSiteV4, RUNTIME_EXIT_V4_ATOMIC_DECLINED, RuntimeContextV4};
use od_machine::{
    ExecutionMode, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, RamLimits, with_paged_cpl0_set_memory,
};

static JIT_LOCK: Mutex<()> = Mutex::new(());

const CR0: u64 = 0x8001_0033;
const CR4: u64 = 1 << 5;
const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
const CR3: u64 = 0x1000;
const RAM_LEN: u64 = 0x2_0000;
const CODE_LINEAR: u64 = 0x0040_0000;
const CODE_FRAME: u64 = 0x6000;
const DATA_LINEAR: u64 = 0x0080_0000;
const DATA_FRAME: u64 = 0x7000;
const DATA_FRAME_1: u64 = 0x8000;
/// The operand of the differential: `DATA_LINEAR + OFFSET`, 16 bytes into the page at least.
const OFFSET: u64 = 0x40;
const GUARD_BEFORE: u64 = 8;
const GUARD_AFTER: u64 = 16;
const DEFINED_FLAGS: u64 = 0x8d5;
/// Leaf entries: present, writable, user; accessed (`0x20`); dirty (`0x40`).
const PTE_RW_ACCESSED: u64 = 0x7027;
const PTE_RO_ACCESSED: u64 = 0x7025;
const PTE_NOT_PRESENT: u64 = 0x7006;

fn ring0() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

fn paging() -> Long64ProbeConfig {
    Long64ProbeConfig::new(CR0, CR3, CR4, EFER).unwrap()
}

fn env() -> AtomicInlineEnv {
    AtomicInlineEnv {
        cr3: CR3,
        efer: EFER,
        access_context: ring0(),
    }
}

/// RAM with the tables above. `data_pte` is the leaf of data page 0 (frame `0x7000`), `data_pte_1`
/// that of page 1 (frame `0x8000`, the same rights bits with the frame replaced).
fn world(data_pte: u64, data_pte_1: u64, mode: ExecutionMode) -> PhysicalMemory {
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(
            GuestPhysicalRange::new(at(0), RAM_LEN).unwrap(),
            MemoryRegionKind::Ram,
        )],
    )
    .unwrap();
    let mut memory = PhysicalMemory::new(&map, RamLimits::new(RAM_LEN)).unwrap();
    memory.set_execution_mode(mode);
    // The leaf of page 1 keeps the flag bits of `data_pte_1` and points at frame 0x8000.
    let leaf_1 = (data_pte_1 & 0xfff) | DATA_FRAME_1;
    for (address, value) in [
        (0x1000, 0x2027),
        (0x2000, 0x3027),
        (0x3010, 0x4027),
        (0x3020, 0x5027),
        (0x4000, CODE_FRAME | 0x27),
        (0x5000, data_pte),
        (0x5008, leaf_1),
    ] {
        memory.store_u64(at(address), value, |_| {}).unwrap();
    }
    memory
}

fn put_code(memory: &PhysicalMemory, code: &[u8]) {
    memory.store_bytes(at(CODE_FRAME), code, |_| {}).unwrap();
}

fn data_frames(memory: &PhysicalMemory) -> Vec<u8> {
    let mut bytes = vec![0u8; 0x2000];
    memory.load_bytes(at(DATA_FRAME), &mut bytes).unwrap();
    bytes
}

fn tables(memory: &PhysicalMemory) -> Vec<u64> {
    [0x1000u64, 0x2000, 0x3010, 0x3020, 0x4000, 0x5000, 0x5008]
        .map(|address| memory.load_u64(at(address)).unwrap())
        .to_vec()
}

fn generation(memory: &PhysicalMemory, address: u64) -> u64 {
    memory.code_page_generation(at(address)).unwrap()
}

/// Fills the operand area (guards around it) and the operand itself.
fn put_operand(memory: &PhysicalMemory, linear_offset: u64, size: usize, value: u64) {
    let frame = if linear_offset >= 0x1000 {
        DATA_FRAME_1
    } else {
        DATA_FRAME
    };
    let physical = frame + (linear_offset & 0xfff);
    for offset in 0..GUARD_BEFORE + 8 + GUARD_AFTER {
        let byte = 0xa5 ^ (offset as u8);
        memory
            .store_u8(at(physical - GUARD_BEFORE + offset), byte, |_| {})
            .unwrap();
    }
    memory
        .store_bytes(at(physical), &value.to_le_bytes()[..size], |_| {})
        .unwrap();
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const EDGES: [u64; 10] = [
    0,
    1,
    0x7f,
    0x80,
    0xff,
    0x7fff_ffff,
    0x8000_0000,
    0xffff_ffff,
    0x7fff_ffff_ffff_ffff,
    u64::MAX,
];

fn state_for(rip: u64, flags_in: u64) -> LongModeState {
    LongModeState {
        rip,
        rflags: 2 | (1 << 9) | flags_in,
        cr0: Some(CR0),
        cr4: Some(CR4),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ds_selector: 0x18,
        es_selector: 0x18,
        ..LongModeState::default()
    }
}

// ---- guest instruction forms (absolute `[disp32]` through SIB) ----

const LOCK: &[u8] = &[0xf0];
const LOCK_W: &[u8] = &[0xf0, 0x48];
const REG_RCX: u8 = 1;
const REG_RBX: u8 = 3;

fn abs_form(prefix: &[u8], opcode: &[u8], reg: u8, suffix: &[u8], disp: u32) -> Vec<u8> {
    let mut code = prefix.to_vec();
    code.extend_from_slice(opcode);
    code.extend_from_slice(&[(reg << 3) | 0x04, 0x25]);
    code.extend_from_slice(&disp.to_le_bytes());
    code.extend_from_slice(suffix);
    code
}

/// The forms of the S6-41 and S6-41b differentials (widths 8, 32, 64; no m16, the decoder has
/// none): ADD, XADD, CMPXCHG, XCHG, INC, DEC, OR, AND, BTS, BTR.
fn forms(disp: u32) -> Vec<(String, Vec<u8>)> {
    let mut forms: Vec<(String, Vec<u8>)> = Vec::new();
    let mut add = |name: &str, code: Vec<u8>| forms.push((name.to_string(), code));
    add(
        "lock xadd m32, ebx",
        abs_form(LOCK, &[0x0f, 0xc1], REG_RBX, &[], disp),
    );
    add(
        "lock xadd m64, rcx",
        abs_form(LOCK_W, &[0x0f, 0xc1], REG_RCX, &[], disp),
    );
    add(
        "lock add m32, ebx",
        abs_form(LOCK, &[0x01], REG_RBX, &[], disp),
    );
    add(
        "lock add m64, rbx",
        abs_form(LOCK_W, &[0x01], REG_RBX, &[], disp),
    );
    add("lock inc m32", abs_form(LOCK, &[0xff], 0, &[], disp));
    add("lock dec m64", abs_form(LOCK_W, &[0xff], 1, &[], disp));
    add(
        "lock cmpxchg m32, ebx",
        abs_form(LOCK, &[0x0f, 0xb1], REG_RBX, &[], disp),
    );
    add(
        "lock cmpxchg m64, rcx",
        abs_form(LOCK_W, &[0x0f, 0xb1], REG_RCX, &[], disp),
    );
    add("xchg m32, ebx", abs_form(&[], &[0x87], REG_RBX, &[], disp));
    add(
        "xchg m64, rcx",
        abs_form(&[0x48], &[0x87], REG_RCX, &[], disp),
    );
    for imm in [0x01u8, 0xf5] {
        add(
            &format!("lock add m8, {imm:#x}"),
            abs_form(LOCK, &[0x80], 0, &[imm], disp),
        );
    }
    add(
        "lock add m64, imm8 sx 0xf0",
        abs_form(LOCK_W, &[0x83], 0, &[0xf0], disp),
    );
    for imm in [0x0fu8, 0x80] {
        add(
            &format!("lock or m8, {imm:#x}"),
            abs_form(LOCK, &[0x80], 1, &[imm], disp),
        );
    }
    add(
        "lock or m64, imm8 sx 0xff",
        abs_form(LOCK_W, &[0x83], 1, &[0xff], disp),
    );
    add(
        "lock and m8, 0xf0",
        abs_form(LOCK, &[0x80], 4, &[0xf0], disp),
    );
    for bit in [0u8, 5, 31, 33, 63] {
        add(
            &format!("lock bts m32, {bit}"),
            abs_form(LOCK, &[0x0f, 0xba], 5, &[bit], disp),
        );
        add(
            &format!("lock btr m64, {bit}"),
            abs_form(LOCK_W, &[0x0f, 0xba], 6, &[bit], disp),
        );
    }
    forms
}

struct CodeBytes(Vec<u8>);

impl GuestMemory for CodeBytes {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        self.0
            .get(address as usize)
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }
    fn read(&mut self, address: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })
    }
    fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })
    }
    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn adapted(code: &[u8]) -> od_jit::ir::Program {
    let mut memory = CodeBytes(code.to_vec());
    let decoded = long_decode::decode(&mut memory, 0, AccessContext::default())
        .unwrap_or_else(|error| panic!("{code:02x?} does not decode: {error:?}"));
    assert_eq!(decoded.length as usize, code.len(), "{code:02x?}");
    lower_atomic_instruction(&decoded, CODE_LINEAR)
        .unwrap_or_else(|error| panic!("{code:02x?}: {error:?}"))
}

fn operand_bytes(program: &od_jit::ir::Program) -> usize {
    program
        .operations
        .iter()
        .find_map(|operation| match operation {
            od_jit::ir::Operation::AtomicRmw { operand, .. } => Some(operand.size.bytes()),
            _ => None,
        })
        .unwrap_or(0)
}

/// What one run of either side leaves behind.
#[derive(Debug, PartialEq)]
struct Outcome {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
    data: Vec<u8>,
    tables: Vec<u64>,
    data_generation: u64,
    table_generation: u64,
    writes: Vec<(u64, u64)>,
}

fn outcome(
    state: &LongModeState,
    memory: &PhysicalMemory,
    writes: &[GuestPhysicalRange],
) -> Outcome {
    Outcome {
        gpr: state.gpr,
        rip: state.rip,
        rflags: state.rflags,
        data: data_frames(memory),
        tables: tables(memory),
        data_generation: generation(memory, DATA_FRAME),
        table_generation: generation(memory, 0x5000),
        writes: writes
            .iter()
            .map(|range| (range.start().get(), range.len()))
            .collect(),
    }
}

/// The interpreter's single step of the instruction at `state.rip`.
fn interpret(
    memory: &PhysicalMemory,
    state: &mut LongModeState,
) -> (
    Cpl0ProfileSetExit,
    u64,
    Vec<GuestPhysicalRange>,
    Option<PageFaultRecord>,
) {
    let mut writes = Vec::new();
    let mut note = |range| writes.push(range);
    let rip = state.rip;
    let (result, fault) = with_paged_cpl0_set_memory(memory, paging(), rip, &mut note, |adapter| {
        od_isa::execute::run_with_cpl0_profile_set(
            state,
            adapter,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: ring0(),
            },
            od_isa::INTEGER_FLAGS,
            &Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1),
            None,
            || false,
        )
        .unwrap()
    });
    (result.exit, result.instructions_executed, writes, fault)
}

fn jit_run(
    block: &AtomicInlineBlock,
    memory: &PhysicalMemory,
    state: &mut LongModeState,
) -> (AtomicInlineReport, Vec<GuestPhysicalRange>) {
    let mut writes = Vec::new();
    let mut note = |range| writes.push(range);
    let report = block.run(state, memory, env(), 1, None, &mut note).unwrap();
    (report, writes)
}

fn initial_state(rng: &mut Rng, round: usize) -> (LongModeState, u64, u64, u64) {
    let pick = |rng: &mut Rng| {
        if rng.next() % 3 == 0 {
            EDGES[(rng.next() % EDGES.len() as u64) as usize]
        } else {
            rng.next()
        }
    };
    let (initial, source) = if round < EDGES.len() {
        (EDGES[round], EDGES[(round * 7 + 3) % EDGES.len()])
    } else {
        (pick(rng), pick(rng))
    };
    let expected = if round % 2 == 0 { initial } else { pick(rng) };
    let flags_in = DEFINED_FLAGS & rng.next();
    let mut state = state_for(CODE_LINEAR, flags_in);
    state.gpr[0] = expected; // RAX
    state.gpr[1] = source ^ 0x0123_4567_89ab_cdef; // RCX
    state.gpr[2] = rng.next(); // RDX
    state.gpr[3] = source; // RBX
    (state, initial, source, expected)
}

const MODES: [(MemoryOrderMode, ExecutionMode); 2] = [
    (MemoryOrderMode::Interleaved, ExecutionMode::Interleaved),
    (MemoryOrderMode::Parallel, ExecutionMode::Parallel),
];

/// (в) Differential: the block of every form, in both modes, on a data page whose leaf has no
/// dirty bit (both sides set it), equals the interpreter in GPRs, RIP, RFLAGS, every data byte
/// (operand and guards), the page-table entries, the page generations and the sequence of
/// committed writes (the dirty-bit write of the walk, then the operand).
#[test]
fn s6_41c_inline_atomic_blocks_equal_the_interpreter_with_the_walk_and_the_generation() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let mut cases = 0usize;
    let mut forms_run = 0usize;
    for (name, code) in forms(DATA_LINEAR as u32 + OFFSET as u32) {
        let program = adapted(&code);
        let size = operand_bytes(&program);
        for (order, exec) in MODES {
            let block =
                AtomicInlineBlock::publish(compile_atomic_block(&program, order).unwrap()).unwrap();
            forms_run += 1;
            for round in 0..48usize {
                let (state, initial, _, _) = initial_state(&mut rng, round);
                let build = || {
                    let memory = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
                    put_code(&memory, &code);
                    put_operand(&memory, OFFSET, size, initial);
                    memory
                };
                let (reference_memory, jit_memory) = (build(), build());
                let before = (generation(&jit_memory, DATA_FRAME), tables(&jit_memory));

                let mut interpreted = state.clone();
                let (exit, retired, writes_i, fault_i) =
                    interpret(&reference_memory, &mut interpreted);
                assert_eq!(retired, 1, "{name}");
                assert!(
                    matches!(
                        exit,
                        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                    ),
                    "{name}: {exit:?}"
                );
                assert!(fault_i.is_none());

                let mut jitted = state.clone();
                let (report, writes_j) = jit_run(&block, &jit_memory, &mut jitted);
                let label = format!("{name} [{order:?}] round={round} initial={initial:#x}");
                assert_eq!(report.exit, AtomicInlineExit::Complete, "{label}");
                assert_eq!(report.retired, 1, "{label}");
                assert_eq!(report.helper_calls, 1, "{label}");
                assert_eq!(
                    report.committed,
                    Some(GuestPhysicalRange::new(at(DATA_FRAME + OFFSET), size as u64).unwrap()),
                    "{label}"
                );
                let expected = outcome(&interpreted, &reference_memory, &writes_i);
                let actual = outcome(&jitted, &jit_memory, &writes_j);
                assert_eq!(actual.gpr, expected.gpr, "GPRs: {label}");
                assert_eq!(actual.rip, expected.rip, "RIP: {label}");
                assert_eq!(
                    actual.rflags, expected.rflags,
                    "RFLAGS: {label} (jit {:#x} interpreter {:#x})",
                    actual.rflags, expected.rflags
                );
                assert_eq!(actual.data, expected.data, "memory: {label}");
                assert_eq!(actual.tables, expected.tables, "tables: {label}");
                assert_eq!(actual.writes, expected.writes, "committed writes: {label}");
                assert_eq!(
                    actual.data_generation, expected.data_generation,
                    "generation: {label}"
                );
                assert_eq!(
                    actual.table_generation, expected.table_generation,
                    "{label}"
                );
                // The generation of the data page grew by exactly one, the dirty bit was set.
                assert_eq!(actual.data_generation, before.0 + 1, "{label}");
                assert_ne!(
                    actual.tables, before.1,
                    "the walk set the dirty bit: {label}"
                );
                assert_eq!(actual.tables[5] & 0x60, 0x60, "{label}");
                cases += 1;
            }
        }
    }
    eprintln!(
        "s6_41c differential: {forms_run} (form, mode) blocks, {cases} cases; GPRs, RIP, RFLAGS, \
         data, tables, generations and committed writes equal the interpreter"
    );
    assert!(cases >= 40 * 2 * 48 / 2);
}

/// A fence block: no helper, no page effect; MFENCE in the parallel mode carries the barrier.
#[test]
fn s6_41c_a_fence_block_runs_without_the_helper_and_changes_only_rip_and_the_budget() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    for code in [[0x0f, 0xae, 0xf0], [0x0f, 0xae, 0xe8], [0x0f, 0xae, 0xf8]] {
        let program = adapted(&code);
        let block = AtomicInlineBlock::publish(
            compile_atomic_block(&program, MemoryOrderMode::Parallel).unwrap(),
        )
        .unwrap();
        let memory = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Parallel);
        put_code(&memory, &code);
        let mut state = state_for(CODE_LINEAR, 0x41);
        state.gpr[0] = 7;
        let before = state.clone();
        let tables_before = tables(&memory);
        let (report, writes) = jit_run(&block, &memory, &mut state);
        assert_eq!(report.exit, AtomicInlineExit::Complete);
        assert_eq!((report.helper_calls, report.retired), (0, 1));
        assert!(report.committed.is_none() && writes.is_empty());
        assert_eq!(state.rip, CODE_LINEAR + 3);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(tables(&memory), tables_before);
    }
}

fn run_fault_case(name: &str, code: &[u8], data_pte: u64, data_pte_1: u64, operand_at: u64) {
    let program = adapted(code);
    let size = operand_bytes(&program);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ operand_at);
    for (order, exec) in MODES {
        let block =
            AtomicInlineBlock::publish(compile_atomic_block(&program, order).unwrap()).unwrap();
        for round in 0..24usize {
            let (state, initial, _, _) = initial_state(&mut rng, round);
            let build = || {
                let memory = world(data_pte, data_pte_1, exec);
                put_code(&memory, code);
                put_operand(&memory, operand_at, size, initial);
                memory
            };
            let (reference_memory, jit_memory) = (build(), build());
            let data_before = data_frames(&jit_memory);
            let (generation_data, generation_tables) = (
                generation(&jit_memory, DATA_FRAME),
                generation(&jit_memory, 0x5000),
            );

            let mut interpreted = state.clone();
            let (_, retired_i, writes_i, fault_i) = interpret(&reference_memory, &mut interpreted);
            assert_eq!(retired_i, 0, "{name}: the interpreter faults too");
            let fault_i = fault_i.expect("the interpreter raised a #PF");

            let mut jitted = state.clone();
            let (report, writes_j) = jit_run(&block, &jit_memory, &mut jitted);
            let label = format!("{name} [{order:?}] round={round}");
            // (а) the fault leaves the CPU, RAM, RIP and the retirement untouched.
            assert_eq!(report.exit, AtomicInlineExit::Fault, "{label}");
            assert_eq!((report.retired, report.remaining_budget), (0, 1), "{label}");
            assert_eq!(report.helper_calls, 1, "{label}");
            assert!(report.committed.is_none(), "{label}");
            assert_eq!(jitted, state, "CPU state: {label}");
            assert_eq!(jitted.rip, CODE_LINEAR, "{label}");
            assert_eq!(data_frames(&jit_memory), data_before, "RAM: {label}");
            assert_eq!(
                generation(&jit_memory, DATA_FRAME),
                generation_data,
                "no generation raised: {label}"
            );
            // The walk's accessed-bit writes are the interpreter's (they may be none here).
            assert_eq!(
                generation(&jit_memory, 0x5000),
                generation(&reference_memory, 0x5000),
                "{label}"
            );
            assert_eq!(generation(&jit_memory, 0x5000), generation_tables);
            assert_eq!(tables(&jit_memory), tables(&reference_memory), "{label}");
            let writes_i: Vec<_> = writes_i
                .iter()
                .map(|r| (r.start().get(), r.len()))
                .collect();
            let writes_j: Vec<_> = writes_j
                .iter()
                .map(|r| (r.start().get(), r.len()))
                .collect();
            assert_eq!(writes_j, writes_i, "{label}");
            // The fault is the interpreter's #PF, with CR2 and the error code.
            let fault_j = report.page_fault.expect("a #PF record");
            assert_eq!(fault_j, fault_i, "{label}");
            assert_eq!(report.outcome.fault_vector, 14, "{label}");
            assert_eq!(report.outcome.has_error_code, 1, "{label}");
            assert_eq!(
                report.outcome.linear_address,
                DATA_LINEAR + operand_at,
                "CR2: {label}"
            );
        }
    }
}

/// (а) A fault before the atomic operation leaves state, RAM, RIP and the retirement untouched:
/// the read-only page (error code P|W, write to a present page) and the not-present page, for the
/// forms of every kind; the block exits at the helper, before the first effect.
#[test]
fn s6_41c_a_fault_before_the_atomic_operation_leaves_everything_untouched() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let disp = DATA_LINEAR as u32 + OFFSET as u32;
    for (name, code) in forms(disp) {
        run_fault_case(&name, &code, PTE_RO_ACCESSED, PTE_RW_ACCESSED, OFFSET);
    }
    // The not-present leaf: the read walk of the locked operation faults first.
    for (name, code) in forms(disp).into_iter().step_by(3) {
        run_fault_case(&name, &code, PTE_NOT_PRESENT, PTE_RW_ACCESSED, OFFSET);
    }
}

/// (г) `CMPXCHG` that does not match still takes the write walk: on a read-only page it is the
/// `#PF` of a write and no dirty bit is set; on a writable page whose leaf has no dirty bit the
/// dirty bit is set although the memory does not change; both as in the interpreter. The data
/// bytes are unchanged, and the generation rises (x86 writes the destination back: the interpreter
/// reports the write too).
#[test]
fn s6_41c_an_unequal_cmpxchg_takes_the_write_intent_like_the_interpreter() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let disp = DATA_LINEAR as u32 + OFFSET as u32;
    for (name, code) in [
        (
            "lock cmpxchg m32, ebx",
            abs_form(LOCK, &[0x0f, 0xb1], REG_RBX, &[], disp),
        ),
        (
            "lock cmpxchg m64, rcx",
            abs_form(LOCK_W, &[0x0f, 0xb1], REG_RCX, &[], disp),
        ),
    ] {
        let program = adapted(&code);
        let size = operand_bytes(&program);
        for (order, exec) in MODES {
            let block =
                AtomicInlineBlock::publish(compile_atomic_block(&program, order).unwrap()).unwrap();
            for (pte, faults) in [(PTE_RO_ACCESSED, true), (PTE_RW_ACCESSED, false)] {
                let build = || {
                    let memory = world(pte, PTE_RW_ACCESSED, exec);
                    put_code(&memory, &code);
                    put_operand(&memory, OFFSET, size, 0x1111_2222_3333_4444);
                    memory
                };
                let (reference_memory, jit_memory) = (build(), build());
                let mut state = state_for(CODE_LINEAR, 0);
                state.gpr[0] = 0x5555_6666_7777_8888; // RAX: never equal to the operand
                state.gpr[1] = 9;
                state.gpr[3] = 9;
                let data_before = data_frames(&jit_memory);
                let generation_before = generation(&jit_memory, DATA_FRAME);

                let mut interpreted = state.clone();
                let (_, retired_i, writes_i, fault_i) =
                    interpret(&reference_memory, &mut interpreted);
                let mut jitted = state.clone();
                let (report, writes_j) = jit_run(&block, &jit_memory, &mut jitted);
                let label = format!("{name} [{order:?}] read-only={faults}");

                assert_eq!(tables(&jit_memory), tables(&reference_memory), "{label}");
                assert_eq!(
                    data_frames(&jit_memory),
                    data_before,
                    "memory unchanged: {label}"
                );
                if faults {
                    assert_eq!(retired_i, 0, "{label}");
                    assert_eq!(report.exit, AtomicInlineExit::Fault, "{label}");
                    assert_eq!(jitted, state, "{label}");
                    assert_eq!(report.page_fault, fault_i, "{label}");
                    assert_eq!(report.outcome.error_code, 3, "P|W: {label}");
                    assert_eq!(
                        tables(&jit_memory)[5] & 0x40,
                        0,
                        "no dirty bit on the faulting write: {label}"
                    );
                    assert_eq!(
                        generation(&jit_memory, DATA_FRAME),
                        generation_before,
                        "{label}"
                    );
                } else {
                    assert_eq!(retired_i, 1, "{label}");
                    assert_eq!(report.exit, AtomicInlineExit::Complete, "{label}");
                    assert_eq!(
                        tables(&jit_memory)[5] & 0x40,
                        0x40,
                        "the dirty bit is set although the compare did not match: {label}"
                    );
                    assert_eq!(jitted.gpr, interpreted.gpr, "{label}");
                    assert_eq!(jitted.rflags, interpreted.rflags, "{label}");
                    // The interpreter reports the write of an unequal compare; so does the block.
                    assert_eq!(
                        generation(&jit_memory, DATA_FRAME),
                        generation(&reference_memory, DATA_FRAME),
                        "{label}"
                    );
                    assert_eq!(generation(&jit_memory, DATA_FRAME), generation_before + 1);
                    let writes_i: Vec<_> = writes_i
                        .iter()
                        .map(|r| (r.start().get(), r.len()))
                        .collect();
                    let writes_j: Vec<_> = writes_j
                        .iter()
                        .map(|r| (r.start().get(), r.len()))
                        .collect();
                    assert_eq!(writes_j, writes_i, "{label}");
                }
            }
        }
    }
}

/// (д) A misaligned operand (inside a page, and across a page boundary) is declined by the helper
/// before any walk: no effect, no accessed or dirty bit, no split-lock refusal counted. The
/// interpreter step that follows (which owns the split `LOCK`) leaves the same final state as the
/// interpreter alone.
#[test]
fn s6_41c_a_misaligned_or_split_operand_is_declined_and_the_interpreter_owns_it() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut cases = 0;
    // (operand offset in the data pages, form index in `forms` order of size)
    for (operand_at, label_at) in [
        (OFFSET + 2, "inside the page, 2 mod 4"),
        (OFFSET + 4, "inside the page, 4 mod 8"),
        (0xffe, "across the page boundary"),
        (0xffd, "across the page boundary, odd"),
        (OFFSET + 1, "odd byte offset"),
    ] {
        let disp = (DATA_LINEAR + operand_at) as u32;
        for (name, code) in forms(disp) {
            let program = adapted(&code);
            let size = operand_bytes(&program);
            if size == 1 || operand_at % size as u64 == 0 {
                continue; // aligned for this width (a byte never is misaligned)
            }
            for (order, exec) in MODES {
                let block =
                    AtomicInlineBlock::publish(compile_atomic_block(&program, order).unwrap())
                        .unwrap();
                let mut rng = Rng(0x1234_5678_9abc_def1 ^ operand_at);
                let (state, initial, _, _) = initial_state(&mut rng, 3);
                let build = || {
                    let memory = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
                    // The leaves have no dirty bit: a walk would set it.
                    put_code(&memory, &code);
                    put_operand(&memory, operand_at.min(0xff0), size, initial);
                    memory
                };
                let (reference_memory, jit_memory) = (build(), build());
                let tables_before = tables(&jit_memory);
                let data_before = data_frames(&jit_memory);

                let mut jitted = state.clone();
                let (report, writes) = jit_run(&block, &jit_memory, &mut jitted);
                let label = format!("{name} {label_at} [{order:?}]");
                assert_eq!(report.exit, AtomicInlineExit::Declined, "{label}");
                assert_eq!(report.retired, 0, "{label}");
                assert!(writes.is_empty() && report.committed.is_none(), "{label}");
                assert_eq!(jitted, state, "{label}");
                assert_eq!(tables(&jit_memory), tables_before, "no walk: {label}");
                assert_eq!(data_frames(&jit_memory), data_before, "{label}");
                assert_eq!(jit_memory.split_lock_refusals(), 0, "not counted: {label}");
                assert_eq!(
                    generation(&jit_memory, DATA_FRAME),
                    generation(&reference_memory, DATA_FRAME),
                    "no generation raised: {label}"
                );

                // The interpreter performs it, on the memory the JIT left, as on a fresh one.
                let mut after_jit = jitted.clone();
                let (exit_j, retired_j, _, _) = interpret(&jit_memory, &mut after_jit);
                let mut alone = state.clone();
                let (exit_i, retired_i, _, _) = interpret(&reference_memory, &mut alone);
                assert_eq!(retired_j, retired_i, "{label}");
                assert_eq!(format!("{exit_j:?}"), format!("{exit_i:?}"), "{label}");
                assert_eq!(after_jit, alone, "{label}");
                assert_eq!(
                    data_frames(&jit_memory),
                    data_frames(&reference_memory),
                    "{label}"
                );
                assert_eq!(tables(&jit_memory), tables(&reference_memory), "{label}");
                cases += 1;
            }
        }
    }
    assert!(cases > 100, "{cases} cases");
}

/// Budget zero and a raised STOP byte leave before the helper: nothing is walked or changed.
#[test]
fn s6_41c_the_guards_leave_before_the_helper_with_no_effect() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let disp = DATA_LINEAR as u32 + OFFSET as u32;
    let code = abs_form(LOCK, &[0x0f, 0xc1], REG_RBX, &[], disp);
    let program = adapted(&code);
    let block = AtomicInlineBlock::publish(
        compile_atomic_block(&program, MemoryOrderMode::Parallel).unwrap(),
    )
    .unwrap();
    let memory = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Parallel);
    put_code(&memory, &code);
    put_operand(&memory, OFFSET, 4, 5);
    let state = state_for(CODE_LINEAR, 0);
    let tables_before = tables(&memory);

    let run = |budget: u64, stop: Option<&AtomicU8>| {
        let mut jitted = state.clone();
        let mut writes = Vec::new();
        let mut note = |range| writes.push(range);
        let report = block
            .run(&mut jitted, &memory, env(), budget, stop, &mut note)
            .unwrap();
        assert_eq!(jitted, state);
        assert!(writes.is_empty());
        (
            report.exit,
            report.helper_calls,
            report.retired,
            report.remaining_budget,
        )
    };
    assert_eq!(run(0, None), (AtomicInlineExit::BudgetExhausted, 0, 0, 0));
    let raised = AtomicU8::new(1);
    assert_eq!(
        run(5, Some(&raised)),
        (AtomicInlineExit::StopRequested, 0, 0, 5)
    );
    // A clear byte and a larger budget run the instruction; the budget is consumed by one.
    let clear = AtomicU8::new(0);
    let mut jitted = state.clone();
    let mut note = |_| {};
    let report = block
        .run(&mut jitted, &memory, env(), 5, Some(&clear), &mut note)
        .unwrap();
    assert_eq!(
        (report.exit, report.retired, report.remaining_budget),
        (AtomicInlineExit::Complete, 1, 4)
    );
    assert_ne!(tables(&memory), tables_before);
}

/// The helper may return two page counters (the array of S6-41c): the block raises both, each
/// once, and a count of one raises only the first. A synthetic helper stands in for the walk.
#[test]
fn s6_41c_the_block_raises_every_generation_the_helper_returns() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    static COUNTERS: [AtomicU64; 3] = [AtomicU64::new(10), AtomicU64::new(20), AtomicU64::new(30)];
    static CELL: AtomicU64 = AtomicU64::new(0);
    static COUNT: AtomicU64 = AtomicU64::new(1);

    unsafe extern "C" fn two_counter_helper(context: *mut RuntimeContextV4) -> u32 {
        // SAFETY: the block passes its own live context.
        let context = unsafe { &mut *context };
        context.operand_host = CELL.as_ptr().cast::<u8>();
        context.generation = [
            COUNTERS[0].as_ptr().cast_const().cast(),
            COUNTERS[1].as_ptr().cast_const().cast(),
        ];
        context.generation_count = COUNT.load(Ordering::Relaxed) as u32;
        context.committed_start = 0x7040;
        context.committed_len = 8;
        0
    }

    let code = abs_form(LOCK_W, &[0x0f, 0xc1], REG_RCX, &[], 0x1000);
    let program = adapted(&code);
    let block = AtomicInlineBlock::publish(
        compile_atomic_block(&program, MemoryOrderMode::Parallel).unwrap(),
    )
    .unwrap();
    for (count, expected) in [(1u64, [11u64, 20]), (2, [11, 21])] {
        COUNT.store(count, Ordering::Relaxed);
        CELL.store(100, Ordering::Relaxed);
        let mut state = state_for(CODE_LINEAR, 0);
        state.gpr[1] = 5;
        let mut context = block.new_context(&mut state, 1, None);
        context.locate_helper = Some(two_counter_helper);
        // SAFETY: the helper honours the V4 contract (it fills the outputs and returns 0); the
        // state is live; no stop byte.
        let status = unsafe { block.call_raw(&mut context) };
        assert_eq!(status, 0);
        assert_eq!(CELL.load(Ordering::Relaxed), 105);
        assert_eq!(
            state.gpr[1], 100,
            "XADD returns the old value in the source register"
        );
        assert_eq!(context.committed_valid, 1);
        assert_eq!(
            [
                COUNTERS[0].load(Ordering::Relaxed),
                COUNTERS[1].load(Ordering::Relaxed)
            ],
            expected
        );
        assert_eq!(
            COUNTERS[2].load(Ordering::Relaxed),
            30,
            "a third counter is never touched"
        );
        COUNTERS[0].store(10, Ordering::Relaxed);
        COUNTERS[1].store(20, Ordering::Relaxed);
    }
}

/// A helper that returns a V4 exit value other than success makes the block leave at once with
/// that value, before the atomic operation (the contract of the helper status).
#[test]
fn s6_41c_a_declining_helper_status_is_returned_before_the_atomic_operation() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    static CELL: AtomicU64 = AtomicU64::new(7);
    unsafe extern "C" fn decline(context: *mut RuntimeContextV4) -> u32 {
        // SAFETY: the block passes its own live context.
        let context = unsafe { &mut *context };
        // A pointer that must never be used: the block must not reach the atomic operation.
        context.operand_host = CELL.as_ptr().cast::<u8>();
        RUNTIME_EXIT_V4_ATOMIC_DECLINED
    }
    let code = abs_form(LOCK_W, &[0x0f, 0xc1], REG_RCX, &[], 0x1000);
    let program = adapted(&code);
    let block = AtomicInlineBlock::publish(
        compile_atomic_block(&program, MemoryOrderMode::Parallel).unwrap(),
    )
    .unwrap();
    let mut state = state_for(CODE_LINEAR, 0);
    state.gpr[1] = 5;
    let before = state.clone();
    let mut context = block.new_context(&mut state, 1, None);
    context.locate_helper = Some(decline);
    // SAFETY: as in the test above.
    let status = unsafe { block.call_raw(&mut context) };
    assert_eq!(status, RUNTIME_EXIT_V4_ATOMIC_DECLINED);
    assert_eq!(context.base.retired_instructions, 0);
    assert_eq!(context.base.remaining_budget, 1);
    assert_eq!(context.committed_valid, 0);
    assert_eq!(CELL.load(Ordering::Relaxed), 7);
    assert_eq!(state, before);
    let _: Option<&AtomicSiteV4> = block.block().sites().first();
}

/// (б) Another vCPU (host thread) sees the generation rise after the atomic write, and the
/// committed-write report. Thread A runs the block (`LOCK XADD m64` adding one) round after
/// round on one cell; thread B, per round: spins on the page counter until it reaches the round's
/// value (an `Acquire` load), then reads the cell, which must already hold the round's sum (the
/// data comes before the `Release` raise), then receives the report (sent by the `committed_write`
/// callback of thread A, which runs after the raise: the counter read inside the callback is
/// already the round's value, and the report's range is the operand).
#[test]
fn s6_41c_another_vcpu_sees_the_generation_and_the_committed_write_after_the_atomic_write() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    const ROUNDS: u64 = 3000;
    let code = abs_form(
        LOCK_W,
        &[0x0f, 0xc1],
        REG_RCX,
        &[],
        DATA_LINEAR as u32 + OFFSET as u32,
    );
    let program = adapted(&code);
    let memory = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Parallel);
    put_code(&memory, &code);
    put_operand(&memory, OFFSET, 8, 0);
    let start = Barrier::new(2);
    let (sender, receiver) = mpsc::channel::<(u64, u64, u64)>();
    let operand = DATA_FRAME + OFFSET;
    let initial_generation = generation(&memory, operand);
    // The page-table dirty-bit write of the first walk is a committed write too (generation of
    // the table page); the operand's page counter moves only with the operand.
    std::thread::scope(|scope| {
        // `sender` moves into thread A: if A panics, B's `recv` fails instead of waiting forever.
        let (program, memory, start) = (&program, &memory, &start);
        scope.spawn(move || {
            let block = AtomicInlineBlock::publish(
                compile_atomic_block(&program, MemoryOrderMode::Parallel).unwrap(),
            )
            .unwrap();
            let mut state = state_for(CODE_LINEAR, 0);
            start.wait();
            for round in 1..=ROUNDS {
                state.rip = CODE_LINEAR;
                state.gpr[1] = 1; // RCX: the addend; XADD leaves the old value there
                let mut note = |range: GuestPhysicalRange| {
                    if range.start().get() == operand {
                        // After the raise: the counter already shows this round.
                        let seen = memory.code_page_generation(range.start()).unwrap();
                        sender.send((round, range.start().get(), seen)).unwrap();
                    }
                };
                let report = block
                    .run(&mut state, &memory, env(), 1, None, &mut note)
                    .unwrap();
                assert_eq!(report.exit, AtomicInlineExit::Complete);
                assert_eq!(state.gpr[1], round - 1);
            }
        });
        scope.spawn(move || {
            start.wait();
            for round in 1..=ROUNDS {
                // Wait for this round's raise; the data must be visible with it.
                let target = initial_generation + round;
                let mut spins = 0u64;
                while generation(&memory, operand) < target {
                    spins += 1;
                    std::hint::spin_loop();
                    assert!(spins < 2_000_000_000, "round {round}: no generation raise");
                }
                let value = memory.load_u64(at(operand)).unwrap();
                assert!(
                    value >= round,
                    "round {round}: generation {target} seen but the cell holds {value}"
                );
                let (reported_round, start_address, seen) = receiver.recv().unwrap();
                assert_eq!(reported_round, round);
                assert_eq!(start_address, operand);
                assert!(
                    seen >= target,
                    "round {round}: the report preceded the generation raise ({seen} < {target})"
                );
            }
        });
    });
    assert_eq!(memory.load_u64(at(operand)).unwrap(), ROUNDS);
    assert_eq!(generation(&memory, operand), initial_generation + ROUNDS);
}
