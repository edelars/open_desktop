//! ARM64 emission for uncommitted data effects in the first W64 block profile.
//!
//! This code reads and writes isolated staging GPR/flags/RIP values plus an
//! observation array. It never updates `LongModeState`, commits an instruction,
//! or returns a guest runtime exit tag.

use crate::allocator::{Allocation, AllocatorError, HelperBoundaryPosition, ValueLocation};
use crate::arm64::{
    AddressBase, EncodeError, EncodedCode, Encoder, PairIndexMode, XReg, XRegOrZero,
};
use crate::ir::{
    self, BranchCondition, ExitKind, FlagEffect, GuestGpr, IntWidth, IntegerKind, Operation,
    Program, TempId, VerifyError,
};
use crate::runtime::{ArithmeticFlagsHelper, HelperOperationTag, RAW_HELPER_CONTRACT_FAILURE_V1};
use od_isa::integer_flags;

pub const DATA_EFFECTS_UNCOMMITTED_V1: u32 = 0xD3E0_0001;
const DATA_EFFECT_CONTEXT_STAGING_GPRS_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV2, staging_gprs) as u32;
const DATA_EFFECT_CONTEXT_OUTPUTS_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV2, output_values) as u32;
const DATA_EFFECT_CONTEXT_STAGING_RFLAGS_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV2, staging_rflags) as u32;
const DATA_EFFECT_CONTEXT_STAGING_RIP_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV2, staging_rip) as u32;
const MAX_DATA_EFFECT_FRAME_BYTES: u32 = 32_768;
const MIN_RESERVED_PREFIX_BYTES: u32 = 40;
const FRAME_STAGING_GPRS_POINTER_OFFSET: u32 = 0;
const FRAME_OUTPUTS_POINTER_OFFSET: u32 = 8;
const FRAME_STAGING_RFLAGS_POINTER_OFFSET: u32 = 16;
const FRAME_STAGING_RIP_POINTER_OFFSET: u32 = 24;
const FRAME_HELPER_RFLAGS_OUTPUT_OFFSET: u32 = 32;

/// Diagnostic staging ABI. Every pointer must be valid, writable, and separate
/// from canonical guest state for the duration of the unsafe generated call.
/// `output_values` must have at least the emitted output count of writable
/// `u64` slots. This is not a runtime-entry context.
#[repr(C)]
pub struct DataEffectContextV2 {
    pub staging_gprs: *mut u64,
    pub output_values: *mut u64,
    pub staging_rflags: *mut u64,
    pub staging_rip: *mut u64,
}

/// Raw return value for the data-effect-only entry. It is deliberately not a
/// `RuntimeExitTag`; it says that outputs are uncommitted observations only.
pub type DataEffectEntry = unsafe extern "C" fn(*mut DataEffectContextV2) -> u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataEffectOutputKind {
    Temporary(TempId),
    GuestRegisterCandidate {
        instruction_index: usize,
        guest_rip: u64,
        register: GuestGpr,
        source: TempId,
    },
    CompareScratch {
        instruction_index: usize,
        guest_rip: u64,
        operation_index: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataEffectOutput {
    pub slot: u32,
    pub kind: DataEffectOutputKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagMaterialization {
    Preserved,
    ArithmeticComplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataEffectTerminal {
    BlockComplete {
        target_rip: u64,
    },
    Branch {
        condition: BranchCondition,
        target_rip: u64,
        fallthrough_rip: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedDataEffects {
    pub code: EncodedCode,
    pub outputs: Vec<DataEffectOutput>,
    pub flags: FlagMaterialization,
    pub source_instruction_count: usize,
    /// This emitter is a result probe and never crosses a guest commit point.
    pub committed_guest_instructions: usize,
    pub terminal: DataEffectTerminal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmitterError {
    InvalidProgram(VerifyError),
    Allocation(AllocatorError),
    AllocationProgramMismatch,
    MissingHelperBoundary {
        operation: usize,
    },
    InsufficientFramePrefix {
        required: u32,
        actual: u32,
    },
    UnsupportedWidth {
        operation: usize,
        width: IntWidth,
    },
    UnsupportedFlagEffect {
        operation: usize,
        effect: FlagEffect,
    },
    UnsupportedTerminal {
        operation: usize,
    },
    UnsupportedExitKind {
        operation: usize,
        kind: ExitKind,
    },
    InvalidOperation {
        operation: usize,
    },
    OutputOffsetOutOfRange {
        slot: usize,
    },
    FrameTooLarge,
    Encoding(EncodeError),
}

impl From<AllocatorError> for EmitterError {
    fn from(error: AllocatorError) -> Self {
        Self::Allocation(error)
    }
}

impl From<EncodeError> for EmitterError {
    fn from(error: EncodeError) -> Self {
        Self::Encoding(error)
    }
}

/// Emit W64 arithmetic/branch candidates for a verifier-checked program.
/// The entry calls `helper` at explicitly allocated pre-operation boundaries.
/// GPRs, flags, and RIP are written only to isolated staging storage; no guest
/// architectural commit or successful runtime exit is performed.
pub fn emit_data_effects(
    program: &Program,
    allocation: &Allocation<'_>,
    helper: ArithmeticFlagsHelper,
) -> Result<EmittedDataEffects, EmitterError> {
    ir::verify(program).map_err(EmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(EmitterError::AllocationProgramMismatch);
    }
    if allocation.frame().reserved_prefix_bytes < MIN_RESERVED_PREFIX_BYTES {
        return Err(EmitterError::InsufficientFramePrefix {
            required: MIN_RESERVED_PREFIX_BYTES,
            actual: allocation.frame().reserved_prefix_bytes,
        });
    }

    let terminal_index = program
        .operations
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Branch { .. } | Operation::Exit { .. })
        })
        .ok_or(EmitterError::InvalidProgram(VerifyError::MissingTerminal))?;
    let terminal = match program.operations.get(terminal_index) {
        Some(Operation::Exit {
            kind: ExitKind::BlockComplete,
            target_rip,
        }) => DataEffectTerminal::BlockComplete {
            target_rip: *target_rip,
        },
        Some(Operation::Exit { kind, .. }) => {
            return Err(EmitterError::UnsupportedExitKind {
                operation: terminal_index,
                kind: *kind,
            });
        }
        Some(Operation::Branch {
            condition,
            target_rip,
            fallthrough_rip,
        }) => DataEffectTerminal::Branch {
            condition: *condition,
            target_rip: *target_rip,
            fallthrough_rip: *fallthrough_rip,
        },
        _ => return Err(EmitterError::InvalidProgram(VerifyError::MissingTerminal)),
    };

    let mut output_kinds = Vec::new();
    let mut operation_output_slots = vec![None; program.operations.len()];
    let mut active_instruction = None;
    let mut has_arithmetic = false;

    for (operation_index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { index } => active_instruction = Some(*index),
            Operation::CommitInstruction { .. } => active_instruction = None,
            Operation::Constant { dst, width, .. } | Operation::ReadGpr { dst, width, .. } => {
                require_w64(*width, operation_index)?;
                operation_output_slots[operation_index] = Some(push_output(
                    &mut output_kinds,
                    DataEffectOutputKind::Temporary(*dst),
                )?);
            }
            Operation::Integer {
                kind,
                result,
                width,
                flags,
                ..
            } => {
                require_w64(*width, operation_index)?;
                if *flags != FlagEffect::Arithmetic {
                    return Err(EmitterError::UnsupportedFlagEffect {
                        operation: operation_index,
                        effect: *flags,
                    });
                }
                has_arithmetic = true;
                if allocation
                    .helper_boundary(HelperBoundaryPosition::BeforeOperation(operation_index))
                    .is_err()
                {
                    return Err(EmitterError::MissingHelperBoundary {
                        operation: operation_index,
                    });
                }
                match (kind, result) {
                    (IntegerKind::Compare, None) => {
                        let instruction_index =
                            active_instruction.ok_or(EmitterError::InvalidOperation {
                                operation: operation_index,
                            })?;
                        let guest_rip = program.guest_instructions[instruction_index].guest_rip;
                        operation_output_slots[operation_index] = Some(push_output(
                            &mut output_kinds,
                            DataEffectOutputKind::CompareScratch {
                                instruction_index,
                                guest_rip,
                                operation_index,
                            },
                        )?);
                    }
                    (IntegerKind::Add | IntegerKind::Subtract, Some(dst)) => {
                        operation_output_slots[operation_index] = Some(push_output(
                            &mut output_kinds,
                            DataEffectOutputKind::Temporary(*dst),
                        )?);
                    }
                    _ => {
                        return Err(EmitterError::InvalidOperation {
                            operation: operation_index,
                        });
                    }
                }
            }
            Operation::WriteGpr {
                register,
                src,
                width,
            } => {
                require_w64(*width, operation_index)?;
                let instruction_index =
                    active_instruction.ok_or(EmitterError::InvalidOperation {
                        operation: operation_index,
                    })?;
                let guest_rip = program.guest_instructions[instruction_index].guest_rip;
                operation_output_slots[operation_index] = Some(push_output(
                    &mut output_kinds,
                    DataEffectOutputKind::GuestRegisterCandidate {
                        instruction_index,
                        guest_rip,
                        register: *register,
                        source: *src,
                    },
                )?);
            }
            Operation::Branch { .. } | Operation::Exit { .. } => {}
        }
    }

    let outputs: Vec<_> = output_kinds
        .into_iter()
        .enumerate()
        .map(|(slot, kind)| {
            Ok(DataEffectOutput {
                slot: output_slot(slot)?,
                kind,
            })
        })
        .collect::<Result<_, EmitterError>>()?;

    let mut encoder = Encoder::new();
    let frame = allocation.frame();
    if frame.frame_bytes % frame.alignment != 0 || frame.frame_bytes > MAX_DATA_EFFECT_FRAME_BYTES {
        return Err(EmitterError::FrameTooLarge);
    }
    let pair_count = frame.frame_bytes / 16;
    let pair_count = usize::try_from(pair_count).map_err(|_| EmitterError::FrameTooLarge)?;
    let helper_failure = encoder.new_label()?;
    let epilogue = encoder.new_label()?;

    // Save LR and establish the frame chain before allocating local slots.
    encoder.stp_x(
        xreg(29)?,
        xreg(30)?,
        AddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.adds_imm_x(XRegOrZero::Reg(xreg(29)?), crate::arm64::XRegOrSp::Sp, 0)?;
    let mut pair_offset = 0usize;
    while pair_offset < pair_count {
        encoder.stp_x(
            xreg(13)?,
            xreg(14)?,
            AddressBase::Sp,
            -16,
            PairIndexMode::PreIndex,
        )?;
        pair_offset += 1;
    }
    for (context_offset, frame_offset) in [
        (
            DATA_EFFECT_CONTEXT_STAGING_GPRS_OFFSET,
            FRAME_STAGING_GPRS_POINTER_OFFSET,
        ),
        (
            DATA_EFFECT_CONTEXT_OUTPUTS_OFFSET,
            FRAME_OUTPUTS_POINTER_OFFSET,
        ),
        (
            DATA_EFFECT_CONTEXT_STAGING_RFLAGS_OFFSET,
            FRAME_STAGING_RFLAGS_POINTER_OFFSET,
        ),
        (
            DATA_EFFECT_CONTEXT_STAGING_RIP_OFFSET,
            FRAME_STAGING_RIP_POINTER_OFFSET,
        ),
    ] {
        encoder.ldr_x(
            XRegOrZero::Reg(xreg(16)?),
            AddressBase::Reg(xreg(0)?),
            context_offset,
        )?;
        encoder.str_x(XRegOrZero::Reg(xreg(16)?), AddressBase::Sp, frame_offset)?;
    }

    for (operation_index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { .. } | Operation::CommitInstruction { .. } => {}
            Operation::Constant { dst, value, .. } => {
                emit_constant(&mut encoder, allocation, *dst, *value)?;
                allocation.emit_load_scratch(&mut encoder, *dst, xreg(13)?)?;
                emit_temp_snapshot(
                    &mut encoder,
                    operation_output_slots[operation_index].ok_or(
                        EmitterError::InvalidOperation {
                            operation: operation_index,
                        },
                    )?,
                    xreg(13)?,
                )?;
            }
            Operation::ReadGpr { dst, register, .. } => {
                let byte_offset = u32::from(register.0)
                    .checked_mul(8)
                    .ok_or(EmitterError::OutputOffsetOutOfRange { slot: usize::MAX })?;
                emit_saved_pointer_load(
                    &mut encoder,
                    FRAME_STAGING_GPRS_POINTER_OFFSET,
                    xreg(16)?,
                )?;
                match allocation.location(*dst)? {
                    ValueLocation::Register(home) => encoder.ldr_x(
                        XRegOrZero::Reg(home),
                        AddressBase::Reg(xreg(16)?),
                        byte_offset,
                    )?,
                    ValueLocation::Spill(_) => {
                        encoder.ldr_x(
                            XRegOrZero::Reg(xreg(13)?),
                            AddressBase::Reg(xreg(16)?),
                            byte_offset,
                        )?;
                        allocation.emit_store_scratch(&mut encoder, *dst, xreg(13)?)?;
                    }
                }
                allocation.emit_load_scratch(&mut encoder, *dst, xreg(13)?)?;
                emit_temp_snapshot(
                    &mut encoder,
                    operation_output_slots[operation_index].ok_or(
                        EmitterError::InvalidOperation {
                            operation: operation_index,
                        },
                    )?,
                    xreg(13)?,
                )?;
            }
            Operation::Integer {
                kind,
                lhs,
                rhs,
                result,
                ..
            } => {
                allocation.emit_save_before_helper(
                    &mut encoder,
                    HelperBoundaryPosition::BeforeOperation(operation_index),
                )?;
                allocation.emit_load_scratch(&mut encoder, *lhs, xreg(13)?)?;
                allocation.emit_load_scratch(&mut encoder, *rhs, xreg(14)?)?;
                encoder.mov_x(xreg(1)?, XRegOrZero::Reg(xreg(13)?));
                encoder.mov_x(xreg(2)?, XRegOrZero::Reg(xreg(14)?));
                encoder.movz_x(xreg(0)?, helper_operation_tag(*kind)? as u16, 0)?;
                emit_saved_pointer_load(
                    &mut encoder,
                    FRAME_STAGING_RFLAGS_POINTER_OFFSET,
                    xreg(16)?,
                )?;
                encoder.ldr_x(XRegOrZero::Reg(xreg(3)?), AddressBase::Reg(xreg(16)?), 0)?;
                encoder.adds_imm_x(
                    XRegOrZero::Reg(xreg(4)?),
                    crate::arm64::XRegOrSp::Sp,
                    FRAME_HELPER_RFLAGS_OUTPUT_OFFSET as u16,
                )?;
                emit_u64_immediate(&mut encoder, xreg(16)?, helper as usize as u64)?;
                encoder.blr(xreg(16)?);
                let helper_succeeded = encoder.new_label()?;
                encoder.cbz_w(crate::arm64::WReg::new(0)?, helper_succeeded)?;
                encoder.b(helper_failure)?;
                encoder.bind(helper_succeeded)?;
                allocation.emit_restore_after_helper(
                    &mut encoder,
                    HelperBoundaryPosition::BeforeOperation(operation_index),
                )?;
                encoder.ldr_x(
                    XRegOrZero::Reg(xreg(13)?),
                    AddressBase::Sp,
                    FRAME_HELPER_RFLAGS_OUTPUT_OFFSET,
                )?;
                emit_saved_pointer_load(
                    &mut encoder,
                    FRAME_STAGING_RFLAGS_POINTER_OFFSET,
                    xreg(16)?,
                )?;
                encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
                allocation.emit_load_scratch(&mut encoder, *lhs, xreg(13)?)?;
                allocation.emit_load_scratch(&mut encoder, *rhs, xreg(14)?)?;
                match kind {
                    IntegerKind::Add => {
                        encoder.add_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        let result = result.ok_or(EmitterError::InvalidOperation {
                            operation: operation_index,
                        })?;
                        allocation.emit_store_scratch(&mut encoder, result, xreg(15)?)?;
                        emit_temp_snapshot(
                            &mut encoder,
                            operation_output_slots[operation_index].ok_or(
                                EmitterError::InvalidOperation {
                                    operation: operation_index,
                                },
                            )?,
                            xreg(15)?,
                        )?;
                    }
                    IntegerKind::Subtract => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        let result = result.ok_or(EmitterError::InvalidOperation {
                            operation: operation_index,
                        })?;
                        allocation.emit_store_scratch(&mut encoder, result, xreg(15)?)?;
                        emit_temp_snapshot(
                            &mut encoder,
                            operation_output_slots[operation_index].ok_or(
                                EmitterError::InvalidOperation {
                                    operation: operation_index,
                                },
                            )?,
                            xreg(15)?,
                        )?;
                    }
                    IntegerKind::Compare => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        emit_temp_snapshot(
                            &mut encoder,
                            operation_output_slots[operation_index].ok_or(
                                EmitterError::InvalidOperation {
                                    operation: operation_index,
                                },
                            )?,
                            xreg(15)?,
                        )?;
                    }
                }
            }
            Operation::WriteGpr { register, src, .. } => {
                allocation.emit_load_scratch(&mut encoder, *src, xreg(13)?)?;
                let byte_offset = u32::from(register.0)
                    .checked_mul(8)
                    .ok_or(EmitterError::OutputOffsetOutOfRange { slot: usize::MAX })?;
                emit_saved_pointer_load(
                    &mut encoder,
                    FRAME_STAGING_GPRS_POINTER_OFFSET,
                    xreg(16)?,
                )?;
                encoder.str_x(
                    XRegOrZero::Reg(xreg(13)?),
                    AddressBase::Reg(xreg(16)?),
                    byte_offset,
                )?;
                emit_temp_snapshot(
                    &mut encoder,
                    operation_output_slots[operation_index].ok_or(
                        EmitterError::InvalidOperation {
                            operation: operation_index,
                        },
                    )?,
                    xreg(13)?,
                )?;
            }
            Operation::Exit { .. } | Operation::Branch { .. } => {}
        }
    }

    match terminal {
        DataEffectTerminal::BlockComplete { target_rip } => {
            emit_rip_candidate(&mut encoder, target_rip)?;
        }
        DataEffectTerminal::Branch {
            condition,
            target_rip,
            fallthrough_rip,
        } => emit_branch_candidate(&mut encoder, condition, target_rip, fallthrough_rip)?,
    }

    emit_u64_immediate(
        &mut encoder,
        xreg(0)?,
        u64::from(DATA_EFFECTS_UNCOMMITTED_V1),
    )?;
    encoder.b(epilogue)?;
    encoder.bind(helper_failure)?;
    emit_u64_immediate(
        &mut encoder,
        xreg(0)?,
        u64::from(RAW_HELPER_CONTRACT_FAILURE_V1),
    )?;
    encoder.bind(epilogue)?;
    for _ in 0..pair_count {
        encoder.ldp_x(
            xreg(13)?,
            xreg(14)?,
            AddressBase::Sp,
            16,
            PairIndexMode::PostIndex,
        )?;
    }
    encoder.ldp_x(
        xreg(29)?,
        xreg(30)?,
        AddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ret(xreg(30)?);

    Ok(EmittedDataEffects {
        code: encoder.finish()?,
        outputs,
        flags: if has_arithmetic {
            FlagMaterialization::ArithmeticComplete
        } else {
            FlagMaterialization::Preserved
        },
        source_instruction_count: program.guest_instructions.len(),
        committed_guest_instructions: 0,
        terminal,
    })
}

fn push_output(
    output_kinds: &mut Vec<DataEffectOutputKind>,
    kind: DataEffectOutputKind,
) -> Result<u32, EmitterError> {
    let slot = output_slot(output_kinds.len())?;
    output_kinds.push(kind);
    Ok(slot)
}

fn helper_operation_tag(kind: IntegerKind) -> Result<HelperOperationTag, EmitterError> {
    match kind {
        IntegerKind::Add => Ok(HelperOperationTag::Add),
        IntegerKind::Subtract => Ok(HelperOperationTag::Subtract),
        IntegerKind::Compare => Ok(HelperOperationTag::Compare),
    }
}

fn emit_saved_pointer_load(
    encoder: &mut Encoder,
    frame_offset: u32,
    destination: XReg,
) -> Result<(), EmitterError> {
    encoder.ldr_x(XRegOrZero::Reg(destination), AddressBase::Sp, frame_offset)?;
    Ok(())
}

fn emit_temp_snapshot(encoder: &mut Encoder, slot: u32, source: XReg) -> Result<(), EmitterError> {
    emit_saved_pointer_load(encoder, FRAME_OUTPUTS_POINTER_OFFSET, xreg(16)?)?;
    encoder.str_x(
        XRegOrZero::Reg(source),
        AddressBase::Reg(xreg(16)?),
        output_byte_offset(slot as usize)?,
    )?;
    Ok(())
}

fn emit_rip_candidate(encoder: &mut Encoder, rip: u64) -> Result<(), EmitterError> {
    emit_u64_immediate(encoder, xreg(13)?, rip)?;
    emit_saved_pointer_load(encoder, FRAME_STAGING_RIP_POINTER_OFFSET, xreg(16)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(13)?), AddressBase::Reg(xreg(16)?), 0)?;
    Ok(())
}

fn emit_branch_candidate(
    encoder: &mut Encoder,
    condition: BranchCondition,
    target_rip: u64,
    fallthrough_rip: u64,
) -> Result<(), EmitterError> {
    let selected = encoder.new_label()?;
    emit_saved_pointer_load(encoder, FRAME_STAGING_RFLAGS_POINTER_OFFSET, xreg(16)?)?;
    encoder.ldr_x(XRegOrZero::Reg(xreg(17)?), AddressBase::Reg(xreg(16)?), 0)?;
    encoder.movz_x(xreg(14)?, integer_flags::ZERO_FLAG as u16, 0)?;
    encoder.and_x(xreg(15)?, xreg(17)?, XRegOrZero::Reg(xreg(14)?));
    match condition {
        BranchCondition::Zero => {
            let not_taken = encoder.new_label()?;
            encoder.cbz_x(XRegOrZero::Reg(xreg(15)?), not_taken)?;
            emit_u64_immediate(encoder, xreg(16)?, target_rip)?;
            encoder.b(selected)?;
            encoder.bind(not_taken)?;
            emit_u64_immediate(encoder, xreg(16)?, fallthrough_rip)?;
        }
        BranchCondition::NotZero => {
            let taken = encoder.new_label()?;
            encoder.cbz_x(XRegOrZero::Reg(xreg(15)?), taken)?;
            emit_u64_immediate(encoder, xreg(16)?, fallthrough_rip)?;
            encoder.b(selected)?;
            encoder.bind(taken)?;
            emit_u64_immediate(encoder, xreg(16)?, target_rip)?;
        }
    }
    encoder.bind(selected)?;
    emit_saved_pointer_load(encoder, FRAME_STAGING_RIP_POINTER_OFFSET, xreg(17)?)?;
    encoder.str_x(XRegOrZero::Reg(xreg(16)?), AddressBase::Reg(xreg(17)?), 0)?;
    Ok(())
}

fn emit_u64_immediate(
    encoder: &mut Encoder,
    destination: XReg,
    value: u64,
) -> Result<(), EmitterError> {
    encoder.movz_x(destination, value as u16, 0)?;
    for halfword in 1..4 {
        encoder.movk_x(
            destination,
            (value >> (halfword * 16)) as u16,
            halfword as u8,
        )?;
    }
    Ok(())
}

fn emit_constant(
    encoder: &mut Encoder,
    allocation: &Allocation<'_>,
    temp: TempId,
    value: u64,
) -> Result<(), EmitterError> {
    let destination = match allocation.location(temp)? {
        ValueLocation::Register(register) => register,
        ValueLocation::Spill(_) => xreg(13)?,
    };
    encoder.movz_x(destination, value as u16, 0)?;
    for halfword in 1..4 {
        encoder.movk_x(
            destination,
            (value >> (halfword * 16)) as u16,
            halfword as u8,
        )?;
    }
    if matches!(allocation.location(temp)?, ValueLocation::Spill(_)) {
        allocation.emit_store_scratch(encoder, temp, destination)?;
    }
    Ok(())
}

fn require_w64(width: IntWidth, operation: usize) -> Result<(), EmitterError> {
    if width == IntWidth::W64 {
        Ok(())
    } else {
        Err(EmitterError::UnsupportedWidth { operation, width })
    }
}

fn output_slot(slot: usize) -> Result<u32, EmitterError> {
    u32::try_from(slot).map_err(|_| EmitterError::OutputOffsetOutOfRange { slot })
}

fn output_byte_offset(slot: usize) -> Result<u32, EmitterError> {
    let slot = output_slot(slot)?;
    slot.checked_mul(8)
        .ok_or(EmitterError::OutputOffsetOutOfRange {
            slot: slot as usize,
        })
}

fn xreg(number: u8) -> Result<XReg, EmitterError> {
    XReg::new(number).map_err(EmitterError::Encoding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block::{self, BlockLimits, CodeView};
    use crate::lowering::{self, LoweringOutcome};
    use od_isa::memory::AccessContext;

    const BASE_RIP: u64 = 0x4000;

    fn lower_bytes(bytes: &[u8]) -> Program {
        let code = CodeView::new(BASE_RIP, bytes).unwrap();
        let block = block::build_block(
            code,
            BASE_RIP,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        match lowering::lower(&block).unwrap() {
            LoweringOutcome::Program(lowered) => lowered.program,
            LoweringOutcome::NoNativePrefix { .. } => panic!("test block has no native prefix"),
        }
    }

    fn allocation(program: &Program, register_homes: u8) -> Allocation<'_> {
        let boundaries: Vec<_> = program
            .operations
            .iter()
            .enumerate()
            .filter_map(|(index, operation)| {
                matches!(operation, Operation::Integer { .. })
                    .then_some(HelperBoundaryPosition::BeforeOperation(index))
            })
            .collect();
        Allocation::build(
            program,
            crate::allocator::AllocatorConfig {
                register_homes,
                reserved_prefix_bytes: MIN_RESERVED_PREFIX_BYTES,
            },
            &boundaries,
        )
        .unwrap()
    }

    #[test]
    fn emits_decoder_derived_data_effects_with_each_supported_budget() {
        let mut bytes = vec![0x48, 0xB8];
        bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // MOV RAX, -1
        bytes.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX, RCX
        bytes.extend_from_slice(&[0x48, 0x2B, 0xD2]); // SUB RDX, RDX
        bytes.extend_from_slice(&[0x48, 0x3B, 0xC1]); // CMP RAX, RCX
        let program = lower_bytes(&bytes);

        for register_homes in 0..=4 {
            let allocation = allocation(&program, register_homes);
            let emitted = emit_data_effects(
                &program,
                &allocation,
                crate::runtime::arithmetic_flags_helper_adapter,
            )
            .unwrap();
            assert!(!emitted.code.words().is_empty());
            assert_eq!(emitted.code.to_le_bytes().unwrap().len() % 4, 0);
            assert_eq!(emitted.flags, FlagMaterialization::ArithmeticComplete);
            assert_eq!(emitted.committed_guest_instructions, 0);
            assert_eq!(emitted.source_instruction_count, 4);
            let temporary_ids: Vec<_> = emitted
                .outputs
                .iter()
                .filter_map(|output| match output.kind {
                    DataEffectOutputKind::Temporary(temp) => Some(temp.0),
                    _ => None,
                })
                .collect();
            assert_eq!(temporary_ids, (0..9).collect::<Vec<_>>());
            assert!(
                emitted.outputs.iter().any(|output| matches!(
                    output.kind,
                    DataEffectOutputKind::CompareScratch { .. }
                ))
            );
            assert!(emitted.outputs.iter().any(|output| matches!(
                output.kind,
                DataEffectOutputKind::GuestRegisterCandidate {
                    register: GuestGpr(0),
                    ..
                }
            )));
        }
    }

    #[test]
    fn accepts_branch_terminal_and_rejects_non_arithmetic_flags() {
        let mut branch_bytes = vec![0x48, 0xB8];
        branch_bytes.extend_from_slice(&9u64.to_le_bytes());
        branch_bytes.extend_from_slice(&[0x74, 0x00]);
        let branch_program = lower_bytes(&branch_bytes);
        let branch_allocation = allocation(&branch_program, 4);
        let emitted = emit_data_effects(
            &branch_program,
            &branch_allocation,
            crate::runtime::arithmetic_flags_helper_adapter,
        )
        .unwrap();
        assert!(matches!(
            emitted.terminal,
            DataEffectTerminal::Branch { .. }
        ));

        let arithmetic_bytes = [0x48, 0x03, 0xC1]; // ADD RAX, RCX
        let mut program = lower_bytes(&arithmetic_bytes);
        let integer = program
            .operations
            .iter_mut()
            .find(|operation| matches!(operation, Operation::Integer { .. }))
            .unwrap();
        if let Operation::Integer { flags, .. } = integer {
            *flags = FlagEffect::Preserve;
        }
        let allocation = allocation(&program, 4);
        assert!(matches!(
            emit_data_effects(
                &program,
                &allocation,
                crate::runtime::arithmetic_flags_helper_adapter
            ),
            Err(EmitterError::UnsupportedFlagEffect { .. })
        ));
    }

    #[test]
    fn allocator_and_emitter_reject_non_w64_programs_and_wrong_program_mapping() {
        let bytes = [0x48, 0xB8, 1, 0, 0, 0, 0, 0, 0, 0];
        let mut program = lower_bytes(&bytes);
        for operation in &mut program.operations {
            match operation {
                Operation::Constant { width, .. } | Operation::WriteGpr { width, .. } => {
                    *width = IntWidth::W32;
                }
                _ => {}
            }
        }
        assert!(matches!(
            Allocation::build(&program, crate::allocator::AllocatorConfig::default(), &[]),
            Err(AllocatorError::UnsupportedWidth {
                width: IntWidth::W32,
                ..
            })
        ));

        let program = lower_bytes(&bytes);
        let allocation = allocation(&program, 4);
        let clone = program.clone();
        assert_eq!(
            emit_data_effects(
                &clone,
                &allocation,
                crate::runtime::arithmetic_flags_helper_adapter
            )
            .unwrap_err(),
            EmitterError::AllocationProgramMismatch
        );
    }
}
