//! On-demand single-instruction execution for the implemented long-mode integer slice.
//! Unlike the fragment API, targets are decoded only when execution reaches them.

use crate::long_decode::{
    self, AddressBase, ArithmeticKind, BitKind, BitOffset, BitScanKind, DecodeError, LogicKind,
    MemoryOperand, OpcodeMap, Operand, Operation, Prefixes, ShiftKind, canonical,
    general_protection,
};
use crate::memory::{AccessContext, ArchitecturalFault, GuestMemory, MemoryError, PrivilegeLevel};
use crate::state::X87Tag;
use crate::x87::{
    RationalRoundingMode, X87Binary80, X87Binary80Class, add_finite_same_exponent,
    divide_finite_significands, multiply_finite_significands, normalize_exact_magnitude,
    pack_normal_rounded_significand, round_pack_exact_coefficient,
};
use crate::{FsDescriptorCache, LongModeState, OperandSize, Register, RegisterView};

/// Maximum successful string iterations processed by one instruction invocation.
pub const REP_ITERATION_WORK_LIMIT: u64 = 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StepOutcome {
    Advanced,
    Branched,
    RepIterationBudgetExhausted,
    StopRequested,
    Fault(ArchitecturalFault),
    Unsupported { opcode: u8 },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunExit {
    StopAddress,
    InstructionBudgetExhausted,
    RepIterationBudgetExhausted,
    StopRequested,
    Fault(ArchitecturalFault),
    Unsupported { opcode: u8 },
    MemoryError(MemoryError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunConfig {
    pub instruction_budget: u64,
    pub stop_rip: Option<u64>,
    pub context: AccessContext,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunResult {
    pub exit: RunExit,
    pub instructions_executed: u64,
    /// Architectural definedness of the admitted integer-profile RFLAGS bits.
    /// Concrete undefined flag values in `state` are not comparison promises.
    pub rflags_defined: u64,
}

pub fn run(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    config: RunConfig,
    mut stop_requested: impl FnMut() -> bool,
) -> RunResult {
    let mut retired = 0;
    let mut rflags_defined = crate::INTEGER_FLAGS;
    loop {
        // The harness address is checked first; STOP then budget are checked
        // before fetch at every instruction boundary. A fault does not retire.
        let exit = if config.stop_rip == Some(state.rip) {
            Some(RunExit::StopAddress)
        } else if stop_requested() {
            Some(RunExit::StopRequested)
        } else if retired == config.instruction_budget {
            Some(RunExit::InstructionBudgetExhausted)
        } else {
            match step_tracked_with_stop(
                state,
                memory,
                config.context,
                &mut rflags_defined,
                &mut stop_requested,
            ) {
                Ok(StepOutcome::Advanced | StepOutcome::Branched) => {
                    retired += 1;
                    None
                }
                Ok(StepOutcome::Fault(fault)) => Some(RunExit::Fault(fault)),
                Ok(StepOutcome::RepIterationBudgetExhausted) => {
                    Some(RunExit::RepIterationBudgetExhausted)
                }
                Ok(StepOutcome::StopRequested) => Some(RunExit::StopRequested),
                Ok(StepOutcome::Unsupported { opcode }) => Some(RunExit::Unsupported { opcode }),
                Err(error) => Some(RunExit::MemoryError(error)),
            }
        };
        if let Some(exit) = exit {
            return RunResult {
                exit,
                instructions_executed: retired,
                rflags_defined,
            };
        }
    }
}

pub fn step(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
) -> Result<StepOutcome, MemoryError> {
    let mut rflags_defined = crate::INTEGER_FLAGS;
    step_tracked(state, memory, context, &mut rflags_defined)
}

fn step_tracked(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    rflags_defined: &mut u64,
) -> Result<StepOutcome, MemoryError> {
    step_tracked_with_stop(state, memory, context, rflags_defined, &mut || false)
}

fn step_tracked_with_stop(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    rflags_defined: &mut u64,
    stop_requested: &mut impl FnMut() -> bool,
) -> Result<StepOutcome, MemoryError> {
    let instruction = match long_decode::decode(memory, state.rip, context) {
        Ok(instruction) => instruction,
        Err(DecodeError::Unsupported { opcode, .. }) => {
            return Ok(StepOutcome::Unsupported {
                opcode: opcode.byte,
            });
        }
        Err(DecodeError::Invalid { .. }) => {
            return Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            }));
        }
        Err(DecodeError::Memory(MemoryError::Architectural(fault))) => {
            return Ok(StepOutcome::Fault(fault));
        }
        Err(DecodeError::Memory(error)) => return Err(error),
    };
    let next_rip = state.rip.wrapping_add(u64::from(instruction.length));
    if matches!(instruction.operation, Operation::VerwGdt)
        && (context.privilege != PrivilegeLevel::Ring3
            || state.gdtr.is_none()
            || state.gpr[Register::Rax as usize] as u16 & 4 != 0)
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if matches!(instruction.operation, Operation::LoadFsSelector)
        && (context.privilege != PrivilegeLevel::Ring3
            || state.gdtr.is_none()
            || state.fs_cache.is_none()
            || {
                let selector = state.gpr[Register::Rax as usize] as u16;
                selector & 0xfffc == 0 || selector & 4 != 0
            })
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if matches!(instruction.operation, Operation::LoadGsSelector)
        && (context.privilege != PrivilegeLevel::Ring3
            || state.gdtr.is_none()
            || state.gs_cache.is_none()
            || {
                let selector = state.gpr[Register::Rax as usize] as u16;
                selector & 0xfffc == 0 || selector & 4 != 0
            })
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if matches!(
        instruction.operation,
        Operation::Fninit
            | Operation::Fnclex
            | Operation::FnstswAx
            | Operation::Fchs
            | Operation::Fabs
            | Operation::Ftst
            | Operation::FcomSt { .. }
            | Operation::FcompSt { .. }
            | Operation::Fcompp
            | Operation::Fucompp
            | Operation::FmulSt0St { .. }
            | Operation::FdivSt0St { .. }
            | Operation::FdivrSt0St { .. }
            | Operation::FdivrStSt0 { .. }
            | Operation::FdivpStSt0 { .. }
            | Operation::FdivrpStSt0 { .. }
            | Operation::FmulStSt0 { .. }
            | Operation::FaddSt0St { .. }
            | Operation::FsubSt0St { .. }
            | Operation::FsubrSt0St { .. }
            | Operation::FsubStSt0 { .. }
            | Operation::FsubrStSt0 { .. }
            | Operation::FaddStSt0 { .. }
            | Operation::FaddpStSt0 { .. }
            | Operation::FmulpStSt0 { .. }
            | Operation::FsubpStSt0 { .. }
            | Operation::FsubrpStSt0 { .. }
            | Operation::FstSti { .. }
            | Operation::FstpSti { .. }
    ) && state.x87.is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FaddSt0St { source } = instruction.operation
        && exact_fadd_result(state, 0, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FmulSt0St { source } = instruction.operation
        && exact_fmul_result(state, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivSt0St { source } = instruction.operation
        && exact_fdiv_result(state, 0, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivrSt0St { source } = instruction.operation
        && exact_fdiv_result(state, source, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivrStSt0 { destination } = instruction.operation
        && exact_fdiv_result(state, 0, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivStSt0 { destination } = instruction.operation
        && exact_fdiv_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivpStSt0 { destination } = instruction.operation
        && exact_fdiv_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FdivrpStSt0 { destination } = instruction.operation
        && exact_fdiv_result(state, 0, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FmulStSt0 { destination } = instruction.operation
        && exact_fmul_result(state, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubSt0St { source } = instruction.operation
        && exact_fsub_result(state, 0, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubrSt0St { source } = instruction.operation
        && exact_fsub_result(state, source, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubStSt0 { destination } = instruction.operation
        && exact_fsub_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubrStSt0 { destination } = instruction.operation
        && exact_fsub_result(state, 0, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FaddStSt0 { destination } = instruction.operation
        && exact_fadd_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FaddpStSt0 { destination } = instruction.operation
        && exact_fadd_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FmulpStSt0 { destination } = instruction.operation
        && exact_fmul_result(state, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubpStSt0 { destination } = instruction.operation
        && exact_fsub_result(state, destination, 0).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FsubrpStSt0 { destination } = instruction.operation
        && exact_fsub_result(state, 0, destination).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if matches!(
        instruction.operation,
        Operation::FstSti { .. }
            | Operation::FstpSti { .. }
            | Operation::Fchs
            | Operation::Fabs
            | Operation::Ftst
    ) && bounded_x87_normal_st0_source(state).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FcomSt { source } = instruction.operation
        && bounded_x87_fcom_result(state, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if let Operation::FcompSt { source } = instruction.operation
        && bounded_x87_fcom_result(state, source).is_none()
    {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if instruction.operation == Operation::Fcompp && bounded_x87_fcom_result(state, 1).is_none() {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if instruction.operation == Operation::Fucompp && bounded_x87_fcom_result(state, 1).is_none() {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if instruction.operation == Operation::Fucom && bounded_x87_fcom_result(state, 1).is_none() {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    if instruction.operation == Operation::Fucomp && bounded_x87_fcom_result(state, 1).is_none() {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    // Direct control flow uses the decoded condition/displacement; integer
    // operations below use normalized operands at each accepted width.
    let executable = match instruction.operation {
        Operation::MovImmediate { .. }
        | Operation::Mov { .. }
        | Operation::MovFsLoad { .. }
        | Operation::MovGsLoad { .. }
        | Operation::MovFsSelectorToRax
        | Operation::MovGsSelectorToRax
        | Operation::VerwGdt
        | Operation::Fninit
        | Operation::Fnclex
        | Operation::FnstswAx
        | Operation::Fchs
        | Operation::Fabs
        | Operation::Ftst
        | Operation::FcomSt { .. }
        | Operation::FcompSt { .. }
        | Operation::Fcompp
        | Operation::Fucompp
        | Operation::Fucom
        | Operation::Fucomp
        | Operation::FdivSt0St { .. }
        | Operation::FdivrSt0St { .. }
        | Operation::FdivrStSt0 { .. }
        | Operation::FdivStSt0 { .. }
        | Operation::FdivpStSt0 { .. }
        | Operation::FdivrpStSt0 { .. }
        | Operation::Extend { .. }
        | Operation::Lea { .. }
        | Operation::Xchg { .. }
        | Operation::Nop
        | Operation::StackRegister { .. }
        | Operation::PopFlags
        | Operation::PushFlags
        | Operation::PushImmediate { .. }
        | Operation::SignExtendAccumulator { .. }
        | Operation::Bswap { .. }
        | Operation::Bit { .. }
        | Operation::BitScan { .. }
        | Operation::Arithmetic { .. }
        | Operation::Logic { .. }
        | Operation::Mul { .. }
        | Operation::Imul { .. }
        | Operation::ImulImmediate { .. }
        | Operation::Divide { .. }
        | Operation::Shift { .. } => true,
        Operation::Mfence => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && instruction.opcode.byte == 0xae
                && instruction.length == 3
                && instruction.prefixes == Prefixes::default()
        }
        Operation::FaddSt0St { source } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xd8
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && source <= 7
        }
        Operation::FmulSt0St { source } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xd8
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && source <= 7
        }
        Operation::FmulStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdc
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FsubSt0St { source } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xd8
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && source <= 7
        }
        Operation::FsubrSt0St { source } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xd8
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && source <= 7
        }
        Operation::FsubStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdc
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FsubrStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdc
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FaddStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdc
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FaddpStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xde
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FmulpStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xde
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FsubpStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xde
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FsubrpStSt0 { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xde
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FstpSti { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdd
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::FstSti { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xdd
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
                && destination <= 7
        }
        Operation::LoadFsSelector => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0x8e
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
        }
        Operation::LoadGsSelector => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0x8e
                && instruction.length == 2
                && instruction.prefixes == Prefixes::default()
        }
        Operation::XchgMemory { memory, .. } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0x87
                && instruction.operand_size == OperandSize::Qword
                && memory.size == OperandSize::Qword
                && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                && instruction.prefixes.group1.is_none()
                && !instruction.prefixes.operand_override
                && !instruction.prefixes.address_override
                && instruction.prefixes.segment.is_none()
        }
        Operation::XaddMemory {
            destination,
            source,
        } => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && instruction.opcode.byte == 0xc1
                && instruction.operand_size == OperandSize::Qword
                && destination.size == OperandSize::Qword
                && source.size() == OperandSize::Qword
                && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                && instruction
                    .prefixes
                    .group1
                    .is_none_or(|prefix| prefix == 0xf0)
                && !instruction.prefixes.operand_override
                && !instruction.prefixes.address_override
                && instruction.prefixes.segment.is_none()
        }
        Operation::CmpxchgMemory {
            destination,
            source,
        } => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && instruction.opcode.byte == 0xb1
                && instruction.operand_size == OperandSize::Qword
                && destination.size == OperandSize::Qword
                && source.size() == OperandSize::Qword
                && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                && instruction.prefixes.group1.is_none()
                && !instruction.prefixes.operand_override
                && !instruction.prefixes.address_override
                && instruction.prefixes.segment.is_none()
        }
        Operation::Xadd {
            destination,
            source,
        } => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && instruction.opcode.byte == 0xc1
                && instruction.operand_size == OperandSize::Qword
                && destination.size() == OperandSize::Qword
                && source.size() == OperandSize::Qword
                && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                && instruction.prefixes.group1.is_none()
                && !instruction.prefixes.operand_override
                && !instruction.prefixes.address_override
                && instruction.prefixes.segment.is_none()
        }
        Operation::Cmpxchg {
            destination,
            source,
        } => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && instruction.opcode.byte == 0xb1
                && instruction.operand_size == OperandSize::Qword
                && destination.size() == OperandSize::Qword
                && source.size() == OperandSize::Qword
                && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                && instruction.prefixes.group1.is_none()
                && !instruction.prefixes.operand_override
                && !instruction.prefixes.address_override
                && instruction.prefixes.segment.is_none()
        }
        Operation::SetCondition { .. } => true,
        Operation::ConditionalMove { .. } => {
            instruction.opcode.map == OpcodeMap::Escape0f
                && (0x40..=0x4f).contains(&instruction.opcode.byte)
                && instruction.prefixes.group1.is_none()
        }
        Operation::Branch { .. } => {
            matches!(
                instruction.opcode.map,
                OpcodeMap::Primary | OpcodeMap::Escape0f
            ) && instruction.prefixes == Prefixes::default()
        }
        Operation::Jump { .. } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.prefixes == Prefixes::default()
        }
        Operation::Call { .. } | Operation::Return { .. } => {
            instruction.opcode.map == OpcodeMap::Primary
                && (instruction.prefixes == Prefixes::default()
                    || (instruction.opcode.byte == 0xc3
                        && instruction.prefixes
                            == (Prefixes {
                                operand_override: true,
                                ..Prefixes::default()
                            })))
                && matches!(instruction.opcode.byte, 0xe8 | 0xc2 | 0xc3)
        }
        Operation::IndirectCall { .. } => {
            let valid_operand_prefixes = match instruction.operation {
                Operation::IndirectCall {
                    operand: Operand::Register(_),
                } => {
                    instruction.prefixes.segment.is_none()
                        && !instruction.prefixes.address_override
                        && instruction.prefixes.rex.is_none_or(|rex| rex == 0x41)
                }
                Operation::IndirectCall {
                    operand: Operand::Memory(_),
                } => !matches!(instruction.prefixes.segment, Some(0x64 | 0x65)),
                _ => false,
            };
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xff
                && instruction.prefixes.group1.is_none()
                && !instruction.prefixes.operand_override
                && valid_operand_prefixes
        }
        Operation::IndirectJump { .. } => {
            instruction.opcode
                == crate::long_decode::Opcode {
                    map: OpcodeMap::Primary,
                    byte: 0xff,
                }
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && !instruction.prefixes.operand_override
        }
        Operation::PushMemory { source } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xff
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && instruction.operand_size == source.size
                && matches!(source.size, OperandSize::Word | OperandSize::Qword)
        }
        Operation::Movs { size } => {
            instruction.opcode.map == OpcodeMap::Primary
                && ((instruction.opcode.byte == 0xa4
                    && instruction.prefixes == Prefixes::default()
                    && size == OperandSize::Byte)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes
                            == (Prefixes {
                                operand_override: true,
                                ..Prefixes::default()
                            })
                        && size == OperandSize::Word)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes == Prefixes::default()
                        && size == OperandSize::Dword)
                    || (instruction.opcode.byte == 0xa5
                        && instruction.prefixes.group1.is_none()
                        && instruction.prefixes.segment.is_none()
                        && !instruction.prefixes.address_override
                        && instruction.prefixes.rex.is_some_and(|rex| rex & 8 != 0)
                        && size == OperandSize::Qword))
        }
        Operation::Stos { size } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xaa
                && instruction.prefixes == Prefixes::default()
                && size == OperandSize::Byte
        }
        Operation::RepStos { size } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0xaa
                && instruction.length == 2
                && instruction.prefixes
                    == (Prefixes {
                        group1: Some(0xf3),
                        ..Prefixes::default()
                    })
                && size == OperandSize::Byte
        }
        Operation::PopMemory { destination } => {
            instruction.opcode.map == OpcodeMap::Primary
                && instruction.opcode.byte == 0x8f
                && instruction.prefixes.group1.is_none()
                && !matches!(instruction.prefixes.segment, Some(0x64 | 0x65))
                && instruction.operand_size == destination.size
                && matches!(destination.size, OperandSize::Word | OperandSize::Qword)
        }
    };
    if !executable {
        return Ok(StepOutcome::Unsupported {
            opcode: instruction.opcode.byte,
        });
    }
    match instruction.operation {
        Operation::Fninit => state
            .x87
            .as_mut()
            .expect("FNINIT is admitted only when x87 state exists")
            .fninit(),
        Operation::Fnclex => state
            .x87
            .as_mut()
            .expect("FNCLEX is admitted only when x87 state exists")
            .fnclex(),
        Operation::FnstswAx => {
            let status_word = state
                .x87
                .as_ref()
                .expect("FNSTSW AX is admitted only when x87 state exists")
                .status_word;
            state.write(
                RegisterView::low(Register::Rax, OperandSize::Word),
                u64::from(status_word),
            );
        }
        Operation::Fchs => {
            let mut source =
                bounded_x87_normal_st0_source(state).expect("FCHS source was prevalidated");
            source[9] ^= 0x80;
            let x87 = state.x87.as_mut().expect("FCHS state was prevalidated");
            let top = usize::from(x87.top());
            x87.registers[top] = source;
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::Fabs => {
            let mut source =
                bounded_x87_normal_st0_source(state).expect("FABS source was prevalidated");
            source[9] &= 0x7f;
            let x87 = state.x87.as_mut().expect("FABS state was prevalidated");
            let top = usize::from(x87.top());
            x87.registers[top] = source;
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::Ftst => {
            let source =
                bounded_x87_normal_st0_source(state).expect("FTST source was prevalidated");
            let negative = source[9] & 0x80 != 0;
            let x87 = state.x87.as_mut().expect("FTST state was prevalidated");
            x87.status_word = (x87.status_word & !0x4700) | if negative { 0x0100 } else { 0 };
            x87.instruction_pointer = state.rip;
        }
        Operation::FcomSt { source } => {
            let condition =
                bounded_x87_fcom_result(state, source).expect("FCOM operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FCOM state was prevalidated");
            x87.status_word = (x87.status_word & !0x4700) | condition;
            x87.instruction_pointer = state.rip;
        }
        Operation::FcompSt { source } => {
            let condition =
                bounded_x87_fcom_result(state, source).expect("FCOMP operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FCOMP state was prevalidated");
            let top = x87.top();
            let old_status = x87.status_word;
            x87.status_word = (old_status & !0x4700) | condition;
            let _ = x87.set_tag(top, X87Tag::Empty);
            x87.status_word = (x87.status_word & !0x3800) | (u16::from((top + 1) & 7) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::Fcompp => {
            let condition =
                bounded_x87_fcom_result(state, 1).expect("FCOMPP operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FCOMPP state was prevalidated");
            let top = x87.top();
            let second = (top + 1) & 7;
            let new_top = (top + 2) & 7;
            x87.status_word = (x87.status_word & !0x4700) | condition;
            let _ = x87.set_tag(top, X87Tag::Empty);
            let _ = x87.set_tag(second, X87Tag::Empty);
            x87.status_word = (x87.status_word & !0x3800) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::Fucompp => {
            let condition =
                bounded_x87_fcom_result(state, 1).expect("FUCOMPP operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FUCOMPP state was prevalidated");
            let top = x87.top();
            let second = (top + 1) & 7;
            let new_top = (top + 2) & 7;
            // C1=0 is the bounded profile's local deterministic policy for normal operands.
            x87.status_word = (x87.status_word & !0x4700) | condition;
            let _ = x87.set_tag(top, X87Tag::Empty);
            let _ = x87.set_tag(second, X87Tag::Empty);
            x87.status_word = (x87.status_word & !0x3800) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::Fucom => {
            let condition =
                bounded_x87_fcom_result(state, 1).expect("FUCOM operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FUCOM state was prevalidated");
            // C1=0 is the bounded profile's deterministic policy. The SDM only
            // specifies C1=0 for stack underflow, not for this normal case.
            x87.status_word = (x87.status_word & !0x4700) | condition;
            x87.instruction_pointer = state.rip;
        }
        Operation::Fucomp => {
            let condition =
                bounded_x87_fcom_result(state, 1).expect("FUCOMP operands were prevalidated");
            let x87 = state.x87.as_mut().expect("FUCOMP state was prevalidated");
            let old_top = x87.top();
            let new_top = (old_top + 1) & 7;
            // Match the bounded compare policy, then pop only the old physical TOP.
            x87.status_word = (x87.status_word & !0x4700) | condition;
            let _ = x87.set_tag(old_top, X87Tag::Empty);
            x87.status_word = (x87.status_word & !0x3800) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FaddSt0St { source } => {
            // The complete bounded precondition and result check ran above, before mutation.
            let result = exact_fadd_result(state, 0, source).expect("FADD result was prevalidated");
            let x87 = state.x87.as_mut().expect("FADD state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FmulpStSt0 { destination } => {
            // The product is fully validated before mutation. In the i=0
            // alias case this captures the square before the old TOP tag is emptied.
            let result =
                exact_fmul_result(state, destination).expect("FMULP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FMULP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FmulStSt0 { destination } => {
            let result =
                exact_fmul_result(state, destination).expect("FMUL result was prevalidated");
            let x87 = state.x87.as_mut().expect("FMUL state was prevalidated");
            let physical_destination = (x87.top() + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FmulSt0St { source } => {
            let result = exact_fmul_result(state, source).expect("FMUL result was prevalidated");
            let x87 = state.x87.as_mut().expect("FMUL state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivSt0St { source } => {
            let result = exact_fdiv_result(state, 0, source).expect("FDIV result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIV state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivrSt0St { source } => {
            // FDIVR ST(0),ST(i) computes old ST(i) / old ST(0), without popping.
            let result =
                exact_fdiv_result(state, source, 0).expect("FDIVR result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIVR state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivrStSt0 { destination } => {
            // FDIVR ST(i),ST(0) computes old ST(0) / old ST(i), without popping.
            let result =
                exact_fdiv_result(state, 0, destination).expect("FDIVR result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIVR state was prevalidated");
            let physical_destination = ((x87.top() + destination) & 7) as usize;
            x87.registers[physical_destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivStSt0 { destination } => {
            // FDIV ST(i),ST(0) computes old ST(i) / old ST(0), without popping.
            let result =
                exact_fdiv_result(state, destination, 0).expect("FDIV result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIV state was prevalidated");
            let physical_destination = ((x87.top() + destination) & 7) as usize;
            x87.registers[physical_destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivpStSt0 { destination } => {
            // Compute from the old operands, then pop old ST(0).
            let result =
                exact_fdiv_result(state, destination, 0).expect("FDIVP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIVP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FdivrpStSt0 { destination } => {
            // FDIVRP computes old ST(0) / old ST(i), then pops old ST(0).
            // The i=0 alias writes exact +1 before emptying the old TOP tag.
            let result =
                exact_fdiv_result(state, 0, destination).expect("FDIVRP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FDIVRP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubrStSt0 { destination } => {
            // FSUBR ST(i),ST(0) computes ST(0)-ST(i), storing to old ST(i).
            let result =
                exact_fsub_result(state, 0, destination).expect("FSUBR result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUBR state was prevalidated");
            let physical_destination = ((x87.top() + destination) & 7) as usize;
            x87.registers[physical_destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubpStSt0 { destination } => {
            let result =
                exact_fsub_result(state, destination, 0).expect("FSUBP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUBP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubrpStSt0 { destination } => {
            let result =
                exact_fsub_result(state, 0, destination).expect("FSUBRP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUBRP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubStSt0 { destination } => {
            let result =
                exact_fsub_result(state, destination, 0).expect("FSUB result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUB state was prevalidated");
            let physical_destination =
                x87.physical_register_for_st(destination)
                    .expect("ST(i) is a valid logical register") as usize;
            x87.registers[physical_destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubSt0St { source } => {
            // The complete bounded precondition and result check ran above, before mutation.
            let result = exact_fsub_result(state, 0, source).expect("FSUB result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUB state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FsubrSt0St { source } => {
            // The helper's logical destination/source order is ST(i) - ST(0);
            // the architectural destination remains the old physical TOP.
            let result =
                exact_fsub_result(state, source, 0).expect("FSUBR result was prevalidated");
            let x87 = state.x87.as_mut().expect("FSUBR state was prevalidated");
            let destination = x87
                .physical_register_for_st(0)
                .expect("ST(0) is a valid logical register") as usize;
            x87.registers[destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FaddStSt0 { destination } => {
            let result =
                exact_fadd_result(state, destination, 0).expect("FADD result was prevalidated");
            let x87 = state.x87.as_mut().expect("FADD state was prevalidated");
            let physical_destination =
                x87.physical_register_for_st(destination)
                    .expect("ST(i) is a valid logical register") as usize;
            x87.registers[physical_destination] = result.to_bytes();
            x87.status_word &= !(1 << 9);
            x87.instruction_pointer = state.rip;
        }
        Operation::FaddpStSt0 { destination } => {
            let result =
                exact_fadd_result(state, destination, 0).expect("FADDP result was prevalidated");
            let x87 = state.x87.as_mut().expect("FADDP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = result.to_bytes();
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word =
                (x87.status_word & !((7 << 11) | (1 << 9))) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        Operation::FstSti { destination } => {
            let source = bounded_x87_normal_st0_source(state).expect("FST source was prevalidated");
            let x87 = state.x87.as_mut().expect("FST state was prevalidated");
            let physical_destination = (x87.top() + destination) & 7;
            x87.registers[usize::from(physical_destination)] = source;
            x87.set_tag(physical_destination, X87Tag::Valid);
            x87.instruction_pointer = state.rip;
        }
        Operation::FstpSti { destination } => {
            // Capture the old ST(0) bytes before any mutation. The target's old
            // tag and payload are deliberately irrelevant, including the i=0 alias.
            let source =
                bounded_x87_normal_st0_source(state).expect("FSTP source was prevalidated");
            let x87 = state.x87.as_mut().expect("FSTP state was prevalidated");
            let old_top = x87.top();
            let physical_destination = (old_top + destination) & 7;
            x87.registers[usize::from(physical_destination)] = source;
            x87.set_tag(physical_destination, X87Tag::Valid);
            x87.set_tag(old_top, X87Tag::Empty);
            let new_top = old_top.wrapping_add(1) & 7;
            x87.status_word = (x87.status_word & !(7 << 11)) | (u16::from(new_top) << 11);
            x87.instruction_pointer = state.rip;
        }
        operation @ (Operation::MovImmediate { .. }
        | Operation::Mov { .. }
        | Operation::MovFsLoad { .. }
        | Operation::MovGsLoad { .. }
        | Operation::MovFsSelectorToRax
        | Operation::MovGsSelectorToRax
        | Operation::Extend { .. }
        | Operation::Lea { .. }
        | Operation::Xchg { .. }
        | Operation::XchgMemory { .. }
        | Operation::Mfence
        | Operation::Nop) => match transfer(state, memory, context, next_rip, operation) {
            Ok(()) => {}
            Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
            Err(error) => return Err(error),
        },
        Operation::SetCondition { condition, dst } => {
            let value = u64::from(condition.evaluate(state.rflags));
            match write_operand(state, memory, context, next_rip, dst, value) {
                Ok(()) => {}
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            }
        }
        Operation::VerwGdt => {
            let selector = state.gpr[Register::Rax as usize] as u16;
            match verify_writable_gdt_selector(state, memory, context, selector) {
                Ok(writable) => {
                    const ZERO_FLAG: u64 = 1 << 6;
                    state.rflags =
                        (state.rflags & !ZERO_FLAG) | if writable { ZERO_FLAG } else { 0 };
                }
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            }
        }
        Operation::LoadFsSelector => {
            let selector = state.gpr[Register::Rax as usize] as u16;
            match load_fs_gdt_selector(state, memory, context, selector) {
                Ok(true) => {}
                Ok(false) => {
                    return Ok(StepOutcome::Unsupported {
                        opcode: instruction.opcode.byte,
                    });
                }
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            }
        }
        Operation::LoadGsSelector => {
            let selector = state.gpr[Register::Rax as usize] as u16;
            match load_gdt_selector(state, memory, context, selector, true) {
                Ok(true) => {}
                Ok(false) => {
                    return Ok(StepOutcome::Unsupported {
                        opcode: instruction.opcode.byte,
                    });
                }
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            }
        }
        Operation::StackRegister { push, register } => {
            let old_rsp = state.gpr[Register::Rsp as usize];
            let stack_width = instruction.operand_size;
            if push {
                // Read the source before changing RSP, which is required for
                // PUSH SP/RSP. A failed bounded-memory write commits neither RSP
                // nor RIP.
                let value = state.read(RegisterView::low(register, stack_width));
                if let Err(error) = push_stack_value(state, memory, context, value, stack_width) {
                    return report_memory_error(error);
                }
            } else {
                let stack_address = match stack_access_address(old_rsp, false, stack_width) {
                    Ok(address) => address,
                    Err(error) => return report_memory_error(error),
                };
                let value = match memory.read_integer(stack_address, context, stack_width) {
                    Ok(value) => value,
                    Err(error) => return report_memory_error(error),
                };
                // POP SP first advances the 64-bit stack pointer by two, then
                // writes the popped word into its low 16-bit alias. The qword
                // form likewise writes RSP after the post-increment step.
                state.gpr[Register::Rsp as usize] =
                    old_rsp.wrapping_add(stack_width.bytes() as u64);
                state.write(RegisterView::low(register, stack_width), value);
            }
        }
        Operation::PushImmediate { value } => {
            if let Err(error) =
                push_stack_value(state, memory, context, value, instruction.operand_size)
            {
                return report_memory_error(error);
            }
        }
        Operation::PushFlags => {
            // Intel SDM 093 PUSHFW stores the low FLAGS word unchanged, while
            // PUSHFQ clears VM/RF in the qword image. Neither form modifies
            // RFLAGS; the selected stack width also controls RSP adjustment.
            let value = match instruction.operand_size {
                OperandSize::Word => state.rflags & 0xffff,
                OperandSize::Qword => state.rflags & 0x00fc_ffff,
                OperandSize::Byte | OperandSize::Dword => {
                    unreachable!("long-mode PUSHF supports only word and qword widths")
                }
            };
            if let Err(error) =
                push_stack_value(state, memory, context, value, instruction.operand_size)
            {
                return report_memory_error(error);
            }
        }
        Operation::Movs { size } => {
            // In long mode the implicit DS/ES bases are flat. Keep the access
            // order explicit: source read, destination write, then commit both
            // index registers. The current bounded executor policy preserves
            // CPU state when either access faults; destination-fault restart
            // state remains an open hardware question (ISSUE-016).
            let source = Operand::Memory(MemoryOperand {
                base: AddressBase::Register(Register::Rsi),
                index: None,
                scale: 1,
                displacement: 0,
                address_size: OperandSize::Qword,
                size,
            });
            let destination = Operand::Memory(MemoryOperand {
                base: AddressBase::Register(Register::Rdi),
                index: None,
                scale: 1,
                displacement: 0,
                address_size: OperandSize::Qword,
                size,
            });
            let value = match read_operand(state, memory, context, next_rip, source) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            };
            if let Err(error) = write_operand(state, memory, context, next_rip, destination, value)
            {
                return report_memory_error(error);
            }
            let step = if state.rflags & (1 << 10) == 0 {
                size.bytes() as u64
            } else {
                0u64.wrapping_sub(size.bytes() as u64)
            };
            state.gpr[Register::Rsi as usize] =
                state.gpr[Register::Rsi as usize].wrapping_add(step);
            state.gpr[Register::Rdi as usize] =
                state.gpr[Register::Rdi as usize].wrapping_add(step);
        }
        Operation::RepStos { size } => {
            debug_assert_eq!(size, OperandSize::Byte);
            let mut completed = 0;
            while state.gpr[Register::Rcx as usize] != 0 {
                let destination = Operand::Memory(MemoryOperand {
                    base: AddressBase::Register(Register::Rdi),
                    index: None,
                    scale: 1,
                    displacement: 0,
                    address_size: OperandSize::Qword,
                    size: OperandSize::Byte,
                });
                let accumulator = state.gpr[Register::Rax as usize] & 0xff;
                if let Err(error) =
                    write_operand(state, memory, context, next_rip, destination, accumulator)
                {
                    return report_memory_error(error);
                }
                let step = if state.rflags & (1 << 10) == 0 {
                    1
                } else {
                    u64::MAX
                };
                state.gpr[Register::Rdi as usize] =
                    state.gpr[Register::Rdi as usize].wrapping_add(step);
                state.gpr[Register::Rcx as usize] -= 1;
                completed += 1;
                if state.gpr[Register::Rcx as usize] == 0 {
                    break;
                }
                if stop_requested() {
                    return Ok(StepOutcome::StopRequested);
                }
                if completed == REP_ITERATION_WORK_LIMIT {
                    return Ok(StepOutcome::RepIterationBudgetExhausted);
                }
            }
        }
        Operation::Stos { size } => {
            // Long-mode ES is flat. STOS reads AL, writes one byte to [RDI],
            // and commits only RDI after the store succeeds. RSI and flags are
            // not part of this instruction's state transition.
            let destination = Operand::Memory(MemoryOperand {
                base: AddressBase::Register(Register::Rdi),
                index: None,
                scale: 1,
                displacement: 0,
                address_size: OperandSize::Qword,
                size,
            });
            let accumulator = state.gpr[Register::Rax as usize] & 0xff;
            if let Err(error) =
                write_operand(state, memory, context, next_rip, destination, accumulator)
            {
                return report_memory_error(error);
            }
            let step = if state.rflags & (1 << 10) == 0 {
                size.bytes() as u64
            } else {
                0u64.wrapping_sub(size.bytes() as u64)
            };
            state.gpr[Register::Rdi as usize] =
                state.gpr[Register::Rdi as usize].wrapping_add(step);
        }
        Operation::PushMemory { source } => {
            // Resolve and read the complete source with the pre-instruction
            // state (notably [RSP]), then attempt the bounded write of the
            // selected word or qword stack item. Neither side commits if
            // either access faults.
            let value =
                match read_operand(state, memory, context, next_rip, Operand::Memory(source)) {
                    Ok(value) => value,
                    Err(MemoryError::Architectural(fault)) => {
                        return Ok(StepOutcome::Fault(fault));
                    }
                    Err(error) => return Err(error),
                };
            if let Err(error) = push_stack_value(state, memory, context, value, source.size) {
                return report_memory_error(error);
            }
        }
        Operation::PopMemory { destination } => {
            // POP reads the selected word/qword from the old stack top, stages
            // the corresponding RSP increment, then forms its memory
            // destination using that post-increment state (Intel SDM Vol.2B POP).
            let old_rsp = state.gpr[Register::Rsp as usize];
            let stack_address = match stack_access_address(old_rsp, false, destination.size) {
                Ok(address) => address,
                Err(error) => return report_memory_error(error),
            };
            let value = match memory.read_integer(stack_address, context, destination.size) {
                Ok(value) => value,
                Err(error) => return report_memory_error(error),
            };
            let mut staged = state.clone();
            staged.gpr[Register::Rsp as usize] =
                old_rsp.wrapping_add(destination.size.bytes() as u64);
            if let Err(error) = write_operand(
                &mut staged,
                memory,
                context,
                next_rip,
                Operand::Memory(destination),
                value,
            ) {
                return report_memory_error(error);
            }
            *state = staged;
        }
        Operation::PopFlags => {
            // This diagnostic slice follows the native user-mode fixture
            // profile: CPL3 with IOPL=0. It models only the six status flags
            // and DF. TF and NT are unsupported because their modeled effects
            // are outside the profile; NT=1 is also excluded while the SDM
            // sources disagree about its 16-bit CPL3/IOPL0 behavior.
            if context.privilege != crate::memory::PrivilegeLevel::Ring3 {
                return Ok(StepOutcome::Unsupported { opcode: 0x9d });
            }
            if state.rflags & (3 << 12) != 0 {
                return Ok(StepOutcome::Unsupported { opcode: 0x9d });
            }
            let width = instruction.operand_size;
            let old_rsp = state.gpr[Register::Rsp as usize];
            let stack_address = match stack_access_address(old_rsp, false, width) {
                Ok(address) => address,
                Err(error) => return report_memory_error(error),
            };
            let value = match memory.read_integer(stack_address, context, width) {
                Ok(value) => value,
                Err(error) => return report_memory_error(error),
            };
            let unsupported_flags = (1 << 8)
                | (1 << 14)
                | if width == OperandSize::Qword {
                    1 << 18
                } else {
                    0
                };
            if (value | state.rflags) & unsupported_flags != 0 {
                return Ok(StepOutcome::Unsupported { opcode: 0x9d });
            }
            state.rflags =
                (state.rflags & !crate::POPFQ_INTEGER_FLAGS) | (value & crate::POPFQ_INTEGER_FLAGS);
            state.gpr[Register::Rsp as usize] = old_rsp.wrapping_add(width.bytes() as u64);
        }
        Operation::ConditionalMove {
            condition,
            dst,
            src,
        } => {
            // CMOVcc performs its source read even when the condition is false.
            // Resolve/fault the operand before changing any architectural state.
            let value = match read_operand(state, memory, context, next_rip, src) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            if condition.evaluate(state.rflags) {
                state.write(dst, value);
            } else if dst.size() == OperandSize::Dword {
                // In 64-bit mode a 32-bit CMOV destination is zero-extended
                // even on the false path (Intel SDM CMOVcc operation).
                let old_low_dword = state.read(dst);
                state.write(dst, old_low_dword);
            }
        }
        Operation::SignExtendAccumulator { double_width } => {
            sign_extend_accumulator(state, instruction.operand_size, double_width);
        }
        Operation::Bswap { dst } => {
            let value = state.read(dst);
            let reversed = match dst.size() {
                OperandSize::Dword => u64::from((value as u32).swap_bytes()),
                OperandSize::Qword => value.swap_bytes(),
                _ => unreachable!("decoder admits only 32- and 64-bit BSWAP"),
            };
            state.write(dst, reversed);
        }
        Operation::Bit { kind, dst, offset } => {
            let size = match dst {
                Operand::Register(view) => view.size(),
                Operand::Memory(memory) => memory.size,
                Operand::Immediate(_) => unreachable!("bit base is register or memory"),
            };
            let width = operand_width(size);
            // Capture the register offset and derive the memory element before
            // any data access or destination update. For memory bit strings the
            // register value is signed; Euclidean division selects an adjacent
            // element for offsets outside this element (Intel SDM 093 Vol.2A
            // §3.1.1.9, BT-family memory forms).
            let (destination, bit_offset) = match (dst, offset) {
                (Operand::Memory(mut operand), BitOffset::Register(view)) => {
                    let signed_offset = signed_value(view.size(), state.read(view));
                    let width = i128::from(width);
                    let element_delta =
                        signed_offset.div_euclid(width) * i128::from(size.bytes() as u32);
                    operand.displacement = operand.displacement.wrapping_add(element_delta as i64);
                    (
                        Operand::Memory(operand),
                        signed_offset.rem_euclid(width) as u64,
                    )
                }
                (destination, BitOffset::Register(view)) => {
                    (destination, state.read(view) & (u64::from(width) - 1))
                }
                (destination, BitOffset::Immediate(value)) => {
                    (destination, u64::from(value) & (u64::from(width) - 1))
                }
            };
            let bit_mask = 1u64 << bit_offset;
            let bit = (|| {
                let base = read_operand(state, memory, context, next_rip, destination)?;
                let old_bit = u64::from(base & bit_mask != 0);
                let result = match kind {
                    BitKind::Test => base,
                    BitKind::Set => base | bit_mask,
                    BitKind::Reset => base & !bit_mask,
                    BitKind::Complement => base ^ bit_mask,
                };
                if kind != BitKind::Test {
                    write_operand(state, memory, context, next_rip, destination, result)?;
                }
                Ok::<_, MemoryError>(old_bit)
            })();
            let old_bit = match bit {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            };
            state.rflags = (state.rflags & !crate::CF) | old_bit;
        }
        Operation::BitScan { kind, dst, src } => {
            // Resolve the full effective address and read the complete source
            // before changing CPU state. This also preserves an aliased register
            // source and leaves the instruction uncommitted on a memory fault.
            let source = match read_operand(state, memory, context, next_rip, src) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            let source_size = match src {
                Operand::Register(view) => view.size(),
                Operand::Memory(memory) => memory.size,
                Operand::Immediate(_) => unreachable!("bit-scan source is register or memory"),
            };
            let width = operand_width(source_size);
            let zero = source == 0;
            if !zero {
                let index = match kind {
                    BitScanKind::Forward => source.trailing_zeros(),
                    BitScanKind::Reverse => width - 1 - (source.leading_zeros() - (64 - width)),
                };
                state.write(dst, u64::from(index));
            }
            // Intel SDM 093 Vol.2A pp. 3-107, 3-109: ZF indicates zero,
            // PF indicates even parity of the source, and CF/OF/SF/AF clear.
            let flags = if zero { crate::ZF } else { 0 }
                | if source.count_ones() % 2 == 0 {
                    crate::PF
                } else {
                    0
                };
            state.rflags = (state.rflags & !crate::ARITHMETIC_FLAGS) | flags;
        }
        Operation::Xadd {
            destination,
            source,
        } => {
            let old_destination = state.read(destination);
            let old_source = state.read(source);
            let (sum, flags) = arithmetic_result(
                OperandSize::Qword,
                ArithmeticKind::Add,
                old_destination,
                old_source,
                state.rflags,
            );
            // The SDM operation stores the old destination to SRC and the sum
            // to DEST. This ordering also gives the sum when both views alias.
            state.write(source, old_destination);
            state.write(destination, sum);
            state.rflags = flags;
        }
        Operation::CmpxchgMemory {
            destination,
            source,
        } => {
            let accumulator = RegisterView::low(Register::Rax, OperandSize::Qword);
            let expected = state.read(accumulator);
            let replacement = state.read(source);
            // Resolve the address from the pre-instruction state: RAX may be
            // both the accumulator and a source/base/index register.
            let address = match destination.access_address_48(state, next_rip) {
                Ok(address) => address,
                Err(error) => return report_memory_error(error),
            };
            let observed = match memory.atomic_compare_exchange_integer(
                address,
                context,
                OperandSize::Qword,
                expected,
                replacement,
            ) {
                Ok(observed) => observed,
                Err(error) => return report_memory_error(error),
            };
            let (_, flags) = arithmetic_result(
                OperandSize::Qword,
                ArithmeticKind::Cmp,
                expected,
                observed,
                state.rflags,
            );
            if expected != observed {
                state.write(accumulator, observed);
            }
            state.rflags = flags;
        }
        Operation::XaddMemory {
            destination,
            source,
        } => {
            // Save the register input before resolving or mutating anything;
            // it may also provide the memory base or index.
            let addend = state.read(source);
            let address = match destination.access_address_48(state, next_rip) {
                Ok(address) => address,
                Err(error) => return report_memory_error(error),
            };
            let old =
                match memory.atomic_fetch_add_integer(address, context, OperandSize::Qword, addend)
                {
                    Ok(old) => old,
                    Err(error) => return report_memory_error(error),
                };
            let (_, flags) = arithmetic_result(
                OperandSize::Qword,
                ArithmeticKind::Add,
                old,
                addend,
                state.rflags,
            );
            state.write(source, old);
            state.rflags = flags;
        }
        Operation::Cmpxchg {
            destination,
            source,
        } => {
            let accumulator = RegisterView::low(Register::Rax, OperandSize::Qword);
            let expected = state.read(accumulator);
            let observed = state.read(destination);
            let (_, flags) = arithmetic_result(
                OperandSize::Qword,
                ArithmeticKind::Cmp,
                expected,
                observed,
                state.rflags,
            );
            if expected == observed {
                state.write(destination, state.read(source));
            } else {
                state.write(accumulator, observed);
            }
            state.rflags = flags;
        }
        Operation::Arithmetic { kind, dst, src } => {
            let arithmetic = (|| {
                let lhs = read_operand(state, memory, context, next_rip, dst)?;
                let rhs = read_operand(state, memory, context, next_rip, src)?;
                let (result, flags) =
                    arithmetic_result(instruction.operand_size, kind, lhs, rhs, state.rflags);
                // Neither flags nor CPU aliases commit until a potentially
                // faulting destination write has completed transactionally.
                if kind != ArithmeticKind::Cmp {
                    write_operand(state, memory, context, next_rip, dst, result)?;
                }
                state.rflags = flags;
                Ok::<_, MemoryError>(())
            })();
            match arithmetic {
                Ok(()) => {}
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            }
        }
        Operation::Logic { kind, dst, src } => {
            let logic = (|| {
                let lhs = read_operand(state, memory, context, next_rip, dst)?;
                let rhs = read_operand(state, memory, context, next_rip, src)?;
                let (result, flags) =
                    logic_result(instruction.operand_size, kind, lhs, rhs, state.rflags);
                if kind != LogicKind::Test {
                    write_operand(state, memory, context, next_rip, dst, result)?;
                }
                state.rflags = flags;
                Ok::<_, MemoryError>(())
            })();
            match logic {
                Ok(()) => {}
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            }
        }
        Operation::Mul { signed, operand } => {
            let source = match read_operand(state, memory, context, next_rip, operand) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            let (low, high, flags) = multiply_implicit(
                instruction.operand_size,
                signed,
                source,
                state.gpr[Register::Rax as usize],
                state.rflags,
            );
            write_implicit_product(state, instruction.operand_size, low, high);
            state.rflags = flags;
        }
        Operation::Imul { dst, src } => {
            let lhs = state.read(dst);
            let rhs = match read_operand(state, memory, context, next_rip, src) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            let (result, flags) = multiply_truncated(dst.size(), lhs, rhs, state.rflags);
            state.write(dst, result);
            state.rflags = flags;
        }
        Operation::ImulImmediate {
            dst,
            src,
            immediate,
        } => {
            let lhs = match read_operand(state, memory, context, next_rip, src) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            let (result, flags) = multiply_truncated(dst.size(), lhs, immediate, state.rflags);
            state.write(dst, result);
            state.rflags = flags;
        }
        Operation::Divide { signed, divisor } => {
            let divisor_value = match read_operand(state, memory, context, next_rip, divisor) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            let Some((quotient, remainder)) = divide_implicit(
                instruction.operand_size,
                signed,
                divisor_value,
                state.gpr[Register::Rax as usize],
                state.gpr[Register::Rdx as usize],
            ) else {
                return Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: 0,
                    error_code: None,
                    linear_address: None,
                }));
            };
            write_implicit_division(state, instruction.operand_size, quotient, remainder);
        }
        Operation::Shift {
            kind,
            dst,
            source,
            count,
        } => {
            let shift = (|| {
                let raw_count = read_operand(state, memory, context, next_rip, count)? as u8;
                let count = shift_count(kind, instruction.operand_size, raw_count);
                let lhs = read_operand(state, memory, context, next_rip, dst)?;
                let source = match source {
                    Some(source) => read_operand(state, memory, context, next_rip, source)?,
                    None => 0,
                };
                let (result, flags) = shift_result(
                    instruction.operand_size,
                    kind,
                    lhs,
                    source,
                    count,
                    raw_count
                        & if instruction.operand_size == OperandSize::Qword {
                            0x3f
                        } else {
                            0x1f
                        },
                    state.rflags,
                );
                write_operand(state, memory, context, next_rip, dst, result)?;
                state.rflags = flags;
                *rflags_defined = instruction
                    .flag_effects_for_count(raw_count)
                    .apply(*rflags_defined);
                Ok::<_, MemoryError>(())
            })();
            match shift {
                Ok(()) => {}
                Err(MemoryError::Architectural(fault)) => return Ok(StepOutcome::Fault(fault)),
                Err(error) => return Err(error),
            }
            // Flag definedness was applied after the faulting write above.
            state.rip = next_rip;
            return Ok(StepOutcome::Advanced);
        }
        Operation::Branch {
            condition,
            displacement,
        } => {
            if condition.evaluate(state.rflags) {
                let target = next_rip.wrapping_add_signed(i64::from(displacement));
                if !canonical(target) {
                    let MemoryError::Architectural(fault) = general_protection() else {
                        unreachable!()
                    };
                    return Ok(StepOutcome::Fault(fault));
                }
                state.rip = target;
                return Ok(StepOutcome::Branched);
            }
        }
        Operation::Jump { displacement } => {
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !canonical(target) {
                let MemoryError::Architectural(fault) = general_protection() else {
                    unreachable!()
                };
                return Ok(StepOutcome::Fault(fault));
            }
            state.rip = target;
            return Ok(StepOutcome::Branched);
        }
        Operation::Call { displacement } => {
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !canonical(target) {
                let MemoryError::Architectural(fault) = general_protection() else {
                    unreachable!()
                };
                return Ok(StepOutcome::Fault(fault));
            }
            if let Err(error) =
                push_stack_value(state, memory, context, next_rip, OperandSize::Qword)
            {
                return report_memory_error(error);
            }
            state.rip = target;
            return Ok(StepOutcome::Branched);
        }
        Operation::IndirectCall { operand } => {
            // Resolve the complete source before the push. Memory sources are
            // read exactly once before target validation; every fault leaves
            // both the architectural state and stack untouched.
            let target = match read_operand(state, memory, context, next_rip, operand) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            if !canonical(target) {
                let MemoryError::Architectural(fault) = general_protection() else {
                    unreachable!()
                };
                return Ok(StepOutcome::Fault(fault));
            }
            if let Err(error) =
                push_stack_value(state, memory, context, next_rip, OperandSize::Qword)
            {
                return report_memory_error(error);
            }
            state.rip = target;
            return Ok(StepOutcome::Branched);
        }
        Operation::Return { stack_adjust } => {
            let old_rsp = state.gpr[Register::Rsp as usize];
            let stack_address = match stack_access_address(old_rsp, false, OperandSize::Qword) {
                Ok(address) => address,
                Err(error) => return report_memory_error(error),
            };
            let target = match memory.read_integer(stack_address, context, OperandSize::Qword) {
                Ok(target) => target,
                Err(error) => return report_memory_error(error),
            };
            if !canonical(target) {
                let MemoryError::Architectural(fault) = general_protection() else {
                    unreachable!()
                };
                return Ok(StepOutcome::Fault(fault));
            }
            state.gpr[Register::Rsp as usize] = old_rsp
                .wrapping_add(8)
                .wrapping_add(u64::from(stack_adjust));
            state.rip = target;
            return Ok(StepOutcome::Branched);
        }
        Operation::IndirectJump { operand } => {
            let target = match read_operand(state, memory, context, next_rip, operand) {
                Ok(value) => value,
                Err(MemoryError::Architectural(fault)) => {
                    return Ok(StepOutcome::Fault(fault));
                }
                Err(error) => return Err(error),
            };
            if !canonical(target) {
                let MemoryError::Architectural(fault) = general_protection() else {
                    unreachable!()
                };
                return Ok(StepOutcome::Fault(fault));
            }
            state.rip = target;
            return Ok(StepOutcome::Branched);
        }
    }
    *rflags_defined = instruction.flag_effects().apply(*rflags_defined);
    state.rip = next_rip;
    Ok(StepOutcome::Advanced)
}

fn operand_width(size: OperandSize) -> u32 {
    match size {
        OperandSize::Byte => 8,
        OperandSize::Word => 16,
        OperandSize::Dword => 32,
        OperandSize::Qword => 64,
    }
}

/// Return the untouched raw ST(0) bytes for bounded forms that require a
/// canonical-Normal Valid source. `None` is an atomic pre-commit rejection.
fn bounded_x87_normal_st0_source(state: &LongModeState) -> Option<[u8; 10]> {
    let x87 = state.x87.as_ref()?;
    if x87.control_word != 0x037f || x87.status_word & 0x80ff != 0 {
        return None;
    }
    let old_top = x87.top();
    let source = x87.registers[usize::from(old_top)];
    if x87.tag(old_top)? != X87Tag::Valid
        || X87Binary80::from_bytes(source).classify() != X87Binary80Class::Normal
    {
        return None;
    }
    Some(source)
}

/// Compare old physical ST(0) and ST(i) without converting through a host
/// floating-point format. `Some` contains only the FSW condition-code bits.
fn bounded_x87_fcom_result(state: &LongModeState, source_index: u8) -> Option<u16> {
    let x87 = state.x87.as_ref()?;
    if x87.control_word != 0x037f || x87.status_word & 0x80ff != 0 {
        return None;
    }
    let top = x87.top();
    let lhs_index = usize::from(top);
    let source_tag = (top + source_index) & 7;
    let rhs_index = usize::from(source_tag);
    if x87.tag(top)? != X87Tag::Valid {
        return None;
    }
    let lhs_raw = X87Binary80::from_bytes(x87.registers[lhs_index]);
    let lhs = lhs_raw.normalize_finite()?;
    if lhs.source_class() != X87Binary80Class::Normal {
        return None;
    }
    if source_index == 0 {
        return Some(0x4000);
    }
    if x87.tag(source_tag)? != X87Tag::Valid {
        return None;
    }
    let rhs_raw = X87Binary80::from_bytes(x87.registers[rhs_index]);
    let rhs = rhs_raw.normalize_finite()?;
    if rhs.source_class() != X87Binary80Class::Normal {
        return None;
    }
    let ordering = if lhs.sign() != rhs.sign() {
        if lhs.sign() {
            core::cmp::Ordering::Less
        } else {
            core::cmp::Ordering::Greater
        }
    } else {
        let magnitude =
            (lhs.exponent(), lhs.significand()).cmp(&(rhs.exponent(), rhs.significand()));
        if lhs.sign() {
            magnitude.reverse()
        } else {
            magnitude
        }
    };
    Some(match ordering {
        core::cmp::Ordering::Greater => 0,
        core::cmp::Ordering::Less => 0x0100,
        core::cmp::Ordering::Equal => 0x4000,
    })
}

fn exact_fadd_result(
    state: &LongModeState,
    destination_index: u8,
    source_index: u8,
) -> Option<X87Binary80> {
    exact_same_exponent_binary_result(state, destination_index, source_index, false)
}

/// Return only exact canonical-Normal results for the bounded FMUL profile.
/// Unlike FADD/FSUB, the operands may have different normalized exponents.
fn exact_fmul_result(state: &LongModeState, source_index: u8) -> Option<X87Binary80> {
    if source_index > 7 {
        return None;
    }
    let x87 = state.x87.as_ref()?;
    if x87.control_word != 0x037f || x87.status_word & 0x80ff != 0 {
        return None;
    }
    let destination = x87.physical_register_for_st(0)?;
    let source = x87.physical_register_for_st(source_index)?;
    if x87.tag(destination)? != X87Tag::Valid
        || (source != destination && x87.tag(source)? != X87Tag::Valid)
    {
        return None;
    }
    let destination_raw = X87Binary80::from_bytes(x87.registers[usize::from(destination)]);
    let source_raw = if source == destination {
        destination_raw
    } else {
        X87Binary80::from_bytes(x87.registers[usize::from(source)])
    };
    if destination_raw.classify() != X87Binary80Class::Normal
        || source_raw.classify() != X87Binary80Class::Normal
    {
        return None;
    }
    let destination_finite = destination_raw.normalize_finite()?;
    let source_finite = source_raw.normalize_finite()?;
    let (sign, exponent, magnitude) =
        multiply_finite_significands(destination_finite, source_finite)?;
    let (sign, exponent, normalized) = normalize_exact_magnitude(sign, exponent, magnitude)?;
    // Exact binary80 precision requires all product bits below the retained
    // 64-bit significand to be zero. No rounding or host floating point is used.
    if normalized as u64 != 0 {
        return None;
    }
    let result = pack_normal_rounded_significand(sign, exponent, normalized >> 64, 64)?;
    (result.classify() == X87Binary80Class::Normal).then_some(result)
}

/// Return only exact canonical-Normal results for the bounded FDIV profile.
/// The quotient remainder is checked before any rounding or packing occurs.
fn exact_fdiv_result(
    state: &LongModeState,
    numerator_index: u8,
    denominator_index: u8,
) -> Option<X87Binary80> {
    if numerator_index > 7 || denominator_index > 7 {
        return None;
    }
    let x87 = state.x87.as_ref()?;
    if x87.control_word != 0x037f || x87.status_word & 0x80ff != 0 {
        return None;
    }
    let numerator = x87.physical_register_for_st(numerator_index)?;
    let denominator = x87.physical_register_for_st(denominator_index)?;
    if x87.tag(numerator)? != X87Tag::Valid
        || (denominator != numerator && x87.tag(denominator)? != X87Tag::Valid)
    {
        return None;
    }
    let numerator_raw = X87Binary80::from_bytes(x87.registers[usize::from(numerator)]);
    let denominator_raw = if denominator == numerator {
        numerator_raw
    } else {
        X87Binary80::from_bytes(x87.registers[usize::from(denominator)])
    };
    if numerator_raw.classify() != X87Binary80Class::Normal
        || denominator_raw.classify() != X87Binary80Class::Normal
    {
        return None;
    }
    let quotient = divide_finite_significands(
        numerator_raw.normalize_finite()?,
        denominator_raw.normalize_finite()?,
    )?;
    if quotient.remainder() != 0 {
        return None;
    }
    let result = pack_normal_rounded_significand(
        quotient.sign(),
        quotient.exponent(),
        u128::from(quotient.quotient()),
        64,
    )?;
    (result.classify() == X87Binary80Class::Normal).then_some(result)
}

/// Return an exact-only local FSUB result. Aliased ST(i)-ST(0) cancellation
/// is deliberately outside this bounded profile.
fn exact_fsub_result(
    state: &LongModeState,
    destination_index: u8,
    source_index: u8,
) -> Option<X87Binary80> {
    if destination_index > 7 || source_index > 7 || destination_index == source_index {
        return None;
    }
    exact_same_exponent_binary_result(state, destination_index, source_index, true)
}

fn exact_same_exponent_binary_result(
    state: &LongModeState,
    destination_index: u8,
    source_index: u8,
    subtract: bool,
) -> Option<X87Binary80> {
    let x87 = state.x87.as_ref()?;
    if x87.control_word != 0x037f || x87.status_word & 0x80ff != 0 {
        return None;
    }
    let destination = x87.physical_register_for_st(destination_index)?;
    let source = x87.physical_register_for_st(source_index)?;
    if x87.tag(destination)? != X87Tag::Valid
        || (source != destination && x87.tag(source)? != X87Tag::Valid)
    {
        return None;
    }
    let lhs_raw = X87Binary80::from_bytes(x87.registers[destination as usize]);
    let rhs_raw = if source == destination {
        lhs_raw
    } else {
        X87Binary80::from_bytes(x87.registers[source as usize])
    };
    if lhs_raw.classify() != X87Binary80Class::Normal
        || rhs_raw.classify() != X87Binary80Class::Normal
    {
        return None;
    }
    let lhs = lhs_raw.normalize_finite()?;
    // FSUB is implemented by toggling the sign in a local raw copy only after
    // the original source has passed the canonical-Normal gate. The guest
    // source register and its raw payload are never modified.
    let rhs_for_sum = if subtract {
        let mut bytes = rhs_raw.to_bytes();
        bytes[9] ^= 0x80;
        X87Binary80::from_bytes(bytes)
    } else {
        rhs_raw
    };
    let rhs = rhs_for_sum.normalize_finite()?;
    if lhs.exponent() != rhs.exponent() {
        return None;
    }
    let (exponent, coefficient) = add_finite_same_exponent(lhs, rhs)?;
    let magnitude = coefficient.unsigned_abs();
    // A 64-bit coefficient is directly representable. The only wider exact
    // sum comes from a carry and is representable when its low bit is zero.
    if magnitude > u64::MAX as u128 && (magnitude > (1_u128 << 65) - 2 || magnitude & 1 != 0) {
        return None;
    }
    let result =
        round_pack_exact_coefficient(exponent, coefficient, RationalRoundingMode::NearestTiesEven)?;
    (result.classify() == X87Binary80Class::Normal).then_some(result)
}

fn verify_writable_gdt_selector(
    state: &LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    selector: u16,
) -> Result<bool, MemoryError> {
    let Some(gdtr) = state.gdtr else {
        return Ok(false);
    };
    let offset = selector & !7;
    if selector & !3 == 0 || u32::from(offset) + 7 > u32::from(gdtr.limit) {
        return Ok(false);
    }
    let address = gdtr
        .base
        .checked_add(u64::from(offset))
        .ok_or_else(general_protection)?;
    let last = address.checked_add(7).ok_or_else(general_protection)?;
    if !canonical(address) || !canonical(last) {
        return Err(general_protection());
    }

    let mut descriptor = [0; 8];
    memory.read(address, context.descriptor_table(), &mut descriptor)?;
    let access = descriptor[5];
    let system = access & 0x10 == 0;
    let executable = access & 0x08 != 0;
    let writable = access & 0x02 != 0;
    let dpl = (access >> 5) & 3;
    let cpl = match context.privilege {
        PrivilegeLevel::Ring0 => 0,
        PrivilegeLevel::Ring1 => 1,
        PrivilegeLevel::Ring2 => 2,
        PrivilegeLevel::Ring3 => 3,
    };
    let rpl = (selector & 3) as u8;
    Ok(!system && !executable && writable && cpl <= dpl && rpl <= dpl)
}

fn selector_fault(vector: u8, selector: u16) -> MemoryError {
    MemoryError::Architectural(ArchitecturalFault {
        vector,
        error_code: Some(u32::from(selector & 0xfffc)),
        linear_address: None,
    })
}

fn load_fs_gdt_selector(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    selector: u16,
) -> Result<bool, MemoryError> {
    load_gdt_selector(state, memory, context, selector, false)
}

fn load_gdt_selector(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    selector: u16,
    gs: bool,
) -> Result<bool, MemoryError> {
    let Some(gdtr) = state.gdtr else {
        return Ok(false);
    };
    let cache = if gs { state.gs_cache } else { state.fs_cache };
    let Some(_) = cache else {
        return Ok(false);
    };
    if selector & 0xfffc == 0 || selector & 4 != 0 {
        return Ok(false);
    }
    let offset = selector & !7;
    if u32::from(offset) + 7 > u32::from(gdtr.limit) {
        return Err(selector_fault(13, selector));
    }
    let address = gdtr
        .base
        .checked_add(u64::from(offset))
        .ok_or_else(|| selector_fault(13, selector))?;
    let last = address
        .checked_add(7)
        .ok_or_else(|| selector_fault(13, selector))?;
    if !canonical(address) || !canonical(last) {
        return Err(selector_fault(13, selector));
    }

    let descriptor_context = context.descriptor_table();
    let mut descriptor = [0; 8];
    memory.read(address, descriptor_context, &mut descriptor)?;
    let access = descriptor[5];
    let kind = access & 0x0f;
    let system = access & 0x10 == 0;
    let code = kind & 0x08 != 0;
    let conforming = code && kind & 0x04 != 0;
    let readable = !code || kind & 0x02 != 0;
    if system || !readable {
        return Err(selector_fault(13, selector));
    }

    let dpl = (access >> 5) & 3;
    let cpl = match context.privilege {
        PrivilegeLevel::Ring0 => 0,
        PrivilegeLevel::Ring1 => 1,
        PrivilegeLevel::Ring2 => 2,
        PrivilegeLevel::Ring3 => 3,
    };
    let rpl = (selector & 3) as u8;
    if !conforming && cpl.max(rpl) > dpl {
        return Err(selector_fault(13, selector));
    }
    if access & 0x80 == 0 {
        return Err(selector_fault(11, selector));
    }
    let limit20 = u32::from(descriptor[0])
        | (u32::from(descriptor[1]) << 8)
        | (u32::from(descriptor[6] & 0x0f) << 16);
    let flags = descriptor[6] >> 4;
    let limit = if flags & 8 != 0 {
        (limit20 << 12) | 0x0fff
    } else {
        limit20
    };
    let base32 = u32::from(descriptor[2])
        | (u32::from(descriptor[3]) << 8)
        | (u32::from(descriptor[4]) << 16)
        | (u32::from(descriptor[7]) << 24);

    if access & 0x01 == 0 {
        // The bounded profile uses one synchronous byte write to set the
        // descriptor's accessed bit. Keep every possible failure before the
        // CPU-state commit; on success the remaining assignments cannot fail.
        memory.write(address + 5, descriptor_context, &[access | 1])?;
    }
    let cached_access = access | 1;

    let cache = Some(FsDescriptorCache {
        limit,
        access: cached_access,
        flags,
    });
    if gs {
        state.gs_selector = selector;
        state.gs_base = u64::from(base32);
        state.gs_cache = cache;
    } else {
        state.fs_selector = selector;
        state.fs_base = u64::from(base32);
        state.fs_cache = cache;
    }
    Ok(true)
}

fn sign_extend_accumulator(state: &mut LongModeState, size: OperandSize, double_width: bool) {
    let width = operand_width(size);
    let accumulator = state.read(RegisterView::low(Register::Rax, size));
    if double_width {
        let sign_bit = 1u64 << (width - 1);
        let high_half = if accumulator & sign_bit != 0 {
            size.mask()
        } else {
            0
        };
        state.write(RegisterView::low(Register::Rdx, size), high_half);
    } else {
        let source_width = width / 2;
        let shift = 64 - source_width;
        let result = (((accumulator << shift) as i64) >> shift) as u64 & size.mask();
        state.write(RegisterView::low(Register::Rax, size), result);
    }
}

fn signed_value(size: OperandSize, value: u64) -> i128 {
    match size {
        OperandSize::Byte => value as u8 as i8 as i128,
        OperandSize::Word => value as u16 as i16 as i128,
        OperandSize::Dword => value as u32 as i32 as i128,
        OperandSize::Qword => value as i64 as i128,
    }
}

fn signed_low_half(size: OperandSize, value: u64) -> i128 {
    signed_value(size, value & size.mask())
}

fn multiply_flags(previous: u64, overflow: bool) -> u64 {
    (previous & !(crate::CF | crate::OF)) | if overflow { crate::CF | crate::OF } else { 0 }
}

fn multiply_implicit(
    size: OperandSize,
    signed: bool,
    source: u64,
    accumulator: u64,
    previous_flags: u64,
) -> (u64, u64, u64) {
    let width = operand_width(size);
    let (product, overflow) = if signed {
        let product = signed_low_half(size, source) * signed_low_half(size, accumulator);
        let low = product as u64 & size.mask();
        (product as u128, product != signed_low_half(size, low))
    } else {
        let product = u128::from(source & size.mask()) * u128::from(accumulator & size.mask());
        (product, product >> width != 0)
    };
    let low = product as u64 & size.mask();
    let high = (product >> width) as u64 & size.mask();
    (low, high, multiply_flags(previous_flags, overflow))
}

fn multiply_truncated(size: OperandSize, lhs: u64, rhs: u64, previous_flags: u64) -> (u64, u64) {
    let product = signed_low_half(size, lhs) * signed_low_half(size, rhs);
    let low = product as u64 & size.mask();
    let overflow = product != signed_low_half(size, low);
    (low, multiply_flags(previous_flags, overflow))
}

fn write_implicit_product(state: &mut LongModeState, size: OperandSize, low: u64, high: u64) {
    if size == OperandSize::Byte {
        state.write(RegisterView::low(Register::Rax, size), low);
        state.write(
            RegisterView::high_byte(Register::Rax, false).expect("legacy AH alias is valid"),
            high,
        );
    } else {
        state.write(RegisterView::low(Register::Rax, size), low);
        state.write(RegisterView::low(Register::Rdx, size), high);
    }
}

fn divide_implicit(
    size: OperandSize,
    signed: bool,
    divisor: u64,
    accumulator: u64,
    data: u64,
) -> Option<(u64, u64)> {
    let width = operand_width(size);
    let low = accumulator & size.mask();
    let high = data & size.mask();
    let dividend_bits = if size == OperandSize::Byte {
        u128::from(accumulator & 0xffff)
    } else {
        (u128::from(high) << width) | u128::from(low)
    };

    if signed {
        let dividend = match size {
            OperandSize::Byte => dividend_bits as u16 as i16 as i128,
            OperandSize::Word => dividend_bits as u32 as i32 as i128,
            OperandSize::Dword => dividend_bits as u64 as i64 as i128,
            OperandSize::Qword => dividend_bits as i128,
        };
        let divisor = signed_value(size, divisor);
        if divisor == 0 || (dividend == i128::MIN && divisor == -1) {
            return None;
        }
        let quotient = dividend / divisor;
        let remainder = dividend % divisor;
        let min = -(1i128 << (width - 1));
        let max = (1i128 << (width - 1)) - 1;
        if quotient < min || quotient > max {
            return None;
        }
        Some((
            quotient as u64 & size.mask(),
            remainder as u64 & size.mask(),
        ))
    } else {
        let divisor = u128::from(divisor & size.mask());
        if divisor == 0 {
            return None;
        }
        let quotient = dividend_bits / divisor;
        if quotient > u128::from(size.mask()) {
            return None;
        }
        Some((quotient as u64, (dividend_bits % divisor) as u64))
    }
}

fn write_implicit_division(
    state: &mut LongModeState,
    size: OperandSize,
    quotient: u64,
    remainder: u64,
) {
    if size == OperandSize::Byte {
        state.write(RegisterView::low(Register::Rax, size), quotient);
        state.write(
            RegisterView::high_byte(Register::Rax, false).expect("legacy AH alias is valid"),
            remainder,
        );
    } else {
        state.write(RegisterView::low(Register::Rax, size), quotient);
        state.write(RegisterView::low(Register::Rdx, size), remainder);
    }
}

fn logic_result(
    size: OperandSize,
    kind: LogicKind,
    lhs: u64,
    rhs: u64,
    previous: u64,
) -> (u64, u64) {
    let result = match kind {
        LogicKind::And | LogicKind::Test => lhs & rhs,
        LogicKind::Or => lhs | rhs,
        LogicKind::Xor => lhs ^ rhs,
        LogicKind::Not => !lhs,
    } & size.mask();
    if kind == LogicKind::Not {
        return (result, previous);
    }
    // SDM093 AND/OR/XOR/TEST: CF/OF cleared; SZP from the truncated result.
    // AF is undefined. Retaining its concrete bit is a deterministic local
    // choice; flag_effects() removes its architectural definedness separately.
    let mut flags = previous & !(crate::CF | crate::OF | crate::SF | crate::ZF | crate::PF);
    if result == 0 {
        flags |= crate::ZF;
    }
    if result & (1 << (size.bytes() * 8 - 1)) != 0 {
        flags |= crate::SF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= crate::PF;
    }
    (result, flags)
}

fn shift_count(kind: ShiftKind, size: OperandSize, raw: u8) -> u8 {
    let width = (size.bytes() * 8) as u8;
    let masked = raw & if width == 64 { 0x3f } else { 0x1f };
    match kind {
        ShiftKind::Rol | ShiftKind::Ror => masked % width,
        ShiftKind::Rcl | ShiftKind::Rcr if width == 8 => masked % 9,
        ShiftKind::Rcl | ShiftKind::Rcr if width == 16 => masked % 17,
        _ => masked,
    }
}

fn shift_result(
    size: OperandSize,
    kind: ShiftKind,
    lhs: u64,
    source: u64,
    count: u8,
    masked_count: u8,
    previous: u64,
) -> (u64, u64) {
    let width = (size.bytes() * 8) as u32;
    let mask = size.mask();
    let lhs = lhs & mask;
    let source = source & mask;
    let sign = 1u64 << (width - 1);
    if masked_count == 0 {
        return (lhs, previous);
    }
    let mut flags = previous & !(crate::CF | crate::OF | crate::SF | crate::ZF | crate::PF);
    let (result, carry, overflow, arithmetic) = match kind {
        ShiftKind::Shl | ShiftKind::Shr | ShiftKind::Sar => {
            let c = u32::from(count);
            let carry = if kind == ShiftKind::Sar && c > width {
                u64::from(lhs & sign != 0)
            } else if c <= width {
                if kind == ShiftKind::Shl {
                    lhs >> (width - c) & 1
                } else {
                    lhs >> (c - 1) & 1
                }
            } else {
                0
            };
            let result = match kind {
                ShiftKind::Shl if c >= width => 0,
                ShiftKind::Shl => lhs << c & mask,
                ShiftKind::Shr if c >= width => 0,
                ShiftKind::Shr => lhs >> c,
                ShiftKind::Sar if c >= width => {
                    if lhs & sign != 0 {
                        mask
                    } else {
                        0
                    }
                }
                ShiftKind::Sar => {
                    let shifted = lhs >> c;
                    if lhs & sign != 0 {
                        shifted | (mask ^ (mask >> c))
                    } else {
                        shifted
                    }
                }
                _ => unreachable!(),
            };
            let overflow = if c != 1 {
                None
            } else {
                Some(match kind {
                    ShiftKind::Shl => ((result & sign != 0) ^ (carry != 0)) as u64,
                    ShiftKind::Shr => ((lhs & sign) != 0) as u64,
                    ShiftKind::Sar => 0,
                    _ => unreachable!(),
                })
            };
            (result, carry, overflow, true)
        }
        ShiftKind::Rol | ShiftKind::Ror => {
            let c = u32::from(count);
            let result = if c == 0 {
                lhs
            } else if kind == ShiftKind::Rol {
                (lhs << c | lhs >> (width - c)) & mask
            } else {
                (lhs >> c | lhs << (width - c)) & mask
            };
            let carry = if masked_count == 0 {
                (previous & crate::CF != 0) as u64
            } else if kind == ShiftKind::Rol {
                result & 1
            } else {
                (result & sign != 0) as u64
            };
            let overflow = if masked_count == 1 {
                Some(if kind == ShiftKind::Rol {
                    ((result & sign != 0) ^ (carry != 0)) as u64
                } else {
                    (((result & sign) != 0) ^ ((result & (sign >> 1)) != 0)) as u64
                })
            } else {
                None
            };
            (result, carry, overflow, false)
        }
        ShiftKind::Rcl | ShiftKind::Rcr => {
            let mut value = lhs;
            let mut carry = (previous & crate::CF != 0) as u64;
            for _ in 0..count {
                let next_carry = if kind == ShiftKind::Rcl {
                    value >> (width - 1)
                } else {
                    value & 1
                };
                value = if kind == ShiftKind::Rcl {
                    (value << 1 | carry) & mask
                } else {
                    value >> 1 | (carry << (width - 1))
                };
                carry = next_carry;
            }
            let overflow = if masked_count == 1 {
                Some(if kind == ShiftKind::Rcl {
                    (((value & sign) != 0) ^ (carry != 0)) as u64
                } else {
                    (((value & sign) != 0) ^ ((value & (sign >> 1)) != 0)) as u64
                })
            } else {
                None
            };
            (value, carry, overflow, false)
        }
        ShiftKind::Shld | ShiftKind::Shrd => {
            let c = u32::from(count);
            let (result, carry) = if kind == ShiftKind::Shld {
                let pair = (u128::from(lhs) << width) | u128::from(source);
                let result = ((pair << c) >> width) as u64 & mask;
                let carry = if c <= width {
                    (lhs >> (width - c)) & 1
                } else {
                    0
                };
                (result, carry)
            } else {
                let pair = (u128::from(source) << width) | u128::from(lhs);
                let result = (pair >> c) as u64 & mask;
                let carry = if c <= width { (lhs >> (c - 1)) & 1 } else { 0 };
                (result, carry)
            };
            let overflow = if c == 1 {
                Some(if kind == ShiftKind::Shld {
                    (((result & sign) != 0) ^ (carry != 0)) as u64
                } else {
                    (((lhs & sign) != 0) ^ ((result & sign) != 0)) as u64
                })
            } else {
                None
            };
            (result, carry, overflow, true)
        }
    };
    if arithmetic {
        if result == 0 {
            flags |= crate::ZF;
        }
        if result & sign != 0 {
            flags |= crate::SF;
        }
        if (result as u8).count_ones() % 2 == 0 {
            flags |= crate::PF;
        }
    } else {
        flags |= previous & (crate::SF | crate::ZF | crate::PF);
    }
    if carry != 0 {
        flags |= crate::CF;
    }
    if let Some(value) = overflow {
        if value != 0 {
            flags |= crate::OF;
        }
    } else {
        flags |= previous & crate::OF;
    }
    (result, flags)
}

fn stack_access_address(rsp: u64, push: bool, size: OperandSize) -> Result<u64, MemoryError> {
    let length = size.bytes();
    let address = if push {
        rsp.wrapping_sub(length as u64)
    } else {
        rsp
    };
    let last = address
        .checked_add(length as u64 - 1)
        .ok_or(MemoryError::AddressOverflow { address, length })?;
    if !long_decode::canonical(address) || !long_decode::canonical(last) {
        return Err(MemoryError::Architectural(ArchitecturalFault {
            vector: 12,
            error_code: Some(0),
            linear_address: None,
        }));
    }
    Ok(address)
}

fn push_stack_value(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    value: u64,
    size: OperandSize,
) -> Result<(), MemoryError> {
    let address = stack_access_address(state.gpr[Register::Rsp as usize], true, size)?;
    memory.write_integer(address, context, size, value)?;
    state.gpr[Register::Rsp as usize] = address;
    Ok(())
}

fn report_memory_error(error: MemoryError) -> Result<StepOutcome, MemoryError> {
    match error {
        MemoryError::Architectural(fault) => Ok(StepOutcome::Fault(fault)),
        error => Err(error),
    }
}

fn transfer(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    next_rip: u64,
    operation: Operation,
) -> Result<(), MemoryError> {
    match operation {
        Operation::MovImmediate { dst, value } => {
            write_operand(state, memory, context, next_rip, dst, value)?
        }
        Operation::Mov { dst, src } => {
            let value = read_operand(state, memory, context, next_rip, src)?;
            write_operand(state, memory, context, next_rip, dst, value)?;
        }
        Operation::MovFsLoad { dst, src } => {
            let address =
                src.access_address_48_with_segment_base(state, next_rip, state.fs_base)?;
            let value = memory.read_integer(address, context, src.size)?;
            state.write(dst, value);
        }
        Operation::MovGsLoad { dst, src } => {
            let address =
                src.access_address_48_with_segment_base(state, next_rip, state.gs_base)?;
            let value = memory.read_integer(address, context, src.size)?;
            state.write(dst, value);
        }
        Operation::MovFsSelectorToRax => {
            state.gpr[Register::Rax as usize] = u64::from(state.fs_selector);
        }
        Operation::MovGsSelectorToRax => {
            state.gpr[Register::Rax as usize] = u64::from(state.gs_selector);
        }
        Operation::Extend { dst, src, signed } => {
            let value = read_operand(state, memory, context, next_rip, src)?;
            let source_size = match src {
                Operand::Register(view) => view.size(),
                Operand::Memory(mem) => mem.size,
                Operand::Immediate(_) => {
                    unreachable!("extension source is a register or memory operand")
                }
            };
            let value = if signed {
                // Explicit two's-complement extension of the source width.
                let shift = 64 - source_size.bytes() * 8;
                (((value << shift) as i64) >> shift) as u64
            } else {
                value
            };
            state.write(dst, value);
        }
        Operation::Lea { dst, src } => state.write(dst, src.effective_address(state, next_rip)),
        Operation::Xchg { left, right } => {
            // Capture both aliases before either write: AL/AH may share a GPR.
            let lhs = state.read(left);
            let rhs = state.read(right);
            state.write(left, rhs);
            state.write(right, lhs);
        }
        Operation::XchgMemory {
            register,
            memory: operand,
        } => {
            let address = operand.access_address_48(state, next_rip)?;
            let replacement = state.read(register);
            let previous =
                memory.atomic_exchange_integer(address, context, operand.size, replacement)?;
            state.write(register, previous);
        }
        Operation::Nop => {}
        Operation::Mfence => memory.mfence(context)?,
        _ => unreachable!("only transfer-family operations call transfer"),
    }
    Ok(())
}

fn read_operand(
    state: &LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    next_rip: u64,
    operand: Operand,
) -> Result<u64, MemoryError> {
    match operand {
        Operand::Register(view) => Ok(state.read(view)),
        Operand::Immediate(value) => Ok(value),
        Operand::Memory(mem) => {
            let address = mem.access_address_48(state, next_rip)?;
            memory.read_integer(address, context, mem.size)
        }
    }
}

fn write_operand(
    state: &mut LongModeState,
    memory: &mut impl GuestMemory,
    context: AccessContext,
    next_rip: u64,
    operand: Operand,
    value: u64,
) -> Result<(), MemoryError> {
    match operand {
        Operand::Register(view) => state.write(view, value),
        Operand::Immediate(_) => unreachable!("an instruction cannot write an immediate"),
        Operand::Memory(mem) => {
            let address = mem.access_address_48(state, next_rip)?;
            memory.write_integer(address, context, mem.size, value)?;
        }
    }
    Ok(())
}

/// Width-aware integer arithmetic, using a 128-bit lane for carry/borrow even
/// at qword width. Intel SDM093 ADD/ADC/SUB/SBB/CMP/INC/DEC/NEG operation and
/// flags sections. No host signed overflow or width-sized shift is relied upon.
fn arithmetic_result(
    size: OperandSize,
    kind: ArithmeticKind,
    lhs: u64,
    rhs: u64,
    previous: u64,
) -> (u64, u64) {
    let mask = size.mask();
    let lhs = lhs & mask;
    let rhs = rhs & mask;
    let (left, right, carry, subtract) = match kind {
        ArithmeticKind::Add => (lhs, rhs, 0, false),
        ArithmeticKind::Adc => (lhs, rhs, previous & crate::CF, false),
        ArithmeticKind::Sub | ArithmeticKind::Cmp => (lhs, rhs, 0, true),
        ArithmeticKind::Sbb => (lhs, rhs, previous & crate::CF, true),
        ArithmeticKind::Inc => (lhs, 1, 0, false),
        ArithmeticKind::Dec => (lhs, 1, 0, true),
        ArithmeticKind::Neg => (0, lhs, 0, true),
    };
    let extended_right = u128::from(right) + u128::from(carry);
    let wide = if subtract {
        u128::from(left).wrapping_sub(extended_right)
    } else {
        u128::from(left) + extended_right
    };
    let result = wide as u64 & mask;
    let cf = if subtract {
        u128::from(left) < extended_right
    } else {
        wide > u128::from(mask)
    };
    let sign = 1u64 << (size.bytes() * 8 - 1);
    let of = if subtract {
        (left ^ right) & (left ^ result) & sign != 0
    } else {
        (!(left ^ right)) & (left ^ result) & sign != 0
    };
    let mut flags = previous & !crate::ARITHMETIC_FLAGS;
    if cf {
        flags |= crate::CF;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= crate::PF;
    }
    if (left ^ right ^ result) & 0x10 != 0 {
        flags |= crate::AF;
    }
    if result == 0 {
        flags |= crate::ZF;
    }
    if result & sign != 0 {
        flags |= crate::SF;
    }
    if of {
        flags |= crate::OF;
    }
    if matches!(kind, ArithmeticKind::Inc | ArithmeticKind::Dec) {
        flags = (flags & !crate::CF) | (previous & crate::CF);
    }
    (result, flags)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Register;
    use crate::memory::AccessKind;

    struct Memory {
        code: Vec<u8>,
        code_base: u64,
        bytes: Vec<u8>,
        accesses: Vec<(AccessKind, u64, usize)>,
        contexts: Vec<(AccessKind, AccessContext)>,
        error: Option<MemoryError>,
        error_kind: Option<AccessKind>,
        deny_writes: bool,
        atomic_exchanges: Vec<(u64, OperandSize, u64)>,
        atomic_adds: Vec<(u64, OperandSize, u64)>,
        atomic_compare_exchanges: Vec<(u64, OperandSize, u64, u64)>,
        atomic_error: Option<MemoryError>,
        fetch_error: Option<(u64, MemoryError)>,
    }
    impl Memory {
        fn new(code: &[u8]) -> Self {
            Self::at(code, 0)
        }
        fn at(code: &[u8], code_base: u64) -> Self {
            Self {
                code: code.to_vec(),
                code_base,
                bytes: (0..32).collect(),
                accesses: vec![],
                contexts: vec![],
                error: None,
                error_kind: None,
                deny_writes: false,
                atomic_exchanges: vec![],
                atomic_adds: vec![],
                atomic_compare_exchanges: vec![],
                atomic_error: None,
                fetch_error: None,
            }
        }
        fn range(
            &mut self,
            address: u64,
            length: usize,
            kind: AccessKind,
        ) -> Result<std::ops::Range<usize>, MemoryError> {
            self.accesses.push((kind, address, length));
            if let Some(error) = &self.error {
                if self.error_kind.is_none_or(|access| access == kind) {
                    return Err(error.clone());
                }
            }
            let start = address
                .checked_sub(0x1000)
                .and_then(|value| usize::try_from(value).ok());
            let range = start.and_then(|start| start.checked_add(length).map(|end| start..end));
            range
                .filter(|range| range.end <= self.bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: kind,
                })
        }
    }
    impl GuestMemory for Memory {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            if let Some((at, error)) = &self.fetch_error {
                if *at == address {
                    return Err(error.clone());
                }
            }
            self.code
                .get(
                    address
                        .checked_sub(self.code_base)
                        .and_then(|offset| usize::try_from(offset).ok())
                        .unwrap_or(usize::MAX),
                )
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(
            &mut self,
            address: u64,
            context: AccessContext,
            destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            self.contexts.push((AccessKind::Read, context));
            let range = self.range(address, destination.len(), AccessKind::Read)?;
            destination.copy_from_slice(&self.bytes[range]);
            Ok(())
        }
        fn write(
            &mut self,
            address: u64,
            context: AccessContext,
            source: &[u8],
        ) -> Result<(), MemoryError> {
            self.contexts.push((AccessKind::Write, context));
            if self.deny_writes {
                self.accesses
                    .push((AccessKind::Write, address, source.len()));
                return Err(MemoryError::PermissionDenied {
                    address,
                    access: AccessKind::Write,
                });
            }
            let range = self.range(address, source.len(), AccessKind::Write)?;
            self.bytes[range].copy_from_slice(source);
            Ok(())
        }

        fn atomic_exchange_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
            replacement: u64,
        ) -> Result<u64, MemoryError> {
            self.atomic_exchanges.push((address, size, replacement));
            if let Some(error) = &self.atomic_error {
                return Err(error.clone());
            }
            let length = size.bytes();
            let read_start = address
                .checked_sub(0x1000)
                .and_then(|value| usize::try_from(value).ok());
            let read_range = read_start
                .and_then(|start| start.checked_add(length).map(|end| start..end))
                .filter(|range| range.end <= self.bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
            let write_range = read_range.clone();
            let mut old_bytes = [0u8; 8];
            old_bytes[..length].copy_from_slice(&self.bytes[read_range]);
            self.bytes[write_range].copy_from_slice(&replacement.to_le_bytes()[..length]);
            Ok(u64::from_le_bytes(old_bytes))
        }

        fn atomic_fetch_add_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
            addend: u64,
        ) -> Result<u64, MemoryError> {
            self.atomic_adds.push((address, size, addend));
            if let Some(error) = &self.atomic_error {
                return Err(error.clone());
            }
            let length = size.bytes();
            let start = address
                .checked_sub(0x1000)
                .and_then(|value| usize::try_from(value).ok());
            let read_range = start
                .and_then(|start| start.checked_add(length).map(|end| start..end))
                .filter(|range| range.end <= self.bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
            let write_range = read_range.clone();
            let mut old_bytes = [0u8; 8];
            old_bytes[..length].copy_from_slice(&self.bytes[read_range]);
            let old = u64::from_le_bytes(old_bytes);
            let sum = old.wrapping_add(addend) & size.mask();
            self.bytes[write_range].copy_from_slice(&sum.to_le_bytes()[..length]);
            Ok(old)
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            address: u64,
            _: AccessContext,
            size: OperandSize,
            expected: u64,
            replacement: u64,
        ) -> Result<u64, MemoryError> {
            self.atomic_compare_exchanges
                .push((address, size, expected, replacement));
            if let Some(error) = &self.atomic_error {
                return Err(error.clone());
            }
            let length = size.bytes();
            let start = address
                .checked_sub(0x1000)
                .and_then(|value| usize::try_from(value).ok());
            let range = start
                .and_then(|start| start.checked_add(length).map(|end| start..end))
                .filter(|range| range.end <= self.bytes.len())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Read,
                })?;
            let mut old_bytes = [0u8; 8];
            old_bytes[..length].copy_from_slice(&self.bytes[range.clone()]);
            let old = u64::from_le_bytes(old_bytes);
            if old == (expected & size.mask()) {
                self.bytes[range].copy_from_slice(&replacement.to_le_bytes()[..length]);
            }
            Ok(old)
        }

        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[test]
    fn direct_relative_branches_use_next_rip_and_preserve_flags() {
        let flags = crate::CF | crate::PF | crate::ZF;
        let mut state = LongModeState {
            gpr: [0x1122; 16],
            rflags: flags,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        let mut defined = crate::ARITHMETIC_FLAGS;
        let mut memory = Memory::new(&[0x74, 0x02]); // JZ +2, target 4
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined
            ),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, 4);
        assert_eq!(state.rflags, flags);
        assert_eq!(defined, crate::ARITHMETIC_FLAGS);

        state.rip = 0;
        state.rflags = crate::ZF;
        let mut memory = Memory::new(&[0x0f, 0x85, 0x02, 0, 0, 0]); // JNZ false; fall through by six
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.rip, 6);

        state.rip = 0;
        let mut memory = Memory::new(&[0xeb, 0xfe]); // JMP -2 from next RIP
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, 0);
        state.rip = 0;
        let mut memory = Memory::new(&[0xe9, 0xfa, 0xff, 0xff, 0xff]); // JMP -6
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, u64::MAX);
    }

    #[test]
    fn indirect_register_jmp_updates_rip_or_faults_before_commit() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::R8 as usize] = 0xffff_8000_0000_1234;
        let before_flags = state.rflags;
        let mut defined = crate::CF | crate::ZF;
        let mut memory = Memory::at(&[0x41, 0xff, 0xe0], state.rip);
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined
            ),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, 0xffff_8000_0000_1234);
        assert_eq!(state.rflags, before_flags);
        assert_eq!(defined, crate::CF | crate::ZF);

        let before = LongModeState {
            gpr: [0x55; 16],
            rflags: before_flags,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        let mut state = before.clone();
        state.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut defined = crate::INTEGER_FLAGS;
        let mut memory = Memory::at(&[0xff, 0xe0], state.rip);
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined
            ),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(state, before);
        assert_eq!(defined, crate::INTEGER_FLAGS);
    }

    #[test]
    fn indirect_memory_jmp_reads_exact_qword_before_target_validation() {
        let base = 0x2000;
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: base,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1008;
        let target = 0xffff_8000_0000_1234u64;
        let mut memory = Memory::at(&[0xff, 0x20], base);
        memory.bytes[8..16].copy_from_slice(&target.to_le_bytes());
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, target);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);

        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: base,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1000;
        let before = state.clone();
        let mut memory = Memory::at(&[0xff, 0x20], base);
        memory.bytes[..8].copy_from_slice(&0x0000_8000_0000_0000u64.to_le_bytes());
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);

        let mut state = before.clone();
        let mut memory = Memory::at(&[0xff, 0x20], base);
        memory.error = Some(MemoryError::Unmapped {
            address: 0x1000,
            access: AccessKind::Read,
        });
        memory.error_kind = Some(AccessKind::Read);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 0x1000,
                access: AccessKind::Read,
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);
    }

    #[test]
    fn indirect_memory_call_reads_source_then_pushes_and_faults_without_commit() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1008;
        state.gpr[Register::Rsp as usize] = 0x1018;
        let mut memory = Memory::at(&[0xff, 0x10], 0x1000);
        let target = 0x1010u64;
        memory.bytes[8..16].copy_from_slice(&target.to_le_bytes());
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, target);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x1010);
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Read, 0x1008, 8),
                (AccessKind::Write, 0x1010, 8)
            ]
        );
        assert_eq!(
            u64::from_le_bytes(memory.bytes[16..24].try_into().unwrap()),
            0x1002
        );

        for (error, kind, expected_accesses) in [
            (
                MemoryError::Unmapped {
                    address: 0x1008,
                    access: AccessKind::Read,
                },
                AccessKind::Read,
                vec![(AccessKind::Read, 0x1008, 8)],
            ),
            (
                MemoryError::Unmapped {
                    address: 0x1010,
                    access: AccessKind::Write,
                },
                AccessKind::Write,
                vec![
                    (AccessKind::Read, 0x1008, 8),
                    (AccessKind::Write, 0x1010, 8),
                ],
            ),
        ] {
            let mut state = LongModeState {
                gpr: [0x55; 16],
                rflags: crate::CF | crate::ZF,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0x1000,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = 0x1008;
            state.gpr[Register::Rsp as usize] = 0x1018;
            let before = state.clone();
            let mut memory = Memory::at(&[0xff, 0x10], 0x1000);
            memory.bytes[8..16].copy_from_slice(&target.to_le_bytes());
            memory.error = Some(error);
            memory.error_kind = Some(kind);
            let result = step(&mut state, &mut memory, AccessContext::default());
            assert_eq!(result, Err(memory.error.clone().unwrap()));
            assert_eq!(state, before);
            assert_eq!(memory.accesses, expected_accesses);
            assert_eq!(memory.bytes[16..24], (16..24).collect::<Vec<_>>());
        }

        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1008;
        state.gpr[Register::Rsp as usize] = 0x1018;
        let before = state.clone();
        let mut memory = Memory::at(&[0xff, 0x10], 0x1000);
        memory.bytes[8..16].copy_from_slice(&0x0000_8000_0000_0000u64.to_le_bytes());
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn popfq_updates_only_profile_flags_and_commits_after_stack_read() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: (1 << 9) | (1 << 16) | (1 << 21) | 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1008;
        let mut memory = Memory::at(&[0x9d], 0x1000);
        // IF=0 and IOPL=3 in the stack value are ignored at CPL3/IOPL0.
        let value = crate::POPFQ_INTEGER_FLAGS | (3 << 12);
        memory.bytes[8..16].copy_from_slice(&value.to_le_bytes());

        let mut defined = crate::INTEGER_FLAGS & !crate::AF;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.rip, 0x1001);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x1010);
        assert_eq!(
            state.rflags,
            crate::POPFQ_INTEGER_FLAGS | (1 << 9) | (1 << 16) | (1 << 21) | 2
        );
        assert_eq!(defined, crate::INTEGER_FLAGS);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn popfq_stack_fault_and_out_of_profile_event_flags_do_not_commit() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: (1 << 9) | 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1008;
        let before = state.clone();
        let mut memory = Memory::at(&[0x9d], 0x1000);
        memory.error = Some(MemoryError::Unmapped {
            address: 0x1008,
            access: AccessKind::Read,
        });
        memory.error_kind = Some(AccessKind::Read);
        let mut defined = crate::INTEGER_FLAGS & !crate::AF;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Err(MemoryError::Unmapped {
                address: 0x1008,
                access: AccessKind::Read,
            })
        );
        assert_eq!(state, before);
        assert_eq!(defined, crate::INTEGER_FLAGS & !crate::AF);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);

        for out_of_profile in [1_u64 << 8, 1_u64 << 14, 1_u64 << 18] {
            let mut state = before.clone();
            let mut memory = Memory::at(&[0x9d], 0x1000);
            memory.bytes[8..16].copy_from_slice(&out_of_profile.to_le_bytes());
            let mut defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut defined,
                ),
                Ok(StepOutcome::Unsupported { opcode: 0x9d })
            );
            assert_eq!(state, before);
            assert_eq!(defined, crate::INTEGER_FLAGS);
            assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);
        }

        let mut state = before.clone();
        state.rflags |= 1 << 14;
        let initial_nt = state.clone();
        let mut memory = Memory::at(&[0x9d], 0x1000);
        let mut defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Unsupported { opcode: 0x9d })
        );
        assert_eq!(state, initial_nt);
        assert_eq!(defined, crate::INTEGER_FLAGS);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1008, 8)]);

        let mut state = before.clone();
        let mut memory = Memory::at(&[0x9d], 0x1000);
        let mut defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext {
                    mode: crate::memory::InstructionMode::Long64,
                    privilege: crate::memory::PrivilegeLevel::Ring0,
                    origin: crate::memory::AccessOrigin::Explicit,
                },
                &mut defined,
            ),
            Ok(StepOutcome::Unsupported { opcode: 0x9d })
        );
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());
    }

    #[test]
    fn pushfq_stores_the_sdm_mask_and_preserves_flags() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: u64::MAX,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1010;
        let mut memory = Memory::at(&[0x9c], 0x1000);

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.rip, 0x1001);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x1008);
        assert_eq!(state.rflags, u64::MAX);
        assert_eq!(
            u64::from_le_bytes(memory.bytes[8..16].try_into().unwrap()),
            0x00fc_ffff
        );
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x1008, 8)]);
    }

    #[test]
    fn pushfq_bounded_write_failure_does_not_commit_cpu_state() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | (1 << 10) | 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1010;
        let before = state.clone();
        let mut memory = Memory::at(&[0x9c], 0x1000);
        memory.error = Some(MemoryError::Unmapped {
            address: 0x1008,
            access: AccessKind::Write,
        });
        memory.error_kind = Some(AccessKind::Write);

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 0x1008,
                access: AccessKind::Write,
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x1008, 8)]);
    }

    #[test]
    fn pushfw_stores_the_low_flags_word_and_preserves_flags() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: 0xffff_ffff_ffff_04d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1010;
        let mut memory = Memory::at(&[0x66, 0x9c], 0x1000);

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.rip, 0x1002);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x100e);
        assert_eq!(state.rflags, 0xffff_ffff_ffff_04d7);
        assert_eq!(&memory.bytes[14..16], &[0xd7, 0x04]);
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x100e, 2)]);
    }

    #[test]
    fn pushfw_bounded_write_failure_does_not_commit_cpu_state() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | (1 << 10) | 2,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x1000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1010;
        let before = state.clone();
        let mut memory = Memory::at(&[0x66, 0x9c], 0x1000);
        memory.error = Some(MemoryError::Unmapped {
            address: 0x100e,
            access: AccessKind::Write,
        });
        memory.error_kind = Some(AccessKind::Write);

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 0x100e,
                access: AccessKind::Write,
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x100e, 2)]);
    }

    #[test]
    fn ret_imm16_reads_qword_then_commits_pop_and_zero_extended_adjustment() {
        for (immediate, expected_rsp) in [(0u16, 0x1008u64), (0x20, 0x1028), (u16::MAX, 0x1_1007)] {
            let mut state = LongModeState {
                gpr: [0x55; 16],
                rflags: crate::CF | crate::ZF,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0x2000,

                x87: None,
            };
            state.gpr[Register::Rsp as usize] = 0x1000;
            let mut memory = Memory::at(&[0xc2, immediate as u8, (immediate >> 8) as u8], 0x2000);
            let target = 0x2003u64;
            memory.bytes[..8].copy_from_slice(&target.to_le_bytes());
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Branched)
            );
            assert_eq!(state.rip, target);
            assert_eq!(state.gpr[Register::Rsp as usize], expected_rsp);
            assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);
        }
    }

    #[test]
    fn ret_imm16_truncation_stack_read_and_noncanonical_target_do_not_commit() {
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::CF | crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0x2000,

            x87: None,
        };
        state.gpr[Register::Rsp as usize] = 0x1000;
        let before = state.clone();
        let mut memory = Memory::at(&[0xc2, 0x20], 0x2000);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 0x2002,
                access: AccessKind::Fetch,
            })
        );
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());

        let mut memory = Memory::at(&[0xc2, 0x20, 0], 0x2000);
        memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(5),
            linear_address: Some(0x1000),
        }));
        memory.error_kind = Some(AccessKind::Read);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(5),
                linear_address: Some(0x1000),
            }))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);

        let mut memory = Memory::at(&[0xc2, 0x20, 0], 0x2000);
        memory.bytes[..8].copy_from_slice(&0x0000_8000_0000_0000u64.to_le_bytes());
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);
    }

    #[test]
    fn direct_branch_wraps_rip_and_taken_noncanonical_target_faults_without_commit() {
        let base = u64::MAX - 1;
        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: base,

            x87: None,
        };
        let mut memory = Memory::at(&[0x74, 0x01], base); // next RIP wraps to zero, target one
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Branched)
        );
        assert_eq!(state.rip, 1);

        let base = 0x0000_7fff_ffff_fffd;
        let before = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: base,

            x87: None,
        };
        let mut state = before.clone();
        let mut defined = 0x8d7;
        let mut memory = Memory::at(&[0x74, 0x01], base); // target crosses canonical boundary
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined
            ),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            }))
        );
        assert_eq!(state, before);
        assert_eq!(defined, 0x8d7);
    }

    #[test]
    fn direct_control_flow_rejects_unsupported_prefixes_and_preserves_state_on_fetch_fault() {
        for code in [
            &[0x66, 0x74, 0][..],
            &[0xf2, 0x75, 0][..],
            &[0xf3, 0xeb, 0][..],
        ] {
            let mut state = LongModeState {
                gpr: [0x55; 16],
                rflags: crate::ZF,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0,

                x87: None,
            };
            let before = state.clone();
            let mut memory = Memory::new(code);
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { .. })
            ));
            assert_eq!(state, before);
        }

        let mut state = LongModeState {
            gpr: [0x55; 16],
            rflags: crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        let before = state.clone();
        let mut memory = Memory::new(&[0x0f, 0x84, 0, 0, 0, 0]);
        memory.fetch_error = Some((
            5,
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(16),
                linear_address: Some(5),
            }),
        ));
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(16),
                linear_address: Some(5),
            }))
        );
        assert_eq!(state, before);
    }

    #[test]
    fn setcc_updates_only_the_selected_byte_alias_and_preserves_flags() {
        for (code, register, expected) in [
            (
                &[0x0f, 0x94, 0xc4][..],
                Register::Rax,
                0x1122_3344_5566_01_88,
            ), // AH
            (
                &[0x40, 0x0f, 0x94, 0xc4][..],
                Register::Rsp,
                0x1122_3344_5566_7701,
            ), // REX selects SPL
            (
                &[0x41, 0x0f, 0x95, 0xc7][..],
                Register::R15,
                0x1122_3344_5566_7700,
            ), // REX.B selects R15B; false condition writes zero
            (
                &[0x66, 0x0f, 0x94, 0xc0][..],
                Register::Rax,
                0x1122_3344_5566_7701,
            ), // operand-size override does not widen SETcc
            (
                &[0x44, 0x0f, 0x94, 0xd0][..],
                Register::Rax,
                0x1122_3344_5566_7701,
            ), // REX.R / ignored ModRM.reg does not change AL destination
        ] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rflags: crate::ZF | crate::CF | crate::OF | crate::PF,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0,

                x87: None,
            };
            if register == Register::R15 {
                state.gpr[Register::R15 as usize] = 0x1122_3344_5566_77ff;
            }
            let flags = state.rflags;
            let mut defined = crate::INTEGER_FLAGS;
            let mut memory = Memory::new(code);
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut defined,
                ),
                Ok(StepOutcome::Advanced),
                "{code:02x?}"
            );
            assert_eq!(state.gpr[register as usize], expected, "{code:02x?}");
            assert_eq!(state.rflags, flags, "{code:02x?}");
            assert_eq!(defined, crate::INTEGER_FLAGS, "{code:02x?}");
            assert_eq!(state.rip, code.len() as u64, "{code:02x?}");
        }
    }

    #[test]
    fn setcc_memory_store_is_one_byte_and_faults_do_not_commit() {
        let code = [0x0f, 0x94, 0x45, 0x00]; // SETZ byte ptr [RBP]
        let mut state = LongModeState {
            gpr: [0; 16],
            rflags: crate::ZF | crate::PF | crate::OF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbp as usize] = 0x1000;
        let before = state.clone();
        let mut defined = crate::INTEGER_FLAGS;
        let mut memory = Memory::new(&code);
        let before_ram = memory.bytes.clone();
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.rip, 4);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(defined, crate::INTEGER_FLAGS);
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x1000, 1)]);
        assert_eq!(memory.bytes[0], 1);
        assert_eq!(memory.bytes[1..], before_ram[1..]);

        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(2),
            linear_address: Some(0x1000),
        });
        let mut state = before.clone();
        let mut defined = 0x8d7;
        let mut memory = Memory::new(&code);
        memory.error = Some(fault.clone());
        memory.error_kind = Some(AccessKind::Write);
        let before_ram = memory.bytes.clone();
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Fault(match fault {
                MemoryError::Architectural(fault) => fault,
                _ => unreachable!(),
            }))
        );
        assert_eq!(state, before);
        assert_eq!(defined, 0x8d7);
        assert_eq!(memory.bytes, before_ram);
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x1000, 1)]);

        let mut state = before.clone();
        state.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let result = step(&mut state, &mut memory, AccessContext::default());
        assert!(
            matches!(
                result,
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 12, .. }))
            ),
            "{result:?}"
        );
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());
    }

    #[test]
    fn setcc_accepts_address_override_and_rejects_unsupported_repeat_or_segment_forms() {
        let mut state = LongModeState {
            gpr: [0; 16],
            rflags: crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbx as usize] = 0x1_0000_1000;
        let mut memory = Memory::new(&[0x67, 0x0f, 0x94, 0x03]); // [EBX]
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.accesses, [(AccessKind::Write, 0x1000, 1)]);
        assert_eq!(memory.bytes[0], 1);

        for code in [
            &[0xf2, 0x0f, 0x94, 0xc0][..],
            &[0xf3, 0x0f, 0x94, 0xc0][..],
            &[0x64, 0x0f, 0x94, 0x00][..],
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rflags: crate::ZF,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0,

                x87: None,
            };
            let before = state.clone();
            let mut memory = Memory::new(code);
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { .. })
            ));
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn cmovcc_reads_memory_even_when_false_and_commits_only_after_success() {
        let code = [0x0f, 0x44, 0x03]; // CMOVE EAX, dword ptr [RBX]
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rflags: crate::CF | crate::OF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbx as usize] = 0x1000;
        let flags = state.rflags;
        let mut defined = crate::INTEGER_FLAGS;
        let mut memory = Memory::new(&code);
        memory.bytes[..4].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);
        assert_eq!(state.gpr[Register::Rax as usize], 0x5566_7788);
        assert_eq!(state.rip, 3);
        assert_eq!(state.rflags, flags);
        assert_eq!(defined, crate::INTEGER_FLAGS);

        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rflags: crate::ZF | crate::PF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbx as usize] = 0x1000;
        let mut memory = Memory::new(&code);
        memory.bytes[..4].copy_from_slice(&[0x78, 0x56, 0x34, 0x12]);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);
        assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678); // 32-bit write zero extends
        assert_eq!(state.rflags, crate::ZF | crate::PF);
        assert_eq!(state.rip, 3);
    }

    #[test]
    fn cmovcc_false_memory_read_faults_before_any_architectural_commit() {
        let code = [0x0f, 0x44, 0x45, 0x00]; // CMOVE EAX, dword ptr [RBP]
        let mut state = LongModeState {
            gpr: [0x8877_6655_4433_2211; 16],
            rflags: crate::CF | crate::PF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbp as usize] = 0x1000;
        let before = state.clone();
        let mut defined = crate::INTEGER_FLAGS;
        let mut memory = Memory::new(&code);
        let fault = ArchitecturalFault {
            vector: 14,
            error_code: Some(4),
            linear_address: Some(0x1000),
        };
        memory.error = Some(MemoryError::Architectural(fault.clone()));
        memory.error_kind = Some(AccessKind::Read);
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Fault(fault))
        );
        assert_eq!(state, before);
        assert_eq!(defined, crate::INTEGER_FLAGS);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);

        // A noncanonical false-condition source faults architecturally before
        // the memory backend sees a data read, and still leaves RIP/state alone.
        let mut state = before.clone();
        state.gpr[Register::Rbp as usize] = 0x0000_8000_0000_0000;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let mut defined = crate::INTEGER_FLAGS;
        assert!(matches!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut defined,
            ),
            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 12, .. }))
        ));
        assert_eq!(state, before);
        assert_eq!(defined, crate::INTEGER_FLAGS);
        assert!(memory.accesses.is_empty());
    }

    #[test]
    fn cmovcc_accepts_operand_and_address_overrides_and_rejects_group1_and_fs_gs_memory() {
        let mut state = LongModeState {
            gpr: [0; 16],
            rflags: crate::ZF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[Register::Rbx as usize] = 0x1_0000_1000;
        let mut memory = Memory::new(&[0x67, 0x66, 0x0f, 0x44, 0x03]); // CMOVE AX, [EBX]
        memory.bytes[..2].copy_from_slice(&[0x34, 0x12]);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 2)]);
        assert_eq!(state.gpr[Register::Rax as usize], 0x1234);

        for code in [
            &[0xf2, 0x0f, 0x44, 0xc0][..],
            &[0xf3, 0x0f, 0x44, 0xc0][..],
            &[0x64, 0x0f, 0x44, 0x00][..],
            &[0x65, 0x0f, 0x44, 0x00][..],
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rflags: 0,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                rip: 0,

                x87: None,
            };
            let before = state.clone();
            let mut memory = Memory::new(code);
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { .. })
            ));
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn mov_load_store_widths_all_registers_preserve_flags_and_other_bytes() {
        for size in [
            OperandSize::Byte,
            OperandSize::Word,
            OperandSize::Dword,
            OperandSize::Qword,
        ] {
            for encoding in 0..16u8 {
                for store in [false, true] {
                    let mut code = vec![];
                    if size == OperandSize::Word {
                        code.push(0x66);
                    }
                    // Explicit REX chooses SPL/BPL/SIL/DIL for byte encodings4..7.
                    code.push(
                        0x40 | if size == OperandSize::Qword { 8 } else { 0 }
                            | ((encoding >> 3) << 2),
                    );
                    code.push(match (store, size == OperandSize::Byte) {
                        (false, true) => 0x8a,
                        (false, false) => 0x8b,
                        (true, true) => 0x88,
                        (true, false) => 0x89,
                    });
                    code.extend([0x40 | ((encoding & 7) << 3) | 3, 1]); // [RBX+1]
                    let mut memory = Memory::new(&code);
                    let before_ram = memory.bytes.clone();
                    let mut state = LongModeState {
                        gpr: [0x8877_6655_4433_2211; 16],
                        rflags: 0x8d7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        gdtr: None,
                        fs_cache: None,
                        gs_cache: None,
                        rip: 0,

                        x87: None,
                    };
                    state.gpr[3] = 0x1000;
                    let before = state.clone();
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    let register = Register::from_encoding(encoding).unwrap();
                    if store {
                        let expected = before.read(crate::RegisterView::low(register, size));
                        assert_eq!(
                            &memory.bytes[1..1 + size.bytes()],
                            &expected.to_le_bytes()[..size.bytes()]
                        );
                        assert_eq!(&memory.bytes[..1], &before_ram[..1]);
                        assert_eq!(
                            &memory.bytes[1 + size.bytes()..],
                            &before_ram[1 + size.bytes()..]
                        );
                        assert_eq!(state.gpr, before.gpr);
                    } else {
                        let value = 0x0807_0605_0403_0201 & size.mask();
                        let expected = match size {
                            OperandSize::Byte => (before.gpr[encoding as usize] & !0xff) | value,
                            OperandSize::Word => (before.gpr[encoding as usize] & !0xffff) | value,
                            OperandSize::Dword | OperandSize::Qword => value,
                        };
                        assert_eq!(state.gpr[encoding as usize], expected);
                        let mut expected_gpr = before.gpr;
                        expected_gpr[encoding as usize] = expected;
                        assert_eq!(state.gpr, expected_gpr);
                        assert_eq!(memory.bytes, before_ram);
                    }
                    assert_eq!(state.rflags, before.rflags);
                    assert_eq!(state.rip, code.len() as u64);
                    assert_eq!(
                        memory.accesses,
                        [(
                            if store {
                                AccessKind::Write
                            } else {
                                AccessKind::Read
                            },
                            0x1001,
                            size.bytes()
                        )]
                    );
                }
            }
        }
    }

    #[test]
    fn high_byte_register_mov_load_and_store_use_legacy_aliases() {
        let mut state = LongModeState {
            gpr: [0; 16],
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            rip: 0,

            x87: None,
        };
        state.gpr[0] = 0x1122_3344_5566_7788;
        state.gpr[3] = 0x1000;
        let mut memory = Memory::new(&[0x8a, 0x23]); // AH <- [RBX]
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[0], 0x1122_3344_5566_0088);
        state.rip = 0;
        let mut memory = Memory::new(&[0x88, 0x63, 1]); // [RBX+1] <- AH
        state.gpr[0] = 0x1122_3344_5566_aa88;
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.bytes[1], 0xaa);
        assert_eq!(state.rflags, 0x8d7);
    }

    #[test]
    fn immediate_and_register_mov_apply_all_alias_widths_without_flags_changes() {
        for size in [
            OperandSize::Byte,
            OperandSize::Word,
            OperandSize::Dword,
            OperandSize::Qword,
        ] {
            for encoding in 0..16u8 {
                let mut code = vec![];
                if size == OperandSize::Word {
                    code.push(0x66);
                }
                code.push(0x40 | if size == OperandSize::Qword { 8 } else { 0 } | (encoding >> 3));
                code.push(
                    (if size == OperandSize::Byte {
                        0xb0
                    } else {
                        0xb8
                    }) | (encoding & 7),
                );
                code.extend_from_slice(&0x8877_6655_4433_2211u64.to_le_bytes()[..size.bytes()]);
                let mut state = LongModeState {
                    gpr: [u64::MAX; 16],
                    rflags: 0x8d7,
                    fs_base: 0,
                    gs_base: 0,
                    fs_selector: 0,
                    gs_selector: 0,
                    gdtr: None,
                    fs_cache: None,
                    gs_cache: None,
                    rip: 0,

                    x87: None,
                };
                let mut memory = Memory::new(&code);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced)
                );
                let expected = match size {
                    OperandSize::Byte => 0xffff_ffff_ffff_ff11,
                    OperandSize::Word => 0xffff_ffff_ffff_2211,
                    OperandSize::Dword => 0x4433_2211,
                    OperandSize::Qword => 0x8877_6655_4433_2211,
                };
                assert_eq!(state.gpr[encoding as usize], expected);
                assert_eq!(state.rflags, 0x8d7);
                assert!(memory.accesses.is_empty());
                // Copy the just-loaded alias to RAX; high-byte forms have their own test.
                let mut code = vec![];
                if size == OperandSize::Word {
                    code.push(0x66);
                }
                code.extend([
                    0x40 | if size == OperandSize::Qword { 8 } else { 0 } | (encoding >> 3),
                    if size == OperandSize::Byte {
                        0x8a
                    } else {
                        0x8b
                    },
                    0xc0 | (encoding & 7),
                ]);
                state.rip = 0;
                let mut memory = Memory::new(&code);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced)
                );
                assert_eq!(
                    state.read(crate::RegisterView::low(Register::Rax, size)),
                    expected & size.mask()
                );
                assert_eq!(state.rflags, 0x8d7);
            }
        }
    }

    #[test]
    fn multiply_forms_write_the_correct_halves_and_truncated_result() {
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1122_3344_5566_00f0;
        state.gpr[Register::Rbx as usize] = 2;
        let mut memory = Memory::new(&[0xf6, 0xe3]); // MUL BL
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_01e0);
        assert_eq!(state.gpr[Register::Rdx as usize], 0);
        assert_eq!(
            state.rflags & (crate::CF | crate::OF),
            crate::CF | crate::OF
        );
        assert_eq!(
            state.rflags & !(crate::CF | crate::OF),
            0x8d7 & !(crate::CF | crate::OF)
        );

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 0x1122_3344_5566_77fe;
        state.gpr[Register::Rbx as usize] = 3;
        let mut memory = Memory::new(&[0xf6, 0xeb]); // IMUL BL
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_fffa);
        assert_eq!(state.rflags & (crate::CF | crate::OF), 0);

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 0x8000_0000_0000_0000;
        state.gpr[Register::Rbx as usize] = 2;
        let mut memory = Memory::new(&[0x48, 0xf7, 0xe3]); // MUL RBX
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0);
        assert_eq!(state.gpr[Register::Rdx as usize], 1);

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = (-7i64) as u64;
        state.gpr[Register::Rbx as usize] = 3;
        let mut memory = Memory::new(&[0x48, 0x0f, 0xaf, 0xc3]); // IMUL RAX, RBX
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], (-21i64) as u64);
        assert_eq!(state.rflags & (crate::CF | crate::OF), 0);

        let mut state = LongModeState::default();
        state.gpr[Register::Rbx as usize] = 3;
        let mut memory = Memory::new(&[0x6b, 0xc3, 0x80]); // IMUL EAX, EBX, -128
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], (-384i32) as u32 as u64);
        assert_eq!(state.rflags & (crate::CF | crate::OF), 0);
    }

    #[test]
    fn sign_extension_accumulator_forms_preserve_flags_and_apply_alias_writes() {
        let cases = [
            (
                &[0x66, 0x98][..],
                Register::Rax,
                0x1122_3344_5566_7780,
                0,
                0x1122_3344_5566_ff80,
            ), // CBW: AX = sign_extend(AL)
            (
                &[0x98][..],
                Register::Rax,
                0x1122_3344_0000_8001,
                0,
                0x0000_0000_ffff_8001,
            ), // CWDE: EAX = sign_extend(AX), then zero-extend to RAX
            (
                &[0x48, 0x98][..],
                Register::Rax,
                0x0000_0000_8000_0001,
                0,
                0xffff_ffff_8000_0001,
            ), // CDQE
            (
                &[0x66, 0x99][..],
                Register::Rdx,
                0x1122_3344_5566_8001,
                0xaabb_ccdd_eeff_1122,
                0xaabb_ccdd_eeff_ffff,
            ), // CWD: DX = sign mask of AX
            (
                &[0x99][..],
                Register::Rdx,
                0x1122_3344_8000_0001,
                0xaabb_ccdd_eeff_1122,
                0x0000_0000_ffff_ffff,
            ), // CDQ: EDX write zero-extends
            (
                &[0x48, 0x99][..],
                Register::Rdx,
                0x8000_0000_0000_0001,
                0xaabb_ccdd_eeff_1122,
                u64::MAX,
            ), // CQO
        ];

        for (code, target, initial_rax, initial_target, expected) in cases {
            let mut state = LongModeState {
                gpr: [0xdead_beef_cafe_babe; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = initial_rax;
            if target != Register::Rax {
                state.gpr[target as usize] = initial_target;
            }
            let before_gprs = state.gpr;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[target as usize], expected);
            assert_eq!(state.rip, code.len() as u64);
            assert_eq!(state.rflags, 0x8d7);
            for index in 0..16 {
                if index != target as usize {
                    assert_eq!(state.gpr[index], before_gprs[index]);
                }
            }
            assert!(memory.accesses.is_empty());
        }

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 0x80;
        state.rflags = 0x8d7;
        let result = run(
            &mut state,
            &mut Memory::new(&[0x66, 0x98]),
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(2),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.rflags_defined, crate::INTEGER_FLAGS);
    }

    #[test]
    fn divide_uses_ax_for_byte_form_and_signed_remainder_truncates_toward_zero() {
        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 0x1122_3344_5566_0100;
        state.gpr[Register::Rbx as usize] = 3;
        let mut memory = Memory::new(&[0xf6, 0xf3]); // DIV BL: AX / BL
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_0155);

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = (-7i64) as u64;
        state.gpr[Register::Rdx as usize] = u64::MAX;
        state.gpr[Register::Rcx as usize] = 3;
        let mut memory = Memory::new(&[0x48, 0xf7, 0xf9]); // IDIV RCX
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], (-2i64) as u64);
        assert_eq!(state.gpr[Register::Rdx as usize], (-1i64) as u64);
    }

    #[test]
    fn multiply_and_divide_update_only_architecturally_defined_flag_metadata() {
        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 3;
        state.gpr[Register::Rbx as usize] = 4;
        let result = run(
            &mut state,
            &mut Memory::new(&[0xf6, 0xe3]), // MUL BL
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(2),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(
            result.rflags_defined,
            (crate::INTEGER_FLAGS | crate::CF | crate::OF)
                & !(crate::PF | crate::AF | crate::ZF | crate::SF)
        );

        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 100;
        state.gpr[Register::Rbx as usize] = 3;
        let result = run(
            &mut state,
            &mut Memory::new(&[0xf6, 0xf3]), // DIV BL
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(2),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(
            result.rflags_defined,
            crate::INTEGER_FLAGS & !crate::ARITHMETIC_FLAGS
        );
    }

    #[test]
    fn divide_errors_preserve_exact_state_and_report_de_before_retirement() {
        let fault = StepOutcome::Fault(ArchitecturalFault {
            vector: 0,
            error_code: None,
            linear_address: None,
        });
        // AX / BL produces a quotient above AL's representable range.
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x100;
        state.gpr[Register::Rbx as usize] = 1;
        let before = state.clone();
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0xf6, 0xf3]),
                AccessContext::default()
            ),
            Ok(fault.clone())
        );
        assert_eq!(state, before);

        // The byte signed minimum divided by -1 has a quotient outside i8.
        let mut state = LongModeState {
            gpr: [0x7654_3210_fedc_ba98; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0xff80; // signed AX dividend = -128
        state.gpr[Register::Rbx as usize] = 0xff; // signed BL divisor = -1
        let before = state.clone();
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0xf6, 0xfb]),
                AccessContext::default()
            ),
            Ok(fault.clone())
        );
        assert_eq!(state, before);

        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x100;
        state.gpr[Register::Rbx as usize] = 1;
        let result = run(
            &mut state,
            &mut Memory::new(&[0xf6, 0xf3]),
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(match &fault {
                StepOutcome::Fault(fault) => fault.clone(),
                _ => unreachable!(),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, crate::INTEGER_FLAGS);

        // This exact case exercises signed 128-bit minimum divided by -1.
        let mut state = LongModeState {
            gpr: [0x8877_6655_4433_2211; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0;
        state.gpr[Register::Rdx as usize] = 0x8000_0000_0000_0000;
        state.gpr[Register::Rcx as usize] = u64::MAX;
        let before = state.clone();
        let mut memory = Memory::new(&[0x48, 0xf7, 0xf9]);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(fault.clone())
        );
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());

        // Zero divisor has the same architectural exception and exact RIP.
        let mut state = LongModeState {
            gpr: [0x1234; 16],
            rip: 0,
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rcx as usize] = 0;
        let before = state.clone();
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0x48, 0xf7, 0xf9]),
                AccessContext::default()
            ),
            Ok(fault)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn memory_divisor_read_fault_does_not_commit_cpu_or_write_memory() {
        let mut state = LongModeState::default();
        state.gpr[Register::Rax as usize] = 100;
        state.gpr[Register::Rbx as usize] = 0x1000;
        let before = state.clone();
        let mut memory = Memory::new(&[0xf7, 0x33]); // DIV dword [RBX]
        memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(0x1000),
        }));
        memory.error_kind = Some(AccessKind::Read);
        let before_ram = memory.bytes.clone();
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x1000),
            }))
        );
        assert_eq!(state, before);
        assert_eq!(memory.bytes, before_ram);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);
    }

    #[test]
    fn memory_bit_scan_read_fault_does_not_commit_cpu_or_definedness() {
        let mut state = LongModeState {
            gpr: [0x1234_5678_9abc_def0; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rbx as usize] = 0x1000;
        let before = state.clone();
        let mut memory = Memory::new(&[0x48, 0x0f, 0xbc, 0x0b]); // BSF RCX,[RBX]
        memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(7),
            linear_address: Some(0x1000),
        }));
        memory.error_kind = Some(AccessKind::Read);
        let before_ram = memory.bytes.clone();
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x1000),
            })
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(result.rflags_defined, crate::INTEGER_FLAGS);
        assert_eq!(state, before);
        assert_eq!(memory.bytes, before_ram);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);
    }

    #[test]
    fn data_fault_or_backend_error_never_commits_cpu_or_ram() {
        for store in [false, true] {
            for error in [
                MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    linear_address: Some(0x1001),
                }),
                MemoryError::Backend("injected mapping failure".into()),
                MemoryError::PermissionDenied {
                    address: 0x1001,
                    access: if store {
                        AccessKind::Write
                    } else {
                        AccessKind::Read
                    },
                },
                MemoryError::Unmapped {
                    address: 0x1001,
                    access: if store {
                        AccessKind::Write
                    } else {
                        AccessKind::Read
                    },
                },
            ] {
                let mut memory = Memory::new(&[0x48, if store { 0x89 } else { 0x8b }, 0x43, 1]);
                memory.error = Some(error.clone());
                let before_ram = memory.bytes.clone();
                let mut state = LongModeState {
                    gpr: [0x1234; 16],
                    rip: 0,
                    rflags: 0x8d7,
                    fs_base: 0,
                    gs_base: 0,
                    fs_selector: 0,
                    gs_selector: 0,
                    gdtr: None,
                    fs_cache: None,
                    gs_cache: None,

                    x87: None,
                };
                state.gpr[3] = 0x1000;
                let before = state.clone();
                let expected = match error {
                    MemoryError::Architectural(fault) => Ok(StepOutcome::Fault(fault)),
                    error => Err(error),
                };
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    expected
                );
                assert_eq!(state, before);
                assert_eq!(memory.bytes, before_ram);
            }
        }
    }

    #[test]
    fn canonical_access_checks_every_byte_and_overflow_before_backend_calls() {
        for (base_encoding, modrm, suffix, vector) in
            [(0, 0u8, &[][..], 13), (5, 0x45, &[0][..], 12)]
        {
            for size in [
                OperandSize::Byte,
                OperandSize::Word,
                OperandSize::Dword,
                OperandSize::Qword,
            ] {
                for store in [false, true] {
                    for boundary in [0x7fff_ffff_ffffu64, u64::MAX] {
                        for crosses in [false, true] {
                            // A one-byte access starting beyond MAX cannot be represented.
                            if crosses && size == OperandSize::Byte && boundary == u64::MAX {
                                continue;
                            }
                            let start = if crosses {
                                boundary - size.bytes() as u64 + 2
                            } else {
                                boundary - size.bytes() as u64 + 1
                            };
                            let mut code = vec![];
                            if size == OperandSize::Word {
                                code.push(0x66);
                            }
                            if size == OperandSize::Qword {
                                code.push(0x48);
                            }
                            code.extend([
                                match (store, size == OperandSize::Byte) {
                                    (false, true) => 0x8a,
                                    (false, false) => 0x8b,
                                    (true, true) => 0x88,
                                    (true, false) => 0x89,
                                },
                                modrm,
                            ]);
                            code.extend(suffix);
                            let mut memory = Memory::new(&code);
                            let mut state = LongModeState::default();
                            state.gpr[base_encoding] = start;
                            let before = state.clone();
                            let result = step(&mut state, &mut memory, AccessContext::default());
                            if crosses && boundary == u64::MAX {
                                assert_eq!(
                                    result,
                                    Err(MemoryError::AddressOverflow {
                                        address: start,
                                        length: size.bytes()
                                    })
                                );
                                assert!(memory.accesses.is_empty());
                            } else if crosses {
                                assert_eq!(
                                    result,
                                    Ok(StepOutcome::Fault(ArchitecturalFault {
                                        vector,
                                        error_code: Some(0),
                                        linear_address: None
                                    })),
                                    "{code:02x?} {start:x}"
                                );
                                assert!(memory.accesses.is_empty());
                            } else {
                                assert!(
                                    matches!(result, Err(MemoryError::Unmapped { .. })),
                                    "{code:02x?} {start:x}"
                                );
                                assert_eq!(memory.accesses.len(), 1);
                            }
                            assert_eq!(state, before);
                        }
                    }
                }
            }
        }
        // 67 wraps only the initial effective offset. The qword data range
        // beginning at FFFFFFFC crosses 4GiB linearly, rather than wrapping32.
        let mut memory = Memory::new(&[0x67, 0x48, 0x8b, 0x00]);
        let mut state = LongModeState::default();
        state.gpr[0] = 0x1_ffff_fffc;
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(memory.accesses, [(AccessKind::Read, 0xffff_fffc, 8)]);
    }

    #[test]
    fn immediate_rm_mov_obeys_widths_sign_extension_and_rip_after_immediate() {
        for (code, expected) in [
            (&[0xc6, 0xc4, 0x80][..], 0x1122_3344_5566_8088),
            (&[0x66, 0xc7, 0xc0, 0xff, 0x7f], 0x1122_3344_5566_7fff),
            (&[0xc7, 0xc0, 0xff, 0xff, 0xff, 0xff], 0xffff_ffff),
            (&[0x48, 0xc7, 0xc0, 0, 0, 0, 0x80], 0xffff_ffff_8000_0000),
            (&[0x4c, 0xc7, 0xc0, 1, 0, 0, 0], 1), // REX.R ignored for /0
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x1122_3344_5566_7788;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected);
            assert_eq!(state.rflags, 0x8d7);
        }
        for (code, expected) in [
            (&[0xc6, 0x43, 1, 0x80][..], vec![0x80]),
            (&[0x66, 0xc7, 0x43, 1, 0, 0x80], vec![0, 0x80]),
            (&[0xc7, 0x43, 1, 1, 0, 0, 0x80], vec![1, 0, 0, 0x80]),
            (
                &[0x48, 0xc7, 0x43, 1, 1, 0, 0, 0x80],
                vec![1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff],
            ),
            // Next RIP is11 includingimm32:11+0xFF6=0x1001.
            (
                &[0x48, 0xc7, 0x05, 0xf6, 0x0f, 0, 0, 1, 0, 0, 0x80],
                vec![1, 0, 0, 0x80, 0xff, 0xff, 0xff, 0xff],
            ),
        ] {
            let mut state = LongModeState::default();
            state.gpr[3] = 0x1000;
            let before = state.gpr;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(&memory.bytes[1..1 + expected.len()], expected);
            assert_eq!(state.gpr, before);
            assert_eq!(state.rip, code.len() as u64);
        }
    }

    #[test]
    fn moffs_load_store_use_address_width_and_accumulator_alias() {
        for addr32 in [false, true] {
            for size in [
                OperandSize::Byte,
                OperandSize::Word,
                OperandSize::Dword,
                OperandSize::Qword,
            ] {
                for store in [false, true] {
                    let mut code = vec![];
                    if addr32 {
                        code.push(0x67);
                    }
                    if size == OperandSize::Word {
                        code.push(0x66);
                    }
                    if size == OperandSize::Qword {
                        code.push(0x48);
                    }
                    code.push(
                        0xa0 | if size == OperandSize::Byte { 0 } else { 1 }
                            | if store { 2 } else { 0 },
                    );
                    code.extend_from_slice(&0x1001u64.to_le_bytes()[..if addr32 { 4 } else { 8 }]);
                    let mut state = LongModeState {
                        gpr: [0; 16],
                        rip: 0,
                        rflags: 0x8d7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        gdtr: None,
                        fs_cache: None,
                        gs_cache: None,

                        x87: None,
                    };
                    state.gpr[0] = 0x8877_6655_4433_2211;
                    let mut memory = Memory::new(&code);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    if store {
                        assert_eq!(
                            &memory.bytes[1..1 + size.bytes()],
                            &0x8877_6655_4433_2211u64.to_le_bytes()[..size.bytes()]
                        );
                    } else {
                        let expected = match size {
                            OperandSize::Byte => 0x8877_6655_4433_2201,
                            OperandSize::Word => 0x8877_6655_4433_0201,
                            OperandSize::Dword => 0x0403_0201,
                            OperandSize::Qword => 0x0807_0605_0403_0201,
                        };
                        assert_eq!(state.gpr[0], expected);
                    }
                    assert_eq!(state.rflags, 0x8d7);
                    assert_eq!(
                        memory.accesses,
                        [(
                            if store {
                                AccessKind::Write
                            } else {
                                AccessKind::Read
                            },
                            0x1001,
                            size.bytes()
                        )]
                    );
                }
            }
        }
    }

    #[test]
    fn extensions_have_exact_source_and_destination_widths() {
        for (code, source, expected) in [
            (&[0x66, 0x0f, 0xb6, 0xc3][..], 0x80, 0x1122_3344_5566_0080),
            (&[0x0f, 0xb6, 0xc3], 0x80, 0x80),
            (&[0x48, 0x0f, 0xb6, 0xc3], 0x80, 0x80),
            (&[0x0f, 0xb7, 0xc3], 0x8001, 0x8001),
            (&[0x48, 0x0f, 0xb7, 0xc3], 0x8001, 0x8001),
            (&[0x66, 0x0f, 0xbe, 0xc3], 0x80, 0x1122_3344_5566_ff80),
            (&[0x0f, 0xbe, 0xc3], 0x80, 0xffff_ff80),
            (&[0x48, 0x0f, 0xbe, 0xc3], 0x80, 0xffff_ffff_ffff_ff80),
            (&[0x0f, 0xbf, 0xc3], 0x8001, 0xffff_8001),
            (&[0x48, 0x0f, 0xbf, 0xc3], 0x8001, 0xffff_ffff_ffff_8001),
            (&[0x66, 0x63, 0xc3], 0x8001, 0x1122_3344_5566_8001),
            (&[0x63, 0xc3], 0x8000_0001, 0x8000_0001),
            (&[0x48, 0x63, 0xc3], 0x8000_0001, 0xffff_ffff_8000_0001),
            (
                &[0x66, 0x48, 0x63, 0xc3],
                0x8000_0001,
                0xffff_ffff_8000_0001,
            ),
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x1122_3344_5566_7788;
            state.gpr[3] = source;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected, "{code:02x?}");
            assert_eq!(state.rflags, 0x8d7);
            assert!(memory.accesses.is_empty());
        }
        for (code, expected) in [
            (&[0x0f, 0xbe, 0xc4][..], 0xffff_ff80),
            (&[0x40, 0x0f, 0xbe, 0xc4][..], 0xffff_ff99),
        ] {
            let mut state = LongModeState::default();
            state.gpr[0] = 0x1234_8088;
            state.gpr[4] = 0x99;
            assert_eq!(
                step(&mut state, &mut Memory::new(code), AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected);
        }
    }

    #[test]
    fn lea_ignores_canonicality_segments_and_mapping_without_data_reads() {
        for (code, base, expected) in [
            (&[0x48, 0x8d, 0x03][..], 0x8000_0000_1234, 0x8000_0000_1234),
            (&[0x66, 0x8d, 0x03], 0x8000_0000_1234, 0x1122_3344_5566_1234),
            (&[0x8d, 0x03], 0x8000_9876_1234, 0x9876_1234),
            (&[0x67, 0x48, 0x8d, 0x03], 0x8000_9876_1234, 0x9876_1234),
            (
                &[0x64, 0x48, 0x8d, 0x03],
                0x8000_0000_1234,
                0x8000_0000_1234,
            ),
            (
                &[0x65, 0x48, 0x8d, 0x03],
                0x8000_0000_1234,
                0x8000_0000_1234,
            ),
            (&[0x48, 0x8d, 0x05, 0xff, 0xff, 0xff, 0xff], 0, 6),
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x1122_3344_5566_7788;
            state.gpr[3] = base;
            let mut memory = Memory::new(code);
            memory.error = Some(MemoryError::Backend(
                "LEA must never call a data backend".into(),
            ));
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected);
            assert_eq!(state.rflags, 0x8d7);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn xchg_captures_aliases_before_writes_and_distinguishes_nop_from_eax_self_write() {
        for code in [&[0x90][..], &[0x40, 0x90], &[0x48, 0x90], &[0x66, 0x90]] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            let before = state.clone();
            assert_eq!(
                step(&mut state, &mut Memory::new(code), AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
        }
        for (code, rax, other_index, other) in [
            (
                &[0x86, 0xc4][..],
                0x1122_3344_5566_8877,
                0,
                0x1122_3344_5566_8877,
            ),
            (&[0x87, 0xc0], 0x5566_7788, 0, 0x5566_7788),
            (&[0x41, 0x90], 0xeeff_0011, 8, 0x5566_7788),
            (
                &[0x49, 0x90],
                0xaabb_ccdd_eeff_0011,
                8,
                0x1122_3344_5566_7788,
            ),
            (
                &[0x66, 0x91],
                0x1122_3344_5566_0011,
                1,
                0xaabb_ccdd_eeff_7788,
            ),
        ] {
            let mut state = LongModeState {
                gpr: [0xaabb_ccdd_eeff_0011; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x1122_3344_5566_7788;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], rax, "{code:02x?}");
            assert_eq!(state.gpr[other_index], other);
            assert_eq!(state.rflags, 0x8d7);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn qword_memory_xchg_uses_one_atomic_call_and_commits_only_on_success() {
        let code = [0x48, 0x87, 0x00]; // XCHG [RAX], RAX
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0x0cd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1008;
        let mut memory = Memory::new(&code);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 3);
        assert_eq!(state.rflags, 0x0cd7);
        assert_eq!(state.gpr[Register::Rax as usize], 0x0f0e_0d0c_0b0a_0908);
        assert_eq!(
            &memory.bytes[8..16],
            &0x1008u64.to_le_bytes(),
            "the source RAX value is written using the original effective address"
        );
        assert_eq!(
            memory.atomic_exchanges,
            vec![(0x1008, OperandSize::Qword, 0x1008)]
        );
        assert!(
            memory.accesses.is_empty(),
            "no separate data read/write calls"
        );

        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(0x1008),
            }),
            MemoryError::Backend("atomic exchange failed".into()),
        ] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rip: 0,
                rflags: 0x0cd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = 0x1008;
            let before_state = state.clone();
            let mut memory = Memory::new(&code);
            memory.atomic_error = Some(injected.clone());
            let before_ram = memory.bytes.clone();
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            match injected {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                error => assert_eq!(result.exit, RunExit::MemoryError(error)),
            }
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_ram);
            assert_eq!(
                memory.atomic_exchanges,
                vec![(0x1008, OperandSize::Qword, 0x1008)]
            );
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn qword_memory_xadd_uses_one_atomic_call_and_commits_only_on_success() {
        let code = [0x48, 0x0f, 0xc1, 0x00]; // XADD [RAX], RAX
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0x0cd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1007;
        let mut memory = Memory::new(&code);
        memory.bytes[7..15].fill(0xff);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 4);
        assert_eq!(state.gpr[Register::Rax as usize], u64::MAX);
        assert_eq!(state.rflags, 0x0417);
        assert_eq!(&memory.bytes[7..15], &0x1006u64.to_le_bytes());
        assert_eq!(
            memory.atomic_adds,
            vec![(0x1007, OperandSize::Qword, 0x1007)]
        );
        assert!(
            memory.accesses.is_empty(),
            "no separate data read/write calls"
        );

        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(0x1007),
            }),
            MemoryError::Backend("atomic fetch-add failed".into()),
        ] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rip: 0,
                rflags: 0x0cd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = 0x1007;
            let before_state = state.clone();
            let mut memory = Memory::new(&code);
            memory.bytes[7..15].fill(0xff);
            memory.atomic_error = Some(injected.clone());
            let before_ram = memory.bytes.clone();
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            match injected {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                error => assert_eq!(result.exit, RunExit::MemoryError(error)),
            }
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_ram);
            assert_eq!(
                memory.atomic_adds,
                vec![(0x1007, OperandSize::Qword, 0x1007)]
            );
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn lock_qword_memory_xadd_uses_one_atomic_call_and_delays_commit_on_error() {
        let code = [0xf0, 0x48, 0x0f, 0xc1, 0x00]; // LOCK XADD [RAX], RAX
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0x0cd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1007;
        let mut memory = Memory::new(&code);
        memory.bytes[7..15].fill(0xff);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 5);
        assert_eq!(state.gpr[Register::Rax as usize], u64::MAX);
        assert_eq!(state.rflags, 0x0417);
        assert_eq!(&memory.bytes[7..15], &0x1006u64.to_le_bytes());
        assert_eq!(
            memory.atomic_adds,
            vec![(0x1007, OperandSize::Qword, 0x1007)]
        );
        assert!(
            memory.accesses.is_empty(),
            "no separate data read/write calls"
        );

        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(0x1007),
            }),
            MemoryError::Backend("locked atomic fetch-add failed".into()),
        ] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rip: 0,
                rflags: 0x0cd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = 0x1007;
            let before_state = state.clone();
            let mut memory = Memory::new(&code);
            memory.bytes[7..15].fill(0xff);
            memory.atomic_error = Some(injected.clone());
            let before_ram = memory.bytes.clone();
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            match injected {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                error => assert_eq!(result.exit, RunExit::MemoryError(error)),
            }
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_ram);
            assert_eq!(
                memory.atomic_adds,
                vec![(0x1007, OperandSize::Qword, 0x1007)]
            );
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn qword_memory_cmpxchg_uses_one_atomic_call_and_preserves_prestate_on_error() {
        let code = [0x48, 0x0f, 0xb1, 0x00]; // CMPXCHG [RAX], RAX
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0x0202,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x1007;
        let mut memory = Memory::new(&code);
        memory.bytes[7..15].copy_from_slice(&0x1008u64.to_le_bytes());
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 4);
        assert_eq!(state.gpr[Register::Rax as usize], 0x1008);
        assert_eq!(state.rflags, 0x0297);
        assert_eq!(&memory.bytes[7..15], &0x1008u64.to_le_bytes());
        assert_eq!(
            memory.atomic_compare_exchanges,
            vec![(0x1007, OperandSize::Qword, 0x1007, 0x1007)]
        );
        assert!(
            memory.accesses.is_empty(),
            "no separate data read/write calls"
        );

        for injected in [
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(3),
                linear_address: Some(0x1007),
            }),
            MemoryError::Backend("atomic compare-exchange failed".into()),
        ] {
            let mut state = LongModeState {
                gpr: [0x1122_3344_5566_7788; 16],
                rip: 0,
                rflags: 0x0202,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[Register::Rax as usize] = 0x1007;
            let before_state = state.clone();
            let mut memory = Memory::new(&code);
            memory.bytes[7..15].copy_from_slice(&0x1008u64.to_le_bytes());
            memory.atomic_error = Some(injected.clone());
            let before_ram = memory.bytes.clone();
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            match injected {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                error => assert_eq!(result.exit, RunExit::MemoryError(error)),
            }
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_ram);
            assert_eq!(
                memory.atomic_compare_exchanges,
                vec![(0x1007, OperandSize::Qword, 0x1007, 0x1007)]
            );
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn qword_register_xadd_alias_doubles_destination_and_sets_add_flags() {
        let code = [0x48, 0x0f, 0xc1, 0xc0]; // XADD RAX, RAX
        let mut state = LongModeState {
            gpr: [0xaabb_ccdd_eeff_0011; 16],
            rip: 0,
            rflags: 0x0cd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[Register::Rax as usize] = 0x7fff_ffff_ffff_ffff;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );

        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(result.rflags_defined, crate::INTEGER_FLAGS);
        assert_eq!(state.rip, 4);
        assert_eq!(state.gpr[Register::Rax as usize], 0xffff_ffff_ffff_fffe);
        assert_eq!(state.rflags, 0x0c92); // DF and bit 1 preserved; AF/SF/OF set.
        for index in 1..16 {
            assert_eq!(state.gpr[index], before.gpr[index]);
        }
        assert!(memory.accesses.is_empty());

        let mut extended_state = LongModeState {
            gpr: [0xaabb_ccdd_eeff_0011; 16],
            rip: 0,
            rflags: 0x0cd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        extended_state.gpr[Register::R8 as usize] = 2;
        extended_state.gpr[Register::R9 as usize] = 3;
        assert_eq!(
            step(
                &mut extended_state,
                &mut Memory::new(&[0x4d, 0x0f, 0xc1, 0xc8]),
                AccessContext::default(),
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(extended_state.gpr[Register::R8 as usize], 5);
        assert_eq!(extended_state.gpr[Register::R9 as usize], 2);
        assert_eq!(extended_state.rflags, 0x0406);
    }

    #[test]
    fn new_transfer_fetch_faults_and_data_faults_have_no_partial_commit() {
        for code in [
            &[0x48, 0xc7, 0x43, 1, 1, 0, 0, 0x80][..],
            &[0x48, 0xa1, 1, 0x10, 0, 0, 0, 0, 0, 0][..],
            &[0x48, 0x0f, 0xbf, 0x43, 1][..],
        ] {
            for cut in 0..code.len() {
                for architectural in [false, true] {
                    let mut state = LongModeState {
                        gpr: [0x1000; 16],
                        rip: 0,
                        rflags: 0x8d7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        gdtr: None,
                        fs_cache: None,
                        gs_cache: None,

                        x87: None,
                    };
                    let before = state.clone();
                    let mut memory = Memory::new(if architectural { code } else { &code[..cut] });
                    let before_ram = memory.bytes.clone();
                    if architectural {
                        memory.fetch_error = Some((
                            cut as u64,
                            MemoryError::Architectural(ArchitecturalFault {
                                vector: 14,
                                error_code: Some(20),
                                linear_address: Some(cut as u64),
                            }),
                        ));
                    }
                    let result = step(&mut state, &mut memory, AccessContext::default());
                    assert!(if architectural {
                        matches!(
                            result,
                            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
                        )
                    } else {
                        matches!(
                            result,
                            Err(MemoryError::Unmapped {
                                access: AccessKind::Fetch,
                                ..
                            })
                        )
                    });
                    assert_eq!(state, before);
                    assert_eq!(memory.bytes, before_ram);
                    assert!(memory.accesses.is_empty());
                }
            }
        }
        for code in [
            &[0x0f, 0xb6, 0x03][..],
            &[0x48, 0x0f, 0xbf, 0x03],
            &[0x48, 0x63, 0x03],
            &[0x48, 0xc7, 0x03, 1, 2, 3, 4],
        ] {
            let mut state = LongModeState {
                gpr: [0x1000; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            let before = state.clone();
            let mut memory = Memory::new(code);
            let before_ram = memory.bytes.clone();
            memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x1000),
            }));
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
            ));
            assert_eq!(state, before);
            assert_eq!(memory.bytes, before_ram);
        }
    }

    #[test]
    fn moffs_address_override_zero_extends_and_unsupported_atomic_forms_do_not_access_data() {
        let mut state = LongModeState::default();
        let before = state.clone();
        let mut memory = Memory::new(&[0x67, 0x48, 0xa1, 0xfe, 0xff, 0xff, 0xff]);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(memory.accesses, [(AccessKind::Read, 0xffff_fffe, 8)]);
        assert_eq!(state, before);
        for code in [
            &[0x86, 0x03][..],
            &[0xf0, 0x48, 0x87, 0x03],
            &[0x66, 0x48, 0x87, 0x03],
        ] {
            let mut memory = Memory::new(code);
            let before_ram = memory.bytes.clone();
            state.gpr[3] = 0x1000;
            let before = state.clone();
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { .. })
            ));
            assert_eq!(state, before);
            assert_eq!(memory.bytes, before_ram);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn eager_arithmetic_flags_match_hand_expected_boundaries_at_every_width() {
        use ArithmeticKind::*;
        for size in [
            OperandSize::Byte,
            OperandSize::Word,
            OperandSize::Dword,
            OperandSize::Qword,
        ] {
            let mask = size.mask();
            let min = 1u64 << (size.bytes() * 8 - 1);
            let max = min - 1;
            // At widths above8, the signed extrema have low byte00/FF (even
            // parity); byte extrema80/7F have odd parity. Expected flags below
            // are hand-derived x86 flag sets, never calculated by another CPU helper.
            let wide_pf = if size == OperandSize::Byte { 0 } else { 4 };
            for (kind, lhs, rhs, carry, result, expected_flags) in [
                (Add, mask, 1, 0, 0, 0x55),
                (Add, max, 1, 0, min, 0x890 | wide_pf),
                (Add, min, min, 0, 0, 0x845),
                (Add, 15, 1, 0, 16, 0x10),
                (Add, 1, 2, 0, 3, 4),
                (Add, 1, 1, 0, 2, 0),
                (Adc, mask, 0, 1, 0, 0x55),
                (Adc, mask, mask, 1, mask, 0x95),
                (Adc, max, 0, 1, min, 0x890 | wide_pf),
                (Adc, 0, max, 1, min, 0x890 | wide_pf),
                (Sub, 0, 1, 0, mask, 0x95),
                (Sub, min, 1, 0, max, 0x810 | wide_pf),
                (Sub, max, mask, 0, min, 0x881 | wide_pf),
                (Cmp, min, min, 0, 0, 0x44),
                (Sbb, 0, mask, 1, 0, 0x55),
                (Sbb, 0, 0, 1, mask, 0x95),
                (Sbb, min, 0, 1, max, 0x810 | wide_pf),
                (Sbb, min, max, 1, 0, 0x854),
                (Sbb, max, mask, 1, max, 0x11 | wide_pf),
                (Inc, mask, 0, 0, 0, 0x54),
                (Inc, mask, 0, 1, 0, 0x55),
                (Inc, max, 0, 1, min, 0x891 | wide_pf),
                (Dec, 0, 0, 0, mask, 0x94),
                (Dec, 0, 0, 1, mask, 0x95),
                (Dec, min, 0, 1, max, 0x811 | wide_pf),
                (Neg, 0, 0, 1, 0, 0x44),
                (Neg, 1, 0, 0, mask, 0x95),
                (Neg, min, 0, 0, min, 0x881 | wide_pf),
                (Neg, 16, 0, 0, mask - 15, 0x85),
                (Neg, 15, 0, 0, mask - 14, 0x91),
            ] {
                let previous =
                    0xa5a5_0000_0000_0602 | (crate::ARITHMETIC_FLAGS & !crate::CF) | carry;
                let actual = arithmetic_result(size, kind, lhs, rhs, previous);
                assert_eq!(
                    actual.0, result,
                    "{kind:?} {size:?} {lhs:x} {rhs:x} CF={carry}"
                );
                assert_eq!(
                    actual.1 & crate::ARITHMETIC_FLAGS,
                    expected_flags,
                    "{kind:?} {size:?} {lhs:x} {rhs:x} CF={carry}"
                );
                assert_eq!(
                    actual.1 & !crate::ARITHMETIC_FLAGS,
                    previous & !crate::ARITHMETIC_FLAGS
                );
            }
        }
    }

    #[test]
    fn arithmetic_aliases_and_sign_extended_immediates_execute_from_literal_bytes() {
        for (code, initial, other, carry, expected, flags) in [
            (
                &[0x00, 0xc8][..],
                0x1122_3344_5566_77ff,
                1,
                0,
                0x1122_3344_5566_7700,
                0x55,
            ),
            (
                &[0x00, 0xc4],
                0x1122_3344_5566_7f01,
                0,
                0,
                0x1122_3344_5566_8001,
                0x890,
            ),
            (
                &[0x40, 0x00, 0xe0],
                0x1122_3344_5566_77ff,
                0,
                0,
                0x1122_3344_5566_7700,
                0x55,
            ),
            (&[0x14, 0xff], 0, 0, 1, 0, 0x55), // ADC AL,FF + carry
            (&[0x1c, 0xff], 0, 0, 1, 0, 0x55), // SBB AL,FF + borrow
            (
                &[0x83, 0xc0, 0xff],
                0x1122_3344_0000_0000,
                0,
                0,
                0xffff_ffff,
                0x84,
            ),
            (&[0x48, 0x83, 0xc0, 0xff], 0, 0, 0, u64::MAX, 0x84),
            (
                &[0x66, 0x83, 0xc0, 0x80],
                0x1122_3344_5566_0000,
                0,
                0,
                0x1122_3344_5566_ff80,
                0x80,
            ),
            (&[0x48, 0x81, 0xe8, 0, 0, 0, 0x80], 0, 0, 0, 0x8000_0000, 5),
            (
                &[0xfe, 0xc4],
                0x1122_3344_5566_ff88,
                0,
                1,
                0x1122_3344_5566_0088,
                0x55,
            ),
            (
                &[0xff, 0xc8],
                0x1122_3344_0000_0000,
                0,
                1,
                0xffff_ffff,
                0x95,
            ),
            (
                &[0xf7, 0xd8],
                0x1122_3344_8000_0000,
                0,
                0,
                0x8000_0000,
                0x885,
            ),
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x402 | carry,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = initial;
            state.gpr[1] = other;
            state.gpr[4] = 1;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected, "{code:02x?}");
            assert_eq!(state.rflags, 0x402 | flags, "{code:02x?}");
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn memory_arithmetic_directions_and_compare_readonly_preserve_exact_state() {
        for (code, initial_rax, initial_mem, expected_rax, expected_mem, flags, writes) in [
            (&[0x03, 0x03][..], 0xffff_ffff, 1u64, 0, 1u64, 0x55, false),
            (&[0x01, 0x03], 1, 0xffff_ffff, 1, 0, 0x55, true),
            (&[0x39, 0x03], 2, 1, 2, 1, 0x95, false), // CMP [RBX],EAX
            (&[0x3b, 0x03], 1, 2, 1, 2, 0x95, false), // CMP EAX,[RBX]
            (&[0x83, 0x03, 0xff], 0, 0, 0, 0xffff_ffff, 0x84, true),
        ] {
            let mut state = LongModeState::default();
            state.gpr[0] = initial_rax;
            state.gpr[3] = 0x1000;
            let mut memory = Memory::new(code);
            memory.bytes[..4].copy_from_slice(&(initial_mem as u32).to_le_bytes());
            let before_ram = memory.bytes.clone();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], expected_rax);
            assert_eq!(&memory.bytes[..4], &(expected_mem as u32).to_le_bytes());
            assert_eq!(&memory.bytes[4..], &before_ram[4..]);
            assert_eq!(state.rflags, 2 | flags);
            assert_eq!(memory.accesses.len(), if writes { 2 } else { 1 });
            assert_eq!(memory.accesses[0], (AccessKind::Read, 0x1000, 4));
            if writes {
                assert_eq!(memory.accesses[1], (AccessKind::Write, 0x1000, 4));
            }
        }
    }

    #[test]
    fn arithmetic_read_and_write_faults_do_not_commit_flags_cpu_or_ram() {
        for code in [
            &[0x48, 0x01, 0x03][..],
            &[0x48, 0x13, 0x03],
            &[0x48, 0x83, 0x1b, 0xff],
            &[0xfe, 0x03],
            &[0x66, 0xff, 0x0b],
            &[0xf7, 0x1b],
        ] {
            for kind in [AccessKind::Read, AccessKind::Write] {
                // ADC register-destination has no data write to fault.
                if code == [0x48, 0x13, 0x03] && kind == AccessKind::Write {
                    continue;
                }
                for architectural in [false, true] {
                    let mut state = LongModeState {
                        gpr: [1; 16],
                        rip: 0,
                        rflags: 0xcd7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        gdtr: None,
                        fs_cache: None,
                        gs_cache: None,

                        x87: None,
                    };
                    state.gpr[3] = 0x1000;
                    let before = state.clone();
                    let mut memory = Memory::new(code);
                    let before_ram = memory.bytes.clone();
                    memory.error_kind = Some(kind);
                    memory.error = Some(if architectural {
                        MemoryError::Architectural(ArchitecturalFault {
                            vector: 14,
                            error_code: Some(7),
                            linear_address: Some(0x1000),
                        })
                    } else {
                        MemoryError::Backend("injected RMW boundary failure".into())
                    });
                    let result = step(&mut state, &mut memory, AccessContext::default());
                    assert!(if architectural {
                        matches!(
                            result,
                            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
                        )
                    } else {
                        matches!(result, Err(MemoryError::Backend(_)))
                    });
                    assert_eq!(state, before, "{code:02x?} {kind:?}");
                    assert_eq!(memory.bytes, before_ram);
                }
            }
        }
    }

    #[test]
    fn arithmetic_immediate_fetch_faults_and_truncations_precede_all_data_accesses() {
        for code in [
            &[0x48, 0x81, 0x6b, 1, 0xff, 0xff, 0xff, 0x7f][..],
            &[0x48, 0x15, 0xff, 0xff, 0xff, 0x7f],
            &[0x67, 0x48, 0x83, 0x44, 0x8b, 0xff, 0x80],
            &[0x48, 0xff, 0x8b, 1, 2, 3, 4],
        ] {
            for cut in 0..code.len() {
                for architectural in [false, true] {
                    let mut state = LongModeState {
                        gpr: [0x1000; 16],
                        rip: 0,
                        rflags: 0xcd7,
                        fs_base: 0,
                        gs_base: 0,
                        fs_selector: 0,
                        gs_selector: 0,
                        gdtr: None,
                        fs_cache: None,
                        gs_cache: None,

                        x87: None,
                    };
                    let before = state.clone();
                    let mut memory = Memory::new(if architectural { code } else { &code[..cut] });
                    let before_ram = memory.bytes.clone();
                    if architectural {
                        memory.fetch_error = Some((
                            cut as u64,
                            MemoryError::Architectural(ArchitecturalFault {
                                vector: 14,
                                error_code: Some(20),
                                linear_address: Some(cut as u64),
                            }),
                        ));
                    }
                    let result = step(&mut state, &mut memory, AccessContext::default());
                    assert!(if architectural {
                        matches!(
                            result,
                            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
                        )
                    } else {
                        matches!(
                            result,
                            Err(MemoryError::Unmapped {
                                access: AccessKind::Fetch,
                                ..
                            })
                        )
                    });
                    assert_eq!(state, before);
                    assert_eq!(memory.bytes, before_ram);
                    assert!(memory.accesses.is_empty());
                }
            }
        }
    }

    #[test]
    fn logical_results_flags_and_untouched_bits_at_every_width() {
        for size in [
            OperandSize::Byte,
            OperandSize::Word,
            OperandSize::Dword,
            OperandSize::Qword,
        ] {
            let sign = 1 << (size.bytes() * 8 - 1);
            let previous = 0x20_0ed7;
            for (kind, left, right, result, szp) in [
                (LogicKind::And, 0x55, 0xaa, 0, 0x44),
                (
                    LogicKind::And,
                    sign | 3,
                    size.mask(),
                    sign | 3,
                    if size == OperandSize::Byte {
                        0x80
                    } else {
                        0x84
                    },
                ),
                (
                    LogicKind::Or,
                    0x80,
                    1,
                    0x81,
                    if size == OperandSize::Byte { 0x84 } else { 4 },
                ),
                (LogicKind::Or, 0x10, 2, 0x12, 4),
                (LogicKind::Xor, size.mask(), size.mask(), 0, 0x44),
                (LogicKind::Xor, 3, 1, 2, 0),
                (
                    LogicKind::Test,
                    sign,
                    sign,
                    sign,
                    if size == OperandSize::Byte {
                        0x80
                    } else {
                        0x84
                    },
                ),
            ] {
                let (actual, flags) = logic_result(size, kind, left, right, previous);
                assert_eq!(actual, result, "{size:?} {kind:?}");
                assert_eq!(
                    flags,
                    (previous & !(crate::ARITHMETIC_FLAGS & !crate::AF)) | szp
                );
            }
            assert_eq!(
                logic_result(size, LogicKind::Not, 0x55, 0, previous),
                (!0x55 & size.mask(), previous)
            );
        }
    }

    #[test]
    fn logical_aliases_and_sign_extended_immediates_from_literal_bytes() {
        for (code, input, source, result) in [
            (
                &[0x20, 0xc4][..],
                0x1122_3344_5566_f00f,
                0,
                0x1122_3344_5566_000f,
            ), // AND AH,AL
            (
                &[0x08, 0xe0][..],
                0x1122_3344_5566_0ff0,
                0,
                0x1122_3344_5566_0fff,
            ), // OR AL,AH
            (
                &[0x66, 0x31, 0xc8][..],
                0x1122_3344_5566_a55a,
                0xffff,
                0x1122_3344_5566_5aa5,
            ),
            (&[0x31, 0xc0][..], u64::MAX, 0, 0), // XOR EAX,EAX also zeroes upper32
            (
                &[0x48, 0x83, 0xe0, 0x80][..],
                0x1234_5678_9abc_deff,
                0,
                0x1234_5678_9abc_de80,
            ),
            (
                &[0x48, 0x0d, 0, 0, 0, 0x80][..],
                0x1234_5678_0000_0001,
                0,
                0xffff_ffff_8000_0001,
            ),
            (
                &[0x48, 0x81, 0xf0, 0xff, 0xff, 0xff, 0xff][..],
                0x1234_5678_9abc_def0,
                0,
                0xedcb_a987_6543_210f,
            ),
            (
                &[0xf6, 0xd4][..],
                0x1122_3344_5566_0f5a,
                0,
                0x1122_3344_5566_f05a,
            ), // NOT AH
            (&[0xf7, 0xd0][..], 0xffff_ffff_0000_0000, 0, 0xffff_ffff),
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0xcd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = input;
            state.gpr[1] = source;
            let mut memory = Memory::new(code);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], result, "{code:02x?}");
            assert_eq!(state.gpr[1], source);
            assert!(memory.accesses.is_empty());
        }
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[4] = 0x1122_3344_5566_77f0;
        state.gpr[1] = 0xff;
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0x40, 0x30, 0xcc]),
                AccessContext::default()
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[4], 0x1122_3344_5566_770f); // XOR SPL,CL
    }

    #[test]
    fn test_is_readonly_even_for_dword_aliases_and_memory() {
        for code in [
            &[0x85, 0xc8][..],
            &[0xa9, 0xff, 0xff, 0xff, 0xff],
            &[0xf7, 0xc0, 0xff, 0xff, 0xff, 0xff],
            &[0x48, 0x85, 0x0b],
            &[0x48, 0xf7, 0x03, 0xff, 0xff, 0xff, 0xff],
        ] {
            let mut state = LongModeState {
                gpr: [u64::MAX; 16],
                rip: 0,
                rflags: 0xcd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[3] = 0x1000;
            let before = state.gpr;
            let mut memory = Memory::new(code);
            // Any attempted write would fail, but TEST must only read.
            memory.error_kind = Some(AccessKind::Write);
            memory.error = Some(MemoryError::Backend("TEST attempted a write".into()));
            let bytes = memory.bytes.clone();
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr, before);
            assert_eq!(memory.bytes, bytes);
            assert!(
                memory
                    .accesses
                    .iter()
                    .all(|(kind, _, _)| *kind == AccessKind::Read)
            );
            assert_eq!(state.rflags & (crate::CF | crate::OF), 0);
        }
        // Qword TEST sign-extends imm32: bit63 of the accumulator must survive
        // the AND with a negative immediate, for both immediate encodings.
        for code in [
            &[0x48, 0xa9, 0, 0, 0, 0x80][..],
            &[0x48, 0xf7, 0xc0, 0, 0, 0, 0x80],
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0xcd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x8000_0000_0000_0000;
            assert_eq!(
                step(&mut state, &mut Memory::new(code), AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0], 0x8000_0000_0000_0000);
            assert_eq!(
                state.rflags & (crate::ARITHMETIC_FLAGS & !crate::AF),
                crate::SF | crate::PF
            );
        }
    }

    #[test]
    fn logical_memory_faults_and_fetch_truncations_never_commit() {
        for code in [
            &[0x48, 0x21, 0x0b][..],
            &[0x48, 0x0b, 0x03],
            &[0x48, 0x83, 0x33, 0xff],
            &[0x48, 0xf7, 0x13],
            &[0x48, 0xf7, 0x03, 0xff, 0xff, 0xff, 0xff],
        ] {
            for access in [AccessKind::Read, AccessKind::Write] {
                if access == AccessKind::Write && matches!(code[1], 0x0b | 0xf7) && code[2] != 0x13
                {
                    continue;
                }
                let mut state = LongModeState {
                    gpr: [1; 16],
                    rip: 0,
                    rflags: 0xcd7,
                    fs_base: 0,
                    gs_base: 0,
                    fs_selector: 0,
                    gs_selector: 0,
                    gdtr: None,
                    fs_cache: None,
                    gs_cache: None,

                    x87: None,
                };
                state.gpr[3] = 0x1000;
                let before = state.clone();
                let mut memory = Memory::new(code);
                let bytes = memory.bytes.clone();
                memory.error_kind = Some(access);
                memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(7),
                    linear_address: Some(0x1000),
                }));
                let mut mask = 0xcc7;
                assert!(matches!(
                    step_tracked(&mut state, &mut memory, AccessContext::default(), &mut mask),
                    Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
                ));
                assert_eq!(state, before);
                assert_eq!(memory.bytes, bytes);
                assert_eq!(mask, 0xcc7);
            }
            for cut in 0..code.len() {
                let mut state = LongModeState {
                    gpr: [0x1000; 16],
                    rip: 0,
                    rflags: 0xcd7,
                    fs_base: 0,
                    gs_base: 0,
                    fs_selector: 0,
                    gs_selector: 0,
                    gdtr: None,
                    fs_cache: None,
                    gs_cache: None,

                    x87: None,
                };
                let before = state.clone();
                let mut memory = Memory::new(&code[..cut]);
                let mut mask = 0xcd7;
                assert!(matches!(
                    step_tracked(&mut state, &mut memory, AccessContext::default(), &mut mask),
                    Err(MemoryError::Unmapped {
                        access: AccessKind::Fetch,
                        ..
                    })
                ));
                assert_eq!(state, before);
                assert_eq!(mask, 0xcd7);
                assert!(memory.accesses.is_empty());
                let mut memory = Memory::new(code);
                memory.fetch_error = Some((
                    cut as u64,
                    MemoryError::Architectural(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(20),
                        linear_address: Some(cut as u64),
                    }),
                ));
                assert!(matches!(
                    step_tracked(&mut state, &mut memory, AccessContext::default(), &mut mask),
                    Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
                ));
                assert_eq!(state, before);
                assert_eq!(mask, 0xcd7);
                assert!(memory.accesses.is_empty());
            }
        }
    }

    #[test]
    fn run_definedness_tracks_successful_instructions_and_preserving_forms() {
        for (code, expected, retired) in [
            (&[0x31, 0xc0][..], 0xcc7, 1),
            (&[0x31, 0xc0, 0x89, 0xc0, 0xf7, 0xd0][..], 0xcc7, 3), // XOR, MOV, NOT
            (&[0x31, 0xc0, 0x83, 0xc0, 1][..], 0xcd7, 2),          // ADD restores AF-defined
            (&[0xf7, 0xd0][..], 0xcd7, 1),
            (&[0x31, 0xc0, 0x48, 0x21, 0x03][..], 0xcc7, 1), // unmapped AND fault
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0xcd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            let mut memory = Memory::new(code);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 10,
                    stop_rip: Some(code.len() as u64),
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.rflags_defined, expected, "{code:02x?}");
            assert_eq!(result.instructions_executed, retired);
        }
    }

    #[test]
    fn shifts_rotates_and_double_shifts_cover_effective_count_boundaries() {
        for (code, initial, _count, result, defined, undefined) in [
            (
                &[0xc0, 0xe0, 1][..],
                0x81,
                1,
                0x02,
                crate::CF | crate::OF | crate::SF | crate::ZF | crate::PF,
                crate::AF,
            ), // SHL AL,1
            (
                &[0xc0, 0xe0, 9][..],
                0x81,
                9,
                0,
                crate::SF | crate::ZF | crate::PF,
                crate::CF | crate::AF | crate::OF,
            ),
            (
                &[0xc0, 0xe0, 8][..],
                0x81,
                8,
                0,
                crate::SF | crate::ZF | crate::PF,
                crate::CF | crate::AF | crate::OF,
            ),
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = initial;
            let mut memory = Memory::new(code);
            let mut flags_defined = 0;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[0] & 0xff, result, "{code:02x?}");
            assert_eq!(flags_defined, defined, "{code:02x?}");
            assert_eq!(
                crate::ARITHMETIC_FLAGS & !flags_defined,
                undefined,
                "{code:02x?}"
            );
        }

        // Rotates reduce the loop count modulo the ring, but flag definedness
        // still follows the architectural masked count before that reduction.
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 1;
        let mut flags_defined = 0;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut Memory::new(&[0xc0, 0xc0, 8]),
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Advanced)
        ); // ROL AL,8 leaves AL but copies bit 0 to CF
        assert_eq!(state.gpr[0] & 0xff, 1);
        assert_eq!(state.rflags & crate::CF, crate::CF);
        assert_eq!(flags_defined, crate::CF);
        assert_eq!(
            crate::ARITHMETIC_FLAGS & !flags_defined,
            crate::OF | crate::AF | crate::SF | crate::ZF | crate::PF
        );

        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: crate::CF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 0x80;
        let mut flags_defined = 0;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut Memory::new(&[0xc0, 0xd0, 9]),
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Advanced)
        ); // RCL AL,9 reduces to zero; OF remains undefined by masked-count rule
        assert_eq!(state.gpr[0] & 0xff, 0x80);
        assert_eq!(state.rflags & crate::CF, crate::CF);
        assert_eq!(flags_defined, 0);
        assert_eq!(
            crate::ARITHMETIC_FLAGS & !flags_defined,
            crate::ARITHMETIC_FLAGS
        );

        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 0x8000_0000_0000_0081;
        let mut flags_defined = 0;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut Memory::new(&[0xc0, 0xf8, 9]),
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Advanced)
        ); // SAR AL,9 keeps shifting in sign and final CF is the sign bit
        assert_eq!(state.gpr[0] & 0xff, 0xff);
        assert_eq!(state.rflags & crate::CF, crate::CF);
        assert_eq!(flags_defined, crate::CF | crate::SF | crate::ZF | crate::PF);

        for (code, index, expected) in [
            (&[0xc1, 0xe0, 0][..], 0, 0x1234_5678), // SHL EAX,0 is a dword write
            (&[0x41, 0xc1, 0xe0, 0][..], 8, 0x1234_5678), // SHL R8D,0
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[index] = 0xabcd_ef01_1234_5678;
            assert_eq!(
                step(&mut state, &mut Memory::new(code), AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[index], expected);
            assert_eq!(state.rflags, 0x8d7);
        }

        for access in [AccessKind::Read, AccessKind::Write] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[3] = 0x1000;
            let before = state.clone();
            let mut memory = Memory::new(&[0x48, 0xc1, 0x23, 0x40]); // SHL qword [RBX],64
            let bytes_before = memory.bytes.clone();
            memory.error_kind = Some(access);
            memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x1000),
            }));
            assert!(matches!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
            ));
            assert_eq!(state, before);
            assert_eq!(memory.bytes, bytes_before);
        }

        // Through-carry rotation of AL includes the incoming CF as a ninth bit.
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: crate::CF,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 0x80;
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0xd0, 0xd0]),
                AccessContext::default()
            ),
            Ok(StepOutcome::Advanced)
        ); // RCL AL,1
        assert_eq!(state.gpr[0] & 0xff, 0x01);
        assert_eq!(state.rflags & crate::CF, crate::CF);

        // SHLD RAX,RCX,1 moves RAX's MSB to CF and RCX's MSB into bit 0.
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 0x8000_0000_0000_0001;
        state.gpr[1] = 0x8000_0000_0000_0000;
        assert_eq!(
            step(
                &mut state,
                &mut Memory::new(&[0x48, 0x0f, 0xa4, 0xc8, 1]),
                AccessContext::default()
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[0], 3);
        assert_eq!(state.rflags & crate::CF, crate::CF);

        // SHLD/SHRD word count 17 has an undefined destination. Confirm the
        // interpreter retires without an oversized host shift and drops every
        // arithmetic-flag definedness promise, without asserting the result.
        for code in [
            &[0x66, 0x0f, 0xa4, 0xc8, 17][..],
            &[0x66, 0x0f, 0xac, 0xc8, 17][..],
        ] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[0] = 0x8001;
            state.gpr[1] = 0x4003;
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut Memory::new(code),
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                Ok(StepOutcome::Advanced),
                "{code:02x?}"
            );
            assert_eq!(state.rip, code.len() as u64);
            assert_eq!(flags_defined & crate::ARITHMETIC_FLAGS, 0, "{code:02x?}");
        }

        // A masked-to-zero SHL preserves flags and definedness while its
        // dword alias still applies normal register write semantics.
        let code = [0x48, 0xc1, 0xe0, 0x40];
        let mut state = LongModeState {
            gpr: [0; 16],
            rip: 0,
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        state.gpr[0] = 0x1234;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[0], before.gpr[0]);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(flags_defined, crate::INTEGER_FLAGS);

        for access in [AccessKind::Read, AccessKind::Write] {
            let mut state = LongModeState {
                gpr: [0; 16],
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[3] = 0x1000;
            let before = state.clone();
            let mut memory = Memory::new(&[0x48, 0xd1, 0x23]); // SHL qword [RBX],1
            let bytes_before = memory.bytes.clone();
            memory.error_kind = Some(access);
            memory.error = Some(MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(7),
                linear_address: Some(0x1000),
            }));
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert!(matches!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut flags_defined
                ),
                Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
            ));
            assert_eq!(state, before);
            assert_eq!(flags_defined, crate::INTEGER_FLAGS);
            assert_eq!(memory.bytes, bytes_before);
        }
    }

    #[test]
    fn bswap_register_forms_reverse_bytes_and_preserve_flags() {
        for (code, register, initial, expected) in [
            (
                &[0x0f, 0xc8][..],
                Register::Rax,
                0x1122_3344_a1b2_c3d4,
                0x0000_0000_d4c3_b2a1,
            ),
            (
                &[0x41, 0x0f, 0xc8][..],
                Register::R8,
                0x1122_3344_a1b2_c3d4,
                0x0000_0000_d4c3_b2a1,
            ),
            (
                &[0x48, 0x0f, 0xc9][..],
                Register::Rcx,
                0x1122_3344_a1b2_c3d4,
                0xd4c3_b2a1_4433_2211,
            ),
            (
                &[0x49, 0x0f, 0xc9][..],
                Register::R9,
                0x1122_3344_a1b2_c3d4,
                0xd4c3_b2a1_4433_2211,
            ),
            (
                &[0x66, 0x49, 0x0f, 0xc9][..],
                Register::R9,
                0x1122_3344_a1b2_c3d4,
                0xd4c3_b2a1_4433_2211,
            ),
        ] {
            let mut state = LongModeState {
                rip: 0,
                rflags: 0x8d7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                ..LongModeState::default()
            };
            state.gpr[register as usize] = initial;
            let before_flags = state.rflags;
            let result = step(&mut state, &mut Memory::new(code), AccessContext::default());
            assert_eq!(result, Ok(StepOutcome::Advanced), "{code:02x?}");
            assert_eq!(state.gpr[register as usize], expected, "{code:02x?}");
            assert_eq!(state.rip, code.len() as u64);
            assert_eq!(state.rflags, before_flags);
        }

        // 66 selects the SDM's undefined 16-bit result; execution must leave
        // the entire state untouched while reporting the encoding unsupported.
        let mut state = LongModeState {
            rip: 0,
            gpr: [0x1122_3344_5566_7788; 16],
            rflags: 0x8d7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        let before = state.clone();
        assert!(matches!(
            step(
                &mut state,
                &mut Memory::new(&[0x66, 0x0f, 0xc8]),
                AccessContext::default()
            ),
            Ok(StepOutcome::Unsupported { opcode: 0xc8 })
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn bit_register_forms_select_modulo_width_and_update_only_cf_and_destination() {
        use crate::long_decode::BitKind;

        let cases = [
            // BT EAX,ECX: 32 selects bit 0; BT leaves all of RAX unchanged.
            (
                &[0x0f, 0xa3, 0xc8][..],
                BitKind::Test,
                32,
                0xaaaa_aaaa_1234_5679,
                32,
                0xaaaa_aaaa_1234_5679,
                1,
            ),
            // BTS BX,CX: signed-looking fff7 selects bit 7 modulo 16.
            (
                &[0x66, 0x0f, 0xab, 0xc8][..],
                BitKind::Set,
                16,
                0x1234,
                0xfff7,
                0x12b4,
                0,
            ),
            // BTS EAX,ECX also checks the architecture's dword zero-extension.
            (
                &[0x0f, 0xab, 0xc8][..],
                BitKind::Set,
                32,
                0xaabb_ccdd_0000_0001,
                31,
                0x8000_0001,
                0,
            ),
            // BTS RAX,RCX selects bit 63, despite the offset's high bits.
            (
                &[0x48, 0x0f, 0xab, 0xc8][..],
                BitKind::Set,
                64,
                1,
                0xffff_ffff_ffff_ffff,
                0x8000_0000_0000_0001,
                0,
            ),
            (
                &[0x66, 0x0f, 0xb3, 0xc8][..],
                BitKind::Reset,
                16,
                0xffff,
                4,
                0xffef,
                1,
            ),
            (
                &[0x0f, 0xb3, 0xc8][..],
                BitKind::Reset,
                32,
                0x8000_0020,
                5,
                0x8000_0000,
                1,
            ),
            (
                &[0x48, 0x0f, 0xb3, 0xc8][..],
                BitKind::Reset,
                64,
                0x8000_0000_0000_0001,
                63,
                1,
                1,
            ),
            (
                &[0x66, 0x0f, 0xbb, 0xc8][..],
                BitKind::Complement,
                16,
                0x8001,
                0xffff,
                1,
                1,
            ),
            (
                &[0x0f, 0xbb, 0xc8][..],
                BitKind::Complement,
                32,
                0x8000_0000,
                31,
                0,
                1,
            ),
            (
                &[0x48, 0x0f, 0xbb, 0xc8][..],
                BitKind::Complement,
                64,
                1,
                64,
                0,
                1,
            ),
        ];

        for (code, kind, width, base, offset, expected, carry) in cases {
            let mut decoder = Memory::new(code);
            let decoded = long_decode::decode(&mut decoder, 0, AccessContext::default()).unwrap();
            assert_eq!(
                decoded.operand_size,
                match width {
                    16 => OperandSize::Word,
                    32 => OperandSize::Dword,
                    64 => OperandSize::Qword,
                    _ => unreachable!(),
                },
                "{code:02x?}"
            );
            assert!(matches!(
                decoded.operation,
                Operation::Bit { kind: decoded_kind, .. } if decoded_kind == kind
            ));
            let mut state = LongModeState {
                rip: 0,
                rflags: 0xcd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                ..LongModeState::default()
            };
            state.gpr[Register::Rax as usize] = base;
            state.gpr[Register::Rcx as usize] = offset;
            let result = run(
                &mut state,
                &mut Memory::new(code),
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(
                result.exit,
                RunExit::InstructionBudgetExhausted,
                "{code:02x?}"
            );
            assert_eq!(result.instructions_executed, 1, "{code:02x?}");
            assert_eq!(state.gpr[Register::Rax as usize], expected, "{code:02x?}");
            assert_eq!(state.rflags, (0xcd7 & !crate::CF) | carry, "{code:02x?}");
            assert_eq!(result.rflags_defined, 0x443, "{code:02x?}");
            assert_eq!(state.rip, code.len() as u64, "{code:02x?}");
        }
    }

    #[test]
    fn bit_immediate_truncation_and_memory_boundaries_do_not_commit() {
        let mut state = LongModeState {
            gpr: [0x1122_3344_5566_7788; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        let before = state.clone();
        let mut truncated = Memory::new(&[0x0f, 0xba, 0xe0]); // BT EAX, imm8 missing
        assert!(matches!(
            step(&mut state, &mut truncated, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 3,
                access: AccessKind::Fetch,
            })
        ));
        assert_eq!(state, before);
        assert!(truncated.accesses.is_empty());

        let mut memory_form = Memory::new(&[0x0f, 0xba, 0x18, 0]); // /3 remains unsupported
        assert_eq!(
            step(&mut state, &mut memory_form, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xba })
        );
        assert_eq!(state, before);
        assert!(memory_form.accesses.is_empty());
    }

    #[test]
    fn bit_immediate_memory_uses_width_mask_exact_access_and_delayed_commit() {
        let code = [0x0f, 0xba, 0x20, 0xff]; // BT dword [RAX],255 => bit 31 at [RAX]
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd6,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x1000;
        let mut memory = Memory::new(&code);
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);
        assert_eq!(state.rip, 4);
        assert_eq!(state.rflags, 0xcd6);
        assert_eq!(flags_defined, 0x443);

        // A write operation whose resulting value is unchanged still issues
        // the ordinary exact-width write before committing flags and RIP.
        let code = [0x0f, 0xba, 0x28, 0]; // BTS dword [RAX],0
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd6,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x1000;
        let mut memory = Memory::new(&code);
        memory.bytes[0] = 1;
        memory.error = Some(MemoryError::Backend("injected write fault".into()));
        memory.error_kind = Some(AccessKind::Write);
        let before_state = state.clone();
        let before_bytes = memory.bytes.clone();
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined,
            ),
            Err(MemoryError::Backend("injected write fault".into()))
        );
        assert_eq!(state, before_state);
        assert_eq!(memory.bytes, before_bytes);
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Read, 0x1000, 4),
                (AccessKind::Write, 0x1000, 4)
            ]
        );
        assert_eq!(flags_defined, crate::INTEGER_FLAGS);
    }

    #[test]
    fn bit_immediate_memory_read_fault_does_not_commit_and_rip_relative_uses_immediate_end() {
        let code = [0x0f, 0xba, 0x28, 0xff]; // BTS dword [RAX],255
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x1000;
        let mut memory = Memory::new(&code);
        memory.error = Some(MemoryError::Backend("injected read fault".into()));
        memory.error_kind = Some(AccessKind::Read);
        let before_state = state.clone();
        let before_bytes = memory.bytes.clone();
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert_eq!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined,
            ),
            Err(MemoryError::Backend("injected read fault".into()))
        );
        assert_eq!(state, before_state);
        assert_eq!(memory.bytes, before_bytes);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 4)]);
        assert_eq!(flags_defined, crate::INTEGER_FLAGS);

        // The displacement makes next_rip (including imm8) land at 0x1000.
        let code = [0x0f, 0xba, 0x3d, 0xf8, 0x0f, 0, 0, 0];
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd6,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Read, 0x1000, 4),
                (AccessKind::Write, 0x1000, 4)
            ]
        );
        assert_eq!(state.rip, code.len() as u64);
    }

    #[test]
    fn bit_register_memory_offsets_select_signed_adjacent_elements() {
        use crate::long_decode::BitKind;

        for (prefix, bytes, size, offset, expected_address, bit_index) in [
            (
                &[0x66][..],
                2usize,
                OperandSize::Word,
                u64::MAX & 0xffff,
                0x100e,
                15,
            ),
            (
                &[][..],
                4,
                OperandSize::Dword,
                u64::MAX & 0xffff_ffff,
                0x100c,
                31,
            ),
            (&[0x48][..], 8, OperandSize::Qword, u64::MAX, 0x1008, 63),
        ] {
            for (opcode, kind, write) in [
                (0xa3, BitKind::Test, false),
                (0xab, BitKind::Set, true),
                (0xb3, BitKind::Reset, true),
                (0xbb, BitKind::Complement, true),
            ] {
                let mut code = prefix.to_vec();
                code.extend([0x0f, opcode, 0x08]); // [RAX], CX
                let mut state = LongModeState {
                    rip: 0,
                    rflags: 0xcd6,
                    fs_base: 0,
                    gs_base: 0,
                    fs_selector: 0,
                    gs_selector: 0,
                    gdtr: None,
                    fs_cache: None,
                    gs_cache: None,
                    ..LongModeState::default()
                };
                state.gpr[Register::Rax as usize] = 0x1010;
                state.gpr[Register::Rcx as usize] = offset;
                let mut memory = Memory::new(&code);
                memory.bytes[expected_address as usize - 0x1000..][..bytes]
                    .copy_from_slice(&(1u64 << bit_index).to_le_bytes()[..bytes]);
                let mut flags_defined = crate::INTEGER_FLAGS;
                assert_eq!(
                    step_tracked(
                        &mut state,
                        &mut memory,
                        AccessContext::default(),
                        &mut flags_defined,
                    ),
                    Ok(StepOutcome::Advanced),
                    "{kind:?} {size:?}"
                );
                let mut accesses = vec![(AccessKind::Read, expected_address, bytes)];
                if write {
                    accesses.push((AccessKind::Write, expected_address, bytes));
                }
                assert_eq!(memory.accesses, accesses, "{kind:?} {size:?}");
                assert_eq!(state.rflags & crate::CF, 1, "{kind:?} {size:?}");
                assert_eq!(state.rip, code.len() as u64);
                assert_eq!(flags_defined, 0x443);
            }
        }

        // Positive offset equal to the operand width selects the next element.
        let code = [0x0f, 0xab, 0x08]; // BTS dword [RAX], ECX
        let mut state = LongModeState {
            rip: 0,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x1000;
        state.gpr[Register::Rcx as usize] = 32;
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            memory.accesses,
            [
                (AccessKind::Read, 0x1004, 4),
                (AccessKind::Write, 0x1004, 4)
            ]
        );
    }

    #[test]
    fn bit_register_memory_address_and_data_faults_leave_state_and_ram_uncommitted() {
        let code = [0x0f, 0xab, 0x08]; // BTS dword [RAX], ECX
        for fault_kind in [Some(AccessKind::Read), Some(AccessKind::Write)] {
            let mut state = LongModeState {
                rip: 0,
                rflags: 0xcd6,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                ..LongModeState::default()
            };
            state.gpr[Register::Rax as usize] = 0x1000;
            state.gpr[Register::Rcx as usize] = 32; // element at 0x1004
            let before_state = state.clone();
            let mut memory = Memory::new(&code);
            if let Some(kind) = fault_kind {
                memory.error = Some(MemoryError::Backend("injected BT-family fault".into()));
                memory.error_kind = Some(kind);
            }
            let before_ram = memory.bytes.clone();
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                if fault_kind.is_some() {
                    Err(MemoryError::Backend("injected BT-family fault".into()))
                } else {
                    Ok(StepOutcome::Advanced)
                }
            );
            if let Some(kind) = fault_kind {
                assert_eq!(state, before_state);
                assert_eq!(memory.bytes, before_ram);
                assert_eq!(flags_defined, crate::INTEGER_FLAGS);
                assert_eq!(memory.accesses[0], (AccessKind::Read, 0x1004, 4));
                if kind == AccessKind::Write {
                    assert_eq!(memory.accesses[1], (AccessKind::Write, 0x1004, 4));
                }
            }
        }

        // A noncanonical final address faults before the memory backend and
        // leaves all CPU state and flag definedness untouched.
        let code = [0x0f, 0xab, 0x08];
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd6,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x0000_8000_0000_0000;
        state.gpr[Register::Rcx as usize] = 0;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert!(matches!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Fault(_))
        ));
        assert_eq!(state, before);
        assert_eq!(flags_defined, crate::INTEGER_FLAGS);
        assert!(memory.accesses.is_empty());

        // With 67, the signed element displacement is added before the
        // architectural 32-bit address-size wrap and exact-width access.
        let code = [0x67, 0x0f, 0xa3, 0x08]; // BT dword [EAX], ECX
        let mut state = LongModeState {
            rip: 0,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0;
        state.gpr[Register::Rcx as usize] = u64::MAX as u32 as u64;
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                address: 0xffff_fffc,
                access: AccessKind::Read,
            })
        );
        assert_eq!(memory.accesses, [(AccessKind::Read, 0xffff_fffc, 4)]);
    }

    #[test]
    fn bit_register_memory_signed_minimum_and_canonical_range_edges_fault_before_commit() {
        for (code, minimum, expected) in [
            (
                &[0x66, 0x0f, 0xa3, 0x08][..],
                0x8000u64,
                Err(MemoryError::Unmapped {
                    address: 0,
                    access: AccessKind::Read,
                }),
            ),
            (
                &[0x0f, 0xa3, 0x08][..],
                0x8000_0000u64,
                Err(MemoryError::Unmapped {
                    address: 0xffff_ffff_f000_1000,
                    access: AccessKind::Read,
                }),
            ),
            (
                &[0x48, 0x0f, 0xa3, 0x08][..],
                0x8000_0000_0000_0000u64,
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: 13,
                    error_code: Some(0),
                    linear_address: None,
                })),
            ),
        ] {
            let mut state = LongModeState {
                rip: 0,
                rflags: 0xcd6,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                ..LongModeState::default()
            };
            state.gpr[Register::Rax as usize] = 0x1000;
            state.gpr[Register::Rcx as usize] = minimum;
            let before = state.clone();
            let mut memory = Memory::new(code);
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                expected,
                "{code:02x?}"
            );
            assert_eq!(state, before, "{code:02x?}");
            assert_eq!(flags_defined, crate::INTEGER_FLAGS, "{code:02x?}");
            if code[0] == 0x48 {
                assert!(memory.accesses.is_empty());
            }
        }

        // The qword starts in the low canonical half but its final byte is
        // beyond the canonical edge, so the exact-width check faults before read.
        let code = [0x48, 0x0f, 0xa3, 0x08];
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd6,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_fffc;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        let mut flags_defined = crate::INTEGER_FLAGS;
        assert!(matches!(
            step_tracked(
                &mut state,
                &mut memory,
                AccessContext::default(),
                &mut flags_defined,
            ),
            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 13, .. }))
        ));
        assert_eq!(state, before);
        assert_eq!(flags_defined, crate::INTEGER_FLAGS);
        assert!(memory.accesses.is_empty());
    }

    #[test]
    fn bit_register_source_aliases_use_the_prewrite_destination_value() {
        use crate::long_decode::BitKind;

        for (code, kind, expected) in [
            (&[0x66, 0x0f, 0xa3, 0xc0][..], BitKind::Test, 0x8001), // BT AX,AX => bit 1
            (&[0x66, 0x0f, 0xab, 0xc0][..], BitKind::Set, 0x8003),
            (&[0x66, 0x0f, 0xb3, 0xc0][..], BitKind::Reset, 0x8001),
            (&[0x66, 0x0f, 0xbb, 0xc0][..], BitKind::Complement, 0x8003),
        ] {
            let mut state = LongModeState {
                rip: 0,
                rflags: 0xcd6,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,
                ..LongModeState::default()
            };
            state.gpr[Register::Rax as usize] = 0x8001;
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut Memory::new(code),
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr[Register::Rax as usize], expected, "{kind:?}");
            assert_eq!(state.rflags & crate::CF, 0, "{kind:?}");
            assert_eq!(state.rflags & crate::ZF, crate::ZF, "ZF must be preserved");
            assert_eq!(flags_defined, 0x443, "{kind:?}");
        }

        let code = [0x4d, 0x0f, 0xbb, 0xd1]; // BTC R9,R10
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0,
            gs_base: 0,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::R9 as usize] = 1;
        state.gpr[Register::R10 as usize] = 63;
        let result = run(
            &mut state,
            &mut Memory::new(&code),
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.gpr[Register::R9 as usize], 0x8000_0000_0000_0001);
        assert_eq!(state.gpr[Register::R10 as usize], 63);
        assert_eq!(state.rflags & crate::CF, 0);
        assert_eq!(result.rflags_defined, 0x443);
    }

    #[test]
    fn bit_scans_cover_width_edges_zero_sources_aliasing_and_flags() {
        use crate::long_decode::BitScanKind::{Forward, Reverse};
        let cases = [
            // Zero leaves the whole destination untouched; zero has even parity.
            (
                &[0x66, 0x0f, 0xbc, 0xc1][..],
                Forward,
                Register::Rax,
                Register::Rcx,
                16,
                0,
                0x1122_3344_5566_7788,
                None,
            ),
            (
                &[0x66, 0x0f, 0xbd, 0xc1][..],
                Reverse,
                Register::Rax,
                Register::Rcx,
                16,
                0x8000,
                0,
                Some(15),
            ),
            (
                &[0x0f, 0xbc, 0xc1][..],
                Forward,
                Register::Rax,
                Register::Rcx,
                32,
                0x8000_0000,
                0xaaaa_bbbb_cccc_dddd,
                Some(31),
            ),
            (
                &[0x0f, 0xbd, 0xc1][..],
                Reverse,
                Register::Rax,
                Register::Rcx,
                32,
                1,
                0,
                Some(0),
            ),
            (
                &[0x48, 0x0f, 0xbc, 0xc1][..],
                Forward,
                Register::Rax,
                Register::Rcx,
                64,
                0,
                0x1234_5678_9abc_def0,
                None,
            ),
            (
                &[0x48, 0x0f, 0xbd, 0xc1][..],
                Reverse,
                Register::Rax,
                Register::Rcx,
                64,
                0x8000_0000_0000_0000,
                0,
                Some(63),
            ),
            // REX.R/B select R8D/R9D; a nonzero 32-bit write clears upper R8.
            (
                &[0x45, 0x0f, 0xbc, 0xc1][..],
                Forward,
                Register::R8,
                Register::R9,
                32,
                3,
                0xaabb_ccdd_eeee_ffff,
                Some(0),
            ),
            // Aliasing proves the source is read before destination write.
            (
                &[0x4d, 0x0f, 0xbd, 0xc9][..],
                Reverse,
                Register::R9,
                Register::R9,
                64,
                0x8000_0000_0000_0001,
                0,
                Some(63),
            ),
        ];

        for (code, kind, dst, src, width, source, old_dst, expected_index) in cases {
            let mut state = LongModeState {
                rip: 0,
                gpr: [0xfeed_face_dead_beef; 16],
                rflags: 0x8000_0000_0000_0cd7,
                fs_base: 0,
                gs_base: 0,
                fs_selector: 0,
                gs_selector: 0,
                gdtr: None,
                fs_cache: None,
                gs_cache: None,

                x87: None,
            };
            state.gpr[dst as usize] = old_dst;
            state.gpr[src as usize] = source;
            let before_dst = state.gpr[dst as usize];
            let mut flags_defined = crate::INTEGER_FLAGS;
            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut Memory::new(code),
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                Ok(StepOutcome::Advanced),
                "{code:02x?}"
            );
            let expected_value = expected_index.map_or(before_dst, |index| index as u64);
            assert_eq!(state.gpr[dst as usize], expected_value, "{code:02x?}");
            let width_mask = if width == 64 {
                u64::MAX
            } else {
                (1u64 << width) - 1
            };
            let masked_source = source & width_mask;
            let expected_flags = (0x8000_0000_0000_0cd7 & !crate::ARITHMETIC_FLAGS)
                | if masked_source == 0 { crate::ZF } else { 0 }
                | if masked_source.count_ones() % 2 == 0 {
                    crate::PF
                } else {
                    0
                };
            assert_eq!(state.rflags, expected_flags, "{kind:?} {code:02x?}");
            assert_eq!(flags_defined, crate::INTEGER_FLAGS, "{kind:?} {code:02x?}");
            assert_eq!(state.rip, code.len() as u64, "{code:02x?}");
        }
    }

    #[test]
    fn fs_selector_read_zero_extends_and_preserves_other_state() {
        let mut state = LongModeState {
            gpr: [0xfeed_face_dead_beef; 16],
            rip: 0,
            rflags: 0x8000_0000_0000_0cd7,
            fs_base: 0xffff_8000_0000_1000,
            gs_base: 0x1234,
            fs_selector: 0xf123,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        let mut memory = Memory::new(&[0x48, 0x8c, 0xe0]);
        let before = state.clone();
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        let mut expected = before;
        expected.gpr[Register::Rax as usize] = 0xf123;
        expected.rip = 3;
        assert_eq!(state, expected);
    }

    #[test]
    fn gs_selector_read_zero_extends_and_preserves_other_state() {
        let mut state = LongModeState {
            gpr: [u64::MAX; 16],
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0x1234,
            gs_base: 0xffff_8000_0000_1000,
            fs_selector: 0xabcd,
            gs_selector: 0xf123,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,

            x87: None,
        };
        let mut memory = Memory::new(&[0x48, 0x8c, 0xe8]);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0xf123);
        assert_eq!(state.rip, 3);
        assert_eq!(state.rflags, 0xcd7);
        assert_eq!(state.fs_base, 0x1234);
        assert_eq!(state.gs_base, 0xffff_8000_0000_1000);
        assert_eq!(state.fs_selector, 0xabcd);
        assert_eq!(state.gs_selector, 0xf123);
    }

    fn verw_state(selector: u16, limit: u16) -> LongModeState {
        let mut state = LongModeState {
            gdtr: Some(crate::state::Gdtr {
                base: 0x1000,
                limit,
            }),
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0000 | u64::from(selector);
        state.rflags = crate::CF | crate::ZF | crate::OF | 2;
        state
    }

    #[test]
    fn verw_reads_one_writable_descriptor_as_supervisor_and_only_changes_zf() {
        let mut state = verw_state(0x0b, 15); // index 8, RPL 3
        let before_gprs = state.gpr;
        let before_flags = state.rflags;
        let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        memory.bytes[8 + 5] = 0xf3; // present, DPL3, writable data, accessed
        let before_bytes = memory.bytes.clone();

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr, before_gprs);
        assert_eq!(state.rflags, before_flags | crate::ZF);
        assert_eq!(state.rip, 3);
        assert_eq!(memory.bytes, before_bytes);
        assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        assert_eq!(
            memory.contexts,
            vec![(
                AccessKind::Read,
                AccessContext::default().descriptor_table()
            )]
        );
        assert_eq!(
            memory.contexts[0].1.privilege,
            PrivilegeLevel::Ring3,
            "instruction CPL remains ring 3 for DPL/RPL checks"
        );
        assert_eq!(
            memory.contexts[0].1.page_privilege(),
            PrivilegeLevel::Ring0,
            "descriptor-table read uses supervisor page permissions"
        );
    }

    #[test]
    fn verw_restores_zf_definedness_for_both_rights_outcomes() {
        for (access, expected_zf) in [(0xf3, true), (0xd3, false)] {
            let mut state = verw_state(0x0b, 15);
            state.rflags &= !crate::ZF;
            let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
            memory.bytes[8 + 5] = access;
            let mut flags_defined = crate::INTEGER_FLAGS & !crate::ZF;

            assert_eq!(
                step_tracked(
                    &mut state,
                    &mut memory,
                    AccessContext::default(),
                    &mut flags_defined,
                ),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.rflags & crate::ZF != 0, expected_zf);
            assert_eq!(flags_defined, crate::INTEGER_FLAGS);
        }
    }

    #[test]
    fn verw_clears_zf_for_read_only_code_system_and_privilege_mismatch() {
        for (access, selector) in [
            (0xf0, 0x0b), // read-only data
            (0xfa, 0x0b), // code descriptor
            (0x82, 0x0b), // system descriptor
            (0xd2, 0x0b), // writable data DPL2; CPL/RPL3
            (0xd2, 0x08), // writable data DPL2; CPL3 fails even with RPL0
            (0xd2, 0x0b), // writable data DPL2; CPL/RPL3
        ] {
            let mut state = verw_state(selector, 15);
            state.rflags &= !crate::ZF;
            let before_gprs = state.gpr;
            let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
            memory.bytes[8 + 5] = access;
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced),
                "access={access:#x} selector={selector:#x}"
            );
            assert_eq!(state.gpr, before_gprs);
            assert_eq!(state.rflags & crate::ZF, 0);
            assert_eq!(state.rflags & !crate::ZF, crate::CF | crate::OF | 2);
            assert_eq!(state.rip, 3);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        }
    }

    #[test]
    fn verw_ignores_present_and_checks_full_gdt_entry_bounds_without_reading() {
        let mut not_present = verw_state(0x0b, 15);
        let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        memory.bytes[8 + 5] = 0x72; // P=0, DPL3, writable data
        assert_eq!(
            step(&mut not_present, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_ne!(not_present.rflags & crate::ZF, 0);

        let mut exact_fit = verw_state(0x0b, 15);
        let mut exact_memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        exact_memory.bytes[13] = 0xf2;
        assert_eq!(
            step(&mut exact_fit, &mut exact_memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_ne!(exact_fit.rflags & crate::ZF, 0);
        assert_eq!(exact_memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);

        let mut short_table = verw_state(0x0b, 14);
        let mut short_memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        short_memory.bytes[13] = 0xf2;
        assert_eq!(
            step(
                &mut short_table,
                &mut short_memory,
                AccessContext::default()
            ),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(short_table.rflags & crate::ZF, 0);
        assert!(short_memory.accesses.is_empty());
        assert_eq!(short_table.rip, 3);
    }

    #[test]
    fn verw_null_selector_and_unsupported_ti_or_cpl_do_not_read_or_commit() {
        let mut null = verw_state(3, 0xffff);
        let mut null_memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        assert_eq!(
            step(&mut null, &mut null_memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(null.rflags & crate::ZF, 0);
        assert!(null_memory.accesses.is_empty());

        for (selector, context) in [
            (0x0c, AccessContext::default()), // TI=1 selects unsupported LDT
            (
                0x0b,
                AccessContext {
                    mode: crate::memory::InstructionMode::Long64,
                    privilege: PrivilegeLevel::Ring2,
                    origin: crate::memory::AccessOrigin::Explicit,
                },
            ),
        ] {
            let mut state = verw_state(selector, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
            assert_eq!(
                step(&mut state, &mut memory, context),
                Ok(StepOutcome::Unsupported { opcode: 0x00 })
            );
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn verw_descriptor_faults_propagate_without_cpu_or_retirement_commit() {
        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0), // descriptor-table page access is supervisor-class
            linear_address: Some(0x1008),
        });
        let mut state = verw_state(0x0b, 15);
        let before = state.clone();
        let mut memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        memory.error = Some(fault.clone());
        memory.error_kind = Some(AccessKind::Read);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            result.exit,
            RunExit::Fault(match fault {
                MemoryError::Architectural(fault) => fault,
                _ => unreachable!(),
            })
        );
        assert_eq!(state, before);

        let backend = MemoryError::Backend("injected descriptor read failure".into());
        let mut backend_state = verw_state(0x0b, 15);
        let before_backend = backend_state.clone();
        let mut backend_memory = Memory::new(&[0x0f, 0x00, 0xe8]);
        backend_memory.error = Some(backend.clone());
        backend_memory.error_kind = Some(AccessKind::Read);
        let backend_result = run(
            &mut backend_state,
            &mut backend_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(backend_result.instructions_executed, 0);
        assert_eq!(backend_result.exit, RunExit::MemoryError(backend));
        assert_eq!(backend_state, before_backend);
    }

    fn fs_load_state(selector: u16, limit: u16) -> LongModeState {
        let mut state = LongModeState {
            gdtr: Some(crate::state::Gdtr {
                base: 0x1000,
                limit,
            }),
            fs_base: 0xffff_8000_1234_5678,
            fs_selector: 0x33,
            fs_cache: Some(FsDescriptorCache {
                limit: 0x1234,
                access: 0xf3,
                flags: 0,
            }),
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0000 | u64::from(selector);
        state.rflags = 0x0cd7;
        state
    }

    fn descriptor(base: u32, limit: u32, access: u8, flags: u8) -> [u8; 8] {
        [
            limit as u8,
            (limit >> 8) as u8,
            base as u8,
            (base >> 8) as u8,
            (base >> 16) as u8,
            access,
            ((flags & 0x0f) << 4) | ((limit >> 16) as u8 & 0x0f),
            (base >> 24) as u8,
        ]
    }

    #[test]
    fn mov_fs_ax_loads_one_cached_gdt_descriptor_and_expands_granular_limit() {
        for (flags, limit, expected_limit) in [
            (0x02, 0x000a_bcde, 0x000a_bcde),
            (0x08, 0x000a_bcde, 0xabcdefff),
        ] {
            let mut state = fs_load_state(0x0b, 15);
            let before_gprs = state.gpr;
            let before_rflags = state.rflags;
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            memory.bytes[8..16].copy_from_slice(&descriptor(0x8123_4567, limit, 0xf3, flags));
            let before_memory = memory.bytes.clone();

            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr, before_gprs);
            assert_eq!(state.rflags, before_rflags);
            assert_eq!(state.rip, 2);
            assert_eq!(state.fs_selector, 0x0b);
            assert_eq!(state.fs_base, 0x8123_4567);
            assert_eq!(
                state.fs_cache,
                Some(FsDescriptorCache {
                    limit: expected_limit,
                    access: 0xf3,
                    flags,
                })
            );
            assert_eq!(
                state.gdtr,
                Some(crate::state::Gdtr {
                    base: 0x1000,
                    limit: 15
                })
            );
            assert_eq!(memory.bytes, before_memory);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
            assert_eq!(
                memory.contexts,
                vec![(
                    AccessKind::Read,
                    AccessContext::default().descriptor_table(),
                )]
            );
            assert_eq!(memory.contexts[0].1.privilege, PrivilegeLevel::Ring3);
            assert_eq!(memory.contexts[0].1.page_privilege(), PrivilegeLevel::Ring0);
        }
    }

    #[test]
    fn mov_fs_ax_sets_only_the_accessed_byte_after_validating_an_a_zero_descriptor() {
        let descriptor_context = AccessContext::default().descriptor_table();
        for access in [0xf2, 0xf0] {
            let mut state = fs_load_state(0x0b, 15);
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            memory.bytes[8..16].copy_from_slice(&descriptor(0x8123_4567, 0xabcde, access, 0x08));
            let before = memory.bytes.clone();

            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.rip, 2);
            assert_eq!(state.fs_selector, 0x0b);
            assert_eq!(state.fs_base, 0x8123_4567);
            assert_eq!(state.fs_cache.unwrap().access, access | 1);
            assert_eq!(memory.bytes[8..13], before[8..13]);
            assert_eq!(memory.bytes[13], access | 1);
            assert_eq!(memory.bytes[14..16], before[14..16]);
            assert_eq!(
                memory.accesses,
                vec![
                    (AccessKind::Read, 0x1008, 8),
                    (AccessKind::Write, 0x100d, 1),
                ]
            );
            assert_eq!(
                memory.contexts,
                vec![
                    (AccessKind::Read, descriptor_context),
                    (AccessKind::Write, descriptor_context),
                ]
            );
            for (_, context) in memory.contexts {
                assert_eq!(context.privilege, PrivilegeLevel::Ring3);
                assert_eq!(context.origin, crate::memory::AccessOrigin::DescriptorTable);
                assert_eq!(context.page_privilege(), PrivilegeLevel::Ring0);
            }
        }
    }

    #[test]
    fn mov_fs_ax_a_one_needs_no_write_permission_or_write_call() {
        let mut state = fs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe0]);
        memory.deny_writes = true;
        memory.bytes[8..16].copy_from_slice(&descriptor(0x1234, 0xffff, 0xf3, 0));
        let before_memory = memory.bytes.clone();

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.fs_selector, 0x0b);
        assert_eq!(state.fs_base, 0x1234);
        assert_eq!(memory.bytes, before_memory);
        assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        assert_eq!(memory.contexts.len(), 1);
        assert_eq!(memory.contexts[0].0, AccessKind::Read);
    }

    #[test]
    fn mov_fs_ax_accessed_write_failures_preserve_cpu_and_memory() {
        let architectural_fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(0x100d),
        });
        let backend_error = MemoryError::Backend("injected descriptor write failure".into());
        for error in [architectural_fault.clone(), backend_error.clone()] {
            let mut state = fs_load_state(0x0b, 15);
            let before_state = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            memory.bytes[8..16].copy_from_slice(&descriptor(0x1234, 0xffff, 0xf2, 0));
            let before_memory = memory.bytes.clone();
            memory.error = Some(error.clone());
            memory.error_kind = Some(AccessKind::Write);

            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            match error {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                other => assert_eq!(result.exit, RunExit::MemoryError(other)),
            }
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_memory);
            assert_eq!(
                memory.accesses,
                vec![
                    (AccessKind::Read, 0x1008, 8),
                    (AccessKind::Write, 0x100d, 1),
                ]
            );
        }

        let mut read_state = fs_load_state(0x0b, 15);
        let before_state = read_state.clone();
        let mut read_memory = Memory::new(&[0x8e, 0xe0]);
        read_memory.error = Some(MemoryError::Backend("descriptor read failure".into()));
        read_memory.error_kind = Some(AccessKind::Read);
        let before_memory = read_memory.bytes.clone();
        let result = run(
            &mut read_state,
            &mut read_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Backend("descriptor read failure".into()))
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(read_state, before_state);
        assert_eq!(read_memory.bytes, before_memory);
        assert_eq!(read_memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn mov_fs_ax_validation_faults_precede_an_armed_accessed_write_failure() {
        for (access, expected_vector) in [(0x82, 13), (0x18, 13), (0x92, 13), (0x72, 11)] {
            let mut state = fs_load_state(0x0b, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            memory.bytes[8..16].copy_from_slice(&descriptor(0, 0xffff, access, 0));
            memory.error = Some(MemoryError::Backend("write should not be reached".into()));
            memory.error_kind = Some(AccessKind::Write);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: expected_vector,
                    error_code: Some(8),
                    linear_address: None,
                }))
            );
            assert_eq!(state, before);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        }
    }

    #[test]
    fn mov_fs_ax_sets_accessed_once_then_a_second_load_reads_without_writing() {
        let mut state = fs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe0, 0x8e, 0xe0]);
        memory.bytes[8..16].copy_from_slice(&descriptor(0x1234, 0xffff, 0xf2, 0));

        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, 4);
        assert_eq!(memory.bytes[13], 0xf3);
        assert_eq!(
            memory.accesses,
            vec![
                (AccessKind::Read, 0x1008, 8),
                (AccessKind::Write, 0x100d, 1),
                (AccessKind::Read, 0x1008, 8),
            ]
        );
    }

    #[test]
    fn mov_fs_ax_accessed_write_remains_committed_before_a_later_fault() {
        let mut state = fs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe0, 0x0f, 0x0b]);
        memory.bytes[8..16].copy_from_slice(&descriptor(0x8123_4567, 0xabcde, 0xf2, 0));

        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 2);
        assert_eq!(state.fs_selector, 0x0b);
        assert_eq!(state.fs_base, 0x8123_4567);
        assert_eq!(state.fs_cache.unwrap().access, 0xf3);
        assert_eq!(memory.bytes[13], 0xf3);
        assert_eq!(
            memory.accesses,
            vec![
                (AccessKind::Read, 0x1008, 8),
                (AccessKind::Write, 0x100d, 1),
            ]
        );
    }

    #[test]
    fn mov_fs_ax_checks_type_privilege_presence_and_accessed_in_order() {
        for (access, vector, error_code) in [
            (0x02, 13, 0x08), // system descriptor, even though not present
            (0x18, 13, 0x08), // execute-only code, even though not present
            (0x52, 13, 0x08), // CPL3/RPL3 exceeds DPL2, even though not present
            (0x93, 13, 0x08), // CPL3/RPL0 still exceeds DPL0
            (0x73, 11, 0x08), // otherwise valid, not present
        ] {
            let mut state = fs_load_state(0x0b, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            memory.bytes[8..16].copy_from_slice(&descriptor(0x1000, 0xffff, access, 0));
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector,
                    error_code: Some(error_code),
                    linear_address: None,
                }))
            );
            assert_eq!(state, before);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        }
    }

    #[test]
    fn mov_fs_ax_bounds_and_unsupported_selectors_do_not_read_or_commit() {
        let mut short = fs_load_state(0x0b, 14);
        let before = short.clone();
        let mut memory = Memory::new(&[0x8e, 0xe0]);
        assert_eq!(
            step(&mut short, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(8),
                linear_address: None,
            }))
        );
        assert_eq!(short, before);
        assert!(memory.accesses.is_empty());

        let mut truncated = fs_load_state(0x0b, 15);
        let before = truncated.clone();
        let mut truncated_memory = Memory::new(&[0x8e]);
        assert_eq!(
            step(
                &mut truncated,
                &mut truncated_memory,
                AccessContext::default()
            ),
            Err(MemoryError::Unmapped {
                address: 1,
                access: AccessKind::Fetch,
            })
        );
        assert_eq!(truncated, before);

        for (selector, context) in [
            (0x03, AccessContext::default()), // null selector
            (0x0c, AccessContext::default()), // LDT selector
            (
                0x0b,
                AccessContext {
                    privilege: PrivilegeLevel::Ring2,
                    ..AccessContext::default()
                },
            ),
        ] {
            let mut state = fs_load_state(selector, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe0]);
            assert_eq!(
                step(&mut state, &mut memory, context),
                Ok(StepOutcome::Unsupported { opcode: 0x8e })
            );
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn mov_fs_ax_descriptor_fault_propagates_without_state_or_retirement_commit() {
        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x1008),
        });
        let mut state = fs_load_state(0x0b, 15);
        let before = state.clone();
        let mut memory = Memory::new(&[0x8e, 0xe0]);
        memory.error = Some(fault.clone());
        memory.error_kind = Some(AccessKind::Read);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            result.exit,
            RunExit::Fault(match fault {
                MemoryError::Architectural(fault) => fault,
                _ => unreachable!(),
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn fs_base_qword_load_uses_prestate_alias_and_faults_without_commit() {
        let code = [0x64, 0x48, 0x8b, 0x00];
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd7,
            fs_base: 0x1000,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0;
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0x0706_0504_0302_0100);
        assert_eq!(state.rip, 4);
        assert_eq!(state.rflags, 0xcd7);
        assert_eq!(state.fs_base, 0x1000);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);

        let mut state = LongModeState {
            rip: 0,
            fs_base: 0x1000,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_fff0;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(crate::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            }))
        );
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());

        let mut edge_state = LongModeState {
            rip: 0,
            ..LongModeState::default()
        };
        edge_state.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_ffff;
        let before = edge_state.clone();
        let mut edge_memory = Memory::new(&code);
        assert!(matches!(
            step(&mut edge_state, &mut edge_memory, AccessContext::default()),
            Ok(StepOutcome::Fault(crate::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                ..
            }))
        ));
        assert_eq!(edge_state, before);
        assert!(edge_memory.accesses.is_empty());

        let mut state = LongModeState {
            rip: 0,
            fs_base: 0x1000,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        memory.error = Some(MemoryError::Backend("injected FS read fault".into()));
        memory.error_kind = Some(AccessKind::Read);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Backend("injected FS read fault".into()))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);

        let mut state = LongModeState {
            rip: 0,
            ..LongModeState::default()
        };
        let before = state.clone();
        let mut memory = Memory::new(&code[..3]);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            })
        ));
        assert_eq!(state, before);

        for unsupported in [
            &[0x64, 0x8b, 0x00][..],         // no effective REX.W
            &[0x64, 0x48, 0x89, 0x00],       // store
            &[0x67, 0x64, 0x48, 0x8b, 0x00], // address-size override
            &[0x64, 0x48, 0x8b, 0x08],       // other ModR/M address
            &[0x48, 0x64, 0x48, 0x8b, 0x00], // earlier ignored REX
        ] {
            let mut state = LongModeState {
                rip: 0,
                ..LongModeState::default()
            };
            let before = state.clone();
            let mut memory = Memory::new(unsupported);
            assert!(
                matches!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { .. })
                ),
                "unexpectedly accepted {unsupported:02x?}"
            );
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn gs_base_qword_load_uses_prestate_alias_and_faults_without_commit() {
        let code = [0x65, 0x48, 0x8b, 0x00];
        let mut state = LongModeState {
            rip: 0,
            rflags: 0xcd7,
            gs_base: 0x1000,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0;
        let mut memory = Memory::new(&code);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gpr[Register::Rax as usize], 0x0706_0504_0302_0100);
        assert_eq!(state.rip, 4);
        assert_eq!(state.rflags, 0xcd7);
        assert_eq!(state.gs_base, 0x1000);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);

        let mut state = LongModeState {
            rip: 0,
            gs_base: 0x1000,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0x0000_7fff_ffff_fff0;
        let before = state.clone();
        let mut memory = Memory::new(&code);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(crate::memory::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                ..
            }))
        ));
        assert_eq!(state, before);
        assert!(memory.accesses.is_empty());

        let mut state = LongModeState {
            rip: 0,
            gs_base: 0x1000,
            fs_selector: 0,
            gs_selector: 0,
            gdtr: None,
            fs_cache: None,
            gs_cache: None,
            ..LongModeState::default()
        };
        let before = state.clone();
        let mut memory = Memory::new(&code);
        memory.error = Some(MemoryError::Backend("injected GS read fault".into()));
        memory.error_kind = Some(AccessKind::Read);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Backend("injected GS read fault".into()))
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, [(AccessKind::Read, 0x1000, 8)]);
    }

    fn gs_load_state(selector: u16, limit: u16) -> LongModeState {
        let mut state = LongModeState {
            gdtr: Some(crate::state::Gdtr {
                base: 0x1000,
                limit,
            }),
            gs_base: 0xffff_8000_1234_5678,
            gs_selector: 0x33,
            fs_base: 0xffff_8000_1122_3344,
            fs_selector: 0x5678,
            fs_cache: Some(FsDescriptorCache {
                limit: 0x7654_3210,
                access: 0x9b,
                flags: 0x0a,
            }),
            gs_cache: Some(FsDescriptorCache {
                limit: 0x1234,
                access: 0xf3,
                flags: 0,
            }),
            ..LongModeState::default()
        };
        state.gpr[Register::Rax as usize] = 0xaabb_ccdd_eeff_0000 | u64::from(selector);
        state.rflags = 0x0cd7;
        state
    }

    fn gs_descriptor(base: u32, limit: u32, access: u8, flags: u8) -> [u8; 8] {
        [
            limit as u8,
            (limit >> 8) as u8,
            base as u8,
            (base >> 8) as u8,
            (base >> 16) as u8,
            access,
            ((flags & 0x0f) << 4) | ((limit >> 16) as u8 & 0x0f),
            (base >> 24) as u8,
        ]
    }

    #[test]
    fn mov_gs_ax_loads_one_cached_gdt_descriptor_and_expands_granular_limit() {
        for (flags, limit, expected_limit) in [
            (0x02, 0x000a_bcde, 0x000a_bcde),
            (0x08, 0x000a_bcde, 0xabcdefff),
        ] {
            let mut state = gs_load_state(0x0b, 15);
            let before_gprs = state.gpr;
            let before_rflags = state.rflags;
            let before_fs = (state.fs_base, state.fs_selector, state.fs_cache);
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x8123_4567, limit, 0xf3, flags));
            let before_memory = memory.bytes.clone();

            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.gpr, before_gprs);
            assert_eq!(state.rflags, before_rflags);
            assert_eq!(
                (state.fs_base, state.fs_selector, state.fs_cache),
                before_fs
            );
            assert_eq!(state.rip, 2);
            assert_eq!(state.gs_selector, 0x0b);
            assert_eq!(state.gs_base, 0x8123_4567);
            assert_eq!(
                state.gs_cache,
                Some(FsDescriptorCache {
                    limit: expected_limit,
                    access: 0xf3,
                    flags,
                })
            );
            assert_eq!(
                state.gdtr,
                Some(crate::state::Gdtr {
                    base: 0x1000,
                    limit: 15
                })
            );
            assert_eq!(memory.bytes, before_memory);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
            assert_eq!(
                memory.contexts,
                vec![(
                    AccessKind::Read,
                    AccessContext::default().descriptor_table(),
                )]
            );
            assert_eq!(memory.contexts[0].1.privilege, PrivilegeLevel::Ring3);
            assert_eq!(memory.contexts[0].1.page_privilege(), PrivilegeLevel::Ring0);
        }
    }

    #[test]
    fn mov_gs_ax_sets_only_the_accessed_byte_after_validating_an_a_zero_gs_descriptor() {
        let descriptor_context = AccessContext::default().descriptor_table();
        for access in [0xf2, 0xf0] {
            let mut state = gs_load_state(0x0b, 15);
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x8123_4567, 0xabcde, access, 0x08));
            let before = memory.bytes.clone();

            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.rip, 2);
            assert_eq!(state.gs_selector, 0x0b);
            assert_eq!(state.gs_base, 0x8123_4567);
            assert_eq!(state.gs_cache.unwrap().access, access | 1);
            assert_eq!(memory.bytes[8..13], before[8..13]);
            assert_eq!(memory.bytes[13], access | 1);
            assert_eq!(memory.bytes[14..16], before[14..16]);
            assert_eq!(
                memory.accesses,
                vec![
                    (AccessKind::Read, 0x1008, 8),
                    (AccessKind::Write, 0x100d, 1),
                ]
            );
            assert_eq!(
                memory.contexts,
                vec![
                    (AccessKind::Read, descriptor_context),
                    (AccessKind::Write, descriptor_context),
                ]
            );
            for (_, context) in memory.contexts {
                assert_eq!(context.privilege, PrivilegeLevel::Ring3);
                assert_eq!(context.origin, crate::memory::AccessOrigin::DescriptorTable);
                assert_eq!(context.page_privilege(), PrivilegeLevel::Ring0);
            }
        }
    }

    #[test]
    fn mov_gs_ax_a_one_needs_no_write_permission_or_write_call() {
        let mut state = gs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe8]);
        memory.deny_writes = true;
        memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x1234, 0xffff, 0xf3, 0));
        let before_memory = memory.bytes.clone();

        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(state.gs_selector, 0x0b);
        assert_eq!(state.gs_base, 0x1234);
        assert_eq!(memory.bytes, before_memory);
        assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        assert_eq!(memory.contexts.len(), 1);
        assert_eq!(memory.contexts[0].0, AccessKind::Read);
    }

    #[test]
    fn mov_gs_ax_accessed_write_failures_preserve_cpu_and_memory() {
        let architectural_fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(3),
            linear_address: Some(0x100d),
        });
        let backend_error = MemoryError::Backend("injected descriptor write failure".into());
        for error in [architectural_fault.clone(), backend_error.clone()] {
            let mut state = gs_load_state(0x0b, 15);
            let before_state = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x1234, 0xffff, 0xf2, 0));
            let before_memory = memory.bytes.clone();
            memory.error = Some(error.clone());
            memory.error_kind = Some(AccessKind::Write);

            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: None,
                    context: AccessContext::default(),
                },
                || false,
            );
            match error {
                MemoryError::Architectural(fault) => {
                    assert_eq!(result.exit, RunExit::Fault(fault));
                }
                other => assert_eq!(result.exit, RunExit::MemoryError(other)),
            }
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before_state);
            assert_eq!(memory.bytes, before_memory);
            assert_eq!(
                memory.accesses,
                vec![
                    (AccessKind::Read, 0x1008, 8),
                    (AccessKind::Write, 0x100d, 1),
                ]
            );
        }

        let mut read_state = gs_load_state(0x0b, 15);
        let before_state = read_state.clone();
        let mut read_memory = Memory::new(&[0x8e, 0xe8]);
        read_memory.error = Some(MemoryError::Backend("descriptor read failure".into()));
        read_memory.error_kind = Some(AccessKind::Read);
        let before_memory = read_memory.bytes.clone();
        let result = run(
            &mut read_state,
            &mut read_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Backend("descriptor read failure".into()))
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(read_state, before_state);
        assert_eq!(read_memory.bytes, before_memory);
        assert_eq!(read_memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn mov_gs_ax_validation_faults_precede_an_armed_accessed_write_failure() {
        for (access, expected_vector) in [(0x82, 13), (0x18, 13), (0x92, 13), (0x72, 11)] {
            let mut state = gs_load_state(0x0b, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            memory.bytes[8..16].copy_from_slice(&gs_descriptor(0, 0xffff, access, 0));
            memory.error = Some(MemoryError::Backend("write should not be reached".into()));
            memory.error_kind = Some(AccessKind::Write);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector: expected_vector,
                    error_code: Some(8),
                    linear_address: None,
                }))
            );
            assert_eq!(state, before);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        }
    }

    #[test]
    fn mov_gs_ax_sets_accessed_once_then_a_second_load_reads_without_writing() {
        let mut state = gs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe8, 0x8e, 0xe8]);
        memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x1234, 0xffff, 0xf2, 0));

        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, 4);
        assert_eq!(memory.bytes[13], 0xf3);
        assert_eq!(
            memory.accesses,
            vec![
                (AccessKind::Read, 0x1008, 8),
                (AccessKind::Write, 0x100d, 1),
                (AccessKind::Read, 0x1008, 8),
            ]
        );
    }

    #[test]
    fn mov_gs_ax_accessed_write_remains_committed_before_a_later_fault() {
        let mut state = gs_load_state(0x0b, 15);
        let mut memory = Memory::new(&[0x8e, 0xe8, 0x0f, 0x0b]);
        memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x8123_4567, 0xabcde, 0xf2, 0));

        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(
            result.exit,
            RunExit::Fault(ArchitecturalFault {
                vector: 6,
                error_code: None,
                linear_address: None,
            })
        );
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 2);
        assert_eq!(state.gs_selector, 0x0b);
        assert_eq!(state.gs_base, 0x8123_4567);
        assert_eq!(state.gs_cache.unwrap().access, 0xf3);
        assert_eq!(memory.bytes[13], 0xf3);
        assert_eq!(
            memory.accesses,
            vec![
                (AccessKind::Read, 0x1008, 8),
                (AccessKind::Write, 0x100d, 1),
            ]
        );
    }

    #[test]
    fn mov_gs_ax_checks_type_privilege_presence_and_accessed_in_order() {
        for (access, vector, error_code) in [
            (0x02, 13, 0x08), // system descriptor, even though not present
            (0x18, 13, 0x08), // execute-only code, even though not present
            (0x52, 13, 0x08), // CPL3/RPL3 exceeds DPL2, even though not present
            (0x93, 13, 0x08), // CPL3/RPL0 still exceeds DPL0
            (0x73, 11, 0x08), // otherwise valid, not present
        ] {
            let mut state = gs_load_state(0x0b, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            memory.bytes[8..16].copy_from_slice(&gs_descriptor(0x1000, 0xffff, access, 0));
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Fault(ArchitecturalFault {
                    vector,
                    error_code: Some(error_code),
                    linear_address: None,
                }))
            );
            assert_eq!(state, before);
            assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
        }
    }

    #[test]
    fn mov_gs_ax_bounds_and_unsupported_selectors_do_not_read_or_commit() {
        let mut short = gs_load_state(0x0b, 14);
        let before = short.clone();
        let mut memory = Memory::new(&[0x8e, 0xe8]);
        assert_eq!(
            step(&mut short, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(8),
                linear_address: None,
            }))
        );
        assert_eq!(short, before);
        assert!(memory.accesses.is_empty());

        let mut truncated = gs_load_state(0x0b, 15);
        let before = truncated.clone();
        let mut truncated_memory = Memory::new(&[0x8e]);
        assert_eq!(
            step(
                &mut truncated,
                &mut truncated_memory,
                AccessContext::default()
            ),
            Err(MemoryError::Unmapped {
                address: 1,
                access: AccessKind::Fetch,
            })
        );
        assert_eq!(truncated, before);

        for (selector, context) in [
            (0x03, AccessContext::default()), // null selector
            (0x0c, AccessContext::default()), // LDT selector
            (
                0x0b,
                AccessContext {
                    privilege: PrivilegeLevel::Ring2,
                    ..AccessContext::default()
                },
            ),
        ] {
            let mut state = gs_load_state(selector, 15);
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            assert_eq!(
                step(&mut state, &mut memory, context),
                Ok(StepOutcome::Unsupported { opcode: 0x8e })
            );
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }

        for missing in ["gdtr", "gs_cache"] {
            let mut state = gs_load_state(0x0b, 15);
            if missing == "gdtr" {
                state.gdtr = None;
            } else {
                state.gs_cache = None;
            }
            let before = state.clone();
            let mut memory = Memory::new(&[0x8e, 0xe8]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0x8e })
            );
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }
    }

    #[test]
    fn mov_gs_ax_descriptor_fault_propagates_without_state_or_retirement_commit() {
        let fault = MemoryError::Architectural(ArchitecturalFault {
            vector: 14,
            error_code: Some(0),
            linear_address: Some(0x1008),
        });
        let mut state = gs_load_state(0x0b, 15);
        let before = state.clone();
        let mut memory = Memory::new(&[0x8e, 0xe8]);
        memory.error = Some(fault.clone());
        memory.error_kind = Some(AccessKind::Read);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            result.exit,
            RunExit::Fault(match fault {
                MemoryError::Architectural(fault) => fault,
                _ => unreachable!(),
            })
        );
        assert_eq!(state, before);
        assert_eq!(memory.accesses, vec![(AccessKind::Read, 0x1008, 8)]);
    }

    #[test]
    fn fninit_resets_only_x87_environment_and_commits_on_success() {
        let mut state = LongModeState::default();
        state.rip = 0x1000;
        state.gpr = std::array::from_fn(|index| 0x1111_2222_3333_4444u64 ^ index as u64);
        state.rflags = 0xa55a_1234_5678_9abc;
        let mut x87 = crate::state::X87State::fixture_seed();
        // Seed pending and unmasked status/control bits to verify they are
        // cleared without making them an FNINIT exception or delivery path.
        x87.control_word = 0;
        x87.status_word = 0xffff;
        x87.registers = std::array::from_fn(|register| {
            std::array::from_fn(|byte| (register as u8).wrapping_mul(29).wrapping_add(byte as u8))
        });
        let payloads = x87.registers;
        state.x87 = Some(x87);
        let before = state.clone();
        let mut memory = Memory::at(&[0xdb, 0xe3], 0x1000);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x1002),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(state.rip, 0x1002);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.rflags, before.rflags);
        let x87 = state.x87.as_ref().unwrap();
        assert_eq!(x87.registers, payloads);
        assert_eq!(x87.control_word, 0x037f);
        assert_eq!(x87.status_word, 0);
        assert_eq!(x87.tag_word, 0xffff);
        assert_eq!(x87.instruction_pointer, 0);
        assert_eq!(x87.data_pointer, 0);
        assert_eq!(x87.last_opcode, 0);
    }

    fn binary80_normal(significand: u64, exponent: u16, negative: bool) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        let sign_exponent = exponent | if negative { 0x8000 } else { 0 };
        bytes[8..].copy_from_slice(&sign_exponent.to_le_bytes());
        bytes
    }

    #[test]
    fn fsubp_sti_st0_commits_destination_then_pops_for_every_top_and_index() {
        let vectors = [
            // ST(i)=1.5, ST(0)=1 -> +0.5
            (
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // ST(i)=1, ST(0)=1.5 -> -0.5
            (
                0x8000_0000_0000_0000,
                false,
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // Negative permutations and opposite signs.
            (
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            (
                0x8000_0000_0000_0000,
                true,
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            (
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x4000,
                false,
            ),
            (
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x4000,
                true,
            ),
        ];
        for (dest_sig, dest_neg, src_sig, src_neg, out_sig, out_exp, out_neg) in vectors {
            for top in 0u8..8 {
                for index in 1u8..8 {
                    let old_top = usize::from(top);
                    let physical_destination = usize::from((top + index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x2000;
                    state.gpr = std::array::from_fn(|i| 0x1122_3344_5566_7788 ^ i as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700;
                    x87.tag_word =
                        u16::MAX & !(3 << (old_top * 2)) & !(3 << (physical_destination * 2));
                    x87.registers = std::array::from_fn(|p| {
                        std::array::from_fn(|b| 0x60u8.wrapping_add((p * 13 + b) as u8))
                    });
                    x87.registers[physical_destination] =
                        binary80_normal(dest_sig, 0x3fff, dest_neg);
                    x87.registers[old_top] = binary80_normal(src_sig, 0x3fff, src_neg);
                    x87.instruction_pointer = 0x0123_4567_89ab_cdef;
                    x87.data_pointer = 0xfedc_ba98_7654_3210;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xde, 0xe8 | index], 0x2000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={index}"
                    );
                    assert_eq!(state.rip, 0x2002);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    let after = state.x87.as_ref().unwrap();
                    assert_eq!(
                        after.registers[physical_destination],
                        binary80_normal(out_sig, out_exp, out_neg)
                    );
                    for physical in 0..8 {
                        if physical != physical_destination {
                            assert_eq!(
                                after.registers[physical],
                                before.x87.as_ref().unwrap().registers[physical]
                            );
                        }
                    }
                    let mut expected_tags = before.x87.as_ref().unwrap().tag_word;
                    expected_tags |= 3 << (old_top * 2);
                    assert_eq!(after.tag_word, expected_tags);
                    assert_eq!(after.top(), top.wrapping_add(1) & 7);
                    assert_eq!(
                        after.status_word,
                        (before.x87.as_ref().unwrap().status_word & !((7 << 11) | (1 << 9)))
                            | (u16::from(top.wrapping_add(1) & 7) << 11)
                    );
                    assert_eq!(
                        after.control_word,
                        before.x87.as_ref().unwrap().control_word
                    );
                    assert_eq!(after.instruction_pointer, 0x2000);
                    assert_eq!(
                        after.data_pointer,
                        before.x87.as_ref().unwrap().data_pointer
                    );
                    assert_eq!(after.last_opcode, before.x87.as_ref().unwrap().last_opcode);
                }
            }
        }
    }

    #[test]
    fn fsubp_commits_exact_boundary_results_before_pop() {
        let vectors = [
            // (DEST, SRC, expected): +2^-63 and -2^-63.
            (
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, true),
            ),
            // Maximum even 65-bit coefficient carries to the next exponent.
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, true),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            // 3 minimum-normal units minus 2 yields minimum Normal.
            (
                binary80_normal(0xc000_0000_0000_0000, 0x0002, false),
                binary80_normal(0x8000_0000_0000_0000, 0x0002, false),
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
            ),
        ];
        for (destination_value, source_value, expected) in vectors {
            let mut state = LongModeState::default();
            state.rip = 0x2800;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (5 << 11) | (1 << 9);
            x87.tag_word = u16::MAX & !(3 << (5 * 2)) & !(3 << (6 * 2));
            x87.registers[5] = source_value;
            x87.registers[6] = destination_value;
            state.x87 = Some(x87);
            let mut memory = Memory::at(&[0xde, 0xe9], 0x2800);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            let after = state.x87.as_ref().unwrap();
            assert_eq!(after.registers[6], expected);
            assert_eq!(after.top(), 6);
            assert_eq!(after.tag_word & (3 << (5 * 2)), 3 << (5 * 2));
            assert_eq!(after.status_word & (1 << 9), 0);
            assert_eq!(after.instruction_pointer, 0x2800);
            assert_eq!(state.rip, 0x2802);
        }
    }

    #[test]
    fn fsubp_alias_and_exact_rejections_are_atomic_for_all_tops() {
        for top in 0u8..8 {
            let mut alias = LongModeState::default();
            alias.rip = 0x3000;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            alias.x87 = Some(x87);
            let before = alias.clone();
            let mut memory = Memory::at(&[0xde, 0xe8], 0x3000);
            assert_eq!(
                step(&mut alias, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(alias, before, "alias TOP={top}");
        }

        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let make_x87 = || {
            let mut x = crate::state::X87State::fixture_seed();
            x.control_word = 0x037f;
            x.status_word = 0;
            x.tag_word = 0;
            x.registers[0] = one;
            x.registers[1] = one;
            x
        };
        let mut rejects = Vec::new();
        let mut bad = make_x87();
        bad.control_word = 0x027f;
        rejects.push(bad);
        for sw in [1, 0x8000] {
            let mut bad = make_x87();
            bad.status_word = sw;
            rejects.push(bad);
        }
        for slot in [0, 1] {
            let mut bad = make_x87();
            bad.tag_word = 3 << (slot * 2);
            rejects.push(bad);
        }
        let mut unequal = make_x87();
        unequal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejects.push(unequal);
        for raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            let mut d = make_x87();
            d.registers[1] = raw;
            rejects.push(d);
            let mut s = make_x87();
            s.registers[0] = raw;
            rejects.push(s);
        }
        let mut cancel = make_x87();
        cancel.registers[1] = one;
        rejects.push(cancel);
        let mut odd = make_x87();
        odd.registers[1] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        odd.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        rejects.push(odd);
        let mut subnormal = make_x87();
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0001, 1, false);
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        rejects.push(subnormal);
        let mut overflow = make_x87();
        overflow.registers[1] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, true);
        rejects.push(overflow);
        for x87 in rejects {
            let mut state = LongModeState::default();
            state.rip = 0x4000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xe9], 0x4000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }
        let mut missing = LongModeState::default();
        missing.rip = 0x4000;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xde, 0xe9], 0x4000);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(missing, before);
    }

    #[test]
    fn fsubp_fetch_fault_stop_and_zero_budget_do_not_commit() {
        let mut state = LongModeState::default();
        state.rip = 0x5000;
        let mut x = crate::state::X87State::fixture_seed();
        x.control_word = 0x037f;
        x.status_word = 0x0200;
        x.tag_word = 0;
        x.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        x.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        state.x87 = Some(x);
        for (bytes, stop, budget) in [
            (&[0xde][..], None, 1),
            (&[0xde, 0xe9][..], Some(0x5000), 1),
            (&[0xde, 0xe9][..], None, 0),
        ] {
            let mut current = state.clone();
            let before = current.clone();
            let mut memory = Memory::at(bytes, 0x5000);
            let result = run(
                &mut current,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(current, before);
            if bytes.len() == 1 {
                assert!(matches!(result.exit, RunExit::MemoryError(_)));
            } else if stop.is_some() {
                assert_eq!(result.exit, RunExit::StopAddress);
            } else {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            }
        }
    }

    #[test]
    fn fadd_st0_st1_commits_exact_normal_sum_and_only_defined_state() {
        for (top, source_index, lhs, rhs, expected) in [
            (
                0,
                1,
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ),
            (
                0,
                1,
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
            ),
            (
                0,
                1,
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            // TOP=7, i=1 wraps the logical source to physical register 0.
            (
                7,
                1,
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x4000, false),
            ),
            (
                0,
                1,
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            (
                3,
                0,
                binary80_normal(0xc000_0000_0000_0001, 0x4000, true),
                binary80_normal(0xc000_0000_0000_0001, 0x4000, true),
                binary80_normal(0xc000_0000_0000_0001, 0x4001, true),
            ),
            // TOP=7, i=7 wraps to physical register 6; only i=0 aliases TOP.
            (
                7,
                7,
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            (
                4,
                2,
                binary80_normal(0x8000_0000_0000_0000, 0x4001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4002, false),
            ),
            (
                5,
                3,
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
            ),
            (
                6,
                4,
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x4000, false),
            ),
            (
                2,
                5,
                binary80_normal(0xa000_0000_0000_0000, 0x4000, false),
                binary80_normal(0xa000_0000_0000_0000, 0x4000, false),
                binary80_normal(0xa000_0000_0000_0000, 0x4001, false),
            ),
            (
                1,
                6,
                binary80_normal(0xe000_0000_0000_0000, 0x3ffe, true),
                binary80_normal(0xe000_0000_0000_0000, 0x3ffe, true),
                binary80_normal(0xe000_0000_0000_0000, 0x3fff, true),
            ),
        ] {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = std::array::from_fn(|index| 0xa1b2_c3d4_e5f6_0718 ^ index as u64);
            state.rflags = 0xa55a_1234_5678_9abc;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x4700 | 0x0200;
            // The alias form needs only its one used physical register to be Valid.
            x87.tag_word = if source_index == 0 {
                u16::MAX & !(3 << (top * 2))
            } else {
                0
            };
            x87.instruction_pointer = 0x0123_4567_89ab_cdef;
            x87.data_pointer = 0xfedc_ba98_7654_3210;
            x87.last_opcode = 0x05aa;
            let destination = usize::from(top);
            let source = usize::from((top + source_index) & 7);
            x87.registers[destination] = lhs;
            x87.registers[source] = rhs;
            state.x87 = Some(x87.clone());
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xc0 | source_index], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.rip, 0x1002);
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
            let result = state.x87.as_ref().unwrap();
            assert_eq!(result.registers[destination], expected);
            for index in 0..8 {
                if index != destination {
                    assert_eq!(
                        result.registers[index],
                        before.x87.as_ref().unwrap().registers[index]
                    );
                }
            }
            assert_eq!(result.control_word, 0x037f);
            assert_eq!(result.tag_word, before.x87.as_ref().unwrap().tag_word);
            assert_eq!(result.top(), top);
            assert_eq!(
                result.status_word,
                before.x87.as_ref().unwrap().status_word & !(1 << 9)
            );
            assert_eq!(result.status_word & 0x4500, 0x4500);
            assert_eq!(result.instruction_pointer, 0x1000);
            assert_eq!(
                result.data_pointer,
                before.x87.as_ref().unwrap().data_pointer
            );
            assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);
        }
    }

    #[test]
    fn fadd_st0_st1_rejects_unsupported_inputs_without_any_state_commit() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let mut absent = LongModeState::default();
        let before = absent.clone();
        let mut memory = Memory::at(&[0xd8, 0xc1], 0);
        assert_eq!(
            step(&mut absent, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(absent, before);

        let mut cases = Vec::new();
        let mut add_case = |mut change: Box<dyn FnMut(&mut crate::state::X87State)>| {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = normal;
            x87.registers[1] = normal;
            change(&mut x87);
            cases.push(x87);
        };
        add_case(Box::new(|x| {
            x.registers[1] = binary80_normal(u64::MAX, 0x3fff, false)
        }));
        add_case(Box::new(|x| {
            x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true)
        }));
        add_case(Box::new(|x| {
            x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false)
        }));
        add_case(Box::new(|x| {
            x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x7fff, false)
        }));
        add_case(Box::new(|x| {
            x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0, false)
        }));
        add_case(Box::new(|x| x.registers[1] = [0; 10]));
        add_case(Box::new(|x| x.registers[1] = binary80_normal(1, 0, false)));
        add_case(Box::new(|x| x.registers[1] = binary80_normal(0, 1, false)));
        add_case(Box::new(|x| x.tag_word = 3));
        add_case(Box::new(|x| x.tag_word = 2));
        add_case(Box::new(|x| x.control_word = 0x027f));
        add_case(Box::new(|x| x.status_word = 1));
        add_case(Box::new(|x| x.status_word = 0x8000));
        add_case(Box::new(|x| {
            x.registers[0] = binary80_normal(0x8000_0000_0000_0001, 1, false);
            x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 1, true);
        }));
        add_case(Box::new(|x| {
            x.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
            x.registers[1] = binary80_normal(u64::MAX, 0x7ffe, false);
        }));

        for x87 in cases {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xc1], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }

        for source_index in 0u8..=7 {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = normal;
            x87.registers[usize::from(source_index)] = normal;
            let top = x87.top();
            let bad_tag_slot = usize::from((top + source_index) & 7);
            x87.tag_word |= 3 << (bad_tag_slot * 2);
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xc0 | source_index], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before, "source index {source_index}");
        }
    }

    #[test]
    fn fsub_st0_sti_commits_literal_exact_results_for_all_top_and_indices() {
        let vectors = [
            // 1.5 - 1 = 0.5
            (
                0xc000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // 1 - 1.5 = -0.5
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0xc000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // -1.5 - (-1) = -0.5
            (
                0xc000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // -1 - (-1.5) = 0.5
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0xc000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // 1 - (-1) = 2
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x4000,
                false,
            ),
            // -1 - 1 = -2
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x4000,
                true,
            ),
            // 1.0000000000000002 - 1 = 2^-63
            (
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fc0,
                false,
            ),
            // 1 - 1.0000000000000002 = -2^-63
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fc0,
                true,
            ),
            // max coefficient - negative max coefficient = max coefficient * 2
            (
                u64::MAX,
                0x3fff,
                false,
                u64::MAX,
                0x3fff,
                true,
                u64::MAX,
                0x4000,
                false,
            ),
            // 3 * minimum normal - 2 * minimum normal = minimum normal
            (
                0xc000_0000_0000_0000,
                0x0002,
                false,
                0x8000_0000_0000_0000,
                0x0002,
                false,
                0x8000_0000_0000_0000,
                0x0001,
                false,
            ),
        ];
        for (lhs_sig, lhs_exp, lhs_neg, rhs_sig, rhs_exp, rhs_neg, out_sig, out_exp, out_neg) in
            vectors
        {
            let lhs = binary80_normal(lhs_sig, lhs_exp, lhs_neg);
            let rhs = binary80_normal(rhs_sig, rhs_exp, rhs_neg);
            let expected = binary80_normal(out_sig, out_exp, out_neg);
            for top in 0u8..8 {
                for source_index in 1u8..8 {
                    let mut state = LongModeState::default();
                    state.rip = 0x2000;
                    state.gpr = std::array::from_fn(|index| 0x1122_3344_5566_7788 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let destination = usize::from(top);
                    let source = usize::from((top + source_index) & 7);
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700;
                    x87.tag_word = u16::MAX & !(3 << (destination * 2)) & !(3 << (source * 2));
                    x87.registers = std::array::from_fn(|physical| {
                        std::array::from_fn(|byte| {
                            0x60u8.wrapping_add((physical * 13 + byte) as u8)
                        })
                    });
                    x87.registers[destination] = lhs;
                    x87.registers[source] = rhs;
                    x87.instruction_pointer = 0x0123_4567_89ab_cdef;
                    x87.data_pointer = 0xfedc_ba98_7654_3210;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xd8, 0xe0 | source_index], 0x2000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={source_index}"
                    );
                    assert_eq!(state.rip, 0x2002);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    let result = state.x87.as_ref().unwrap();
                    assert_eq!(result.registers[destination], expected);
                    for physical in 0..8 {
                        if physical == destination {
                            continue;
                        }
                        assert_eq!(
                            result.registers[physical],
                            before.x87.as_ref().unwrap().registers[physical]
                        );
                    }
                    assert_eq!(result.control_word, 0x037f);
                    assert_eq!(result.tag_word, before.x87.as_ref().unwrap().tag_word);
                    assert_eq!(result.top(), top);
                    assert_eq!(
                        result.status_word,
                        before.x87.as_ref().unwrap().status_word & !(1 << 9)
                    );
                    assert_eq!(result.instruction_pointer, 0x2000);
                    assert_eq!(
                        result.data_pointer,
                        before.x87.as_ref().unwrap().data_pointer
                    );
                    assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);
                }
            }
        }
    }

    #[test]
    fn fsubr_st0_sti_commits_sti_minus_st0_to_old_top_for_every_top_and_index() {
        let vectors = [
            // ST(i)=1.5, ST(0)=1 -> +0.5.
            (
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // ST(i)=1, ST(0)=1.5 -> -0.5.
            (
                0x8000_0000_0000_0000,
                false,
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // ST(i)=-1, ST(0)=-1.5 -> +0.5.
            (
                0x8000_0000_0000_0000,
                true,
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // ST(i)=-1.5, ST(0)=-1 -> -0.5.
            (
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // ST(i)=1.5, ST(0)=-1 -> +2.5.
            (
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                true,
                0xa000_0000_0000_0000,
                0x4000,
                false,
            ),
            // ST(i)=-1.5, ST(0)=1 -> -2.5.
            (
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                false,
                0xa000_0000_0000_0000,
                0x4000,
                true,
            ),
        ];
        for (source_sig, source_negative, top_sig, top_negative, out_sig, out_exp, out_negative) in
            vectors
        {
            for top in 0u8..8 {
                for source_index in 1u8..8 {
                    let destination = usize::from(top);
                    let source_phys = usize::from((top + source_index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x2000;
                    state.gpr = std::array::from_fn(|index| 0x1122_3344_5566_7788 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700;
                    x87.tag_word = u16::MAX & !(3 << (destination * 2)) & !(3 << (source_phys * 2));
                    x87.registers = std::array::from_fn(|physical| {
                        std::array::from_fn(|byte| {
                            0x60u8.wrapping_add((physical * 13 + byte) as u8)
                        })
                    });
                    x87.registers[destination] = binary80_normal(top_sig, 0x3fff, top_negative);
                    x87.registers[source_phys] =
                        binary80_normal(source_sig, 0x3fff, source_negative);
                    x87.instruction_pointer = 0x0123_4567_89ab_cdef;
                    x87.data_pointer = 0xfedc_ba98_7654_3210;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xd8, 0xe8 | source_index], 0x2000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={source_index}"
                    );
                    assert_eq!(state.rip, 0x2002);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    let result = state.x87.as_ref().unwrap();
                    assert_eq!(
                        result.registers[destination],
                        binary80_normal(out_sig, out_exp, out_negative)
                    );
                    for physical in 0..8 {
                        if physical != destination {
                            assert_eq!(
                                result.registers[physical],
                                before.x87.as_ref().unwrap().registers[physical]
                            );
                        }
                    }
                    assert_eq!(result.control_word, 0x037f);
                    assert_eq!(result.tag_word, before.x87.as_ref().unwrap().tag_word);
                    assert_eq!(result.top(), top);
                    assert_eq!(
                        result.status_word,
                        before.x87.as_ref().unwrap().status_word & !(1 << 9)
                    );
                    assert_eq!(result.instruction_pointer, 0x2000);
                    assert_eq!(
                        result.data_pointer,
                        before.x87.as_ref().unwrap().data_pointer
                    );
                    assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);
                    let mut expected_state = before.clone();
                    let expected_x87 = expected_state.x87.as_mut().unwrap();
                    expected_x87.registers[destination] =
                        binary80_normal(out_sig, out_exp, out_negative);
                    expected_x87.status_word &= !(1 << 9);
                    expected_x87.instruction_pointer = before.rip;
                    expected_state.rip = before.rip + 2;
                    assert_eq!(
                        state, expected_state,
                        "complete state TOP={top}, i={source_index}"
                    );
                }
            }
        }
    }

    #[test]
    fn fsubr_st0_sti_accepts_exact_boundaries_and_rejects_domains_atomically() {
        for (source, destination, expected) in [
            (
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, true),
            ),
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, true),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 1, false),
            ),
        ] {
            for top in 0u8..8 {
                for source_index in 1u8..8 {
                    let source_phys = usize::from((top + source_index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x3000;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = u16::from(top) << 11;
                    x87.tag_word =
                        u16::MAX & !(3 << (usize::from(top) * 2)) & !(3 << (source_phys * 2));
                    x87.registers[usize::from(top)] = destination;
                    x87.registers[source_phys] = source;
                    state.x87 = Some(x87);
                    let mut memory = Memory::at(&[0xd8, 0xe8 | source_index], 0x3000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(
                        state.x87.as_ref().unwrap().registers[usize::from(top)],
                        expected
                    );
                }
            }
        }

        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            let mut state = LongModeState::default();
            state.rip = 0x4000;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = one;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xe8], 0x4000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before, "TOP={top}");
        }

        let mut base = crate::state::X87State::fixture_seed();
        base.control_word = 0x037f;
        base.status_word = 0;
        base.tag_word = 0;
        base.registers[0] = one;
        base.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        let mut rejected_states = Vec::new();
        let mut bad = base.clone();
        bad.control_word = 0x027f;
        rejected_states.push(bad);
        for status_word in [1, 0x8000] {
            let mut bad = base.clone();
            bad.status_word = status_word;
            rejected_states.push(bad);
        }
        for slot in [0, 1] {
            let mut bad = base.clone();
            bad.tag_word |= 3 << (slot * 2);
            rejected_states.push(bad);
        }
        let mut unequal = base.clone();
        unequal.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        rejected_states.push(unequal);
        for raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            let mut bad = base.clone();
            bad.registers[1] = raw;
            rejected_states.push(bad);
        }
        let mut odd = base.clone();
        odd.registers[1] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        odd.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        rejected_states.push(odd);
        let mut cancellation = base.clone();
        cancellation.registers[1] = one;
        rejected_states.push(cancellation);
        let mut subnormal = base.clone();
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0001, 1, false);
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        rejected_states.push(subnormal);
        let mut overflow = base.clone();
        overflow.registers[1] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, true);
        rejected_states.push(overflow);
        for x87 in rejected_states {
            let mut state = LongModeState::default();
            state.rip = 0x5000;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xe9], 0x5000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }
        let mut missing = LongModeState::default();
        missing.rip = 0x5000;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xd8, 0xe9], 0x5000);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(missing, before);
    }

    #[test]
    fn fsubr_st0_sti_fetch_stop_and_budget_do_not_commit() {
        let mut initial = LongModeState::default();
        initial.rip = 0x6000;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.tag_word = 0;
        x87.registers[0] = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        x87.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        initial.x87 = Some(x87);
        let mut fault_state = initial.clone();
        let before_fault = fault_state.clone();
        let mut fault_memory = Memory::at(&[0xd8], 0x6000);
        let result = run(
            &mut fault_state,
            &mut fault_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(result.exit, RunExit::MemoryError(_)));
        assert_eq!(fault_state, before_fault);
        let mut stopped = initial.clone();
        let before_stop = stopped.clone();
        let mut stop_memory = Memory::at(&[0xd8, 0xe9], 0x6000);
        let result = run(
            &mut stopped,
            &mut stop_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x6000),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(stopped, before_stop);
        let mut budget = initial;
        let before_budget = budget.clone();
        let mut budget_memory = Memory::at(&[0xd8, 0xe9], 0x6000);
        let result = run(
            &mut budget,
            &mut budget_memory,
            RunConfig {
                instruction_budget: 0,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(budget, before_budget);
    }

    #[test]
    fn fsub_st0_sti_rejects_i0_and_unsupported_domains_atomically() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = one;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xe0], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before, "TOP={top}");
        }

        let mut invalid_states = Vec::new();
        let make_state = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = one;
            x87.registers[1] = one;
            x87
        };
        invalid_states.push(make_state());
        let mut x = make_state();
        x.control_word = 0x027f;
        invalid_states.push(x);
        let mut x = make_state();
        x.status_word = 1;
        invalid_states.push(x);
        let mut x = make_state();
        x.status_word = 0x8000;
        invalid_states.push(x);
        let mut x = make_state();
        x.tag_word = 3;
        invalid_states.push(x);
        let mut x = make_state();
        x.tag_word = 2;
        invalid_states.push(x);
        let mut x = make_state();
        x.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        invalid_states.push(x);
        for bad in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0000, 1, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            let mut x = make_state();
            x.registers[1] = bad;
            invalid_states.push(x);
        }
        let mut odd_65 = make_state();
        odd_65.registers[0] = one;
        odd_65.registers[1] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, true);
        invalid_states.push(odd_65);
        let mut overflow = make_state();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(u64::MAX, 0x7ffe, true);
        invalid_states.push(overflow);
        let mut subnormal_result = make_state();
        subnormal_result.registers[0] = binary80_normal(0xc000_0000_0000_0000, 1, false);
        subnormal_result.registers[1] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        invalid_states.push(subnormal_result);

        for x87 in invalid_states {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xe1], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }

        let mut absent = LongModeState::default();
        let absent_before = absent.clone();
        let mut memory = Memory::at(&[0xd8, 0xe1], 0);
        assert_eq!(
            step(&mut absent, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(absent, absent_before);
    }

    #[test]
    fn fsub_sti_st0_commits_exact_results_to_only_the_physical_destination() {
        let vectors = [
            // ST(i)=1.5, ST(0)=1 -> 0.5
            (
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // ST(i)=1, ST(0)=1.5 -> -0.5
            (
                0x8000_0000_0000_0000,
                false,
                0xc000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // ST(i)=-1.5, ST(0)=-1 -> -0.5
            (
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
            ),
            // ST(i)=-1, ST(0)=-1.5 -> 0.5
            (
                0x8000_0000_0000_0000,
                true,
                0xc000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
            ),
            // ST(i)=-1, ST(0)=1 -> -2
            (
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                0x4000,
                true,
            ),
            // ST(i)=1, ST(0)=-1 -> 2
            (
                0x8000_0000_0000_0000,
                false,
                0x8000_0000_0000_0000,
                true,
                0x8000_0000_0000_0000,
                0x4000,
                false,
            ),
        ];
        for (
            destination_sig,
            destination_negative,
            source_sig,
            source_negative,
            out_sig,
            out_exp,
            out_negative,
        ) in vectors
        {
            let destination_value = binary80_normal(destination_sig, 0x3fff, destination_negative);
            let source_value = binary80_normal(source_sig, 0x3fff, source_negative);
            let expected = binary80_normal(out_sig, out_exp, out_negative);
            for top in 0u8..8 {
                for destination_index in 1u8..8 {
                    let source = usize::from(top);
                    let destination = usize::from((top + destination_index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x2000;
                    state.gpr = std::array::from_fn(|index| 0x1122_3344_5566_7788 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700;
                    x87.tag_word = u16::MAX & !(3 << (source * 2)) & !(3 << (destination * 2));
                    x87.registers = std::array::from_fn(|physical| {
                        std::array::from_fn(|byte| {
                            0x60u8.wrapping_add((physical * 13 + byte) as u8)
                        })
                    });
                    x87.registers[destination] = destination_value;
                    x87.registers[source] = source_value;
                    x87.instruction_pointer = 0x0123_4567_89ab_cdef;
                    x87.data_pointer = 0xfedc_ba98_7654_3210;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xdc, 0xe8 | destination_index], 0x2000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={destination_index}"
                    );
                    assert_eq!(state.rip, 0x2002);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    let result = state.x87.as_ref().unwrap();
                    assert_eq!(result.registers[destination], expected);
                    assert_eq!(
                        result.registers[source],
                        before.x87.as_ref().unwrap().registers[source]
                    );
                    for physical in 0..8 {
                        if physical != destination {
                            assert_eq!(
                                result.registers[physical],
                                before.x87.as_ref().unwrap().registers[physical]
                            );
                        }
                    }
                    assert_eq!(result.control_word, 0x037f);
                    assert_eq!(result.tag_word, before.x87.as_ref().unwrap().tag_word);
                    assert_eq!(result.top(), top);
                    assert_eq!(
                        result.status_word,
                        before.x87.as_ref().unwrap().status_word & !(1 << 9)
                    );
                    assert_eq!(result.instruction_pointer, 0x2000);
                    assert_eq!(
                        result.data_pointer,
                        before.x87.as_ref().unwrap().data_pointer
                    );
                    assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);
                }
            }
        }
    }

    #[test]
    fn fsub_sti_st0_alias_and_unsupported_state_reject_atomically() {
        for top in 0u8..8 {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xe8], 0x1000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before, "TOP={top}");
        }

        let mut invalid = LongModeState::default();
        invalid.rip = 0x1000;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.tag_word = 0;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        invalid.x87 = Some(x87);
        let before = invalid.clone();
        let mut memory = Memory::at(&[0xdc, 0xe9], 0x1000);
        assert_eq!(
            step(&mut invalid, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdc })
        );
        assert_eq!(invalid, before);
    }

    #[test]
    fn fsub_sti_st0_accepts_exact_boundary_results_for_every_top_and_index() {
        let vectors = [
            // (DEST,SRC)=(1+ulp,1) -> +2^-63.
            (
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fc0,
                false,
            ),
            // (DEST,SRC)=(1,1+ulp) -> -2^-63.
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fc0,
                true,
            ),
            // Maximum even 65-bit exact coefficient packs to maximum significand.
            (
                u64::MAX,
                0x3fff,
                false,
                u64::MAX,
                0x3fff,
                true,
                u64::MAX,
                0x4000,
                false,
            ),
            // Three minimum-normal units minus two yields minimum Normal.
            (
                0xc000_0000_0000_0000,
                0x0002,
                false,
                0x8000_0000_0000_0000,
                0x0002,
                false,
                0x8000_0000_0000_0000,
                0x0001,
                false,
            ),
        ];
        for (dest_sig, dest_exp, dest_neg, src_sig, src_exp, src_neg, out_sig, out_exp, out_neg) in
            vectors
        {
            for top in 0u8..8 {
                for destination_index in 1u8..8 {
                    let source = usize::from(top);
                    let destination = usize::from((top + destination_index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x3000;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = u16::from(top) << 11;
                    x87.tag_word = u16::MAX & !(3 << (source * 2)) & !(3 << (destination * 2));
                    x87.registers[source] = binary80_normal(src_sig, src_exp, src_neg);
                    x87.registers[destination] = binary80_normal(dest_sig, dest_exp, dest_neg);
                    state.x87 = Some(x87);
                    let mut memory = Memory::at(&[0xdc, 0xe8 | destination_index], 0x3000);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={destination_index}"
                    );
                    assert_eq!(
                        state.x87.as_ref().unwrap().registers[destination],
                        binary80_normal(out_sig, out_exp, out_neg),
                        "TOP={top}, i={destination_index}"
                    );
                }
            }
        }
    }

    #[test]
    fn fsub_sti_st0_rejects_unsupported_operands_and_environment_atomically() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let make_x87 = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = one;
            x87.registers[1] = one;
            x87
        };
        let mut rejected_states = Vec::new();
        let mut bad = make_x87();
        bad.control_word = 0x027f;
        rejected_states.push(bad);
        for status_word in [1, 0x8000] {
            let mut bad = make_x87();
            bad.status_word = status_word;
            rejected_states.push(bad);
        }
        for invalid_tag_slot in [0, 1] {
            let mut bad = make_x87();
            bad.tag_word = 3 << (invalid_tag_slot * 2);
            rejected_states.push(bad);
        }
        let mut unequal_exponents = make_x87();
        unequal_exponents.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejected_states.push(unequal_exponents);

        for invalid_raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            let mut bad_destination = make_x87();
            bad_destination.registers[1] = invalid_raw;
            rejected_states.push(bad_destination);
            let mut bad_source = make_x87();
            bad_source.registers[0] = invalid_raw;
            rejected_states.push(bad_source);
        }

        let mut cancellation = make_x87();
        cancellation.registers[1] = one;
        rejected_states.push(cancellation);
        let mut odd_65_bit = make_x87();
        odd_65_bit.registers[1] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        odd_65_bit.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        rejected_states.push(odd_65_bit);
        let mut exact_subnormal = make_x87();
        exact_subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0001, 1, false);
        exact_subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        rejected_states.push(exact_subnormal);
        let mut overflow = make_x87();
        overflow.registers[1] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, true);
        rejected_states.push(overflow);

        for x87 in rejected_states {
            let mut state = LongModeState::default();
            state.rip = 0x4000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xe9], 0x4000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x4000;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xdc, 0xe9], 0x4000);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdc })
        );
        assert_eq!(missing, before);
    }

    #[test]
    fn fsub_sti_st0_fetch_stop_and_budget_do_not_commit() {
        let mut initial = LongModeState::default();
        initial.rip = 0x5000;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x0200;
        x87.tag_word = 0;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        x87.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        initial.x87 = Some(x87);

        let mut fault_state = initial.clone();
        let before_fault = fault_state.clone();
        let mut fault_memory = Memory::at(&[0xdc], 0x5000);
        let result = run(
            &mut fault_state,
            &mut fault_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(result.exit, RunExit::MemoryError(_)));
        assert_eq!(fault_state, before_fault);

        let mut stopped = initial.clone();
        let before_stop = stopped.clone();
        let mut stop_memory = Memory::at(&[0xdc, 0xe9], 0x5000);
        let result = run(
            &mut stopped,
            &mut stop_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x5000),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(stopped, before_stop);

        let mut budget = initial;
        let before_budget = budget.clone();
        let mut budget_memory = Memory::at(&[0xdc, 0xe9], 0x5000);
        let result = run(
            &mut budget,
            &mut budget_memory,
            RunConfig {
                instruction_budget: 0,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(budget, before_budget);
    }

    #[test]
    fn fsubr_sti_st0_writes_st0_minus_sti_without_popping_for_all_physical_slots() {
        let vectors = [
            (
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, true),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, true),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
            ),
        ];
        for (st0_value, sti_value, expected) in vectors {
            for top in 0u8..8 {
                for index in 1u8..8 {
                    let source = usize::from(top);
                    let destination = usize::from((top + index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x6100;
                    state.gpr = std::array::from_fn(|n| 0x1234_5678_9abc_def0 ^ n as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                    x87.tag_word = u16::MAX & !(3 << (source * 2)) & !(3 << (destination * 2));
                    x87.registers = std::array::from_fn(|physical| {
                        std::array::from_fn(|byte| (0x30 + physical * 7 + byte) as u8)
                    });
                    x87.registers[source] = st0_value;
                    x87.registers[destination] = sti_value;
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xdc, 0xe0 | index], 0x6100);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    let after = state.x87.as_ref().unwrap();
                    assert_eq!(state.rip, 0x6102);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    assert_eq!(after.registers[destination], expected);
                    assert_eq!(
                        after.registers[source],
                        before.x87.as_ref().unwrap().registers[source]
                    );
                    for slot in 0..8 {
                        if slot != destination {
                            assert_eq!(
                                after.registers[slot],
                                before.x87.as_ref().unwrap().registers[slot]
                            );
                        }
                    }
                    assert_eq!(after.top(), top);
                    assert_eq!(after.tag_word, before.x87.as_ref().unwrap().tag_word);
                    assert_eq!(
                        after.status_word,
                        before.x87.as_ref().unwrap().status_word & !(1 << 9)
                    );
                    assert_eq!(after.instruction_pointer, 0x6100);
                    assert_eq!(after.data_pointer, 0x5678);
                    assert_eq!(after.last_opcode, 0x05aa);
                }
            }
        }
    }

    #[test]
    fn fsubr_sti_st0_boundaries_rejects_and_execution_boundaries_are_atomic() {
        for (st0, sti, expected) in [
            (
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, true),
            ),
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, true),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 1, false),
            ),
        ] {
            let mut state = LongModeState::default();
            state.rip = 0x6200;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = st0;
            x87.registers[1] = sti;
            state.x87 = Some(x87);
            let mut memory = Memory::at(&[0xdc, 0xe1], 0x6200);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[1], expected);
        }

        for top in 0u8..8 {
            let mut state = LongModeState::default();
            state.rip = 0x6300;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = u16::from(top) << 11;
            x87.tag_word = 0;
            x87.registers[usize::from(top)] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xe0], 0x6300);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }

        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
            x87.registers[1] = one;
            x87
        };
        let mut rejected = Vec::new();
        let mut cancellation = make_pair();
        cancellation.registers[0] = one;
        rejected.push(cancellation); // exact cancellation for non-aliased ST(1).
        let mut bad = make_pair();
        bad.control_word = 0x027f;
        rejected.push(bad);
        let mut bad = make_pair();
        bad.status_word = 1;
        rejected.push(bad);
        let mut bad = make_pair();
        bad.status_word = 0x8000;
        rejected.push(bad);
        let mut bad = make_pair();
        bad.tag_word = 3 << 2;
        rejected.push(bad);
        let mut bad = make_pair();
        bad.tag_word = 3;
        rejected.push(bad);
        let mut bad = make_pair();
        bad.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejected.push(bad);
        for invalid_raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            for slot in 0..=1 {
                let mut bad = make_pair();
                bad.registers[slot] = invalid_raw;
                rejected.push(bad);
            }
        }
        let mut bad = make_pair();
        bad.registers[0] = binary80_normal(0x8000_0000_0000_0001, 1, false);
        bad.registers[1] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        rejected.push(bad);
        let mut bad = make_pair();
        bad.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        bad.registers[1] = binary80_normal(u64::MAX, 0x7ffe, true);
        rejected.push(bad);
        let mut odd_65_bit = make_pair();
        odd_65_bit.registers[0] = binary80_normal(u64::MAX, 0x3fff, false);
        odd_65_bit.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        rejected.push(odd_65_bit);
        for x87 in rejected {
            let mut state = LongModeState::default();
            state.rip = 0x6350;
            state.x87 = Some(x87);
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xe1], 0x6350);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }
        let mut missing = LongModeState::default();
        missing.rip = 0x6350;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xdc, 0xe1], 0x6350);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdc })
        );
        assert_eq!(missing, before);
        let mut initial = LongModeState::default();
        initial.rip = 0x6400;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.tag_word = 0;
        x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        initial.x87 = Some(x87);
        for (bytes, stop, budget) in [
            (&[0xdc][..], None, 1),
            (&[0xdc, 0xe1][..], Some(0x6400), 1),
            (&[0xdc, 0xe1][..], None, 0),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, 0x6400);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fsubrp_sti_st0_writes_old_sti_then_pops_for_every_top_and_index() {
        let vectors = [
            (
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, true),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, true),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
            ),
            // ±2^-63 exact residuals exercise source/destination significance order.
            (
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fc0, true),
            ),
            // The largest accepted even carry and minimum Normal output.
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                binary80_normal(u64::MAX, 0x3fff, true),
                binary80_normal(u64::MAX, 0x4000, false),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 2, false),
                binary80_normal(0x8000_0000_0000_0000, 1, false),
            ),
        ];
        for (st0_value, sti_value, expected_value) in vectors {
            for top in 0u8..8 {
                for index in 1u8..8 {
                    let old_top = usize::from(top);
                    let old_destination = usize::from((top + index) & 7);
                    let mut state = LongModeState::default();
                    state.rip = 0x7100;
                    state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                    x87.tag_word = u16::MAX & !(3 << (old_top * 2)) & !(3 << (old_destination * 2));
                    x87.registers = std::array::from_fn(|physical| {
                        std::array::from_fn(|byte| (0x30 + physical * 7 + byte) as u8)
                    });
                    x87.registers[old_top] = st0_value;
                    x87.registers[old_destination] = sti_value;
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xde, 0xe0 | index], 0x7100);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={index}"
                    );
                    let result = state.x87.as_ref().unwrap();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.registers[old_destination] = expected_value;
                    expected_x87.set_tag(old_top as u8, X87Tag::Empty);
                    let new_top = top.wrapping_add(1) & 7;
                    expected_x87.status_word = (expected_x87.status_word & !((7 << 11) | (1 << 9)))
                        | (u16::from(new_top) << 11);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;
                    assert_eq!(state, expected, "complete state TOP={top}, i={index}");
                    assert_eq!(
                        result.registers[old_top],
                        before.x87.as_ref().unwrap().registers[old_top]
                    );
                    assert_eq!(result.tag(old_top as u8), Some(X87Tag::Empty));
                }
            }
        }
    }

    #[test]
    fn fsubrp_sti_st0_rejects_alias_domains_and_execution_boundaries_atomically() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            let mut state = LongModeState::default();
            state.rip = 0x7200;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x0200;
            x87.tag_word = u16::MAX & !(3 << (usize::from(top) * 2));
            x87.registers[usize::from(top)] = one;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xe0], 0x7200);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before, "alias TOP={top}");
        }

        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
            x87.registers[1] = one;
            x87
        };
        let mut rejected = Vec::new();
        for dirty_cw in [0x027f, 0x037e, 0x077f] {
            let mut bad = make_pair();
            bad.control_word = dirty_cw;
            rejected.push(bad);
        }
        for dirty_sw in [1, 0x80, 0x8000] {
            let mut bad = make_pair();
            bad.status_word = dirty_sw;
            rejected.push(bad);
        }
        for physical in [0, 1] {
            let mut bad = make_pair();
            bad.set_tag(physical, X87Tag::Empty);
            rejected.push(bad);
            let mut bad = make_pair();
            bad.set_tag(physical, X87Tag::Special);
            rejected.push(bad);
        }
        let mut unequal = make_pair();
        unequal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejected.push(unequal);
        for invalid_raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            for slot in [0, 1] {
                let mut bad = make_pair();
                bad.registers[slot] = invalid_raw;
                rejected.push(bad);
            }
        }
        let mut cancellation = make_pair();
        cancellation.registers[1] = cancellation.registers[0];
        rejected.push(cancellation);
        let mut odd_65_bit = make_pair();
        odd_65_bit.registers[0] = binary80_normal(u64::MAX, 0x3fff, false);
        odd_65_bit.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        rejected.push(odd_65_bit);
        let mut subnormal_result = make_pair();
        subnormal_result.registers[0] = binary80_normal(0x8000_0000_0000_0001, 1, false);
        subnormal_result.registers[1] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        rejected.push(subnormal_result);
        let mut overflow = make_pair();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(u64::MAX, 0x7ffe, true);
        rejected.push(overflow);

        for x87 in rejected {
            let mut state = LongModeState::default();
            state.rip = 0x7250;
            state.x87 = Some(x87);
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xe1], 0x7250);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }

        let mut initial = LongModeState::default();
        initial.rip = 0x7300;
        initial.x87 = Some(make_pair());
        for (bytes, stop, budget) in [
            (&[0xde][..], None, 1),
            (&[0xde, 0xe1][..], Some(0x7300), 1),
            (&[0xde, 0xe1][..], None, 0),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, 0x7300);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
        let mut fetch_fault = initial.clone();
        let before_fetch_fault = fetch_fault.clone();
        let mut fetch_memory = Memory::at(&[0xde, 0xe1], 0x7300);
        fetch_memory.fetch_error = Some((
            0x7301,
            MemoryError::Unmapped {
                address: 0x7301,
                access: AccessKind::Fetch,
            },
        ));
        let result = run(
            &mut fetch_fault,
            &mut fetch_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert!(matches!(result.exit, RunExit::MemoryError(_)));
        assert_eq!(fetch_fault, before_fetch_fault);
        let mut missing = LongModeState::default();
        missing.rip = 0x7350;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xde, 0xe1], 0x7350);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(missing, before);
    }

    #[test]
    fn fsub_fetch_fault_stop_and_budget_preserve_atomic_retirement_rules() {
        let mut fault_state = LongModeState::default();
        fault_state.rip = 0x1000;
        let mut fault_x87 = crate::state::X87State::fixture_seed();
        fault_x87.control_word = 0x037f;
        fault_x87.tag_word = 0;
        fault_x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        fault_x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        fault_state.x87 = Some(fault_x87);
        let before_fault = fault_state.clone();
        let mut fault_memory = Memory::at(&[0xd8], 0x1000);
        fault_memory.fetch_error = Some((
            0x1001,
            MemoryError::Unmapped {
                address: 0x1001,
                access: AccessKind::Fetch,
            },
        ));
        let result = run(
            &mut fault_state,
            &mut fault_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(
            result.exit,
            RunExit::MemoryError(MemoryError::Unmapped {
                address: 0x1001,
                access: AccessKind::Fetch
            })
        );
        assert_eq!(fault_state, before_fault);

        let mut stopped = LongModeState::default();
        stopped.rip = 0x1000;
        let stopped_before = stopped.clone();
        let mut memory = Memory::at(&[0xd8, 0xe1], 0x1000);
        let result = run(
            &mut stopped,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x1000),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(stopped, stopped_before);

        let mut budget = LongModeState::default();
        budget.rip = 0x1000;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x0200;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        budget.x87 = Some(x87);
        let before = budget.clone();
        let mut memory = Memory::at(&[0xd8, 0xe1], 0x1000);
        let result = run(
            &mut budget,
            &mut memory,
            RunConfig {
                instruction_budget: 0,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(budget, before);

        let result = run(
            &mut budget,
            &mut memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: Some(0x1002),
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.exit, RunExit::StopAddress);
        assert_eq!(result.instructions_executed, 1);
        assert_eq!(budget.rip, 0x1002);
        assert_eq!(
            budget.x87.as_ref().unwrap().registers[0],
            binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false)
        );
    }

    #[test]
    fn fadd_sti_st0_commits_only_the_selected_destination_for_all_indices() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        for destination_index in 0u8..=7 {
            let top = (destination_index.wrapping_mul(3).wrapping_add(5)) & 7;
            let physical_destination = (top + destination_index) & 7;
            let physical_source = top;
            let mut state = LongModeState::default();
            state.rip = 0x2000;
            state.gpr = std::array::from_fn(|index| 0x1122_3344_5566_7788 ^ index as u64);
            state.rflags = 0xa55a_1234_5678_9abc;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | 0x4500 | 0x0200;
            x87.tag_word = u16::MAX;
            x87.tag_word &= !(3 << (physical_destination * 2));
            x87.tag_word &= !(3 << (physical_source * 2));
            x87.registers[usize::from(physical_destination)] = one;
            x87.registers[usize::from(physical_source)] = one;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xc0 | destination_index], 0x2000);
            let run_result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(0x2002),
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(run_result.exit, RunExit::StopAddress);
            assert_eq!(run_result.instructions_executed, 1);
            assert_eq!(state.rip, 0x2002);
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
            let result = state.x87.as_ref().unwrap();
            assert_eq!(
                result.registers[usize::from(physical_destination)],
                two,
                "destination ST({destination_index}), TOP={top}"
            );
            for physical in 0..8 {
                if physical != usize::from(physical_destination) {
                    assert_eq!(
                        result.registers[physical],
                        before.x87.as_ref().unwrap().registers[physical],
                        "preserved physical register {physical} for ST({destination_index})"
                    );
                }
            }
            assert_eq!(
                result.control_word,
                before.x87.as_ref().unwrap().control_word
            );
            assert_eq!(result.tag_word, before.x87.as_ref().unwrap().tag_word);
            assert_eq!(result.top(), top);
            assert_eq!(
                result.status_word,
                before.x87.as_ref().unwrap().status_word & !(1 << 9)
            );
            assert_eq!(result.instruction_pointer, 0x2000);
            assert_eq!(
                result.data_pointer,
                before.x87.as_ref().unwrap().data_pointer
            );
            assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);
        }
    }

    #[test]
    fn fadd_sti_st0_rejects_without_any_state_commit() {
        for destination_index in 0u8..=7 {
            let top = (destination_index.wrapping_mul(5).wrapping_add(1)) & 7;
            let physical_destination = (top + destination_index) & 7;
            let physical_source = top;
            for rejection in 0..5 {
                // The i=0 alias has one exponent and its self-sum coefficient
                // is necessarily even; its single tag is covered by rejection 3.
                if destination_index == 0 && matches!(rejection, 0 | 2 | 4) {
                    continue;
                }
                let mut state = LongModeState::default();
                state.rip = 0x3000;
                state.gpr = [0x0123_4567_89ab_cdef; 16];
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = u16::from(top) << 11;
                x87.tag_word = 0;
                x87.registers[usize::from(physical_destination)] =
                    binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
                x87.registers[usize::from(physical_source)] =
                    binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
                match rejection {
                    0 => {
                        x87.registers[usize::from(physical_source)] =
                            binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
                    }
                    1 => {
                        x87.registers[usize::from(physical_source)] =
                            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
                    }
                    2 => {
                        x87.registers[usize::from(physical_destination)] =
                            binary80_normal(u64::MAX, 0x3fff, false);
                        x87.registers[usize::from(physical_source)] =
                            binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
                    }
                    3 => {
                        let bad_slot = if destination_index == 0 {
                            physical_destination
                        } else {
                            physical_source
                        };
                        x87.tag_word |= 3 << (bad_slot * 2);
                    }
                    4 => x87.tag_word |= 3 << (physical_destination * 2),
                    _ => unreachable!(),
                }
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdc, 0xc0 | destination_index], 0x3000);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { opcode: 0xdc }),
                    "ST({destination_index}), TOP={top}, rejection={rejection}"
                );
                assert_eq!(state, before);
            }
        }
    }

    #[test]
    fn faddp_sti_st0_pops_physical_top_and_preserves_register_order_for_every_index() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        for top in 0u8..=7 {
            for destination_index in 0u8..=7 {
                let physical_top = top;
                let physical_destination = top.wrapping_add(destination_index) & 7;
                let mut state = LongModeState::default();
                state.rip = 0x2000;
                state.gpr = std::array::from_fn(|index| 0x1122_3344_5566_7788 ^ index as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4500 | 0x0200;
                x87.tag_word = u16::MAX;
                for physical in 0..8 {
                    x87.registers[physical] = [physical as u8; 10];
                }
                x87.registers[usize::from(physical_top)] = one;
                x87.registers[usize::from(physical_destination)] = one;
                x87.set_tag(physical_top, X87Tag::Valid);
                x87.set_tag(physical_destination, X87Tag::Valid);
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xde, 0xc0 | destination_index], 0x2000);
                let run_result = run(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: 1,
                        stop_rip: Some(0x2002),
                        context: AccessContext::default(),
                    },
                    || false,
                );
                assert_eq!(run_result.exit, RunExit::StopAddress);
                assert_eq!(run_result.instructions_executed, 1);
                assert_eq!(state.rip, 0x2002);
                assert_eq!(state.gpr, before.gpr);
                assert_eq!(state.rflags, before.rflags);
                let result = state.x87.as_ref().unwrap();
                for physical in 0..8u8 {
                    let expected_payload = if physical == physical_destination {
                        two
                    } else {
                        before.x87.as_ref().unwrap().registers[usize::from(physical)]
                    };
                    assert_eq!(
                        result.registers[usize::from(physical)],
                        expected_payload,
                        "payload physical={physical}, TOP={top}, i={destination_index}"
                    );
                    let expected_tag = if physical == physical_top {
                        X87Tag::Empty
                    } else {
                        before.x87.as_ref().unwrap().tag(physical).unwrap()
                    };
                    assert_eq!(
                        result.tag(physical),
                        Some(expected_tag),
                        "tag physical={physical}, TOP={top}, i={destination_index}"
                    );
                }
                assert_eq!(result.control_word, 0x037f);
                let new_top = top.wrapping_add(1) & 7;
                assert_eq!(result.top(), new_top);
                assert_eq!(
                    result.status_word,
                    (before.x87.as_ref().unwrap().status_word & !((7 << 11) | (1 << 9)))
                        | (u16::from(new_top) << 11)
                );
                assert_eq!(result.instruction_pointer, 0x2000);
                assert_eq!(
                    result.data_pointer,
                    before.x87.as_ref().unwrap().data_pointer
                );
                assert_eq!(result.last_opcode, before.x87.as_ref().unwrap().last_opcode);

                if destination_index == 1 {
                    assert_eq!(
                        result.physical_register_for_st(0),
                        Some(physical_destination)
                    );
                    assert_eq!(
                        result.registers[usize::from(result.physical_register_for_st(0).unwrap())],
                        two,
                        "FADDP ST(1), ST(0) leaves the result in new ST(0)"
                    );
                }
                if destination_index == 0 {
                    assert_eq!(result.tag(physical_top), Some(X87Tag::Empty));
                    assert_eq!(result.registers[usize::from(physical_top)], two);
                    assert_eq!(
                        result.registers[usize::from(new_top)],
                        before.x87.as_ref().unwrap().registers[usize::from(new_top)],
                        "i=0 pop leaves the next physical payload untouched"
                    );
                }
            }
        }
    }

    #[test]
    fn fstp_sti_copies_raw_st0_then_pops_for_every_top_and_index() {
        let raw_sources = [
            binary80_normal(0x8000_0000_0000_0001, 1, false),
            binary80_normal(0xffff_ffff_ffff_ffff, 0x7ffe, true),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, true),
        ];
        let destination_tags = [X87Tag::Valid, X87Tag::Zero, X87Tag::Special, X87Tag::Empty];
        for top in 0u8..=7 {
            for destination_index in 0u8..=7 {
                for destination_tag_index in 0..if destination_index == 0 { 1 } else { 4 } {
                    let old_top = top;
                    let physical_destination = (top + destination_index) & 7;
                    let source = raw_sources[usize::from((top + destination_index) % 3)];
                    let mut state = LongModeState::default();
                    state.rip = 0x8800;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4500 | (1 << 9);
                    x87.tag_word = u16::MAX;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x30 + physical as u8; 10];
                    }
                    x87.registers[usize::from(old_top)] = source;
                    x87.set_tag(old_top, X87Tag::Valid);
                    if physical_destination != old_top {
                        x87.set_tag(
                            physical_destination,
                            destination_tags[destination_tag_index],
                        );
                    }
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xdd, 0xd8 | destination_index], state.rip);
                    let result = run(
                        &mut state,
                        &mut memory,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: Some(0x8802),
                            context: AccessContext::default(),
                        },
                        || false,
                    );
                    assert_eq!(result.exit, RunExit::StopAddress);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(state.rip, 0x8802);
                    assert_eq!(state.gpr, before.gpr);
                    assert_eq!(state.rflags, before.rflags);
                    let expected_top = top.wrapping_add(1) & 7;
                    let result_x87 = state.x87.as_ref().unwrap();
                    for physical in 0u8..8 {
                        let expected_payload = if physical == physical_destination {
                            source
                        } else {
                            before.x87.as_ref().unwrap().registers[usize::from(physical)]
                        };
                        assert_eq!(
                            result_x87.registers[usize::from(physical)],
                            expected_payload,
                            "payload top={top}, i={destination_index}, physical={physical}"
                        );
                        let expected_tag = if physical == old_top {
                            X87Tag::Empty
                        } else if physical == physical_destination {
                            X87Tag::Valid
                        } else {
                            before.x87.as_ref().unwrap().tag(physical).unwrap()
                        };
                        assert_eq!(
                            result_x87.tag(physical),
                            Some(expected_tag),
                            "tag top={top}, i={destination_index}, physical={physical}"
                        );
                    }
                    assert_eq!(result_x87.control_word, 0x037f);
                    assert_eq!(
                        result_x87.status_word,
                        (before.x87.as_ref().unwrap().status_word & !(7 << 11))
                            | (u16::from(expected_top) << 11)
                    );
                    assert_eq!(result_x87.instruction_pointer, 0x8800);
                    assert_eq!(result_x87.data_pointer, 0x5678);
                    assert_eq!(result_x87.last_opcode, 0x05aa);
                    if destination_index == 0 {
                        assert_eq!(result_x87.registers[usize::from(top)], source);
                        assert_eq!(result_x87.tag(top), Some(X87Tag::Empty));
                    }
                    if destination_index == 1 {
                        assert_eq!(
                            result_x87.physical_register_for_st(0),
                            Some(physical_destination)
                        );
                        assert_eq!(result_x87.tag(physical_destination), Some(X87Tag::Valid));
                    }
                }
            }
        }
    }

    #[test]
    fn fchs_flips_only_the_raw_sign_byte_for_normal_st0_and_clears_c1() {
        let sources = [
            binary80_normal(0x8000_0000_0000_0000, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 1, true),
            binary80_normal(u64::MAX, 0x7ffe, false),
            binary80_normal(u64::MAX, 0x7ffe, true),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, false),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, true),
        ];
        for top in 0u8..=7 {
            for source in sources {
                for initial_c1 in [false, true] {
                    let mut state = LongModeState::default();
                    state.rip = 0x8f00;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word =
                        (u16::from(top) << 11) | 0x4500 | if initial_c1 { 1 << 9 } else { 0 };
                    x87.tag_word = u16::MAX;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x30 + physical as u8; 10];
                    }
                    x87.registers[usize::from(top)] = source;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.registers[usize::from(top)][9] ^= 0x80;
                    expected_x87.status_word &= !(1 << 9);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xd9, 0xe0], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(state, expected, "TOP={top}, source={source:02x?}");

                    let mut memory = Memory::at(&[0xd9, 0xe0], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(
                        state.x87.as_ref().unwrap().registers[usize::from(top)],
                        source
                    );
                    assert_eq!(state.x87.as_ref().unwrap().status_word & (1 << 9), 0);
                    assert_eq!(state.rip, before.rip + 4);
                }
            }
        }
    }

    #[test]
    fn fabs_clears_only_the_raw_sign_byte_for_normal_st0_and_clears_c1() {
        let sources = [
            binary80_normal(0x8000_0000_0000_0000, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 1, true),
            binary80_normal(u64::MAX, 0x7ffe, false),
            binary80_normal(u64::MAX, 0x7ffe, true),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, false),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, true),
        ];
        for top in 0u8..=7 {
            for source in sources {
                for initial_c1 in [false, true] {
                    let mut state = LongModeState::default();
                    state.rip = 0x8f20;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word =
                        (u16::from(top) << 11) | 0x4500 | if initial_c1 { 1 << 9 } else { 0 };
                    x87.tag_word = u16::MAX;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x50 + physical as u8; 10];
                    }
                    x87.registers[usize::from(top)] = source;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.instruction_pointer = 0x4321;
                    x87.data_pointer = 0x8765;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.registers[usize::from(top)][9] &= 0x7f;
                    expected_x87.status_word &= !(1 << 9);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xd9, 0xe1], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(state, expected, "TOP={top}, source={source:02x?}");

                    let mut memory = Memory::at(&[0xd9, 0xe1], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(
                        state.x87.as_ref().unwrap().registers[usize::from(top)],
                        expected.x87.as_ref().unwrap().registers[usize::from(top)]
                    );
                    assert_eq!(state.x87.as_ref().unwrap().status_word & (1 << 9), 0);
                    assert_eq!(state.rip, before.rip + 4);
                }
            }
        }
    }

    #[test]
    fn ftst_replaces_all_condition_codes_from_the_normal_st0_sign() {
        let sources = [
            binary80_normal(0x8000_0000_0000_0000, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 1, true),
            binary80_normal(u64::MAX, 0x7ffe, false),
            binary80_normal(u64::MAX, 0x7ffe, true),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, false),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, true),
        ];
        for top in 0u8..=7 {
            for source in sources {
                for condition_codes in 0u8..16 {
                    let mut state = LongModeState::default();
                    state.rip = 0x8fa0;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    let condition_bits = (u16::from(condition_codes & 1) << 8)
                        | (u16::from((condition_codes >> 1) & 1) << 9)
                        | (u16::from((condition_codes >> 2) & 1) << 10)
                        | (u16::from((condition_codes >> 3) & 1) << 14);
                    x87.status_word = (u16::from(top) << 11) | condition_bits;
                    x87.tag_word = 0xa55a;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x60 + physical as u8; 10];
                    }
                    x87.registers[usize::from(top)] = source;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let negative = source[9] & 0x80 != 0;
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.status_word =
                        (expected_x87.status_word & !0x4700) | if negative { 0x0100 } else { 0 };
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xd9, 0xe4], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced)
                    );
                    assert_eq!(
                        state, expected,
                        "TOP={top}, source={source:02x?}, cc={condition_codes:04b}"
                    );
                }
            }
        }
    }

    #[test]
    fn ftst_status_composes_with_fnstsw_ax() {
        let mut state = LongModeState::default();
        state.rip = 0x8fe0;
        state.gpr[Register::Rax as usize] = 0xfeed_face_cafe_beef;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x4700;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        x87.set_tag(0, X87Tag::Valid);
        state.x87 = Some(x87);

        let mut memory = Memory::at(&[0xd9, 0xe4, 0xdf, 0xe0], state.rip);
        let result = run(
            &mut state,
            &mut memory,
            RunConfig {
                instruction_budget: 2,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || false,
        );
        assert_eq!(result.instructions_executed, 2);
        assert_eq!(state.rip, 0x8fe4);
        assert_eq!(state.gpr[Register::Rax as usize], 0xfeed_face_cafe_0100);
        assert_eq!(state.x87.as_ref().unwrap().status_word, 0x0100);
        assert_eq!(state.x87.as_ref().unwrap().instruction_pointer, 0x8fe0);
    }

    #[test]
    fn fcom_sti_compares_exact_normals_for_every_top_source_and_initial_cc_field() {
        let vectors = [
            // lhs, rhs, expected condition bits
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x4000,
            ),
            (
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3ffe,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x0000,
            ),
            (
                0x8000_0000_0000_0001,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0001,
                0x3fff,
                true,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                1,
                false,
                u64::MAX,
                0x7ffe,
                false,
                0x0100,
            ),
            (
                u64::MAX,
                0x7ffe,
                true,
                0x8000_0000_0000_0000,
                1,
                true,
                0x0100,
            ),
        ];
        for top in 0u8..=7 {
            for source in 0u8..=7 {
                for (lhs_sig, lhs_exp, lhs_neg, rhs_sig, rhs_exp, rhs_neg, condition) in vectors {
                    for initial_cc in 0u8..16 {
                        let mut state = LongModeState::default();
                        state.rip = 0x9100;
                        state.gpr =
                            std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                        state.rflags = 0xa55a_1234_5678_9abc;
                        let mut x87 = crate::state::X87State::fixture_seed();
                        x87.control_word = 0x037f;
                        x87.status_word = (u16::from(top) << 11)
                            | (u16::from(initial_cc & 1) << 8)
                            | (u16::from((initial_cc >> 1) & 1) << 9)
                            | (u16::from((initial_cc >> 2) & 1) << 10)
                            | (u16::from((initial_cc >> 3) & 1) << 14);
                        x87.tag_word = 0xa55a;
                        for physical in 0..8 {
                            x87.registers[physical] = [0x70 + physical as u8; 10];
                        }
                        let lhs_physical = usize::from(top);
                        let rhs_physical = usize::from((top + source) & 7);
                        let lhs = binary80_normal(lhs_sig, lhs_exp, lhs_neg);
                        let rhs = binary80_normal(rhs_sig, rhs_exp, rhs_neg);
                        x87.registers[lhs_physical] = lhs;
                        if source != 0 {
                            x87.registers[rhs_physical] = rhs;
                        }
                        x87.set_tag(top, X87Tag::Valid);
                        x87.set_tag((top + source) & 7, X87Tag::Valid);
                        x87.instruction_pointer = 0x1234;
                        x87.data_pointer = 0x5678;
                        x87.last_opcode = 0x05aa;
                        state.x87 = Some(x87);
                        let before = state.clone();
                        let mut expected = before.clone();
                        let expected_x87 = expected.x87.as_mut().unwrap();
                        expected_x87.status_word = (expected_x87.status_word & !0x4700)
                            | if source == 0 { 0x4000 } else { condition };
                        expected_x87.instruction_pointer = before.rip;
                        expected.rip += 2;

                        let mut memory = Memory::at(&[0xd8, 0xd0 | source], state.rip);
                        assert_eq!(
                            step(&mut state, &mut memory, AccessContext::default()),
                            Ok(StepOutcome::Advanced),
                            "TOP={top}, i={source}, lhs={lhs:02x?}, rhs={rhs:02x?}, cc={initial_cc:04b}"
                        );
                        assert_eq!(state, expected);
                    }
                }
            }
        }
    }

    #[test]
    fn fcom_sti_rejects_either_operand_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for source in 1u8..=7 {
            for invalid_operand in 0..2 {
                for invalidity in 0..7 {
                    let mut state = LongModeState::default();
                    state.rip = 0x9140;
                    state.gpr = [0xfeed_face_cafe_beef; 16];
                    state.rflags = 0x0bad_f00d_0123_4567;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    let top = 6u8;
                    x87.control_word = if invalidity == 5 { 0x027f } else { 0x037f };
                    x87.status_word =
                        (u16::from(top) << 11) | if invalidity == 6 { 0x0080 } else { 0x4500 };
                    x87.tag_word = 0xa55a;
                    let lhs_physical = top;
                    let rhs_physical = (top + source) & 7;
                    x87.registers[usize::from(lhs_physical)] = normal;
                    x87.registers[usize::from(rhs_physical)] = normal;
                    x87.set_tag(lhs_physical, X87Tag::Valid);
                    x87.set_tag(rhs_physical, X87Tag::Valid);
                    let physical = if invalid_operand == 0 {
                        lhs_physical
                    } else {
                        rhs_physical
                    };
                    match invalidity {
                        0 => {
                            x87.set_tag(physical, X87Tag::Empty);
                        }
                        1 => {
                            x87.set_tag(physical, X87Tag::Zero);
                        }
                        2 => {
                            x87.set_tag(physical, X87Tag::Special);
                        }
                        3 => x87.registers[usize::from(physical)] = [0; 10],
                        4 => x87.registers[usize::from(physical)] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                        _ => {}
                    }
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xd8, 0xd0 | source], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Unsupported { opcode: 0xd8 })
                    );
                    assert_eq!(
                        state, before,
                        "i={source}, operand={invalid_operand}, invalidity={invalidity}"
                    );
                }
            }
        }

        for invalidity in 0..5 {
            let mut state = LongModeState::default();
            state.rip = 0x9150;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0x4700;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            match invalidity {
                0 => {
                    x87.set_tag(0, X87Tag::Empty);
                }
                1 => {
                    x87.set_tag(0, X87Tag::Zero);
                }
                2 => {
                    x87.set_tag(0, X87Tag::Special);
                }
                3 => x87.registers[0] = [0; 10],
                _ => x87.registers[0] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            }
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xd0], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before, "alias invalidity={invalidity}");
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x9160;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xd8, 0xd1], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(missing, before);

        let mut state = LongModeState::default();
        state.rip = 0x9180;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x4700;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xd8][..], None, 1, false, false),
            (&[0xd8, 0xd1][..], Some(0x9180), 1, false, false),
            (&[0xd8, 0xd1][..], None, 0, false, false),
            (&[0xd8, 0xd1][..], None, 1, true, false),
            (&[0xd8, 0xd1][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn fcomp_sti_compares_then_pops_the_old_physical_top() {
        let vectors = [
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3ffe,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x4000,
            ),
            (
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0001,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                false,
                0x8000_0000_0000_0000,
                0x3fff,
                true,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                1,
                false,
                u64::MAX,
                0x7ffe,
                false,
                0x0100,
            ),
            (
                u64::MAX,
                0x7ffe,
                true,
                0x8000_0000_0000_0000,
                1,
                true,
                0x0100,
            ),
        ];
        for top in 0u8..=7 {
            for source in 0u8..=7 {
                for (lhs_sig, lhs_exp, lhs_neg, rhs_sig, rhs_exp, rhs_neg, condition) in vectors {
                    for initial_cc in 0u8..16 {
                        let mut state = LongModeState::default();
                        state.rip = 0x91a0;
                        state.gpr =
                            std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                        state.rflags = 0xa55a_1234_5678_9abc;
                        let mut x87 = crate::state::X87State::fixture_seed();
                        x87.control_word = 0x037f;
                        x87.status_word = (u16::from(top) << 11)
                            | (u16::from(initial_cc & 1) << 8)
                            | (u16::from((initial_cc >> 1) & 1) << 9)
                            | (u16::from((initial_cc >> 2) & 1) << 10)
                            | (u16::from((initial_cc >> 3) & 1) << 14);
                        x87.tag_word = 0xa55a;
                        for physical in 0..8 {
                            x87.registers[physical] = [0x70 + physical as u8; 10];
                        }
                        let lhs_physical = usize::from(top);
                        let rhs_physical = usize::from((top + source) & 7);
                        x87.registers[lhs_physical] = binary80_normal(lhs_sig, lhs_exp, lhs_neg);
                        if source != 0 {
                            x87.registers[rhs_physical] =
                                binary80_normal(rhs_sig, rhs_exp, rhs_neg);
                        }
                        x87.set_tag(top, X87Tag::Valid);
                        x87.set_tag((top + source) & 7, X87Tag::Valid);
                        x87.instruction_pointer = 0x1234;
                        x87.data_pointer = 0x5678;
                        x87.last_opcode = 0x05aa;
                        state.x87 = Some(x87);
                        let before = state.clone();
                        let mut expected = before.clone();
                        let expected_x87 = expected.x87.as_mut().unwrap();
                        expected_x87.status_word = (expected_x87.status_word & !0x4700)
                            | if source == 0 { 0x4000 } else { condition };
                        expected_x87.status_word =
                            (expected_x87.status_word & !0x3800) | (u16::from((top + 1) & 7) << 11);
                        expected_x87.set_tag(top, X87Tag::Empty);
                        expected_x87.instruction_pointer = before.rip;
                        expected.rip += 2;

                        let mut memory = Memory::at(&[0xd8, 0xd8 | source], state.rip);
                        assert_eq!(
                            step(&mut state, &mut memory, AccessContext::default()),
                            Ok(StepOutcome::Advanced),
                            "TOP={top}, i={source}, lhs={lhs_sig:016x}/{lhs_exp:04x}/{lhs_neg}, rhs={rhs_sig:016x}/{rhs_exp:04x}/{rhs_neg}, cc={initial_cc:04b}"
                        );
                        assert_eq!(state, expected);
                        assert_eq!(state.x87.as_ref().unwrap().tag(top), Some(X87Tag::Empty));
                        assert_eq!(state.x87.as_ref().unwrap().top(), (top + 1) & 7);
                    }
                }
            }
        }
    }

    #[test]
    fn fcomp_sti_rejects_independent_operands_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for source in 0u8..=7 {
            for invalid_operand in 0..2 {
                for invalidity in 0..5 {
                    let mut state = LongModeState::default();
                    state.rip = 0x91c0;
                    state.gpr = [0xfeed_face_cafe_beef; 16];
                    state.rflags = 0x0bad_f00d_0123_4567;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = 0x4700;
                    x87.registers[0] = normal;
                    x87.registers[usize::from(source)] = normal;
                    x87.set_tag(0, X87Tag::Valid);
                    x87.set_tag(source, X87Tag::Valid);
                    let physical = if invalid_operand == 0 { 0 } else { source };
                    match invalidity {
                        0 => {
                            x87.set_tag(physical, X87Tag::Empty);
                        }
                        1 => {
                            x87.set_tag(physical, X87Tag::Zero);
                        }
                        2 => {
                            x87.set_tag(physical, X87Tag::Special);
                        }
                        3 => x87.registers[usize::from(physical)] = [0; 10],
                        _ => x87.registers[usize::from(physical)] = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                    }
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xd8, 0xd8 | source], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Unsupported { opcode: 0xd8 })
                    );
                    assert_eq!(
                        state, before,
                        "i={source}, operand={invalid_operand}, invalidity={invalidity}"
                    );
                }
            }
        }

        for (missing_state, invalid_environment) in [(true, false), (false, false), (false, true)] {
            let mut state = LongModeState::default();
            state.rip = 0x91d0;
            if !missing_state {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = if invalid_environment { 0x027f } else { 0x037f };
                x87.status_word = if invalid_environment { 0x4700 } else { 0x4780 };
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xd9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 }),
                "missing_state={missing_state}, invalid_environment={invalid_environment}"
            );
            assert_eq!(state, before);
        }

        let mut state = LongModeState::default();
        state.rip = 0x91e0;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x4700;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xd8][..], None, 1, false, false),
            (&[0xd8, 0xd9][..], Some(0x91e0), 1, false, false),
            (&[0xd8, 0xd9][..], None, 0, false, false),
            (&[0xd8, 0xd9][..], None, 1, true, false),
            (&[0xd8, 0xd9][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn fcompp_compares_old_st0_st1_and_pops_two_physical_entries() {
        let vectors = [
            (
                0x8000_0000_0000_0000,
                0x3fff,
                0x8000_0000_0000_0000,
                0x3ffe,
                0x0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x3ffe,
                0x8000_0000_0000_0000,
                0x3fff,
                0x0100,
            ),
            (
                0x8000_0000_0000_0000,
                0x3fff,
                0x8000_0000_0000_0000,
                0x3fff,
                0x4000,
            ),
        ];
        for top in 0u8..=7 {
            for (lhs_sig, lhs_exp, rhs_sig, rhs_exp, condition) in vectors {
                for initial_cc in 0u8..16 {
                    let mut state = LongModeState::default();
                    state.rip = 0x91f0;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11)
                        | (u16::from(initial_cc & 1) << 8)
                        | (u16::from((initial_cc >> 1) & 1) << 9)
                        | (u16::from((initial_cc >> 2) & 1) << 10)
                        | (u16::from((initial_cc >> 3) & 1) << 14);
                    x87.tag_word = 0xa55a;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x80 + physical as u8; 10];
                    }
                    x87.registers[usize::from(top)] = binary80_normal(lhs_sig, lhs_exp, false);
                    let second = (top + 1) & 7;
                    x87.registers[usize::from(second)] = binary80_normal(rhs_sig, rhs_exp, false);
                    x87.set_tag(top, X87Tag::Valid);
                    x87.set_tag(second, X87Tag::Valid);
                    // The unused third stack entry may be Empty; FCOMPP reads only ST(0)/ST(1).
                    x87.set_tag((top + 2) & 7, X87Tag::Empty);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.status_word = (expected_x87.status_word & !0x4700) | condition;
                    expected_x87.status_word =
                        (expected_x87.status_word & !0x3800) | (u16::from((top + 2) & 7) << 11);
                    expected_x87.set_tag(top, X87Tag::Empty);
                    expected_x87.set_tag(second, X87Tag::Empty);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xde, 0xd9], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, condition={condition:04x}, cc={initial_cc:04b}"
                    );
                    assert_eq!(state, expected);
                    assert_eq!(state.x87.as_ref().unwrap().top(), (top + 2) & 7);
                }
            }
        }
    }

    #[test]
    fn fcompp_rejects_bad_operands_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for invalid_operand in 0..2 {
            let mut state = LongModeState::default();
            state.rip = 0x9200;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0x4700;
            x87.registers[0] = normal;
            x87.registers[1] = normal;
            x87.set_tag(0, X87Tag::Valid);
            x87.set_tag(1, X87Tag::Valid);
            x87.set_tag(if invalid_operand == 0 { 0 } else { 1 }, X87Tag::Empty);
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xd9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before, "invalid_operand={invalid_operand}");
        }
        for (missing_state, invalid_environment) in [(true, false), (false, false), (false, true)] {
            let mut state = LongModeState::default();
            state.rip = 0x9220;
            if !missing_state {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = if invalid_environment { 0x027f } else { 0x037f };
                x87.status_word = if invalid_environment { 0x4700 } else { 0x4780 };
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xd9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }
        let mut state = LongModeState::default();
        state.rip = 0x9240;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x4700;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xde][..], None, 1, false, false),
            (&[0xde, 0xd9][..], Some(0x9240), 1, false, false),
            (&[0xde, 0xd9][..], None, 0, false, false),
            (&[0xde, 0xd9][..], None, 1, true, false),
            (&[0xde, 0xd9][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn fucompp_compares_old_st0_st1_and_pops_two_physical_entries() {
        let relations = [
            (
                binary80_normal(0x8000_0000_0000_0003, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                0x0000,
            ),
            (
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0003, 0x3fff, false),
                0x0100,
            ),
            (
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                0x4000,
            ),
        ];
        for top in 0u8..=7 {
            for (lhs, rhs, relation) in relations {
                for initial_cc in 0u8..16 {
                    let mut state = LongModeState::default();
                    state.rip = 0x9250;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11)
                        | (u16::from(initial_cc & 1) << 8)
                        | (u16::from((initial_cc >> 1) & 1) << 9)
                        | (u16::from((initial_cc >> 2) & 1) << 10)
                        | (u16::from((initial_cc >> 3) & 1) << 14);
                    x87.tag_word = 0xa55a;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x80 + physical as u8; 10];
                    }
                    let second = (top + 1) & 7;
                    x87.registers[usize::from(top)] = lhs;
                    x87.registers[usize::from(second)] = rhs;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.set_tag(second, X87Tag::Valid);
                    // FUCOMPP only reads these two entries; the next old stack slot can be Empty.
                    x87.set_tag((top + 2) & 7, X87Tag::Empty);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.status_word = (expected_x87.status_word & !0x4700) | relation;
                    expected_x87.status_word =
                        (expected_x87.status_word & !0x3800) | (u16::from((top + 2) & 7) << 11);
                    expected_x87.set_tag(top, X87Tag::Empty);
                    expected_x87.set_tag(second, X87Tag::Empty);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xda, 0xe9], state.rip);
                    let result = run(
                        &mut state,
                        &mut memory,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: Some(expected.rip),
                            context: AccessContext::default(),
                        },
                        || false,
                    );
                    assert_eq!(result.exit, RunExit::StopAddress);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(state, expected);
                    let actual_x87 = state.x87.as_ref().unwrap();
                    assert_eq!(actual_x87.top(), (top + 2) & 7);
                    assert_eq!(actual_x87.tag(top), Some(X87Tag::Empty));
                    assert_eq!(actual_x87.tag(second), Some(X87Tag::Empty));
                    assert_eq!(
                        actual_x87.registers,
                        before.x87.as_ref().unwrap().registers,
                        "FUCOMPP must leave physical payloads untouched"
                    );
                }
            }
        }
    }

    #[test]
    fn fucompp_rejects_invalid_operands_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in [0u8, 7] {
            let second = (top + 1) & 7;
            for (physical, bad_tag) in [(top, X87Tag::Empty), (second, X87Tag::Zero)] {
                let mut state = LongModeState::default();
                state.rip = 0x9270;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = u16::from(top) << 11;
                x87.registers[usize::from(top)] = normal;
                x87.registers[usize::from(second)] = normal;
                x87.set_tag(top, X87Tag::Valid);
                x87.set_tag(second, X87Tag::Valid);
                x87.set_tag(physical, bad_tag);
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xda, 0xe9], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { opcode: 0xda })
                );
                assert_eq!(state, before);
            }
        }

        for invalid in [
            [0; 10],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x40],
        ] {
            for physical in [0usize, 1] {
                let mut state = LongModeState::default();
                state.rip = 0x9280;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = 0;
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                x87.registers[physical] = invalid;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xda, 0xe9], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { opcode: 0xda })
                );
                assert_eq!(state, before, "physical={physical}, raw={invalid:02x?}");
            }
        }

        for (missing_state, control_word, status_word) in [
            (true, 0x037f, 0),
            (false, 0x027f, 0),
            (false, 0x037f, 0x0080),
            (false, 0x037f, 0x0001),
            (false, 0x037f, 0x8000),
        ] {
            let mut state = LongModeState::default();
            state.rip = 0x9290;
            if !missing_state {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = control_word;
                x87.status_word = status_word;
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xda, 0xe9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xda })
            );
            assert_eq!(state, before);
        }

        let mut state = LongModeState::default();
        state.rip = 0x92a0;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xda][..], None, 1, false, false),
            (&[0xda, 0xe9][..], Some(0x92a0), 1, false, false),
            (&[0xda, 0xe9][..], None, 0, false, false),
            (&[0xda, 0xe9][..], None, 1, true, false),
            (&[0xda, 0xe9][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn fucom_st0_st1_replaces_condition_codes_without_pop() {
        let vectors = [
            // Positive values differ only in low significand bits.
            (
                binary80_normal(0x8000_0000_0000_0003, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                0x0000,
            ),
            // Sign dominates the magnitude ordering.
            (
                binary80_normal(0xffff_ffff_ffff_fffe, 0x4000, true),
                binary80_normal(0x8000_0000_0000_0001, 0x3fff, false),
                0x0100,
            ),
            // Same-sign exponent ordering for positive and negative values.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                0x0000,
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
                0x0100,
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
                0x0000,
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
                0x0100,
            ),
            (
                binary80_normal(0x8000_0000_0000_0001, 0x3ffe, true),
                binary80_normal(0x8000_0000_0000_0001, 0x3ffe, true),
                0x4000,
            ),
        ];
        for top in 0u8..=7 {
            for (lhs, rhs, condition) in vectors {
                for initial_cc in 0u8..16 {
                    let mut state = LongModeState::default();
                    state.rip = 0x9310;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11)
                        | (u16::from(initial_cc & 1) << 8)
                        | (u16::from((initial_cc >> 1) & 1) << 9)
                        | (u16::from((initial_cc >> 2) & 1) << 10)
                        | (u16::from((initial_cc >> 3) & 1) << 14);
                    x87.tag_word = 0xa55a;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x80 + physical as u8; 10];
                    }
                    let second = (top + 1) & 7;
                    x87.registers[usize::from(top)] = lhs;
                    x87.registers[usize::from(second)] = rhs;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.set_tag(second, X87Tag::Valid);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.status_word = (expected_x87.status_word & !0x4700) | condition;
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xdd, 0xe1], state.rip);
                    let result = run(
                        &mut state,
                        &mut memory,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: Some(expected.rip),
                            context: AccessContext::default(),
                        },
                        || false,
                    );
                    assert_eq!(result.exit, RunExit::StopAddress);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(
                        state, expected,
                        "TOP={top}, condition={condition:04x}, cc={initial_cc:04b}"
                    );
                    assert_eq!(state.x87.as_ref().unwrap().top(), top);
                }
            }
        }
    }

    #[test]
    fn fucom_rejects_bad_operands_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for invalid_operand in 0..2 {
            let mut state = LongModeState::default();
            state.rip = 0x9400;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0x4700;
            x87.registers[0] = normal;
            x87.registers[1] = normal;
            x87.set_tag(0, X87Tag::Valid);
            x87.set_tag(1, X87Tag::Valid);
            x87.set_tag(invalid_operand, X87Tag::Empty);
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xe1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd })
            );
            assert_eq!(state, before, "invalid_operand={invalid_operand}");
        }
        for (missing_state, invalid_environment) in [(true, false), (false, false), (false, true)] {
            let mut state = LongModeState::default();
            state.rip = 0x9420;
            if !missing_state {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = if invalid_environment { 0x027f } else { 0x037f };
                x87.status_word = if invalid_environment { 0x4700 } else { 0x4780 };
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xe1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd })
            );
            assert_eq!(state, before);
        }

        for invalid_side in 0..2usize {
            let mut state = LongModeState::default();
            state.rip = 0x9440;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.registers[0] = normal;
            x87.registers[1] = normal;
            x87.set_tag(0, X87Tag::Valid);
            x87.set_tag(1, X87Tag::Valid);
            x87.registers[invalid_side] = [0; 10];
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xe1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd })
            );
            assert_eq!(state, before, "invalid_class side={invalid_side}");
        }

        let mut state = LongModeState::default();
        state.rip = 0x9460;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xdd][..], None, 1, false, false),
            (&[0xdd, 0xe1][..], Some(0x9460), 1, false, false),
            (&[0xdd, 0xe1][..], None, 0, false, false),
            (&[0xdd, 0xe1][..], None, 1, true, false),
            (&[0xdd, 0xe1][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn fucomp_st0_st1_compares_then_pops_only_old_physical_top() {
        let relations = [
            (
                binary80_normal(0x8000_0000_0000_0003, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                0x0000,
            ),
            (
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0003, 0x3fff, false),
                0x0100,
            ),
            (
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0002, 0x3fff, false),
                0x4000,
            ),
        ];
        for top in 0u8..=7 {
            for (lhs, rhs, condition) in relations {
                for initial_cc in 0u8..16 {
                    let mut state = LongModeState::default();
                    state.rip = 0x9510;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11)
                        | (u16::from(initial_cc & 1) << 8)
                        | (u16::from((initial_cc >> 1) & 1) << 9)
                        | (u16::from((initial_cc >> 2) & 1) << 10)
                        | (u16::from((initial_cc >> 3) & 1) << 14);
                    x87.tag_word = 0xa55a;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x80 + physical as u8; 10];
                    }
                    let second = (top + 1) & 7;
                    x87.registers[usize::from(top)] = lhs;
                    x87.registers[usize::from(second)] = rhs;
                    x87.set_tag(top, X87Tag::Valid);
                    x87.set_tag(second, X87Tag::Valid);
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.status_word = (expected_x87.status_word & !0x4700) | condition;
                    expected_x87.set_tag(top, X87Tag::Empty);
                    expected_x87.status_word =
                        (expected_x87.status_word & !0x3800) | (u16::from((top + 1) & 7) << 11);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;

                    let mut memory = Memory::at(&[0xdd, 0xe9], state.rip);
                    let result = run(
                        &mut state,
                        &mut memory,
                        RunConfig {
                            instruction_budget: 1,
                            stop_rip: Some(expected.rip),
                            context: AccessContext::default(),
                        },
                        || false,
                    );
                    assert_eq!(result.exit, RunExit::StopAddress);
                    assert_eq!(result.instructions_executed, 1);
                    assert_eq!(state, expected, "TOP={top}, CC={initial_cc:04b}");
                    let actual_x87 = state.x87.as_ref().unwrap();
                    assert_eq!(actual_x87.top(), (top + 1) & 7);
                    assert_eq!(actual_x87.tag(top), Some(X87Tag::Empty));
                    assert_eq!(actual_x87.tag(second), Some(X87Tag::Valid));
                    assert_eq!(
                        actual_x87.registers,
                        before.x87.as_ref().unwrap().registers,
                        "physical payloads must not be rotated or cleared"
                    );
                }
            }
        }
    }

    #[test]
    fn fucomp_rejects_bad_operands_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in [0u8, 7] {
            let second = (top + 1) & 7;
            for (physical, bad_tag) in [(top, X87Tag::Empty), (second, X87Tag::Zero)] {
                let mut state = LongModeState::default();
                state.rip = 0x9600;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = u16::from(top) << 11;
                x87.registers[usize::from(top)] = normal;
                x87.registers[usize::from(second)] = normal;
                x87.set_tag(top, X87Tag::Valid);
                x87.set_tag(second, X87Tag::Valid);
                x87.set_tag(physical, bad_tag);
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdd, 0xe9], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { opcode: 0xdd })
                );
                assert_eq!(state, before);
            }
        }

        for invalid in [
            [0; 10],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x40],
        ] {
            for physical in [0usize, 1] {
                let mut state = LongModeState::default();
                state.rip = 0x9620;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = 0;
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                x87.registers[physical] = invalid;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdd, 0xe9], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Unsupported { opcode: 0xdd })
                );
                assert_eq!(state, before, "physical={physical}, raw={invalid:02x?}");
            }
        }

        for (missing_state, control_word, status_word) in [
            (true, 0x037f, 0),
            (false, 0x027f, 0),
            (false, 0x037f, 0x0080),
            (false, 0x037f, 0x0001),
            (false, 0x037f, 0x8000),
        ] {
            let mut state = LongModeState::default();
            state.rip = 0x9640;
            if !missing_state {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = control_word;
                x87.status_word = status_word;
                x87.registers[0] = normal;
                x87.registers[1] = normal;
                x87.set_tag(0, X87Tag::Valid);
                x87.set_tag(1, X87Tag::Valid);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xe9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd })
            );
            assert_eq!(state, before);
        }

        let mut state = LongModeState::default();
        state.rip = 0x9660;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.registers[1] = normal;
        x87.set_tag(0, X87Tag::Valid);
        x87.set_tag(1, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, stop_requested, fetch_fault) in [
            (&[0xdd][..], None, 1, false, false),
            (&[0xdd, 0xe9][..], Some(0x9660), 1, false, false),
            (&[0xdd, 0xe9][..], None, 0, false, false),
            (&[0xdd, 0xe9][..], None, 1, true, false),
            (&[0xdd, 0xe9][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || stop_requested,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
        }
    }

    #[test]
    fn ftst_rejects_invalid_source_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let invalid_sources = [
            None,
            Some((normal, X87Tag::Empty, 0x037f, 0)),
            Some((normal, X87Tag::Zero, 0x037f, 0)),
            Some((normal, X87Tag::Special, 0x037f, 0)),
            Some((normal, X87Tag::Valid, 0x027f, 0)),
            Some((normal, X87Tag::Valid, 0x037f, 1)),
            Some((normal, X87Tag::Valid, 0x037f, 0x80)),
            Some((normal, X87Tag::Valid, 0x037f, 0x8000)),
            Some(([0; 10], X87Tag::Valid, 0x037f, 0)),
            Some(([1, 0, 0, 0, 0, 0, 0, 0, 0, 0], X87Tag::Valid, 0x037f, 0)),
            Some(([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0], X87Tag::Valid, 0x037f, 0)),
            Some((
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
                X87Tag::Valid,
                0x037f,
                0,
            )),
            Some((
                [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
                X87Tag::Valid,
                0x037f,
                0,
            )),
            Some(([0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40], X87Tag::Valid, 0x037f, 0)),
        ];
        for (index, candidate) in invalid_sources.into_iter().enumerate() {
            let mut state = LongModeState::default();
            state.rip = 0x9000;
            state.gpr = [0xfeed_face_cafe_beef; 16];
            state.rflags = 0x0bad_f00d_0123_4567;
            if let Some((raw, tag, control_word, status_word)) = candidate {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = control_word;
                x87.status_word = status_word | 0x4700;
                x87.registers[0] = raw;
                x87.set_tag(0, tag);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xd9, 0xe4], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd9 }),
                "rejection={index}"
            );
            assert_eq!(state, before, "rejection={index}");
        }

        let mut state = LongModeState::default();
        state.rip = 0x9020;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0x4700;
        x87.registers[0] = normal;
        x87.set_tag(0, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, request_stop, fetch_fault) in [
            (&[0xd9][..], None, 1, false, false),
            (&[0xd9, 0xe3][..], None, 1, false, false),
            (&[0xf0, 0xd9, 0xe4][..], None, 1, false, false),
            (&[0x40, 0xd9, 0xe4][..], None, 1, false, false),
            (&[0x9b, 0xd9, 0xe4][..], None, 1, false, false),
            (&[0xd9, 0xe4][..], Some(0x9020), 1, false, false),
            (&[0xd9, 0xe4][..], None, 0, false, false),
            (&[0xd9, 0xe4][..], None, 1, true, false),
            (&[0xd9, 0xe4][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fabs_rejects_missing_or_invalid_source_and_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, true);
        let invalid_sources = [
            None,
            Some((normal, X87Tag::Empty, 0x037f, 0)),
            Some((normal, X87Tag::Valid, 0x027f, 0)),
            Some((normal, X87Tag::Valid, 0x037f, 1)),
            Some(([0; 10], X87Tag::Valid, 0x037f, 0)),
            Some(([1, 0, 0, 0, 0, 0, 0, 0, 0, 0], X87Tag::Valid, 0x037f, 0)),
            Some(([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0], X87Tag::Valid, 0x037f, 0)),
            Some((
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
                X87Tag::Valid,
                0x037f,
                0,
            )),
            Some((
                [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
                X87Tag::Valid,
                0x037f,
                0,
            )),
            Some((
                [0, 0, 0, 0, 0, 0, 0, 0x00, 0, 0x40],
                X87Tag::Valid,
                0x037f,
                0,
            )),
        ];
        for (index, candidate) in invalid_sources.into_iter().enumerate() {
            let mut state = LongModeState::default();
            state.rip = 0x8f60;
            state.gpr = [0xfeed_face_cafe_beef; 16];
            state.rflags = 0x0bad_f00d_0123_4567;
            if let Some((raw, tag, control_word, status_word)) = candidate {
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = control_word;
                x87.status_word = status_word | (1 << 9) | 0x4500;
                x87.registers[0] = raw;
                x87.set_tag(0, tag);
                state.x87 = Some(x87);
            }
            let before = state.clone();
            let mut memory = Memory::at(&[0xd9, 0xe1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd9 }),
                "rejection={index}"
            );
            assert_eq!(state, before, "rejection={index}");
        }

        let mut state = LongModeState::default();
        state.rip = 0x8f80;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.set_tag(0, X87Tag::Valid);
        state.x87 = Some(x87);
        for (bytes, stop_rip, budget, request_stop, fetch_fault) in [
            (&[0xd9][..], None, 1, false, false),
            (&[0xd9, 0xe1][..], Some(0x8f80), 1, false, false),
            (&[0xd9, 0xe1][..], None, 0, false, false),
            (&[0xd9, 0xe1][..], None, 1, true, false),
            (&[0xd9, 0xe1][..], None, 1, false, true),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fchs_rejects_invalid_source_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let invalid_raw = [
            [0; 10],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x00],
            [0, 0, 0, 0, 0, 0, 0, 0x00, 0xff, 0x3f],
        ];
        let mut rejected = Vec::new();
        for raw in invalid_raw {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.registers[0] = raw;
            x87.set_tag(0, X87Tag::Valid);
            rejected.push(x87);
        }
        for tag in [X87Tag::Zero, X87Tag::Special, X87Tag::Empty] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.registers[0] = normal;
            x87.set_tag(0, tag);
            rejected.push(x87);
        }
        for control_word in [0x027f, 0x037e, 0x077f] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = control_word;
            x87.status_word = 0;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            rejected.push(x87);
        }
        for status_word in [1, 0x80, 0x8000] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = status_word;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            rejected.push(x87);
        }
        for (index, x87) in rejected.into_iter().enumerate() {
            let mut state = LongModeState::default();
            state.rip = 0x8f80;
            state.gpr = [0xfeed_face_cafe_beef; 16];
            state.rflags = 0x0bad_f00d_0123_4567;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd9, 0xe0], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd9 }),
                "rejection={index}"
            );
            assert_eq!(state, before, "rejection={index}");
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x8f80;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xd9, 0xe0], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd9 })
        );
        assert_eq!(missing, before);

        let mut boundary = LongModeState::default();
        boundary.rip = 0x8fc0;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.set_tag(0, X87Tag::Valid);
        boundary.x87 = Some(x87);
        for (bytes, stop_rip, budget, request_stop, fetch_fault) in [
            (&[0xd9][..], None, 1, false, false),
            (&[0xd9, 0xe0][..], Some(0x8fc0), 1, false, false),
            (&[0xd9, 0xe0][..], None, 0, false, false),
            (&[0xd9, 0xe0][..], None, 1, true, false),
            (&[0xd9, 0xe0][..], None, 1, false, true),
        ] {
            let mut state = boundary.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fstp_sti_rejects_invalid_source_and_environment_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let invalid_raw = [
            [0; 10],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x00],
            [0, 0, 0, 0, 0, 0, 0, 0x00, 0xff, 0x3f],
        ];
        for rejection in 0..12 {
            let mut state = LongModeState::default();
            state.rip = 0x9900;
            state.gpr = [0xfeed_face_cafe_beef; 16];
            state.rflags = 0x0bad_f00d_0123_4567;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0x0200;
            x87.tag_word = 0;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            match rejection {
                0..=5 => x87.registers[0] = invalid_raw[rejection],
                6..=8 => {
                    x87.set_tag(0, destination_tags_for_rejection(rejection));
                }
                9 => x87.control_word = 0x027f,
                10 => x87.status_word |= 1,
                11 => x87.status_word |= 1 << 15,
                _ => unreachable!(),
            }
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xd9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd }),
                "rejection={rejection}"
            );
            assert_eq!(state, before, "rejection={rejection}");
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x9900;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xdd, 0xd9], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdd })
        );
        assert_eq!(missing, before);

        let mut boundary = LongModeState::default();
        boundary.rip = 0x9a00;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.set_tag(0, X87Tag::Valid);
        boundary.x87 = Some(x87);
        for (bytes, stop_rip, budget, fetch_fault) in [
            (&[0xdd][..], None, 1, false),
            (&[0xdd, 0xd9][..], Some(0x9a00), 1, false),
            (&[0xdd, 0xd9][..], None, 0, false),
            (&[0xdd, 0xd9][..], None, 1, true),
        ] {
            let before = boundary.clone();
            let mut memory = Memory::at(bytes, boundary.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    boundary.rip + 1,
                    MemoryError::Unmapped {
                        address: boundary.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut boundary,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(boundary, before);
        }
    }

    #[test]
    fn fst_sti_copies_raw_st0_without_pop_for_every_top_and_index() {
        let raw_sources = [
            binary80_normal(0x8000_0000_0000_0001, 1, false),
            binary80_normal(0xffff_ffff_ffff_ffff, 0x7ffe, true),
            binary80_normal(0x8123_4567_89ab_cdef, 0x4000, true),
        ];
        let destination_tags = [X87Tag::Valid, X87Tag::Zero, X87Tag::Special, X87Tag::Empty];
        for top in 0u8..8 {
            for destination_index in 0u8..8 {
                for destination_tag_index in 0..if destination_index == 0 { 1 } else { 4 } {
                    let physical_destination = (top + destination_index) & 7;
                    let source = raw_sources[usize::from((top + destination_index) % 3)];
                    let mut state = LongModeState::default();
                    state.rip = 0x9b00;
                    state.gpr = std::array::from_fn(|index| 0x1234_5678_9abc_def0 ^ index as u64);
                    state.rflags = 0xa55a_1234_5678_9abc;
                    let mut x87 = crate::state::X87State::fixture_seed();
                    x87.control_word = 0x037f;
                    x87.status_word = (u16::from(top) << 11) | 0x4500 | (1 << 9);
                    x87.tag_word = u16::MAX;
                    for physical in 0..8 {
                        x87.registers[physical] = [0x50 + physical as u8; 10];
                    }
                    x87.registers[usize::from(top)] = source;
                    x87.set_tag(top, X87Tag::Valid);
                    if physical_destination != top {
                        x87.set_tag(
                            physical_destination,
                            destination_tags[destination_tag_index],
                        );
                    }
                    x87.instruction_pointer = 0x1234;
                    x87.data_pointer = 0x5678;
                    x87.last_opcode = 0x05aa;
                    state.x87 = Some(x87);
                    let before = state.clone();
                    let mut memory = Memory::at(&[0xdd, 0xd0 | destination_index], state.rip);
                    assert_eq!(
                        step(&mut state, &mut memory, AccessContext::default()),
                        Ok(StepOutcome::Advanced),
                        "TOP={top}, i={destination_index}"
                    );
                    let mut expected = before.clone();
                    let expected_x87 = expected.x87.as_mut().unwrap();
                    expected_x87.registers[usize::from(physical_destination)] = source;
                    expected_x87.set_tag(physical_destination, X87Tag::Valid);
                    expected_x87.instruction_pointer = before.rip;
                    expected.rip += 2;
                    assert_eq!(
                        state, expected,
                        "full state TOP={top}, i={destination_index}"
                    );
                    assert_eq!(
                        state.x87.as_ref().unwrap().status_word,
                        before.x87.as_ref().unwrap().status_word,
                        "complete FSW preservation TOP={top}, i={destination_index}"
                    );
                }
            }
        }
    }

    #[test]
    fn fst_sti_rejects_invalid_source_and_execution_boundaries_atomically() {
        let normal = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let invalid_raw = [
            [0; 10],
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x00],
            [0, 0, 0, 0, 0, 0, 0, 0x00, 0xff, 0x3f],
        ];
        let mut rejected = Vec::new();
        for raw in invalid_raw {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.set_tag(0, X87Tag::Valid);
            x87.registers[0] = raw;
            rejected.push(x87);
        }
        for tag in [X87Tag::Zero, X87Tag::Special, X87Tag::Empty] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.registers[0] = normal;
            x87.set_tag(0, tag);
            rejected.push(x87);
        }
        for control_word in [0x027f, 0x037e, 0x077f] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = control_word;
            x87.status_word = 0;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            rejected.push(x87);
        }
        for status_word in [1, 0x80, 0x8000] {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = status_word;
            x87.registers[0] = normal;
            x87.set_tag(0, X87Tag::Valid);
            rejected.push(x87);
        }
        for (index, x87) in rejected.into_iter().enumerate() {
            let mut state = LongModeState::default();
            state.rip = 0x9c00;
            state.gpr = [0xfeed_face_cafe_beef; 16];
            state.rflags = 0x0bad_f00d_0123_4567;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdd, 0xd1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdd }),
                "rejection={index}"
            );
            assert_eq!(state, before, "rejection={index}");
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x9c00;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xdd, 0xd1], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdd })
        );
        assert_eq!(missing, before);

        let mut boundary = LongModeState::default();
        boundary.rip = 0x9d00;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.registers[0] = normal;
        x87.set_tag(0, X87Tag::Valid);
        boundary.x87 = Some(x87);
        for (bytes, stop_rip, budget, request_stop, fetch_fault) in [
            (&[0xdd][..], None, 1, false, false),
            (&[0xdd, 0xd1][..], Some(0x9d00), 1, false, false),
            (&[0xdd, 0xd1][..], None, 0, false, false),
            (&[0xdd, 0xd1][..], None, 1, true, false),
            (&[0xdd, 0xd1][..], None, 1, false, true),
        ] {
            let mut state = boundary.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    fn destination_tags_for_rejection(index: usize) -> X87Tag {
        match index {
            6 => X87Tag::Zero,
            7 => X87Tag::Special,
            8 => X87Tag::Empty,
            _ => unreachable!(),
        }
    }

    #[test]
    fn faddp_sti_st0_rejects_without_any_state_commit_including_i0_overflow() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let mut cases = Vec::new();
        for destination_index in 0u8..=7 {
            let top = 5u8;
            let physical_destination = top.wrapping_add(destination_index) & 7;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = u16::from(top) << 11;
            x87.tag_word = 0;
            x87.registers[usize::from(top)] = one;
            x87.registers[usize::from(physical_destination)] = one;
            let mut bad_source_tag = x87.clone();
            bad_source_tag.set_tag(top, X87Tag::Empty);
            cases.push((destination_index, bad_source_tag));

            if destination_index != 0 {
                let mut special = x87.clone();
                special.registers[usize::from(top)] =
                    binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
                cases.push((destination_index, special));
                let mut unequal_exponents = x87.clone();
                unequal_exponents.registers[usize::from(top)] =
                    binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
                cases.push((destination_index, unequal_exponents));
                let mut inexact = x87.clone();
                inexact.registers[usize::from(physical_destination)] =
                    binary80_normal(u64::MAX, 0x3fff, false);
                inexact.registers[usize::from(top)] = one;
                cases.push((destination_index, inexact));
            }
            let mut bad_destination_tag = x87.clone();
            bad_destination_tag.set_tag(physical_destination, X87Tag::Empty);
            cases.push((destination_index, bad_destination_tag));
        }
        let mut overflow_i0 = crate::state::X87State::fixture_seed();
        overflow_i0.control_word = 0x037f;
        overflow_i0.status_word = 3 << 11;
        overflow_i0.tag_word = 0;
        overflow_i0.registers[3] = binary80_normal(u64::MAX, 0x7ffe, false);
        cases.push((0, overflow_i0));

        for (destination_index, x87) in cases {
            let opcode = [0xde, 0xc0 | destination_index];
            let mut state = LongModeState::default();
            state.rip = 0x3000;
            state.gpr = [0x0123_4567_89ab_cdef; 16];
            state.rflags = 0xa55a_1234_5678_9abc;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&opcode, 0x3000);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde }),
                "ST({destination_index})"
            );
            assert_eq!(state, before);
        }
    }

    #[test]
    fn faddp_sti_st0_missing_state_fetch_fault_stop_and_budget_are_atomic() {
        let opcode = [0xde, 0xc1];
        let mut absent = LongModeState::default();
        absent.rip = 0x4000;
        let before_absent = absent.clone();
        let mut memory = Memory::at(&opcode, 0x4000);
        assert_eq!(
            step(&mut absent, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(absent, before_absent);

        let mut state = LongModeState::default();
        state.rip = 0x4000;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        x87.registers[1] = x87.registers[0];
        state.x87 = Some(x87);

        for (fault_at, stop_rip, budget) in
            [(Some(1), None, 1), (None, Some(0x4000), 1), (None, None, 0)]
        {
            let before = state.clone();
            let mut memory = Memory::at(
                if fault_at == Some(1) {
                    &[0xde][..]
                } else {
                    &opcode
                },
                0x4000,
            );
            if fault_at.is_some() {
                memory.fetch_error = Some((
                    0x4001,
                    MemoryError::Unmapped {
                        address: 0x4001,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
            if fault_at.is_some() {
                assert_eq!(
                    result.exit,
                    RunExit::MemoryError(MemoryError::Unmapped {
                        address: 0x4001,
                        access: AccessKind::Fetch,
                    })
                );
            } else if stop_rip.is_some() {
                assert_eq!(result.exit, RunExit::StopAddress);
            } else {
                assert_eq!(result.exit, RunExit::InstructionBudgetExhausted);
            }
        }
    }

    #[test]
    fn fnstsw_ax_copies_raw_status_and_preserves_local_state() {
        for status_word in [0x0000, 0xffff, 0x2800, 0x45a5, 0x8001] {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = std::array::from_fn(|index| {
                0xa1b2_c3d4_e5f6_0718u64 ^ (index as u64).wrapping_mul(0x1111_1111)
            });
            state.gpr[Register::Rax as usize] = 0xfeed_cafe_89ab_7654;
            state.rflags = 0xa55a_1234_5678_9abc;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.status_word = status_word;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdf, 0xe0], 0x1000);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(0x1002),
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(state.rip, 0x1002);
            assert_eq!(
                state.gpr[Register::Rax as usize] & 0xffff,
                u64::from(status_word)
            );
            assert_eq!(state.gpr[Register::Rax as usize] >> 16, before.gpr[0] >> 16);
            for index in 1..state.gpr.len() {
                assert_eq!(state.gpr[index], before.gpr[index]);
            }
            assert_eq!(state.rflags, before.rflags);
            // C0/C1/C2/C3 are architecturally undefined after FNSTSW. The
            // local profile deterministically leaves its raw x87 snapshot intact.
            assert_eq!(state.x87, before.x87);
        }
    }

    #[test]
    fn fnstsw_ax_missing_state_and_fetch_or_boundary_stops_do_not_commit() {
        let mut absent = LongModeState::default();
        let before = absent.clone();
        let mut complete = Memory::new(&[0xdf, 0xe0]);
        assert_eq!(
            step(&mut absent, &mut complete, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdf })
        );
        assert_eq!(absent, before);

        let mut initial = LongModeState::default();
        initial.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
        initial.x87 = Some(crate::state::X87State::fixture_seed());
        for (bytes, fault, expected_exit) in [
            (&[0xdf][..], None, None),
            (
                &[0xdf, 0xe0][..],
                Some(1),
                Some(RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(4),
                    linear_address: Some(1),
                })),
            ),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::new(bytes);
            if fault.is_some() {
                memory.fetch_error = Some((
                    1,
                    MemoryError::Architectural(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(4),
                        linear_address: Some(1),
                    }),
                ));
            }
            if let Some(expected_exit) = expected_exit {
                let result = run(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: 1,
                        stop_rip: None,
                        context: AccessContext::default(),
                    },
                    || false,
                );
                assert_eq!(result.exit, expected_exit);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert!(matches!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Err(MemoryError::Unmapped {
                        access: AccessKind::Fetch,
                        ..
                    })
                ));
            }
            assert_eq!(state, before);
        }

        for (stop_rip, budget, expected_exit) in [
            (Some(0), 1, RunExit::StopAddress),
            (None, 0, RunExit::InstructionBudgetExhausted),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::new(&[0xdf, 0xe0]);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, expected_exit);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fnclex_clears_only_architecturally_cleared_status_bits() {
        for (status_word, expected_status) in [
            (0x0001, 0x0000),
            (0x0002, 0x0000),
            (0x0004, 0x0000),
            (0x0008, 0x0000),
            (0x0010, 0x0000),
            (0x0020, 0x0000),
            (0x0040, 0x0000),
            (0x0080, 0x0000),
            (0x8000, 0x0000),
            (0xffff, 0x7f00),
            (0xa5c3, 0x2500),
            (0x3800, 0x3800),
            (0x4700, 0x4700),
            (0xc700, 0x4700),
        ] {
            let mut state = LongModeState::default();
            state.rip = 0x1000;
            state.gpr = std::array::from_fn(|index| {
                0x1234_5678_9abc_def0u64 ^ (index as u64).wrapping_mul(0x1111_1111)
            });
            state.rflags = 0xa55a_1234_5678_9abc;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x0072;
            x87.status_word = status_word;
            x87.tag_word = 0x1234;
            x87.instruction_pointer = 0x1234_5678_9abc_def0;
            x87.data_pointer = 0xfedc_ba98_7654_3210;
            x87.last_opcode = 0x0567;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdb, 0xe2], 0x1000);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: 1,
                    stop_rip: Some(0x1002),
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, RunExit::StopAddress);
            assert_eq!(result.instructions_executed, 1);
            assert_eq!(state.rip, 0x1002);
            assert_eq!(state.gpr, before.gpr);
            assert_eq!(state.rflags, before.rflags);
            let observed = state.x87.as_ref().unwrap();
            assert_eq!(observed.status_word, expected_status);
            let mut expected_x87 = before.x87.unwrap();
            expected_x87.status_word = expected_status;
            assert_eq!(observed, &expected_x87);
        }
    }

    #[test]
    fn fnclex_is_idempotent_and_missing_state_or_unretired_execution_does_not_commit() {
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.status_word = 0xffff;
        x87.fnclex();
        assert_eq!(x87.status_word, 0x7f00);
        x87.fnclex();
        assert_eq!(x87.status_word, 0x7f00);

        let mut absent = LongModeState::default();
        let before_absent = absent.clone();
        let mut complete = Memory::new(&[0xdb, 0xe2]);
        assert_eq!(
            step(&mut absent, &mut complete, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdb })
        );
        assert_eq!(absent, before_absent);

        let mut initial = LongModeState::default();
        initial.x87 = Some(crate::state::X87State::fixture_seed());
        for (bytes, inject_fault, expected_exit) in [
            (&[0xdb][..], false, None),
            (
                &[0xdb, 0xe2][..],
                true,
                Some(RunExit::Fault(ArchitecturalFault {
                    vector: 14,
                    error_code: Some(4),
                    linear_address: Some(1),
                })),
            ),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::new(bytes);
            if inject_fault {
                memory.fetch_error = Some((
                    1,
                    MemoryError::Architectural(ArchitecturalFault {
                        vector: 14,
                        error_code: Some(4),
                        linear_address: Some(1),
                    }),
                ));
            }
            if let Some(expected_exit) = expected_exit {
                let result = run(
                    &mut state,
                    &mut memory,
                    RunConfig {
                        instruction_budget: 1,
                        stop_rip: None,
                        context: AccessContext::default(),
                    },
                    || false,
                );
                assert_eq!(result.exit, expected_exit);
                assert_eq!(result.instructions_executed, 0);
            } else {
                assert!(matches!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Err(MemoryError::Unmapped {
                        access: AccessKind::Fetch,
                        ..
                    })
                ));
            }
            assert_eq!(state, before);
        }

        for (stop_rip, budget, expected_exit) in [
            (Some(0), 1, RunExit::StopAddress),
            (None, 0, RunExit::InstructionBudgetExhausted),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::new(&[0xdb, 0xe2]);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, expected_exit);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fninit_missing_state_and_fetch_or_boundary_stops_do_not_commit() {
        let mut absent = LongModeState::default();
        let before = absent.clone();
        let mut complete = Memory::new(&[0xdb, 0xe3]);
        assert_eq!(
            step(&mut absent, &mut complete, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdb })
        );
        assert_eq!(absent, before);

        let mut initial = LongModeState::default();
        initial.x87 = Some(crate::state::X87State::fixture_seed());
        let mut truncated = initial.clone();
        truncated.rip = 0;
        let before = truncated.clone();
        let mut short_code = Memory::new(&[0xdb]);
        assert!(matches!(
            step(&mut truncated, &mut short_code, AccessContext::default()),
            Err(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            })
        ));
        assert_eq!(truncated, before);

        let mut faulted = initial.clone();
        faulted.rip = 0;
        let before = faulted.clone();
        let mut fault_memory = Memory::new(&[0xdb, 0xe3]);
        fault_memory.fetch_error = Some((
            1,
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(4),
                linear_address: Some(1),
            }),
        ));
        assert!(matches!(
            step(&mut faulted, &mut fault_memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault { vector: 14, .. }))
        ));
        assert_eq!(faulted, before);

        for (stop, budget, expected) in [
            (Some(0), 1, RunExit::StopAddress),
            (None, 0, RunExit::InstructionBudgetExhausted),
        ] {
            let mut stopped = initial.clone();
            let before = stopped.clone();
            let mut code = Memory::new(&[0xdb, 0xe3]);
            let result = run(
                &mut stopped,
                &mut code,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, expected);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(stopped, before);
            assert!(code.accesses.is_empty());
        }
    }

    #[test]
    fn fmul_st0_sti_multiplies_exactly_for_all_top_and_indices_without_pop() {
        let one_and_half = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let two_and_quarter = binary80_normal(0x9000_0000_0000_0000, 0x4000, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let physical_top = usize::from(top);
                let physical_source = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7400;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x30 + physical * 7 + byte) as u8)
                });
                x87.registers[physical_top] = one_and_half;
                x87.set_tag(top, X87Tag::Valid);
                if index == 0 {
                    // i=0 is the defined square ST(0)*ST(0), and reads one tag.
                } else {
                    x87.registers[physical_source] = two;
                    x87.set_tag(physical_source as u8, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xd8, 0xc8 | index], 0x7400);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_top] =
                    if index == 0 { two_and_quarter } else { three };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(
                    state, expected,
                    "complete physical state TOP={top}, i={index}"
                );
                assert_eq!(state.x87.as_ref().unwrap().top(), top);
                assert_eq!(
                    state.x87.as_ref().unwrap().tag_word,
                    before.x87.as_ref().unwrap().tag_word
                );
                if index != 0 {
                    assert_eq!(
                        state.x87.as_ref().unwrap().registers[physical_source],
                        before.x87.as_ref().unwrap().registers[physical_source]
                    );
                }
            }
        }

        let expected_magnitude = binary80_normal(0xc000_0000_0000_0000, 0x3ffe, false);
        for (a_sign, b_sign) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, a_sign);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, b_sign);
            state.x87 = Some(x87);
            let mut memory = Memory::at(&[0xd8, 0xc9], 0);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            let mut bytes = expected_magnitude;
            bytes[9] |= u8::from(a_sign ^ b_sign) << 7;
            assert_eq!(state.x87.as_ref().unwrap().registers[0], bytes);
        }

        let mut state = LongModeState::default();
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(u64::MAX, 0x3fff, false);
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        state.x87 = Some(x87);
        let mut memory = Memory::at(&[0xd8, 0xc9], 0);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            state.x87.as_ref().unwrap().registers[0],
            binary80_normal(u64::MAX, 0x4000, false)
        );
    }

    #[test]
    fn fmul_sti_st0_writes_selected_physical_destination_without_pop() {
        let one_and_half = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let two_and_quarter = binary80_normal(0x9000_0000_0000_0000, 0x4000, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let physical_top = usize::from(top);
                let physical_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7800;
                state.gpr = std::array::from_fn(|n| 0xa500_0000_0000_0000 ^ n as u64);
                state.rflags = 0x95aa_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x70 + physical * 7 + byte) as u8)
                });
                x87.registers[physical_top] = one_and_half;
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[physical_destination] = two;
                    x87.set_tag(physical_destination as u8, X87Tag::Valid);
                }
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdc, 0xc8 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_destination] =
                    if index == 0 { two_and_quarter } else { three };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        for (source_negative, destination_negative) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, source_negative);
            x87.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x3ffe, destination_negative);
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xdc, 0xc9]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(
                state.x87.as_ref().unwrap().registers[1],
                binary80_normal(
                    0x9000_0000_0000_0000,
                    0x3fff,
                    source_negative ^ destination_negative,
                )
            );
        }

        let full_significand = binary80_normal(u64::MAX, 0x3fff, false);
        let power_of_two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let mut state = LongModeState::default();
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = power_of_two;
        x87.registers[1] = full_significand;
        state.x87 = Some(x87);
        let mut memory = Memory::new(&[0xdc, 0xc9]);
        assert_eq!(
            step(&mut state, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            state.x87.as_ref().unwrap().registers[1],
            binary80_normal(u64::MAX, 0x4000, false)
        );
    }

    #[test]
    fn fdiv_st0_sti_commits_only_exact_quotients_for_every_top_and_index() {
        let fifteen = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = binary80_normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let mut state = LongModeState::default();
                state.rip = 0x7e00;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let old_top = usize::from(top);
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
                });
                if index == 0 {
                    // Aliased FDIV reads one Valid tag and computes x/x as +1.
                    x87.registers[old_top] =
                        binary80_normal(0xc123_4567_89ab_cdef, 0x4000, top & 1 != 0);
                } else {
                    x87.registers[old_top] = fifteen;
                    let source = usize::from((top + index) & 7);
                    x87.registers[source] = three;
                    x87.set_tag((top + index) & 7, crate::state::X87Tag::Valid);
                }
                x87.set_tag(top, crate::state::X87Tag::Valid);
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xd8, 0xf0 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[old_top] = if index == 0 { one } else { five };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        // Unequal exponents, a quotient normalized through lhs < rhs, full-width
        // significands, and all sign combinations use the exact integer path.
        for (lhs, rhs, expected) in [
            (
                binary80_normal(0x9000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3ffe, false),
            ), // lhs significand < rhs; 1.125/1.5 = 0.75
            (
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                one,
                binary80_normal(u64::MAX, 0x3fff, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                one,
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
            ), // minimum Normal result
            (
                binary80_normal(u64::MAX, 0x7ffe, false),
                one,
                binary80_normal(u64::MAX, 0x7ffe, false),
            ), // maximum Normal result
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = lhs;
            x87.registers[1] = rhs;
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xd8, 0xf1]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[0], expected);
        }
        for input in [
            binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
            binary80_normal(0x8000_0000_0000_0000, 0x0001, true),
            binary80_normal(u64::MAX, 0x7ffe, false),
            binary80_normal(u64::MAX, 0x7ffe, true),
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfffc;
            x87.registers[0] = input;
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xd8, 0xf0]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(
                state.x87.as_ref().unwrap().registers[0],
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, false)
            );
        }
        for (lhs_negative, rhs_negative) in
            [(false, false), (false, true), (true, false), (true, true)]
        {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, lhs_negative);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, rhs_negative);
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xd8, 0xf1]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(
                state.x87.as_ref().unwrap().registers[0],
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, lhs_negative ^ rhs_negative)
            );
        }
    }

    #[test]
    fn fdivr_st0_sti_commits_exact_sti_over_st0_without_pop() {
        let fifteen = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = binary80_normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let physical_top = usize::from(top);
                let physical_source = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7e30;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
                });
                x87.registers[physical_top] = if index == 0 { one } else { three };
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[physical_source] = fifteen;
                    x87.set_tag((top + index) & 7, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xd8, 0xf8 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_top] = if index == 0 { one } else { five };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        // The direction is observable: old ST(i)=15 divided by old ST(0)=3.
        for (source, denominator, expected) in [
            (fifteen, three, five),
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, true),
                three,
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                fifteen,
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                binary80_normal(0x9000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3ffe, false),
            ), // 1.125 / 1.5 = 0.75, with unequal exponents permitted.
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                one,
                binary80_normal(u64::MAX, 0x3fff, false),
            ),
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = denominator;
            x87.registers[1] = source;
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xd8, 0xf9]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[0], expected);
        }
    }

    #[test]
    fn fdivr_st0_sti_rejects_inexact_and_invalid_inputs_atomically() {
        let opcode = [0xd8, 0xf9];
        let mut base = LongModeState::default();
        base.rip = 0x7e40;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false); // 3
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false); // 1/3
        base.x87 = Some(x87);
        let mut rejects = vec![base.clone()];
        let mut bad_control = base.clone();
        bad_control.x87.as_mut().unwrap().control_word ^= 1;
        rejects.push(bad_control);
        let mut bad_status = base.clone();
        bad_status.x87.as_mut().unwrap().status_word |= 1;
        rejects.push(bad_status);
        let mut bad_tag = base.clone();
        bad_tag.x87.as_mut().unwrap().set_tag(1, X87Tag::Empty);
        rejects.push(bad_tag);
        let mut noncanonical = base.clone();
        noncanonical.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x4000_0000_0000_0000, 0x3fff, false);
        rejects.push(noncanonical);
        let mut special = base.clone();
        special.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
        rejects.push(special);
        let mut zero_operand = base.clone();
        zero_operand.x87.as_mut().unwrap().registers[1] = [0; 10];
        rejects.push(zero_operand);
        let mut no_state = base.clone();
        no_state.x87 = None;
        rejects.push(no_state);
        for (numerator, denominator) in [
            (
                binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ), // finite quotient overflows the canonical Normal range.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ), // exact quotient is subnormal.
        ] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[0] = denominator;
            state.x87.as_mut().unwrap().registers[1] = numerator;
            rejects.push(state);
        }
        for mut state in rejects {
            let before = state.clone();
            let mut memory = Memory::at(&opcode, state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }

        let mut state = base.clone();
        let before = state.clone();
        let mut memory = Memory::at(&opcode[..1], state.rip);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);

        let mut faulted = base.clone();
        let before = faulted.clone();
        let mut memory = Memory::at(&opcode, faulted.rip);
        memory.fetch_error = Some((faulted.rip + 1, MemoryError::Backend("fetch failed".into())));
        assert_eq!(
            step(&mut faulted, &mut memory, AccessContext::default()),
            Err(MemoryError::Backend("fetch failed".into()))
        );
        assert_eq!(faulted, before);

        for (truncated, fetch_fault, stop_rip, budget) in [
            (true, false, None, 1),
            (false, false, Some(base.rip), 1),
            (false, false, None, 0),
            (false, true, None, 1),
        ] {
            let mut state = base.clone();
            let before = state.clone();
            let bytes = if truncated { &opcode[..1] } else { &opcode[..] };
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
            let expected_exit = if truncated || fetch_fault {
                RunExit::MemoryError(MemoryError::Unmapped {
                    address: state.rip + 1,
                    access: AccessKind::Fetch,
                })
            } else if stop_rip.is_some() {
                RunExit::StopAddress
            } else {
                RunExit::InstructionBudgetExhausted
            };
            assert_eq!(result.exit, expected_exit);
        }
    }

    #[test]
    fn fdivr_sti_st0_writes_exact_st0_over_sti_without_pop() {
        let six = binary80_normal(0xc000_0000_0000_0000, 0x4001, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);

        for top in 0u8..8 {
            for index in 0u8..8 {
                let physical_top = usize::from(top);
                let physical_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7e30;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x0700 | (1 << 9);
                x87.tag_word = 0xffff;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
                });
                x87.registers[physical_top] = if index == 0 { one } else { six };
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[physical_destination] = three;
                    x87.set_tag((top + index) & 7, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdc, 0xf0 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );

                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_destination] = if index == 0 { one } else { two };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        // The direction and the non-power-of-two quotient are observable.
        for (numerator, denominator, expected) in [
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, false),
                three,
                binary80_normal(0xa000_0000_0000_0000, 0x4001, false),
            ), // 15/3 = 5.
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, true),
                three,
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                six,
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, true),
            ),
            (
                binary80_normal(0xc000_0000_0000_0000, 0x4001, true),
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                two,
            ),
            (
                binary80_normal(0x9000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3ffe, false),
            ), // 1.125/1.5 = 0.75, with unequal exponents.
            (
                binary80_normal(u64::MAX, 0x7ffe, false),
                binary80_normal(u64::MAX, 0x7ffe, false),
                one,
            ), // Upper normal edge.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, true),
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
            ), // Lower normal edge.
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = numerator;
            x87.registers[1] = denominator;
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xdc, 0xf1]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[1], expected);
        }
    }

    #[test]
    fn fdiv_sti_st0_commits_exact_quotients_for_every_top_and_index() {
        let fifteen = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = binary80_normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);

        for top in 0u8..8 {
            for index in 0u8..8 {
                let physical_top = usize::from(top);
                let physical_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7e70;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x60 + physical * 7 + byte) as u8)
                });
                x87.registers[physical_top] = if index == 0 { one } else { three };
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[physical_destination] = fifteen;
                    x87.set_tag((top + index) & 7, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xdc, 0xf8 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );

                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_destination] = if index == 0 { one } else { five };
                expected_x87.status_word &= !(1 << 9);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        // Ordered operands are observable: old ST(i)=15 divided by old ST(0)=3.
        for (numerator, denominator, result) in [
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, false),
                three,
                five,
            ),
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, true),
                three,
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, false),
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                binary80_normal(0x9000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x3ffe, false),
            ), // 1.125/1.5 = 0.75, with differing exponents.
            (
                binary80_normal(u64::MAX, 0x7ffe, true),
                binary80_normal(u64::MAX, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
            ), // Upper Normal edge.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, true),
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3fff, true),
            ), // Lower Normal edge.
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = denominator;
            x87.registers[1] = numerator;
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xdc, 0xf9]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[1], result);
        }
    }

    #[test]
    fn fdiv_sti_st0_rejects_inexact_and_invalid_inputs_atomically() {
        let opcode = [0xdc, 0xf9];
        let mut base = LongModeState::default();
        base.rip = 0x7e80;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        base.x87 = Some(x87);

        let mut rejects = vec![base.clone()]; // 1/3 is inexact with both tags Valid.
        let mut bad_control = base.clone();
        bad_control.x87.as_mut().unwrap().control_word ^= 1;
        rejects.push(bad_control);
        let mut bad_status = base.clone();
        bad_status.x87.as_mut().unwrap().status_word |= 1;
        rejects.push(bad_status);
        let mut bad_top_tag = base.clone();
        bad_top_tag.x87.as_mut().unwrap().set_tag(0, X87Tag::Empty);
        rejects.push(bad_top_tag);
        let mut bad_destination_tag = base.clone();
        bad_destination_tag
            .x87
            .as_mut()
            .unwrap()
            .set_tag(1, X87Tag::Empty);
        rejects.push(bad_destination_tag);
        let mut noncanonical = base.clone();
        noncanonical.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x4000_0000_0000_0000, 0x4000, false);
        rejects.push(noncanonical);
        let mut special = base.clone();
        special.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
        rejects.push(special);
        let mut zero_operand = base.clone();
        zero_operand.x87.as_mut().unwrap().registers[1] = [0; 10];
        rejects.push(zero_operand);
        let mut no_state = base.clone();
        no_state.x87 = None;
        rejects.push(no_state);
        for (numerator, denominator) in [
            (
                binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ), // Overflow.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ), // Subnormal output.
        ] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[0] = denominator;
            state.x87.as_mut().unwrap().registers[1] = numerator;
            rejects.push(state);
        }
        for mut state in rejects {
            let before = state.clone();
            let mut memory = Memory::at(&opcode, state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }

        let mut state = base.clone();
        let before = state.clone();
        let mut memory = Memory::at(&opcode[..1], state.rip);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);

        let mut faulted = base.clone();
        let before = faulted.clone();
        let mut memory = Memory::at(&opcode, faulted.rip);
        memory.fetch_error = Some((faulted.rip + 1, MemoryError::Backend("fetch failed".into())));
        assert_eq!(
            step(&mut faulted, &mut memory, AccessContext::default()),
            Err(MemoryError::Backend("fetch failed".into()))
        );
        assert_eq!(faulted, before);

        for (stop_rip, budget) in [(Some(base.rip), 1), (None, 0)] {
            let mut state = base.clone();
            let before = state.clone();
            let mut memory = Memory::at(&opcode, state.rip);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(
                result.exit,
                if stop_rip.is_some() {
                    RunExit::StopAddress
                } else {
                    RunExit::InstructionBudgetExhausted
                }
            );
        }
    }

    #[test]
    fn fdivr_sti_st0_rejects_inexact_and_invalid_inputs_atomically() {
        let opcode = [0xdc, 0xf1];
        let mut base = LongModeState::default();
        base.rip = 0x7e40;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x4001, false); // 6
        x87.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false); // 3
        base.x87 = Some(x87);
        let mut rejects = Vec::new();
        let mut inexact = base.clone();
        inexact.x87.as_mut().unwrap().registers[0] =
            binary80_normal(0x8000_0000_0000_0000, 0x3fff, false); // 1
        rejects.push(inexact); // 1/3 is inexact with both tags Valid.
        let mut bad_control = base.clone();
        bad_control.x87.as_mut().unwrap().control_word ^= 1;
        rejects.push(bad_control);
        let mut bad_status = base.clone();
        bad_status.x87.as_mut().unwrap().status_word |= 1;
        rejects.push(bad_status);
        let mut bad_top_tag = base.clone();
        bad_top_tag.x87.as_mut().unwrap().set_tag(0, X87Tag::Empty);
        rejects.push(bad_top_tag);
        let mut bad_destination_tag = base.clone();
        bad_destination_tag
            .x87
            .as_mut()
            .unwrap()
            .set_tag(1, X87Tag::Empty);
        rejects.push(bad_destination_tag);
        let mut noncanonical = base.clone();
        noncanonical.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x4000_0000_0000_0000, 0x4000, false);
        rejects.push(noncanonical);
        let mut special = base.clone();
        special.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
        rejects.push(special);
        let mut zero_operand = base.clone();
        zero_operand.x87.as_mut().unwrap().registers[1] = [0; 10];
        rejects.push(zero_operand);
        let mut no_state = base.clone();
        no_state.x87 = None;
        rejects.push(no_state);
        for (numerator, denominator) in [
            (
                binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ), // Overflow.
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ), // Subnormal output.
        ] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[0] = numerator;
            state.x87.as_mut().unwrap().registers[1] = denominator;
            rejects.push(state);
        }
        for mut state in rejects {
            let before = state.clone();
            let mut memory = Memory::at(&opcode, state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }

        let mut state = base.clone();
        let before = state.clone();
        let mut memory = Memory::at(&opcode[..1], state.rip);
        assert!(matches!(
            step(&mut state, &mut memory, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);

        let mut faulted = base.clone();
        let before = faulted.clone();
        let mut memory = Memory::at(&opcode, faulted.rip);
        memory.fetch_error = Some((faulted.rip + 1, MemoryError::Backend("fetch failed".into())));
        assert_eq!(
            step(&mut faulted, &mut memory, AccessContext::default()),
            Err(MemoryError::Backend("fetch failed".into()))
        );
        assert_eq!(faulted, before);

        for (truncated, fetch_fault, stop_rip, budget) in [
            (true, false, None, 1),
            (false, false, Some(base.rip), 1),
            (false, false, None, 0),
            (false, true, None, 1),
        ] {
            let mut state = base.clone();
            let before = state.clone();
            let bytes = if truncated { &opcode[..1] } else { &opcode[..] };
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
            let expected_exit = if truncated || fetch_fault {
                RunExit::MemoryError(MemoryError::Unmapped {
                    address: state.rip + 1,
                    access: AccessKind::Fetch,
                })
            } else if stop_rip.is_some() {
                RunExit::StopAddress
            } else {
                RunExit::InstructionBudgetExhausted
            };
            assert_eq!(result.exit, expected_exit);
        }
    }

    #[test]
    fn fdiv_st0_sti_rejections_and_execution_boundaries_are_atomic() {
        let opcode = [0xd8, 0xf1];
        let mut base = LongModeState::default();
        base.rip = 0x7f00;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        x87.registers[1] = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        base.x87 = Some(x87);

        let mut rejects = Vec::new();
        let mut inexact = base.clone();
        inexact.x87.as_mut().unwrap().registers[0] =
            binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        inexact.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0xc000_0000_0000_0000, 0x3fff, false); // 1/1.5 = 2/3
        rejects.push(inexact);
        let mut zero_divisor = base.clone();
        zero_divisor.x87.as_mut().unwrap().registers[1] = [0; 10];
        rejects.push(zero_divisor);
        let mut zero_numerator = base.clone();
        zero_numerator.x87.as_mut().unwrap().registers[0] = [0; 10];
        rejects.push(zero_numerator);
        let mut invalid_control = base.clone();
        invalid_control.x87.as_mut().unwrap().control_word ^= 1;
        rejects.push(invalid_control);
        let mut invalid_status = base.clone();
        invalid_status.x87.as_mut().unwrap().status_word |= 1;
        rejects.push(invalid_status);
        let mut invalid_tag = base.clone();
        invalid_tag
            .x87
            .as_mut()
            .unwrap()
            .set_tag(1, crate::state::X87Tag::Empty);
        rejects.push(invalid_tag);
        let mut noncanonical = base.clone();
        noncanonical.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x4000_0000_0000_0000, 0x3fff, false);
        rejects.push(noncanonical);
        let mut special = base.clone();
        special.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false);
        rejects.push(special);
        let mut one_third = base.clone();
        one_third.x87.as_mut().unwrap().registers[0] =
            binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        one_third.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        rejects.push(one_third);

        // Exact finite quotients outside the canonical Normal result range.
        for (lhs, rhs) in [
            (
                binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ), // 2^16383 / 0.5
            (
                binary80_normal(0x8000_0000_0000_0000, 0x0001, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ), // 2^-16382 / 2
        ] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[0] = lhs;
            state.x87.as_mut().unwrap().registers[1] = rhs;
            rejects.push(state);
        }
        for mut state in rejects {
            let before = state.clone();
            let mut memory = Memory::at(&opcode, state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }
        let mut absent = LongModeState::default();
        absent.rip = base.rip;
        let before = absent.clone();
        let mut memory = Memory::at(&opcode, absent.rip);
        assert_eq!(
            step(&mut absent, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(absent, before);

        for (fetch_fault, stop_rip, budget) in [
            (true, None, 1),
            (false, Some(base.rip), 1),
            (false, None, 0),
        ] {
            let mut state = base.clone();
            let before = state.clone();
            let mut memory = Memory::at(if fetch_fault { &[0xd8][..] } else { &opcode }, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(state, before);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(
                result.exit,
                if fetch_fault {
                    RunExit::MemoryError(MemoryError::Unmapped {
                        address: base.rip + 1,
                        access: AccessKind::Fetch,
                    })
                } else if stop_rip.is_some() {
                    RunExit::StopAddress
                } else {
                    RunExit::InstructionBudgetExhausted
                }
            );
        }
    }

    #[test]
    fn fmulp_sti_st0_multiplies_then_pops_for_every_top_and_index() {
        let one_and_half = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
        let two = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let two_and_quarter = binary80_normal(0x9000_0000_0000_0000, 0x4000, false);

        for top in 0u8..8 {
            for index in 0u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7b00;
                state.gpr = std::array::from_fn(|n| 0x3141_5926_5358_9793 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0xa0 + physical * 7 + byte) as u8)
                });
                x87.registers[old_top] = one_and_half;
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[physical_destination] = two;
                    x87.set_tag(physical_destination as u8, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xde, 0xc8 | index], state.rip);

                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_destination] =
                    if index == 0 { two_and_quarter } else { three };
                expected_x87.set_tag(top, X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected_x87.status_word = (expected_x87.status_word & !((7 << 11) | (1 << 9)))
                    | (u16::from(new_top) << 11);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
                assert_eq!(state.x87.as_ref().unwrap().top(), new_top);
                assert_eq!(
                    state.x87.as_ref().unwrap().tag(top),
                    Some(X87Tag::Empty),
                    "old TOP tag is empty for TOP={top}, i={index}"
                );
            }
        }

        for (top_sign, index_sign) in [(false, false), (false, true), (true, false), (true, true)] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 1 << 9;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, top_sign);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, index_sign);
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xde, 0xc9]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(
                state.x87.as_ref().unwrap().registers[1],
                binary80_normal(0xc000_0000_0000_0000, 0x3ffe, top_sign ^ index_sign)
            );
            assert_eq!(state.x87.as_ref().unwrap().tag(0), Some(X87Tag::Empty));
            assert_eq!(state.x87.as_ref().unwrap().top(), 1);
        }

        let mut full_width = LongModeState::default();
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfff0;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        x87.registers[1] = binary80_normal(u64::MAX, 0x3fff, false);
        full_width.x87 = Some(x87);
        let mut memory = Memory::new(&[0xde, 0xc9]);
        assert_eq!(
            step(&mut full_width, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            full_width.x87.as_ref().unwrap().registers[1],
            binary80_normal(u64::MAX, 0x4000, false)
        );
        assert_eq!(full_width.x87.as_ref().unwrap().tag(0), Some(X87Tag::Empty));
    }

    #[test]
    fn fmulp_sti_st0_rejects_bad_inputs_and_execution_boundaries_atomically() {
        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            x87
        };
        let mut inexact = make_pair();
        inexact.registers[0] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        inexact.registers[1] = inexact.registers[0];
        let mut overflow = make_pair();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let mut subnormal = make_pair();
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false);
        let mut rejected = vec![inexact, overflow, subnormal];
        for control_word in [0x027f, 0x037e, 0x077f] {
            let mut bad = make_pair();
            bad.control_word = control_word;
            rejected.push(bad);
        }
        for status_word in [1, 0x80, 0x8000] {
            let mut bad = make_pair();
            bad.status_word = status_word;
            rejected.push(bad);
        }
        for physical in [0, 1] {
            for tag in [X87Tag::Empty, X87Tag::Special] {
                let mut bad = make_pair();
                bad.set_tag(physical, tag);
                rejected.push(bad);
            }
            for raw in [
                binary80_normal(0, 0, false),
                binary80_normal(1, 0, false),
                binary80_normal(0x8000_0000_0000_0000, 0, false),
                binary80_normal(0, 1, false),
                binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
                binary80_normal(0, 0x7fff, false),
                binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
                binary80_normal(1, 0x7fff, false),
                binary80_normal(0x4000_0000_0000_0000, 1, false),
            ] {
                let mut bad = make_pair();
                bad.registers[usize::from(physical)] = raw;
                rejected.push(bad);
            }
        }
        for x87 in rejected {
            let mut state = LongModeState::default();
            state.rip = 0x7c00;
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xc9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }

        let mut i0_inexact = LongModeState::default();
        i0_inexact.rip = 0x7c20;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfffc;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        x87.set_tag(0, X87Tag::Valid);
        i0_inexact.x87 = Some(x87);
        let before_i0 = i0_inexact.clone();
        let mut i0_memory = Memory::at(&[0xde, 0xc8], i0_inexact.rip);
        assert_eq!(
            step(&mut i0_inexact, &mut i0_memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(i0_inexact, before_i0);

        let mut i0_overflow = LongModeState::default();
        i0_overflow.rip = 0x7c30;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 0;
        x87.tag_word = 0xfffc;
        x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false);
        x87.set_tag(0, X87Tag::Valid);
        i0_overflow.x87 = Some(x87);
        let before_i0 = i0_overflow.clone();
        let mut i0_memory = Memory::at(&[0xde, 0xc8], i0_overflow.rip);
        assert_eq!(
            step(&mut i0_overflow, &mut i0_memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(i0_overflow, before_i0);

        let mut missing = LongModeState::default();
        missing.rip = 0x7c50;
        let before_missing = missing.clone();
        let mut missing_memory = Memory::at(&[0xde, 0xc9], missing.rip);
        assert_eq!(
            step(&mut missing, &mut missing_memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(missing, before_missing);

        let mut state = LongModeState::default();
        state.rip = 0x7d00;
        state.x87 = Some(make_pair());
        let before = state.clone();
        let mut truncated = Memory::at(&[0xde], state.rip);
        assert!(matches!(
            step(&mut state, &mut truncated, AccessContext::default()),
            Err(MemoryError::Unmapped { .. })
        ));
        assert_eq!(state, before);

        for (stop, budget, expected) in [
            (Some(0x7d00), 1, RunExit::StopAddress),
            (None, 0, RunExit::InstructionBudgetExhausted),
        ] {
            let mut state = before.clone();
            let mut memory = Memory::at(&[0xde, 0xc9], state.rip);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, expected);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
            assert!(memory.accesses.is_empty());
        }

        let mut fetch_fault = before.clone();
        let mut memory = Memory::at(&[0xde, 0xc9], fetch_fault.rip);
        memory.fetch_error = Some((
            fetch_fault.rip + 1,
            MemoryError::Architectural(ArchitecturalFault {
                vector: 14,
                error_code: Some(4),
                linear_address: Some(fetch_fault.rip + 1),
            }),
        ));
        assert_eq!(
            step(&mut fetch_fault, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Fault(ArchitecturalFault {
                vector: 14,
                error_code: Some(4),
                linear_address: Some(fetch_fault.rip + 1),
            }))
        );
        assert_eq!(fetch_fault, before);
    }

    #[test]
    fn fmul_sti_st0_rejects_inexact_inputs_and_execution_boundaries_atomically() {
        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
            x87
        };
        let mut inexact = make_pair();
        inexact.registers[0] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        inexact.registers[1] = inexact.registers[0];
        let mut overflow = make_pair();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        let mut subnormal = make_pair();
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false);
        let mut rejected = vec![inexact, overflow, subnormal];
        for control_word in [0x027f, 0x037e, 0x077f] {
            let mut bad = make_pair();
            bad.control_word = control_word;
            rejected.push(bad);
        }
        for status_word in [1, 0x80, 0x8000] {
            let mut bad = make_pair();
            bad.status_word = status_word;
            rejected.push(bad);
        }
        for physical in [0, 1] {
            for tag in [X87Tag::Empty, X87Tag::Special] {
                let mut bad = make_pair();
                bad.set_tag(physical, tag);
                rejected.push(bad);
            }
            for raw in [
                binary80_normal(0, 0, false),
                binary80_normal(1, 0, false),
                binary80_normal(0x8000_0000_0000_0000, 0, false),
                binary80_normal(0, 1, false),
                binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
                binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
                binary80_normal(0, 0x7fff, false),
                binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
                binary80_normal(1, 0x7fff, false),
                binary80_normal(0x4000_0000_0000_0000, 1, false),
            ] {
                let mut bad = make_pair();
                bad.registers[usize::from(physical)] = raw;
                rejected.push(bad);
            }
        }
        for x87 in rejected {
            let mut state = LongModeState::default();
            state.rip = 0x7900;
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xc9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xdc })
            );
            assert_eq!(state, before);
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x7950;
        let before_missing = missing.clone();
        let mut missing_memory = Memory::at(&[0xdc, 0xc9], missing.rip);
        assert_eq!(
            step(&mut missing, &mut missing_memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xdc })
        );
        assert_eq!(missing, before_missing);

        let mut state = LongModeState::default();
        state.rip = 0x7a00;
        state.x87 = Some(make_pair());
        for (stop, budget, expected) in [
            (Some(0x7a00), 1, RunExit::StopAddress),
            (None, 0, RunExit::InstructionBudgetExhausted),
        ] {
            let before = state.clone();
            let mut memory = Memory::at(&[0xdc, 0xc9], state.rip);
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(result.exit, expected);
            assert_eq!(result.instructions_executed, 0);
            assert_eq!(state, before);
        }
        let before = state.clone();
        let mut truncated = Memory::at(&[0xdc], state.rip);
        assert!(matches!(
            step(&mut state, &mut truncated, AccessContext::default()),
            Err(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            })
        ));
        assert_eq!(state, before);
        let mut fault_memory = Memory::at(&[0xdc, 0xc9], state.rip);
        fault_memory.fetch_error = Some((
            state.rip + 1,
            MemoryError::Unmapped {
                address: state.rip + 1,
                access: AccessKind::Fetch,
            },
        ));
        assert!(matches!(
            step(&mut state, &mut fault_memory, AccessContext::default()),
            Err(MemoryError::Unmapped {
                access: AccessKind::Fetch,
                ..
            })
        ));
        assert_eq!(state, before);

        let mut requested_stop = state.clone();
        let before_stop = requested_stop.clone();
        let mut stop_memory = Memory::at(&[0xdc, 0xc9], requested_stop.rip);
        let result = run(
            &mut requested_stop,
            &mut stop_memory,
            RunConfig {
                instruction_budget: 1,
                stop_rip: None,
                context: AccessContext::default(),
            },
            || true,
        );
        assert_eq!(result.exit, RunExit::StopRequested);
        assert_eq!(result.instructions_executed, 0);
        assert_eq!(requested_stop, before_stop);
    }

    #[test]
    fn fmul_st0_sti_rejects_inexact_range_and_bad_state_atomically() {
        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = 0xfff0;
            x87.registers[0] = binary80_normal(0xc000_0000_0000_0000, 0x3fff, false);
            x87.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false);
            x87
        };
        let mut rejected = Vec::new();
        for dirty_cw in [0x027f, 0x037e, 0x077f] {
            let mut bad = make_pair();
            bad.control_word = dirty_cw;
            rejected.push(bad);
        }
        for dirty_sw in [1, 0x80, 0x8000] {
            let mut bad = make_pair();
            bad.status_word = dirty_sw;
            rejected.push(bad);
        }
        for physical in [0, 1] {
            for tag in [X87Tag::Empty, X87Tag::Special] {
                let mut bad = make_pair();
                bad.set_tag(physical, tag);
                rejected.push(bad);
            }
        }
        for invalid_raw in [
            binary80_normal(0, 0, false),
            binary80_normal(1, 0, false),
            binary80_normal(0x8000_0000_0000_0000, 0, false),
            binary80_normal(0, 1, false),
            binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            binary80_normal(0x8000_0000_0000_0001, 0x7fff, false),
            binary80_normal(0, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 0x7fff, false),
            binary80_normal(1, 0x7fff, false),
            binary80_normal(0x4000_0000_0000_0000, 1, false),
        ] {
            for slot in [0, 1] {
                let mut bad = make_pair();
                bad.registers[slot] = invalid_raw;
                rejected.push(bad);
            }
        }
        let mut inexact = make_pair();
        inexact.registers[0] = binary80_normal(0x8000_0000_0000_0001, 0x3fff, false);
        inexact.registers[1] = inexact.registers[0];
        rejected.push(inexact);
        let mut overflow = make_pair();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejected.push(overflow);
        let mut subnormal = make_pair();
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false);
        rejected.push(subnormal);

        for x87 in rejected {
            let mut state = LongModeState::default();
            state.rip = 0x7500;
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            state.x87 = Some(x87);
            let before = state.clone();
            let mut memory = Memory::at(&[0xd8, 0xc9], 0x7500);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xd8 })
            );
            assert_eq!(state, before);
        }
        let mut missing = LongModeState::default();
        missing.rip = 0x7600;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xd8, 0xc8], 0x7600);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xd8 })
        );
        assert_eq!(missing, before);

        let mut initial = LongModeState::default();
        initial.rip = 0x7700;
        initial.x87 = Some(make_pair());
        for (bytes, stop, budget, request_stop, injected_fetch_fault) in [
            (&[0xd8][..], None, 1, false, false),
            (&[0xd8, 0xc9][..], Some(0x7700), 1, false, false),
            (&[0xd8, 0xc9][..], None, 0, false, false),
            (&[0xd8, 0xc9][..], None, 1, true, false),
            (&[0xd8, 0xc9][..], None, 1, false, true),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, 0x7700);
            if injected_fetch_fault {
                memory.fetch_error = Some((
                    0x7701,
                    MemoryError::Unmapped {
                        address: 0x7701,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            if injected_fetch_fault {
                assert!(matches!(result.exit, RunExit::MemoryError(_)));
            }
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fdivp_sti_st0_commits_ordered_quotient_then_pops_atomically() {
        let fifteen = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        let five = binary80_normal(0xa000_0000_0000_0000, 0x4001, false);
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let old_top = usize::from(top);
                let physical_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x7e80;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|physical| {
                    std::array::from_fn(|byte| (0x70 + physical * 7 + byte) as u8)
                });
                if index == 0 {
                    x87.registers[old_top] =
                        binary80_normal(0xd123_4567_89ab_cdef, 0x4000, top & 1 != 0);
                } else {
                    // ST(i)=15 and ST(0)=3 yields 5. Reversing operands is inexact.
                    x87.registers[physical_destination] = fifteen;
                    x87.set_tag((top + index) & 7, crate::state::X87Tag::Valid);
                    x87.registers[old_top] = three;
                }
                x87.set_tag(top, crate::state::X87Tag::Valid);
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xde, 0xf8 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[physical_destination] = if index == 0 { one } else { five };
                expected_x87.set_tag(top, crate::state::X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected_x87.status_word = (expected_x87.status_word & !((7 << 11) | (1 << 9)))
                    | (u16::from(new_top) << 11);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "complete state TOP={top}, i={index}");
            }
        }

        for (numerator, denominator, expected) in [
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, false),
                three,
                five,
            ),
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, true),
                three,
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                fifteen,
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                binary80_normal(0xa000_0000_0000_0000, 0x4001, true),
            ),
            (
                binary80_normal(0xf000_0000_0000_0000, 0x4002, true),
                binary80_normal(0xc000_0000_0000_0000, 0x4000, true),
                five,
            ),
            (
                binary80_normal(u64::MAX, 0x3fff, false),
                one,
                binary80_normal(u64::MAX, 0x3fff, false),
            ),
        ] {
            let mut state = LongModeState::default();
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = u16::MAX;
            x87.registers[0] = denominator;
            x87.registers[1] = numerator;
            x87.set_tag(0, crate::state::X87Tag::Valid);
            x87.set_tag(1, crate::state::X87Tag::Valid);
            state.x87 = Some(x87);
            let mut memory = Memory::new(&[0xde, 0xf9]);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            assert_eq!(state.x87.as_ref().unwrap().registers[1], expected);
        }

        let mut base = LongModeState::default();
        base.rip = 0x7f00;
        let mut valid = crate::state::X87State::fixture_seed();
        valid.control_word = 0x037f;
        valid.status_word = 0;
        valid.tag_word = u16::MAX;
        valid.registers[0] = three;
        valid.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        valid.set_tag(0, crate::state::X87Tag::Valid);
        valid.set_tag(1, crate::state::X87Tag::Valid);
        base.x87 = Some(valid);
        let mut rejects = Vec::new();
        let mut inexact_one_third = base.clone();
        inexact_one_third.x87.as_mut().unwrap().registers[1] = one;
        rejects.push(inexact_one_third);
        let mut inexact_two_thirds = base.clone();
        inexact_two_thirds.x87.as_mut().unwrap().registers[1] =
            binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejects.push(inexact_two_thirds);
        for (numerator, denominator) in [
            (
                binary80_normal(0x8000_0000_0000_0000, 0x7ffe, false),
                binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false),
            ),
            (
                binary80_normal(0x8000_0000_0000_0000, 1, false),
                binary80_normal(0x8000_0000_0000_0000, 0x4000, false),
            ),
        ] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[1] = numerator;
            state.x87.as_mut().unwrap().registers[0] = denominator;
            rejects.push(state);
        }
        for bad_tag_index in [0, 1] {
            let mut state = base.clone();
            state
                .x87
                .as_mut()
                .unwrap()
                .set_tag(bad_tag_index, crate::state::X87Tag::Empty);
            rejects.push(state);
        }
        let mut bad_control = base.clone();
        bad_control.x87.as_mut().unwrap().control_word ^= 1;
        rejects.push(bad_control);
        let mut bad_status = base.clone();
        bad_status.x87.as_mut().unwrap().status_word |= 1;
        rejects.push(bad_status);
        for bad_index in [0, 1] {
            let mut state = base.clone();
            state.x87.as_mut().unwrap().registers[bad_index] = [0; 10];
            rejects.push(state);
        }
        for mut state in rejects {
            state.rip = 0x7f00;
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xf9], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }
        let mut missing = LongModeState::default();
        missing.rip = 0x7f00;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xde, 0xf9], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(missing, before);

        // The alias writes +1 before popping its only physical register.
        for (top, negative) in [(0, false), (7, true)] {
            let mut state = LongModeState::default();
            state.rip = 0x7f80;
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = (u16::from(top) << 11) | (1 << 9);
            x87.tag_word = u16::MAX;
            x87.registers[usize::from(top)] =
                binary80_normal(0xc123_4567_89ab_cdef, 0x4000, negative);
            x87.set_tag(top, crate::state::X87Tag::Valid);
            state.x87 = Some(x87);
            let mut memory = Memory::at(&[0xde, 0xf8], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Advanced)
            );
            let x87 = state.x87.as_ref().unwrap();
            assert_eq!(x87.registers[usize::from(top)], one);
            assert_eq!(x87.tag(top), Some(crate::state::X87Tag::Empty));
            assert_eq!(x87.top(), top.wrapping_add(1) & 7);
        }

        let mut initial = base.clone();
        initial.rip = 0x7f80;
        for (bytes, stop, budget, request_stop, fetch_fault) in [
            (&[0xde][..], None, 1, false, false),
            (&[0xde, 0xf9][..], Some(0x7f80), 1, false, false),
            (&[0xde, 0xf9][..], None, 0, false, false),
            (&[0xde, 0xf9][..], None, 1, true, false),
            (&[0xde, 0xf9][..], None, 1, false, true),
        ] {
            let mut state = initial.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, 0x7f80);
            if fetch_fault {
                memory.fetch_error = Some((
                    0x7f81,
                    MemoryError::Unmapped {
                        address: 0x7f81,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let result = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip: stop,
                    context: AccessContext::default(),
                },
                || request_stop,
            );
            assert_eq!(result.instructions_executed, 0);
            if fetch_fault {
                assert!(matches!(result.exit, RunExit::MemoryError(_)));
            }
            assert_eq!(state, before);
        }
    }

    #[test]
    fn fdivrp_sti_st0_writes_exact_st0_over_sti_then_pops() {
        let one = binary80_normal(0x8000_0000_0000_0000, 0x3fff, false);
        let fifteen = binary80_normal(0xf000_0000_0000_0000, 0x4002, false);
        let three = binary80_normal(0xc000_0000_0000_0000, 0x4000, false);
        for top in 0u8..8 {
            for index in 0u8..8 {
                let old_top = usize::from(top);
                let old_destination = usize::from((top + index) & 7);
                let mut state = LongModeState::default();
                state.rip = 0x8100;
                state.gpr = std::array::from_fn(|n| 0x1122_3344_5566_7788 ^ n as u64);
                state.rflags = 0xa55a_1234_5678_9abc;
                let mut x87 = crate::state::X87State::fixture_seed();
                x87.control_word = 0x037f;
                x87.status_word = (u16::from(top) << 11) | 0x4700 | (1 << 9);
                x87.tag_word = u16::MAX;
                x87.registers = std::array::from_fn(|slot| {
                    std::array::from_fn(|byte| (0x30 + slot * 7 + byte) as u8)
                });
                let st0_negative = top & 1 != 0;
                let sti_negative = index & 2 != 0;
                x87.registers[old_top] = if index == 0 {
                    binary80_normal(0x8000_0000_0000_0000, 0x3fff, st0_negative)
                } else {
                    binary80_normal(0xf000_0000_0000_0000, 0x4002, st0_negative)
                };
                x87.set_tag(top, X87Tag::Valid);
                if index != 0 {
                    x87.registers[old_destination] =
                        binary80_normal(0xc000_0000_0000_0000, 0x4000, sti_negative);
                    x87.set_tag(old_destination as u8, X87Tag::Valid);
                }
                x87.instruction_pointer = 0x1234;
                x87.data_pointer = 0x5678;
                x87.last_opcode = 0x05aa;
                state.x87 = Some(x87);
                let before = state.clone();
                let mut memory = Memory::at(&[0xde, 0xf0 | index], state.rip);
                assert_eq!(
                    step(&mut state, &mut memory, AccessContext::default()),
                    Ok(StepOutcome::Advanced),
                    "TOP={top}, i={index}"
                );
                let mut expected = before.clone();
                let expected_x87 = expected.x87.as_mut().unwrap();
                expected_x87.registers[old_destination] = if index == 0 {
                    one
                } else {
                    binary80_normal(0xa000_0000_0000_0000, 0x4001, st0_negative ^ sti_negative)
                };
                expected_x87.set_tag(top, X87Tag::Empty);
                let new_top = top.wrapping_add(1) & 7;
                expected_x87.status_word = (expected_x87.status_word & !((7 << 11) | (1 << 9)))
                    | (u16::from(new_top) << 11);
                expected_x87.instruction_pointer = before.rip;
                expected.rip += 2;
                assert_eq!(state, expected, "full state TOP={top}, i={index}");
            }
        }

        let mut full_width = LongModeState::default();
        full_width.rip = 0x8180;
        let mut x87 = crate::state::X87State::fixture_seed();
        x87.control_word = 0x037f;
        x87.status_word = 7 << 11;
        x87.tag_word = u16::MAX;
        x87.registers[7] = binary80_normal(u64::MAX, 0x3fff, true);
        x87.registers[0] = one;
        x87.set_tag(7, X87Tag::Valid);
        x87.set_tag(0, X87Tag::Valid);
        full_width.x87 = Some(x87);
        let mut memory = Memory::at(&[0xde, 0xf1], full_width.rip);
        assert_eq!(
            step(&mut full_width, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Advanced)
        );
        assert_eq!(
            full_width.x87.as_ref().unwrap().registers[0],
            binary80_normal(u64::MAX, 0x3fff, true)
        );
        assert_eq!(full_width.x87.as_ref().unwrap().top(), 0);
        assert_eq!(full_width.x87.as_ref().unwrap().tag(7), Some(X87Tag::Empty));

        let make_pair = || {
            let mut x87 = crate::state::X87State::fixture_seed();
            x87.control_word = 0x037f;
            x87.status_word = 0;
            x87.tag_word = u16::MAX;
            x87.registers[0] = fifteen;
            x87.registers[1] = three;
            x87.set_tag(0, X87Tag::Valid);
            x87.set_tag(1, X87Tag::Valid);
            x87
        };
        let mut rejects = Vec::new();
        let mut inexact_one_third = make_pair();
        inexact_one_third.registers[0] = one;
        rejects.push(inexact_one_third);
        let mut two_thirds = make_pair();
        two_thirds.registers[0] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejects.push(two_thirds);
        let mut overflow = make_pair();
        overflow.registers[0] = binary80_normal(u64::MAX, 0x7ffe, false);
        overflow.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x3ffe, false);
        rejects.push(overflow);
        let mut subnormal = make_pair();
        subnormal.registers[0] = binary80_normal(0x8000_0000_0000_0000, 1, false);
        subnormal.registers[1] = binary80_normal(0x8000_0000_0000_0000, 0x4000, false);
        rejects.push(subnormal);
        for dirty_control in [0x027f, 0x037e, 0x077f] {
            let mut bad = make_pair();
            bad.control_word = dirty_control;
            rejects.push(bad);
        }
        for dirty_status in [1, 0x80, 0x8000] {
            let mut bad = make_pair();
            bad.status_word = dirty_status;
            rejects.push(bad);
        }
        for physical in [0, 1] {
            let mut bad = make_pair();
            bad.set_tag(physical, X87Tag::Empty);
            rejects.push(bad);
            let mut bad = make_pair();
            bad.set_tag(physical, X87Tag::Special);
            rejects.push(bad);
            for invalid_raw in [
                [0; 10],
                binary80_normal(0x4000_0000_0000_0000, 0x3fff, false),
                binary80_normal(0x8000_0000_0000_0000, 0, false),
                binary80_normal(0x8000_0000_0000_0000, 0x7fff, false),
                binary80_normal(0xc000_0000_0000_0000, 0x7fff, false),
            ] {
                let mut bad = make_pair();
                bad.registers[physical as usize] = invalid_raw;
                rejects.push(bad);
            }
        }
        for x87 in rejects {
            let mut state = LongModeState::default();
            state.rip = 0x8200;
            state.x87 = Some(x87);
            state.gpr = [0x1234_5678_9abc_def0; 16];
            state.rflags = 0xa55a;
            let before = state.clone();
            let mut memory = Memory::at(&[0xde, 0xf1], state.rip);
            assert_eq!(
                step(&mut state, &mut memory, AccessContext::default()),
                Ok(StepOutcome::Unsupported { opcode: 0xde })
            );
            assert_eq!(state, before);
        }

        let mut missing = LongModeState::default();
        missing.rip = 0x8300;
        let before = missing.clone();
        let mut memory = Memory::at(&[0xde, 0xf0], missing.rip);
        assert_eq!(
            step(&mut missing, &mut memory, AccessContext::default()),
            Ok(StepOutcome::Unsupported { opcode: 0xde })
        );
        assert_eq!(missing, before);

        let mut valid = LongModeState::default();
        valid.rip = 0x8400;
        valid.x87 = Some(make_pair());
        for (bytes, stop_rip, budget, fetch_fault) in [
            (&[0xde][..], None, 1, false),
            (&[0xde, 0xf1][..], Some(0x8400), 1, false),
            (&[0xde, 0xf1][..], None, 0, false),
            (&[0xde, 0xf1][..], None, 1, true),
        ] {
            let mut state = valid.clone();
            let before = state.clone();
            let mut memory = Memory::at(bytes, state.rip);
            if fetch_fault {
                memory.fetch_error = Some((
                    state.rip + 1,
                    MemoryError::Unmapped {
                        address: state.rip + 1,
                        access: AccessKind::Fetch,
                    },
                ));
            }
            let outcome = run(
                &mut state,
                &mut memory,
                RunConfig {
                    instruction_budget: budget,
                    stop_rip,
                    context: AccessContext::default(),
                },
                || false,
            );
            assert_eq!(outcome.instructions_executed, 0);
            assert_eq!(state, before);
        }
    }
}
