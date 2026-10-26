//! ARM64 emission for uncommitted data effects in the first W64 block profile.
//!
//! This code reads a staging GPR array and writes effect observations to a
//! separate output array. It never updates `LongModeState`, materializes x86
//! flags, commits an instruction, or returns a guest runtime exit tag.

use crate::allocator::{Allocation, AllocatorError, ValueLocation};
use crate::arm64::{
    AddressBase, EncodeError, EncodedCode, Encoder, PairIndexMode, XReg, XRegOrZero,
};
use crate::ir::{
    self, ExitKind, FlagEffect, GuestGpr, IntWidth, IntegerKind, Operation, Program, TempId,
    VerifyError,
};

pub const DATA_EFFECTS_UNCOMMITTED_V1: u32 = 0xD3E0_0001;
const DATA_EFFECT_CONTEXT_STAGING_GPRS_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV1, staging_gprs) as u32;
const DATA_EFFECT_CONTEXT_OUTPUTS_OFFSET: u32 =
    std::mem::offset_of!(DataEffectContextV1, output_values) as u32;
const MAX_DATA_EFFECT_FRAME_BYTES: u32 = 32_768;

/// Test/diagnostic staging ABI. `staging_gprs` must point to a separate mutable
/// four-register copy initialized by the caller; `output_values` must point to
/// at least the emitted output count of writable u64 slots. Neither pointer may
/// alias canonical guest state. This is not a runtime-entry context.
#[repr(C)]
pub struct DataEffectContextV1 {
    pub staging_gprs: *mut u64,
    pub output_values: *mut u64,
}

/// Raw return value for the data-effect-only entry. It is deliberately not a
/// `RuntimeExitTag`; it says that outputs are uncommitted observations only.
pub type DataEffectEntry = unsafe extern "C" fn(*mut DataEffectContextV1) -> u32;

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
    ArithmeticIncomplete,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmittedDataEffects {
    pub code: EncodedCode,
    pub outputs: Vec<DataEffectOutput>,
    pub flags: FlagMaterialization,
    pub source_instruction_count: usize,
    /// This emitter is a result probe and never crosses a guest commit point.
    pub committed_guest_instructions: usize,
    pub terminal_target_rip: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EmitterError {
    InvalidProgram(VerifyError),
    Allocation(AllocatorError),
    AllocationProgramMismatch,
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

/// Emit a straight-line W64 data-effect probe for a verifier-checked program.
/// The resulting entry takes `DataEffectContextV1*` in x0 and returns
/// `DATA_EFFECTS_UNCOMMITTED_V1`. Callers must treat all staged outputs as
/// observations: no guest state, flags, RIP, budget, retirement count, or exit
/// state has been updated. The allocation must be built from this same Program.
pub fn emit_data_effects(
    program: &Program,
    allocation: &Allocation<'_>,
) -> Result<EmittedDataEffects, EmitterError> {
    ir::verify(program).map_err(EmitterError::InvalidProgram)?;
    if !std::ptr::eq(program, allocation.program()) {
        return Err(EmitterError::AllocationProgramMismatch);
    }

    let terminal_index = program
        .operations
        .iter()
        .position(|operation| {
            matches!(operation, Operation::Branch { .. } | Operation::Exit { .. })
        })
        .ok_or(EmitterError::InvalidProgram(VerifyError::MissingTerminal))?;
    let terminal_target_rip = match program.operations.get(terminal_index) {
        Some(Operation::Exit {
            kind: ExitKind::BlockComplete,
            target_rip,
        }) => *target_rip,
        Some(Operation::Exit { kind, .. }) => {
            return Err(EmitterError::UnsupportedExitKind {
                operation: terminal_index,
                kind: *kind,
            });
        }
        Some(Operation::Branch { .. }) => {
            return Err(EmitterError::UnsupportedTerminal {
                operation: terminal_index,
            });
        }
        _ => return Err(EmitterError::InvalidProgram(VerifyError::MissingTerminal)),
    };

    let mut output_kinds = Vec::new();
    let mut compare_slots = vec![None; program.operations.len()];
    let mut active_instruction = None;
    let mut arithmetic_flags_incomplete = false;

    for (operation_index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { index } => active_instruction = Some(*index),
            Operation::CommitInstruction { .. } => active_instruction = None,
            Operation::Constant { dst, width, .. } | Operation::ReadGpr { dst, width, .. } => {
                require_w64(*width, operation_index)?;
                output_slot(output_kinds.len())?;
                output_kinds.push(DataEffectOutputKind::Temporary(*dst));
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
                arithmetic_flags_incomplete = true;
                match (kind, result) {
                    (IntegerKind::Compare, None) => {
                        let instruction_index =
                            active_instruction.ok_or(EmitterError::InvalidOperation {
                                operation: operation_index,
                            })?;
                        let guest_rip = program.guest_instructions[instruction_index].guest_rip;
                        let slot = output_slot(output_kinds.len())?;
                        compare_slots[operation_index] = Some(slot);
                        output_kinds.push(DataEffectOutputKind::CompareScratch {
                            instruction_index,
                            guest_rip,
                            operation_index,
                        });
                    }
                    (IntegerKind::Add | IntegerKind::Subtract, Some(dst)) => {
                        output_slot(output_kinds.len())?;
                        output_kinds.push(DataEffectOutputKind::Temporary(*dst));
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
                output_kinds.push(DataEffectOutputKind::GuestRegisterCandidate {
                    instruction_index,
                    guest_rip,
                    register: *register,
                    source: *src,
                });
            }
            Operation::Branch { .. } => {
                return Err(EmitterError::UnsupportedTerminal {
                    operation: operation_index,
                });
            }
            Operation::Exit { .. } => {}
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
    encoder.ldr_x(
        XRegOrZero::Reg(xreg(7)?),
        AddressBase::Reg(xreg(0)?),
        DATA_EFFECT_CONTEXT_STAGING_GPRS_OFFSET,
    )?;
    if !outputs.is_empty() {
        encoder.ldr_x(
            XRegOrZero::Reg(xreg(8)?),
            AddressBase::Reg(xreg(0)?),
            DATA_EFFECT_CONTEXT_OUTPUTS_OFFSET,
        )?;
    }

    for (operation_index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { .. } | Operation::CommitInstruction { .. } => {}
            Operation::Constant { dst, value, .. } => {
                emit_constant(&mut encoder, allocation, *dst, *value)?;
            }
            Operation::ReadGpr { dst, register, .. } => {
                let byte_offset = u32::from(register.0)
                    .checked_mul(8)
                    .ok_or(EmitterError::OutputOffsetOutOfRange { slot: usize::MAX })?;
                match allocation.location(*dst)? {
                    ValueLocation::Register(home) => encoder.ldr_x(
                        XRegOrZero::Reg(home),
                        AddressBase::Reg(xreg(7)?),
                        byte_offset,
                    )?,
                    ValueLocation::Spill(_) => {
                        encoder.ldr_x(
                            XRegOrZero::Reg(xreg(13)?),
                            AddressBase::Reg(xreg(7)?),
                            byte_offset,
                        )?;
                        allocation.emit_store_scratch(&mut encoder, *dst, xreg(13)?)?;
                    }
                }
            }
            Operation::Integer {
                kind,
                lhs,
                rhs,
                result,
                ..
            } => {
                allocation.emit_load_scratch(&mut encoder, *lhs, xreg(13)?)?;
                allocation.emit_load_scratch(&mut encoder, *rhs, xreg(14)?)?;
                match kind {
                    IntegerKind::Add => {
                        encoder.add_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        allocation.emit_store_scratch(
                            &mut encoder,
                            (*result).ok_or(EmitterError::InvalidOperation {
                                operation: operation_index,
                            })?,
                            xreg(15)?,
                        )?;
                    }
                    IntegerKind::Subtract => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        allocation.emit_store_scratch(
                            &mut encoder,
                            (*result).ok_or(EmitterError::InvalidOperation {
                                operation: operation_index,
                            })?,
                            xreg(15)?,
                        )?;
                    }
                    IntegerKind::Compare => {
                        encoder.sub_x(xreg(15)?, xreg(13)?, XRegOrZero::Reg(xreg(14)?));
                        let slot = compare_slots[operation_index].ok_or(
                            EmitterError::InvalidOperation {
                                operation: operation_index,
                            },
                        )?;
                        encoder.str_x(
                            XRegOrZero::Reg(xreg(15)?),
                            AddressBase::Reg(xreg(8)?),
                            output_byte_offset(slot as usize)?,
                        )?;
                    }
                }
            }
            Operation::WriteGpr { register, src, .. } => {
                allocation.emit_load_scratch(&mut encoder, *src, xreg(13)?)?;
                let byte_offset = u32::from(register.0)
                    .checked_mul(8)
                    .ok_or(EmitterError::OutputOffsetOutOfRange { slot: usize::MAX })?;
                encoder.str_x(
                    XRegOrZero::Reg(xreg(13)?),
                    AddressBase::Reg(xreg(7)?),
                    byte_offset,
                )?;
            }
            Operation::Exit { .. } => {}
            Operation::Branch { .. } => {
                return Err(EmitterError::UnsupportedTerminal {
                    operation: operation_index,
                });
            }
        }
    }

    for output in &outputs {
        let temp = match output.kind {
            DataEffectOutputKind::Temporary(temp) => Some(temp),
            DataEffectOutputKind::GuestRegisterCandidate { source, .. } => Some(source),
            DataEffectOutputKind::CompareScratch { .. } => None,
        };
        if let Some(temp) = temp {
            allocation.emit_load_scratch(&mut encoder, temp, xreg(13)?)?;
            encoder.str_x(
                XRegOrZero::Reg(xreg(13)?),
                AddressBase::Reg(xreg(8)?),
                output_byte_offset(output.slot as usize)?,
            )?;
        }
    }

    let status = xreg(0)?;
    encoder.movz_x(status, DATA_EFFECTS_UNCOMMITTED_V1 as u16, 0)?;
    encoder.movk_x(status, (DATA_EFFECTS_UNCOMMITTED_V1 >> 16) as u16, 1)?;
    for _ in 0..pair_count {
        encoder.ldp_x(
            xreg(13)?,
            xreg(14)?,
            AddressBase::Sp,
            16,
            PairIndexMode::PostIndex,
        )?;
    }
    encoder.ret(xreg(30)?);

    Ok(EmittedDataEffects {
        code: encoder.finish()?,
        outputs,
        flags: if arithmetic_flags_incomplete {
            FlagMaterialization::ArithmeticIncomplete
        } else {
            FlagMaterialization::Preserved
        },
        source_instruction_count: program.guest_instructions.len(),
        committed_guest_instructions: 0,
        terminal_target_rip,
    })
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

    #[test]
    fn emits_decoder_derived_data_effects_with_each_supported_budget() {
        let mut bytes = vec![0x48, 0xB8];
        bytes.extend_from_slice(&u64::MAX.to_le_bytes()); // MOV RAX, -1
        bytes.extend_from_slice(&[0x48, 0x03, 0xC1]); // ADD RAX, RCX
        bytes.extend_from_slice(&[0x48, 0x2B, 0xD2]); // SUB RDX, RDX
        bytes.extend_from_slice(&[0x48, 0x3B, 0xC1]); // CMP RAX, RCX
        let program = lower_bytes(&bytes);

        for register_homes in 0..=4 {
            let allocation = Allocation::build(
                &program,
                crate::allocator::AllocatorConfig {
                    register_homes,
                    reserved_prefix_bytes: 0,
                },
                &[],
            )
            .unwrap();
            let emitted = emit_data_effects(&program, &allocation).unwrap();
            assert!(!emitted.code.words().is_empty());
            assert_eq!(emitted.code.to_le_bytes().unwrap().len() % 4, 0);
            assert_eq!(emitted.flags, FlagMaterialization::ArithmeticIncomplete);
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
    fn rejects_branch_terminal_and_non_arithmetic_flags() {
        let mut branch_bytes = vec![0x48, 0xB8];
        branch_bytes.extend_from_slice(&9u64.to_le_bytes());
        branch_bytes.extend_from_slice(&[0x74, 0x00]);
        let branch_program = lower_bytes(&branch_bytes);
        let branch_allocation = Allocation::build(
            &branch_program,
            crate::allocator::AllocatorConfig::default(),
            &[],
        )
        .unwrap();
        assert!(matches!(
            emit_data_effects(&branch_program, &branch_allocation),
            Err(EmitterError::UnsupportedTerminal { .. })
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
        let allocation =
            Allocation::build(&program, crate::allocator::AllocatorConfig::default(), &[]).unwrap();
        assert!(matches!(
            emit_data_effects(&program, &allocation),
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
        let allocation =
            Allocation::build(&program, crate::allocator::AllocatorConfig::default(), &[]).unwrap();
        let clone = program.clone();
        assert_eq!(
            emit_data_effects(&clone, &allocation).unwrap_err(),
            EmitterError::AllocationProgramMismatch
        );
    }
}
