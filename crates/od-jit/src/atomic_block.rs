//! S6-41c: a native block of one atomic guest instruction with an inline memory operand.
//!
//! The block is the inline memory emitter that S6-41b left open (obligations 1 and 5 of S6-40):
//! the operand address is translated, permission-checked and aligned by a host helper, and the
//! atomic operation of `arm64::atomic_sequence` then runs on the host pointer the helper returned.
//! Everything the guest can observe happens in this order:
//!
//! 1. Guards (budget, STOP byte): the block leaves before any effect.
//! 2. The locate helper (`RuntimeContextV4::locate_helper`): the linear address is computed from
//!    the guest state, translated with the intent to write (the walks of the interpreter's locked
//!    operation, also for the unequal branch of `CMPXCHG`), checked for rights and alignment.
//!    A fault or a decline returns the block here, **before any effect**: the PC-map entry
//!    [`PcMapKind::BeforeEffects`] covers every word up to [`AtomicBlockLayout::effects_start`].
//! 3. The atomic sequence (`emit_atomic_program`): GPR reads, the host atomic operation, the six
//!    flags, GPR and RFLAGS writes. Nothing in it can fail (`VerifyError::AtomicRmwOrder`).
//! 4. Data, then `Release`: each page-generation counter the helper returned is raised with
//!    `LDADDAL` (acquire-release, not weaker than `Release`); then `committed_valid` is set, and
//!    the host reports `committed_write` after the block returned (S6-30b order: data,
//!    `Release` generation, report). The counters are raised also when `CMPXCHG` did not match:
//!    x86 writes the destination back, and the interpreter's compare reports a write then too.
//! 5. RIP, `remaining_budget`, `retired_instructions`, return `0`.
//!
//! The emitter emits instruction words only: no `unsafe`, no pointer arithmetic (the amendment
//! (в′) of S6-30a keeps the `unsafe` that touches RAM in `od-machine`'s `ram_storage.rs`).
//! The fences (`MFENCE`, `LFENCE`, `SFENCE`) have no memory operand and no helper.
//!
//! Profile: ONE guest instruction per block (S6-41d lifts it), and the session
//! (`PagedFetchedLoadSession`) does not admit these blocks yet (S6-41d).

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
    RUNTIME_CONTEXT_V4_COMMITTED_VALID_OFFSET, RUNTIME_CONTEXT_V4_GENERATION_COUNT_OFFSET,
    RUNTIME_CONTEXT_V4_GENERATION_OFFSET, RUNTIME_CONTEXT_V4_LOCATE_HELPER_OFFSET,
    RUNTIME_CONTEXT_V4_OPERAND_HOST_OFFSET, RuntimeExitTag,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomicBlockError {
    Encode(EncodeError),
    Sequence(SequenceError),
    InvalidProgram(ir::VerifyError),
    /// The profile is one guest instruction per block.
    NotOneInstruction,
    /// More than one atomic read-modify-write in the instruction.
    MoreThanOneMemoryEffect,
    /// The host lacks FEAT_LSE (S6-41b (л)): no inline atomic form is generated.
    LseUnavailable,
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
    /// and the retirement counters are as at entry. A fault or decline exit lies here.
    BeforeEffects,
    /// The instruction's effects have started (the atomic operation, flag and register writes,
    /// the generation raise, RIP). A host fault here is a host failure (S3-27 is open).
    InInstruction,
    /// The instruction is complete; only the epilogue is left.
    AfterInstruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PcMapEntry {
    pub guest_rip: u64,
    /// First host word (index into the block) of the range this entry covers.
    pub host_word: usize,
    pub kind: PcMapKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtomicBlockLayout {
    /// Host word of the `BLR` of the locate helper, when the instruction has a memory operand.
    pub helper_call_word: Option<usize>,
    /// First host word that may change guest-visible state; every earlier word is guards,
    /// the helper call and its check (stores through `x19` to the context are not guest state).
    pub effects_start: usize,
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

/// A compiled V4 block. The block always bumps the first generation counter of the runtime context
/// and the second one only when `generation_count` is at least 2; a `generation_count` of 0 is a
/// contract violation of the caller (the context must name at least the page of the data), not a
/// supported case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AtomicBlock {
    pub words: Vec<u32>,
    pub layout: AtomicBlockLayout,
    /// The site the helper reads; `None` for a fence (no memory operand, no helper).
    pub site: Option<AtomicSiteV4>,
    pub mode: MemoryOrderMode,
    pub guest_rip: u64,
    pub next_rip: u64,
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

/// Compile the one atomic guest instruction of `program` into a V4 block for `mode`.
pub fn compile_atomic_block(
    program: &Program,
    mode: MemoryOrderMode,
) -> Result<AtomicBlock, AtomicBlockError> {
    ir::verify(program).map_err(AtomicBlockError::InvalidProgram)?;
    if program.guest_instructions.len() != 1 {
        return Err(AtomicBlockError::NotOneInstruction);
    }
    if !mode_admitted_on_host(mode) {
        return Err(AtomicBlockError::LseUnavailable);
    }
    let instruction = program.guest_instructions[0];
    let mut memory = program
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

    let context = x(CONTEXT);
    let state = x(STATE);
    let scratch = x(9);
    let mut encoder = Encoder::new();
    let epilogue = encoder.new_label()?;
    let budget_exit = encoder.new_label()?;
    let stop_exit = encoder.new_label()?;
    let no_stop = encoder.new_label()?;

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
    // Counters as for an exit with no instruction retired (every guard and helper exit below).
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
    encoder.cbz_x(reg(scratch), budget_exit)?;
    // STOP byte (S3-28c guard before the instruction): a raised byte leaves before any effect.
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
    if operand.is_some() {
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
        program,
    )?;

    if operand.is_some() {
        // Data first (the sequence), then the Release of every touched page generation, then the
        // report flag. The second counter is raised only when the helper returned two.
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

    // Retirement: RIP, the budget, the count.
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
        RUNTIME_CONTEXT_INITIAL_BUDGET_OFFSET as u32,
    )?;
    encoder.subs_imm_x(reg(value), XRegOrSp::Reg(value), 1)?;
    encoder.str_x(
        reg(value),
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_REMAINING_BUDGET_OFFSET as u32,
    )?;
    encoder.movz_x(value, 1, 0)?;
    encoder.str_x(
        reg(value),
        AddressBase::Reg(context),
        RUNTIME_CONTEXT_RETIRED_INSTRUCTIONS_OFFSET as u32,
    )?;
    encoder.movz_x(x(0), RuntimeExitTag::BlockComplete as u16, 0)?;
    let after_instruction = encoder.position();

    // Epilogue and the two guard exits.
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

    let words = encoder.finish()?.into_words();
    let rip = instruction.guest_rip;
    let layout = AtomicBlockLayout {
        helper_call_word,
        effects_start,
        epilogue_word,
        pc_map: vec![
            PcMapEntry {
                guest_rip: rip,
                host_word: 0,
                kind: PcMapKind::BeforeEffects,
            },
            PcMapEntry {
                guest_rip: rip,
                host_word: effects_start,
                kind: PcMapKind::InInstruction,
            },
            PcMapEntry {
                guest_rip: instruction.next_rip,
                host_word: after_instruction,
                kind: PcMapKind::AfterInstruction,
            },
        ],
    };
    Ok(AtomicBlock {
        words,
        layout,
        site: operand.map(|operand| AtomicSiteV4 {
            guest_rip: instruction.guest_rip,
            next_rip: instruction.next_rip,
            operand,
        }),
        mode,
        guest_rip: instruction.guest_rip,
        next_rip: instruction.next_rip,
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
            let call = layout
                .helper_call_word
                .expect("a memory operand has a helper");
            assert!(call < layout.effects_start);
            for (index, word) in block.words[..layout.effects_start].iter().enumerate() {
                assert!(
                    !is_store_through(*word, STATE) && !is_store_through(*word, 1),
                    "word {index} {word:#010x} stores to guest state or the operand"
                );
                assert!(!is_lse_atomic(*word), "word {index} {word:#010x} is atomic");
            }
            // The atomic operation is after it (the sanity check of the classifier).
            assert!(
                block.words[layout.effects_start..]
                    .iter()
                    .any(|word| is_lse_atomic(*word)),
                "the classifier finds the atomic operation after the start of effects"
            );
            assert!(
                block.words[layout.effects_start..]
                    .iter()
                    .any(|word| is_store_through(*word, STATE))
            );
            for word in [0, call, layout.effects_start - 1] {
                let entry = layout.attribute(word, block.words.len()).unwrap();
                assert_eq!(
                    (entry.kind, entry.guest_rip),
                    (PcMapKind::BeforeEffects, RIP)
                );
            }
            let first = layout
                .attribute(layout.effects_start, block.words.len())
                .unwrap();
            assert_eq!(first.kind, PcMapKind::InInstruction);
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
        assert!(block.site.is_none());
        assert!(block.layout.helper_call_word.is_none());
        assert!(!block.words.iter().any(|word| is_lse_atomic(*word)));
        // DMB ISH is in the block.
        assert!(block.words.contains(&0xd503_3bbf));
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
        // An empty instruction compiles to guards and a retirement: it is not atomic, but it has
        // no effect the block could get wrong; the profile check is the guest instruction count.
        assert!(compile_atomic_block(&no_atomic, MemoryOrderMode::Parallel).is_ok());
    }
}
