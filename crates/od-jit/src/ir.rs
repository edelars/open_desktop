//! Small, linear, typed IR for the first register-only block compiler.
//!
//! This module records effects supplied by a future decoder adapter. It does
//! not implement x86 instruction semantics or lower operations to host code.

use std::collections::HashMap;

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

/// Register number from the guest encoding. The verifier accepts only the
/// four GPRs in the initial S0-09 JIT profile (RAX..RBX).
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
                validate_gpr(*register, operation_index)?;
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
