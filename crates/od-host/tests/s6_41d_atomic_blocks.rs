//! S6-41d: atomic guest instructions admitted into blocks (JIT_CONTRACT, S6-41d).
//!
//! S6-41c ran a block of ONE atomic instruction; S6-41b compared the arm64 SEQUENCES with the
//! interpreter. This file repeats the differential through BLOCKS of two and three instructions
//! (`LOCK` forms, `XCHG`, fences, in any order, operands on one page and across two pages), the
//! fault paths between the instructions of a block, and the admission of such blocks into
//! `PagedFetchedLoadSession` (`execute_atomic`): the key by the memory-ordering mode, the host
//! policy, `CodeInvalidated`, and the neighbourhood of ordinary instructions.
//!
//! The reference is `od_isa`'s CPL0 profile set over the same adapter that the board's
//! interpreter uses, on a twin of the memory. Layout of the guest: 4-level tables at physical
//! `0x1000..0x5fff`, code at linear `0x400000` (frame `0x6000`), data at linear `0x800000` (page 0
//! frame `0x7000`, page 1 frame `0x8000`).
//!
//! Generated code is called through `PublishedCode` (one `MAP_JIT` region per process, so every
//! test takes `JIT_LOCK`).
#![cfg(all(target_os = "macos", target_arch = "aarch64"))]

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use od_host::atomic_inline::{AtomicInlineBlock, AtomicInlineEnv, AtomicInlineExit};
use od_host::{
    PAGED_ATOMIC_FETCH_WINDOW, PagedFetchAdmission, PagedFetchedLoadErrorKind,
    PagedFetchedLoadSession,
};
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, RunConfig, RunExit,
};
use od_isa::long_decode::{self, Instruction};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, CpuActivity, GuestMemory, InstructionMode,
    LongModeState, MemoryError, PageFaultRecord, PrivilegeLevel, StiShadow,
};
use od_jit::arm64::memory_order::{MemoryOrderMode, mode_admitted_on_host};
use od_jit::atomic_adapter::{AtomicAdapterError, lower_atomic_sequence};
use od_jit::atomic_block::{MAX_ATOMIC_BLOCK_INSTRUCTIONS, compile_atomic_block};
use od_machine::{
    AccessWidth, ExecutionMode, GuestPhysicalAddress, GuestPhysicalRange, Long64ProbeConfig,
    MachineBus, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory, RamLimits,
    with_paged_cpl0_set_memory,
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
const GUARD_BEFORE: u64 = 8;
const GUARD_AFTER: u64 = 16;
const DEFINED_FLAGS: u64 = 0x8d5;
/// Leaf entries: present, writable, user; accessed (`0x20`); dirty (`0x40`).
const PTE_RW_ACCESSED: u64 = 0x7027;
const PTE_RO_ACCESSED: u64 = 0x7025;
const PTE_RO_UNACCESSED: u64 = 0x7005;
const PTE_NOT_PRESENT: u64 = 0x7006;
/// Operand slots (offsets from `DATA_LINEAR`): three on page 0, one on page 1.
const PAGE0_SLOTS: [u64; 3] = [0x40, 0x80, 0x100];
const PAGE1_SLOT: u64 = 0x1040;

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

/// A machine bus with the tables above. `data_pte` is the leaf of data page 0 (frame `0x7000`
/// unless the caller aliases another frame), `data_pte_1` the flag bits of page 1 (frame
/// `0x8000`).
fn world(data_pte: u64, data_pte_1: u64, mode: ExecutionMode) -> MachineBus {
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(
            GuestPhysicalRange::new(at(0), RAM_LEN).unwrap(),
            MemoryRegionKind::Ram,
        )],
    )
    .unwrap();
    let mut bus = MachineBus::new(map, RamLimits::new(RAM_LEN), vec![], vec![], vec![]).unwrap();
    bus.set_execution_mode(mode);
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
        bus.write_physical(at(address), AccessWidth::Qword, value, |_| {})
            .unwrap();
    }
    bus
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

/// Fills the operand area (guards around it) and the operand itself, at `linear_offset` from
/// `DATA_LINEAR`.
fn put_operand(memory: &PhysicalMemory, linear_offset: u64, value: u64) {
    let frame = if linear_offset >= 0x1000 {
        DATA_FRAME_1
    } else {
        DATA_FRAME
    };
    let physical = frame + (linear_offset & 0xfff);
    for offset in 0..GUARD_BEFORE + 8 + GUARD_AFTER {
        let byte = 0xa5 ^ (offset as u8) ^ (linear_offset as u8);
        memory
            .store_u8(at(physical - GUARD_BEFORE + offset), byte, |_| {})
            .unwrap();
    }
    memory
        .store_bytes(at(physical), &value.to_le_bytes(), |_| {})
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

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
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

fn random_state(rng: &mut Rng, round: usize) -> LongModeState {
    let pick = |rng: &mut Rng| {
        if rng.next() % 3 == 0 {
            EDGES[rng.below(EDGES.len())]
        } else {
            rng.next()
        }
    };
    let (initial, source) = if round < EDGES.len() {
        (EDGES[round], EDGES[(round * 7 + 3) % EDGES.len()])
    } else {
        (pick(rng), pick(rng))
    };
    let mut state = state_for(CODE_LINEAR, DEFINED_FLAGS & rng.next());
    state.gpr[0] = if round % 2 == 0 { initial } else { pick(rng) }; // RAX
    state.gpr[1] = source ^ 0x0123_4567_89ab_cdef; // RCX
    state.gpr[2] = rng.next(); // RDX
    state.gpr[3] = source; // RBX
    state
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

/// The forms of the S6-41 and S6-41b differentials, widths 8, 32 and 64 (the decoder has no
/// m16): ADD, XADD, CMPXCHG, XCHG, INC, DEC, OR, AND, BTS, BTR.
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
    add(
        "lock add m8, 0xf5",
        abs_form(LOCK, &[0x80], 0, &[0xf5], disp),
    );
    add(
        "lock add m64, imm8 sx 0xf0",
        abs_form(LOCK_W, &[0x83], 0, &[0xf0], disp),
    );
    add(
        "lock or m8, 0x0f",
        abs_form(LOCK, &[0x80], 1, &[0x0f], disp),
    );
    add(
        "lock or m64, imm8 sx 0xff",
        abs_form(LOCK_W, &[0x83], 1, &[0xff], disp),
    );
    add(
        "lock and m8, 0xf0",
        abs_form(LOCK, &[0x80], 4, &[0xf0], disp),
    );
    add(
        "lock bts m32, 31",
        abs_form(LOCK, &[0x0f, 0xba], 5, &[31], disp),
    );
    add(
        "lock btr m64, 33",
        abs_form(LOCK_W, &[0x0f, 0xba], 6, &[33], disp),
    );
    forms
}

const FENCES: [[u8; 3]; 3] = [[0x0f, 0xae, 0xf0], [0x0f, 0xae, 0xe8], [0x0f, 0xae, 0xf8]];

/// One instruction of a generated sequence.
#[derive(Clone)]
struct Item {
    name: String,
    code: Vec<u8>,
    /// The operand slot (offset from `DATA_LINEAR`), `None` for a fence.
    slot: Option<u64>,
}

fn pick_item(rng: &mut Rng, slots: &[u64], allow_fence: bool) -> Item {
    if allow_fence && rng.below(6) == 0 {
        return Item {
            name: "fence".to_string(),
            code: FENCES[rng.below(3)].to_vec(),
            slot: None,
        };
    }
    let slot = slots[rng.below(slots.len())];
    let mut all = forms((DATA_LINEAR + slot) as u32);
    let (name, code) = all.swap_remove(rng.below(all.len()));
    Item {
        name,
        code,
        slot: Some(slot),
    }
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

/// Decodes `items` one after another (each at its own place of the concatenated code).
fn decode_items(items: &[Item]) -> Vec<Instruction> {
    let code: Vec<u8> = items.iter().flat_map(|item| item.code.clone()).collect();
    let mut memory = CodeBytes(code);
    let mut offset = 0u64;
    let mut decoded = Vec::new();
    for item in items {
        let instruction = long_decode::decode(&mut memory, offset, AccessContext::default())
            .unwrap_or_else(|error| panic!("{} does not decode: {error:?}", item.name));
        assert_eq!(
            instruction.length as usize,
            item.code.len(),
            "{}",
            item.name
        );
        offset += u64::from(instruction.length);
        decoded.push(instruction);
    }
    decoded
}

fn block_of(items: &[Item], order: MemoryOrderMode) -> AtomicInlineBlock {
    let decoded = decode_items(items);
    let refs: Vec<&Instruction> = decoded.iter().collect();
    let program = lower_atomic_sequence(&refs, CODE_LINEAR)
        .unwrap_or_else(|error| panic!("{:?}: {error:?}", names(items)));
    AtomicInlineBlock::publish(compile_atomic_block(&program, order).unwrap()).unwrap()
}

fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(|item| item.name.as_str()).collect()
}

fn code_of(items: &[Item]) -> Vec<u8> {
    items.iter().flat_map(|item| item.code.clone()).collect()
}

fn rip_of(items: &[Item], index: usize) -> u64 {
    CODE_LINEAR
        + items[..index]
            .iter()
            .map(|item| item.code.len() as u64)
            .sum::<u64>()
}

/// What one run of either side leaves behind.
#[derive(Debug, PartialEq)]
struct Outcome {
    gpr: [u64; 16],
    rip: u64,
    rflags: u64,
    data: Vec<u8>,
    tables: Vec<u64>,
    generations: [u64; 3],
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
        generations: [
            generation(memory, DATA_FRAME),
            generation(memory, DATA_FRAME_1),
            generation(memory, 0x5000),
        ],
        writes: writes
            .iter()
            .map(|range| (range.start().get(), range.len()))
            .collect(),
    }
}

/// The interpreter's run of up to `budget` instructions from `state.rip`, one step at a time with
/// an adapter of its own for every instruction (as the board runs it: the saved RIP of a `#PF`
/// is the RIP of the faulting instruction, not that of the first one of the run). Stops at the
/// first instruction that does not retire. Returns the exit of the last step, the number retired,
/// every committed write in order and the page fault of the stopping step, if any.
fn interpret(
    memory: &PhysicalMemory,
    state: &mut LongModeState,
    budget: u64,
) -> (
    Cpl0ProfileSetExit,
    u64,
    Vec<GuestPhysicalRange>,
    Option<PageFaultRecord>,
) {
    let mut writes = Vec::new();
    let mut retired = 0;
    let mut last = None;
    for _ in 0..budget {
        let mut note = |range| writes.push(range);
        let rip = state.rip;
        let (result, fault) =
            with_paged_cpl0_set_memory(memory, paging(), rip, &mut note, |adapter| {
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
        retired += result.instructions_executed;
        let stopped = result.instructions_executed == 0 || fault.is_some();
        if stopped {
            return (result.exit, retired, writes, fault);
        }
        last = Some(result.exit);
    }
    (
        last.expect("a budget of at least one instruction"),
        retired,
        writes,
        None,
    )
}

const MODES: [(MemoryOrderMode, ExecutionMode); 2] = [
    (MemoryOrderMode::Interleaved, ExecutionMode::Interleaved),
    (MemoryOrderMode::Parallel, ExecutionMode::Parallel),
];

fn sorted(mut writes: Vec<(u64, u64)>) -> Vec<(u64, u64)> {
    writes.sort_unstable();
    writes
}

/// (7) The differential through BLOCKS of two and three instructions: LOCK forms, XCHG and fences
/// in any order, operands on page 0 only (the order of the committed writes is then the
/// interpreter's exactly) and on both pages (the same set: the block reports the writes of its
/// operands after it returned, so a later instruction's page-table write comes before the
/// report of an earlier operand, which the interpreter interleaves). Both modes. Every observable
/// is compared with the interpreter on a twin: GPRs, RIP, RFLAGS, both data pages (operands and
/// guards), the table entries, the generations, the committed writes.
#[test]
fn s6_41d_blocks_of_two_and_three_atomic_instructions_equal_the_interpreter() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let (mut sequences, mut cases, mut memory_instructions, mut same_slot) = (0, 0, 0, 0);
    let mut seen_forms = std::collections::BTreeSet::new();
    for sequence in 0..72usize {
        let count = 2 + sequence % 2;
        let two_pages = sequence % 3 == 2;
        let slots: Vec<u64> = if two_pages {
            PAGE0_SLOTS.iter().copied().chain([PAGE1_SLOT]).collect()
        } else {
            PAGE0_SLOTS.to_vec()
        };
        let items: Vec<Item> = (0..count)
            .map(|_| pick_item(&mut rng, &slots, true))
            .collect();
        if items.iter().all(|item| item.slot.is_none()) {
            continue;
        }
        let code = code_of(&items);
        for (order, exec) in MODES {
            let block = block_of(&items, order);
            assert_eq!(block.block().instruction_count(), count);
            for round in 0..10usize {
                let state = random_state(&mut rng, round);
                let initials: Vec<u64> = slots.iter().map(|_| rng.next()).collect();
                let build = || {
                    let bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
                    put_code(bus.atomic_inline_memory(), &code);
                    for (slot, initial) in slots.iter().zip(&initials) {
                        put_operand(bus.atomic_inline_memory(), *slot, *initial);
                    }
                    bus
                };
                let (reference, subject) = (build(), build());
                let (reference_memory, memory) = (
                    reference.atomic_inline_memory(),
                    subject.atomic_inline_memory(),
                );

                let mut interpreted = state.clone();
                let (exit, retired, writes_i, fault_i) =
                    interpret(reference_memory, &mut interpreted, count as u64);
                assert_eq!(retired, count as u64, "{:?}", names(&items));
                assert!(
                    matches!(
                        exit,
                        Cpl0ProfileSetExit::Run(RunExit::InstructionBudgetExhausted)
                    ),
                    "{:?}: {exit:?}",
                    names(&items)
                );
                assert!(fault_i.is_none());

                let mut jitted = state.clone();
                let mut writes_j = Vec::new();
                let report = block
                    .run(
                        &mut jitted,
                        memory,
                        env(),
                        count as u64,
                        None,
                        &mut |range| writes_j.push(range),
                    )
                    .unwrap();
                let label = format!("{:?} [{order:?}] round={round}", names(&items));
                let memory_count = items.iter().filter(|item| item.slot.is_some()).count();
                assert_eq!(report.exit, AtomicInlineExit::Complete, "{label}");
                assert_eq!(report.retired, count as u64, "{label}");
                assert_eq!(report.remaining_budget, 0, "{label}");
                assert_eq!(report.helper_calls as usize, memory_count, "{label}");
                assert_eq!(report.committed_writes.len(), memory_count, "{label}");

                let expected = outcome(&interpreted, reference_memory, &writes_i);
                let actual = outcome(&jitted, memory, &writes_j);
                assert_eq!(actual.gpr, expected.gpr, "GPRs: {label}");
                assert_eq!(actual.rip, expected.rip, "RIP: {label}");
                assert_eq!(actual.rip, CODE_LINEAR + code.len() as u64, "{label}");
                assert_eq!(
                    actual.rflags, expected.rflags,
                    "RFLAGS: {label} (jit {:#x} interpreter {:#x})",
                    actual.rflags, expected.rflags
                );
                assert_eq!(actual.data, expected.data, "memory: {label}");
                assert_eq!(actual.tables, expected.tables, "tables: {label}");
                assert_eq!(
                    actual.generations, expected.generations,
                    "generations: {label}"
                );
                if two_pages {
                    assert_eq!(
                        sorted(actual.writes.clone()),
                        sorted(expected.writes.clone()),
                        "committed writes (as a set): {label}"
                    );
                } else {
                    assert_eq!(actual.writes, expected.writes, "committed writes: {label}");
                }
                cases += 1;
            }
            memory_instructions += items.iter().filter(|item| item.slot.is_some()).count();
        }
        let slots_used: Vec<u64> = items.iter().filter_map(|item| item.slot).collect();
        if slots_used
            .iter()
            .enumerate()
            .any(|(i, slot)| slots_used[..i].contains(slot))
        {
            same_slot += 1;
        }
        for item in &items {
            seen_forms.insert(item.name.clone());
        }
        sequences += 1;
    }
    eprintln!(
        "s6_41d block differential: {sequences} sequences ({same_slot} with two instructions on \
         one operand, {} distinct forms), {cases} cases, {memory_instructions} memory \
         instructions; GPRs, RIP, RFLAGS, data, tables, generations and committed writes equal \
         the interpreter",
        seen_forms.len()
    );
    assert!(sequences >= 40 && cases >= 800);
    assert!(
        same_slot >= 5,
        "dependent instructions on one operand are covered"
    );
    assert!(seen_forms.len() >= 12);
}

/// (7) A fault between the instructions of a block: the page of the faulting instruction is
/// read-only (`#PF` P|W) or not present; the instructions before it have retired in full (data,
/// flags, generation, RIP). The faulting operation may set Accessed on its read-only leaf.
/// The state, RAM, tables, generations, committed writes and fault record match the interpreter.
#[test]
fn s6_41d_a_fault_between_the_instructions_of_a_block_leaves_the_earlier_ones_retired() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let (mut cases, mut late_faults) = (0, 0);
    for sequence in 0..48usize {
        let count = 2 + sequence % 2;
        let fault_at = rng.below(count);
        let pte_1 = match sequence % 3 {
            0 => PTE_RO_ACCESSED,
            1 => PTE_NOT_PRESENT,
            _ => PTE_RO_UNACCESSED,
        };
        let mut items: Vec<Item> = (0..count)
            .map(|_| pick_item(&mut rng, &PAGE0_SLOTS, true))
            .collect();
        // The faulting instruction is a memory form on page 1; later instructions may be on either
        // page (they never run).
        items[fault_at] = pick_item(&mut rng, &[PAGE1_SLOT], false);
        for item in items.iter_mut().skip(fault_at + 1) {
            *item = pick_item(&mut rng, &[PAGE0_SLOTS[0], PAGE1_SLOT], true);
        }
        let code = code_of(&items);
        for (order, exec) in MODES {
            let block = block_of(&items, order);
            for round in 0..8usize {
                let state = random_state(&mut rng, round);
                let initials: Vec<u64> = PAGE0_SLOTS
                    .iter()
                    .chain([&PAGE1_SLOT])
                    .map(|_| rng.next())
                    .collect();
                let build = || {
                    let bus = world(PTE_RW_ACCESSED, pte_1, exec);
                    put_code(bus.atomic_inline_memory(), &code);
                    for (slot, initial) in PAGE0_SLOTS.iter().chain([&PAGE1_SLOT]).zip(&initials) {
                        put_operand(bus.atomic_inline_memory(), *slot, *initial);
                    }
                    bus
                };
                let (reference, subject) = (build(), build());
                let (reference_memory, memory) = (
                    reference.atomic_inline_memory(),
                    subject.atomic_inline_memory(),
                );
                let generation_before = generation(memory, DATA_FRAME);

                let mut interpreted = state.clone();
                let (_, retired_i, writes_i, fault_i) =
                    interpret(reference_memory, &mut interpreted, count as u64);
                let label = format!(
                    "{:?} fault_at={fault_at} pte={pte_1:#x} [{order:?}] round={round}",
                    names(&items)
                );
                assert_eq!(
                    retired_i, fault_at as u64,
                    "the interpreter faults too: {label}"
                );
                let fault_i = fault_i.expect("the interpreter raised a #PF");

                let mut jitted = state.clone();
                let mut writes_j = Vec::new();
                let report = block
                    .run(
                        &mut jitted,
                        memory,
                        env(),
                        count as u64,
                        None,
                        &mut |range| writes_j.push(range),
                    )
                    .unwrap();
                assert_eq!(report.exit, AtomicInlineExit::Fault, "{label}");
                assert_eq!(report.retired, fault_at as u64, "{label}");
                assert_eq!(
                    report.remaining_budget,
                    (count - fault_at) as u64,
                    "{label}"
                );
                assert_eq!(
                    jitted.rip,
                    rip_of(&items, fault_at),
                    "RIP at the fault: {label}"
                );
                assert_eq!(report.page_fault.as_ref(), Some(&fault_i), "{label}");
                assert_eq!(report.outcome.fault_vector, 14, "{label}");
                let memory_before_fault = items[..fault_at]
                    .iter()
                    .filter(|item| item.slot.is_some())
                    .count();
                assert_eq!(
                    report.committed_writes.len(),
                    memory_before_fault,
                    "{label}"
                );
                assert_eq!(
                    generation(memory, DATA_FRAME),
                    generation_before + memory_before_fault as u64,
                    "one generation raise per retired memory instruction: {label}"
                );

                let expected = outcome(&interpreted, reference_memory, &writes_i);
                let actual = outcome(&jitted, memory, &writes_j);
                assert_eq!(actual.gpr, expected.gpr, "GPRs: {label}");
                assert_eq!(actual.rip, expected.rip, "RIP: {label}");
                assert_eq!(actual.rflags, expected.rflags, "RFLAGS: {label}");
                assert_eq!(actual.data, expected.data, "memory: {label}");
                assert_eq!(actual.tables, expected.tables, "tables: {label}");
                assert_eq!(actual.generations, expected.generations, "{label}");
                if pte_1 == PTE_RO_UNACCESSED {
                    assert_ne!(memory.load_u64(at(0x5008)).unwrap() & 0x20, 0, "{label}");
                    assert!(writes_j.iter().any(|range| range.start() == at(0x5008)));
                }
                assert_eq!(
                    sorted(actual.writes),
                    sorted(expected.writes),
                    "committed writes: {label}"
                );
                cases += 1;
                if fault_at > 0 {
                    late_faults += 1;
                }
            }
        }
    }
    eprintln!(
        "s6_41d fault paths: {cases} cases ({late_faults} with earlier instructions retired)"
    );
    assert!(cases >= 300 && late_faults >= 100);
}

/// The STOP byte and the budget between the instructions of a block: the instructions before the
/// stop retired, the one it stopped did not run.
#[test]
fn s6_41d_the_guards_between_instructions_leave_the_earlier_ones_retired() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let items = vec![
        pick_item(&mut Rng(7), &[PAGE0_SLOTS[0]], false),
        pick_item(&mut Rng(11), &[PAGE0_SLOTS[1]], false),
        pick_item(&mut Rng(13), &[PAGE0_SLOTS[2]], false),
    ];
    let code = code_of(&items);
    for (order, exec) in MODES {
        let block = block_of(&items, order);
        // Budget 2 of 3: two retire, the third is not started.
        for budget in 0..=3u64 {
            let bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
            let memory = bus.atomic_inline_memory();
            put_code(memory, &code);
            let mut state = random_state(&mut Rng(3), 4);
            let reference = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
            put_code(reference.atomic_inline_memory(), &code);
            let mut twin = state.clone();
            for slot in PAGE0_SLOTS {
                put_operand(memory, slot, 0x1111);
                put_operand(reference.atomic_inline_memory(), slot, 0x1111);
            }
            if budget > 0 {
                interpret(reference.atomic_inline_memory(), &mut twin, budget);
            }
            let report = block
                .run(&mut state, memory, env(), budget, None, &mut |_| {})
                .unwrap();
            let label = format!("[{order:?}] budget={budget}");
            let expected_exit = if budget == 3 {
                AtomicInlineExit::Complete
            } else {
                AtomicInlineExit::BudgetExhausted
            };
            assert_eq!(report.exit, expected_exit, "{label}");
            assert_eq!(report.retired, budget, "{label}");
            assert_eq!(report.remaining_budget, 0, "{label}");
            assert_eq!(state.gpr, twin.gpr, "{label}");
            assert_eq!(state.rip, twin.rip, "{label}");
            assert_eq!(state.rflags, twin.rflags, "{label}");
            assert_eq!(
                data_frames(memory),
                data_frames(reference.atomic_inline_memory()),
                "{label}"
            );
        }

        // A raised STOP byte: no instruction runs.
        let bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
        let memory = bus.atomic_inline_memory();
        put_code(memory, &code);
        let stop = AtomicU8::new(1);
        let mut state = random_state(&mut Rng(3), 4);
        let before = state.clone();
        let report = block
            .run(&mut state, memory, env(), 3, Some(&stop), &mut |_| {})
            .unwrap();
        assert_eq!(report.exit, AtomicInlineExit::StopRequested);
        assert_eq!((report.retired, report.helper_calls), (0, 0));
        assert_eq!(state, before);

        // The walk sets the first operand's Dirty bit after the first guard.
        // Raise STOP there to exercise the next guard in the same native call.
        stop.store(0, Ordering::SeqCst);
        let mut writes = Vec::new();
        let report = block
            .run(&mut state, memory, env(), 3, Some(&stop), &mut |range| {
                if range.start() == at(0x5000) {
                    stop.store(1, Ordering::SeqCst);
                }
                writes.push(range);
            })
            .unwrap();
        assert_eq!(report.exit, AtomicInlineExit::StopRequested);
        assert_eq!((report.retired, report.helper_calls), (1, 1));
        assert_eq!(report.remaining_budget, 2);
        assert_eq!(report.committed_writes.len(), 1);

        let reference = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
        let reference = reference.atomic_inline_memory();
        put_code(reference, &code);
        let mut interpreted = before;
        let (_, retired, expected_writes, fault) = interpret(reference, &mut interpreted, 1);
        assert_eq!(retired, 1);
        assert!(fault.is_none());
        assert_eq!(state, interpreted);
        assert_eq!(data_frames(memory), data_frames(reference));
        assert_eq!(tables(memory), tables(reference));
        assert_eq!(writes, expected_writes);
    }
}

// ---------------------------------------------------------------------------------------------
// The session: `PagedFetchedLoadSession::execute_atomic`.
// ---------------------------------------------------------------------------------------------

fn session_state(rip: u64) -> LongModeState {
    let mut state = state_for(rip, 0x41);
    state.gpr[0] = 0x1122_3344;
    state.gpr[1] = 0x5566_7788_99aa_bbcc;
    state.gpr[3] = 0x0f0f_0f0f_0f0f_0f0f;
    state
}

fn lock_add_m32() -> Item {
    Item {
        name: "lock add m32, ebx".into(),
        code: abs_form(
            LOCK,
            &[0x01],
            REG_RBX,
            &[],
            (DATA_LINEAR + PAGE0_SLOTS[0]) as u32,
        ),
        slot: Some(PAGE0_SLOTS[0]),
    }
}

fn xchg_m64(slot: u64) -> Item {
    Item {
        name: "xchg m64, rcx".into(),
        code: abs_form(&[0x48], &[0x87], REG_RCX, &[], (DATA_LINEAR + slot) as u32),
        slot: Some(slot),
    }
}

fn execute(
    session: &mut PagedFetchedLoadSession,
    state: &mut LongModeState,
    bus: &mut MachineBus,
    budget: u64,
) -> Result<od_host::PagedFetchedAtomicReport, od_host::PagedFetchedLoadError> {
    session.execute_atomic(state, bus, CR3, EFER, ring0(), budget)
}

/// (3, 8) Code built for `Interleaved` does not run when the bus is `Parallel`, and the reverse:
/// the mode is part of the key of the admitted block. The session admits under the mode of the
/// bus, reuses within it, drops the block when the mode changes (and counts it), and the code
/// that ran in each mode is the code of that mode (the data words carry the acquire-release bits
/// in `Parallel` only). Results equal the interpreter in both modes.
#[test]
fn s6_41d_code_built_for_interleaved_does_not_run_in_parallel() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let items = vec![lock_add_m32(), xchg_m64(PAGE0_SLOTS[1])];
    let code = code_of(&items);
    let words_of = |order| block_of(&items, order).block().words.clone();
    let (interleaved_words, parallel_words) = (
        words_of(MemoryOrderMode::Interleaved),
        words_of(MemoryOrderMode::Parallel),
    );
    assert_ne!(
        interleaved_words, parallel_words,
        "the modes generate different code"
    );

    let mut bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Interleaved);
    put_code(bus.atomic_inline_memory(), &code);
    let mut session = PagedFetchedLoadSession::new();
    let mut reference = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Interleaved);
    put_code(reference.atomic_inline_memory(), &code);

    let mut expected_drops = 0;
    // Interleaved, Interleaved (reused), Parallel (dropped), Parallel (reused), Interleaved
    // (dropped again).
    let schedule = [
        (ExecutionMode::Interleaved, PagedFetchAdmission::Admitted),
        (ExecutionMode::Interleaved, PagedFetchAdmission::Reused),
        (ExecutionMode::Parallel, PagedFetchAdmission::Admitted),
        (ExecutionMode::Parallel, PagedFetchAdmission::Reused),
        (ExecutionMode::Interleaved, PagedFetchAdmission::Admitted),
    ];
    let mut previous = None;
    for (step, (exec, expected_admission)) in schedule.into_iter().enumerate() {
        bus.set_execution_mode(exec);
        reference.set_execution_mode(exec);
        if previous.is_some_and(|previous| previous != exec) {
            expected_drops += 1;
        }
        previous = Some(exec);
        let order = if exec == ExecutionMode::Parallel {
            MemoryOrderMode::Parallel
        } else {
            MemoryOrderMode::Interleaved
        };
        let mut state = session_state(CODE_LINEAR);
        let mut twin = state.clone();
        for slot in PAGE0_SLOTS {
            put_operand(bus.atomic_inline_memory(), slot, 0xaaaa_0000 + step as u64);
            put_operand(
                reference.atomic_inline_memory(),
                slot,
                0xaaaa_0000 + step as u64,
            );
        }
        interpret(reference.atomic_inline_memory(), &mut twin, 2);
        let report = execute(&mut session, &mut state, &mut bus, 2).unwrap();
        let label = format!("step {step} [{exec:?}]");
        assert_eq!(report.admission, expected_admission, "{label}");
        assert_eq!(report.mode, exec, "{label}");
        assert_eq!(report.block_instructions, 2, "{label}");
        assert_eq!(report.run.exit, AtomicInlineExit::Complete, "{label}");
        assert_eq!(report.run.retired, 2, "{label}");
        assert_eq!(report.fetched_code, code, "{label}");
        // The code that ran is the code of the mode.
        assert_eq!(session.admitted_atomic_mode(), Some(order), "{label}");
        assert_eq!(
            session.admitted_atomic_words().unwrap(),
            if exec == ExecutionMode::Parallel {
                &parallel_words[..]
            } else {
                &interleaved_words[..]
            },
            "{label}"
        );
        assert_eq!(session.atomic_mode_drops(), expected_drops, "{label}");
        // The result is the interpreter's in either mode.
        assert_eq!(state.gpr, twin.gpr, "{label}");
        assert_eq!(state.rip, twin.rip, "{label}");
        assert_eq!(state.rflags, twin.rflags, "{label}");
        assert_eq!(
            data_frames(bus.atomic_inline_memory()),
            data_frames(reference.atomic_inline_memory()),
            "{label}"
        );
        assert_eq!(
            tables(bus.atomic_inline_memory()),
            tables(reference.atomic_inline_memory())
        );
    }
    assert_eq!(expected_drops, 2);
    assert_eq!(session.atomic_mode_drops(), 2);
    assert_eq!(
        session.admissions(),
        3,
        "the first admission and one per mode change"
    );
    assert!(session.reset());
    assert!(!session.has_admitted_atomic_block());
}

/// (4) The host policy is asked when the cache of a mode is created: a host without FEAT_LSE
/// (stood for by a refusing policy) refuses a `Parallel` block, explicitly and before anything is
/// walked; the policy of this host (Apple, LSE present) admits both modes.
#[test]
fn s6_41d_a_host_without_lse_refuses_the_atomic_block_before_any_walk() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // The policy of this host: LSE is baseline on Apple silicon.
    assert!(mode_admitted_on_host(MemoryOrderMode::Interleaved));
    assert!(mode_admitted_on_host(MemoryOrderMode::Parallel));

    let items = vec![lock_add_m32()];
    let code = code_of(&items);
    for (exec, refused) in [
        (ExecutionMode::Parallel, true),
        (ExecutionMode::Interleaved, false),
    ] {
        // A host without LSE for `Parallel` only; and one without it for every mode.
        for policy in [
            (|mode| mode != MemoryOrderMode::Parallel) as fn(MemoryOrderMode) -> bool,
            |_| false,
        ] {
            // Every entry of the code and data paths has the accessed bit clear: a walk (which
            // sets it) done before the policy is asked would change the tables.
            let mut bus = world(DATA_FRAME | 0x07, DATA_FRAME_1 | 0x07, exec);
            for (address, value) in [
                (0x1000, 0x2007),
                (0x2000, 0x3007),
                (0x3010, 0x4007),
                (0x3020, 0x5007),
                (0x4000, CODE_FRAME | 0x07),
            ] {
                bus.write_physical(at(address), AccessWidth::Qword, value, |_| {})
                    .unwrap();
            }
            put_code(bus.atomic_inline_memory(), &code);
            put_operand(bus.atomic_inline_memory(), PAGE0_SLOTS[0], 5);
            let tables_before = tables(bus.atomic_inline_memory());
            assert!(
                tables_before.iter().all(|entry| entry & 0x20 == 0),
                "the accessed bit is clear on every path entry"
            );
            let generations_before = [CODE_FRAME, DATA_FRAME, DATA_FRAME_1, 0x5000]
                .map(|address| generation(bus.atomic_inline_memory(), address));
            let mut state = session_state(CODE_LINEAR);
            let before = state.clone();
            let mut session = PagedFetchedLoadSession::new();
            session.set_atomic_mode_policy(Some(policy));
            let all_refused = !policy(MemoryOrderMode::Interleaved);
            let result = execute(&mut session, &mut state, &mut bus, 1);
            if refused || all_refused {
                let error = result.err().expect("the policy refuses the mode");
                assert_eq!(
                    error.kind,
                    PagedFetchedLoadErrorKind::AtomicModeUnavailable(exec)
                );
                assert!(error.fetch_committed_writes.is_empty(), "no walk happened");
                assert_eq!(state, before);
                assert_eq!(
                    tables(bus.atomic_inline_memory()),
                    tables_before,
                    "no table entry was touched (no accessed bit set)"
                );
                assert_eq!(
                    [CODE_FRAME, DATA_FRAME, DATA_FRAME_1, 0x5000]
                        .map(|address| generation(bus.atomic_inline_memory(), address)),
                    generations_before
                );
                assert!(!session.has_admitted_atomic_block());
                assert_eq!(session.admissions(), 0);
            } else {
                let report = result.unwrap();
                assert_eq!(report.run.exit, AtomicInlineExit::Complete);
                // The set-up is sensitive: an admitted block does walk and set the accessed bit.
                assert_ne!(tables(bus.atomic_inline_memory()), tables_before);
            }
        }
    }
}

/// (6) A write into the code page of the active block ends the call in `CodeInvalidated`: the
/// instruction that wrote retires, the later instructions of the block do not run (they would
/// run from bytes the write may have changed), the block is retired and never reused, and the next
/// call fetches the new bytes. The state after the exit is the interpreter's after the same
/// number of instructions. Both modes. A write to another page does not invalidate.
#[test]
fn s6_41d_a_write_into_the_active_code_page_ends_the_block_with_code_invalidated() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    // Data page 0 aliases the code frame: DATA_LINEAR+x is physical 0x6000+x.
    let aliased_leaf = CODE_FRAME | 0x27;
    for exec in [ExecutionMode::Interleaved, ExecutionMode::Parallel] {
        // i1: lock add [page 1 slot], ebx (another page, no invalidation);
        // i2: xchg [DATA+0x10], ebx: physical 0x6010 is inside i3's bytes (code offset 16);
        // i3: lock inc [page 1 slot + 4].
        let i1 = Item {
            name: "lock add m32 (page 1)".into(),
            code: abs_form(
                LOCK,
                &[0x01],
                REG_RBX,
                &[],
                (DATA_LINEAR + PAGE1_SLOT) as u32,
            ),
            slot: Some(PAGE1_SLOT),
        };
        let i2 = Item {
            name: "xchg m32 into the code".into(),
            code: abs_form(&[], &[0x87], REG_RBX, &[], (DATA_LINEAR + 0x10) as u32),
            slot: Some(0x10),
        };
        let i3 = Item {
            name: "lock inc m32 (page 1)".into(),
            code: abs_form(LOCK, &[0xff], 0, &[], (DATA_LINEAR + PAGE1_SLOT + 4) as u32),
            slot: Some(PAGE1_SLOT + 4),
        };
        let items = vec![i1, i2, i3];
        let code = code_of(&items);
        assert_eq!(rip_of(&items, 2), CODE_LINEAR + 15);
        let build = || {
            let bus = world(aliased_leaf, PTE_RW_ACCESSED, exec);
            put_code(bus.atomic_inline_memory(), &code);
            put_operand(bus.atomic_inline_memory(), PAGE1_SLOT, 0x1000);
            bus
        };
        let (mut bus, reference) = (build(), build());
        let mut state = session_state(CODE_LINEAR);
        state.gpr[3] = 0x9090_9090; // RBX: four NOPs for the XCHG to put into the code
        let mut twin = state.clone();
        let mut session = PagedFetchedLoadSession::new();

        let report = execute(&mut session, &mut state, &mut bus, 3).unwrap();
        let label = format!("[{exec:?}]");
        assert_eq!(report.admission, PagedFetchAdmission::Admitted, "{label}");
        assert_eq!(report.block_instructions, 3, "{label}");
        assert_eq!(
            report.run.exit,
            AtomicInlineExit::CodeInvalidated,
            "{label}"
        );
        assert_eq!(
            report.run.retired, 2,
            "i1 and i2 retired, i3 did not run: {label}"
        );
        assert_eq!(state.rip, rip_of(&items, 2), "{label}");
        // The interpreter after two instructions is the same state and memory.
        let (_, retired, _, _) = interpret(reference.atomic_inline_memory(), &mut twin, 2);
        assert_eq!(retired, 2);
        assert_eq!(state.gpr, twin.gpr, "{label}");
        assert_eq!(state.rip, twin.rip, "{label}");
        assert_eq!(state.rflags, twin.rflags, "{label}");
        let physical = |bus: &MachineBus| {
            let mut bytes = vec![0u8; 0x3000];
            bus.atomic_inline_memory()
                .load_bytes(at(CODE_FRAME), &mut bytes)
                .unwrap();
            bytes
        };
        assert_eq!(physical(&bus), physical(&reference), "{label}");
        assert_eq!(
            &physical(&bus)[16..20],
            &[0x90; 4],
            "the XCHG wrote the NOPs: {label}"
        );
        // The committed writes include the operand on the code page.
        assert!(
            report
                .committed_writes
                .iter()
                .any(|range| range.start().get() == CODE_FRAME + 0x10 && range.len() == 4),
            "{label}: {:?}",
            report.committed_writes
        );

        // The block is retired: the next call fetches the new bytes, never runs the old block.
        let admissions = session.admissions();
        let error = execute(&mut session, &mut state, &mut bus, 1)
            .err()
            .expect("i3 was overwritten: what is at RIP is no atomic form");
        assert!(
            matches!(error.kind, PagedFetchedLoadErrorKind::AtomicForm(_)),
            "{label}: {:?}",
            error.kind
        );
        assert_eq!(
            session.admissions(),
            admissions,
            "nothing was admitted: {label}"
        );
        assert!(
            !session.has_admitted_atomic_block(),
            "the stale block was dropped: {label}"
        );
    }

    // A write to ANOTHER page does not invalidate: the whole block runs and is reused.
    let items = vec![lock_add_m32(), xchg_m64(PAGE0_SLOTS[1])];
    let code = code_of(&items);
    let mut bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Interleaved);
    put_code(bus.atomic_inline_memory(), &code);
    let mut session = PagedFetchedLoadSession::new();
    for expected in [PagedFetchAdmission::Admitted, PagedFetchAdmission::Reused] {
        let mut state = session_state(CODE_LINEAR);
        let report = execute(&mut session, &mut state, &mut bus, 2).unwrap();
        assert_eq!(report.run.exit, AtomicInlineExit::Complete);
        assert_eq!(report.admission, expected);
    }
}

/// (2) The neighbourhood of ordinary instructions: the session admits the leading run of atomic
/// forms as one block and stops before the first ordinary instruction (an ordinary load here),
/// which stays with the interpreter; a block of the run after it is admitted from the new RIP.
/// The sequence `lock add; xchg; mov rax,[rbx]; lock inc; mfence` run as block, interpreter step,
/// block ends in the state the interpreter reaches alone.
#[test]
fn s6_41d_atomic_blocks_run_next_to_ordinary_instructions_through_the_session() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let load = Item {
        name: "mov rax, [rbx]".into(),
        code: vec![0x48, 0x8b, 0x03],
        slot: None,
    };
    let inc = Item {
        name: "lock inc m32".into(),
        code: abs_form(LOCK, &[0xff], 0, &[], (DATA_LINEAR + PAGE0_SLOTS[2]) as u32),
        slot: Some(PAGE0_SLOTS[2]),
    };
    let fence = Item {
        name: "mfence".into(),
        code: FENCES[0].to_vec(),
        slot: None,
    };
    let items = vec![lock_add_m32(), xchg_m64(PAGE0_SLOTS[1]), load, inc, fence];
    let code = code_of(&items);
    for exec in [ExecutionMode::Interleaved, ExecutionMode::Parallel] {
        let build = || {
            let bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, exec);
            put_code(bus.atomic_inline_memory(), &code);
            for slot in PAGE0_SLOTS {
                put_operand(bus.atomic_inline_memory(), slot, 0x0102_0304_0506_0708);
            }
            bus
        };
        let (mut bus, reference) = (build(), build());
        let mut state = session_state(CODE_LINEAR);
        // RBX is the addend of `lock add` and the pointer of the load: a data slot, aligned.
        state.gpr[3] = DATA_LINEAR + PAGE0_SLOTS[2] + 8;
        put_operand(
            bus.atomic_inline_memory(),
            PAGE0_SLOTS[2] + 8,
            0x1234_5678_9abc_def0,
        );
        put_operand(
            reference.atomic_inline_memory(),
            PAGE0_SLOTS[2] + 8,
            0x1234_5678_9abc_def0,
        );
        let mut twin = state.clone();
        let (_, retired, _, _) = interpret(reference.atomic_inline_memory(), &mut twin, 5);
        assert_eq!(retired, 5);

        let mut session = PagedFetchedLoadSession::new();
        // Block 1: the two atomic forms; the ordinary load ends it.
        let report = execute(&mut session, &mut state, &mut bus, 16).unwrap();
        assert_eq!(report.block_instructions, 2);
        assert_eq!(report.run.exit, AtomicInlineExit::Complete);
        assert_eq!(state.rip, rip_of(&items, 2));
        // The interpreter owns the ordinary load.
        let (_, retired, _, _) = interpret(bus.atomic_inline_memory(), &mut state, 1);
        assert_eq!(retired, 1);
        assert_eq!(state.rip, rip_of(&items, 3));
        // Block 2: `lock inc` and `mfence` from the new RIP.
        let report = execute(&mut session, &mut state, &mut bus, 16).unwrap();
        assert_eq!(report.admission, PagedFetchAdmission::Admitted);
        assert_eq!(report.block_instructions, 2);
        assert_eq!(report.run.retired, 2);
        assert_eq!(state.rip, CODE_LINEAR + code.len() as u64);
        let label = format!("[{exec:?}]");
        assert_eq!(state.gpr, twin.gpr, "{label}");
        assert_eq!(state.rip, twin.rip, "{label}");
        assert_eq!(state.rflags, twin.rflags, "{label}");
        assert_eq!(
            data_frames(bus.atomic_inline_memory()),
            data_frames(reference.atomic_inline_memory()),
            "{label}"
        );
        assert_eq!(
            tables(bus.atomic_inline_memory()),
            tables(reference.atomic_inline_memory())
        );

        // The ordinary load as the first instruction: refused, nothing ran.
        let mut state = session_state(rip_of(&items, 2));
        let before = state.clone();
        let error = execute(&mut session, &mut state, &mut bus, 16)
            .err()
            .expect("an ordinary load is no atomic block");
        assert_eq!(
            error.kind,
            PagedFetchedLoadErrorKind::AtomicForm(AtomicAdapterError::NotAnAtomicForm)
        );
        assert_eq!(state, before);
    }
}

/// Limits of the profile, recorded as tests: forms the adapter does not lower end the block or
/// refuse it (`LOCK BTS m, r`, a data register outside RAX..RBX), a run is bounded by
/// `MAX_ATOMIC_BLOCK_INSTRUCTIONS` and by the fetch window, and the guest bytes are never
/// decoded beyond the window.
#[test]
fn s6_41d_unsupported_forms_and_the_block_bounds_are_explicit() {
    let _guard = JIT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut bus = world(PTE_RW_ACCESSED, PTE_RW_ACCESSED, ExecutionMode::Interleaved);
    // `lock bts [m64], rcx` (register bit offset: the element address depends on the register).
    let bts_register = vec![0xf0, 0x48, 0x0f, 0xab, 0x0c, 0x25, 0x40, 0x00, 0x80, 0x00];
    // `lock xadd [m64], rsi` (RSI is outside the RAX..RBX profile of the IR).
    let xadd_rsi = vec![0xf0, 0x48, 0x0f, 0xc1, 0x34, 0x25, 0x40, 0x00, 0x80, 0x00];
    // An FS-override locked form is not an atomic form of the adapter.
    let fs_form = vec![0x64, 0xf0, 0x01, 0x1c, 0x25, 0x40, 0x00, 0x00, 0x00];
    for (name, bytes, expected) in [
        (
            "lock bts m64, rcx",
            bts_register,
            Some(AtomicAdapterError::RegisterBitOffset),
        ),
        (
            "lock xadd m64, rsi",
            xadd_rsi,
            Some(AtomicAdapterError::UnsupportedRegister),
        ),
        ("fs lock add", fs_form, None),
    ] {
        put_code(bus.atomic_inline_memory(), &bytes);
        let mut state = session_state(CODE_LINEAR);
        let before = state.clone();
        let mut session = PagedFetchedLoadSession::new();
        let error = execute(&mut session, &mut state, &mut bus, 4)
            .err()
            .unwrap_or_else(|| panic!("{name} must be refused"));
        match (&error.kind, expected) {
            (PagedFetchedLoadErrorKind::AtomicForm(got), Some(expected)) => {
                assert_eq!(*got, expected, "{name}")
            }
            (PagedFetchedLoadErrorKind::AtomicForm(_), None) => {}
            (other, _) => panic!("{name}: {other:?}"),
        }
        assert_eq!(state, before, "{name}");
        assert!(!session.has_admitted_atomic_block(), "{name}");
    }

    // A long run is cut at the block bound; the rest runs from the next RIP.
    let one = lock_add_m32();
    let run: Vec<Item> =
        std::iter::repeat_n(one.clone(), MAX_ATOMIC_BLOCK_INSTRUCTIONS + 3).collect();
    assert!(
        run.iter().map(|item| item.code.len()).sum::<usize>() > PAGED_ATOMIC_FETCH_WINDOW
            || run.len() > MAX_ATOMIC_BLOCK_INSTRUCTIONS
    );
    put_code(bus.atomic_inline_memory(), &code_of(&run));
    put_operand(bus.atomic_inline_memory(), PAGE0_SLOTS[0], 0);
    let mut state = session_state(CODE_LINEAR);
    state.gpr[3] = 1;
    let mut session = PagedFetchedLoadSession::new();
    let report = execute(&mut session, &mut state, &mut bus, 100).unwrap();
    assert_eq!(report.block_instructions, MAX_ATOMIC_BLOCK_INSTRUCTIONS);
    assert_eq!(report.run.retired as usize, MAX_ATOMIC_BLOCK_INSTRUCTIONS);
    assert_eq!(state.rip, rip_of(&run, MAX_ATOMIC_BLOCK_INSTRUCTIONS));
    let mut cell = [0u8; 4];
    bus.atomic_inline_memory()
        .load_bytes(at(DATA_FRAME + PAGE0_SLOTS[0]), &mut cell)
        .unwrap();
    assert_eq!(
        u32::from_le_bytes(cell) as usize,
        MAX_ATOMIC_BLOCK_INSTRUCTIONS
    );
    // The rest: a second block of three, from the new RIP.
    let report = execute(&mut session, &mut state, &mut bus, 100).unwrap();
    assert_eq!(report.admission, PagedFetchAdmission::Admitted);
    assert_eq!(report.block_instructions, 3);
}
