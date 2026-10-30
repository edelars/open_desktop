//! Lower decoded S0-09 guest records into the linear typed IR.
//!
//! This module maps decoder effects; it does not decode x86 again or define
//! arithmetic/flags semantics. Each included guest instruction has one
//! commit marker. For a terminal Jcc, that marker and the immediately
//! following `Branch` together represent retirement and target selection;
//! no observable exit or helper may be inserted between them.

use od_isa::RegisterView;
use od_isa::long_decode::{
    ArithmeticKind, ConditionCode, Instruction, LogicKind, Operand, Operation as DecodedOperation,
};

use crate::block::{
    self, AdmittedInstructionBoundaries, BlockStopReason, BranchTargetResolution, BuildOutcome,
    DecodeBoundary, DecodedInstruction, S0_09NopAddRaxImm8Admission, S0_09NopAdmission,
    S0_09NopCmpRaxImm8Admission, S0_09NopDecRaxAdmission, S0_09NopIncRaxAdmission,
    S0_09NopLeaRaxRbxAdmission, S0_09NopLeaRaxRbxDisp8Admission, S0_09NopLeaRaxRbxRcxAdmission,
    S0_09NopMovRaxImm32Admission, S0_09NopNegRaxAdmission, S0_09NopNotRaxAdmission,
    S0_09NopSubRaxImm8Admission, S0_09NopXchgRaxRbxAdmission,
};
use crate::ir::{
    self, BranchCondition, ExitKind, FlagEffect, GuestGpr, GuestInstruction, IntWidth, IntegerKind,
    Operation, Program, TempId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoweredProgram {
    pub program: Program,
    /// Decode boundary is deferred metadata, not a guest fault or code-end.
    pub deferred_error: Option<DecodeBoundary>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoweringOutcome {
    Program(LoweredProgram),
    /// No guest instruction can be emitted. The caller must defer observation
    /// to dispatch/interpreter; this is not an empty or successful program.
    NoNativePrefix {
        rip: u64,
        reason: BlockStopReason,
        deferred_error: Option<DecodeBoundary>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LoweringError {
    InvalidDecodedForm { guest_rip: u64 },
    Verification(ir::VerifyError),
}

/// Lower one bounded block-builder result and verify every emitted Program.
pub fn lower(outcome: &BuildOutcome) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome, None, None, None, None, false, false, None, None, None, None, None,
    )
}

/// Lower a block for a future dispatcher using complete fixture-v1 decode
/// boundaries. Unlike `lower`, an admitted Jcc commits and returns a typed
/// dispatch candidate; unresolved targets absent from the proof remain
/// non-retired `NeedInterpreter` exits.
pub(crate) fn lower_for_dispatch(
    outcome: &BuildOutcome,
    admission: &AdmittedInstructionBoundaries,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
    )
}

/// Lower a bounded block for the separately admitted S0-09-plus-bare-NOP
/// profile. Only an admitted exact NOP becomes a `NeedInterpreter` exit.
pub(crate) fn lower_for_nop_fallback(
    outcome: &BuildOutcome,
    admission: &S0_09NopAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        Some(admission),
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_nop_inc_rax_fallback(
    outcome: &BuildOutcome,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        Some(admission),
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_inc_rax(
    outcome: &BuildOutcome,
    admission: &S0_09NopIncRaxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        Some(admission),
        None,
        true,
        false,
        None,
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_dec_rax(
    outcome: &BuildOutcome,
    admission: &S0_09NopDecRaxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        Some(admission),
        false,
        true,
        None,
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_neg_rax(
    outcome: &BuildOutcome,
    admission: &S0_09NopNegRaxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        Some(admission),
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_not_rax(
    outcome: &BuildOutcome,
    admission: &S0_09NopNotRaxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        Some(admission),
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_lea_rax_rbx(
    outcome: &BuildOutcome,
    admission: &S0_09NopLeaRaxRbxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission_and_lea(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        Some(admission),
        None,
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_lea_rax_rbx_disp8(
    outcome: &BuildOutcome,
    admission: &S0_09NopLeaRaxRbxDisp8Admission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission_and_lea(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(admission),
        None,
        None,
    )
}

pub(crate) fn lower_for_native_xchg_rax_rbx(
    outcome: &BuildOutcome,
    admission: &S0_09NopXchgRaxRbxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission_and_lea(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(admission),
        None,
        None,
        None,
    )
}

pub(crate) fn lower_for_native_add_rax_imm8(
    outcome: &BuildOutcome,
    admission: &S0_09NopAddRaxImm8Admission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        Some(admission),
        None,
        None,
    )
}

pub(crate) fn lower_for_native_cmp_rax_imm8(
    outcome: &BuildOutcome,
    admission: &S0_09NopCmpRaxImm8Admission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        Some(admission),
        None,
    )
}

pub(crate) fn lower_for_native_lea_rax_rbx_rcx(
    outcome: &BuildOutcome,
    admission: &S0_09NopLeaRaxRbxRcxAdmission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission_and_lea(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(admission),
    )
}

pub(crate) fn lower_for_native_sub_rax_imm8(
    outcome: &BuildOutcome,
    admission: &S0_09NopSubRaxImm8Admission,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        Some(admission),
    )
}

pub(crate) fn lower_for_native_mov_rax_imm32(
    outcome: &BuildOutcome,
    admission: &S0_09NopMovRaxImm32Admission,
) -> Result<LoweringOutcome, LoweringError> {
    if let BuildOutcome::Block(block) = outcome {
        for instruction in &block.instructions {
            if let Some(admitted_value) = admission.mov_rax_imm32_value(instruction.guest_rip) {
                if !matches!(instruction.decoded.operation,
                    DecodedOperation::MovImmediate {
                        dst: Operand::Register(dst), value,
                    } if dst.register() == od_isa::Register::Rax
                        && dst.size() == od_isa::OperandSize::Qword
                        && value == admitted_value)
                {
                    return Err(LoweringError::InvalidDecodedForm {
                        guest_rip: instruction.guest_rip,
                    });
                }
            }
        }
    }
    lower_with_admission_and_lea(
        outcome,
        Some(admission.boundaries()),
        None,
        None,
        None,
        false,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(admission),
        None,
    )
}

fn lower_with_admission(
    outcome: &BuildOutcome,
    admission: Option<&AdmittedInstructionBoundaries>,
    nop_admission: Option<&S0_09NopAdmission>,
    nop_inc_rax_admission: Option<&S0_09NopIncRaxAdmission>,
    nop_dec_rax_admission: Option<&S0_09NopDecRaxAdmission>,
    native_inc_rax: bool,
    native_dec_rax: bool,
    native_neg_rax_admission: Option<&S0_09NopNegRaxAdmission>,
    native_not_rax_admission: Option<&S0_09NopNotRaxAdmission>,
    native_add_rax_imm8_admission: Option<&S0_09NopAddRaxImm8Admission>,
    native_cmp_rax_imm8_admission: Option<&S0_09NopCmpRaxImm8Admission>,
    native_sub_rax_imm8_admission: Option<&S0_09NopSubRaxImm8Admission>,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission_and_lea(
        outcome,
        admission,
        nop_admission,
        nop_inc_rax_admission,
        nop_dec_rax_admission,
        native_inc_rax,
        native_dec_rax,
        native_neg_rax_admission,
        native_not_rax_admission,
        native_add_rax_imm8_admission,
        native_cmp_rax_imm8_admission,
        native_sub_rax_imm8_admission,
        None,
        None,
        None,
        None,
        None,
    )
}

fn lower_with_admission_and_lea(
    outcome: &BuildOutcome,
    admission: Option<&AdmittedInstructionBoundaries>,
    nop_admission: Option<&S0_09NopAdmission>,
    nop_inc_rax_admission: Option<&S0_09NopIncRaxAdmission>,
    nop_dec_rax_admission: Option<&S0_09NopDecRaxAdmission>,
    native_inc_rax: bool,
    native_dec_rax: bool,
    native_neg_rax_admission: Option<&S0_09NopNegRaxAdmission>,
    native_not_rax_admission: Option<&S0_09NopNotRaxAdmission>,
    native_add_rax_imm8_admission: Option<&S0_09NopAddRaxImm8Admission>,
    native_cmp_rax_imm8_admission: Option<&S0_09NopCmpRaxImm8Admission>,
    native_sub_rax_imm8_admission: Option<&S0_09NopSubRaxImm8Admission>,
    native_lea_rax_rbx_admission: Option<&S0_09NopLeaRaxRbxAdmission>,
    native_xchg_rax_rbx_admission: Option<&S0_09NopXchgRaxRbxAdmission>,
    native_lea_rax_rbx_disp8_admission: Option<&S0_09NopLeaRaxRbxDisp8Admission>,
    native_mov_rax_imm32_admission: Option<&S0_09NopMovRaxImm32Admission>,
    native_lea_rax_rbx_rcx_admission: Option<&S0_09NopLeaRaxRbxRcxAdmission>,
) -> Result<LoweringOutcome, LoweringError> {
    let block = match outcome {
        BuildOutcome::NoBlock(no_block) => {
            let deferred_error = deferred_error(&no_block.stop_reason);
            return Ok(LoweringOutcome::NoNativePrefix {
                rip: no_block.entry_rip,
                reason: no_block.stop_reason.clone(),
                deferred_error,
            });
        }
        BuildOutcome::Block(block) => block,
    };

    let deferred = deferred_error(&block.stop_reason);
    let has_memory = block.instructions.iter().any(|instruction| {
        block::is_checked_qword_load_form(instruction.decoded)
            || block::is_checked_qword_store_form(instruction.decoded)
    });
    let single_memory = block.instructions.len() == 1
        && (block::is_checked_qword_load_form(block.instructions[0].decoded)
            || block::is_checked_qword_store_form(block.instructions[0].decoded));
    let exact_memory_pair = exact_checked_memory_pair(block);
    if has_memory && !single_memory && !exact_memory_pair {
        return Err(LoweringError::InvalidDecodedForm {
            guest_rip: block.entry_rip,
        });
    }
    let (included_count, terminal) = match &block.stop_reason {
        BlockStopReason::Branch { target } => {
            let Some(branch) = block.instructions.last() else {
                return Err(LoweringError::InvalidDecodedForm {
                    guest_rip: block.entry_rip,
                });
            };
            if !matches!(branch.decoded.operation, DecodedOperation::Branch { .. }) {
                return Err(LoweringError::InvalidDecodedForm {
                    guest_rip: branch.guest_rip,
                });
            }
            match admission {
                Some(admission)
                    if admission.contains(target.rip) && admission.contains(branch.next_rip) =>
                {
                    (
                        block.instructions.len(),
                        Terminal::DispatchBranch {
                            target_rip: target.rip,
                            admission: admission.clone(),
                        },
                    )
                }
                Some(_) => (
                    block.instructions.len() - 1,
                    Terminal::Exit {
                        kind: ExitKind::NeedInterpreter,
                        target_rip: branch.guest_rip,
                    },
                ),
                None => match target.resolution {
                    BranchTargetResolution::KnownInstructionStart
                    | BranchTargetResolution::NextRipBoundary
                    | BranchTargetResolution::CodeEnd => (
                        block.instructions.len(),
                        Terminal::Branch {
                            target_rip: target.rip,
                        },
                    ),
                    BranchTargetResolution::Unresolved => (
                        block.instructions.len() - 1,
                        Terminal::Exit {
                            kind: ExitKind::NeedInterpreter,
                            target_rip: branch.guest_rip,
                        },
                    ),
                },
            }
        }
        BlockStopReason::UnsupportedBranchTarget { branch_rip, .. } => (
            block.instructions.len(),
            Terminal::Exit {
                kind: ExitKind::Unsupported,
                target_rip: *branch_rip,
            },
        ),
        BlockStopReason::UnsupportedDecoded { guest_rip, decoded }
            if nop_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
            }) || nop_inc_rax_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_inc_rax(*guest_rip)
                        && block::is_exact_inc_rax(*decoded, &[0x48, 0xff, 0xc0])
            }) || nop_dec_rax_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_dec_rax(*guest_rip)
                        && block::is_exact_dec_rax(*decoded, &[0x48, 0xff, 0xc8])
            }) || native_neg_rax_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_neg_rax(*guest_rip)
                        && block::is_exact_neg_rax(*decoded, &[0x48, 0xf7, 0xd8])
            }) || native_not_rax_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_not_rax(*guest_rip)
                        && block::is_exact_not_rax(*decoded, &[0x48, 0xf7, 0xd0])
            }) || native_lea_rax_rbx_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_lea_rax_rbx(*guest_rip)
                        && block::is_exact_lea_rax_rbx(*decoded, &[0x48, 0x8d, 0x03])
            }) || native_lea_rax_rbx_disp8_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_lea_rax_rbx_disp8(*guest_rip)
                        && matches!(decoded.operation,
                            DecodedOperation::Lea { dst, src }
                            if dst.register() == od_isa::Register::Rax
                                && dst.size() == od_isa::OperandSize::Qword
                                && src.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
                                && src.index.is_none()
                                && src.scale == 1
                                && admission.lea_rbx_disp8_value(*guest_rip)
                                    == Some(src.displacement)
                        )
            }) || native_lea_rax_rbx_rcx_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_lea_rax_rbx_rcx(*guest_rip)
                        && block::is_exact_lea_rax_rbx_rcx(*decoded, &[0x48, 0x8d, 0x04, 0x0b])
            }) || native_xchg_rax_rbx_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.admits_xchg_rax_rbx(*guest_rip)
                        && block::is_exact_xchg_rax_rbx(*decoded, &[0x48, 0x87, 0xd8])
            }) || native_add_rax_imm8_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission
                        .add_rax_imm8_value(*guest_rip)
                        .is_some_and(|expected| {
                            matches!(decoded.operation,
                            DecodedOperation::Arithmetic {
                                kind: ArithmeticKind::Add,
                                dst: Operand::Register(destination),
                                src: Operand::Immediate(value),
                            } if destination.register() == od_isa::Register::Rax
                                && destination.size() == od_isa::OperandSize::Qword
                                && value == expected)
                        })
            }) || native_sub_rax_imm8_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission
                        .sub_rax_imm8_value(*guest_rip)
                        .is_some_and(|expected| {
                            matches!(decoded.operation,
                            DecodedOperation::Arithmetic {
                                kind: ArithmeticKind::Sub,
                                dst: Operand::Register(destination),
                                src: Operand::Immediate(value),
                            } if destination.register() == od_isa::Register::Rax
                                && destination.size() == od_isa::OperandSize::Qword
                                && value == expected)
                        })
            }) || native_mov_rax_imm32_admission.is_some_and(|admission| {
                admission.admits_nop(*guest_rip)
                    && matches!(decoded.operation, DecodedOperation::Nop)
                    || admission.mov_rax_imm32_value(*guest_rip).is_some_and(|expected| {
                        matches!(decoded.operation,
                            DecodedOperation::MovImmediate {
                                dst: Operand::Register(destination), value,
                            } if destination.register() == od_isa::Register::Rax
                                && destination.size() == od_isa::OperandSize::Qword
                                && value == expected)
                    })
            }) =>
        {
            (
                block.instructions.len(),
                Terminal::Exit {
                    kind: ExitKind::NeedInterpreter,
                    target_rip: *guest_rip,
                },
            )
        }
        BlockStopReason::UnsupportedDecoded { guest_rip, .. }
        | BlockStopReason::UnsupportedEncoding { guest_rip, .. } => (
            block.instructions.len(),
            Terminal::Exit {
                kind: ExitKind::Unsupported,
                target_rip: *guest_rip,
            },
        ),
        BlockStopReason::DecodeBoundary(boundary) => (
            block.instructions.len(),
            Terminal::Exit {
                kind: ExitKind::NeedInterpreter,
                target_rip: boundary.guest_rip,
            },
        ),
        BlockStopReason::BufferEnd
        | BlockStopReason::InstructionLimit
        | BlockStopReason::ByteLimit => {
            let Some(last) = block.instructions.last() else {
                return Ok(LoweringOutcome::NoNativePrefix {
                    rip: block.entry_rip,
                    reason: block.stop_reason.clone(),
                    deferred_error: deferred,
                });
            };
            (
                block.instructions.len(),
                Terminal::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: last.next_rip,
                },
            )
        }
    };

    if included_count == 0 {
        let rip = terminal.target_rip();
        return Ok(LoweringOutcome::NoNativePrefix {
            rip,
            reason: block.stop_reason.clone(),
            deferred_error: deferred,
        });
    }

    let instructions = &block.instructions[..included_count];
    let mut records = Vec::with_capacity(instructions.len());
    let mut operations = Vec::new();
    let mut next_temp = 0u32;
    for (index, instruction) in instructions.iter().enumerate() {
        let admitted_native_inc = native_inc_rax
            && nop_inc_rax_admission
                .is_some_and(|proof| proof.admits_inc_rax(instruction.guest_rip));
        let admitted_native_dec = native_dec_rax
            && nop_dec_rax_admission
                .is_some_and(|proof| proof.admits_dec_rax(instruction.guest_rip));
        let admitted_native_neg = native_neg_rax_admission
            .is_some_and(|proof| proof.admits_neg_rax(instruction.guest_rip));
        let admitted_native_not = native_not_rax_admission
            .is_some_and(|proof| proof.admits_not_rax(instruction.guest_rip));
        let admitted_native_lea = native_lea_rax_rbx_admission
            .is_some_and(|proof| proof.admits_lea_rax_rbx(instruction.guest_rip));
        let admitted_native_lea_disp8 = native_lea_rax_rbx_disp8_admission
            .and_then(|proof| proof.lea_rbx_disp8_value(instruction.guest_rip));
        let admitted_native_lea_sib = native_lea_rax_rbx_rcx_admission
            .is_some_and(|proof| proof.admits_lea_rax_rbx_rcx(instruction.guest_rip));
        let admitted_native_xchg = native_xchg_rax_rbx_admission
            .is_some_and(|proof| proof.admits_xchg_rax_rbx(instruction.guest_rip));
        let admitted_native_add_rax_imm8 = native_add_rax_imm8_admission
            .is_some_and(|proof| proof.admits_add_rax_imm8(instruction.guest_rip));
        let admitted_native_cmp_rax_imm8 = native_cmp_rax_imm8_admission
            .is_some_and(|proof| proof.admits_cmp_rax_imm8(instruction.guest_rip));
        let admitted_native_sub_rax_imm8 = native_sub_rax_imm8_admission
            .is_some_and(|proof| proof.admits_sub_rax_imm8(instruction.guest_rip));
        let admitted_native_mov_rax_imm32 = native_mov_rax_imm32_admission
            .and_then(|proof| proof.mov_rax_imm32_value(instruction.guest_rip));
        if !admitted_native_inc
            && !admitted_native_dec
            && !admitted_native_neg
            && !admitted_native_not
            && !admitted_native_lea
            && admitted_native_lea_disp8.is_none()
            && !admitted_native_lea_sib
            && !admitted_native_xchg
            && !admitted_native_add_rax_imm8
            && !admitted_native_cmp_rax_imm8
            && !admitted_native_sub_rax_imm8
            && admitted_native_mov_rax_imm32.is_none()
            && !block::is_s0_09_form(instruction.decoded)
            && !block::is_checked_qword_load_form(instruction.decoded)
            && !block::is_checked_qword_store_form(instruction.decoded)
        {
            return Err(LoweringError::InvalidDecodedForm {
                guest_rip: instruction.guest_rip,
            });
        }
        records.push(GuestInstruction {
            guest_rip: instruction.guest_rip,
            next_rip: instruction.next_rip,
        });
        if admitted_native_inc {
            lower_exact_inc_rax(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_dec {
            lower_exact_dec_rax(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_neg {
            lower_exact_neg_rax(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_not {
            lower_exact_not_rax(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_lea {
            lower_exact_lea_rax_rbx(instruction, index, &mut next_temp, &mut operations)?;
        } else if let Some(displacement) = admitted_native_lea_disp8 {
            lower_exact_lea_rax_rbx_disp8(
                instruction,
                index,
                displacement,
                &mut next_temp,
                &mut operations,
            )?;
        } else if admitted_native_lea_sib {
            lower_exact_lea_rax_rbx_rcx(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_xchg {
            lower_exact_xchg_rax_rbx(instruction, index, &mut next_temp, &mut operations)?;
        } else if admitted_native_add_rax_imm8 {
            let admitted_value = native_add_rax_imm8_admission
                .and_then(|proof| proof.add_rax_imm8_value(instruction.guest_rip))
                .ok_or(LoweringError::InvalidDecodedForm {
                    guest_rip: instruction.guest_rip,
                })?;
            lower_exact_add_rax_imm8(
                instruction,
                index,
                admitted_value,
                &mut next_temp,
                &mut operations,
            )?;
        } else if admitted_native_cmp_rax_imm8 {
            let admitted_value = native_cmp_rax_imm8_admission
                .and_then(|proof| proof.cmp_rax_imm8_value(instruction.guest_rip))
                .ok_or(LoweringError::InvalidDecodedForm {
                    guest_rip: instruction.guest_rip,
                })?;
            lower_exact_cmp_rax_imm8(
                instruction,
                index,
                admitted_value,
                &mut next_temp,
                &mut operations,
            )?;
        } else if admitted_native_sub_rax_imm8 {
            let admitted_value = native_sub_rax_imm8_admission
                .and_then(|proof| proof.sub_rax_imm8_value(instruction.guest_rip))
                .ok_or(LoweringError::InvalidDecodedForm {
                    guest_rip: instruction.guest_rip,
                })?;
            lower_exact_sub_rax_imm8(
                instruction,
                index,
                admitted_value,
                &mut next_temp,
                &mut operations,
            )?;
        } else if let Some(value) = admitted_native_mov_rax_imm32 {
            lower_exact_mov_rax_imm32(instruction, index, value, &mut next_temp, &mut operations)?;
        } else {
            lower_instruction(instruction, index, &mut next_temp, &mut operations)?;
        }
    }

    match terminal {
        Terminal::Branch { target_rip } => {
            let branch = instructions
                .last()
                .ok_or(LoweringError::InvalidDecodedForm {
                    guest_rip: block.entry_rip,
                })?;
            let DecodedOperation::Branch { condition, .. } = branch.decoded.operation else {
                return Err(LoweringError::InvalidDecodedForm {
                    guest_rip: branch.guest_rip,
                });
            };
            operations.push(Operation::Branch {
                condition: match condition {
                    ConditionCode::Equal => BranchCondition::Zero,
                    ConditionCode::NotEqual => BranchCondition::NotZero,
                    _ => {
                        return Err(LoweringError::InvalidDecodedForm {
                            guest_rip: branch.guest_rip,
                        });
                    }
                },
                target_rip,
                fallthrough_rip: branch.next_rip,
            });
        }
        Terminal::DispatchBranch {
            target_rip,
            admission,
        } => {
            let branch = instructions
                .last()
                .ok_or(LoweringError::InvalidDecodedForm {
                    guest_rip: block.entry_rip,
                })?;
            let DecodedOperation::Branch { condition, .. } = branch.decoded.operation else {
                return Err(LoweringError::InvalidDecodedForm {
                    guest_rip: branch.guest_rip,
                });
            };
            operations.push(Operation::DispatchBranch {
                condition: match condition {
                    ConditionCode::Equal => BranchCondition::Zero,
                    ConditionCode::NotEqual => BranchCondition::NotZero,
                    _ => {
                        return Err(LoweringError::InvalidDecodedForm {
                            guest_rip: branch.guest_rip,
                        });
                    }
                },
                target_rip,
                fallthrough_rip: branch.next_rip,
                admission,
            });
        }
        Terminal::Exit { kind, target_rip } => {
            operations.push(Operation::Exit { kind, target_rip });
        }
    }

    let program = Program {
        guest_instructions: records,
        operations,
        code_end: block.code_end,
    };
    ir::verify(&program).map_err(LoweringError::Verification)?;
    Ok(LoweringOutcome::Program(LoweredProgram {
        program,
        deferred_error: deferred,
    }))
}

pub(crate) fn exact_checked_memory_pair(block: &crate::block::BasicBlock) -> bool {
    if block.instructions.len() != 2 {
        return false;
    }
    let [first, second] = block.instructions.as_slice() else {
        return false;
    };
    if first.next_rip.checked_sub(first.guest_rip) != Some(3)
        || second.next_rip.checked_sub(second.guest_rip) != Some(3)
        || first.next_rip != second.guest_rip
    {
        return false;
    }
    let store_to_load = matches!(
        first.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Memory(operand),
            src: Operand::Register(source),
        } if operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && source.register() == od_isa::Register::Rax
            && source.size() == od_isa::OperandSize::Qword
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    ) && matches!(
        second.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } if destination.register() == od_isa::Register::Rcx
            && destination.size() == od_isa::OperandSize::Qword
            && operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    );
    let load_to_store = matches!(
        first.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword
            && operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    ) && matches!(
        second.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Memory(operand),
            src: Operand::Register(source),
        } if operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rdx)
            && source.register() == od_isa::Register::Rax
            && source.size() == od_isa::OperandSize::Qword
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    );
    let load_to_load = matches!(
        first.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword
            && operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    ) && matches!(
        second.decoded.operation,
        DecodedOperation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } if destination.register() == od_isa::Register::Rcx
            && destination.size() == od_isa::OperandSize::Qword
            && operand.base == od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
            && operand.index.is_none()
            && operand.displacement == 0
            && operand.address_size == od_isa::OperandSize::Qword
            && operand.size == od_isa::OperandSize::Qword
    );
    store_to_load || load_to_store || load_to_load
}

#[derive(Clone)]
enum Terminal {
    Branch {
        target_rip: u64,
    },
    DispatchBranch {
        target_rip: u64,
        admission: AdmittedInstructionBoundaries,
    },
    Exit {
        kind: ExitKind,
        target_rip: u64,
    },
}

impl Terminal {
    fn target_rip(self) -> u64 {
        match self {
            Self::Branch { target_rip }
            | Self::DispatchBranch { target_rip, .. }
            | Self::Exit { target_rip, .. } => target_rip,
        }
    }
}

fn deferred_error(reason: &BlockStopReason) -> Option<DecodeBoundary> {
    match reason {
        BlockStopReason::DecodeBoundary(boundary) => Some(boundary.clone()),
        _ => None,
    }
}

fn lower_exact_mov_rax_imm32(
    instruction: &DecodedInstruction,
    index: usize,
    value: u64,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation,
        DecodedOperation::MovImmediate {
            dst: Operand::Register(destination), value: decoded_value,
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword
            && decoded_value == value)
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let constant = allocate_temp(next_temp);
    operations.push(Operation::Constant {
        dst: constant,
        width: IntWidth::W64,
        value,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: constant,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_inc_rax(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation,
        DecodedOperation::Arithmetic {
            kind: ArithmeticKind::Inc,
            dst: Operand::Register(destination),
            src: Operand::Immediate(1),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword)
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let lhs = allocate_temp(next_temp);
    let rhs = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: lhs,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: rhs,
        width: IntWidth::W64,
        value: 1,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Add,
        lhs,
        rhs,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::WriteMask(crate::ir::INC_RAX_FLAGS_MASK),
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_dec_rax(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation,
        DecodedOperation::Arithmetic {
            kind: ArithmeticKind::Dec,
            dst: Operand::Register(destination),
            src: Operand::Immediate(1),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword)
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let lhs = allocate_temp(next_temp);
    let rhs = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: lhs,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: rhs,
        width: IntWidth::W64,
        value: 1,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Subtract,
        lhs,
        rhs,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::WriteMask(crate::ir::INC_DEC_RAX_FLAGS_MASK),
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_neg_rax(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation,
        DecodedOperation::Arithmetic {
            kind: ArithmeticKind::Neg,
            dst: Operand::Register(destination),
            src: Operand::Immediate(0),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword)
        || !block::is_exact_neg_rax(instruction.decoded, &[0x48, 0xf7, 0xd8])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let rax = allocate_temp(next_temp);
    let zero = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: rax,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: zero,
        width: IntWidth::W64,
        value: 0,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Subtract,
        lhs: zero,
        rhs: rax,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Arithmetic,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_not_rax(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation,
        DecodedOperation::Logic {
            kind: LogicKind::Not,
            dst: Operand::Register(destination),
            src: Operand::Immediate(0),
        } if destination.register() == od_isa::Register::Rax
            && destination.size() == od_isa::OperandSize::Qword)
        || !block::is_exact_not_rax(instruction.decoded, &[0x48, 0xf7, 0xd0])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let all_ones = allocate_temp(next_temp);
    let rax = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::Constant {
        dst: all_ones,
        width: IntWidth::W64,
        value: u64::MAX,
    });
    operations.push(Operation::ReadGpr {
        dst: rax,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Subtract,
        lhs: all_ones,
        rhs: rax,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Preserve,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_lea_rax_rbx(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation, DecodedOperation::Lea { .. })
        || !block::is_exact_lea_rax_rbx(instruction.decoded, &[0x48, 0x8d, 0x03])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let rbx = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: rbx,
        register: crate::ir::GuestGpr(3),
        width: IntWidth::W64,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: rbx,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_lea_rax_rbx_disp8(
    instruction: &DecodedInstruction,
    index: usize,
    displacement: i64,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    let DecodedOperation::Lea { dst, src } = instruction.decoded.operation else {
        return Err(invalid());
    };
    if dst.register() != od_isa::Register::Rax
        || dst.size() != od_isa::OperandSize::Qword
        || src.base != od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbx)
        || src.index.is_some()
        || src.scale != 1
        || src.displacement != displacement
        || src.address_size != od_isa::OperandSize::Qword
        || src.size != od_isa::OperandSize::Qword
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let rbx = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: rbx,
        register: crate::ir::GuestGpr(3),
        width: IntWidth::W64,
    });
    let displacement_value = allocate_temp(next_temp);
    operations.push(Operation::Constant {
        dst: displacement_value,
        value: displacement as u64,
        width: IntWidth::W64,
    });
    let result = allocate_temp(next_temp);
    operations.push(Operation::Integer {
        kind: IntegerKind::Add,
        lhs: rbx,
        rhs: displacement_value,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Preserve,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_lea_rax_rbx_rcx(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !block::is_exact_lea_rax_rbx_rcx(instruction.decoded, &[0x48, 0x8d, 0x04, 0x0b]) {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let rbx = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: rbx,
        register: GuestGpr(3),
        width: IntWidth::W64,
    });
    let rcx = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: rcx,
        register: GuestGpr(1),
        width: IntWidth::W64,
    });
    let result = allocate_temp(next_temp);
    operations.push(Operation::Integer {
        kind: IntegerKind::Add,
        lhs: rbx,
        rhs: rcx,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Preserve,
    });
    operations.push(Operation::WriteGpr {
        register: GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_xchg_rax_rbx(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    if !matches!(instruction.decoded.operation, DecodedOperation::Xchg { .. })
        || !block::is_exact_xchg_rax_rbx(instruction.decoded, &[0x48, 0x87, 0xd8])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let old_rax = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: old_rax,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    let old_rbx = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: old_rbx,
        register: crate::ir::GuestGpr(3),
        width: IntWidth::W64,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: old_rbx,
        width: IntWidth::W64,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(3),
        src: old_rax,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_add_rax_imm8(
    instruction: &DecodedInstruction,
    index: usize,
    admitted_value: u64,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    let DecodedOperation::Arithmetic {
        kind: ArithmeticKind::Add,
        dst: Operand::Register(destination),
        src: Operand::Immediate(value),
    } = instruction.decoded.operation
    else {
        return Err(invalid());
    };
    if destination.register() != od_isa::Register::Rax
        || destination.size() != od_isa::OperandSize::Qword
        || value != admitted_value
        || !block::is_exact_add_rax_imm8(instruction.decoded, &[0x48, 0x83, 0xc0, value as u8])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let lhs = allocate_temp(next_temp);
    let rhs = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: lhs,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: rhs,
        width: IntWidth::W64,
        value,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Add,
        lhs,
        rhs,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Arithmetic,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_sub_rax_imm8(
    instruction: &DecodedInstruction,
    index: usize,
    admitted_value: u64,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    let DecodedOperation::Arithmetic {
        kind: ArithmeticKind::Sub,
        dst: Operand::Register(destination),
        src: Operand::Immediate(value),
    } = instruction.decoded.operation
    else {
        return Err(invalid());
    };
    if destination.register() != od_isa::Register::Rax
        || destination.size() != od_isa::OperandSize::Qword
        || value != admitted_value
        || !block::is_exact_sub_rax_imm8(instruction.decoded, &[0x48, 0x83, 0xe8, value as u8])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let lhs = allocate_temp(next_temp);
    let rhs = allocate_temp(next_temp);
    let result = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: lhs,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: rhs,
        width: IntWidth::W64,
        value,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Subtract,
        lhs,
        rhs,
        result: Some(result),
        width: IntWidth::W64,
        flags: FlagEffect::Arithmetic,
    });
    operations.push(Operation::WriteGpr {
        register: crate::ir::GuestGpr(0),
        src: result,
        width: IntWidth::W64,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_exact_cmp_rax_imm8(
    instruction: &DecodedInstruction,
    index: usize,
    admitted_value: u64,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    let DecodedOperation::Arithmetic {
        kind: ArithmeticKind::Cmp,
        dst: Operand::Register(destination),
        src: Operand::Immediate(value),
    } = instruction.decoded.operation
    else {
        return Err(invalid());
    };
    if destination.register() != od_isa::Register::Rax
        || destination.size() != od_isa::OperandSize::Qword
        || value != admitted_value
        || !block::is_exact_cmp_rax_imm8(instruction.decoded, &[0x48, 0x83, 0xf8, value as u8])
    {
        return Err(invalid());
    }
    operations.push(Operation::BeginInstruction { index });
    let lhs = allocate_temp(next_temp);
    let rhs = allocate_temp(next_temp);
    operations.push(Operation::ReadGpr {
        dst: lhs,
        register: crate::ir::GuestGpr(0),
        width: IntWidth::W64,
    });
    operations.push(Operation::Constant {
        dst: rhs,
        width: IntWidth::W64,
        value,
    });
    operations.push(Operation::Integer {
        kind: IntegerKind::Compare,
        lhs,
        rhs,
        result: None,
        width: IntWidth::W64,
        flags: FlagEffect::Arithmetic,
    });
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn lower_instruction(
    instruction: &DecodedInstruction,
    index: usize,
    next_temp: &mut u32,
    operations: &mut Vec<Operation>,
) -> Result<(), LoweringError> {
    let invalid = || LoweringError::InvalidDecodedForm {
        guest_rip: instruction.guest_rip,
    };
    operations.push(Operation::BeginInstruction { index });
    match instruction.decoded.operation {
        DecodedOperation::Mov {
            dst: Operand::Register(destination),
            src: Operand::Memory(operand),
        } if block::is_checked_qword_load_form(instruction.decoded) => {
            let destination =
                guest_memory_data_gpr(destination, instruction.decoded).ok_or_else(invalid)?;
            let value = allocate_temp(next_temp);
            operations.push(Operation::CheckedLoad {
                dst: value,
                operand,
            });
            operations.push(Operation::WriteGpr {
                register: destination,
                src: value,
                width: IntWidth::W64,
            });
        }
        DecodedOperation::Mov {
            dst: Operand::Memory(operand),
            src: Operand::Register(source),
        } if block::is_checked_qword_store_form(instruction.decoded) => {
            operations.push(Operation::CheckedStore {
                src: guest_memory_data_gpr(source, instruction.decoded).ok_or_else(invalid)?,
                operand,
            });
        }
        DecodedOperation::MovImmediate {
            dst: Operand::Register(dst),
            value,
        } => {
            let dst = guest_gpr(dst).ok_or_else(invalid)?;
            let value_temp = allocate_temp(next_temp);
            operations.push(Operation::Constant {
                dst: value_temp,
                width: IntWidth::W64,
                value,
            });
            operations.push(Operation::WriteGpr {
                register: dst,
                src: value_temp,
                width: IntWidth::W64,
            });
            // MOV preserves flags: no flags-writing effect is emitted.
        }
        DecodedOperation::Arithmetic { kind, dst, src } => {
            let (Operand::Register(dst), Operand::Register(src)) = (dst, src) else {
                return Err(invalid());
            };
            let dst_gpr = guest_gpr(dst).ok_or_else(invalid)?;
            let src_gpr = guest_gpr(src).ok_or_else(invalid)?;
            // Capture both operands before any destination write, including
            // the aliasing case where dst and src name the same guest GPR.
            let lhs = allocate_temp(next_temp);
            let rhs = allocate_temp(next_temp);
            operations.push(Operation::ReadGpr {
                dst: lhs,
                register: dst_gpr,
                width: IntWidth::W64,
            });
            operations.push(Operation::ReadGpr {
                dst: rhs,
                register: src_gpr,
                width: IntWidth::W64,
            });
            let (kind, result) = match kind {
                ArithmeticKind::Add => (IntegerKind::Add, Some(allocate_temp(next_temp))),
                ArithmeticKind::Sub => (IntegerKind::Subtract, Some(allocate_temp(next_temp))),
                ArithmeticKind::Cmp => (IntegerKind::Compare, None),
                _ => return Err(invalid()),
            };
            operations.push(Operation::Integer {
                kind,
                lhs,
                rhs,
                result,
                width: IntWidth::W64,
                flags: FlagEffect::Arithmetic,
            });
            if let Some(result) = result {
                operations.push(Operation::WriteGpr {
                    register: dst_gpr,
                    src: result,
                    width: IntWidth::W64,
                });
            }
        }
        DecodedOperation::Branch { .. } => {
            // A conditional branch has no data effects in this IR slice. Its
            // condition/target selection follows its commit marker as terminal.
        }
        _ => return Err(invalid()),
    }
    operations.push(Operation::CommitInstruction { index });
    Ok(())
}

fn allocate_temp(next_temp: &mut u32) -> TempId {
    let result = TempId(*next_temp);
    *next_temp += 1;
    result
}

fn guest_gpr(register: RegisterView) -> Option<GuestGpr> {
    let number = register.register() as u8;
    (number < 4 && register.size() == od_isa::OperandSize::Qword).then_some(GuestGpr(number))
}

fn guest_memory_data_gpr(register: RegisterView, instruction: Instruction) -> Option<GuestGpr> {
    let number = register.register() as u8;
    (register.size() == od_isa::OperandSize::Qword
        && (number < 4
            || number == 4
                && (block::is_checked_qword_load_form(instruction)
                    || block::is_checked_qword_store_form(instruction))
            || number == 5
                && (block::is_checked_qword_load_form(instruction)
                    || block::is_checked_qword_store_form(instruction))
            || matches!(number, 6 | 7)
                && (block::is_checked_qword_load_form(instruction)
                    || block::is_checked_qword_store_form(instruction))))
    .then_some(GuestGpr(number))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{self, BlockLimits, CodeView};
    use od_isa::memory::AccessContext;

    const BASE: u64 = 0x1000;

    fn build(bytes: &[u8]) -> BuildOutcome {
        block::build_block(
            CodeView::new(BASE, bytes).unwrap(),
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap()
    }

    fn admission(bytes: &[u8]) -> AdmittedInstructionBoundaries {
        let code = CodeView::new(BASE, bytes).unwrap();
        let decoded = od_isa::decode(bytes).unwrap();
        AdmittedInstructionBoundaries::from_fixture_decode(code, &decoded).unwrap()
    }

    fn program(bytes: &[u8]) -> LoweredProgram {
        match lower(&build(bytes)).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected lowered program, got {other:?}"),
        }
    }

    fn mov_rax(value: u64) -> Vec<u8> {
        [vec![0x48, 0xb8], value.to_le_bytes().to_vec()].concat()
    }

    #[test]
    fn lowers_exact_store_then_load_as_two_committed_memory_effects() {
        let outcome = block::build_checked_qword_two_site_block(
            CodeView::new(BASE, &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b]).unwrap(),
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower(&outcome).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected program, got {other:?}"),
        };
        assert_eq!(lowered.program.guest_instructions.len(), 2);
        assert!(matches!(
            lowered.program.operations.as_slice(),
            [
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedStore {
                    src: GuestGpr(0),
                    ..
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
                Operation::CheckedLoad { .. },
                Operation::WriteGpr {
                    register: GuestGpr(1),
                    width: IntWidth::W64,
                    ..
                },
                Operation::CommitInstruction { index: 1 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x1006
                },
            ]
        ));
    }

    #[test]
    fn lowers_exact_load_then_load_as_two_committed_memory_effects() {
        let outcome = block::build_checked_qword_two_site_block(
            CodeView::new(BASE, &[0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b]).unwrap(),
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower(&outcome).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected program, got {other:?}"),
        };
        assert_eq!(lowered.program.guest_instructions.len(), 2);
        assert!(matches!(
            lowered.program.operations.as_slice(),
            [
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedLoad { dst: TempId(0), .. },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(0),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
                Operation::CheckedLoad { dst: TempId(1), .. },
                Operation::WriteGpr {
                    register: GuestGpr(1),
                    src: TempId(1),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 1 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x1006,
                },
            ]
        ));
        assert_eq!(ir::verify(&lowered.program), Ok(()));

        let mut missing_first_commit = lowered.program.clone();
        missing_first_commit.operations.remove(3);
        assert!(ir::verify(&missing_first_commit).is_err());
        let mut early_second_commit = lowered.program.clone();
        early_second_commit.operations.swap(5, 7);
        assert!(ir::verify(&early_second_commit).is_err());
    }

    #[test]
    fn lowers_mov_add_sub_cmp_with_exact_width_flags_and_effects() {
        let mov = program(&mov_rax(0x1234));
        assert_eq!(mov.program.guest_instructions.len(), 1);
        assert!(matches!(
            mov.program.operations[1],
            Operation::Constant {
                width: IntWidth::W64,
                value: 0x1234,
                ..
            }
        ));
        assert!(matches!(
            mov.program.operations[2],
            Operation::WriteGpr {
                register: GuestGpr(0),
                width: IntWidth::W64,
                ..
            }
        ));
        assert!(
            !mov.program
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Integer { .. }))
        );
        assert!(matches!(
            mov.program.operations.last(),
            Some(Operation::Exit {
                kind: ExitKind::BlockComplete,
                target_rip: 0x100a
            })
        ));

        for (bytes, expected) in [
            (&[0x48, 0x03, 0xc1][..], IntegerKind::Add),
            (&[0x48, 0x2b, 0xc1][..], IntegerKind::Subtract),
            (&[0x48, 0x3b, 0xc1][..], IntegerKind::Compare),
        ] {
            let lowered = program(bytes);
            let integers: Vec<_> = lowered
                .program
                .operations
                .iter()
                .filter_map(|op| match op {
                    Operation::Integer {
                        kind,
                        lhs,
                        rhs,
                        result,
                        width,
                        flags,
                    } => Some((*kind, *lhs, *rhs, *result, *width, *flags)),
                    _ => None,
                })
                .collect();
            assert_eq!(integers.len(), 1);
            assert_eq!(integers[0].0, expected);
            assert_eq!(integers[0].4, IntWidth::W64);
            assert_eq!(integers[0].5, FlagEffect::Arithmetic);
            assert_eq!(integers[0].1, TempId(0));
            assert_eq!(integers[0].2, TempId(1));
            let writes = lowered
                .program
                .operations
                .iter()
                .filter(|op| matches!(op, Operation::WriteGpr { .. }))
                .count();
            assert_eq!(writes, usize::from(expected != IntegerKind::Compare));
            assert_eq!(
                lowered.program.guest_instructions,
                vec![GuestInstruction {
                    guest_rip: BASE,
                    next_rip: BASE + 3
                }]
            );
        }
    }

    #[test]
    fn native_neg_rax_lowers_to_zero_minus_rax_with_full_arithmetic_flags() {
        let code = CodeView::new(BASE, &[0x48, 0xf7, 0xd8, 0x74, 0x00]).unwrap();
        let admission = S0_09NopNegRaxAdmission::from_code(code).unwrap();
        let built = block::build_block_for_native_neg_rax(
            code,
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower_for_native_neg_rax(&built, &admission).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected NEG native prefix, got {other:?}"),
        };
        assert!(matches!(
            &lowered.program.operations[..7],
            [
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(0),
                    register: GuestGpr(0),
                    width: IntWidth::W64
                },
                Operation::Constant {
                    dst: TempId(1),
                    width: IntWidth::W64,
                    value: 0
                },
                Operation::Integer {
                    kind: IntegerKind::Subtract,
                    lhs: TempId(1),
                    rhs: TempId(0),
                    result: Some(TempId(2)),
                    width: IntWidth::W64,
                    flags: FlagEffect::Arithmetic
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(2),
                    width: IntWidth::W64
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
            ]
        ));
        assert_eq!(ir::verify(&lowered.program), Ok(()));
    }

    #[test]
    fn native_not_rax_lowers_to_all_ones_minus_rax_while_preserving_flags() {
        let code = CodeView::new(BASE, &[0x48, 0xf7, 0xd0, 0x74, 0x00]).unwrap();
        let admission = S0_09NopNotRaxAdmission::from_code(code).unwrap();
        let built = block::build_block_for_native_not_rax(
            code,
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower_for_native_not_rax(&built, &admission).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected NOT native prefix, got {other:?}"),
        };
        assert!(matches!(
            &lowered.program.operations[..7],
            [
                Operation::BeginInstruction { index: 0 },
                Operation::Constant {
                    dst: TempId(0),
                    width: IntWidth::W64,
                    value: u64::MAX
                },
                Operation::ReadGpr {
                    dst: TempId(1),
                    register: GuestGpr(0),
                    width: IntWidth::W64
                },
                Operation::Integer {
                    kind: IntegerKind::Subtract,
                    lhs: TempId(0),
                    rhs: TempId(1),
                    result: Some(TempId(2)),
                    width: IntWidth::W64,
                    flags: FlagEffect::Preserve
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(2),
                    width: IntWidth::W64
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
            ]
        ));
        assert_eq!(ir::verify(&lowered.program), Ok(()));
    }

    #[test]
    fn native_lea_rax_rbx_lowers_to_gpr_copy_without_memory_or_flags_operations() {
        let code = CodeView::new(BASE, &[0x48, 0x8d, 0x03, 0x74, 0x00]).unwrap();
        let admission = S0_09NopLeaRaxRbxAdmission::from_code(code).unwrap();
        let built = block::build_block_for_native_lea_rax_rbx(
            code,
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower_for_native_lea_rax_rbx(&built, &admission).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected LEA native prefix, got {other:?}"),
        };
        assert!(matches!(
            &lowered.program.operations[..5],
            [
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(0),
                    register: GuestGpr(3),
                    width: IntWidth::W64
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(0),
                    width: IntWidth::W64
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
            ]
        ));
        assert!(!lowered.program.operations.iter().any(|operation| matches!(
            operation,
            Operation::CheckedLoad { .. }
                | Operation::CheckedStore { .. }
                | Operation::Integer { .. }
        )));
        assert_eq!(ir::verify(&lowered.program), Ok(()));
    }

    #[test]
    fn native_xchg_rax_rbx_reads_both_prestate_values_before_writes() {
        let code = CodeView::new(BASE, &[0x48, 0x87, 0xd8, 0x74, 0x00]).unwrap();
        let admission = S0_09NopXchgRaxRbxAdmission::from_code(code).unwrap();
        let built = block::build_block_for_native_xchg_rax_rbx(
            code,
            BASE,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let lowered = match lower_for_native_xchg_rax_rbx(&built, &admission).unwrap() {
            LoweringOutcome::Program(program) => program,
            other => panic!("expected XCHG native prefix, got {other:?}"),
        };
        assert!(matches!(
            &lowered.program.operations[..7],
            [
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(0),
                    register: GuestGpr(0),
                    width: IntWidth::W64
                },
                Operation::ReadGpr {
                    dst: TempId(1),
                    register: GuestGpr(3),
                    width: IntWidth::W64
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(1),
                    width: IntWidth::W64
                },
                Operation::WriteGpr {
                    register: GuestGpr(3),
                    src: TempId(0),
                    width: IntWidth::W64
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
            ]
        ));
        assert_eq!(
            lowered
                .program
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::CommitInstruction { index: 0 }))
                .count(),
            1
        );
        assert!(!lowered.program.operations.iter().any(|operation| matches!(
            operation,
            Operation::CheckedLoad { .. }
                | Operation::CheckedStore { .. }
                | Operation::Integer { .. }
        )));
        assert_eq!(ir::verify(&lowered.program), Ok(()));
    }

    #[test]
    fn native_add_rax_imm8_lowers_decoded_sign_extended_value() {
        for (immediate, value) in [
            (0x00, 0),
            (0x01, 1),
            (0x7f, 127),
            (0x80, u64::MAX - 127),
            (0xff, u64::MAX),
        ] {
            let bytes = [0x48, 0x83, 0xc0, immediate];
            let code = CodeView::new(BASE, &bytes).unwrap();
            let admission = S0_09NopAddRaxImm8Admission::from_code(code).unwrap();
            let built = block::build_block_for_native_add_rax_imm8(
                code,
                BASE,
                AccessContext::default(),
                BlockLimits::default(),
            )
            .unwrap();
            let lowered = match lower_for_native_add_rax_imm8(&built, &admission).unwrap() {
                LoweringOutcome::Program(program) => program,
                other => panic!("expected ADD immediate native prefix, got {other:?}"),
            };
            assert!(matches!(
                &lowered.program.operations[..6],
                [
                    Operation::BeginInstruction { index: 0 },
                    Operation::ReadGpr {
                        dst: TempId(0),
                        register: GuestGpr(0),
                        width: IntWidth::W64
                    },
                    Operation::Constant {
                        dst: TempId(1),
                        width: IntWidth::W64,
                        value: constant
                    },
                    Operation::Integer {
                        kind: IntegerKind::Add,
                        lhs: TempId(0),
                        rhs: TempId(1),
                        result: Some(TempId(2)),
                        width: IntWidth::W64,
                        flags: FlagEffect::Arithmetic
                    },
                    Operation::WriteGpr {
                        register: GuestGpr(0),
                        src: TempId(2),
                        width: IntWidth::W64
                    },
                    Operation::CommitInstruction { index: 0 },
                ] if *constant == value
            ));
            assert_eq!(lowered.program.guest_instructions[0].next_rip, BASE + 4);
            assert_eq!(ir::verify(&lowered.program), Ok(()));
        }
    }

    #[test]
    fn native_sub_rax_imm8_lowers_decoded_sign_extended_value() {
        for (immediate, value) in [
            (0x00, 0),
            (0x01, 1),
            (0x7f, 127),
            (0x80, u64::MAX - 127),
            (0xff, u64::MAX),
        ] {
            let bytes = [0x48, 0x83, 0xe8, immediate];
            let code = CodeView::new(BASE, &bytes).unwrap();
            let admission = S0_09NopSubRaxImm8Admission::from_code(code).unwrap();
            let built = block::build_block_for_native_sub_rax_imm8(
                code,
                BASE,
                AccessContext::default(),
                BlockLimits::default(),
            )
            .unwrap();
            let lowered = match lower_for_native_sub_rax_imm8(&built, &admission).unwrap() {
                LoweringOutcome::Program(program) => program,
                other => panic!("expected SUB immediate native prefix, got {other:?}"),
            };
            assert!(matches!(
                &lowered.program.operations[..6],
                [
                    Operation::BeginInstruction { index: 0 },
                    Operation::ReadGpr {
                        dst: TempId(0),
                        register: GuestGpr(0),
                        width: IntWidth::W64
                    },
                    Operation::Constant {
                        dst: TempId(1),
                        width: IntWidth::W64,
                        value: constant
                    },
                    Operation::Integer {
                        kind: IntegerKind::Subtract,
                        lhs: TempId(0),
                        rhs: TempId(1),
                        result: Some(TempId(2)),
                        width: IntWidth::W64,
                        flags: FlagEffect::Arithmetic
                    },
                    Operation::WriteGpr {
                        register: GuestGpr(0),
                        src: TempId(2),
                        width: IntWidth::W64
                    },
                    Operation::CommitInstruction { index: 0 },
                ] if *constant == value
            ));
            assert_eq!(lowered.program.guest_instructions[0].next_rip, BASE + 4);
            assert_eq!(ir::verify(&lowered.program), Ok(()));
        }
    }

    #[test]
    fn native_cmp_rax_imm8_lowers_to_flags_only_compare() {
        for (immediate, value) in [
            (0x00, 0),
            (0x01, 1),
            (0x7f, 127),
            (0x80, u64::MAX - 127),
            (0xff, u64::MAX),
        ] {
            let bytes = [0x48, 0x83, 0xf8, immediate];
            let code = CodeView::new(BASE, &bytes).unwrap();
            let admission = S0_09NopCmpRaxImm8Admission::from_code(code).unwrap();
            let built = block::build_block_for_native_cmp_rax_imm8(
                code,
                BASE,
                AccessContext::default(),
                BlockLimits::default(),
            )
            .unwrap();
            let lowered = match lower_for_native_cmp_rax_imm8(&built, &admission).unwrap() {
                LoweringOutcome::Program(program) => program,
                other => panic!("expected CMP immediate native prefix, got {other:?}"),
            };
            assert!(matches!(
                &lowered.program.operations[..5],
                [
                    Operation::BeginInstruction { index: 0 },
                    Operation::ReadGpr {
                        dst: TempId(0),
                        register: GuestGpr(0),
                        width: IntWidth::W64
                    },
                    Operation::Constant {
                        dst: TempId(1),
                        width: IntWidth::W64,
                        value: constant
                    },
                    Operation::Integer {
                        kind: IntegerKind::Compare,
                        lhs: TempId(0),
                        rhs: TempId(1),
                        result: None,
                        width: IntWidth::W64,
                        flags: FlagEffect::Arithmetic
                    },
                    Operation::CommitInstruction { index: 0 },
                ] if *constant == value
            ));
            assert!(
                !lowered
                    .program
                    .operations
                    .iter()
                    .any(|operation| matches!(operation, Operation::WriteGpr { .. }))
            );
            assert_eq!(lowered.program.guest_instructions[0].next_rip, BASE + 4);
            assert_eq!(ir::verify(&lowered.program), Ok(()));
        }
    }

    #[test]
    fn arithmetic_reads_both_inputs_before_write_even_when_registers_alias() {
        let lowered = program(&[0x48, 0x03, 0xc0]); // ADD RAX, RAX
        let read_positions: Vec<_> = lowered
            .program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(i, op)| matches!(op, Operation::ReadGpr { .. }).then_some(i))
            .collect();
        let write_position = lowered
            .program
            .operations
            .iter()
            .position(|op| matches!(op, Operation::WriteGpr { .. }))
            .unwrap();
        assert_eq!(read_positions.len(), 2);
        assert!(read_positions[0] < read_positions[1] && read_positions[1] < write_position);
        assert!(matches!(
            lowered.program.operations[read_positions[0]],
            Operation::ReadGpr {
                register: GuestGpr(0),
                ..
            }
        ));
        assert!(matches!(
            lowered.program.operations[read_positions[1]],
            Operation::ReadGpr {
                register: GuestGpr(0),
                ..
            }
        ));
    }

    #[test]
    fn lowers_branch_boundaries_and_keeps_terminal_branch_after_commit() {
        let mut backward = mov_rax(1);
        backward.extend([0x74, 0xf4]); // target BASE
        let backward = program(&backward);
        assert!(matches!(
            backward.program.operations.last(),
            Some(Operation::Branch {
                condition: BranchCondition::Zero,
                target_rip: BASE,
                fallthrough_rip: 0x100c
            })
        ));
        let branch_begin = backward
            .program
            .operations
            .iter()
            .position(|op| matches!(op, Operation::BeginInstruction { index: 1 }))
            .unwrap();
        assert_eq!(
            backward.program.operations[branch_begin + 1],
            Operation::CommitInstruction { index: 1 }
        );
        assert!(matches!(
            backward.program.operations[branch_begin + 2],
            Operation::Branch { .. }
        ));

        let self_branch = program(&[0x74, 0xfe]);
        assert!(matches!(
            self_branch.program.operations.last(),
            Some(Operation::Branch {
                target_rip: BASE,
                ..
            })
        ));
        let next_rip = program(&[0x75, 0x00, 0xff]);
        assert!(matches!(
            next_rip.program.operations.last(),
            Some(Operation::Branch {
                condition: BranchCondition::NotZero,
                target_rip: 0x1002,
                ..
            })
        ));
        let code_end = program(&[0x74, 0x03, 0x90, 0x90, 0x90]);
        assert_eq!(code_end.program.code_end, BASE + 5);
        assert!(matches!(
            code_end.program.operations.last(),
            Some(Operation::Branch {
                target_rip: 0x1005,
                ..
            })
        ));
        assert!(ir::verify(&code_end.program).is_ok());
    }

    #[test]
    fn unresolved_branch_exits_before_jcc_and_preserves_exact_prefix() {
        let first = build(&[0x74, 0x02, 0x90, 0x90, 0x90, 0x90]);
        assert!(matches!(
            lower(&first).unwrap(),
            LoweringOutcome::NoNativePrefix {
                rip: BASE,
                reason: BlockStopReason::Branch { .. },
                ..
            }
        ));

        let mut bytes = mov_rax(7);
        bytes.extend([0x74, 0x02, 0x90, 0x90, 0x90, 0x90]);
        let lowered = program(&bytes);
        assert_eq!(
            lowered.program.guest_instructions,
            vec![GuestInstruction {
                guest_rip: BASE,
                next_rip: BASE + 10
            }]
        );
        assert!(matches!(
            lowered.program.operations.last(),
            Some(Operation::Exit {
                kind: ExitKind::NeedInterpreter,
                target_rip: 0x100a
            })
        ));
        assert_eq!(
            lowered
                .program
                .operations
                .iter()
                .filter(|op| matches!(op, Operation::CommitInstruction { .. }))
                .count(),
            1
        );
        assert!(
            !lowered
                .program
                .operations
                .iter()
                .any(|op| matches!(op, Operation::Branch { .. }))
        );
    }

    #[test]
    fn dispatch_lowering_hands_off_only_admitted_full_fixture_boundaries() {
        // First JZ targets the second instruction in the undecoded tail. The
        // bounded builder stops after JZ; fixture admission supplies the full
        // boundary proof without asking the builder to decode that tail.
        let forward = [0x74, 0x02, 0x74, 0x00, 0x74, 0x00];
        let built = build(&forward);
        let BuildOutcome::Block(block) = &built else {
            panic!("valid branch should form a block");
        };
        assert_eq!(block.instructions.len(), 1);
        assert!(matches!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: crate::block::BranchTarget {
                    resolution: BranchTargetResolution::Unresolved,
                    ..
                }
            }
        ));
        let dispatched = lower_for_dispatch(&built, &admission(&forward)).unwrap();
        let LoweringOutcome::Program(dispatched) = dispatched else {
            panic!("admitted Jcc must produce a dispatchable prefix");
        };
        assert_eq!(dispatched.program.guest_instructions.len(), 1);
        assert!(matches!(
            dispatched.program.operations.last(),
            Some(Operation::DispatchBranch {
                condition: BranchCondition::Zero,
                target_rip,
                fallthrough_rip,
                ..
            }) if *target_rip == BASE + 4 && *fallthrough_rip == BASE + 2
        ));
        assert_eq!(
            dispatched
                .program
                .operations
                .iter()
                .filter(|operation| matches!(operation, Operation::CommitInstruction { .. }))
                .count(),
            1
        );
        ir::verify(&dispatched.program).unwrap();

        // A target inside an admitted instruction is not promoted to a
        // dispatch boundary, even though its scalar RIP is in the code view.
        let mut malformed = dispatched.program;
        let Some(Operation::DispatchBranch { target_rip, .. }) = malformed.operations.last_mut()
        else {
            unreachable!();
        };
        *target_rip = BASE + 3;
        assert!(matches!(
            ir::verify(&malformed),
            Err(ir::VerifyError::InvalidDispatchBoundary { rip, .. }) if rip == BASE + 3
        ));
    }

    #[test]
    fn dispatch_lowering_covers_backward_and_code_end_edges_but_one_block_stays_legacy() {
        let backward = [
            0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0, // MOV RAX, 1
            0x74, 0xF4, // JZ entry
            0x74, 0x00, // valid fixture tail
        ];
        let backward_program =
            match lower_for_dispatch(&build(&backward), &admission(&backward)).unwrap() {
                LoweringOutcome::Program(lowered) => lowered.program,
                other => panic!("expected dispatch program, got {other:?}"),
            };
        assert!(matches!(
            backward_program.operations.last(),
            Some(Operation::DispatchBranch {
                target_rip: BASE,
                fallthrough_rip,
                ..
            }) if *fallthrough_rip == BASE + 12
        ));

        let code_end = [0x75, 0x00]; // JNZ target and fallthrough are code_end.
        let code_end_program =
            match lower_for_dispatch(&build(&code_end), &admission(&code_end)).unwrap() {
                LoweringOutcome::Program(lowered) => lowered.program,
                other => panic!("expected code_end dispatch program, got {other:?}"),
            };
        assert!(matches!(
            code_end_program.operations.last(),
            Some(Operation::DispatchBranch {
                condition: BranchCondition::NotZero,
                target_rip,
                fallthrough_rip,
                ..
            }) if *target_rip == BASE + 2 && *fallthrough_rip == BASE + 2
        ));

        // Without fixture-admission proof, the existing one-block API keeps
        // Jcc out of the retired prefix and requests a later execution step.
        let mut one_block = vec![0x48, 0xB8];
        one_block.extend_from_slice(&1_u64.to_le_bytes());
        one_block.extend_from_slice(&[0x74, 0x02, 0x74, 0x00, 0x74, 0x00]);
        let legacy = lower(&build(&one_block)).unwrap();
        let LoweringOutcome::Program(legacy) = legacy else {
            panic!("MOV prefix should remain compilable");
        };
        assert!(
            !legacy
                .program
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::DispatchBranch { .. }))
        );
        assert!(
            !legacy
                .program
                .operations
                .iter()
                .any(|operation| matches!(operation, Operation::Branch { .. }))
        );
        assert!(matches!(
            legacy.program.operations.last(),
            Some(Operation::Exit {
                kind: ExitKind::NeedInterpreter,
                target_rip,
            })
            if *target_rip == BASE + 10
        ));
    }

    #[test]
    fn unsupported_target_is_not_retired_and_decode_boundary_stays_deferred() {
        let mut bytes = mov_rax(7);
        bytes.extend([0x74, 0xf7]); // target inside the MOV immediate
        let lowered = program(&bytes);
        assert_eq!(lowered.program.guest_instructions.len(), 1);
        assert!(
            matches!(lowered.program.operations.last(), Some(Operation::Exit { kind: ExitKind::Unsupported, target_rip: rip }) if *rip == BASE + 10)
        );
        assert_eq!(
            lowered
                .program
                .operations
                .iter()
                .filter(|op| matches!(op, Operation::CommitInstruction { .. }))
                .count(),
            1
        );

        let mut truncated = mov_rax(9);
        truncated.push(0x48);
        let lowered = program(&truncated);
        assert_eq!(
            lowered.deferred_error.as_ref().unwrap().guest_rip,
            BASE + 10
        );
        assert!(
            matches!(lowered.program.operations.last(), Some(Operation::Exit { kind: ExitKind::NeedInterpreter, target_rip: rip }) if *rip == BASE + 10)
        );

        let first_truncated = build(&[0x48]);
        assert!(
            matches!(lower(&first_truncated).unwrap(), LoweringOutcome::NoNativePrefix { rip: BASE, deferred_error: Some(boundary), .. } if boundary.guest_rip == BASE)
        );
    }

    #[test]
    fn unsupported_decoded_form_and_encoding_exit_before_problem_instruction() {
        let mut decoded_form = mov_rax(4);
        decoded_form.push(0x90);
        let lowered = program(&decoded_form);
        assert_eq!(lowered.program.guest_instructions.len(), 1);
        assert!(matches!(
            lowered.program.operations.last(),
            Some(Operation::Exit {
                kind: ExitKind::Unsupported,
                target_rip: 0x100a
            })
        ));

        let mut outcome = build(&mov_rax(4));
        let BuildOutcome::Block(block) = &mut outcome else {
            panic!("MOV should form a prefix block");
        };
        block.stop_reason = BlockStopReason::UnsupportedEncoding {
            guest_rip: 0x100a,
            error: od_isa::long_decode::DecodeError::Unsupported {
                opcode: od_isa::long_decode::Opcode {
                    map: od_isa::long_decode::OpcodeMap::Primary,
                    byte: 0xff,
                },
                length: 1,
            },
        };
        let lowered = match lower(&outcome).unwrap() {
            LoweringOutcome::Program(lowered) => lowered,
            other => panic!("expected program, got {other:?}"),
        };
        assert_eq!(lowered.program.guest_instructions.len(), 1);
        assert!(matches!(
            lowered.program.operations.last(),
            Some(Operation::Exit {
                kind: ExitKind::Unsupported,
                target_rip: 0x100a
            })
        ));
    }

    #[test]
    fn code_end_is_the_only_non_record_branch_target_admitted_by_verifier() {
        let mut lowered = program(&[0x74, 0x03, 0x90, 0x90, 0x90]);
        let Operation::Branch {
            condition,
            fallthrough_rip,
            ..
        } = *lowered.program.operations.last().unwrap()
        else {
            panic!("branch expected")
        };
        *lowered.program.operations.last_mut().unwrap() = Operation::Branch {
            condition,
            target_rip: lowered.program.code_end + 1,
            fallthrough_rip,
        };
        assert!(matches!(
            ir::verify(&lowered.program),
            Err(ir::VerifyError::InvalidBranchTarget { .. })
        ));
    }
}
