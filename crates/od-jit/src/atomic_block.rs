//! S6-41c / S6-41d: a native block of atomic guest instructions with inline memory operands.
//!
//! The block is the inline memory emitter that S6-41b left open (obligations 1 and 5 of S6-40):
//! the operand address is translated, permission-checked and aligned by a host helper, and the
//! atomic operation of `arm64::atomic_sequence` then runs on the host pointer the helper returned.
//! S6-41c built the block of one guest instruction; S6-41d lets the block hold up to
//! [`MAX_ATOMIC_BLOCK_INSTRUCTIONS`] of them (atomic read-modify-write forms and fences, in any
//! order), each retiring on its own. Everything the guest can observe happens, for every
//! instruction of the block, in this order:
//!
//! 1. Guards (budget, STOP byte): the block leaves before any effect of the instruction.
//! 2. The locate helper (`RuntimeContextV4::locate_helper`): the linear address is computed from
//!    the guest state, translated with the intent to write (the walks of the interpreter's locked
//!    operation, also for the unequal branch of `CMPXCHG`), checked for rights and alignment.
//!    A fault or a decline returns the block here, **before any effect of this instruction**:
//!    the PC-map entry [`PcMapKind::BeforeEffects`] covers every word up to the instruction's
//!    [`AtomicInstructionLayout::effects_start`]. The instructions before it have retired in
//!    full (RIP, `remaining_budget`, `retired_instructions` are theirs), so the state at the exit
//!    is exactly the interpreter's state before the faulting instruction.
//! 3. The atomic sequence (`emit_atomic_program`): GPR reads, the host atomic operation, the six
//!    flags, GPR and RFLAGS writes. Nothing in it can fail (`VerifyError::AtomicRmwOrder`).
//! 4. Data, then `Release`: each page-generation counter the helper returned is raised with
//!    `LDADDAL` (acquire-release, not weaker than `Release`); then `committed_valid` is set, and
//!    the host reports `committed_write` after the block returned (S6-30b order: data,
//!    `Release` generation, report). The counters are raised also when `CMPXCHG` did not match:
//!    x86 writes the destination back, and the interpreter's compare reports a write then too.
//! 5. RIP, `remaining_budget -= 1`, `retired_instructions += 1`. If the helper noted that the
//!    walk or operand overlaps the guarded code range of the block (`RuntimeContextV4::code_dirty`), the block
//!    exits [`RUNTIME_EXIT_V4_CODE_INVALIDATED`] here: the instruction retired, the later ones of
//!    the block did not run (S4-16i, the exit of the store into the own code).
//!
//! After the last instruction: return `0`.
//!
//! The emitter emits instruction words only: no `unsafe`, no pointer arithmetic (the amendment
//! (в′) of S6-30a keeps the `unsafe` that touches RAM in `od-machine`'s `ram_storage.rs`).
//! The fences (`MFENCE`, `LFENCE`, `SFENCE`) have no memory operand and no helper.
//!
//! Profile (S6-41d): atomic read-modify-write forms and fences only. An ordinary memory effect
//! (`CheckedLoad`, `CheckedStore`) or an integer effect in the program is refused by the sequence
//! emitter (`SequenceError::UnsupportedEffect`): the V2 helper of ordinary accesses and the V4
//! helper are different ABIs, and ordinary guest instructions stay with the interpreter next to
//! the block. The general block emitter and its register allocator keep their explicit refusal
//! of the atomic effects (S6-41b (е)): this module has its own register plan.

use crate::arm64::atomic_sequence::{SequenceEnv, SequenceError, emit_atomic_program, materialize};
use crate::arm64::memory_order::{MemoryOrderMode, mode_admitted_on_host};
use crate::arm64::{
    AccessWidth, AddressBase, AtomicOrder, Condition, EncodeError, Encoder, PairIndexMode, XReg,
    XRegOrSp, XRegOrZero,
};
use crate::ir::{self, Operation, Program};
use crate::runtime::{
    AtomicSiteV4, LONG_MODE_RIP_OFFSET, RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET,
    RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET, RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET,
    RUNTIME_CONTEXT_STATE_OFFSET, RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET,
    RUNTIME_CONTEXT_V4_CODE_DIRTY_OFFSET, RUNTIME_CONTEXT_V4_COMMITTED_VALID_OFFSET,
    RUNTIME_CONTEXT_V4_GENERATION_COUNT_OFFSET, RUNTIME_CONTEXT_V4_GENERATION_OFFSET,
    RUNTIME_CONTEXT_V4_LOCATE_HELPER_OFFSET, RUNTIME_CONTEXT_V4_OPERAND_HOST_OFFSET,
    RUNTIME_CONTEXT_V4_SITE_INDEX_OFFSET, RUNTIME_EXIT_V4_CODE_INVALIDATED, RuntimeExitTag,
};

/// The most guest instructions one atomic block holds (S6-41d). A bound of the profile, not a
/// measured value: the S6-38 measurements decide whether it should change.
pub const MAX_ATOMIC_BLOCK_INSTRUCTIONS: usize = 16;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomicBlockError {
    Encode(EncodeError),
    Sequence(SequenceError),
    InvalidProgram(ir::VerifyError),
    /// The program has no guest instruction, or more than
    /// [`MAX_ATOMIC_BLOCK_INSTRUCTIONS`].
    InstructionCount {
        count: usize,
    },
    /// More than one atomic read-modify-write in one guest instruction.
    MoreThanOneMemoryEffect,
    /// An instruction in the block has neither an atomic operation nor a fence.
    MissingAtomicEffect {
        rip: u64,
    },
    /// The host lacks FEAT_LSE (S6-41b (л)): no inline atomic form is generated.
    LseUnavailable,
    /// A 16-bit atomic operand: the lowering accepts the width but no differential covers it
    /// (the decoder of `od_isa` produces no `LOCK ... m16`, S6-41b (г)), so the block refuses it
    /// explicitly rather than run code nobody has compared with the interpreter.
    UnsupportedWidth16,
}

impl From<EncodeError> for AtomicBlockError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

impl From<SequenceError> for AtomicBlockError {
    fn from(error: SequenceError) -> Self {
        Self::Sequence(error)
    }
}

/// What a host word of the block means for the guest state (the PC-map of the block).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PcMapKind {
    /// No guest-visible effect of the instruction has happened: registers, flags, RIP, memory
    /// and the retirement counters are those of the retired instructions before it. A fault or
    /// decline exit lies here.
    BeforeEffects,
    /// The instruction's effects have started (the atomic operation, flag and register writes,
    /// the generation raise, RIP). A host fault here is a host failure (S3-27 is open).
    InInstruction,
    /// Every instruction is complete; only the epilogue is left.
    AfterInstruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PcMapEntry {
    pub guest_rip: u64,
    /// First host word (index into the block) of the range this entry covers.
    pub host_word: usize,
    pub kind: PcMapKind,
}

/// The words of one guest instruction of the block.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AtomicInstructionLayout {
    pub guest_rip: u64,
    pub next_rip: u64,
    /// First host word of the instruction (its guards).
    pub guard_start: usize,
    /// Host word of the `BLR` of the locate helper, when the instruction has a memory operand.
    pub helper_call_word: Option<usize>,
    /// First host word that may change guest-visible state; every earlier word of the
    /// instruction is guards, the helper call and its check (stores through `x19` to the context
    /// are not guest state).
    pub effects_start: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtomicBlockLayout {
    /// One entry per guest instruction, in program order.
    pub instructions: Vec<AtomicInstructionLayout>,
    /// First host word of the epilogue (restore and return).
    pub epilogue_word: usize,
    /// Ranges in order of `host_word`; the first covers word 0.
    pub pc_map: Vec<PcMapEntry>,
}

impl AtomicBlockLayout {
    /// The guest RIP and the state of the instruction at host word `word` of the block, or
    /// `None` outside the block.
    pub fn attribute(&self, word: usize, block_words: usize) -> Option<PcMapEntry> {
        if word >= block_words {
            return None;
        }
        self.pc_map
            .iter()
            .rev()
            .find(|entry| entry.host_word <= word)
            .copied()
    }
}

/// A compiled V4 block. For every memory instruction the block bumps the first generation counter
/// of the runtime context and the second one only when `generation_count` is at least 2; a
/// `generation_count` of 0 is a contract violation of the caller (the context must name at least
/// the page of the data), not a supported case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtomicBlock {
    pub words: Vec<u32>,
    pub layout: AtomicBlockLayout,
    /// The sites the helper reads, one per memory instruction in program order (the block stores
    /// the index before each call); empty when the block holds fences only.
    pub sites: Vec<AtomicSiteV4>,
    pub mode: MemoryOrderMode,
    pub guest_rip: u64,
    /// The RIP after the last instruction of the block.
    pub next_rip: u64,
}

impl AtomicBlock {
    pub fn instruction_count(&self) -> usize {
        self.layout.instructions.len()
    }
}

fn x(number: u8) -> XReg {
    XReg::new(number).expect("register plan uses registers below 31")
}

fn reg(register: XReg) -> XRegOrZero {
    XRegOrZero::Reg(register)
}

// Register plan: x19 = context, x20 = guest state (callee-saved, preserved by the helper call
// and untouched by the sequence, which clobbers x2..=x16), x0/x1 scratch outside the sequence.
const CONTEXT: u8 = 19;
const STATE: u8 = 20;

/// One guest instruction of `program` as a program of its own (index 0, an `Exit` at its next RIP).
fn instruction_programs(program: &Program) -> Vec<Program> {
    let mut parts = Vec::new();
    let mut current: Option<Vec<Operation>> = None;
    for operation in &program.operations {
        match operation {
            Operation::BeginInstruction { .. } => {
                current = Some(vec![Operation::BeginInstruction { index: 0 }]);
            }
            Operation::CommitInstruction { .. } => {
                if let Some(mut operations) = current.take() {
                    let index = parts.len();
                    let instruction = program.guest_instructions[index];
                    operations.push(Operation::CommitInstruction { index: 0 });
                    operations.push(Operation::Exit {
                        kind: ir::ExitKind::BlockComplete,
                        target_rip: instruction.next_rip,
                    });
                    parts.push(Program {
                        guest_instructions: vec![instruction],
                        operations,
                        code_end: instruction.next_rip,
                    });
                }
            }
            Operation::Exit { .. } => {}
            other => {
                if let Some(operations) = current.as_mut() {
                    operations.push(other.clone());
                }
            }
        }
    }
    parts
}

/// Compile the atomic guest instructions of `program` (one or up to
/// [`MAX_ATOMIC_BLOCK_INSTRUCTIONS`], contiguous) into a V4 block for `mode`.
pub fn compile_atomic_block(
    program: &Program,
    mode: MemoryOrderMode,
) -> Result<AtomicBlock, AtomicBlockError> {
    compile_atomic_block_with(program, mode, &mode_admitted_on_host)
}

/// [`compile_atomic_block`] with the host policy (S6-41b (л), `mode_admitted_on_host`) given by the
/// caller, so a test can stand for a host without FEAT_LSE. The policy is asked once, before any
/// code is generated: a mode it refuses never produces a block (S6-41d: the cache of a `Parallel`
/// session asks it when the block is created).
pub fn compile_atomic_block_with(
    program: &Program,
    mode: MemoryOrderMode,
    mode_admitted: &dyn Fn(MemoryOrderMode) -> bool,
) -> Result<AtomicBlock, AtomicBlockError> {
    ir::verify(program).map_err(AtomicBlockError::InvalidProgram)?;
    let count = program.guest_instructions.len();
    if count == 0 || count > MAX_ATOMIC_BLOCK_INSTRUCTIONS {
        return Err(AtomicBlockError::InstructionCount { count });
    }
    if !mode_admitted(mode) {
        return Err(AtomicBlockError::LseUnavailable);
    }
    let parts = instruction_programs(program);
    debug_assert_eq!(
        parts.len(),
        count,
        "a verified program commits every instruction"
    );
    let mut operands = Vec::with_capacity(count);
    for part in &parts {
        if part.operations.iter().any(|operation| {
            matches!(
                operation,
                Operation::AtomicRmw {
                    width: ir::IntWidth::W16,
                    ..
                } | Operation::AtomicFlags {
                    width: ir::IntWidth::W16,
                    ..
                }
            )
        }) {
            return Err(AtomicBlockError::UnsupportedWidth16);
        }
        let mut memory = part
            .operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::AtomicRmw { operand, .. } => Some(*operand),
                _ => None,
            });
        let operand = memory.next();
        if memory.next().is_some() {
            return Err(AtomicBlockError::MoreThanOneMemoryEffect);
        }
        if operand.is_none()
            && !part
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Fence { .. }))
        {
            return Err(AtomicBlockError::MissingAtomicEffect {
                rip: part.guest_instructions[0].guest_rip,
            });
        }
        operands.push(operand);
    }

    let context = x(CONTEXT);
    let state = x(STATE);
    let scratch = x(9);
    let mut encoder = Encoder::new();
    let epilogue = encoder.new_label()?;
    let budget_exit = encoder.new_label()?;
    let stop_exit = encoder.new_label()?;
    let code_exit = encoder.new_label()?;

    // Prologue: frame for x29/x30 (the helper call clobbers x30) and the two callee-saved
    // registers the block keeps its context and state in.
    encoder.stp_x(x(29), x(30), AddressBase::Sp, -32, PairIndexMode::PreIndex)?;
    encoder.stp_x(context, state, AddressBase::Sp, 16, PairIndexMode::Offset)?;
    encoder.mov_x(context, reg(x(0)));
    encoder.ldr_x(
        reg(state),
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_STATE_OFFSET as u32,
    )?;
    // Counters as for an exit with no instruction retired; each retired instruction below moves
    // them, so every guard and helper exit reports exactly the instructions retired before it.
    encoder.ldr_x(
        reg(scratch),
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        reg(scratch),
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.str_x(
        XRegOrZero::Zero,
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;

    let mut instructions = Vec::with_capacity(count);
    let mut sites = Vec::new();
    let mut pc_map = Vec::with_capacity(2 * count + 1);
    for (part, operand) in parts.iter().zip(&operands) {
        let instruction = part.guest_instructions[0];
        let guard_start = encoder.position();

        // Guard on the budget left (S3-28c: before the instruction, no effect).
        encoder.ldr_x(
            reg(scratch),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
        )?;
        encoder.cbz_x(reg(scratch), budget_exit)?;
        // STOP byte: a raised byte leaves before any effect of the instruction.
        let no_stop = encoder.new_label()?;
        encoder.ldr_x(
            reg(scratch),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_STOP_REQUESTED_OFFSET as u32,
        )?;
        encoder.cbz_x(reg(scratch), no_stop)?;
        encoder.ldr_sized(
            AccessWidth::Byte,
            reg(scratch),
            AddressBase::Reg(scratch),
            0,
        )?;
        encoder.cbnz_x(reg(scratch), stop_exit)?;
        encoder.bind(no_stop)?;

        // The helper: translate, check, return the host pointer, or leave with its exit value.
        let mut helper_call_word = None;
        if let Some(operand) = operand {
            let site_index = sites.len();
            sites.push(AtomicSiteV4 {
                guest_rip: instruction.guest_rip,
                next_rip: instruction.next_rip,
                operand: *operand,
            });
            encoder.movz_x(x(1), site_index as u16, 0)?;
            encoder.str_sized(
                AccessWidth::Word,
                reg(x(1)),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_SITE_INDEX_OFFSET as u32,
            )?;
            encoder.ldr_x(
                reg(scratch),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_LOCATE_HELPER_OFFSET as u32,
            )?;
            encoder.mov_x(x(0), reg(context));
            helper_call_word = Some(encoder.position());
            encoder.blr(scratch);
            // w0 != 0: a V4 exit value (fault, decline, failure); the block returns it untouched.
            encoder.cbnz_w(crate::arm64::WReg::new(0)?, epilogue)?;
            encoder.ldr_x(
                reg(x(1)),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_OPERAND_HOST_OFFSET as u32,
            )?;
        }
        let effects_start = encoder.position();

        // The atomic instruction itself. `address` is x1: outside the sequence's x2..=x16.
        emit_atomic_program(
            &mut encoder,
            &SequenceEnv {
                mode,
                state,
                address: x(1),
            },
            part,
        )?;

        if operand.is_some() {
            // Data first (the sequence), then the Release of every touched page generation, then
            // the report flag. The second counter is raised only when the helper returned two.
            let one = x(2);
            let counter = x(1);
            let old = x(3);
            let count = x(4);
            let single = encoder.new_label()?;
            encoder.movz_x(one, 1, 0)?;
            encoder.ldr_x(
                reg(counter),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_GENERATION_OFFSET as u32,
            )?;
            encoder.ldadd(
                AccessWidth::Dword,
                AtomicOrder::AcquireRelease,
                reg(one),
                old,
                AddressBase::Reg(counter),
            );
            encoder.ldr_sized(
                AccessWidth::Word,
                reg(count),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_GENERATION_COUNT_OFFSET as u32,
            )?;
            encoder.subs_imm_x(XRegOrZero::Zero, XRegOrSp::Reg(count), 2)?;
            encoder.b_cond(Condition::Lower, single)?;
            encoder.ldr_x(
                reg(counter),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_GENERATION_OFFSET as u32 + 8,
            )?;
            encoder.ldadd(
                AccessWidth::Dword,
                AtomicOrder::AcquireRelease,
                reg(one),
                old,
                AddressBase::Reg(counter),
            );
            encoder.bind(single)?;
            encoder.str_sized(
                AccessWidth::Word,
                reg(one),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_COMMITTED_VALID_OFFSET as u32,
            )?;
        }

        // Retirement: RIP, the budget left, the count.
        let value = x(2);
        materialize(&mut encoder, value, instruction.next_rip)?;
        encoder.str_x(
            reg(value),
            AddressBase::Reg(state),
            LONG_MODE_RIP_OFFSET as u32,
        )?;
        encoder.ldr_x(
            reg(value),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
        )?;
        encoder.subs_imm_x(reg(value), XRegOrSp::Reg(value), 1)?;
        encoder.str_x(
            reg(value),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
        )?;
        encoder.ldr_x(
            reg(value),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
        )?;
        encoder.adds_imm_x(reg(value), XRegOrSp::Reg(value), 1)?;
        encoder.str_x(
            reg(value),
            AddressBase::Reg(context),
            RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
        )?;
        if operand.is_some() {
            // The helper noted an overlap with the guarded code range of this block: the instruction has
            // retired, the later ones must not run from bytes that may be stale.
            encoder.ldr_sized(
                AccessWidth::Word,
                reg(value),
                AddressBase::Reg(context),
                RUNTIME_CONTEXT_V4_CODE_DIRTY_OFFSET as u32,
            )?;
            encoder.cbnz_x(reg(value), code_exit)?;
        }

        pc_map.push(PcMapEntry {
            guest_rip: instruction.guest_rip,
            // The prologue belongs to the first instruction: the first entry covers word 0.
            host_word: if instructions.is_empty() {
                0
            } else {
                guard_start
            },
            kind: PcMapKind::BeforeEffects,
        });
        pc_map.push(PcMapEntry {
            guest_rip: instruction.guest_rip,
            host_word: effects_start,
            kind: PcMapKind::InInstruction,
        });
        instructions.push(AtomicInstructionLayout {
            guest_rip: instruction.guest_rip,
            next_rip: instruction.next_rip,
            guard_start,
            helper_call_word,
            effects_start,
        });
    }

    let last = *program
        .guest_instructions
        .last()
        .expect("the instruction count was checked");
    encoder.movz_x(x(0), RuntimeExitTag::BlockComplete as u16, 0)?;
    let after_instruction = encoder.position();
    pc_map.push(PcMapEntry {
        guest_rip: last.next_rip,
        host_word: after_instruction,
        kind: PcMapKind::AfterInstruction,
    });

    // Epilogue and the guard exits.
    encoder.bind(epilogue)?;
    let epilogue_word = encoder.position();
    encoder.ldp_x(context, state, AddressBase::Sp, 16, PairIndexMode::Offset)?;
    encoder.ldp_x(x(29), x(30), AddressBase::Sp, 32, PairIndexMode::PostIndex)?;
    encoder.ret(x(30));
    encoder.bind(budget_exit)?;
    encoder.movz_x(x(0), RuntimeExitTag::BudgetExhausted as u16, 0)?;
    encoder.b(epilogue)?;
    encoder.bind(stop_exit)?;
    encoder.movz_x(x(0), RuntimeExitTag::StopRequested as u16, 0)?;
    encoder.b(epilogue)?;
    encoder.bind(code_exit)?;
    encoder.movz_x(x(0), RUNTIME_EXIT_V4_CODE_INVALIDATED as u16, 0)?;
    encoder.b(epilogue)?;

    let words = encoder.finish()?.into_words();
    let first = program.guest_instructions[0];
    Ok(AtomicBlock {
        words,
        layout: AtomicBlockLayout {
            instructions,
            epilogue_word,
            pc_map,
        },
        sites,
        mode,
        guest_rip: first.guest_rip,
        next_rip: last.next_rip,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{
        AtomicFlagsKind, AtomicRmwKind, FenceKind, GuestGpr, GuestInstruction, IntWidth, TempId,
    };
    use od_isa::OperandSize;
    use od_isa::long_decode::{AddressBase as GuestBase, MemoryOperand};

    const RIP: u64 = 0x2000;

    fn lock_add_m32_r32() -> Program {
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: RIP,
                next_rip: RIP + 4,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(0),
                    register: GuestGpr(3),
                    width: IntWidth::W32,
                },
                Operation::AtomicRmw {
                    kind: AtomicRmwKind::Add,
                    width: IntWidth::W32,
                    operand: MemoryOperand {
                        base: GuestBase::None,
                        index: None,
                        scale: 1,
                        displacement: 0x1000,
                        address_size: OperandSize::Qword,
                        size: OperandSize::Dword,
                    },
                    value: TempId(0),
                    expected: None,
                    dst: TempId(1),
                },
                Operation::AtomicFlags {
                    kind: AtomicFlagsKind::Add,
                    width: IntWidth::W32,
                    lhs: TempId(1),
                    rhs: Some(TempId(0)),
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ir::ExitKind::BlockComplete,
                    target_rip: RIP + 4,
                },
            ],
            code_end: RIP + 4,
        }
    }

    fn fence() -> Program {
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: RIP,
                next_rip: RIP + 3,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::Fence {
                    kind: FenceKind::Full,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ir::ExitKind::BlockComplete,
                    target_rip: RIP + 3,
                },
            ],
            code_end: RIP + 3,
        }
    }

    fn is_store_through(word: u32, base: u8) -> bool {
        // STR/STRB/STRH/STR (unsigned immediate): size|111|0|01|00 ; STLR/STUR/STP are not used
        // by the block before `effects_start`; any store class with Rn == base is flagged.
        let rn = ((word >> 5) & 0x1f) as u8;
        let unsigned_store = (word & 0x3f40_0000) == 0x3900_0000 && (word & 0x0400_0000) == 0;
        let store_pair = (word & 0x3e40_0000) == 0x2800_0000;
        (unsigned_store || store_pair) && rn == base
    }

    fn is_lse_atomic(word: u32) -> bool {
        // LDADD/LDCLR/LDSET/SWP: size|111|0|00|AR|1|Rs|o3|opc|00|Rn|Rt
        (word & 0x3f20_0c00) == 0x3820_0000
            // CAS: size|0010001|L|1|Rs|o0|11111|Rn|Rt
            || (word & 0x3fa0_7c00) == 0x0880_7c00
    }

    /// The fault exit of the helper (and the guard exits) are before every effect: no store
    /// through the guest state or the operand register, and no atomic instruction, lies before
    /// `effects_start`; the helper call is before it; the PC-map says "before effects" there.
    #[test]
    fn the_exits_of_the_helper_lie_before_every_effect_of_the_instruction() {
        for mode in [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel] {
            let block = compile_atomic_block(&lock_add_m32_r32(), mode).unwrap();
            let layout = &block.layout;
            let first = layout.instructions[0];
            let call = first
                .helper_call_word
                .expect("a memory operand has a helper");
            let effects_start = first.effects_start;
            assert!(call < effects_start);
            for (index, word) in block.words[..effects_start].iter().enumerate() {
                assert!(
                    !is_store_through(*word, STATE) && !is_store_through(*word, 1),
                    "word {index} {word:#010x} stores to guest state or the operand"
                );
                assert!(!is_lse_atomic(*word), "word {index} {word:#010x} is atomic");
            }
            // The atomic operation is after it (the sanity check of the classifier).
            assert!(
                block.words[effects_start..]
                    .iter()
                    .any(|word| is_lse_atomic(*word)),
                "the classifier finds the atomic operation after the start of effects"
            );
            assert!(
                block.words[effects_start..]
                    .iter()
                    .any(|word| is_store_through(*word, STATE))
            );
            for word in [0, call, effects_start - 1] {
                let entry = layout.attribute(word, block.words.len()).unwrap();
                assert_eq!(
                    (entry.kind, entry.guest_rip),
                    (PcMapKind::BeforeEffects, RIP)
                );
            }
            let in_instruction = layout.attribute(effects_start, block.words.len()).unwrap();
            assert_eq!(in_instruction.kind, PcMapKind::InInstruction);
            let last = layout
                .attribute(layout.epilogue_word, block.words.len())
                .unwrap();
            assert_eq!(
                (last.kind, last.guest_rip),
                (PcMapKind::AfterInstruction, RIP + 4)
            );
            assert!(
                layout
                    .attribute(block.words.len(), block.words.len())
                    .is_none()
            );
        }
    }

    /// The release that publishes the page generation is an acquire-release read-modify-write in
    /// both modes, and the data word comes before it (the order of S6-30b).
    #[test]
    fn the_generation_is_raised_after_the_data_word_with_release_in_both_modes() {
        for mode in [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel] {
            let block = compile_atomic_block(&lock_add_m32_r32(), mode).unwrap();
            let atomics: Vec<usize> = block
                .words
                .iter()
                .enumerate()
                .filter(|(_, word)| is_lse_atomic(**word))
                .map(|(index, _)| index)
                .collect();
            // The data operation, then two guarded generation raises (counter 0 and counter 1).
            assert_eq!(atomics.len(), 3, "{mode:?}: {atomics:?}");
            for &index in &atomics[1..] {
                let word = block.words[index];
                // LDADDAL X: size 3, acquire and release bits (23, 22) set.
                assert_eq!(word & 0xffe0_fc00, 0xf8e0_0000, "{mode:?} word {index}");
            }
            let data = block.words[atomics[0]];
            let ordering_bits = (data >> 22) & 3;
            assert_eq!(
                ordering_bits,
                if mode == MemoryOrderMode::Parallel {
                    3
                } else {
                    0
                }
            );
        }
    }

    #[test]
    fn a_fence_block_has_no_helper_and_no_generation_raise() {
        let block = compile_atomic_block(&fence(), MemoryOrderMode::Parallel).unwrap();
        assert!(block.sites.is_empty());
        assert!(block.layout.instructions[0].helper_call_word.is_none());
        assert!(!block.words.iter().any(|word| is_lse_atomic(*word)));
        // DMB ISH is in the block.
        assert!(block.words.contains(&0xd503_3bbf));
    }

    /// A program of `kinds.len()` guest instructions: `true` is `LOCK ADD m32, ebx` (4 bytes) with
    /// the displacement `0x1000 + 0x10 * index`, `false` is `MFENCE` (3 bytes). Temporaries are
    /// numbered across the program.
    fn sequence(kinds: &[bool]) -> Program {
        let mut guest_instructions = Vec::new();
        let mut operations = Vec::new();
        let mut rip = RIP;
        for (index, &memory) in kinds.iter().enumerate() {
            let length = if memory { 4 } else { 3 };
            guest_instructions.push(GuestInstruction {
                guest_rip: rip,
                next_rip: rip + length,
            });
            operations.push(Operation::BeginInstruction { index });
            if memory {
                let value = TempId(2 * index as u32);
                let old = TempId(2 * index as u32 + 1);
                operations.push(Operation::ReadGpr {
                    dst: value,
                    register: GuestGpr(3),
                    width: IntWidth::W32,
                });
                operations.push(Operation::AtomicRmw {
                    kind: AtomicRmwKind::Add,
                    width: IntWidth::W32,
                    operand: MemoryOperand {
                        base: GuestBase::None,
                        index: None,
                        scale: 1,
                        displacement: 0x1000 + 0x10 * index as i64,
                        address_size: OperandSize::Qword,
                        size: OperandSize::Dword,
                    },
                    value,
                    expected: None,
                    dst: old,
                });
                operations.push(Operation::AtomicFlags {
                    kind: AtomicFlagsKind::Add,
                    width: IntWidth::W32,
                    lhs: old,
                    rhs: Some(value),
                });
            } else {
                operations.push(Operation::Fence {
                    kind: FenceKind::Full,
                });
            }
            operations.push(Operation::CommitInstruction { index });
            rip += length;
        }
        operations.push(Operation::Exit {
            kind: ir::ExitKind::BlockComplete,
            target_rip: rip,
        });
        Program {
            guest_instructions,
            operations,
            code_end: rip,
        }
    }

    /// S6-41d: a block of several instructions has a site per memory instruction (none for the
    /// fence), a layout and a PC-map entry per instruction, and the retirement of the instruction
    /// before it is in the words that precede the guards of the next one.
    #[test]
    fn s6_41d_a_block_of_three_instructions_has_one_site_and_one_layout_entry_each() {
        for mode in [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel] {
            let block = compile_atomic_block(&sequence(&[true, false, true]), mode).unwrap();
            assert_eq!(block.instruction_count(), 3);
            assert_eq!(block.sites.len(), 2, "the fence has no site");
            assert_eq!(block.sites[0].operand.displacement, 0x1000);
            assert_eq!(block.sites[1].operand.displacement, 0x1020);
            assert_eq!((block.guest_rip, block.next_rip), (RIP, RIP + 11));
            let layout = &block.layout;
            let calls: Vec<bool> = layout
                .instructions
                .iter()
                .map(|instruction| instruction.helper_call_word.is_some())
                .collect();
            assert_eq!(calls, [true, false, true]);
            let mut previous = 0;
            for (index, instruction) in layout.instructions.iter().enumerate() {
                assert!(instruction.guard_start >= previous, "instruction {index}");
                assert!(instruction.guard_start < instruction.effects_start);
                previous = instruction.effects_start;
                // Before the effects of every instruction: no guest-state store, no atomic.
                for (word_index, word) in block.words
                    [instruction.guard_start..instruction.effects_start]
                    .iter()
                    .enumerate()
                {
                    assert!(
                        !is_store_through(*word, STATE) && !is_store_through(*word, 1),
                        "instruction {index} word {word_index} {word:#010x} stores to the guest"
                    );
                    assert!(
                        !is_lse_atomic(*word),
                        "instruction {index} word {word_index}"
                    );
                }
                let before = layout
                    .attribute(instruction.guard_start, block.words.len())
                    .unwrap();
                assert_eq!(
                    (before.kind, before.guest_rip),
                    (PcMapKind::BeforeEffects, instruction.guest_rip)
                );
                let inside = layout
                    .attribute(instruction.effects_start, block.words.len())
                    .unwrap();
                assert_eq!(
                    (inside.kind, inside.guest_rip),
                    (PcMapKind::InInstruction, instruction.guest_rip)
                );
            }
            assert_eq!(layout.instructions[1].guest_rip, RIP + 4);
            assert_eq!(layout.instructions[2].guest_rip, RIP + 7);
            // Every memory instruction raises the generation after its own data word: three
            // atomics per memory instruction (the data word and the two guarded raises).
            let atomics = block
                .words
                .iter()
                .filter(|word| is_lse_atomic(**word))
                .count();
            assert_eq!(atomics, 6, "{mode:?}");
        }
    }

    /// The profile of S6-41d: atomic forms and fences only, at most
    /// `MAX_ATOMIC_BLOCK_INSTRUCTIONS` of them, one memory effect per instruction.
    #[test]
    fn s6_41d_the_profile_refuses_ordinary_memory_effects_and_oversized_blocks() {
        let too_many = vec![false; MAX_ATOMIC_BLOCK_INSTRUCTIONS + 1];
        assert!(matches!(
            compile_atomic_block(&sequence(&too_many), MemoryOrderMode::Interleaved),
            Err(AtomicBlockError::InstructionCount { count }) if count == too_many.len()
        ));
        let largest = vec![false; MAX_ATOMIC_BLOCK_INSTRUCTIONS];
        assert!(compile_atomic_block(&sequence(&largest), MemoryOrderMode::Interleaved).is_ok());

        // An ordinary load next to an atomic instruction: refused, not silently dropped.
        let mut mixed = sequence(&[true]);
        let at = mixed
            .operations
            .iter()
            .position(|operation| matches!(operation, Operation::AtomicFlags { .. }))
            .unwrap();
        mixed.operations.insert(
            at + 1,
            Operation::CheckedLoad {
                dst: TempId(9),
                operand: MemoryOperand {
                    base: GuestBase::None,
                    index: None,
                    scale: 1,
                    displacement: 0x1000,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Qword,
                },
            },
        );
        // The IR itself rejects a fallible effect after the atomic operation (`AtomicRmwOrder`).
        assert!(matches!(
            compile_atomic_block(&mixed, MemoryOrderMode::Interleaved),
            Err(AtomicBlockError::InvalidProgram(_))
        ));
    }

    /// S6-41d: the host policy is asked when the block is created, for both modes (every atomic
    /// form is a FEAT_LSE instruction, also in the interleaving mode); a refusing policy yields
    /// no block, and the real policy of this host admits the modes it can run.
    #[test]
    fn s6_41d_the_host_policy_is_asked_when_the_block_is_created() {
        let program = sequence(&[true, false]);
        for mode in [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel] {
            let asked = std::cell::RefCell::new(Vec::new());
            let refusing = |mode| {
                asked.borrow_mut().push(mode);
                false
            };
            assert_eq!(
                compile_atomic_block_with(&program, mode, &refusing),
                Err(AtomicBlockError::LseUnavailable)
            );
            assert_eq!(
                *asked.borrow(),
                [mode],
                "asked once, for the mode of the block"
            );
            let accepting = |_| true;
            assert!(compile_atomic_block_with(&program, mode, &accepting).is_ok());
            // The production entry point follows `mode_admitted_on_host`.
            assert_eq!(
                compile_atomic_block(&program, mode).is_ok(),
                mode_admitted_on_host(mode)
            );
        }
    }

    /// S6-41d: width 16 is refused explicitly (no decoded form, no differential).
    #[test]
    fn s6_41d_a_sixteen_bit_atomic_operand_is_refused_explicitly() {
        let mut program = sequence(&[true]);
        for operation in &mut program.operations {
            match operation {
                Operation::ReadGpr { width, .. } => *width = IntWidth::W16,
                Operation::AtomicRmw { width, operand, .. } => {
                    *width = IntWidth::W16;
                    operand.size = OperandSize::Word;
                }
                Operation::AtomicFlags { width, .. } => *width = IntWidth::W16,
                _ => {}
            }
        }
        assert_eq!(
            compile_atomic_block(&program, MemoryOrderMode::Interleaved),
            Err(AtomicBlockError::UnsupportedWidth16)
        );
    }

    #[test]
    fn programs_outside_the_profile_are_refused() {
        let mut two = lock_add_m32_r32();
        two.guest_instructions.push(GuestInstruction {
            guest_rip: RIP + 4,
            next_rip: RIP + 8,
        });
        assert!(compile_atomic_block(&two, MemoryOrderMode::Parallel).is_err());
        let no_atomic = Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: RIP,
                next_rip: RIP + 1,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ir::ExitKind::BlockComplete,
                    target_rip: RIP + 1,
                },
            ],
            code_end: RIP + 1,
        };
        let mut mixed = sequence(&[true, false, true]);
        mixed
            .operations
            .retain(|operation| !matches!(operation, Operation::Fence { .. }));
        let middle = mixed.guest_instructions[1].guest_rip;
        for mode in [MemoryOrderMode::Interleaved, MemoryOrderMode::Parallel] {
            for (program, rip) in [(&no_atomic, RIP), (&mixed, middle)] {
                assert_eq!(
                    compile_atomic_block_with(program, mode, &|_| true),
                    Err(AtomicBlockError::MissingAtomicEffect { rip })
                );
            }
        }
    }
}
