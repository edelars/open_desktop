//! Lower decoded S0-09 guest records into the linear typed IR.
//!
//! This module maps decoder effects; it does not decode x86 again or define
//! arithmetic/flags semantics. Each included guest instruction has one
//! commit marker. For a terminal Jcc, that marker and the immediately
//! following `Branch` together represent retirement and target selection;
//! no observable exit or helper may be inserted between them.

use od_isa::RegisterView;
use od_isa::long_decode::{ArithmeticKind, ConditionCode, Operand, Operation as DecodedOperation};

use crate::block::{
    self, AdmittedInstructionBoundaries, BlockStopReason, BranchTargetResolution, BuildOutcome,
    DecodeBoundary, DecodedInstruction,
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
    lower_with_admission(outcome, None)
}

/// Lower a block for a future dispatcher using complete fixture-v1 decode
/// boundaries. Unlike `lower`, an admitted Jcc commits and returns a typed
/// dispatch candidate; unresolved targets absent from the proof remain
/// non-retired `NeedInterpreter` exits.
pub(crate) fn lower_for_dispatch(
    outcome: &BuildOutcome,
    admission: &AdmittedInstructionBoundaries,
) -> Result<LoweringOutcome, LoweringError> {
    lower_with_admission(outcome, Some(admission))
}

fn lower_with_admission(
    outcome: &BuildOutcome,
    admission: Option<&AdmittedInstructionBoundaries>,
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
        if !block::is_s0_09_form(instruction.decoded) {
            return Err(LoweringError::InvalidDecodedForm {
                guest_rip: instruction.guest_rip,
            });
        }
        records.push(GuestInstruction {
            guest_rip: instruction.guest_rip,
            next_rip: instruction.next_rip,
        });
        lower_instruction(instruction, index, &mut next_temp, &mut operations)?;
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
