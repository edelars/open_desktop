//! Small, linear, typed IR for the first register-only block compiler.
//!
//! This module records effects supplied by a future decoder adapter. It does
//! not implement x86 instruction semantics or lower operations to host code.

use std::collections::HashMap;

use crate::block::AdmittedInstructionBoundaries;
use od_isa::long_decode::{AddressBase, MemoryOperand};

/// Identity of a temporary value. Temporaries are separate from guest GPRs.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TempId(pub u32);

/// Integer width carried by every value-producing or value-consuming effect.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntWidth {
    W8,
    W16,
    W32,
    W64,
}

/// Register number from the guest encoding. General operations accept the
/// initial four-GPR profile (RAX..RBX); checked memory has an exact scoped
/// exception for RSP as the MOV data register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestGpr(pub u8);

/// Arithmetic operation kind. The IR records decoded effects; it does not
/// define the arithmetic or flag formulas.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerKind {
    Add,
    Subtract,
    Compare,
}

/// Which architectural flag effects an integer operation has.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlagEffect {
    Preserve,
    Arithmetic,
    /// Raw flag-bit mask, validated against the six defined S0-09 flags.
    WriteMask(u64),
}

/// A guest instruction boundary supplied by the block builder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestInstruction {
    pub guest_rip: u64,
    pub next_rip: u64,
}

/// A linear list of effects enclosed by guest instruction begin/commit
/// markers, followed by one explicit control-flow terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Program {
    pub guest_instructions: Vec<GuestInstruction>,
    pub operations: Vec<Operation>,
    /// Exclusive end copied from the validated source `CodeView`. This is the
    /// sole additional branch boundary admitted beyond IR record boundaries.
    pub code_end: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchCondition {
    Zero,
    NotZero,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitKind {
    BlockComplete,
    NeedInterpreter,
    Unsupported,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Operation {
    BeginInstruction {
        index: usize,
    },
    Constant {
        dst: TempId,
        width: IntWidth,
        value: u64,
    },
    ReadGpr {
        dst: TempId,
        register: GuestGpr,
        width: IntWidth,
    },
    /// A checked qword read. `dst` is defined only when the invocation-local
    /// checked helper succeeds; the generated entry stages it until commit.
    CheckedLoad {
        dst: TempId,
        operand: MemoryOperand,
    },
    /// One checked qword write from the canonical guest GPR. The store has
    /// no temporary result and must be followed immediately by its commit.
    CheckedStore {
        src: GuestGpr,
        operand: MemoryOperand,
    },
    Integer {
        kind: IntegerKind,
        lhs: TempId,
        rhs: TempId,
        result: Option<TempId>,
        width: IntWidth,
        flags: FlagEffect,
    },
    WriteGpr {
        register: GuestGpr,
        src: TempId,
        width: IntWidth,
    },
    CommitInstruction {
        index: usize,
    },
    Branch {
        condition: BranchCondition,
        target_rip: u64,
        fallthrough_rip: u64,
    },
    /// Commit a conditional guest branch and return its selected RIP as a
    /// dispatcher candidate. Both edges carry a full fixture-decode witness.
    DispatchBranch {
        condition: BranchCondition,
        target_rip: u64,
        fallthrough_rip: u64,
        admission: AdmittedInstructionBoundaries,
    },
    Exit {
        kind: ExitKind,
        target_rip: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum VerifyError {
    EmptyInstructionList,
    InvalidInstructionRange {
        index: usize,
    },
    DiscontinuousInstructionRange {
        index: usize,
    },
    InstructionOutsideCodeEnd {
        index: usize,
        code_end: u64,
    },
    MissingTerminal,
    MultipleOrNonFinalTerminal {
        operation: usize,
    },
    InvalidInstructionMarker {
        operation: usize,
        index: usize,
    },
    NestedInstruction {
        operation: usize,
    },
    EffectOutsideInstruction {
        operation: usize,
    },
    CommitWithoutInstruction {
        operation: usize,
    },
    CommitIndexMismatch {
        operation: usize,
        expected: usize,
        actual: usize,
    },
    MissingCommit {
        index: usize,
    },
    InvalidGpr {
        operation: usize,
        register: u8,
    },
    InvalidFlagMask {
        operation: usize,
        mask: u64,
    },
    UndefinedTemporary {
        operation: usize,
        temp: TempId,
    },
    DuplicateTemporary {
        operation: usize,
        temp: TempId,
    },
    WidthMismatch {
        operation: usize,
        temp: TempId,
        expected: IntWidth,
        actual: IntWidth,
    },
    InvalidCheckedLoadDescriptor {
        operation: usize,
    },
    CheckedLoadInstructionMismatch {
        operation: usize,
    },
    CheckedLoadWithoutWrite {
        operation: usize,
    },
    InvalidCheckedStoreDescriptor {
        operation: usize,
    },
    CheckedStoreInstructionMismatch {
        operation: usize,
    },
    CheckedStoreCommitOrder {
        operation: usize,
    },
    InvalidIntegerResult {
        operation: usize,
        kind: IntegerKind,
    },
    InvalidTerminalRip {
        operation: usize,
        rip: u64,
    },
    InvalidBranchTarget {
        operation: usize,
        rip: u64,
    },
    InvalidFallthrough {
        operation: usize,
        rip: u64,
    },
    DispatchAdmissionMismatch {
        operation: usize,
    },
    InvalidDispatchBoundary {
        operation: usize,
        rip: u64,
    },
}

/// Verify a complete linear IR program before any executable code can be
/// published. Branch targets are checked against the full guest-record list,
/// not against caller-provided scalar ranges. The later block builder must
/// establish that those records came from successful decoding.
pub fn verify(program: &Program) -> Result<(), VerifyError> {
    if program.guest_instructions.is_empty() {
        return Err(VerifyError::EmptyInstructionList);
    }

    for (index, instruction) in program.guest_instructions.iter().enumerate() {
        if instruction.guest_rip >= instruction.next_rip {
            return Err(VerifyError::InvalidInstructionRange { index });
        }
        if index > 0 && program.guest_instructions[index - 1].next_rip != instruction.guest_rip {
            return Err(VerifyError::DiscontinuousInstructionRange { index });
        }
        if instruction.next_rip > program.code_end {
            return Err(VerifyError::InstructionOutsideCodeEnd {
                index,
                code_end: program.code_end,
            });
        }
    }

    let boundaries: Vec<u64> = program
        .guest_instructions
        .iter()
        .map(|instruction| instruction.guest_rip)
        .chain(
            program
                .guest_instructions
                .last()
                .map(|instruction| instruction.next_rip),
        )
        .chain(std::iter::once(program.code_end))
        .collect();

    let mut definitions = HashMap::<TempId, IntWidth>::new();
    let mut active_instruction = None;
    let mut next_commit = 0usize;
    let mut terminal_seen = false;

    for (operation_index, operation) in program.operations.iter().enumerate() {
        if terminal_seen {
            return Err(VerifyError::MultipleOrNonFinalTerminal {
                operation: operation_index,
            });
        }
        match operation {
            Operation::BeginInstruction { index } => {
                if let Some(active) = active_instruction {
                    if *index > active {
                        return Err(VerifyError::MissingCommit { index: active });
                    }
                    return Err(VerifyError::NestedInstruction {
                        operation: operation_index,
                    });
                }
                if *index > next_commit && *index < program.guest_instructions.len() {
                    return Err(VerifyError::MissingCommit { index: next_commit });
                }
                if *index != next_commit || *index >= program.guest_instructions.len() {
                    return Err(VerifyError::InvalidInstructionMarker {
                        operation: operation_index,
                        index: *index,
                    });
                }
                active_instruction = Some(*index);
            }
            Operation::Constant { dst, width, .. } => {
                require_active(active_instruction, operation_index)?;
                define(&mut definitions, *dst, *width, operation_index)?;
            }
            Operation::ReadGpr {
                dst,
                register,
                width,
            } => {
                require_active(active_instruction, operation_index)?;
                validate_gpr(*register, operation_index)?;
                define(&mut definitions, *dst, *width, operation_index)?;
            }
            Operation::CheckedLoad { dst, operand } => {
                require_active(active_instruction, operation_index)?;
                if operand.size != od_isa::OperandSize::Qword
                    || !is_checked_memory_address(
                        operand,
                        instruction_length(program, active_instruction),
                    )
                {
                    return Err(VerifyError::InvalidCheckedLoadDescriptor {
                        operation: operation_index,
                    });
                }
                define(&mut definitions, *dst, IntWidth::W64, operation_index)?;
            }
            Operation::CheckedStore { src, operand } => {
                require_active(active_instruction, operation_index)?;
                let active = active_instruction.expect("require_active checked above");
                if src.0 == 4 || src.0 == 5 {
                    if !is_scoped_memory_data_operand(
                        *operand,
                        instruction_length(program, active_instruction),
                        src.0,
                    ) {
                        return Err(VerifyError::InvalidGpr {
                            operation: operation_index,
                            register: src.0,
                        });
                    }
                } else if matches!(src.0, 6 | 7) {
                    let length = instruction_length(program, active_instruction);
                    let addr64_profile = matches!(length, 3 | 4 | 7)
                        && operand.address_size == od_isa::OperandSize::Qword
                        && match length {
                            3 => operand.displacement == 0,
                            4 => i8::try_from(operand.displacement).is_ok(),
                            7 => i32::try_from(operand.displacement).is_ok(),
                            _ => false,
                        };
                    let addr32_profile = matches!(length, 4 | 5 | 8)
                        && operand.address_size == od_isa::OperandSize::Dword
                        && match length {
                            4 => operand.displacement == 0,
                            5 => i8::try_from(operand.displacement).is_ok(),
                            8 => i32::try_from(operand.displacement).is_ok(),
                            _ => false,
                        };
                    if !(addr64_profile || addr32_profile)
                        || operand.size != od_isa::OperandSize::Qword
                        || operand.index.is_some()
                        || operand.scale != 1
                        || !matches!(
                            operand.base,
                            od_isa::long_decode::AddressBase::Register(register)
                                if (register as u8) < 4
                        )
                    {
                        return Err(VerifyError::InvalidGpr {
                            operation: operation_index,
                            register: src.0,
                        });
                    }
                } else {
                    validate_gpr(*src, operation_index)?;
                }
                if operand.size != od_isa::OperandSize::Qword
                    || !is_checked_memory_address(
                        operand,
                        instruction_length(program, active_instruction),
                    )
                {
                    return Err(VerifyError::InvalidCheckedStoreDescriptor {
                        operation: operation_index,
                    });
                }
                if !matches!(
                    program.operations.get(operation_index + 1),
                    Some(Operation::CommitInstruction { index }) if *index == active
                ) {
                    return Err(VerifyError::CheckedStoreCommitOrder {
                        operation: operation_index,
                    });
                }
            }
            Operation::Integer {
                kind,
                lhs,
                rhs,
                result,
                width,
                flags,
            } => {
                require_active(active_instruction, operation_index)?;
                require_temp(&definitions, *lhs, *width, operation_index)?;
                require_temp(&definitions, *rhs, *width, operation_index)?;
                validate_flags(*flags, operation_index)?;
                match (kind, result) {
                    (IntegerKind::Compare, None) | (_, Some(_)) => {}
                    _ => {
                        return Err(VerifyError::InvalidIntegerResult {
                            operation: operation_index,
                            kind: *kind,
                        });
                    }
                }
                if let Some(dst) = result {
                    define(&mut definitions, *dst, *width, operation_index)?;
                }
            }
            Operation::WriteGpr {
                register,
                src,
                width,
            } => {
                require_active(active_instruction, operation_index)?;
                if register.0 == 4 || register.0 == 5 {
                    let valid_scoped_data_load = matches!(
                        program.operations.get(operation_index.wrapping_sub(1)),
                        Some(Operation::CheckedLoad {
                            dst: loaded_value,
                            operand,
                        })
                            if *loaded_value == *src
                                && is_scoped_memory_data_operand(
                                *operand,
                                instruction_length(program, active_instruction),
                                register.0,
                            )
                    );
                    if !valid_scoped_data_load {
                        return Err(VerifyError::InvalidGpr {
                            operation: operation_index,
                            register: register.0,
                        });
                    }
                } else if register.0 == 6 || register.0 == 7 {
                    let valid_scoped_data_load = matches!(
                        program.operations.get(operation_index.wrapping_sub(1)),
                        Some(Operation::CheckedLoad {
                            dst: loaded_value,
                            operand,
                        }) if *loaded_value == *src && is_direct_low_base_rsi_rdi_load_operand(
                            *operand,
                            instruction_length(program, active_instruction),
                            register.0,
                        )
                    );
                    if !valid_scoped_data_load {
                        return Err(VerifyError::InvalidGpr {
                            operation: operation_index,
                            register: register.0,
                        });
                    }
                } else {
                    validate_gpr(*register, operation_index)?;
                }
                require_temp(&definitions, *src, *width, operation_index)?;
            }
            Operation::CommitInstruction { index } => {
                let Some(active) = active_instruction else {
                    return Err(VerifyError::CommitWithoutInstruction {
                        operation: operation_index,
                    });
                };
                if active != *index || active != next_commit {
                    return Err(VerifyError::CommitIndexMismatch {
                        operation: operation_index,
                        expected: active,
                        actual: *index,
                    });
                }
                let instruction_begin = program.operations[..operation_index]
                    .iter()
                    .rposition(|operation| matches!(operation, Operation::BeginInstruction { .. }))
                    .unwrap_or(0);
                let store_position = program.operations[instruction_begin..operation_index]
                    .iter()
                    .rposition(|operation| matches!(operation, Operation::CheckedStore { .. }))
                    .map(|position| instruction_begin + position);
                if store_position.is_some_and(|position| position + 1 != operation_index) {
                    return Err(VerifyError::CheckedStoreCommitOrder {
                        operation: store_position.unwrap_or(operation_index),
                    });
                }
                let has_checked_load = program.operations[..operation_index]
                    .iter()
                    .rposition(|operation| matches!(operation, Operation::BeginInstruction { .. }))
                    .is_some_and(|begin| {
                        program.operations[begin + 1..operation_index]
                            .iter()
                            .any(|operation| matches!(operation, Operation::CheckedLoad { .. }))
                    });
                if has_checked_load {
                    let Some(load_position) = program.operations[..operation_index]
                        .iter()
                        .rposition(|operation| matches!(operation, Operation::CheckedLoad { .. }))
                    else {
                        return Err(VerifyError::CheckedLoadWithoutWrite {
                            operation: operation_index,
                        });
                    };
                    let load_result = match &program.operations[load_position] {
                        Operation::CheckedLoad { dst, .. } => *dst,
                        _ => unreachable!(),
                    };
                    if !program.operations[load_position + 1..operation_index]
                        .iter()
                        .any(|operation| matches!(operation, Operation::WriteGpr { src, width: IntWidth::W64, .. } if *src == load_result))
                    {
                        return Err(VerifyError::CheckedLoadWithoutWrite {
                            operation: load_position,
                        });
                    }
                }
                active_instruction = None;
                next_commit += 1;
            }
            Operation::Branch {
                condition: _,
                target_rip,
                fallthrough_rip,
            } => {
                validate_terminal_position(
                    active_instruction,
                    next_commit,
                    program.guest_instructions.len(),
                    operation_index,
                )?;
                let final_rip = program.guest_instructions.last().unwrap().next_rip;
                if !boundaries.contains(target_rip) {
                    return Err(VerifyError::InvalidBranchTarget {
                        operation: operation_index,
                        rip: *target_rip,
                    });
                }
                if *fallthrough_rip != final_rip {
                    return Err(VerifyError::InvalidFallthrough {
                        operation: operation_index,
                        rip: *fallthrough_rip,
                    });
                }
                terminal_seen = true;
            }
            Operation::DispatchBranch {
                condition: _,
                target_rip,
                fallthrough_rip,
                admission,
            } => {
                validate_terminal_position(
                    active_instruction,
                    next_commit,
                    program.guest_instructions.len(),
                    operation_index,
                )?;
                if admission.code_end() != program.code_end
                    || !admission.covers_records(&program.guest_instructions)
                {
                    return Err(VerifyError::DispatchAdmissionMismatch {
                        operation: operation_index,
                    });
                }
                for rip in [target_rip, fallthrough_rip] {
                    if !admission.contains(*rip) {
                        return Err(VerifyError::InvalidDispatchBoundary {
                            operation: operation_index,
                            rip: *rip,
                        });
                    }
                }
                if *fallthrough_rip != program.guest_instructions.last().unwrap().next_rip {
                    return Err(VerifyError::InvalidFallthrough {
                        operation: operation_index,
                        rip: *fallthrough_rip,
                    });
                }
                terminal_seen = true;
            }
            Operation::Exit {
                kind: _,
                target_rip,
            } => {
                validate_terminal_position(
                    active_instruction,
                    next_commit,
                    program.guest_instructions.len(),
                    operation_index,
                )?;
                let final_rip = program.guest_instructions.last().unwrap().next_rip;
                if *target_rip != final_rip {
                    return Err(VerifyError::InvalidTerminalRip {
                        operation: operation_index,
                        rip: *target_rip,
                    });
                }
                terminal_seen = true;
            }
        }
    }

    if !terminal_seen {
        return Err(VerifyError::MissingTerminal);
    }
    if let Some(index) = active_instruction {
        return Err(VerifyError::MissingCommit { index });
    }
    if next_commit != program.guest_instructions.len() {
        return Err(VerifyError::MissingCommit { index: next_commit });
    }
    Ok(())
}

fn require_active(active: Option<usize>, operation: usize) -> Result<(), VerifyError> {
    if active.is_some() {
        Ok(())
    } else {
        Err(VerifyError::EffectOutsideInstruction { operation })
    }
}

fn validate_terminal_position(
    active: Option<usize>,
    committed: usize,
    instruction_count: usize,
    operation: usize,
) -> Result<(), VerifyError> {
    if active.is_some() {
        return Err(VerifyError::EffectOutsideInstruction { operation });
    }
    if committed != instruction_count {
        return Err(VerifyError::MissingCommit { index: committed });
    }
    Ok(())
}

fn instruction_length(program: &Program, active: Option<usize>) -> u64 {
    active
        .and_then(|index| program.guest_instructions.get(index))
        .and_then(|instruction| instruction.next_rip.checked_sub(instruction.guest_rip))
        .unwrap_or(0)
}

fn is_checked_memory_address(
    operand: &od_isa::long_decode::MemoryOperand,
    instruction_length: u64,
) -> bool {
    if operand.address_size == od_isa::OperandSize::Dword {
        let base_supported =
            matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4);
        let rsp_no_index_sib = matches!(instruction_length, 5 | 6 | 9)
            && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
            && operand.index.is_none()
            && operand.scale == 1
            && match instruction_length {
                5 => operand.displacement == 0,
                6 => i8::try_from(operand.displacement).is_ok(),
                9 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let rbp_direct = matches!(instruction_length, 5 | 8)
            && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
            && operand.index.is_none()
            && operand.scale == 1
            && match instruction_length {
                5 => i8::try_from(operand.displacement).is_ok(),
                8 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let direct = operand.index.is_none()
            && operand.scale == 1
            && match instruction_length {
                4 => operand.displacement == 0,
                5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
                8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
                _ => false,
            };
        let indexed = instruction_length == 5 && operand.displacement == 0
            || instruction_length == 6
                && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
            || instruction_length == 9
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
        let indexed = indexed
            && operand.index.is_some_and(|index| (index as u8) < 4)
            && matches!(operand.scale, 1 | 2 | 4 | 8);
        let low_base_rsi_rdi_indexed_sib =
            is_address32_low_base_rsi_rdi_indexed_sib_operand(*operand, instruction_length);
        let low_base_no_index_sib =
            is_address32_low_base_no_index_sib_operand(*operand, instruction_length);
        let rbp_base_no_index_sib =
            is_address32_rbp_base_no_index_sib_operand(*operand, instruction_length);
        let rsi_rdi_base_no_index_sib =
            is_address32_rsi_rdi_base_no_index_sib_operand(*operand, instruction_length);
        let rsi_rdi_base_rsi_rdi_indexed_sib =
            is_address32_rsi_rdi_base_rsi_rdi_indexed_sib_data_operand(
                *operand,
                instruction_length,
            );
        let rbp_indexed = matches!(instruction_length, 5 | 6 | 9)
            && operand.index == Some(od_isa::Register::Rbp)
            && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && match instruction_length {
                5 => operand.displacement == 0,
                6 => i8::try_from(operand.displacement).is_ok(),
                9 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let rsi_rdi_base_rbp_indexed_sib = matches!(instruction_length, 5 | 6 | 9)
            && matches!(
                operand.base,
                AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
            )
            && operand.index == Some(od_isa::Register::Rbp)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && match instruction_length {
                5 => operand.displacement == 0,
                6 => i8::try_from(operand.displacement).is_ok(),
                9 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let rsp_base_rbp_indexed = matches!(instruction_length, 5 | 6 | 9)
            && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
            && operand.index == Some(od_isa::Register::Rbp)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && match instruction_length {
                5 => operand.displacement == 0,
                6 => i8::try_from(operand.displacement).is_ok(),
                9 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let rsp_base_rsi_rdi_indexed = is_address32_rsp_base_rsi_rdi_indexed_sib_stack_data_operand(
            *operand,
            instruction_length,
        );
        let rbp_base_rsi_rdi_indexed =
            is_address32_rbp_base_rsi_rdi_indexed_sib_data_operand(*operand, instruction_length);
        let rbp_base_rbp_indexed = matches!(instruction_length, 6 | 9)
            && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
            && operand.index == Some(od_isa::Register::Rbp)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && match instruction_length {
                6 => i8::try_from(operand.displacement).is_ok(),
                9 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        let absolute_sib_disp32 = instruction_length == 9
            && matches!(operand.base, AddressBase::None)
            && operand.index.is_none()
            && operand.scale == 1
            && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
        let indexed_no_base_sib_disp32 = instruction_length == 9
            && matches!(operand.base, AddressBase::None)
            && operand.index.is_some_and(|index| (index as u8) < 4)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
        let indexed_no_base_rbp_sib_disp32 = instruction_length == 9
            && matches!(operand.base, AddressBase::None)
            && operand.index == Some(od_isa::Register::Rbp)
            && matches!(operand.scale, 1 | 2 | 4 | 8)
            && i32::try_from(operand.displacement).is_ok();
        let indexed_no_base_rsi_rdi_sib_disp32 =
            is_address32_indexed_no_base_rsi_rdi_sib_data_operand(*operand, instruction_length);
        let indexed_base_supported = base_supported
            || operand.index.is_some_and(|index| (index as u8) < 4)
                && (matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                    || matches!(
                        operand.base,
                        AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
                    )
                    || matches!(instruction_length, 6 | 9)
                        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp)));
        let rsi_rdi_direct = matches!(instruction_length, 4 | 5 | 8)
            && matches!(
                operand.base,
                AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
            )
            && operand.index.is_none()
            && operand.scale == 1
            && match instruction_length {
                4 => operand.displacement == 0,
                5 => i8::try_from(operand.displacement).is_ok(),
                8 => i32::try_from(operand.displacement).is_ok(),
                _ => false,
            };
        return (base_supported && direct)
            || rsi_rdi_direct
            || (indexed_base_supported && indexed)
            || rbp_indexed
            || rsp_base_rbp_indexed
            || rsp_base_rsi_rdi_indexed
            || rbp_base_rsi_rdi_indexed
            || rbp_base_rbp_indexed
            || rsi_rdi_base_rbp_indexed_sib
            || low_base_rsi_rdi_indexed_sib
            || low_base_no_index_sib
            || rbp_base_no_index_sib
            || rsi_rdi_base_no_index_sib
            || rsi_rdi_base_rsi_rdi_indexed_sib
            || rsp_no_index_sib
            || rbp_direct
            || absolute_sib_disp32
            || indexed_no_base_sib_disp32
            || indexed_no_base_rbp_sib_disp32
            || indexed_no_base_rsi_rdi_sib_disp32;
    }
    if operand.address_size != od_isa::OperandSize::Qword {
        return false;
    }
    let rbp_indexed_sib = matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(register) if (register as u8) < 4
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        };
    let rsi_rdi_base_rbp_indexed_sib = matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        };
    let low_base_rsi_rdi_indexed_sib = matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        };
    let rsp_base_rbp_indexed_sib = matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
            8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
            _ => false,
        };
    let rbp_base_rbp_indexed_sib = matches!(instruction_length, 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        };
    let rbp_base_rsi_rdi_indexed_sib =
        is_rbp_base_rsi_rdi_index_qword_sib_data_operand(*operand, instruction_length);
    let rsi_rdi_base_rsi_rdi_indexed_sib =
        is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(*operand, instruction_length);
    let rsp_base_rsi_rdi_indexed_sib =
        is_rsp_base_rsi_rdi_index_qword_sib_data_operand(*operand, instruction_length);
    let indexed_no_base_rbp_sib_disp32 = instruction_length == 8
        && matches!(operand.base, AddressBase::None)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok();
    let indexed_no_base_rsi_rdi_sib_disp32 =
        is_indexed_no_base_rsi_rdi_sib_data_operand(*operand, instruction_length);
    let supported_index = match operand.index {
        Some(index) => {
            (matches!(index as u8, 0..=3)
                || rbp_indexed_sib
                || rsi_rdi_base_rbp_indexed_sib
                || low_base_rsi_rdi_indexed_sib
                || rsp_base_rbp_indexed_sib
                || rsp_base_rsi_rdi_indexed_sib
                || rbp_base_rbp_indexed_sib
                || rbp_base_rsi_rdi_indexed_sib
                || rsi_rdi_base_rsi_rdi_indexed_sib
                || indexed_no_base_rbp_sib_disp32
                || indexed_no_base_rsi_rdi_sib_disp32)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && match instruction_length {
                    4 => operand.displacement == 0,
                    5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
                    8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
                    _ => false,
                }
                && (matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
                    || matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                    || rsi_rdi_base_rbp_indexed_sib
                    || rsi_rdi_base_rsi_rdi_indexed_sib
                    || low_base_rsi_rdi_indexed_sib
                    || matches!(instruction_length, 5 | 8)
                        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
                    || matches!(instruction_length, 4 | 5 | 8)
                        && matches!(
                            operand.base,
                            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
                        )
                    || instruction_length == 8 && matches!(operand.base, AddressBase::None))
        }
        None => operand.scale == 1,
    };
    supported_index
        && (matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
            || rsi_rdi_base_rbp_indexed_sib
            || rsi_rdi_base_rsi_rdi_indexed_sib
            || low_base_rsi_rdi_indexed_sib
            || operand.index.is_some()
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
            || operand.index.is_some()
                && matches!(instruction_length, 5 | 8)
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
            || matches!(instruction_length, 4 | 5 | 8)
                && operand.index.is_some_and(|index| (index as u8) < 4)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && match instruction_length {
                    4 => operand.displacement == 0,
                    5 => i8::try_from(operand.displacement).is_ok(),
                    8 => i32::try_from(operand.displacement).is_ok(),
                    _ => false,
                }
                && matches!(
                    operand.base,
                    AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
                )
            || instruction_length == 4
                && operand.index.is_none()
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
            || instruction_length == 7
                && operand.index.is_none()
                && operand.scale == 1
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
            || is_rbp_base_no_index_qword_sib_data_operand(*operand, instruction_length)
            || is_rsi_rdi_base_no_index_qword_sib_data_operand(*operand, instruction_length)
            || matches!(instruction_length, 4 | 5 | 8)
                && operand.index.is_none()
                && operand.scale == 1
                && ((instruction_length == 4 && operand.displacement == 0)
                    || (instruction_length == 5
                        && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement))
                    || (instruction_length == 8
                        && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)))
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
            || matches!(operand.base, AddressBase::NextRip)
                && operand.index.is_none()
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
            || instruction_length == 8
                && matches!(operand.base, AddressBase::None)
                && operand.index.is_none()
                && operand.scale == 1
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
            || instruction_length == 8
                && matches!(operand.base, AddressBase::None)
                && operand.index.is_some_and(|index| (index as u8) < 4)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
            || matches!(instruction_length, 3 | 4 | 7)
                && operand.index.is_none()
                && operand.scale == 1
                && match instruction_length {
                    3 => operand.displacement == 0,
                    4 => i8::try_from(operand.displacement).is_ok(),
                    7 => i32::try_from(operand.displacement).is_ok(),
                    _ => false,
                }
                && matches!(
                    operand.base,
                    AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
                )
            || indexed_no_base_rbp_sib_disp32
            || indexed_no_base_rsi_rdi_sib_disp32)
}

fn validate_gpr(register: GuestGpr, operation: usize) -> Result<(), VerifyError> {
    if register.0 <= 3 {
        Ok(())
    } else {
        Err(VerifyError::InvalidGpr {
            operation,
            register: register.0,
        })
    }
}

fn is_direct_low_base_rsi_rdi_load_operand(
    operand: MemoryOperand,
    instruction_length: u64,
    data_register: u8,
) -> bool {
    matches!(data_register, 6 | 7)
        && (matches!(instruction_length, 3 | 4 | 7)
            && operand.address_size == od_isa::OperandSize::Qword
            || matches!(instruction_length, 4 | 5 | 8)
                && operand.address_size == od_isa::OperandSize::Dword)
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match (operand.address_size, instruction_length) {
            (od_isa::OperandSize::Qword, 3) => operand.displacement == 0,
            (od_isa::OperandSize::Qword, 4) => i8::try_from(operand.displacement).is_ok(),
            (od_isa::OperandSize::Qword, 7) => i32::try_from(operand.displacement).is_ok(),
            (od_isa::OperandSize::Dword, 4) => operand.displacement == 0,
            (od_isa::OperandSize::Dword, 5) => i8::try_from(operand.displacement).is_ok(),
            (od_isa::OperandSize::Dword, 8) => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_scoped_memory_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
    data_register: u8,
) -> bool {
    operand.size == od_isa::OperandSize::Qword
        && ((operand.address_size == od_isa::OperandSize::Qword
            && (is_indexed_qword_sib_data_operand(operand, instruction_length)
                || is_indexed_no_base_qword_sib_data_operand(operand, instruction_length)
                || is_indexed_no_base_rsi_rdi_sib_data_operand(operand, instruction_length)
                || is_rbp_index_qword_sib_data_operand(operand, instruction_length)
                || is_rsi_rdi_base_rbp_index_qword_sib_data_operand(operand, instruction_length)
                || is_rsp_base_rbp_index_qword_sib_data_operand(operand, instruction_length)
                || is_rsp_base_rsi_rdi_index_qword_sib_data_operand(operand, instruction_length)
                || is_rbp_base_rbp_index_qword_sib_data_operand(operand, instruction_length)
                || is_rbp_base_no_index_qword_sib_data_operand(operand, instruction_length)
                || is_rbp_base_rsi_rdi_index_qword_sib_data_operand(operand, instruction_length)
                || is_low_base_rsi_rdi_index_qword_sib_data_operand(operand, instruction_length)
                || is_low_base_no_index_qword_sib_data_operand(operand, instruction_length)
                || is_rsi_rdi_base_no_index_qword_sib_data_operand(operand, instruction_length)
                || is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(
                    operand,
                    instruction_length,
                )
                || (operand.index.is_none()
                    && operand.scale == 1
                    && match data_register {
                        4 => {
                            (matches!(instruction_length, 3 | 4 | 7)
                                && match instruction_length {
                                    3 => operand.displacement == 0,
                                    4 => i8::try_from(operand.displacement).is_ok(),
                                    7 => i32::try_from(operand.displacement).is_ok(),
                                    _ => false,
                                }
                                && matches!(
                                    operand.base,
                                    AddressBase::Register(
                                        od_isa::Register::Rsi | od_isa::Register::Rdi
                                    )
                                ))
                                || (instruction_length == 8
                                    && matches!(operand.base, AddressBase::None)
                                    && operand.index.is_none()
                                    && operand.scale == 1
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement))
                                || (instruction_length == 3
                                    && operand.displacement == 0
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(register) if (register as u8) < 4
                                    ))
                                || (instruction_length == 4
                                    && operand.displacement == 0
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                                || (instruction_length == 4
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rbp)
                                    ))
                                || (instruction_length == 4
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4))
                                || (instruction_length == 5
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                                || (instruction_length == 8
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rbp)
                                    ))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::NextRip))
                        }
                        5 => {
                            (matches!(instruction_length, 3 | 4 | 7)
                                && match instruction_length {
                                    3 => operand.displacement == 0,
                                    4 => i8::try_from(operand.displacement).is_ok(),
                                    7 => i32::try_from(operand.displacement).is_ok(),
                                    _ => false,
                                }
                                && matches!(
                                    operand.base,
                                    AddressBase::Register(
                                        od_isa::Register::Rsi | od_isa::Register::Rdi
                                    )
                                ))
                                || (instruction_length == 8
                                    && matches!(operand.base, AddressBase::None)
                                    && operand.index.is_none()
                                    && operand.scale == 1
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement))
                                || (instruction_length == 3
                                    && operand.displacement == 0
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(register) if (register as u8) < 4
                                    ))
                                || (instruction_length == 4
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rbp)
                                    ))
                                || (instruction_length == 4
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rbp)
                                    ))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4))
                                || (instruction_length == 7
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(operand.base, AddressBase::NextRip))
                                || (instruction_length == 4
                                    && operand.displacement == 0
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                                || (instruction_length == 5
                                    && (i8::MIN as i64..=i8::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                                || (instruction_length == 8
                                    && (i32::MIN as i64..=i32::MAX as i64)
                                        .contains(&operand.displacement)
                                    && matches!(
                                        operand.base,
                                        AddressBase::Register(od_isa::Register::Rsp)
                                    ))
                        }
                        _ => false,
                    })))
            || is_address32_direct_stack_data_operand(operand, instruction_length)
            || is_address32_rbp_base_no_index_sib_operand(operand, instruction_length)
            || is_address32_rsi_rdi_base_no_index_sib_operand(operand, instruction_length)
            || is_address32_indexed_sib_stack_data_operand(operand, instruction_length)
            || is_address32_rbp_indexed_sib_stack_data_operand(operand, instruction_length)
            || is_address32_low_base_rsi_rdi_indexed_sib_operand(operand, instruction_length)
            || is_address32_low_base_no_index_sib_operand(operand, instruction_length)
            || is_address32_rsi_rdi_base_rsi_rdi_indexed_sib_data_operand(
                operand,
                instruction_length,
            )
            || is_address32_rsi_rdi_base_rbp_indexed_sib_data_operand(operand, instruction_length)
            || is_address32_rsp_base_rbp_indexed_sib_stack_data_operand(
                operand,
                instruction_length,
            )
            || is_address32_rsp_base_rsi_rdi_indexed_sib_stack_data_operand(
                operand,
                instruction_length,
            )
            || is_address32_rbp_base_rsi_rdi_indexed_sib_data_operand(operand, instruction_length)
            || is_address32_rbp_base_rbp_indexed_sib_stack_data_operand(
                operand,
                instruction_length,
            )
            || is_address32_rsp_sib_stack_base_operand(operand, instruction_length)
            || is_address32_rbp_direct_stack_base_operand(operand, instruction_length)
            || is_address32_absolute_sib_stack_data_operand(operand, instruction_length)
            || is_address32_indexed_no_base_sib_stack_data_operand(operand, instruction_length)
            || is_address32_indexed_no_base_rbp_sib_stack_data_operand(operand, instruction_length)
            || is_address32_indexed_no_base_rsi_rdi_sib_stack_data_operand(
                operand,
                instruction_length,
            )
            || is_indexed_no_base_rbp_sib_stack_data_operand(operand, instruction_length))
}

fn is_low_base_no_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rsi_rdi_base_no_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_indexed_no_base_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    instruction_length == 8
        && matches!(operand.base, AddressBase::None)
        && operand.index.is_some_and(|index| (index as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && operand.size == od_isa::OperandSize::Qword
        && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
}

fn is_indexed_no_base_rbp_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    instruction_length == 8
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::None)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_indexed_no_base_rsi_rdi_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    instruction_length == 8
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::None)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_absolute_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && instruction_length == 9
        && matches!(operand.base, AddressBase::None)
        && operand.index.is_none()
        && operand.scale == 1
        && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
}

fn is_address32_indexed_no_base_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && instruction_length == 9
        && matches!(operand.base, AddressBase::None)
        && operand.index.is_some_and(|index| (index as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && operand.size == od_isa::OperandSize::Qword
        && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
}

fn is_address32_indexed_no_base_rbp_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && instruction_length == 9
        && matches!(operand.base, AddressBase::None)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && operand.size == od_isa::OperandSize::Qword
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_indexed_no_base_rsi_rdi_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && instruction_length == 9
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::None)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_indexed_no_base_rsi_rdi_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    is_address32_indexed_no_base_rsi_rdi_sib_data_operand(operand, instruction_length)
}

fn is_address32_direct_stack_data_operand(operand: MemoryOperand, instruction_length: u64) -> bool {
    let displacement_valid = match instruction_length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.index.is_none()
        && operand.scale == 1
        && matches!(
            operand.base,
            AddressBase::Register(
                od_isa::Register::Rax
                    | od_isa::Register::Rcx
                    | od_isa::Register::Rdx
                    | od_isa::Register::Rbx
                    | od_isa::Register::Rsi
                    | od_isa::Register::Rdi
            )
        )
        && displacement_valid
}

fn is_address32_indexed_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && (matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
            || matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
            || matches!(
                operand.base,
                AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
            )
            || matches!(instruction_length, 6 | 9)
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp)))
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_low_base_rsi_rdi_indexed_sib_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_low_base_no_index_sib_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(instruction_length, 5 | 6 | 9)
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rbp_base_no_index_sib_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(instruction_length, 6 | 9)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rsi_rdi_base_no_index_sib_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(instruction_length, 5 | 6 | 9)
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rbp_indexed_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsi_rdi_base_rbp_indexed_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsi_rdi_base_rsi_rdi_indexed_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rsp_base_rbp_indexed_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsp_base_rsi_rdi_indexed_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rbp_base_rsi_rdi_indexed_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rbp_base_rbp_indexed_sib_stack_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && operand.size == od_isa::OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsp_sib_stack_base_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index.is_none()
        && operand.scale == 1
        && displacement_valid
}

fn is_address32_rbp_direct_stack_base_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    let displacement_valid = match instruction_length {
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    operand.address_size == od_isa::OperandSize::Dword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index.is_none()
        && operand.scale == 1
        && displacement_valid
}

fn is_indexed_qword_sib_data_operand(operand: MemoryOperand, instruction_length: u64) -> bool {
    let displacement_valid = match instruction_length {
        4 => operand.displacement == 0,
        5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
        8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
        _ => false,
    };
    (matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        || matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        || matches!(instruction_length, 4 | 5 | 8)
            && matches!(
                operand.base,
                AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
            )
        || matches!(instruction_length, 5 | 8)
            && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp)))
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rbp_index_qword_sib_data_operand(operand: MemoryOperand, instruction_length: u64) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(register) if (register as u8) < 4
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rsi_rdi_base_rbp_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_low_base_rsi_rdi_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(register) if (register as u8) < 4)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(
            operand.base,
            AddressBase::Register(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rsp_base_rbp_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rsp_base_rsi_rdi_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 4 | 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rbp_base_rbp_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rbp_base_no_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction_length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn is_rbp_base_rsi_rdi_index_qword_sib_data_operand(
    operand: MemoryOperand,
    instruction_length: u64,
) -> bool {
    matches!(instruction_length, 5 | 8)
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp))
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction_length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && operand.address_size == od_isa::OperandSize::Qword
        && operand.size == od_isa::OperandSize::Qword
}

fn validate_flags(flags: FlagEffect, operation: usize) -> Result<(), VerifyError> {
    const S0_FLAGS: u64 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
    if let FlagEffect::WriteMask(mask) = flags {
        if mask & !S0_FLAGS != 0 {
            return Err(VerifyError::InvalidFlagMask { operation, mask });
        }
    }
    Ok(())
}

fn define(
    definitions: &mut HashMap<TempId, IntWidth>,
    temp: TempId,
    width: IntWidth,
    operation: usize,
) -> Result<(), VerifyError> {
    if definitions.insert(temp, width).is_some() {
        return Err(VerifyError::DuplicateTemporary { operation, temp });
    }
    Ok(())
}

fn require_temp(
    definitions: &HashMap<TempId, IntWidth>,
    temp: TempId,
    expected: IntWidth,
    operation: usize,
) -> Result<(), VerifyError> {
    let Some(actual) = definitions.get(&temp).copied() else {
        return Err(VerifyError::UndefinedTemporary { operation, temp });
    };
    if actual != expected {
        return Err(VerifyError::WidthMismatch {
            operation,
            temp,
            expected,
            actual,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked_load_program() -> Program {
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x2000,
                next_rip: 0x2003,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedLoad {
                    dst: TempId(0),
                    operand: MemoryOperand {
                        base: AddressBase::Register(od_isa::Register::Rax),
                        index: None,
                        scale: 1,
                        displacement: 0,
                        address_size: od_isa::OperandSize::Qword,
                        size: od_isa::OperandSize::Qword,
                    },
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(0),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x2003,
                },
            ],
            code_end: 0x2003,
        }
    }

    #[test]
    fn checked_load_requires_valid_descriptor_result_write_and_commit_order() {
        assert_eq!(verify(&checked_load_program()), Ok(()));

        let mut malformed = checked_load_program();
        if let Operation::CheckedLoad { operand, .. } = &mut malformed.operations[1] {
            operand.index = Some(od_isa::Register::Rbx);
        }
        assert!(matches!(
            verify(&malformed),
            Err(VerifyError::InvalidCheckedLoadDescriptor { .. })
        ));

        let mut unavailable = checked_load_program();
        if let Operation::WriteGpr { src, .. } = &mut unavailable.operations[2] {
            *src = TempId(8);
        }
        assert!(matches!(
            verify(&unavailable),
            Err(VerifyError::UndefinedTemporary {
                temp: TempId(8),
                ..
            })
        ));

        let mut missing_write = checked_load_program();
        missing_write.operations.remove(2);
        assert!(matches!(
            verify(&missing_write),
            Err(VerifyError::CheckedLoadWithoutWrite { .. })
        ));

        let mut early_commit = checked_load_program();
        early_commit.operations.swap(2, 3);
        assert!(matches!(
            verify(&early_commit),
            Err(VerifyError::CheckedLoadWithoutWrite { .. })
        ));
    }

    #[test]
    fn rsi_rdi_write_is_scoped_to_immediately_preceding_exact_low_base_load() {
        for register in [6_u8, 7] {
            let mut allowed = checked_load_program();
            let Operation::WriteGpr { register: dst, .. } = &mut allowed.operations[2] else {
                unreachable!();
            };
            *dst = GuestGpr(register);
            assert_eq!(verify(&allowed), Ok(()));

            let mut wrong_temp = allowed.clone();
            if let Operation::WriteGpr { src, .. } = &mut wrong_temp.operations[2] {
                *src = TempId(99);
            }
            assert!(matches!(
                verify(&wrong_temp),
                Err(VerifyError::InvalidGpr { register: rejected, .. }) if rejected == register
            ));

            let mut non_adjacent = allowed.clone();
            non_adjacent.operations.insert(
                2,
                Operation::Constant {
                    dst: TempId(1),
                    width: IntWidth::W64,
                    value: 1,
                },
            );
            assert!(matches!(
                verify(&non_adjacent),
                Err(VerifyError::InvalidGpr { register: rejected, .. }) if rejected == register
            ));

            let mut wrong_length = allowed;
            wrong_length.guest_instructions[0].next_rip = 0x2005;
            wrong_length.code_end = 0x2005;
            if let Some(Operation::Exit { target_rip, .. }) = wrong_length.operations.last_mut() {
                *target_rip = 0x2005;
            }
            assert!(matches!(
                verify(&wrong_length),
                Err(VerifyError::InvalidGpr { register: rejected, .. }) if rejected == register
            ));
        }
    }

    #[test]
    fn checked_load_addr32_no_base_rbp_index_requires_exact_nine_byte_profile() {
        for length in [8_u64, 9] {
            let mut forged = checked_load_program();
            let next_rip = 0x2000 + length;
            forged.guest_instructions[0].next_rip = next_rip;
            forged.code_end = next_rip;
            if let Operation::CheckedLoad { operand, .. } = &mut forged.operations[1] {
                *operand = MemoryOperand {
                    base: AddressBase::None,
                    index: Some(od_isa::Register::Rbp),
                    scale: 1,
                    displacement: 0,
                    address_size: od_isa::OperandSize::Dword,
                    size: od_isa::OperandSize::Qword,
                };
            }
            if let Operation::Exit { target_rip, .. } = forged.operations.last_mut().unwrap() {
                *target_rip = next_rip;
            }
            if length == 8 {
                assert!(matches!(
                    verify(&forged),
                    Err(VerifyError::InvalidCheckedLoadDescriptor { .. })
                ));
            } else {
                assert!(verify(&forged).is_ok());
            }
        }
    }

    #[test]
    fn checked_load_addr32_rsi_indexed_sib_is_admitted_by_address32_profile() {
        let mut forged = checked_load_program();
        forged.guest_instructions[0].next_rip = 0x2005;
        forged.code_end = 0x2005;
        if let Operation::CheckedLoad { operand, .. } = &mut forged.operations[1] {
            *operand = MemoryOperand {
                base: AddressBase::Register(od_isa::Register::Rsi),
                index: Some(od_isa::Register::Rax),
                scale: 1,
                displacement: 0,
                address_size: od_isa::OperandSize::Dword,
                size: od_isa::OperandSize::Qword,
            };
        }
        if let Operation::Exit { target_rip, .. } = forged.operations.last_mut().unwrap() {
            *target_rip = 0x2005;
        }
        assert!(verify(&forged).is_ok());
    }

    fn checked_store_program() -> Program {
        let operand = MemoryOperand {
            base: AddressBase::Register(od_isa::Register::Rax),
            index: None,
            scale: 1,
            displacement: 0,
            address_size: od_isa::OperandSize::Qword,
            size: od_isa::OperandSize::Qword,
        };
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: 0x3000,
                next_rip: 0x3003,
            }],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedStore {
                    src: GuestGpr(0),
                    operand,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x3003,
                },
            ],
            code_end: 0x3003,
        }
    }

    fn checked_store_then_load_program() -> Program {
        let operand = MemoryOperand {
            base: AddressBase::Register(od_isa::Register::Rbx),
            index: None,
            scale: 1,
            displacement: 0,
            address_size: od_isa::OperandSize::Qword,
            size: od_isa::OperandSize::Qword,
        };
        Program {
            guest_instructions: vec![
                GuestInstruction {
                    guest_rip: 0x4000,
                    next_rip: 0x4003,
                },
                GuestInstruction {
                    guest_rip: 0x4003,
                    next_rip: 0x4006,
                },
            ],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::CheckedStore {
                    src: GuestGpr(0),
                    operand,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
                Operation::CheckedLoad {
                    dst: TempId(0),
                    operand,
                },
                Operation::WriteGpr {
                    register: GuestGpr(1),
                    src: TempId(0),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 1 },
                Operation::Exit {
                    kind: ExitKind::BlockComplete,
                    target_rip: 0x4006,
                },
            ],
            code_end: 0x4006,
        }
    }

    #[test]
    fn two_memory_effects_are_bound_to_separate_commits_and_keep_store_order() {
        let program = checked_store_then_load_program();
        assert_eq!(verify(&program), Ok(()));
        let mut missing_store_commit = program.clone();
        missing_store_commit.operations.remove(2);
        assert!(matches!(
            verify(&missing_store_commit),
            Err(VerifyError::CheckedStoreCommitOrder { .. })
        ));
        let mut effect_before_store_commit = program.clone();
        effect_before_store_commit.operations.swap(2, 3);
        assert!(verify(&effect_before_store_commit).is_err());
        let mut early_load_commit = program.clone();
        early_load_commit.operations.swap(5, 6);
        assert!(matches!(
            verify(&early_load_commit),
            Err(VerifyError::CheckedLoadWithoutWrite { .. })
        ));
        let mut wrong_commit_index = program;
        wrong_commit_index.operations[6] = Operation::CommitInstruction { index: 0 };
        assert!(matches!(
            verify(&wrong_commit_index),
            Err(VerifyError::CommitIndexMismatch { .. })
        ));
    }

    #[test]
    fn checked_store_is_unique_and_commits_immediately_after_write() {
        let program = checked_store_program();
        assert_eq!(verify(&program), Ok(()));
        let mut duplicate = program.clone();
        duplicate
            .operations
            .insert(2, duplicate.operations[1].clone());
        assert!(matches!(
            verify(&duplicate),
            Err(VerifyError::CheckedStoreCommitOrder { .. })
        ));
        let mut missing_commit = program.clone();
        missing_commit.operations.remove(2);
        assert!(matches!(
            verify(&missing_commit),
            Err(VerifyError::CheckedStoreCommitOrder { .. })
        ));
        let mut early_commit = program.clone();
        early_commit.operations.swap(1, 2);
        assert!(verify(&early_commit).is_err());
        let mut effect_after_store = program;
        effect_after_store.operations.insert(
            2,
            Operation::ReadGpr {
                dst: TempId(4),
                register: GuestGpr(1),
                width: IntWidth::W64,
            },
        );
        assert!(matches!(
            verify(&effect_after_store),
            Err(VerifyError::CheckedStoreCommitOrder { .. })
        ));
    }

    fn base_program() -> Program {
        Program {
            code_end: 0x1005,
            guest_instructions: vec![
                GuestInstruction {
                    guest_rip: 0x1000,
                    next_rip: 0x1003,
                },
                GuestInstruction {
                    guest_rip: 0x1003,
                    next_rip: 0x1005,
                },
            ],
            operations: vec![
                Operation::BeginInstruction { index: 0 },
                Operation::ReadGpr {
                    dst: TempId(0),
                    register: GuestGpr(0),
                    width: IntWidth::W64,
                },
                Operation::Constant {
                    dst: TempId(1),
                    width: IntWidth::W64,
                    value: 5,
                },
                Operation::Integer {
                    kind: IntegerKind::Add,
                    lhs: TempId(0),
                    rhs: TempId(1),
                    result: Some(TempId(2)),
                    width: IntWidth::W64,
                    flags: FlagEffect::Arithmetic,
                },
                Operation::WriteGpr {
                    register: GuestGpr(0),
                    src: TempId(2),
                    width: IntWidth::W64,
                },
                Operation::CommitInstruction { index: 0 },
                Operation::BeginInstruction { index: 1 },
                Operation::CommitInstruction { index: 1 },
                Operation::Branch {
                    condition: BranchCondition::Zero,
                    target_rip: 0x1000,
                    fallthrough_rip: 0x1005,
                },
            ],
        }
    }

    #[test]
    fn accepts_typed_effects_with_commit_and_known_branch_boundaries() {
        assert_eq!(verify(&base_program()), Ok(()));
    }

    #[test]
    fn rejects_undefined_temporary() {
        let mut program = base_program();
        program.operations[3] = Operation::Integer {
            kind: IntegerKind::Add,
            lhs: TempId(99),
            rhs: TempId(1),
            result: Some(TempId(2)),
            width: IntWidth::W64,
            flags: FlagEffect::Arithmetic,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::UndefinedTemporary {
                temp: TempId(99),
                ..
            })
        ));
    }

    #[test]
    fn rejects_width_mismatch() {
        let mut program = base_program();
        program.operations[2] = Operation::Constant {
            dst: TempId(1),
            width: IntWidth::W32,
            value: 5,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::WidthMismatch {
                temp: TempId(1),
                ..
            })
        ));
    }

    #[test]
    fn rejects_invalid_register_and_flag_references() {
        let mut program = base_program();
        program.operations[1] = Operation::ReadGpr {
            dst: TempId(0),
            register: GuestGpr(4),
            width: IntWidth::W64,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::InvalidGpr { register: 4, .. })
        ));

        for register in [6_u8, 7] {
            let mut program = base_program();
            program.operations[1] = Operation::ReadGpr {
                dst: TempId(0),
                register: GuestGpr(register),
                width: IntWidth::W64,
            };
            assert!(matches!(
                verify(&program),
                Err(VerifyError::InvalidGpr { register: rejected, .. }) if rejected == register
            ));

            let mut program = base_program();
            program.operations[4] = Operation::WriteGpr {
                register: GuestGpr(register),
                src: TempId(2),
                width: IntWidth::W64,
            };
            assert!(matches!(
                verify(&program),
                Err(VerifyError::InvalidGpr { register: rejected, .. }) if rejected == register
            ));
        }

        let mut program = base_program();
        program.operations[3] = Operation::Integer {
            kind: IntegerKind::Add,
            lhs: TempId(0),
            rhs: TempId(1),
            result: Some(TempId(2)),
            width: IntWidth::W64,
            flags: FlagEffect::WriteMask(1 << 9),
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::InvalidFlagMask { .. })
        ));
    }

    #[test]
    fn rejects_missing_or_duplicate_commit() {
        let mut program = base_program();
        program.operations.remove(5);
        assert!(matches!(
            verify(&program),
            Err(VerifyError::MissingCommit { index: 0 })
        ));

        let mut program = base_program();
        program
            .operations
            .insert(6, Operation::CommitInstruction { index: 0 });
        assert!(matches!(
            verify(&program),
            Err(VerifyError::CommitWithoutInstruction { .. })
        ));
    }

    #[test]
    fn rejects_targets_not_in_record_boundaries_and_invalid_terminal_continuations() {
        let mut program = base_program();
        *program.operations.last_mut().unwrap() = Operation::Branch {
            condition: BranchCondition::Zero,
            target_rip: 0x1004,
            fallthrough_rip: 0x1005,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::InvalidBranchTarget { rip: 0x1004, .. })
        ));

        let mut program = base_program();
        *program.operations.last_mut().unwrap() = Operation::Branch {
            condition: BranchCondition::NotZero,
            target_rip: 0x1000,
            fallthrough_rip: 0x1003,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::InvalidFallthrough { rip: 0x1003, .. })
        ));

        let mut program = base_program();
        *program.operations.last_mut().unwrap() = Operation::Exit {
            kind: ExitKind::BlockComplete,
            target_rip: 0x1003,
        };
        assert!(matches!(
            verify(&program),
            Err(VerifyError::InvalidTerminalRip { rip: 0x1003, .. })
        ));
    }

    #[test]
    fn rejects_non_contiguous_guest_records_and_missing_terminal() {
        let mut program = base_program();
        program.guest_instructions[1].guest_rip = 0x1004;
        assert!(matches!(
            verify(&program),
            Err(VerifyError::DiscontinuousInstructionRange { index: 1 })
        ));

        let mut program = base_program();
        program.operations.pop();
        assert_eq!(verify(&program), Err(VerifyError::MissingTerminal));
    }

    #[test]
    fn rejects_guest_records_beyond_source_code_end() {
        let mut program = base_program();
        program.code_end = 0x1004;
        assert_eq!(
            verify(&program),
            Err(VerifyError::InstructionOutsideCodeEnd {
                index: 1,
                code_end: 0x1004,
            })
        );
    }
}
