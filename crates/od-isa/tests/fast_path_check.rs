//! PERF-22: the fast-path check (ADR-0017, section 5) over scripted programs with RAM and a
//! side-effecting MMIO window: the check must not change what a run does, the devices must see
//! every access exactly once, an empty fast path must give no divergence, and a wrong draft
//! handler must be caught.

use od_isa::execute::{RunConfig, RunExit, RunResult, run, run_with_fast_path_check};
use od_isa::fast_path_check::{
    self, CheckCounters, CheckMemory, DraftFastStep, FastPathMode, FastStepResult,
};
use od_isa::long_decode::{ArithmeticKind, Instruction, LogicKind, Operand, Operation};
use od_isa::{
    AccessContext, AccessKind, AccessOrigin, GuestMemory, InstructionMode, LongModeState,
    MemoryError, OperandSize, PrivilegeLevel, Register, StiShadow,
};

const CODE: u64 = 0x1000;
const RAM: u64 = 0x2000;
const RAM_LEN: usize = 0x100;
const MMIO: u64 = 0xfee0_0000;
const MMIO_LEN: u64 = 0x100;

/// What a device saw: method, address, width, and the bytes of a write or the value returned.
#[derive(Clone, Debug, Eq, PartialEq)]
enum DeviceEvent {
    Read {
        address: u64,
        len: usize,
        value: u64,
    },
    Write {
        address: u64,
        data: Vec<u8>,
    },
}

/// Code, RAM, and an MMIO window whose every read returns a new value (a read has an effect, so a
/// second read of the same register would be visible).
#[derive(Clone)]
struct Bus {
    code: Vec<u8>,
    ram: Vec<u8>,
    mmio_reads: u64,
    events: Vec<DeviceEvent>,
}

impl Bus {
    fn new(code: &[u8]) -> Self {
        Self {
            code: code.to_vec(),
            ram: (0..RAM_LEN).map(|index| index as u8 ^ 0x5a).collect(),
            mmio_reads: 0,
            events: Vec::new(),
        }
    }

    fn in_mmio(address: u64, len: usize) -> bool {
        address >= MMIO && address + len as u64 <= MMIO + MMIO_LEN
    }

    fn ram_slot(address: u64, len: usize) -> Option<usize> {
        let offset = address.checked_sub(RAM)? as usize;
        (offset + len <= RAM_LEN).then_some(offset)
    }
}

impl GuestMemory for Bus {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        address
            .checked_sub(CODE)
            .and_then(|offset| self.code.get(offset as usize))
            .copied()
            .ok_or(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        address: u64,
        _: AccessContext,
        destination: &mut [u8],
    ) -> Result<(), MemoryError> {
        if Self::in_mmio(address, destination.len()) {
            self.mmio_reads += 1;
            let value = 0x1100_0000_0000_0000 + self.mmio_reads * 0x0101;
            destination.copy_from_slice(&value.to_le_bytes()[..destination.len()]);
            self.events.push(DeviceEvent::Read {
                address,
                len: destination.len(),
                value,
            });
            return Ok(());
        }
        let offset = Self::ram_slot(address, destination.len()).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Read,
        })?;
        destination.copy_from_slice(&self.ram[offset..offset + destination.len()]);
        Ok(())
    }

    fn write(&mut self, address: u64, _: AccessContext, source: &[u8]) -> Result<(), MemoryError> {
        if Self::in_mmio(address, source.len()) {
            self.events.push(DeviceEvent::Write {
                address,
                data: source.to_vec(),
            });
            return Ok(());
        }
        let offset = Self::ram_slot(address, source.len()).ok_or(MemoryError::Unmapped {
            address,
            access: AccessKind::Write,
        })?;
        self.ram[offset..offset + source.len()].copy_from_slice(source);
        Ok(())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}

fn context() -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    }
}

fn config(code_len: usize) -> RunConfig {
    RunConfig {
        instruction_budget: 200,
        stop_rip: Some(CODE + code_len as u64),
        context: context(),
    }
}

fn initial_state() -> LongModeState {
    let mut state = LongModeState {
        rip: CODE,
        rflags: 2,
        // The fast path requires a modelled, clear interrupt shadow (ADR-0017 section 4).
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rbx as usize] = MMIO;
    state.gpr[Register::Rsi as usize] = RAM;
    state.gpr[Register::Rsp as usize] = RAM + RAM_LEN as u64 - 8;
    state.gpr[Register::Rax as usize] = 0x1234_5678_9abc_def0;
    state
}

/// RAM and MMIO loads and stores, memory read-modify-write, stack forms, a counted loop, and
/// flag-setting arithmetic.
const PROGRAM: &[u8] = &[
    0x48, 0x8b, 0x03, //             mov rax, [rbx]         MMIO read
    0x48, 0x89, 0x46, 0x08, //       mov [rsi+8], rax       RAM write
    0x48, 0x89, 0x43, 0x10, //       mov [rbx+0x10], rax    MMIO write
    0x48, 0x01, 0x06, //             add [rsi], rax         RAM read-modify-write
    0x48, 0x01, 0x03, //             add [rbx], rax         MMIO read-modify-write
    0x50, //                         push rax
    0x5a, //                         pop rdx
    0xe8, 0x00, 0x00, 0x00, 0x00, // call +0
    0x59, //                         pop rcx
    0x48, 0x83, 0xc0, 0x05, //       add rax, 5
    0x48, 0x39, 0xd0, //             cmp rax, rdx
    0x48, 0x29, 0xd0, //             sub rax, rdx
    0xb9, 0x03, 0x00, 0x00, 0x00, // mov ecx, 3
    0x48, 0xff, 0xc9, //             dec rcx
    0x75, 0xfb, //                   jnz -5
    0x48, 0x01, 0xd8, //             add rax, rbx
];

struct Outcome {
    result: RunResult,
    state: LongModeState,
    bus: Bus,
}

fn unchecked(program: &[u8]) -> Outcome {
    let mut state = initial_state();
    let mut bus = Bus::new(program);
    let result = run(&mut state, &mut bus, config(program.len()), || false);
    Outcome { result, state, bus }
}

fn checked(program: &[u8], mode: FastPathMode) -> (Outcome, CheckCounters) {
    fast_path_check::set_mode(mode);
    fast_path_check::reset();
    let mut state = initial_state();
    let mut bus = Bus::new(program);
    let result = run_with_fast_path_check(&mut state, &mut bus, config(program.len()), || false);
    fast_path_check::set_mode(FastPathMode::Tree);
    (Outcome { result, state, bus }, fast_path_check::counters())
}

fn assert_same_run(checked: &Outcome, baseline: &Outcome) {
    assert_eq!(checked.result, baseline.result);
    assert_eq!(checked.state, baseline.state);
    assert_eq!(checked.bus.ram, baseline.bus.ram);
    // The devices saw every access exactly once and in the same order as without the check.
    assert_eq!(checked.bus.events, baseline.bus.events);
    assert_eq!(checked.bus.mmio_reads, baseline.bus.mmio_reads);
}

#[test]
fn the_program_reaches_the_device_and_ram() {
    let baseline = unchecked(PROGRAM);
    assert_eq!(
        baseline.result.exit,
        RunExit::StopAddress,
        "{:?}",
        baseline.result
    );
    // Two MMIO reads (the load and the read-modify-write), two MMIO writes.
    assert_eq!(baseline.bus.mmio_reads, 2);
    let writes = baseline
        .bus
        .events
        .iter()
        .filter(|event| matches!(event, DeviceEvent::Write { .. }))
        .count();
    assert_eq!(writes, 2);
}

#[test]
fn the_general_path_against_itself_gives_no_divergence_and_touches_devices_once() {
    let baseline = unchecked(PROGRAM);
    let (outcome, counters) = checked(PROGRAM, FastPathMode::GeneralPath);
    assert!(fast_path_check::take_divergence().is_none());
    assert_same_run(&outcome, &baseline);
    assert_eq!(counters.divergences, 0);
    assert_eq!(counters.refused_by_admission, 0);
    // Every retired instruction was compared, memory calls included.
    assert_eq!(counters.compared, baseline.result.instructions_executed);
    assert!(counters.memory_calls_compared >= 10, "{counters:?}");
}

fn refuse_all(
    _: &mut LongModeState,
    _: &mut CheckMemory<'_>,
    _: AccessContext,
    _: &mut u64,
    _: &Instruction,
    _: bool,
) -> FastStepResult {
    None
}

#[test]
fn the_tree_fast_path_takes_register_alu_agrees_and_changes_nothing() {
    // PERF-23: the fast path of the tree handles the register ALU forms (group G2) of the program
    // (add/cmp/sub rax and the rest are taken, the memory forms and stack forms are refused, call is
    // refused; the branches are taken (G3)) and the run is the unchecked run.
    let baseline = unchecked(PROGRAM);
    let (outcome, counters) = checked(PROGRAM, FastPathMode::Tree);
    assert!(fast_path_check::take_divergence().is_none());
    assert_same_run(&outcome, &baseline);
    assert_eq!(counters.divergences, 0);
    // add rax,5 / cmp rax,rdx / sub rax,rdx / dec rcx (three times) / add rax,rbx.
    assert!(counters.compared >= 7, "{counters:?}");
    assert_eq!(
        counters.compared + counters.refused_by_admission,
        baseline.result.instructions_executed,
        "{counters:?}"
    );
}

#[test]
fn a_fast_path_that_refuses_everything_takes_nothing_and_changes_nothing() {
    let baseline = unchecked(PROGRAM);
    let (outcome, counters) = checked(PROGRAM, FastPathMode::Custom(refuse_all as DraftFastStep));
    assert!(fast_path_check::take_divergence().is_none());
    assert_same_run(&outcome, &baseline);
    assert_eq!(counters.divergences, 0);
    assert_eq!(counters.compared, 0);
    assert_eq!(
        counters.refused_by_admission,
        baseline.result.instructions_executed
    );
}

#[test]
fn a_memory_error_outcome_is_compared_too() {
    // mov rax, [rsi+0x1000]: outside RAM, an error of the memory; then nothing runs.
    let program: &[u8] = &[0x48, 0x8b, 0x86, 0x00, 0x10, 0x00, 0x00, 0x90];
    let baseline = unchecked(program);
    assert!(
        matches!(baseline.result.exit, RunExit::MemoryError(_)),
        "{:?}",
        baseline.result
    );
    let (outcome, counters) = checked(program, FastPathMode::GeneralPath);
    assert!(fast_path_check::take_divergence().is_none());
    assert_same_run(&outcome, &baseline);
    assert_eq!((counters.compared, counters.divergences), (1, 0));
}

// ---- a draft handler: register and immediate ADD, SUB and CMP ----

const CF: u64 = 1;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const ARITHMETIC: u64 = CF | PF | AF | ZF | SF | OF;

/// The flags of an addition or subtraction written out here, independently of the interpreter.
fn flags(kind: ArithmeticKind, size: OperandSize, left: u64, right: u64) -> (u64, u64) {
    let mask = size.mask();
    let bits = size.bytes() as u32 * 8;
    let (left, right) = (left & mask, right & mask);
    let (result, carry, overflow) = match kind {
        ArithmeticKind::Add => {
            let result = left.wrapping_add(right) & mask;
            (
                result,
                result < left,
                ((left ^ result) & (right ^ result)) >> (bits - 1) & 1 != 0,
            )
        }
        _ => {
            let result = left.wrapping_sub(right) & mask;
            (
                result,
                left < right,
                ((left ^ right) & (left ^ result)) >> (bits - 1) & 1 != 0,
            )
        }
    };
    let mut flags = 0;
    flags |= u64::from(carry) * CF;
    flags |= u64::from((result as u8).count_ones() % 2 == 0) * PF;
    flags |= u64::from((left ^ right ^ result) & 0x10 != 0) * AF;
    flags |= u64::from(result == 0) * ZF;
    flags |= u64::from(result >> (bits - 1) & 1 != 0) * SF;
    flags |= u64::from(overflow) * OF;
    (result, flags)
}

fn draft(
    state: &mut LongModeState,
    _: &mut CheckMemory<'_>,
    _: AccessContext,
    defined: &mut u64,
    instruction: &Instruction,
    corrupt_overflow: bool,
) -> FastStepResult {
    let Operation::Arithmetic { kind, dst, src } = instruction.operation else {
        return None;
    };
    if !matches!(
        kind,
        ArithmeticKind::Add | ArithmeticKind::Sub | ArithmeticKind::Cmp
    ) {
        return None;
    }
    let Operand::Register(dst) = dst else {
        return None;
    };
    let right = match src {
        Operand::Register(view) => state.read(view),
        Operand::Immediate(value) => value,
        Operand::Memory(_) => return None,
    };
    let size = dst.size();
    let left = state.read(dst);
    let (result, mut produced) = flags(kind, size, left, right);
    if corrupt_overflow {
        produced ^= OF;
    }
    if !matches!(kind, ArithmeticKind::Cmp) {
        state.write(dst, result);
    }
    state.rflags = state.rflags & !ARITHMETIC | produced;
    *defined |= ARITHMETIC;
    state.rip = state.rip.wrapping_add(u64::from(instruction.length));
    Some(Ok(od_isa::execute::StepOutcome::Advanced))
}

fn correct_draft(
    state: &mut LongModeState,
    memory: &mut CheckMemory<'_>,
    context: AccessContext,
    defined: &mut u64,
    instruction: &Instruction,
    _: bool,
) -> FastStepResult {
    draft(state, memory, context, defined, instruction, false)
}

fn corrupted_draft(
    state: &mut LongModeState,
    memory: &mut CheckMemory<'_>,
    context: AccessContext,
    defined: &mut u64,
    instruction: &Instruction,
    _: bool,
) -> FastStepResult {
    draft(state, memory, context, defined, instruction, true)
}

#[test]
fn a_correct_draft_handler_agrees_with_the_general_path() {
    let baseline = unchecked(PROGRAM);
    let (outcome, counters) = checked(
        PROGRAM,
        FastPathMode::Custom(correct_draft as DraftFastStep),
    );
    assert!(fast_path_check::take_divergence().is_none());
    assert_same_run(&outcome, &baseline);
    assert_eq!(counters.divergences, 0);
    // add rax,5 / cmp / sub / add rax,rbx / dec is not taken (Inc/Dec): at least four taken.
    assert!(counters.compared >= 4, "{counters:?}");
    assert!(counters.refused_by_admission > 0);
}

#[test]
fn a_draft_handler_with_one_corrupted_flag_is_caught_at_the_first_instruction_it_takes() {
    let (outcome, counters) = checked(
        PROGRAM,
        FastPathMode::Custom(corrupted_draft as DraftFastStep),
    );
    let divergence = fast_path_check::take_divergence().expect("the corrupted flag is found");
    assert_eq!(counters.divergences, 1);
    // The run stops there, with the report in the exit.
    let RunExit::MemoryError(MemoryError::Backend(report)) = &outcome.result.exit else {
        panic!("{:?}", outcome.result);
    };
    assert_eq!(report, &divergence.to_string());
    // The first instruction the draft takes is "add rax, 5" at offset 25 of the program.
    assert_eq!(divergence.rip, CODE + 25, "{divergence}");
    assert_eq!(divergence.bytes, [0x48, 0x83, 0xc0, 0x05]);
    assert!(
        divergence.instruction.contains("Arithmetic"),
        "{divergence}"
    );
    assert!(
        divergence
            .differences
            .iter()
            .any(|line| line.starts_with("state.rflags:")),
        "{divergence}"
    );
    // Nothing after the divergent instruction ran.
    assert_eq!(counters.compared, 0);
}

// ---- drafts that break the contract in other ways ----

/// Takes `and rax, 1` (the general path leaves AF undefined there) but leaves the definedness mask
/// alone.
fn draft_without_defined(
    state: &mut LongModeState,
    _: &mut CheckMemory<'_>,
    _: AccessContext,
    _: &mut u64,
    instruction: &Instruction,
    _: bool,
) -> FastStepResult {
    let Operation::Logic {
        kind: LogicKind::And,
        dst: Operand::Register(dst),
        src: Operand::Immediate(value),
    } = instruction.operation
    else {
        return None;
    };
    let result = state.read(dst) & value;
    state.write(dst, result);
    let produced = u64::from(result == 0) * ZF
        | u64::from(result >> 63 != 0) * SF
        | u64::from((result as u8).count_ones() % 2 == 0) * PF;
    state.rflags = state.rflags & !ARITHMETIC | produced;
    state.rip = state.rip.wrapping_add(u64::from(instruction.length));
    Some(Ok(od_isa::execute::StepOutcome::Advanced))
}

/// Takes the four-byte `mov [rsi+8], rax` and stores rax + 1 instead of rax.
fn draft_store_wrong_value(
    state: &mut LongModeState,
    memory: &mut CheckMemory<'_>,
    context: AccessContext,
    _: &mut u64,
    instruction: &Instruction,
    _: bool,
) -> FastStepResult {
    if instruction.length != 4 {
        return None;
    }
    let value = state.gpr[Register::Rax as usize].wrapping_add(1);
    if let Err(error) = memory.write_integer(RAM + 8, context, OperandSize::Qword, value) {
        return Some(Err(error));
    }
    state.rip = state.rip.wrapping_add(u64::from(instruction.length));
    Some(Ok(od_isa::execute::StepOutcome::Advanced))
}

/// Writes rax and then says "not handled".
fn draft_side_effect_then_none(
    state: &mut LongModeState,
    _: &mut CheckMemory<'_>,
    _: AccessContext,
    _: &mut u64,
    _: &Instruction,
    _: bool,
) -> FastStepResult {
    state.gpr[Register::Rax as usize] ^= 1;
    None
}

#[test]
fn a_draft_that_stores_another_value_is_caught_at_the_first_memory_call() {
    // mov [rsi+8], rax
    let program: &[u8] = &[0x48, 0x89, 0x46, 0x08];
    let (_, counters) = checked(
        program,
        FastPathMode::Custom(draft_store_wrong_value as DraftFastStep),
    );
    let divergence = fast_path_check::take_divergence().expect("the wrong value is found");
    assert_eq!(counters.divergences, 1);
    assert!(
        divergence
            .differences
            .iter()
            .any(|line| line.starts_with("memory call 0:")),
        "{divergence}"
    );
}

#[test]
fn a_draft_that_forgets_the_definedness_mask_is_caught() {
    // and rax, 1
    let program: &[u8] = &[0x48, 0x83, 0xe0, 0x01];
    let (_, counters) = checked(
        program,
        FastPathMode::Custom(draft_without_defined as DraftFastStep),
    );
    let divergence = fast_path_check::take_divergence().expect("the mask is found");
    assert_eq!(counters.divergences, 1);
    assert!(
        divergence
            .differences
            .iter()
            .any(|line| line.starts_with("rflags_defined:")),
        "{divergence}"
    );
}

#[test]
fn a_draft_that_returns_none_after_a_side_effect_is_caught() {
    let (_, counters) = checked(
        PROGRAM,
        FastPathMode::Custom(draft_side_effect_then_none as DraftFastStep),
    );
    let divergence = fast_path_check::take_divergence().expect("the side effect is found");
    assert_eq!(counters.divergences, 1);
    assert!(
        divergence
            .differences
            .iter()
            .any(|line| line.starts_with("admission: None after a side effect")),
        "{divergence}"
    );
    // The first instruction already fails, so nothing was compared or refused cleanly.
    assert_eq!((counters.compared, counters.refused_by_admission), (0, 0));
}
