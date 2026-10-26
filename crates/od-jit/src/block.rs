//! Bounded, read-only construction of one guest basic block.
//!
//! The builder uses the long-mode decoder one instruction at a time. It does
//! not execute instructions or translate them to IR/native code.

use od_isa::OperandSize;
use od_isa::long_decode::{
    self, ArithmeticKind, ConditionCode, DecodeError, Instruction, OpcodeMap, Operand, Operation,
};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

pub const MAX_BLOCK_INSTRUCTIONS: usize = 32;
pub const MAX_BLOCK_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockLimits {
    pub instructions: usize,
    pub bytes: usize,
}

impl Default for BlockLimits {
    fn default() -> Self {
        Self {
            instructions: MAX_BLOCK_INSTRUCTIONS,
            bytes: MAX_BLOCK_BYTES,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildError {
    AddressOverflow {
        base_rip: u64,
        length: usize,
    },
    EntryOutsideCode {
        entry_rip: u64,
        base_rip: u64,
        code_end: u64,
    },
    InvalidLimits {
        limits: BlockLimits,
    },
    InvalidDecodedLength {
        guest_rip: u64,
        length: u8,
    },
    DecodedRangeOverflow {
        guest_rip: u64,
        length: u8,
    },
}

/// Immutable borrowed code with checked guest-address bounds.
#[derive(Clone, Copy, Debug)]
pub struct CodeView<'a> {
    base_rip: u64,
    code_end: u64,
    bytes: &'a [u8],
}

impl<'a> CodeView<'a> {
    pub fn new(base_rip: u64, bytes: &'a [u8]) -> Result<Self, BuildError> {
        let length = u64::try_from(bytes.len()).map_err(|_| BuildError::AddressOverflow {
            base_rip,
            length: bytes.len(),
        })?;
        let code_end = base_rip
            .checked_add(length)
            .ok_or(BuildError::AddressOverflow {
                base_rip,
                length: bytes.len(),
            })?;
        Ok(Self {
            base_rip,
            code_end,
            bytes,
        })
    }

    pub const fn base_rip(self) -> u64 {
        self.base_rip
    }

    pub const fn code_end(self) -> u64 {
        self.code_end
    }

    pub fn len(self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub(crate) fn bytes(self) -> &'a [u8] {
        self.bytes
    }

    fn offset(self, rip: u64) -> Option<usize> {
        let offset = rip.checked_sub(self.base_rip)?;
        let offset = usize::try_from(offset).ok()?;
        (offset <= self.bytes.len()).then_some(offset)
    }
}

/// Full fixture-v1 instruction boundaries admitted by `od_isa::decode`.
///
/// This is distinct from a bounded block's decoded prefix: it may establish
/// that a branch target in the fixture tail is an instruction start, without
/// asking the block builder to fetch or decode that tail again.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmittedInstructionBoundaries {
    code_base: u64,
    code_end: u64,
    code_bytes: Vec<u8>,
    boundaries: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionError {
    FixtureDecodeRejected(od_isa::DecodeError),
    FixtureDecodeDoesNotMatchCode,
    NonContiguousFixtureDecode {
        index: usize,
        expected_offset: usize,
        actual_offset: usize,
    },
    InvalidFixtureInstructionLength {
        index: usize,
    },
    IncompleteFixtureDecode {
        decoded_length: usize,
        code_length: usize,
    },
}

impl AdmittedInstructionBoundaries {
    /// Construct from the complete successful output of fixture-v1 decoding.
    /// The offsets must cover the entire immutable `CodeView` contiguously.
    pub fn from_fixture_decode(
        code: CodeView<'_>,
        decoded: &[od_isa::Instruction],
    ) -> Result<Self, AdmissionError> {
        let actual_decode =
            od_isa::decode(code.bytes()).map_err(AdmissionError::FixtureDecodeRejected)?;
        if actual_decode != decoded {
            return Err(AdmissionError::FixtureDecodeDoesNotMatchCode);
        }

        let mut boundaries = Vec::with_capacity(decoded.len() + 1);
        let mut expected_offset = 0usize;
        for (index, instruction) in decoded.iter().enumerate() {
            if instruction.offset != expected_offset {
                return Err(AdmissionError::NonContiguousFixtureDecode {
                    index,
                    expected_offset,
                    actual_offset: instruction.offset,
                });
            }
            if instruction.length == 0 {
                return Err(AdmissionError::InvalidFixtureInstructionLength { index });
            }
            let end_offset = instruction
                .offset
                .checked_add(instruction.length)
                .ok_or(AdmissionError::InvalidFixtureInstructionLength { index })?;
            if end_offset > code.len() {
                return Err(AdmissionError::InvalidFixtureInstructionLength { index });
            }
            let offset = u64::try_from(instruction.offset)
                .map_err(|_| AdmissionError::InvalidFixtureInstructionLength { index })?;
            boundaries.push(
                code.base_rip()
                    .checked_add(offset)
                    .ok_or(AdmissionError::InvalidFixtureInstructionLength { index })?,
            );
            expected_offset = end_offset;
        }
        if expected_offset != code.len() {
            return Err(AdmissionError::IncompleteFixtureDecode {
                decoded_length: expected_offset,
                code_length: code.len(),
            });
        }
        boundaries.push(code.code_end());
        Ok(Self {
            code_base: code.base_rip(),
            code_end: code.code_end(),
            code_bytes: code.bytes().to_vec(),
            boundaries,
        })
    }

    pub const fn code_base(&self) -> u64 {
        self.code_base
    }

    pub const fn code_end(&self) -> u64 {
        self.code_end
    }

    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(&rip)
    }

    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.code_base == code.base_rip()
            && self.code_end == code.code_end()
            && self.code_bytes == code.bytes()
    }

    pub(crate) fn covers_records(&self, records: &[crate::ir::GuestInstruction]) -> bool {
        records.iter().all(|record| {
            record.guest_rip < record.next_rip
                && self.contains(record.guest_rip)
                && self.contains(record.next_rip)
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GuestByteRange {
    pub start_rip: u64,
    /// Exclusive end. An instruction crossing a page remains one exact range.
    pub end_rip: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedInstruction {
    pub guest_rip: u64,
    pub next_rip: u64,
    pub decoded: Instruction,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchTargetResolution {
    CodeEnd,
    KnownInstructionStart,
    /// The branch target equals the sequential RIP after this decoded Jcc.
    /// This boundary is known even though the following bytes are not decoded.
    NextRipBoundary,
    /// The target is not part of the decoded prefix; later block/dispatcher
    /// work must resolve it without pretending `ir::verify` proved it.
    Unresolved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BranchTarget {
    pub rip: u64,
    pub resolution: BranchTargetResolution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodeBoundary {
    pub guest_rip: u64,
    pub error: DecodeError,
    /// Bytes successfully fetched before the error, when the bounded view
    /// exposes the failing fetch address.
    pub consumed_prefix_length: Option<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlockStopReason {
    Branch {
        target: BranchTarget,
    },
    UnsupportedBranchTarget {
        branch_rip: u64,
        target_rip: u64,
    },
    BufferEnd,
    InstructionLimit,
    ByteLimit,
    UnsupportedDecoded {
        guest_rip: u64,
        decoded: Instruction,
    },
    UnsupportedEncoding {
        guest_rip: u64,
        error: DecodeError,
    },
    DecodeBoundary(DecodeBoundary),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BasicBlock {
    pub entry_rip: u64,
    /// Exclusive end copied from the validated source `CodeView`.
    pub code_end: u64,
    pub instructions: Vec<DecodedInstruction>,
    pub guest_ranges: Vec<GuestByteRange>,
    pub stop_reason: BlockStopReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NoBlock {
    pub entry_rip: u64,
    pub stop_reason: BlockStopReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuildOutcome {
    Block(BasicBlock),
    NoBlock(NoBlock),
}

/// Decode a bounded block from `entry_rip`. The code slice is borrowed and
/// immutable; fetch failures describe this view's boundary, not guest paging.
pub fn build_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    validate_limits(limits)?;
    let entry_offset = code.offset(entry_rip).ok_or(BuildError::EntryOutsideCode {
        entry_rip,
        base_rip: code.base_rip,
        code_end: code.code_end,
    })?;

    if entry_rip == code.code_end {
        return Ok(finish(
            entry_rip,
            code.code_end,
            Vec::new(),
            Vec::new(),
            BlockStopReason::BufferEnd,
        ));
    }

    let read_end_offset = entry_offset
        .checked_add(limits.bytes)
        .ok_or(BuildError::AddressOverflow {
            base_rip: entry_rip,
            length: limits.bytes,
        })?
        .min(code.bytes.len());
    let read_end_rip = code
        .base_rip
        .checked_add(
            u64::try_from(read_end_offset).map_err(|_| BuildError::AddressOverflow {
                base_rip: code.base_rip,
                length: read_end_offset,
            })?,
        )
        .ok_or(BuildError::AddressOverflow {
            base_rip: code.base_rip,
            length: read_end_offset,
        })?;

    let mut instructions = Vec::new();
    let mut guest_ranges = Vec::new();
    let mut guest_rip = entry_rip;
    loop {
        let consumed_bytes = guest_rip
            .checked_sub(entry_rip)
            .and_then(|delta| usize::try_from(delta).ok())
            .ok_or(BuildError::AddressOverflow {
                base_rip: entry_rip,
                length: limits.bytes,
            })?;
        if instructions.len() >= limits.instructions {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::InstructionLimit,
            ));
        }
        if consumed_bytes >= limits.bytes {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::ByteLimit,
            ));
        }
        if guest_rip == code.code_end {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::BufferEnd,
            ));
        }

        let mut memory = CodeMemory { code, read_end_rip };
        let decoded = match long_decode::decode(&mut memory, guest_rip, context) {
            Ok(instruction) => instruction,
            Err(error) => {
                if is_byte_limit_fetch(&error, read_end_rip, code.code_end) {
                    return Ok(finish(
                        entry_rip,
                        code.code_end,
                        instructions,
                        guest_ranges,
                        BlockStopReason::ByteLimit,
                    ));
                }
                let reason = match error {
                    DecodeError::Unsupported { .. } => {
                        BlockStopReason::UnsupportedEncoding { guest_rip, error }
                    }
                    error => BlockStopReason::DecodeBoundary(deferred_boundary(guest_rip, error)),
                };
                return Ok(finish(
                    entry_rip,
                    code.code_end,
                    instructions,
                    guest_ranges,
                    reason,
                ));
            }
        };
        if !(1..=15).contains(&decoded.length) {
            return Err(BuildError::InvalidDecodedLength {
                guest_rip,
                length: decoded.length,
            });
        }

        if !is_s0_09_form(decoded) {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::UnsupportedDecoded { guest_rip, decoded },
            ));
        }

        let next_rip = guest_rip.checked_add(u64::from(decoded.length)).ok_or(
            BuildError::DecodedRangeOverflow {
                guest_rip,
                length: decoded.length,
            },
        )?;
        if next_rip > read_end_rip {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::ByteLimit,
            ));
        }
        if next_rip > code.code_end {
            return Err(BuildError::DecodedRangeOverflow {
                guest_rip,
                length: decoded.length,
            });
        }

        let branch_target = if let Operation::Branch { displacement, .. } = decoded.operation {
            let target_rip = next_rip.wrapping_add_signed(i64::from(displacement));
            let candidate = DecodedInstruction {
                guest_rip,
                next_rip,
                decoded,
            };
            match classify_target(target_rip, code.code_end, &instructions, candidate) {
                TargetClassification::InsideDecodedInstruction => {
                    return Ok(finish(
                        entry_rip,
                        code.code_end,
                        instructions,
                        guest_ranges,
                        BlockStopReason::UnsupportedBranchTarget {
                            branch_rip: guest_rip,
                            target_rip,
                        },
                    ));
                }
                TargetClassification::Resolved(resolution) => Some(BranchTarget {
                    rip: target_rip,
                    resolution,
                }),
            }
        } else {
            None
        };

        instructions.push(DecodedInstruction {
            guest_rip,
            next_rip,
            decoded,
        });
        guest_ranges.push(GuestByteRange {
            start_rip: guest_rip,
            end_rip: next_rip,
        });

        if let Some(target) = branch_target {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::Branch { target },
            ));
        }
        guest_rip = next_rip;
    }
}

fn validate_limits(limits: BlockLimits) -> Result<(), BuildError> {
    if limits.instructions == 0
        || limits.instructions > MAX_BLOCK_INSTRUCTIONS
        || limits.bytes == 0
        || limits.bytes > MAX_BLOCK_BYTES
    {
        return Err(BuildError::InvalidLimits { limits });
    }
    Ok(())
}

fn finish(
    entry_rip: u64,
    code_end: u64,
    instructions: Vec<DecodedInstruction>,
    guest_ranges: Vec<GuestByteRange>,
    stop_reason: BlockStopReason,
) -> BuildOutcome {
    if instructions.is_empty() {
        BuildOutcome::NoBlock(NoBlock {
            entry_rip,
            stop_reason,
        })
    } else {
        BuildOutcome::Block(BasicBlock {
            entry_rip,
            code_end,
            instructions,
            guest_ranges,
            stop_reason,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(
        bytes: &[u8],
        base: u64,
        entry: u64,
        limits: BlockLimits,
    ) -> Result<BuildOutcome, BuildError> {
        build_block(
            CodeView::new(base, bytes).unwrap(),
            entry,
            AccessContext::default(),
            limits,
        )
    }

    fn mov_rax_imm(value: u64) -> [u8; 10] {
        let mut bytes = [0; 10];
        bytes[0] = 0x48;
        bytes[1] = 0xb8;
        bytes[2..].copy_from_slice(&value.to_le_bytes());
        bytes
    }

    fn block(outcome: BuildOutcome) -> BasicBlock {
        match outcome {
            BuildOutcome::Block(block) => block,
            BuildOutcome::NoBlock(no_block) => panic!("expected block: {no_block:?}"),
        }
    }

    fn no_block(outcome: BuildOutcome) -> NoBlock {
        match outcome {
            BuildOutcome::NoBlock(no_block) => no_block,
            BuildOutcome::Block(block) => panic!("expected no block: {block:?}"),
        }
    }

    #[test]
    fn entry_inside_buffer_starts_a_new_decode_boundary() {
        let mut bytes = vec![0x90];
        bytes.extend(mov_rax_imm(7));
        let block = block(build(&bytes, 0x2000, 0x2001, BlockLimits::default()).unwrap());
        assert_eq!(block.entry_rip, 0x2001);
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(block.instructions[0].guest_rip, 0x2001);
        assert_eq!(block.instructions[0].next_rip, 0x200b);
    }

    #[test]
    fn entry_at_code_end_is_valid_but_has_no_block() {
        let bytes = mov_rax_imm(1);
        let no_block = no_block(build(&bytes, 0x2000, 0x200a, BlockLimits::default()).unwrap());
        assert_eq!(no_block.stop_reason, BlockStopReason::BufferEnd);
    }

    #[test]
    fn instruction_and_byte_limits_stop_before_including_partial_instruction() {
        let mut bytes = vec![];
        bytes.extend(mov_rax_imm(1));
        bytes.extend(mov_rax_imm(2));
        let instruction_limited = block(
            build(
                &bytes,
                0x3000,
                0x3000,
                BlockLimits {
                    instructions: 1,
                    bytes: 20,
                },
            )
            .unwrap(),
        );
        assert_eq!(instruction_limited.instructions.len(), 1);
        assert_eq!(
            instruction_limited.stop_reason,
            BlockStopReason::InstructionLimit
        );

        let byte_limited = block(
            build(
                &bytes,
                0x3000,
                0x3000,
                BlockLimits {
                    instructions: 4,
                    bytes: 10,
                },
            )
            .unwrap(),
        );
        assert_eq!(byte_limited.instructions.len(), 1);
        assert_eq!(byte_limited.stop_reason, BlockStopReason::ByteLimit);

        let no_block = no_block(
            build(
                &bytes,
                0x3000,
                0x3000,
                BlockLimits {
                    instructions: 4,
                    bytes: 9,
                },
            )
            .unwrap(),
        );
        assert_eq!(no_block.stop_reason, BlockStopReason::ByteLimit);
    }

    #[test]
    fn truncation_is_deferred_at_first_instruction_and_after_valid_prefix() {
        let no_block = no_block(build(&[0x48], 0x4000, 0x4000, BlockLimits::default()).unwrap());
        let BlockStopReason::DecodeBoundary(boundary) = no_block.stop_reason else {
            panic!("expected deferred decode boundary");
        };
        assert_eq!(boundary.guest_rip, 0x4000);
        assert_eq!(boundary.consumed_prefix_length, Some(1));
        assert!(matches!(
            boundary.error,
            DecodeError::Memory(MemoryError::Unmapped {
                address: 0x4001,
                ..
            })
        ));

        let mut bytes = mov_rax_imm(3).to_vec();
        bytes.push(0x48);
        let block = block(build(&bytes, 0x4000, 0x4000, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 1);
        let BlockStopReason::DecodeBoundary(boundary) = block.stop_reason else {
            panic!("expected deferred decode boundary");
        };
        assert_eq!(boundary.guest_rip, 0x400a);
        assert_eq!(boundary.consumed_prefix_length, Some(1));
    }

    #[test]
    fn unsupported_instruction_stops_after_the_valid_prefix() {
        let mut bytes = mov_rax_imm(3).to_vec();
        bytes.push(0x90);
        let block = block(build(&bytes, 0x5000, 0x5000, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 1);
        assert!(matches!(
            block.stop_reason,
            BlockStopReason::UnsupportedDecoded {
                guest_rip: 0x500a,
                ..
            }
        ));
    }

    #[test]
    fn branch_terminates_without_decoding_unreachable_tail() {
        let mut bytes = mov_rax_imm(3).to_vec();
        bytes.extend([0x74, 0xf4]); // Back to the decoded MOV at the entry.
        bytes.extend([0x90, 0x90]);
        let block = block(build(&bytes, 0x6000, 0x6000, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 2);
        assert_eq!(
            block.guest_ranges[1],
            GuestByteRange {
                start_rip: 0x600a,
                end_rip: 0x600c
            }
        );
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0x6000,
                    resolution: BranchTargetResolution::KnownInstructionStart,
                }
            }
        );
    }

    #[test]
    fn branch_at_buffer_end_is_valid_without_unreachable_bytes() {
        let mut bytes = mov_rax_imm(3).to_vec();
        bytes.extend([0x75, 0xf4]); // Back to the decoded MOV at the entry.
        let block = block(build(&bytes, 0x6100, 0x6100, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 2);
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0x6100,
                    resolution: BranchTargetResolution::KnownInstructionStart,
                }
            }
        );
    }

    #[test]
    fn zero_displacement_is_a_known_next_rip_boundary_without_decoding_tail() {
        let block =
            block(build(&[0x74, 0x00, 0xff], 0x6200, 0x6200, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(block.instructions[0].next_rip, 0x6202);
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0x6202,
                    resolution: BranchTargetResolution::NextRipBoundary,
                }
            }
        );
    }

    #[test]
    fn zero_displacement_at_code_end_is_classified_as_code_end() {
        let block = block(build(&[0x74, 0x00], 0x6300, 0x6300, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0x6302,
                    resolution: BranchTargetResolution::CodeEnd,
                }
            }
        );
    }

    #[test]
    fn stops_before_mid_instruction_target_and_marks_forward_target_unresolved() {
        let mut mid_instruction = mov_rax_imm(3).to_vec();
        mid_instruction.extend([0x74, 0xf5]); // Target is byte 1 of MOV.
        let prefix =
            block(build(&mid_instruction, 0x7000, 0x7000, BlockLimits::default()).unwrap());
        assert_eq!(prefix.instructions.len(), 1);
        assert_eq!(
            prefix.stop_reason,
            BlockStopReason::UnsupportedBranchTarget {
                branch_rip: 0x700a,
                target_rip: 0x7001,
            }
        );

        let no_block =
            no_block(build(&[0x74, 0xff], 0x7100, 0x7100, BlockLimits::default()).unwrap());
        assert_eq!(
            no_block.stop_reason,
            BlockStopReason::UnsupportedBranchTarget {
                branch_rip: 0x7100,
                target_rip: 0x7101,
            }
        );

        let bytes = [0x74, 0x02, 0x90, 0x90, 0x90];
        let block = block(build(&bytes, 0x8000, 0x8000, BlockLimits::default()).unwrap());
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0x8004,
                    resolution: BranchTargetResolution::Unresolved,
                }
            }
        );
    }

    #[test]
    fn branch_target_wraps_at_u64_max_and_stays_unresolved_outside_view() {
        let block = block(
            build(
                &[0x74, 1],
                u64::MAX - 2,
                u64::MAX - 2,
                BlockLimits::default(),
            )
            .unwrap(),
        );
        assert_eq!(block.instructions.len(), 1);
        assert_eq!(
            block.stop_reason,
            BlockStopReason::Branch {
                target: BranchTarget {
                    rip: 0,
                    resolution: BranchTargetResolution::Unresolved,
                }
            }
        );
    }

    #[test]
    fn exact_ranges_preserve_page_crossing_and_reject_extra_prefixes() {
        let bytes = mov_rax_imm(9);
        let page_block = block(build(&bytes, 0xffc, 0xffc, BlockLimits::default()).unwrap());
        assert_eq!(
            page_block.guest_ranges,
            vec![GuestByteRange {
                start_rip: 0xffc,
                end_rip: 0x1006,
            }]
        );

        let bytes = [0x48, 0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let prefixed_block =
            no_block(build(&bytes, 0x9000, 0x9000, BlockLimits::default()).unwrap());
        assert!(matches!(
            prefixed_block.stop_reason,
            BlockStopReason::UnsupportedDecoded {
                guest_rip: 0x9000,
                ..
            }
        ));
    }

    #[test]
    fn rejects_overflowing_code_ranges_bad_entries_and_bad_limits() {
        assert!(matches!(
            CodeView::new(u64::MAX, &[0, 0]),
            Err(BuildError::AddressOverflow { .. })
        ));
        let view = CodeView::new(0xa000, &[0x90]).unwrap();
        assert!(matches!(
            build_block(
                view,
                0x9fff,
                AccessContext::default(),
                BlockLimits::default()
            ),
            Err(BuildError::EntryOutsideCode { .. })
        ));
        for limits in [
            BlockLimits {
                instructions: 0,
                bytes: 1,
            },
            BlockLimits {
                instructions: MAX_BLOCK_INSTRUCTIONS + 1,
                bytes: 1,
            },
            BlockLimits {
                instructions: 1,
                bytes: MAX_BLOCK_BYTES + 1,
            },
        ] {
            assert!(matches!(
                build_block(view, 0xa000, AccessContext::default(), limits),
                Err(BuildError::InvalidLimits { .. })
            ));
        }
    }
}

pub(crate) fn is_s0_09_form(instruction: Instruction) -> bool {
    let prefixes = instruction.prefixes;
    let no_legacy_prefixes = prefixes.group1.is_none()
        && prefixes.segment.is_none()
        && !prefixes.operand_override
        && !prefixes.address_override;
    let exact_rex_w = no_legacy_prefixes && prefixes.rex == Some(0x48);
    let no_prefix = prefixes == Default::default();

    match instruction.operation {
        Operation::MovImmediate { dst, .. } => {
            let Operand::Register(dst) = dst else {
                return false;
            };
            instruction.opcode.map == OpcodeMap::Primary
                && (0xb8..=0xbb).contains(&instruction.opcode.byte)
                && exact_rex_w
                && instruction.operand_size == OperandSize::Qword
                && instruction.address_size == OperandSize::Qword
                && instruction.length == 10
                && (dst.register() as u8) < 4
                && dst.size() == OperandSize::Qword
        }
        Operation::Arithmetic { kind, dst, src } => {
            let opcode_kind = match (instruction.opcode.map, instruction.opcode.byte) {
                (OpcodeMap::Primary, 0x03) => Some(ArithmeticKind::Add),
                (OpcodeMap::Primary, 0x2b) => Some(ArithmeticKind::Sub),
                (OpcodeMap::Primary, 0x3b) => Some(ArithmeticKind::Cmp),
                _ => None,
            };
            let register_operand = |operand| match operand {
                Operand::Register(register) => {
                    (register.register() as u8) < 4 && register.size() == OperandSize::Qword
                }
                _ => false,
            };
            opcode_kind == Some(kind)
                && exact_rex_w
                && instruction.operand_size == OperandSize::Qword
                && instruction.address_size == OperandSize::Qword
                && instruction.length == 3
                && register_operand(dst)
                && register_operand(src)
        }
        Operation::Branch { condition, .. } => {
            let exact_opcode = matches!(
                (instruction.opcode.map, instruction.opcode.byte, condition),
                (OpcodeMap::Primary, 0x74, ConditionCode::Equal)
                    | (OpcodeMap::Primary, 0x75, ConditionCode::NotEqual)
            );
            exact_opcode
                && no_prefix
                && instruction.operand_size == OperandSize::Qword
                && instruction.address_size == OperandSize::Qword
                && instruction.length == 2
        }
        _ => false,
    }
}

enum TargetClassification {
    Resolved(BranchTargetResolution),
    InsideDecodedInstruction,
}

fn classify_target(
    target_rip: u64,
    code_end: u64,
    instructions: &[DecodedInstruction],
    candidate: DecodedInstruction,
) -> TargetClassification {
    if target_rip == code_end {
        return TargetClassification::Resolved(BranchTargetResolution::CodeEnd);
    }
    if target_rip == candidate.guest_rip
        || instructions
            .iter()
            .any(|instruction| instruction.guest_rip == target_rip)
    {
        return TargetClassification::Resolved(BranchTargetResolution::KnownInstructionStart);
    }
    if target_rip == candidate.next_rip {
        return TargetClassification::Resolved(BranchTargetResolution::NextRipBoundary);
    }
    if (target_rip > candidate.guest_rip && target_rip < candidate.next_rip)
        || instructions.iter().any(|instruction| {
            target_rip > instruction.guest_rip && target_rip < instruction.next_rip
        })
    {
        return TargetClassification::InsideDecodedInstruction;
    }
    TargetClassification::Resolved(BranchTargetResolution::Unresolved)
}

fn deferred_boundary(guest_rip: u64, error: DecodeError) -> DecodeBoundary {
    let consumed_prefix_length = match &error {
        DecodeError::Memory(MemoryError::Unmapped { address, .. }) => address
            .checked_sub(guest_rip)
            .and_then(|length| u8::try_from(length).ok()),
        DecodeError::Unsupported { length, .. } | DecodeError::Invalid { length, .. } => {
            Some(*length)
        }
        DecodeError::Memory(_) => None,
    };
    DecodeBoundary {
        guest_rip,
        error,
        consumed_prefix_length,
    }
}

fn is_byte_limit_fetch(error: &DecodeError, read_end_rip: u64, code_end: u64) -> bool {
    read_end_rip < code_end
        && matches!(
            error,
            DecodeError::Memory(MemoryError::Unmapped {
                address,
                access: AccessKind::Fetch,
            }) if *address == read_end_rip
        )
}

struct CodeMemory<'a> {
    code: CodeView<'a>,
    read_end_rip: u64,
}

impl CodeMemory<'_> {
    fn unavailable(address: u64, access: AccessKind) -> MemoryError {
        MemoryError::Unmapped { address, access }
    }

    fn unsupported_data_access() -> MemoryError {
        MemoryError::Backend("block CodeView supports instruction fetch only".into())
    }
}

impl GuestMemory for CodeMemory<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        if address < self.code.base_rip || address >= self.read_end_rip {
            return Err(Self::unavailable(address, AccessKind::Fetch));
        }
        let offset = self
            .code
            .offset(address)
            .ok_or_else(|| Self::unavailable(address, AccessKind::Fetch))?;
        self.code
            .bytes
            .get(offset)
            .copied()
            .ok_or_else(|| Self::unavailable(address, AccessKind::Fetch))
    }

    fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
        Err(Self::unsupported_data_access())
    }

    fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
        Err(Self::unsupported_data_access())
    }

    fn atomic_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported_data_access())
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported_data_access())
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _: u64,
        _: AccessContext,
        _: OperandSize,
        _: u64,
        _: u64,
    ) -> Result<u64, MemoryError> {
        Err(Self::unsupported_data_access())
    }

    fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
        Ok(())
    }
}
