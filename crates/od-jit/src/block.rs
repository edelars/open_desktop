//! Bounded, read-only construction of one guest basic block.
//!
//! The builder uses the long-mode decoder one instruction at a time. It does
//! not execute instructions or translate them to IR/native code.

use od_isa::OperandSize;
use od_isa::long_decode::{
    self, AddressBase, ArithmeticKind, ConditionCode, DecodeError, Instruction, LogicKind,
    OpcodeMap, Operand, Operation,
};
use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

pub const MAX_BLOCK_INSTRUCTIONS: usize = 32;
pub const MAX_BLOCK_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BlockBuildProfile {
    S0_09,
    S0_09NativeIncRax,
    S0_09NativeDecRax,
    S0_09NativeNegRax,
    S0_09NativeNotRax,
    S0_09NativeLeaRaxRbx,
    S0_09NativeLeaRaxRbxDisp8,
    S0_09NativeLeaRaxRbxRcx,
    S0_09NativeLeaRaxRipDisp32,
    S0_09NativeXchgRaxRbx,
    S0_09NativeAddRaxImm8,
    S0_09NativeCmpRaxImm8,
    S0_09NativeSubRaxImm8,
    S0_09NativeMovRaxImm32,
    S0_09NativeRegisterV1,
    CheckedQwordLoad,
    CheckedDwordLoad,
    CheckedDwordStore,
    CheckedQwordStore,
    CheckedQwordMemoryPair,
}

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

/// Full-code admission for S0-09 plus exact bare `0x90` NOP fallback.
/// Unlike fixture-v1 admission, this is a separate opt-in profile witness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus bare NOP and exact `INC RAX`.
/// The NOP-only admission above intentionally remains strict.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopIncRaxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    inc_rax_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus bare NOP and exact `DEC RAX`.
/// The INC admission above remains limited to INC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopDecRaxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    dec_rax_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus bare NOP and exact `NEG RAX`.
/// Neither INC-only nor DEC-only admission is widened by this profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopNegRaxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    neg_rax_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus bare NOP and exact `NOT RAX`.
/// The NEG admission above remains limited to NEG.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopNotRaxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    not_rax_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus exact bare `LEA RAX,[RBX]`.
/// Existing instruction-specific admissions remain unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopLeaRaxRbxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    lea_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus exact bare `LEA RAX,[RBX+disp8]`.
/// The no-displacement LEA admission above remains unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopLeaRaxRbxDisp8Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    lea_rips: Vec<(u64, i64)>,
}

/// Separate bounded admission for S0-09 plus exact bare `LEA RAX,[RBX+RCX]`.
/// Earlier RAX/RBX LEA profiles remain unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopLeaRaxRbxRcxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    lea_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus exact bare
/// `LEA RAX,[RIP+disp32]`; each displacement is bound to its instruction RIP.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopLeaRaxRipDisp32Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    lea_displacements: Vec<(u64, i64)>,
}

/// Separate bounded admission for S0-09 plus exact bare `XCHG RAX,RBX`.
/// Existing instruction-specific admissions remain unchanged.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopXchgRaxRbxAdmission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    xchg_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus exact bare `ADD RAX, sign-extended imm8`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopAddRaxImm8Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    add_rax_imm8_values: Vec<(u64, u64)>,
}

/// Exact, uncached S3-32a experiment admission for bare `48 03 C0` only.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S3_32aAddRaxRaxAdmission {
    boundaries: S0_09NopAdmission,
    add_rax_rax_rips: Vec<u64>,
}

/// Separate bounded admission for S0-09 plus exact bare `CMP RAX, sign-extended imm8`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopCmpRaxImm8Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    cmp_rax_imm8_values: Vec<(u64, u64)>,
}

/// Separate bounded admission for S0-09 plus exact bare `SUB RAX, sign-extended imm8`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopSubRaxImm8Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    sub_rax_imm8_values: Vec<(u64, u64)>,
}

/// Separate bounded admission for S0-09 plus exact bare `MOV RAX, sign-extended imm32`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct S0_09NopMovRaxImm32Admission {
    boundaries: AdmittedInstructionBoundaries,
    nop_rips: Vec<u64>,
    mov_rax_imm32_values: Vec<(u64, u64)>,
}

/// Opt-in union of the committed exact native register forms in S3-33a–m,
/// the S0-09 baseline, and the exact bare-NOP interpreter handoff.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeRegisterV1Admission {
    boundaries: AdmittedInstructionBoundaries,
    instructions: Vec<NativeRegisterV1AdmittedInstruction>,
    nop_rips: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NativeRegisterV1Instruction {
    IncRax,
    DecRax,
    NegRax,
    NotRax,
    AddRaxImm8(u64),
    CmpRaxImm8(u64),
    SubRaxImm8(u64),
    MovRaxImm32(u64),
    XchgRaxRbx,
    LeaRaxRbx,
    LeaRaxRbxDisp8(i64),
    LeaRaxRbxRcx,
    LeaRaxRipDisp32(i64),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct NativeRegisterV1AdmittedInstruction {
    rip: u64,
    next_rip: u64,
    decoded: Instruction,
    source_bytes: Vec<u8>,
    native: Option<NativeRegisterV1Instruction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum S0_09NopAdmissionError {
    Decode {
        rip: u64,
        error: long_decode::DecodeError,
    },
    InvalidLength {
        rip: u64,
        length: u8,
    },
    RangeOverflow {
        rip: u64,
        length: u8,
    },
    UnsupportedInstruction {
        rip: u64,
    },
    BranchTargetNotBoundary {
        rip: u64,
        target: u64,
    },
}

impl S0_09NopAdmission {
    /// Decode the complete immutable code view and admit only S0-09 forms plus
    /// unprefixed one-byte NOP. Every branch target must be a decoded start or
    /// the exclusive code end.
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();

        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());

        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }

        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }

    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }

    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }

    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
}

impl S3_32aAddRaxRaxAdmission {
    /// Admit an immutable S0-09 code view only when it contains the exact
    /// bare `48 03 C0` form at a decoded instruction boundary.
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let boundaries = S0_09NopAdmission::from_code(code)?;
        let mut add_rax_rax_rips = Vec::new();
        let mut rip = code.base_rip();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if is_exact_add_rax_rax(instruction, &code.bytes()[offset..]) {
                add_rax_rax_rips.push(rip);
            }
            rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
        }
        if add_rax_rax_rips.is_empty() {
            return Err(S0_09NopAdmissionError::UnsupportedInstruction {
                rip: code.base_rip(),
            });
        }
        Ok(Self {
            boundaries,
            add_rax_rax_rips,
        })
    }

    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }

    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }

    pub(crate) fn admits_add_rax_rax(&self, rip: u64) -> bool {
        self.add_rax_rax_rips.contains(&rip)
    }
}

impl S0_09NopIncRaxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 FF C0` (`INC RAX`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut inc_rax_rips = Vec::new();

        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_inc_rax(instruction, &code.bytes()[offset..]) {
                inc_rax_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            inc_rax_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_inc_rax(&self, rip: u64) -> bool {
        self.inc_rax_rips.contains(&rip)
    }
}

impl S0_09NopDecRaxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 FF C8` (`DEC RAX`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut dec_rax_rips = Vec::new();

        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_dec_rax(instruction, &code.bytes()[offset..]) {
                dec_rax_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            dec_rax_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_dec_rax(&self, rip: u64) -> bool {
        self.dec_rax_rips.contains(&rip)
    }
}

impl S0_09NopNegRaxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 F7 D8` (`NEG RAX`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut neg_rax_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_neg_rax(instruction, &code.bytes()[offset..]) {
                neg_rax_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            neg_rax_rips,
        })
    }
    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_neg_rax(&self, rip: u64) -> bool {
        self.neg_rax_rips.contains(&rip)
    }
}

impl S0_09NopNotRaxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 F7 D0` (`NOT RAX`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut not_rax_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_not_rax(instruction, &code.bytes()[offset..]) {
                not_rax_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            not_rax_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_not_rax(&self, rip: u64) -> bool {
        self.not_rax_rips.contains(&rip)
    }
}

impl S0_09NopLeaRaxRbxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 8D 03` (`LEA RAX,[RBX]`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut lea_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_lea_rax_rbx(instruction, &code.bytes()[offset..]) {
                lea_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            lea_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_lea_rax_rbx(&self, rip: u64) -> bool {
        self.lea_rips.contains(&rip)
    }
}

impl S0_09NopLeaRaxRbxDisp8Admission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 8D 43 db` (`LEA RAX,[RBX+sign-extended disp8]`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut lea_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_lea_rax_rbx_disp8(instruction, &code.bytes()[offset..]) {
                let Operation::Lea { src, .. } = instruction.operation else {
                    unreachable!("exact LEA predicate checked the decoded operation");
                };
                lea_rips.push((rip, src.displacement));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            lea_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_lea_rax_rbx_disp8(&self, rip: u64) -> bool {
        self.lea_rbx_disp8_value(rip).is_some()
    }
    pub(crate) fn lea_rbx_disp8_value(&self, rip: u64) -> Option<i64> {
        self.lea_rips
            .iter()
            .find_map(|(admitted_rip, displacement)| {
                (*admitted_rip == rip).then_some(*displacement)
            })
    }
}

impl S0_09NopLeaRaxRbxRcxAdmission {
    /// Decode the immutable view, admitting only S0-09, exact bare NOP, and
    /// exact `48 8D 04 0B` (`LEA RAX,[RBX+RCX]`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut lea_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_lea_rax_rbx_rcx(instruction, &code.bytes()[offset..]) {
                lea_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            lea_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_lea_rax_rbx_rcx(&self, rip: u64) -> bool {
        self.lea_rips.contains(&rip)
    }
}

impl S0_09NopLeaRaxRipDisp32Admission {
    /// Decode the immutable view, admitting only S0-09, exact bare NOP, and
    /// exact `48 8D 05 dd dd dd dd` (`LEA RAX,[RIP+disp32]`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut lea_displacements = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_lea_rax_rip_disp32(instruction, &code.bytes()[offset..]) {
                let Operation::Lea { src, .. } = instruction.operation else {
                    unreachable!("exact RIP-relative LEA predicate checked the operation")
                };
                lea_displacements.push((rip, src.displacement));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            lea_displacements,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn displacement(&self, rip: u64) -> Option<i64> {
        self.lea_displacements
            .iter()
            .find_map(|(admitted_rip, displacement)| {
                (*admitted_rip == rip).then_some(*displacement)
            })
    }
    pub(crate) fn admits_lea_rax_rip_disp32(&self, rip: u64) -> bool {
        self.displacement(rip).is_some()
    }
}

impl S0_09NopXchgRaxRbxAdmission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 87 D8` (`XCHG RAX,RBX`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut xchg_rips = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_xchg_rax_rbx(instruction, &code.bytes()[offset..]) {
                xchg_rips.push(rip);
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            xchg_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_xchg_rax_rbx(&self, rip: u64) -> bool {
        self.xchg_rips.contains(&rip)
    }
}

impl S0_09NopAddRaxImm8Admission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 83 C0 ib` (`ADD RAX, sign-extended imm8`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut add_rax_imm8_values = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_add_rax_imm8(instruction, &code.bytes()[offset..]) {
                let Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } = instruction.operation
                else {
                    unreachable!("exact ADD immediate predicate guarantees its operation");
                };
                add_rax_imm8_values.push((rip, value));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            add_rax_imm8_values,
        })
    }
    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_add_rax_imm8(&self, rip: u64) -> bool {
        self.add_rax_imm8_values
            .iter()
            .any(|(admitted_rip, _)| *admitted_rip == rip)
    }
    pub(crate) fn add_rax_imm8_value(&self, rip: u64) -> Option<u64> {
        self.add_rax_imm8_values
            .iter()
            .find_map(|(admitted_rip, value)| (*admitted_rip == rip).then_some(*value))
    }
}

impl S0_09NopCmpRaxImm8Admission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 83 F8 ib` (`CMP RAX, sign-extended imm8`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut cmp_rax_imm8_values = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_cmp_rax_imm8(instruction, &code.bytes()[offset..]) {
                let Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } = instruction.operation
                else {
                    unreachable!("exact CMP immediate predicate guarantees its operation");
                };
                cmp_rax_imm8_values.push((rip, value));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            cmp_rax_imm8_values,
        })
    }
    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_cmp_rax_imm8(&self, rip: u64) -> bool {
        self.cmp_rax_imm8_values
            .iter()
            .any(|(admitted_rip, _)| *admitted_rip == rip)
    }
    pub(crate) fn cmp_rax_imm8_value(&self, rip: u64) -> Option<u64> {
        self.cmp_rax_imm8_values
            .iter()
            .find_map(|(admitted_rip, value)| (*admitted_rip == rip).then_some(*value))
    }
}

impl S0_09NopSubRaxImm8Admission {
    /// Decode the full immutable view, admitting only S0-09, exact bare NOP,
    /// and exact `48 83 E8 ib` (`SUB RAX, sign-extended imm8`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut sub_rax_imm8_values = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_sub_rax_imm8(instruction, &code.bytes()[offset..]) {
                let Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } = instruction.operation
                else {
                    unreachable!("exact SUB immediate predicate guarantees its operation");
                };
                sub_rax_imm8_values.push((rip, value));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            sub_rax_imm8_values,
        })
    }
    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }
    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }
    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }
    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }
    pub(crate) fn admits_sub_rax_imm8(&self, rip: u64) -> bool {
        self.sub_rax_imm8_values
            .iter()
            .any(|(admitted_rip, _)| *admitted_rip == rip)
    }
    pub(crate) fn sub_rax_imm8_value(&self, rip: u64) -> Option<u64> {
        self.sub_rax_imm8_values
            .iter()
            .find_map(|(admitted_rip, value)| (*admitted_rip == rip).then_some(*value))
    }
}

impl S0_09NopMovRaxImm32Admission {
    /// Decode the full immutable view, admitting S0-09, bare NOP, and exact
    /// `48 C7 C0 id` (`MOV RAX, sign-extended imm32`).
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();
        let mut mov_rax_imm32_values = Vec::new();
        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let instruction = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&instruction.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: instruction.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(instruction.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: instruction.length,
                });
            }
            if is_exact_bare_nop(instruction, code.bytes()[offset]) {
                nop_rips.push(rip);
            } else if is_exact_mov_rax_imm32(instruction, &code.bytes()[offset..]) {
                let Operation::MovImmediate { value, .. } = instruction.operation else {
                    unreachable!("exact MOV immediate predicate guarantees its operation");
                };
                mov_rax_imm32_values.push((rip, value));
            } else if !is_s0_09_form(instruction) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            boundaries.push(rip);
            instructions.push((rip, next_rip, instruction));
            rip = next_rip;
        }
        boundaries.push(code.code_end());
        for (branch_rip, next_rip, instruction) in &instructions {
            let Operation::Branch { displacement, .. } = instruction.operation else {
                continue;
            };
            let target = next_rip.wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: *branch_rip,
                    target,
                });
            }
        }
        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            nop_rips,
            mov_rax_imm32_values,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }

    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }

    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }

    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }

    pub(crate) fn admits_mov_rax_imm32(&self, rip: u64) -> bool {
        self.mov_rax_imm32_values
            .iter()
            .any(|(admitted_rip, _)| *admitted_rip == rip)
    }

    pub(crate) fn mov_rax_imm32_value(&self, rip: u64) -> Option<u64> {
        self.mov_rax_imm32_values
            .iter()
            .find_map(|(admitted_rip, value)| (*admitted_rip == rip).then_some(*value))
    }
}

impl NativeRegisterV1Admission {
    /// Admit one complete immutable view in a single decoder pass. The stored
    /// records bind each boundary to its decoder result, exact source bytes,
    /// and (for the added native forms) the already-decoded operand value.
    pub fn from_code(code: CodeView<'_>) -> Result<Self, S0_09NopAdmissionError> {
        let mut rip = code.base_rip();
        let mut boundaries = Vec::new();
        let mut instructions = Vec::new();
        let mut nop_rips = Vec::new();

        while rip < code.code_end() {
            let offset = usize::try_from(rip - code.base_rip())
                .map_err(|_| S0_09NopAdmissionError::RangeOverflow { rip, length: 0 })?;
            let mut memory = CodeMemory::full_code(code);
            let decoded = long_decode::decode(&mut memory, rip, AccessContext::default())
                .map_err(|error| S0_09NopAdmissionError::Decode { rip, error })?;
            if !(1..=15).contains(&decoded.length) {
                return Err(S0_09NopAdmissionError::InvalidLength {
                    rip,
                    length: decoded.length,
                });
            }
            let next_rip = rip.checked_add(u64::from(decoded.length)).ok_or(
                S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: decoded.length,
                },
            )?;
            if next_rip > code.code_end() {
                return Err(S0_09NopAdmissionError::RangeOverflow {
                    rip,
                    length: decoded.length,
                });
            }
            let source_bytes = code.bytes()[offset..offset + usize::from(decoded.length)].to_vec();
            let native = native_register_v1_instruction(decoded, &source_bytes);
            let nop = is_exact_bare_nop(decoded, source_bytes[0]);
            if !nop && native.is_none() && !is_s0_09_form(decoded) {
                return Err(S0_09NopAdmissionError::UnsupportedInstruction { rip });
            }
            if nop {
                nop_rips.push(rip);
            }
            boundaries.push(rip);
            instructions.push(NativeRegisterV1AdmittedInstruction {
                rip,
                next_rip,
                decoded,
                source_bytes,
                native,
            });
            rip = next_rip;
        }
        boundaries.push(code.code_end());

        for instruction in &instructions {
            let Operation::Branch { displacement, .. } = instruction.decoded.operation else {
                continue;
            };
            let target = instruction
                .next_rip
                .wrapping_add_signed(i64::from(displacement));
            if !boundaries.contains(&target) {
                return Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                    rip: instruction.rip,
                    target,
                });
            }
        }

        Ok(Self {
            boundaries: AdmittedInstructionBoundaries {
                code_base: code.base_rip(),
                code_end: code.code_end(),
                code_bytes: code.bytes().to_vec(),
                boundaries,
            },
            instructions,
            nop_rips,
        })
    }

    pub(crate) fn boundaries(&self) -> &AdmittedInstructionBoundaries {
        &self.boundaries
    }

    pub(crate) fn matches_code(&self, code: CodeView<'_>) -> bool {
        self.boundaries.matches_code(code)
    }

    pub(crate) fn contains(&self, rip: u64) -> bool {
        self.boundaries.contains(rip)
    }

    pub(crate) fn admits_nop(&self, rip: u64) -> bool {
        self.nop_rips.contains(&rip)
    }

    pub(crate) fn instruction(&self, rip: u64) -> Option<NativeRegisterV1Instruction> {
        self.instructions
            .iter()
            .find(|instruction| instruction.rip == rip)
            .and_then(|instruction| {
                let _captured_source = &instruction.source_bytes;
                Some(instruction.native?)
            })
    }

    pub(crate) fn validates_decoded(&self, rip: u64, decoded: Instruction) -> bool {
        self.instructions
            .iter()
            .find(|instruction| instruction.rip == rip)
            .is_some_and(|instruction| instruction.decoded == decoded)
    }
}

fn native_register_v1_instruction(
    decoded: Instruction,
    bytes: &[u8],
) -> Option<NativeRegisterV1Instruction> {
    if is_exact_inc_rax(decoded, bytes) {
        Some(NativeRegisterV1Instruction::IncRax)
    } else if is_exact_dec_rax(decoded, bytes) {
        Some(NativeRegisterV1Instruction::DecRax)
    } else if is_exact_neg_rax(decoded, bytes) {
        Some(NativeRegisterV1Instruction::NegRax)
    } else if is_exact_not_rax(decoded, bytes) {
        Some(NativeRegisterV1Instruction::NotRax)
    } else if is_exact_add_rax_imm8(decoded, bytes) {
        let Operation::Arithmetic {
            src: Operand::Immediate(value),
            ..
        } = decoded.operation
        else {
            return None;
        };
        Some(NativeRegisterV1Instruction::AddRaxImm8(value))
    } else if is_exact_cmp_rax_imm8(decoded, bytes) {
        let Operation::Arithmetic {
            src: Operand::Immediate(value),
            ..
        } = decoded.operation
        else {
            return None;
        };
        Some(NativeRegisterV1Instruction::CmpRaxImm8(value))
    } else if is_exact_sub_rax_imm8(decoded, bytes) {
        let Operation::Arithmetic {
            src: Operand::Immediate(value),
            ..
        } = decoded.operation
        else {
            return None;
        };
        Some(NativeRegisterV1Instruction::SubRaxImm8(value))
    } else if is_exact_mov_rax_imm32(decoded, bytes) {
        let Operation::MovImmediate { value, .. } = decoded.operation else {
            return None;
        };
        Some(NativeRegisterV1Instruction::MovRaxImm32(value))
    } else if is_exact_xchg_rax_rbx(decoded, bytes) {
        Some(NativeRegisterV1Instruction::XchgRaxRbx)
    } else if is_exact_lea_rax_rbx(decoded, bytes) {
        Some(NativeRegisterV1Instruction::LeaRaxRbx)
    } else if is_exact_lea_rax_rbx_disp8(decoded, bytes) {
        let Operation::Lea { src, .. } = decoded.operation else {
            return None;
        };
        Some(NativeRegisterV1Instruction::LeaRaxRbxDisp8(
            src.displacement,
        ))
    } else if is_exact_lea_rax_rbx_rcx(decoded, bytes) {
        Some(NativeRegisterV1Instruction::LeaRaxRbxRcx)
    } else if is_exact_lea_rax_rip_disp32(decoded, bytes) {
        let Operation::Lea { src, .. } = decoded.operation else {
            return None;
        };
        Some(NativeRegisterV1Instruction::LeaRaxRipDisp32(
            src.displacement,
        ))
    } else {
        None
    }
}

pub(crate) fn is_exact_mov_rax_imm32(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::MovImmediate { dst: Operand::Register(dst), .. }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0xc7
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 7
        && bytes.get(..3) == Some(&[0x48, 0xc7, 0xc0])
        && bytes.get(3..7).is_some_and(|immediate| {
            let raw = u32::from_le_bytes([immediate[0], immediate[1], immediate[2], immediate[3]]);
            let decoded = match instruction.operation {
                Operation::MovImmediate { value, .. } => value,
                _ => return false,
            };
            decoded == (raw as i32 as i64 as u64)
        })
}

pub(crate) fn is_exact_cmp_rax_imm8(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Cmp, dst: Operand::Register(dst), src: Operand::Immediate(_) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x83
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 4
        && bytes.get(..3) == Some(&[0x48, 0x83, 0xf8])
        && bytes.get(3).is_some_and(|immediate| {
            let decoded = match instruction.operation {
                Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } => value,
                _ => return false,
            };
            decoded == (*immediate as i8 as i64) as u64
        })
}

pub(crate) fn is_exact_add_rax_imm8(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Add, dst: Operand::Register(dst), src: Operand::Immediate(_) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x83
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 4
        && bytes.get(..3) == Some(&[0x48, 0x83, 0xc0])
        && bytes.get(3).is_some_and(|immediate| {
            let decoded = match instruction.operation {
                Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } => value,
                _ => return false,
            };
            decoded == (*immediate as i8 as i64) as u64
        })
}

fn is_exact_add_rax_rax(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(
        instruction.operation,
        Operation::Arithmetic {
            kind: ArithmeticKind::Add,
            dst: Operand::Register(dst),
            src: Operand::Register(src),
        } if dst.register() == od_isa::Register::Rax
            && dst.size() == OperandSize::Qword
            && src.register() == od_isa::Register::Rax
            && src.size() == OperandSize::Qword
    ) && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x03
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0x03, 0xc0])
}

pub(crate) fn is_exact_sub_rax_imm8(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Sub, dst: Operand::Register(dst), src: Operand::Immediate(_) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x83
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 4
        && bytes.get(..3) == Some(&[0x48, 0x83, 0xe8])
        && bytes.get(3).is_some_and(|immediate| {
            let decoded = match instruction.operation {
                Operation::Arithmetic {
                    src: Operand::Immediate(value),
                    ..
                } => value,
                _ => return false,
            };
            decoded == (*immediate as i8 as i64) as u64
        })
}

pub(crate) fn is_exact_neg_rax(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Neg, dst: Operand::Register(dst), src: Operand::Immediate(0) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0xf7
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0xf7, 0xd8])
}

pub(crate) fn is_exact_not_rax(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Logic { kind: LogicKind::Not, dst: Operand::Register(dst), src: Operand::Immediate(0) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0xf7
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0xf7, 0xd0])
}

pub(crate) fn is_exact_lea_rax_rbx(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Lea { dst, src }
        if dst.register() == od_isa::Register::Rax
            && dst.size() == OperandSize::Qword
            && src.base == AddressBase::Register(od_isa::Register::Rbx)
            && src.index.is_none()
            && src.scale == 1
            && src.displacement == 0
            && src.address_size == OperandSize::Qword
            && src.size == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8d
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0x8d, 0x03])
}

pub(crate) fn is_exact_lea_rax_rbx_disp8(instruction: Instruction, bytes: &[u8]) -> bool {
    let Some(&raw_displacement) = bytes.get(3) else {
        return false;
    };
    let displacement = i64::from(raw_displacement as i8);
    matches!(instruction.operation,
        Operation::Lea { dst, src }
        if dst.register() == od_isa::Register::Rax
            && dst.size() == OperandSize::Qword
            && src.base == AddressBase::Register(od_isa::Register::Rbx)
            && src.index.is_none()
            && src.scale == 1
            && src.displacement == displacement
            && src.address_size == OperandSize::Qword
            && src.size == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8d
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 4
        && bytes.get(..3) == Some(&[0x48, 0x8d, 0x43])
}

pub(crate) fn is_exact_lea_rax_rbx_rcx(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Lea { dst, src }
        if dst.register() == od_isa::Register::Rax
            && dst.size() == OperandSize::Qword
            && src.base == AddressBase::Register(od_isa::Register::Rbx)
            && src.index == Some(od_isa::Register::Rcx)
            && src.scale == 1
            && src.displacement == 0
            && src.address_size == OperandSize::Qword
            && src.size == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8d
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 4
        && bytes.get(..4) == Some(&[0x48, 0x8d, 0x04, 0x0b])
}

pub(crate) fn is_exact_lea_rax_rip_disp32(instruction: Instruction, bytes: &[u8]) -> bool {
    let Some(raw_displacement) = bytes.get(3..7) else {
        return false;
    };
    let Ok(raw_displacement) = <[u8; 4]>::try_from(raw_displacement) else {
        return false;
    };
    let displacement = i64::from(i32::from_le_bytes(raw_displacement));
    matches!(instruction.operation,
        Operation::Lea { dst, src }
        if dst.register() == od_isa::Register::Rax
            && dst.size() == OperandSize::Qword
            && src.base == AddressBase::NextRip
            && src.index.is_none()
            && src.scale == 1
            && src.displacement == displacement
            && src.address_size == OperandSize::Qword
            && src.size == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8d
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 7
        && bytes.get(..3) == Some(&[0x48, 0x8d, 0x05])
}

pub(crate) fn is_exact_xchg_rax_rbx(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Xchg { left, right }
        if left.register() == od_isa::Register::Rbx
            && left.size() == OperandSize::Qword
            && right.register() == od_isa::Register::Rax
            && right.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x87
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0x87, 0xd8])
}

pub(crate) fn is_exact_inc_rax(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Inc, dst: Operand::Register(dst), src: Operand::Immediate(1) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0xff
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0xff, 0xc0])
}

pub(crate) fn is_exact_dec_rax(instruction: Instruction, bytes: &[u8]) -> bool {
    matches!(instruction.operation,
        Operation::Arithmetic { kind: ArithmeticKind::Dec, dst: Operand::Register(dst), src: Operand::Immediate(1) }
        if dst.register() == od_isa::Register::Rax && dst.size() == OperandSize::Qword)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0xff
        && instruction.prefixes.rex == Some(0x48)
        && instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && !instruction.prefixes.address_override
        && instruction.operand_size == OperandSize::Qword
        && instruction.address_size == OperandSize::Qword
        && instruction.length == 3
        && bytes.get(..3) == Some(&[0x48, 0xff, 0xc8])
}

pub(crate) fn is_exact_bare_nop(instruction: Instruction, first_byte: u8) -> bool {
    matches!(instruction.operation, Operation::Nop)
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x90
        && instruction.prefixes == Default::default()
        && instruction.length == 1
        && first_byte == 0x90
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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
    build_block_with_profile(code, entry_rip, context, limits, BlockBuildProfile::S0_09)
}

/// Build an opt-in S0-09 block that may include only the exact admitted
/// `48 FF C0` native INC RAX form. Callers must also provide the matching
/// full-code admission witness before using this profile.
pub(crate) fn build_block_for_native_inc_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeIncRax,
    )
}

/// Build an opt-in S0-09 block that may include exact bare W64 `DEC RAX`.
pub(crate) fn build_block_for_native_dec_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeDecRax,
    )
}

pub(crate) fn build_block_for_native_neg_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeNegRax,
    )
}

pub(crate) fn build_block_for_native_not_rax(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeNotRax,
    )
}

pub(crate) fn build_block_for_native_lea_rax_rbx(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeLeaRaxRbx,
    )
}

pub(crate) fn build_block_for_native_lea_rax_rbx_disp8(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeLeaRaxRbxDisp8,
    )
}

pub(crate) fn build_block_for_native_lea_rax_rbx_rcx(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeLeaRaxRbxRcx,
    )
}

pub(crate) fn build_block_for_native_lea_rax_rip_disp32(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeLeaRaxRipDisp32,
    )
}

pub(crate) fn build_block_for_native_xchg_rax_rbx(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeXchgRaxRbx,
    )
}

pub(crate) fn build_block_for_native_add_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeAddRaxImm8,
    )
}

pub(crate) fn build_block_for_native_cmp_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeCmpRaxImm8,
    )
}

pub(crate) fn build_block_for_native_sub_rax_imm8(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeSubRaxImm8,
    )
}

pub(crate) fn build_block_for_native_mov_rax_imm32(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeMovRaxImm32,
    )
}

pub(crate) fn build_block_for_native_register_v1(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::S0_09NativeRegisterV1,
    )
}

/// Build one exact checked-qword-load candidate. This profile is opt-in and
/// stops after its first instruction; it does not widen the S0-09 block path.
pub(crate) fn build_checked_qword_load_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::CheckedQwordLoad,
    )
}

/// Build exact unprefixed `8B 03`, `8B 00`, `8B 04 24`, `8B 43 ib`, and
/// `8B 83 id` checked dword loads.
pub(crate) fn build_checked_dword_load_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::CheckedDwordLoad,
    )
}

/// Build exact unprefixed `89 03`, `89 00`, `89 43 ib`, `89 83 id`, or
/// `89 05 id` (`MOV [RIP+disp32],EAX`) checked-store forms.
pub(crate) fn build_checked_dword_store_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::CheckedDwordStore,
    )
}

/// Build exactly one checked qword store candidate. This remains opt-in and
/// separate from the ordinary block profile.
pub(crate) fn build_checked_qword_store_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::CheckedQwordStore,
    )
}

/// Build one of the two reviewed checked-memory pair profiles.
pub(crate) fn build_checked_qword_two_site_block(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
) -> Result<BuildOutcome, BuildError> {
    build_block_with_profile(
        code,
        entry_rip,
        context,
        limits,
        BlockBuildProfile::CheckedQwordMemoryPair,
    )
}

fn build_block_with_profile(
    code: CodeView<'_>,
    entry_rip: u64,
    context: AccessContext,
    limits: BlockLimits,
    profile: BlockBuildProfile,
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

        let mut memory = CodeMemory::limited(code, guest_rip, read_end_rip);
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

        let guest_offset = code.offset(guest_rip).ok_or(BuildError::EntryOutsideCode {
            entry_rip: guest_rip,
            base_rip: code.base_rip,
            code_end: code.code_end,
        })?;
        let admitted = if profile == BlockBuildProfile::CheckedQwordMemoryPair {
            let bytes = code.bytes.get(entry_offset..entry_offset.saturating_add(6));
            let store_load = bytes == Some(&[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b][..]);
            let load_store = bytes == Some(&[0x48, 0x8b, 0x03, 0x48, 0x89, 0x02][..]);
            let load_load = bytes == Some(&[0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b][..]);
            match (instructions.len(), store_load, load_store, load_load) {
                (0, true, _, _) => {
                    is_checked_qword_store_form(decoded)
                        && exact_direct_qword_memory_bytes(code, guest_rip, decoded.length, 0x89)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x89, 0x03][..])
                }
                (1, true, _, _) => {
                    is_checked_qword_load_form(decoded)
                        && exact_direct_qword_load_bytes(code, guest_rip, decoded.length)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x8b, 0x0b][..])
                }
                (0, _, true, _) | (0, _, _, true) => {
                    is_checked_qword_load_form(decoded)
                        && exact_direct_qword_load_bytes(code, guest_rip, decoded.length)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x8b, 0x03][..])
                }
                (1, _, true, _) => {
                    is_checked_qword_store_form(decoded)
                        && exact_direct_qword_memory_bytes(code, guest_rip, decoded.length, 0x89)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x89, 0x02][..])
                }
                (1, _, _, true) => {
                    is_checked_qword_load_form(decoded)
                        && exact_direct_qword_load_bytes(code, guest_rip, decoded.length)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x8b, 0x0b][..])
                }
                (0, false, false, false) => {
                    is_checked_qword_store_form(decoded)
                        && exact_direct_qword_memory_bytes(code, guest_rip, decoded.length, 0x89)
                        && code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                            == Some(&[0x48, 0x89, 0x03][..])
                }
                _ => false,
            }
        } else if profile == BlockBuildProfile::CheckedQwordStore {
            instructions.is_empty()
                && is_checked_qword_store_form(decoded)
                && (exact_direct_qword_memory_bytes(code, guest_rip, decoded.length, 0x89)
                    || exact_rip_relative_qword_rsi_rdi_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_direct_qword_store_rsi_rdi_bytes(code, guest_rip, decoded.length)
                    || exact_qword_address32_memory_bytes(code, guest_rip, decoded.length, 0x89)
                    || exact_qword_address32_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_sib_displacement_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_low_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rsi_rdi_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rbp_base_no_index_sib_disp_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_absolute_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_absolute_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_indexed_no_base_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_indexed_no_base_rbp_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_indexed_no_base_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_indexed_no_base_rbp_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rsp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rsp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rbp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rbp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_low_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_rsi_rdi_base_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_low_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_low_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rbp_base_no_index_sib_disp_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rsi_rdi_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rsi_rdi_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rsi_rdi_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rsp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rsp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rbp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    )
                    || exact_qword_address32_rbp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x89,
                    ))
        } else if profile == BlockBuildProfile::CheckedDwordLoad {
            instructions.is_empty()
                && is_checked_dword_load_form(decoded)
                && (code.bytes.get(guest_offset..guest_offset.saturating_add(2))
                    == Some(&[0x8b, 0x03][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x8b, 0x04, 0x24][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(2))
                        == Some(&[0x8b, 0x00][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x8b, 0x04, 0x0b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x8b, 0x04, 0x4b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x8b, 0x04, 0x8b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x8b, 0x04, 0xcb][..])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| {
                            bytes[..2] == [0x8b, 0x44]
                                && matches!(bytes[2], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| {
                            bytes[..2] == [0x8b, 0x84]
                                && matches!(bytes[2], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x04, 0x25])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x04, 0x0d])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x04, 0x4d])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x04, 0x8d])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x04, 0xcd])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x67, 0x8b, 0x03][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x67, 0x8b, 0x00][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(4))
                        == Some(&[0x67, 0x8b, 0x04, 0x24][..])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| bytes[..4] == [0x67, 0x8b, 0x04, 0x0d])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x8b, 0x04]
                                && matches!(bytes[3], 0x4d | 0x8d | 0xcd)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(5))
                        .is_some_and(|bytes| bytes[..4] == [0x67, 0x8b, 0x44, 0x24])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| bytes[..4] == [0x67, 0x8b, 0x84, 0x24])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x8b, 0x04]
                                && matches!(bytes[3], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(5))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x8b, 0x44]
                                && matches!(bytes[3], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x8b, 0x84]
                                && matches!(bytes[3], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x8b, 0x43])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x8b, 0x45])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x8b, 0x83])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x8b, 0x85])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x43])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x40])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x45])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x80])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x83])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x85])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[0..2] == [0x8b, 0x05])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x44, 0x24])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x8b, 0x84, 0x24]))
        } else if profile == BlockBuildProfile::CheckedDwordStore {
            instructions.is_empty()
                && is_checked_dword_store_form(decoded)
                && (code.bytes.get(guest_offset..guest_offset.saturating_add(2))
                    == Some(&[0x89, 0x03][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(2))
                        == Some(&[0x89, 0x00][..])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x40])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x89, 0x04, 0x0b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x89, 0x04, 0x4b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x89, 0x04, 0x8b][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x89, 0x04, 0xcb][..])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x89, 0x04, 0x24][..])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x89, 0x44, 0x24])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x89, 0x84, 0x24])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| {
                            bytes[..2] == [0x89, 0x44]
                                && matches!(bytes[2], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| {
                            bytes[..2] == [0x89, 0x84]
                                && matches!(bytes[2], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x89, 0x04, 0x25])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| {
                            bytes[..2] == [0x89, 0x04]
                                && matches!(bytes[2], 0x0d | 0x4d | 0x8d | 0xcd)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| bytes[..4] == [0x67, 0x89, 0x04, 0x25])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x89, 0x04]
                                && matches!(bytes[3], 0x0d | 0x4d | 0x8d | 0xcd)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(5))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x89, 0x44]
                                && matches!(bytes[3], 0x0b | 0x24 | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(8))
                        .is_some_and(|bytes| {
                            bytes[..4] == [0x67, 0x89, 0x84, 0x24]
                                || bytes[..3] == [0x67, 0x89, 0x84]
                                    && matches!(bytes[3], 0x0b | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x43])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(3))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x45])
                    || code.bytes.get(guest_offset..guest_offset.saturating_add(3))
                        == Some(&[0x67, 0x89, 0x03][..])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x89, 0x43])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x89, 0x45])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(4))
                        .is_some_and(|bytes| {
                            bytes[..3] == [0x67, 0x89, 0x04]
                                && matches!(bytes[3], 0x0b | 0x24 | 0x4b | 0x8b | 0xcb)
                        })
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x89, 0x83])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(7))
                        .is_some_and(|bytes| bytes[..3] == [0x67, 0x89, 0x85])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x83])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x80])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x85])
                    || code
                        .bytes
                        .get(guest_offset..guest_offset.saturating_add(6))
                        .is_some_and(|bytes| bytes[..2] == [0x89, 0x05]))
        } else if profile == BlockBuildProfile::CheckedQwordLoad {
            instructions.is_empty()
                && is_checked_qword_load_form(decoded)
                && (exact_direct_qword_load_rsi_rdi_bytes(code, guest_rip, decoded.length)
                    || exact_rip_relative_qword_rsi_rdi_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_direct_qword_load_bytes(code, guest_rip, decoded.length)
                    || exact_qword_address32_memory_bytes(code, guest_rip, decoded.length, 0x8b)
                    || exact_qword_address32_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_sib_displacement_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_low_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rsi_rdi_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rbp_base_no_index_sib_disp_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_absolute_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_absolute_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_indexed_no_base_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_indexed_no_base_rbp_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_indexed_no_base_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_indexed_no_base_rbp_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rsp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rsp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rbp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rbp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_low_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_rsi_rdi_base_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_low_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_low_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rbp_base_no_index_sib_disp_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rsi_rdi_base_no_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rsi_rdi_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rsi_rdi_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rsp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rsp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rbp_base_rsi_rdi_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    )
                    || exact_qword_address32_rbp_base_rbp_index_sib_memory_bytes(
                        code,
                        guest_rip,
                        decoded.length,
                        0x8b,
                    ))
        } else if profile == BlockBuildProfile::S0_09NativeIncRax {
            is_s0_09_form(decoded) || is_exact_inc_rax(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeDecRax {
            is_s0_09_form(decoded) || is_exact_dec_rax(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeNegRax {
            is_s0_09_form(decoded) || is_exact_neg_rax(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeNotRax {
            is_s0_09_form(decoded) || is_exact_not_rax(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeLeaRaxRbx {
            is_s0_09_form(decoded) || is_exact_lea_rax_rbx(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeLeaRaxRbxDisp8 {
            is_s0_09_form(decoded)
                || is_exact_lea_rax_rbx_disp8(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeLeaRaxRbxRcx {
            is_s0_09_form(decoded) || is_exact_lea_rax_rbx_rcx(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeLeaRaxRipDisp32 {
            is_s0_09_form(decoded)
                || is_exact_lea_rax_rip_disp32(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeXchgRaxRbx {
            is_s0_09_form(decoded) || is_exact_xchg_rax_rbx(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeAddRaxImm8 {
            is_s0_09_form(decoded) || is_exact_add_rax_imm8(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeCmpRaxImm8 {
            is_s0_09_form(decoded) || is_exact_cmp_rax_imm8(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeSubRaxImm8 {
            is_s0_09_form(decoded) || is_exact_sub_rax_imm8(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeMovRaxImm32 {
            is_s0_09_form(decoded) || is_exact_mov_rax_imm32(decoded, &code.bytes[guest_offset..])
        } else if profile == BlockBuildProfile::S0_09NativeRegisterV1 {
            is_s0_09_form(decoded)
                || native_register_v1_instruction(decoded, &code.bytes[guest_offset..]).is_some()
        } else {
            is_s0_09_form(decoded)
        };
        if !admitted {
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

        let complete_bounded_profile = match profile {
            BlockBuildProfile::CheckedQwordMemoryPair => instructions.len() == 2,
            BlockBuildProfile::CheckedQwordLoad
            | BlockBuildProfile::CheckedDwordLoad
            | BlockBuildProfile::CheckedDwordStore
            | BlockBuildProfile::CheckedQwordStore => true,
            BlockBuildProfile::S0_09
            | BlockBuildProfile::S0_09NativeIncRax
            | BlockBuildProfile::S0_09NativeDecRax
            | BlockBuildProfile::S0_09NativeNegRax
            | BlockBuildProfile::S0_09NativeNotRax
            | BlockBuildProfile::S0_09NativeLeaRaxRbx
            | BlockBuildProfile::S0_09NativeLeaRaxRbxDisp8
            | BlockBuildProfile::S0_09NativeLeaRaxRbxRcx
            | BlockBuildProfile::S0_09NativeLeaRaxRipDisp32
            | BlockBuildProfile::S0_09NativeXchgRaxRbx
            | BlockBuildProfile::S0_09NativeAddRaxImm8
            | BlockBuildProfile::S0_09NativeCmpRaxImm8
            | BlockBuildProfile::S0_09NativeRegisterV1 => false,
            BlockBuildProfile::S0_09NativeSubRaxImm8
            | BlockBuildProfile::S0_09NativeMovRaxImm32 => false,
        };
        if complete_bounded_profile {
            return Ok(finish(
                entry_rip,
                code.code_end,
                instructions,
                guest_ranges,
                BlockStopReason::InstructionLimit,
            ));
        }

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

    #[test]
    fn inc_rax_admission_is_exact_and_separate_from_nop_profile() {
        let inc = [0x48, 0xff, 0xc0];
        let view = CodeView::new(0x1000, &inc).unwrap();
        let admitted = S0_09NopIncRaxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_inc_rax(0x1000));
        assert!(!admitted.admits_nop(0x1000));
        assert!(matches!(
            S0_09NopAdmission::from_code(view),
            Err(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x1000 })
        ));

        for bytes in [
            &[0x48, 0xff][..],
            &[0xf3, 0x48, 0xff, 0xc0][..],
            &[0x66, 0x48, 0xff, 0xc0][..],
            &[0x48, 0x40, 0xff, 0xc0][..],
            &[0x48, 0xff, 0xc8][..],
            &[0x48, 0xff, 0xc1][..],
            &[0x48, 0xff, 0xc3][..],
            &[0xff, 0xc0][..],
        ] {
            let view = CodeView::new(0x2000, bytes).unwrap();
            assert!(
                S0_09NopIncRaxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }
        let branch_into_inc = [0x74, 0x01, 0x48, 0xff, 0xc0];
        let view = CodeView::new(0x3000, &branch_into_inc).unwrap();
        assert!(matches!(
            S0_09NopIncRaxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3003, .. })
        ));
    }

    #[test]
    fn dec_rax_admission_is_exact_and_separate_from_inc_profile() {
        let dec = [0x48, 0xff, 0xc8];
        let view = CodeView::new(0x1800, &dec).unwrap();
        let admitted = S0_09NopDecRaxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_dec_rax(0x1800));
        assert!(!admitted.admits_nop(0x1800));
        assert!(matches!(
            S0_09NopIncRaxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x1800 })
        ));
        assert!(matches!(
            S0_09NopAdmission::from_code(view),
            Err(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x1800 })
        ));

        for bytes in [
            &[0x48, 0xff][..],
            &[0xf3, 0x48, 0xff, 0xc8][..],
            &[0x66, 0x48, 0xff, 0xc8][..],
            &[0x48, 0x40, 0xff, 0xc8][..],
            &[0x48, 0xff, 0xc0][..],
            &[0x48, 0xff, 0xc1][..],
            &[0x48, 0xff, 0xc3][..],
            &[0x48, 0xff, 0xc9][..],
            &[0x48, 0xff, 0xcb][..],
            &[0x49, 0xff, 0xc8][..],
            &[0xff, 0xc8][..],
            &[0x48, 0x83, 0xe8, 1][..],
            &[0x48, 0xff, 0x08][..],
        ] {
            let view = CodeView::new(0x2800, bytes).unwrap();
            assert!(
                S0_09NopDecRaxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }
        let branch_into_dec = [0x74, 0x01, 0x48, 0xff, 0xc8];
        let view = CodeView::new(0x3800, &branch_into_dec).unwrap();
        assert!(matches!(
            S0_09NopDecRaxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3803, .. })
        ));
    }

    #[test]
    fn neg_rax_admission_is_exact_and_separate_from_inc_dec_profiles() {
        let neg = [0x48, 0xf7, 0xd8];
        let view = CodeView::new(0x1900, &neg).unwrap();
        let admitted = S0_09NopNegRaxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_neg_rax(0x1900));
        assert!(!admitted.admits_nop(0x1900));
        assert!(S0_09NopIncRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopDecRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopAdmission::from_code(view).is_err());

        for bytes in [
            &[0x48, 0xf7][..],
            &[0xf3, 0x48, 0xf7, 0xd8][..],
            &[0x66, 0x48, 0xf7, 0xd8][..],
            &[0x48, 0x40, 0xf7, 0xd8][..],
            &[0x49, 0xf7, 0xd8][..],
            &[0x48, 0xf7, 0xd9][..],
            &[0x48, 0xf7, 0xdb][..],
            &[0x48, 0xf7, 0x18][..],
            &[0xf7, 0xd8][..],
            &[0x48, 0xf7, 0xd0][..],
        ] {
            let view = CodeView::new(0x2900, bytes).unwrap();
            assert!(
                S0_09NopNegRaxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }
        let branch_into_neg = [0x74, 0x01, 0x48, 0xf7, 0xd8];
        let view = CodeView::new(0x3900, &branch_into_neg).unwrap();
        assert!(matches!(
            S0_09NopNegRaxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3903, .. })
        ));
    }

    #[test]
    fn not_rax_admission_is_exact_and_separate_from_neg_profile() {
        let not = [0x48, 0xf7, 0xd0];
        let view = CodeView::new(0x1b00, &not).unwrap();
        let admitted = S0_09NopNotRaxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_not_rax(0x1b00));
        assert!(!admitted.admits_nop(0x1b00));
        assert!(S0_09NopNegRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopIncRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopDecRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopAdmission::from_code(view).is_err());

        for bytes in [
            &[0x48, 0xf7][..],
            &[0xf3, 0x48, 0xf7, 0xd0][..],
            &[0x66, 0x48, 0xf7, 0xd0][..],
            &[0x48, 0x40, 0xf7, 0xd0][..],
            &[0x49, 0xf7, 0xd0][..],
            &[0x48, 0xf7, 0xd1][..],
            &[0x48, 0xf7, 0xd8][..],
            &[0x48, 0xf7, 0x10][..],
            &[0xf7, 0xd0][..],
        ] {
            let view = CodeView::new(0x2b00, bytes).unwrap();
            assert!(
                S0_09NopNotRaxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }
        let branch_into_not = [0x74, 0x01, 0x48, 0xf7, 0xd0];
        let view = CodeView::new(0x3b00, &branch_into_not).unwrap();
        assert!(matches!(
            S0_09NopNotRaxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3b03, .. })
        ));
    }

    #[test]
    fn lea_rax_rbx_admission_is_exact_and_separate_from_other_profiles() {
        let lea = [0x48, 0x8d, 0x03];
        let view = CodeView::new(0x1c00, &lea).unwrap();
        let admitted = S0_09NopLeaRaxRbxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_lea_rax_rbx(0x1c00));
        assert!(!admitted.admits_nop(0x1c00));
        assert!(S0_09NopAdmission::from_code(view).is_err());
        assert!(S0_09NopNotRaxAdmission::from_code(view).is_err());
        assert!(S0_09NopSubRaxImm8Admission::from_code(view).is_err());

        let nop_lea_nop = [0x90, 0x48, 0x8d, 0x03, 0x90];
        let view = CodeView::new(0x1d00, &nop_lea_nop).unwrap();
        let admitted = S0_09NopLeaRaxRbxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_nop(0x1d00));
        assert!(admitted.admits_lea_rax_rbx(0x1d01));
        assert!(admitted.admits_nop(0x1d04));

        for bytes in [
            &[0x48, 0x8d, 0x0b][..],                         // other destination
            &[0x48, 0x8d, 0x18][..],                         // other base register
            &[0x48, 0x8d, 0x04, 0x23][..],                   // equivalent SIB form
            &[0x48, 0x8d, 0x43, 0x00][..],                   // disp8=0
            &[0x48, 0x8d, 0x83, 0x00, 0x00, 0x00, 0x00][..], // disp32=0
            &[0x48, 0x8d, 0x05, 0x00, 0x00, 0x00, 0x00][..], // RIP relative
            &[0x8d, 0x03][..],                               // 32-bit destination
            &[0x67, 0x48, 0x8d, 0x03][..],                   // address32
            &[0x66, 0x48, 0x8d, 0x03][..],                   // operand override
            &[0x2e, 0x48, 0x8d, 0x03][..],                   // segment prefix
            &[0xf3, 0x48, 0x8d, 0x03][..],                   // REP prefix
            &[0x48, 0x40, 0x8d, 0x03][..],                   // extra REX
            &[0x49, 0x8d, 0x03][..],                         // different base register
            &[0x4c, 0x8d, 0x03][..],                         // different destination register
            &[0x48, 0x8d, 0x44, 0x23, 0x00][..],             // SIB plus disp8
            &[0x48, 0x8d][..],
        ] {
            let view = CodeView::new(0x2c00, bytes).unwrap();
            assert!(
                S0_09NopLeaRaxRbxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_lea = [0x74, 0x01, 0x48, 0x8d, 0x03];
        let view = CodeView::new(0x3c00, &branch_into_lea).unwrap();
        assert!(matches!(
            S0_09NopLeaRaxRbxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3c03, .. })
        ));
    }

    #[test]
    fn lea_rax_rbx_disp8_admission_covers_all_signed_bytes_and_rejects_neighbors() {
        for raw_displacement in 0u8..=u8::MAX {
            let bytes = [0x48, 0x8d, 0x43, raw_displacement];
            let view = CodeView::new(0x1c80, &bytes).unwrap();
            let admitted = S0_09NopLeaRaxRbxDisp8Admission::from_code(view).unwrap();
            assert!(
                admitted.admits_lea_rax_rbx_disp8(0x1c80),
                "{raw_displacement:#04x}"
            );
            assert!(
                S0_09NopLeaRaxRbxAdmission::from_code(view).is_err(),
                "the no-displacement profile must reject disp8={raw_displacement:#04x}"
            );
        }

        let nop_lea_nop = [0x90, 0x48, 0x8d, 0x43, 0xff, 0x90];
        let view = CodeView::new(0x1d80, &nop_lea_nop).unwrap();
        let admitted = S0_09NopLeaRaxRbxDisp8Admission::from_code(view).unwrap();
        assert!(admitted.admits_nop(0x1d80));
        assert!(admitted.admits_lea_rax_rbx_disp8(0x1d81));
        assert!(admitted.admits_nop(0x1d85));

        for bytes in [
            &[0x48, 0x8d, 0x4b, 0x01][..],                   // other destination
            &[0x48, 0x8d, 0x4b, 0x80][..], // other destination and signed-negative displacement
            &[0x48, 0x8d, 0x45, 0x01][..], // other base
            &[0x48, 0x8d, 0x44, 0x23, 0x01][..], // SIB
            &[0x48, 0x8d, 0x83, 0x01, 0x00, 0x00, 0x00][..], // disp32
            &[0x48, 0x8d, 0x05, 0x01, 0x00, 0x00, 0x00][..], // RIP relative
            &[0x48, 0x8d, 0x03][..],       // old no-displacement encoding
            &[0x8d, 0x43, 0x01][..],       // 32-bit destination
            &[0x67, 0x48, 0x8d, 0x43, 0x01][..], // address32
            &[0x66, 0x48, 0x8d, 0x43, 0x01][..], // operand override
            &[0x2e, 0x48, 0x8d, 0x43, 0x01][..], // segment prefix
            &[0xf3, 0x48, 0x8d, 0x43, 0x01][..], // REP prefix
            &[0x48, 0x40, 0x8d, 0x43, 0x01][..], // extra REX
            &[0x49, 0x8d, 0x43, 0x01][..], // extended base
            &[0x4c, 0x8d, 0x43, 0x01][..], // extended destination
            &[0x48, 0x8d, 0x43][..],       // truncated displacement
            &[0x48, 0x8d][..],
        ] {
            let view = CodeView::new(0x2c80, bytes).unwrap();
            assert!(
                S0_09NopLeaRaxRbxDisp8Admission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_displacement = [0x74, 0x03, 0x48, 0x8d, 0x43, 0x00];
        let view = CodeView::new(0x3c80, &branch_into_displacement).unwrap();
        assert!(matches!(
            S0_09NopLeaRaxRbxDisp8Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3c85, .. })
        ));
    }

    #[test]
    fn lea_rax_rbx_rcx_admission_is_exact_and_separate_from_other_lea_profiles() {
        let lea = [0x48, 0x8d, 0x04, 0x0b];
        let view = CodeView::new(0x1e80, &lea).unwrap();
        let admitted = S0_09NopLeaRaxRbxRcxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_lea_rax_rbx_rcx(0x1e80));
        assert!(!admitted.admits_nop(0x1e80));
        for rejects in [
            S0_09NopAdmission::from_code(view).is_err(),
            S0_09NopLeaRaxRbxAdmission::from_code(view).is_err(),
            S0_09NopLeaRaxRbxDisp8Admission::from_code(view).is_err(),
        ] {
            assert!(rejects, "the SIB form must remain its own exact profile");
        }

        let nop_lea_nop = [0x90, 0x48, 0x8d, 0x04, 0x0b, 0x90];
        let view = CodeView::new(0x1f80, &nop_lea_nop).unwrap();
        let admitted = S0_09NopLeaRaxRbxRcxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_nop(0x1f80));
        assert!(admitted.admits_lea_rax_rbx_rcx(0x1f81));
        assert!(admitted.admits_nop(0x1f85));

        for bytes in [
            &[0x48, 0x8d, 0x04, 0x1b][..],             // swapped base/index
            &[0x48, 0x8d, 0x04, 0x4b][..],             // scale 2
            &[0x48, 0x8d, 0x04, 0x8b][..],             // scale 4
            &[0x48, 0x8d, 0x04, 0xcb][..],             // scale 8
            &[0x48, 0x8d, 0x03][..],                   // no SIB/index
            &[0x48, 0x8d, 0x44, 0x0b, 0x00][..],       // SIB disp8=0
            &[0x48, 0x8d, 0x84, 0x0b, 0, 0, 0, 0][..], // SIB disp32=0
            &[0x48, 0x8d, 0x05, 0, 0, 0, 0][..],       // RIP-relative
            &[0x48, 0x8d, 0x0c, 0x0b][..],             // other destination
            &[0x48, 0x8d, 0x04, 0x0a][..],             // other base
            &[0x48, 0x8d, 0x04, 0x09][..],             // other index
            &[0x8d, 0x04, 0x0b][..],                   // 32-bit destination
            &[0x67, 0x48, 0x8d, 0x04, 0x0b][..],       // address32
            &[0x66, 0x48, 0x8d, 0x04, 0x0b][..],       // operand-size prefix
            &[0x2e, 0x48, 0x8d, 0x04, 0x0b][..],       // segment prefix
            &[0xf3, 0x48, 0x8d, 0x04, 0x0b][..],       // REP prefix
            &[0x48, 0x40, 0x8d, 0x04, 0x0b][..],       // extra REX
            &[0x49, 0x8d, 0x04, 0x0b][..],             // extended base
            &[0x4c, 0x8d, 0x04, 0x0b][..],             // extended index/destination
            &[0x48, 0x8d, 0x04][..],                   // truncated SIB
            &[0x48, 0x8d][..],
        ] {
            let view = CodeView::new(0x2e80, bytes).unwrap();
            assert!(
                S0_09NopLeaRaxRbxRcxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_sib = [0x74, 0x03, 0x48, 0x8d, 0x04, 0x0b];
        let view = CodeView::new(0x3e80, &branch_into_sib).unwrap();
        assert!(matches!(
            S0_09NopLeaRaxRbxRcxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3e85, .. })
        ));
    }

    #[test]
    fn lea_rax_rip_disp32_admission_is_exact_and_uses_signed_decoded_disp() {
        let bytes = [0x48, 0x8d, 0x05, 0x8b, 0x4d, 0x12, 0xe3];
        let view = CodeView::new(0x1000, &bytes).unwrap();
        let admitted = S0_09NopLeaRaxRipDisp32Admission::from_code(view).unwrap();
        assert!(admitted.admits_lea_rax_rip_disp32(0x1000));
        assert_eq!(
            admitted.displacement(0x1000),
            Some(i32::from_le_bytes([0x8b, 0x4d, 0x12, 0xe3]) as i64)
        );
        assert!(!S0_09NopLeaRaxRbxRcxAdmission::from_code(view).is_ok());

        for bytes in [
            &[0x67, 0x48, 0x8d, 0x05, 0, 0, 0, 0][..], // address32
            &[0x48, 0x8d, 0x0d, 0, 0, 0, 0][..],       // other destination
            &[0x48, 0x8d, 0x04, 0x25, 0, 0, 0, 0][..], // SIB
            &[0x48, 0x8d, 0x45, 0][..],                // disp8 register form
            &[0x66, 0x48, 0x8d, 0x05, 0, 0, 0, 0][..], // prefix
            &[0x48, 0x8d, 0x05, 0, 0, 0][..],          // truncated
        ] {
            let view = CodeView::new(0x2000, bytes).unwrap();
            assert!(
                S0_09NopLeaRaxRipDisp32Admission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }
        let branch_into_displacement = [0x74, 0x03, 0x48, 0x8d, 0x05, 0, 0, 0, 0];
        let view = CodeView::new(0x3000, &branch_into_displacement).unwrap();
        assert!(matches!(
            S0_09NopLeaRaxRipDisp32Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3005, .. })
        ));
    }

    #[test]
    fn xchg_rax_rbx_admission_is_exact_and_separate_from_other_profiles() {
        let xchg = [0x48, 0x87, 0xd8];
        let view = CodeView::new(0x1e00, &xchg).unwrap();
        let admitted = S0_09NopXchgRaxRbxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_xchg_rax_rbx(0x1e00));
        assert!(!admitted.admits_nop(0x1e00));
        assert!(S0_09NopAdmission::from_code(view).is_err());
        assert!(S0_09NopLeaRaxRbxAdmission::from_code(view).is_err());
        assert!(S0_09NopNotRaxAdmission::from_code(view).is_err());

        let nop_xchg_nop = [0x90, 0x48, 0x87, 0xd8, 0x90];
        let view = CodeView::new(0x1f00, &nop_xchg_nop).unwrap();
        let admitted = S0_09NopXchgRaxRbxAdmission::from_code(view).unwrap();
        assert!(admitted.admits_nop(0x1f00));
        assert!(admitted.admits_xchg_rax_rbx(0x1f01));
        assert!(admitted.admits_nop(0x1f04));

        for bytes in [
            &[0x48, 0x87, 0xc3][..], // alternate RAX/RBX encoding
            &[0x48, 0x93][..],       // accumulator short form
            &[0x48, 0x87, 0xc0][..], // self exchange
            &[0x48, 0x87, 0xd9][..], // wrong register pair
            &[0x48, 0x87, 0x18][..], // memory form
            &[0xf0, 0x48, 0x87, 0xd8][..],
            &[0x66, 0x48, 0x87, 0xd8][..],
            &[0x2e, 0x48, 0x87, 0xd8][..],
            &[0x49, 0x87, 0xd8][..],
            &[0x48, 0x87][..],
            &[0xf4][..],       // HLT is not an allowed interpreter-fallback form
            &[0x0f, 0x0b][..], // UD2 is not an allowed interpreter-fallback form
            &[0xc3][..],       // RET is not an allowed interpreter-fallback form
        ] {
            let view = CodeView::new(0x2d00, bytes).unwrap();
            assert!(
                S0_09NopXchgRaxRbxAdmission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_xchg = [0x74, 0x01, 0x48, 0x87, 0xd8];
        let view = CodeView::new(0x3d00, &branch_into_xchg).unwrap();
        assert!(matches!(
            S0_09NopXchgRaxRbxAdmission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3d03, .. })
        ));
    }

    #[test]
    fn mov_rax_imm32_admission_sign_extends_and_rejects_neighbors() {
        for raw in [
            0x0000_0000u32,
            0x0000_0001,
            0xffff_ffff,
            0x7fff_ffff,
            0x8000_0000,
            0x8000_0001,
            0x1234_5678,
            0x8765_4321,
            0xaaaa_5555,
            0x5555_aaaa,
        ] {
            let immediate = raw.to_le_bytes();
            let bytes = [
                0x48,
                0xc7,
                0xc0,
                immediate[0],
                immediate[1],
                immediate[2],
                immediate[3],
            ];
            let view = CodeView::new(0x2e00, &bytes).unwrap();
            let admission = S0_09NopMovRaxImm32Admission::from_code(view).unwrap();
            let expected = raw as i32 as i64 as u64;
            assert!(admission.admits_mov_rax_imm32(0x2e00));
            assert_eq!(admission.mov_rax_imm32_value(0x2e00), Some(expected));
            assert!(!admission.admits_nop(0x2e00));
            assert!(S0_09NopAdmission::from_code(view).is_err());

            let mut memory = CodeMemory::full_code(view);
            let decoded =
                long_decode::decode(&mut memory, 0x2e00, AccessContext::default()).unwrap();
            assert!(is_exact_mov_rax_imm32(decoded, &bytes));
            if raw == 0x8765_4321 {
                assert_eq!(expected, 0xffff_ffff_8765_4321);
            }
        }

        let imm64 = [0x48, 0xb8, 0x21, 0x43, 0x65, 0x87, 0x00, 0x00, 0x00, 0x00];
        let view = CodeView::new(0x2e80, &imm64).unwrap();
        let mut memory = CodeMemory::full_code(view);
        let decoded = long_decode::decode(&mut memory, 0x2e80, AccessContext::default()).unwrap();
        assert!(!is_exact_mov_rax_imm32(decoded, &imm64));
        // Existing baseline MOV RAX,imm64 remains a distinct accepted shape.
        assert!(S0_09NopAdmission::from_code(view).is_ok());
        assert!(S0_09NopMovRaxImm32Admission::from_code(view).is_ok());

        for bytes in [
            &[0x48, 0xc7, 0xc1, 1, 0, 0, 0][..],       // other destination
            &[0x49, 0xc7, 0xc0, 1, 0, 0, 0][..],       // R8 destination
            &[0x48, 0xc7, 0x00, 1, 0, 0, 0][..],       // memory destination
            &[0x48, 0xc7, 0xc8, 1, 0, 0, 0][..],       // /1
            &[0x48, 0xc7, 0xd0, 1, 0, 0, 0][..],       // /2
            &[0xc7, 0xc0, 1, 0, 0, 0][..],             // W32 zero-extending form
            &[0x66, 0x48, 0xc7, 0xc0, 1, 0, 0, 0][..], // operand override
            &[0x67, 0x48, 0xc7, 0xc0, 1, 0, 0, 0][..], // address override
            &[0x2e, 0x48, 0xc7, 0xc0, 1, 0, 0, 0][..], // segment prefix
            &[0xf3, 0x48, 0xc7, 0xc0, 1, 0, 0, 0][..], // REP prefix
            &[0x48, 0x40, 0xc7, 0xc0, 1, 0, 0, 0][..], // extra REX
            &[0x48, 0xc6, 0xc0, 1][..],                // byte MOV immediate
            &[0x48, 0xc7, 0xc0][..],
            &[0x48, 0xc7, 0xc0, 1][..],
            &[0x48, 0xc7, 0xc0, 1, 0][..],
            &[0x48, 0xc7, 0xc0, 1, 0, 0][..],
        ] {
            let view = CodeView::new(0x2f00, bytes).unwrap();
            assert!(
                S0_09NopMovRaxImm32Admission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_immediate = [0x74, 0x03, 0x48, 0xc7, 0xc0, 0, 0, 0, 0];
        let view = CodeView::new(0x3e00, &branch_into_immediate).unwrap();
        assert!(matches!(
            S0_09NopMovRaxImm32Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x3e05, .. })
        ));
    }

    #[test]
    fn add_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors() {
        for immediate in 0..=u8::MAX {
            let bytes = [0x48, 0x83, 0xc0, immediate];
            let view = CodeView::new(0x1a00, &bytes).unwrap();
            let admission = S0_09NopAddRaxImm8Admission::from_code(view).unwrap();
            assert!(admission.admits_add_rax_imm8(0x1a00));
            assert_eq!(
                admission.add_rax_imm8_value(0x1a00),
                Some((immediate as i8 as i64) as u64),
                "immediate byte {immediate:#04x}"
            );
            assert!(!admission.admits_nop(0x1a00));
            for other in [
                S0_09NopIncRaxAdmission::from_code(view).err(),
                S0_09NopDecRaxAdmission::from_code(view).err(),
                S0_09NopNegRaxAdmission::from_code(view).err(),
            ] {
                assert!(matches!(
                    other,
                    Some(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x1a00 })
                ));
            }
        }

        for bytes in [
            &[0x48, 0x83, 0xc1, 0x01][..],          // ADD RCX, imm8
            &[0x49, 0x83, 0xc0, 0x01][..],          // ADD R8, imm8
            &[0x48, 0x83, 0xe8, 0x01][..],          // SUB RAX, imm8 (/5)
            &[0x48, 0x83, 0xf8, 0x01][..],          // CMP RAX, imm8 (/7)
            &[0x48, 0x81, 0xc0, 0x01, 0, 0, 0][..], // ADD RAX, imm32
            &[0x48, 0x05, 0x01, 0, 0, 0][..],       // accumulator form
            &[0x48, 0x83, 0x00, 0x01][..],          // memory destination
            &[0x48, 0x80, 0xc0, 0x01][..],          // 8-bit ADD
            &[0x66, 0x48, 0x83, 0xc0, 0x01][..],    // operand override
            &[0x67, 0x48, 0x83, 0xc0, 0x01][..],    // address override
            &[0xf3, 0x48, 0x83, 0xc0, 0x01][..],    // group-1 prefix
            &[0x83, 0xc0, 0x01][..],                // W32
            &[0x48, 0x83, 0xc0][..],                // truncated
        ] {
            let view = CodeView::new(0x1b00, bytes).unwrap();
            assert!(
                S0_09NopAddRaxImm8Admission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_immediate = [0x74, 0x03, 0x48, 0x83, 0xc0, 0x01];
        let view = CodeView::new(0x1c00, &branch_into_immediate).unwrap();
        assert!(matches!(
            S0_09NopAddRaxImm8Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x1c05, .. })
        ));
    }

    #[test]
    fn cmp_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors() {
        for byte in 0u8..=u8::MAX {
            let source = [0x48, 0x83, 0xf8, byte];
            let view = CodeView::new(0x4c00, &source).unwrap();
            let admission = S0_09NopCmpRaxImm8Admission::from_code(view).unwrap();
            let expected = (byte as i8 as i64) as u64;
            assert!(admission.admits_cmp_rax_imm8(0x4c00), "{byte:#04x}");
            assert_eq!(admission.cmp_rax_imm8_value(0x4c00), Some(expected));
            assert!(S0_09NopAddRaxImm8Admission::from_code(view).is_err());
            assert!(S0_09NopIncRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopDecRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopNegRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopAdmission::from_code(view).is_err());
        }

        for bytes in [
            &[0x48, 0x83, 0xf8][..],                // truncated immediate
            &[0x48, 0x83, 0xc0, 0x01][..],          // ADD /0
            &[0x48, 0x83, 0xe8, 0x01][..],          // SUB /5
            &[0x48, 0x83, 0xf9, 0x01][..],          // CMP RCX
            &[0x49, 0x83, 0xf8, 0x01][..],          // CMP R8
            &[0x48, 0x81, 0xf8, 0x01, 0, 0, 0][..], // full immediate
            &[0x48, 0x3d, 0x01, 0, 0, 0][..],       // accumulator encoding
            &[0x48, 0x83, 0x38, 0x01][..],          // CMP [RAX], imm8
            &[0x83, 0xf8, 0x01][..],                // 32-bit width
            &[0x66, 0x83, 0xf8, 0x01][..],          // 16-bit width
            &[0x80, 0xf8, 0x01][..],                // 8-bit width
            &[0x66, 0x48, 0x83, 0xf8, 0x01][..],
            &[0xf3, 0x48, 0x83, 0xf8, 0x01][..],
            &[0x48, 0x40, 0x83, 0xf8, 0x01][..],
        ] {
            let view = CodeView::new(0x4d00, bytes).unwrap();
            assert!(
                S0_09NopCmpRaxImm8Admission::from_code(view).is_err(),
                "{bytes:02x?}"
            );
        }

        let branch_into_immediate = [0x74, 0x03, 0x48, 0x83, 0xf8, 0xff];
        let view = CodeView::new(0x4e00, &branch_into_immediate).unwrap();
        assert!(matches!(
            S0_09NopCmpRaxImm8Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x4e05, .. })
        ));
    }

    #[test]
    fn sub_rax_imm8_admission_covers_signed_byte_range_and_rejects_neighbors() {
        for byte in 0u8..=u8::MAX {
            let source = [0x48, 0x83, 0xe8, byte];
            let view = CodeView::new(0x4f00, &source).unwrap();
            let admission = S0_09NopSubRaxImm8Admission::from_code(view).unwrap();
            let expected = (byte as i8 as i64) as u64;
            assert!(admission.admits_sub_rax_imm8(0x4f00), "{byte:#04x}");
            assert_eq!(admission.sub_rax_imm8_value(0x4f00), Some(expected));
            assert!(!admission.admits_nop(0x4f00));
            assert!(S0_09NopAddRaxImm8Admission::from_code(view).is_err());
            assert!(S0_09NopCmpRaxImm8Admission::from_code(view).is_err());
            assert!(S0_09NopIncRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopDecRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopNegRaxAdmission::from_code(view).is_err());
            assert!(S0_09NopAdmission::from_code(view).is_err());
        }

        for source in [
            &[0x48, 0x83, 0xe8][..],                // truncated immediate
            &[0x48, 0x83, 0xc0, 0x01][..],          // ADD RAX, imm8 (/0)
            &[0x48, 0x83, 0xf8, 0x01][..],          // CMP RAX, imm8 (/7)
            &[0x48, 0x83, 0xe9, 0x01][..],          // SUB RCX, imm8
            &[0x49, 0x83, 0xe8, 0x01][..],          // SUB R8, imm8
            &[0x48, 0x81, 0xe8, 0x01, 0, 0, 0][..], // SUB RAX, imm32
            &[0x48, 0x2d, 0x01, 0, 0, 0][..],       // accumulator form
            &[0x48, 0x83, 0x28, 0x01][..],          // SUB [RAX], imm8
            &[0x48, 0x80, 0xe8, 0x01][..],          // 8-bit SUB
            &[0x66, 0x48, 0x83, 0xe8, 0x01][..],    // operand override
            &[0x67, 0x48, 0x83, 0xe8, 0x01][..],    // address override
            &[0xf3, 0x48, 0x83, 0xe8, 0x01][..],    // group-1 prefix
            &[0x48, 0x40, 0x83, 0xe8, 0x01][..],    // extra REX prefix
            &[0x83, 0xe8, 0x01][..],                // W32
        ] {
            let view = CodeView::new(0x5000, source).unwrap();
            assert!(
                S0_09NopSubRaxImm8Admission::from_code(view).is_err(),
                "{source:02x?}"
            );
        }

        let branch_into_immediate = [0x74, 0x03, 0x48, 0x83, 0xe8, 0xff];
        let view = CodeView::new(0x5100, &branch_into_immediate).unwrap();
        assert!(matches!(
            S0_09NopSubRaxImm8Admission::from_code(view),
            Err(S0_09NopAdmissionError::BranchTargetNotBoundary { target: 0x5105, .. })
        ));
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
    fn exact_store_then_load_profile_builds_two_instructions_only() {
        let code = CodeView::new(0x4000, &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b, 0x90]).unwrap();
        let outcome = build_checked_qword_two_site_block(
            code,
            0x4000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let built = block(outcome);
        assert_eq!(built.instructions.len(), 2);
        assert_eq!(built.instructions[0].guest_rip, 0x4000);
        assert_eq!(built.instructions[0].next_rip, 0x4003);
        assert_eq!(built.instructions[1].guest_rip, 0x4003);
        assert_eq!(built.instructions[1].next_rip, 0x4006);
        assert_eq!(built.stop_reason, BlockStopReason::InstructionLimit);

        let bytes = &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0f][..];
        {
            let outcome = build_checked_qword_two_site_block(
                CodeView::new(0x4000, bytes).unwrap(),
                0x4000,
                AccessContext::default(),
                BlockLimits::default(),
            )
            .unwrap();
            let BuildOutcome::Block(block) = outcome else {
                panic!("first store should be decoded");
            };
            assert_eq!(block.instructions.len(), 1);
        }
        let outcome = build_checked_qword_two_site_block(
            CodeView::new(
                0x4000,
                &[0x48, 0x89, 0x03, 0x48, 0x8b, 0x0b, 0x48, 0x89, 0x03],
            )
            .unwrap(),
            0x4000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        assert_eq!(block(outcome).instructions.len(), 2);
    }

    #[test]
    fn exact_load_then_load_profile_builds_two_instructions_only() {
        let bytes = [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x0b];
        let outcome = build_checked_qword_two_site_block(
            CodeView::new(0x5000, &bytes).unwrap(),
            0x5000,
            AccessContext::default(),
            BlockLimits::default(),
        )
        .unwrap();
        let built = block(outcome);
        assert_eq!(built.instructions.len(), 2);
        assert_eq!(built.instructions[0].guest_rip, 0x5000);
        assert_eq!(built.instructions[0].next_rip, 0x5003);
        assert_eq!(built.instructions[1].guest_rip, 0x5003);
        assert_eq!(built.instructions[1].next_rip, 0x5006);
        assert_eq!(built.stop_reason, BlockStopReason::InstructionLimit);

        for unsupported in [
            [0x48, 0x8b, 0x03, 0x48, 0x8b, 0x13],
            [0x48, 0x8b, 0x0b, 0x48, 0x8b, 0x03],
            [0x48, 0x8b, 0x03, 0x48, 0x89, 0x0b],
        ] {
            let outcome = build_checked_qword_two_site_block(
                CodeView::new(0x5000, &unsupported).unwrap(),
                0x5000,
                AccessContext::default(),
                BlockLimits::default(),
            )
            .unwrap();
            if let BuildOutcome::Block(block) = outcome {
                assert_ne!(block.instructions.len(), 2);
            }
        }
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

    #[test]
    fn native_register_v1_admission_is_exact_and_combines_all_committed_forms() {
        let forms: [&[u8]; 13] = [
            &[0x48, 0xff, 0xc0],
            &[0x48, 0xff, 0xc8],
            &[0x48, 0xf7, 0xd8],
            &[0x48, 0xf7, 0xd0],
            &[0x48, 0x83, 0xc0, 0x80],
            &[0x48, 0x83, 0xf8, 0x7f],
            &[0x48, 0x83, 0xe8, 0xff],
            &[0x48, 0xc7, 0xc0, 0x00, 0x00, 0x00, 0x80],
            &[0x48, 0x87, 0xd8],
            &[0x48, 0x8d, 0x03],
            &[0x48, 0x8d, 0x43, 0x80],
            &[0x48, 0x8d, 0x04, 0x0b],
            &[0x48, 0x8d, 0x05, 0x01, 0x00, 0x00, 0x00],
        ];
        let mut bytes = vec![0x90, 0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        for form in forms {
            bytes.extend_from_slice(form);
        }
        let view = CodeView::new(0x1200, &bytes).unwrap();
        let admission = NativeRegisterV1Admission::from_code(view).unwrap();
        assert!(admission.admits_nop(0x1200));
        assert_eq!(admission.boundaries.boundaries.len(), 16);
        assert!(admission.boundaries.contains(view.code_end()));
        assert!(matches!(
            admission.instruction(0x1200 + 11),
            Some(NativeRegisterV1Instruction::IncRax)
        ));
        let native_count = admission
            .instructions
            .iter()
            .filter(|instruction| instruction.native.is_some())
            .count();
        assert_eq!(native_count, 13);
        assert_eq!(
            admission.instruction(0x1200 + 11 + 12),
            Some(NativeRegisterV1Instruction::AddRaxImm8((-128i64) as u64))
        );
        assert_eq!(
            admission.instruction(view.code_end() - 7),
            Some(NativeRegisterV1Instruction::LeaRaxRipDisp32(1))
        );

        macro_rules! assert_old_admission_rejects_foreign_form {
            ($admission:ty, $index:expr) => {{
                let mut isolated = forms[$index].to_vec();
                isolated.extend_from_slice(forms[($index + 1) % forms.len()]);
                let isolated_view = CodeView::new(0x1a00, &isolated).unwrap();
                assert!(matches!(
                    <$admission>::from_code(isolated_view),
                    Err(S0_09NopAdmissionError::UnsupportedInstruction { .. })
                ));
            }};
        }
        assert_old_admission_rejects_foreign_form!(S0_09NopIncRaxAdmission, 0);
        assert_old_admission_rejects_foreign_form!(S0_09NopDecRaxAdmission, 1);
        assert_old_admission_rejects_foreign_form!(S0_09NopNegRaxAdmission, 2);
        assert_old_admission_rejects_foreign_form!(S0_09NopNotRaxAdmission, 3);
        assert_old_admission_rejects_foreign_form!(S0_09NopAddRaxImm8Admission, 4);
        assert_old_admission_rejects_foreign_form!(S0_09NopCmpRaxImm8Admission, 5);
        assert_old_admission_rejects_foreign_form!(S0_09NopSubRaxImm8Admission, 6);
        assert_old_admission_rejects_foreign_form!(S0_09NopMovRaxImm32Admission, 7);
        assert_old_admission_rejects_foreign_form!(S0_09NopXchgRaxRbxAdmission, 8);
        assert_old_admission_rejects_foreign_form!(S0_09NopLeaRaxRbxAdmission, 9);
        assert_old_admission_rejects_foreign_form!(S0_09NopLeaRaxRbxDisp8Admission, 10);
        assert_old_admission_rejects_foreign_form!(S0_09NopLeaRaxRbxRcxAdmission, 11);
        assert_old_admission_rejects_foreign_form!(S0_09NopLeaRaxRipDisp32Admission, 12);

        for foreign in [
            &[0x48, 0xff, 0xc1][..],             // INC RCX
            &[0x49, 0xff, 0xc0][..],             // different REX
            &[0x48, 0x8d, 0x83, 0, 0, 0, 0][..], // disp32 variant
            &[0x48, 0x89, 0x03][..],             // memory data operation
        ] {
            let view = CodeView::new(0x2200, foreign).unwrap();
            assert!(matches!(
                NativeRegisterV1Admission::from_code(view),
                Err(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x2200 })
            ));
        }

        let interior_targets: [(&[u8], usize); 4] = [
            (&[0x48, 0xff, 0xc0], 3),
            (&[0x48, 0x83, 0xc0, 0x01], 4),
            (&[0x48, 0xc7, 0xc0, 0x01, 0, 0, 0], 7),
            (&[0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0], 10),
        ];
        for (form, length) in interior_targets {
            for interior_offset in 1..length {
                let mut branch = vec![0x74, interior_offset as u8];
                branch.extend_from_slice(form);
                let target = 0x3200 + 2 + interior_offset as u64;
                assert!(matches!(
                    NativeRegisterV1Admission::from_code(
                        CodeView::new(0x3200, &branch).unwrap()
                    ),
                    Err(S0_09NopAdmissionError::BranchTargetNotBoundary {
                        target: actual,
                        ..
                    }) if actual == target
                ));
            }
        }
        let branch_over_variable_lengths = [
            0x74, 0x04, 0x48, 0x83, 0xc0, 0x01, 0x48, 0x8d, 0x05, 0, 0, 0, 0,
        ];
        let branch_admission = NativeRegisterV1Admission::from_code(
            CodeView::new(0x4200, &branch_over_variable_lengths).unwrap(),
        )
        .unwrap();
        assert!(branch_admission.contains(0x4206));

        let isolated = [0x48, 0xff, 0xc0, 0x48, 0xff, 0xc8];
        assert!(matches!(
            S0_09NopIncRaxAdmission::from_code(CodeView::new(0x5200, &isolated).unwrap()),
            Err(S0_09NopAdmissionError::UnsupportedInstruction { rip: 0x5203 })
        ));
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

pub(crate) fn is_checked_qword_load_form(instruction: Instruction) -> bool {
    let exact_prefix = instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && instruction.prefixes.rex == Some(0x48);
    let Operation::Mov {
        dst: Operand::Register(destination),
        src: Operand::Memory(operand),
    } = instruction.operation
    else {
        return false;
    };
    exact_prefix
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8b
        && instruction.operand_size == OperandSize::Qword
        && matches!(instruction.length, 3 | 4 | 5 | 6 | 7 | 8 | 9)
        && ((destination.register() as u8) < 4
            || matches!(destination.register() as u8, 6 | 7)
                && is_rsp_base_no_index_qword_sib_data_operand(instruction, operand)
            || matches!(destination.register() as u8, 6 | 7)
                && operand.size == OperandSize::Qword
                && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
                && operand.index.is_none()
                && operand.scale == 1
                && (instruction.address_size == OperandSize::Qword
                    && operand.address_size == OperandSize::Qword
                    && match instruction.length {
                        3 => operand.displacement == 0,
                        4 => i8::try_from(operand.displacement).is_ok(),
                        7 => i32::try_from(operand.displacement).is_ok(),
                        _ => false,
                    }
                    || instruction.address_size == OperandSize::Dword
                        && operand.address_size == OperandSize::Dword
                        && match instruction.length {
                            4 => operand.displacement == 0,
                            5 => i8::try_from(operand.displacement).is_ok(),
                            8 => i32::try_from(operand.displacement).is_ok(),
                            _ => false,
                        })
            || matches!(destination.register() as u8, 6 | 7)
                && is_rbp_direct_qword_memory_data_operand(instruction, operand)
            || matches!(destination.register() as u8, 6 | 7)
                && instruction.address_size == OperandSize::Qword
                && instruction.length == 7
                && operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Qword
                && matches!(operand.base, od_isa::long_decode::AddressBase::NextRip)
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok()
            || destination.register() as u8 == 4
                && is_rsp_memory_data_operand(instruction, operand)
            || destination.register() as u8 == 5
                && is_rbp_memory_data_operand(instruction, operand))
        && destination.size() == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && is_checked_qword_memory_address(instruction, operand)
}

pub(crate) fn is_checked_dword_load_form(instruction: Instruction) -> bool {
    let Operation::Mov {
        dst: Operand::Register(destination),
        src: Operand::Memory(operand),
    } = instruction.operation
    else {
        return false;
    };
    let address32_profile = instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && (((matches!(operand.base, AddressBase::Register(od_isa::Register::Rbx))
            || matches!(instruction.length, 4 | 7)
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp)))
            && ((operand.index.is_none()
                && operand.scale == 1
                && ((instruction.length == 3 && operand.displacement == 0)
                    || (instruction.length == 4 && i8::try_from(operand.displacement).is_ok())
                    || (instruction.length == 7
                        && i32::try_from(operand.displacement).is_ok())))
                || (matches!(instruction.length, 4 | 5 | 8)
                    && operand.index == Some(od_isa::Register::Rcx)
                    && matches!(operand.scale, 1 | 2 | 4 | 8)
                    && ((instruction.length == 4 && operand.displacement == 0)
                        || (instruction.length == 5
                            && i8::try_from(operand.displacement).is_ok())
                        || (instruction.length == 8
                            && i32::try_from(operand.displacement).is_ok())))))
            || (instruction.length == 4
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && operand.displacement == 0)
            || (instruction.length == 5
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i8::try_from(operand.displacement).is_ok())
            || (instruction.length == 8
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 8
                && matches!(operand.base, AddressBase::None)
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 8
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rcx)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 8
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rax)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 3
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rax))
                && operand.index.is_none()
                && operand.scale == 1
                && operand.displacement == 0));
    let exact_prefix = instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && instruction.prefixes.rex.is_none()
        && (!instruction.prefixes.address_override || address32_profile);
    let address64_profile = instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.index.is_none()
        && operand.scale == 1
        && ((instruction.length == 2
            && matches!(
                operand.base,
                AddressBase::Register(od_isa::Register::Rax | od_isa::Register::Rbx)
            )
            && operand.displacement == 0)
            || (instruction.length == 3
                && matches!(
                    operand.base,
                    AddressBase::Register(
                        od_isa::Register::Rax | od_isa::Register::Rbx | od_isa::Register::Rbp
                    )
                )
                && i8::try_from(operand.displacement).is_ok())
            || (instruction.length == 6
                && matches!(
                    operand.base,
                    AddressBase::Register(
                        od_isa::Register::Rax | od_isa::Register::Rbx | od_isa::Register::Rbp
                    )
                )
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 6
                && matches!(operand.base, AddressBase::NextRip)
                && i32::try_from(operand.displacement).is_ok())
            || (instruction.length == 7
                && matches!(operand.base, AddressBase::None)
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)));
    let rcx_no_base_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::None)
        && operand.index == Some(od_isa::Register::Rcx)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok();
    let rax_no_base_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::None)
        && operand.index == Some(od_isa::Register::Rax)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok();
    let rbx_rcx_sib_profile = matches!(instruction.length, 3 | 4 | 7)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbx))
        && operand.index == Some(od_isa::Register::Rcx)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && ((instruction.length == 3 && operand.displacement == 0)
            || (instruction.length == 4 && i8::try_from(operand.displacement).is_ok())
            || (instruction.length == 7 && i32::try_from(operand.displacement).is_ok()));
    let rsp_no_index_sib_profile = instruction.length == 3
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index.is_none()
        && operand.scale == 1
        && operand.displacement == 0;
    let rsp_disp8_sib_profile = instruction.length == 4
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index.is_none()
        && operand.scale == 1
        && i8::try_from(operand.displacement).is_ok();
    let rsp_disp32_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
        && operand.index.is_none()
        && operand.scale == 1
        && i32::try_from(operand.displacement).is_ok();
    let exact_address = address64_profile
        || address32_profile
        || rcx_no_base_sib_profile
        || rax_no_base_sib_profile
        || rbx_rcx_sib_profile
        || rsp_no_index_sib_profile
        || rsp_disp8_sib_profile
        || rsp_disp32_sib_profile;
    exact_prefix
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x8b
        && instruction.operand_size == OperandSize::Dword
        && (instruction.address_size == OperandSize::Qword || address32_profile)
        && destination.register() == od_isa::Register::Rax
        && destination.size() == OperandSize::Dword
        && operand.size == OperandSize::Dword
        && exact_address
}

pub(crate) fn is_checked_dword_store_form(instruction: Instruction) -> bool {
    let address32_rbx_rcx_sib_profile = instruction.prefixes.address_override
        && matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbx))
                && operand.index == Some(od_isa::Register::Rcx)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && ((instruction.length == 4 && operand.displacement == 0)
                    || (instruction.length == 5
                        && i8::try_from(operand.displacement).is_ok())
                    || (instruction.length == 8
                        && i32::try_from(operand.displacement).is_ok())));
    let address32_rsp_sib_profile = instruction.prefixes.address_override
        && instruction.length == 4
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && operand.displacement == 0);
    let address32_rsp_disp8_sib_profile = instruction.prefixes.address_override
        && instruction.length == 5
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i8::try_from(operand.displacement).is_ok());
    let address32_rsp_disp32_sib_profile = instruction.prefixes.address_override
        && instruction.length == 8
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok());
    let rax_disp8_profile = instruction.length == 3
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rax))
                && operand.index.is_none()
                && operand.scale == 1
                && i8::try_from(operand.displacement).is_ok());
    let rax_disp32_profile = instruction.length == 6
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rax))
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok());
    let address32_absolute_sib_profile = instruction.prefixes.address_override
        && instruction.length == 8
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::None)
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok());
    let address32_rcx_no_base_sib_profile = instruction.prefixes.address_override
        && instruction.length == 8
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rcx)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok());
    let address32_rax_no_base_sib_profile = instruction.prefixes.address_override
        && instruction.length == 8
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rax)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok());
    let rax_index_no_base_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rax)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok());
    let address32_profile = instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && (matches!(operand.base, AddressBase::Register(od_isa::Register::Rbx))
                    || matches!(instruction.length, 4 | 7)
                        && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbp)))
                && operand.index.is_none()
                && operand.scale == 1
                && ((instruction.length == 3 && operand.displacement == 0)
                    || (instruction.length == 4 && i8::try_from(operand.displacement).is_ok())
                    || (instruction.length == 7
                        && i32::try_from(operand.displacement).is_ok())));
    // Keep the addr32 RAX base confined to its exact zero-displacement ModRM
    // form; the existing no-base SIB RAX profiles are separate encodings.
    let address32_rax_base_profile = instruction.prefixes.address_override
        && instruction.length == 3
        && instruction.address_size == OperandSize::Dword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Dword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rax))
                && operand.index.is_none()
                && operand.scale == 1
                && operand.displacement == 0);
    let rcx_no_base_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::None)
                && operand.index == Some(od_isa::Register::Rcx)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok());
    let exact_prefix = instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && instruction.prefixes.rex.is_none()
        && (!instruction.prefixes.address_override
            || address32_profile
            || address32_rsp_sib_profile
            || address32_rsp_disp8_sib_profile
            || address32_rsp_disp32_sib_profile
            || address32_absolute_sib_profile
            || address32_rcx_no_base_sib_profile
            || address32_rax_no_base_sib_profile
            || address32_rax_base_profile
            || address32_rbx_rcx_sib_profile);
    let rbx_rcx_sib_profile = matches!(instruction.length, 3 | 4 | 7)
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rbx))
                && operand.index == Some(od_isa::Register::Rcx)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && ((instruction.length == 3 && operand.displacement == 0)
                    || (instruction.length == 4
                        && i8::try_from(operand.displacement).is_ok())
                    || (instruction.length == 7
                        && i32::try_from(operand.displacement).is_ok())));
    let rsp_no_index_sib_profile = instruction.length == 3
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && operand.displacement == 0);
    let rsp_disp8_sib_profile = instruction.length == 4
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i8::try_from(operand.displacement).is_ok());
    let rsp_disp32_sib_profile = instruction.length == 7
        && instruction.address_size == OperandSize::Qword
        && matches!(&instruction.operation, Operation::Mov { dst: Operand::Memory(operand), .. }
            if operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Dword
                && matches!(operand.base, AddressBase::Register(od_isa::Register::Rsp))
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok());
    let Operation::Mov {
        dst: Operand::Memory(operand),
        src: Operand::Register(source),
    } = instruction.operation
    else {
        return false;
    };
    exact_prefix
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x89
        && instruction.operand_size == OperandSize::Dword
        && (instruction.address_size == OperandSize::Qword
            || address32_profile
            || address32_rsp_sib_profile
            || address32_rsp_disp8_sib_profile
            || address32_rsp_disp32_sib_profile
            || address32_absolute_sib_profile
            || address32_rcx_no_base_sib_profile
            || address32_rax_no_base_sib_profile
            || address32_rax_base_profile
            || address32_rbx_rcx_sib_profile)
        && source.register() == od_isa::Register::Rax
        && source.size() == OperandSize::Dword
        && operand.size == OperandSize::Dword
        && operand.address_size == instruction.address_size
        && (rbx_rcx_sib_profile
            || rcx_no_base_sib_profile
            || rax_index_no_base_sib_profile
            || address32_rbx_rcx_sib_profile
            || address32_rsp_sib_profile
            || address32_rsp_disp8_sib_profile
            || address32_rsp_disp32_sib_profile
            || address32_absolute_sib_profile
            || address32_rcx_no_base_sib_profile
            || address32_rax_no_base_sib_profile
            || address32_rax_base_profile
            || rsp_no_index_sib_profile
            || rsp_disp8_sib_profile
            || rsp_disp32_sib_profile
            || rax_disp8_profile
            || rax_disp32_profile
            || (operand.index.is_none()
                && operand.scale == 1
                && ((instruction.length == 2
                    && matches!(
                        operand.base,
                        AddressBase::Register(od_isa::Register::Rax | od_isa::Register::Rbx)
                    )
                    && operand.displacement == 0)
                    || (instruction.length == 3
                        && matches!(
                            operand.base,
                            AddressBase::Register(od_isa::Register::Rbx | od_isa::Register::Rbp)
                        )
                        && i8::try_from(operand.displacement).is_ok())
                    || (instruction.length == 6
                        && matches!(
                            operand.base,
                            AddressBase::Register(od_isa::Register::Rbx | od_isa::Register::Rbp)
                        )
                        && i32::try_from(operand.displacement).is_ok())
                    || (instruction.length == 6
                        && matches!(operand.base, AddressBase::NextRip)
                        && i32::try_from(operand.displacement).is_ok())
                    || (instruction.length == 7
                        && matches!(operand.base, AddressBase::None)
                        && i32::try_from(operand.displacement).is_ok())
                    || address32_profile)))
}

pub(crate) fn is_checked_qword_store_form(instruction: Instruction) -> bool {
    let exact_prefix = instruction.prefixes.group1.is_none()
        && instruction.prefixes.segment.is_none()
        && !instruction.prefixes.operand_override
        && instruction.prefixes.rex == Some(0x48);
    let Operation::Mov {
        dst: Operand::Memory(operand),
        src: Operand::Register(source),
    } = instruction.operation
    else {
        return false;
    };
    exact_prefix
        && instruction.opcode.map == OpcodeMap::Primary
        && instruction.opcode.byte == 0x89
        && instruction.operand_size == OperandSize::Qword
        && matches!(instruction.length, 3 | 4 | 5 | 6 | 7 | 8 | 9)
        && ((source.register() as u8) < 4
            || source.register() as u8 == 4 && is_rsp_memory_data_operand(instruction, operand)
            || source.register() as u8 == 5 && is_rbp_memory_data_operand(instruction, operand)
            || matches!(source.register() as u8, 6 | 7)
                && is_rsp_base_no_index_qword_sib_data_operand(instruction, operand)
            || matches!(source.register() as u8, 6 | 7)
                && operand.index.is_none()
                && operand.scale == 1
                && (instruction.address_size == OperandSize::Qword
                    && operand.address_size == OperandSize::Qword
                    && match instruction.length {
                        3 => operand.displacement == 0,
                        4 => i8::try_from(operand.displacement).is_ok(),
                        7 => i32::try_from(operand.displacement).is_ok(),
                        _ => false,
                    }
                    || instruction.address_size == OperandSize::Dword
                        && operand.address_size == OperandSize::Dword
                        && match instruction.length {
                            4 => operand.displacement == 0,
                            5 => i8::try_from(operand.displacement).is_ok(),
                            8 => i32::try_from(operand.displacement).is_ok(),
                            _ => false,
                        })
                && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
            || matches!(source.register() as u8, 6 | 7)
                && is_rbp_direct_qword_memory_data_operand(instruction, operand)
            || matches!(source.register() as u8, 6 | 7)
                && instruction.address_size == OperandSize::Qword
                && instruction.length == 7
                && operand.address_size == OperandSize::Qword
                && operand.size == OperandSize::Qword
                && matches!(operand.base, od_isa::long_decode::AddressBase::NextRip)
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok())
        && source.size() == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && is_checked_qword_memory_address(instruction, operand)
}

fn is_indexed_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
        8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
        _ => false,
    };
    instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && (matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
            || matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
            )
            || matches!(instruction.length, 4 | 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                )
            || matches!(instruction.length, 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                ))
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rbp_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rsi_rdi_base_rbp_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_low_base_rsi_rdi_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_low_base_no_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rsi_rdi_base_no_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_low_base_rsi_rdi_index_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 5 | 6 | 9)
        && instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction.length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_low_base_no_index_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 5 | 6 | 9)
        && instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rbp_base_no_index_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 6 | 9)
        && instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && if instruction.length == 6 {
            i8::try_from(operand.displacement).is_ok()
        } else {
            i32::try_from(operand.displacement).is_ok()
        }
}

fn is_address32_rsi_rdi_base_no_index_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 5 | 6 | 9)
        && instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            5 => operand.displacement == 0,
            6 => i8::try_from(operand.displacement).is_ok(),
            9 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_address32_rsi_rdi_base_rsi_rdi_index_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.prefixes.address_override
        && instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rsp_base_rbp_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
        8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rsp_base_rsi_rdi_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 4 | 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rsp_base_no_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            4 => operand.displacement == 0,
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rbp_base_rbp_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    matches!(instruction.length, 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_rbp_base_rsi_rdi_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    matches!(instruction.length, 5 | 8)
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && match instruction.length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_indexed_no_base_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.length == 8
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_indexed_no_base_rbp_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.length == 8
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_indexed_no_base_rsi_rdi_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.length == 8
        && instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_direct_stack_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        4 => operand.displacement == 0,
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && operand.index.is_none()
        && operand.scale == 1
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
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
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && (matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
            || matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
            )
            || matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(
                    od_isa::Register::Rsi | od_isa::Register::Rdi
                )
            )
            || matches!(instruction.length, 6 | 9)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                ))
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rbp_indexed_sib_stack_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsi_rdi_base_rbp_indexed_sib_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsp_base_rbp_index_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rsp_base_rsi_rdi_index_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rbp_base_rsi_rdi_index_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_rbp_base_rbp_index_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && displacement_valid
}

fn is_address32_indexed_no_base_sib_stack_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Dword
        && instruction.length == 9
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index.is_some_and(|register| (register as u8) < 4)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_indexed_no_base_rbp_sib_stack_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Dword
        && instruction.length == 9
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index == Some(od_isa::Register::Rbp)
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_indexed_no_base_rsi_rdi_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Dword
        && instruction.length == 9
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && matches!(
            operand.index,
            Some(od_isa::Register::Rsi | od_isa::Register::Rdi)
        )
        && matches!(operand.scale, 1 | 2 | 4 | 8)
        && i32::try_from(operand.displacement).is_ok()
}

fn is_address32_rsp_sib_stack_base_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => operand.displacement == 0,
        6 => i8::try_from(operand.displacement).is_ok(),
        9 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && displacement_valid
}

fn is_address32_rbp_direct_stack_base_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    let displacement_valid = match instruction.length {
        5 => i8::try_from(operand.displacement).is_ok(),
        8 => i32::try_from(operand.displacement).is_ok(),
        _ => false,
    };
    instruction.address_size == OperandSize::Dword
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && displacement_valid
}

fn is_rsi_rdi_direct_qword_memory_address(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Qword
        && matches!(instruction.length, 3 | 4 | 7)
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            3 => operand.displacement == 0,
            4 => i8::try_from(operand.displacement).is_ok(),
            7 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(
                od_isa::Register::Rsi | od_isa::Register::Rdi
            )
        )
}

fn is_rbp_base_no_index_qword_sib_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(instruction.length, 5 | 8)
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            5 => i8::try_from(operand.displacement).is_ok(),
            8 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_rsp_memory_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    if is_rbp_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_direct_qword_memory_address(instruction, operand) {
        return true;
    }
    if is_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_low_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_low_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsp_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rbp_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rbp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_rbp_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_direct_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_low_base_rsi_rdi_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_low_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_rsi_rdi_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_indexed_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_rbp_indexed_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_base_rbp_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_base_rsi_rdi_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_rsi_rdi_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_rbp_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_rbp_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_sib_stack_base_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_direct_stack_base_operand(instruction, operand) {
        return true;
    }
    if instruction.address_size == OperandSize::Dword
        && instruction.length == 9
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index.is_none()
        && operand.scale == 1
        && i32::try_from(operand.displacement).is_ok()
    {
        return true;
    }
    instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            3 => {
                operand.displacement == 0
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
                    )
            }
            4 => {
                (operand.displacement == 0
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    ))
                    || (i8::try_from(operand.displacement).is_ok()
                        && matches!(
                            operand.base,
                            od_isa::long_decode::AddressBase::Register(register)
                                if (register as u8) < 4
                        ))
                    || (i8::try_from(operand.displacement).is_ok()
                        && matches!(
                            operand.base,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                        ))
            }
            5 => {
                (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    )
            }
            8 => {
                (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    )
                    || (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
            }
            7 => {
                i32::try_from(operand.displacement).is_ok()
                    && (matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(register)
                            if (register as u8) < 4
                    ) || matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                    ) || matches!(operand.base, od_isa::long_decode::AddressBase::NextRip))
            }
            _ => false,
        }
}

fn is_rbp_memory_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    if is_rbp_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_direct_qword_memory_address(instruction, operand) {
        return true;
    }
    if is_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_low_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_low_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_no_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsp_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rsp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rbp_base_rbp_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_rbp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_qword_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_rbp_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_direct_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_low_base_rsi_rdi_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_low_base_no_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_rsi_rdi_index_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_indexed_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsi_rdi_base_rbp_indexed_sib_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_base_rbp_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_base_rsi_rdi_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_rsi_rdi_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_base_rbp_index_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_rbp_sib_stack_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand) {
        return true;
    }
    if is_address32_rsp_sib_stack_base_operand(instruction, operand) {
        return true;
    }
    if is_address32_rbp_direct_stack_base_operand(instruction, operand) {
        return true;
    }
    if instruction.address_size == OperandSize::Dword
        && instruction.length == 9
        && operand.address_size == OperandSize::Dword
        && operand.size == OperandSize::Qword
        && matches!(operand.base, od_isa::long_decode::AddressBase::None)
        && operand.index.is_none()
        && operand.scale == 1
        && i32::try_from(operand.displacement).is_ok()
    {
        return true;
    }
    instruction.address_size == OperandSize::Qword
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && operand.index.is_none()
        && operand.scale == 1
        && ((instruction.length == 3
            && operand.displacement == 0
            && matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
            ))
            || (instruction.length == 4
                && operand.displacement == 0
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                ))
            || (instruction.length == 4
                && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                && (matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(register)
                        if (register as u8) < 4
                ) || matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )))
            || (instruction.length == 5
                && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                ))
            || (instruction.length == 7
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                && (matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(register)
                        if (register as u8) < 4
                ) || matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )))
            || (instruction.length == 8
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                ))
            || (instruction.length == 7
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                && matches!(operand.base, od_isa::long_decode::AddressBase::NextRip))
            || (instruction.length == 8
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                && matches!(operand.base, od_isa::long_decode::AddressBase::None)))
}

fn is_rbp_direct_qword_memory_data_operand(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    instruction.address_size == OperandSize::Qword
        && matches!(instruction.length, 4 | 7)
        && operand.address_size == OperandSize::Qword
        && operand.size == OperandSize::Qword
        && matches!(
            operand.base,
            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
        )
        && operand.index.is_none()
        && operand.scale == 1
        && match instruction.length {
            4 => i8::try_from(operand.displacement).is_ok(),
            7 => i32::try_from(operand.displacement).is_ok(),
            _ => false,
        }
}

fn is_checked_qword_memory_address(
    instruction: Instruction,
    operand: od_isa::long_decode::MemoryOperand,
) -> bool {
    if operand.size != OperandSize::Qword || operand.address_size != instruction.address_size {
        return false;
    }
    match instruction.address_size {
        OperandSize::Dword => {
            let base_supported = matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
            );
            let direct = operand.index.is_none()
                && operand.scale == 1
                && address32_checked_displacement_matches_length(
                    instruction.length,
                    operand.displacement,
                );
            let rsi_rdi_direct = matches!(instruction.length, 4 | 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                )
                && operand.index.is_none()
                && operand.scale == 1
                && address32_checked_displacement_matches_length(
                    instruction.length,
                    operand.displacement,
                );
            let sib = matches!(instruction.length, 5 | 6 | 9)
                && operand.index.is_some_and(|index| (index as u8) < 4)
                && (base_supported
                    || matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    )
                    || matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(
                            od_isa::Register::Rsi | od_isa::Register::Rdi
                        )
                    )
                    || matches!(instruction.length, 6 | 9)
                        && matches!(
                            operand.base,
                            od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                        ))
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && if instruction.length == 5 {
                    operand.displacement == 0
                } else if instruction.length == 6 {
                    (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                } else {
                    (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement)
                };
            let rbp_indexed_sib = matches!(instruction.length, 5 | 6 | 9)
                && base_supported
                && operand.index == Some(od_isa::Register::Rbp)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && if instruction.length == 5 {
                    operand.displacement == 0
                } else if instruction.length == 6 {
                    i8::try_from(operand.displacement).is_ok()
                } else {
                    i32::try_from(operand.displacement).is_ok()
                };
            let rsi_rdi_base_rbp_indexed_sib =
                is_address32_rsi_rdi_base_rbp_indexed_sib_operand(instruction, operand);
            let low_base_rsi_rdi_index_sib =
                is_address32_low_base_rsi_rdi_index_sib_operand(instruction, operand);
            let low_base_no_index_sib =
                is_address32_low_base_no_index_sib_operand(instruction, operand);
            let rsi_rdi_base_rsi_rdi_index_sib =
                is_address32_rsi_rdi_base_rsi_rdi_index_sib_operand(instruction, operand);
            let rsp_base_rbp_indexed_sib = matches!(instruction.length, 5 | 6 | 9)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                && operand.index == Some(od_isa::Register::Rbp)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && if instruction.length == 5 {
                    operand.displacement == 0
                } else if instruction.length == 6 {
                    i8::try_from(operand.displacement).is_ok()
                } else {
                    i32::try_from(operand.displacement).is_ok()
                };
            let rsp_base_rsi_rdi_indexed_sib =
                is_address32_rsp_base_rsi_rdi_index_sib_data_operand(instruction, operand);
            let rbp_base_rsi_rdi_indexed_sib =
                is_address32_rbp_base_rsi_rdi_index_sib_data_operand(instruction, operand);
            let rbp_base_rbp_indexed_sib = matches!(instruction.length, 6 | 9)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                && operand.index == Some(od_isa::Register::Rbp)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && if instruction.length == 6 {
                    i8::try_from(operand.displacement).is_ok()
                } else {
                    i32::try_from(operand.displacement).is_ok()
                };
            let rsp_no_index_sib = matches!(instruction.length, 5 | 6 | 9)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                && operand.index.is_none()
                && operand.scale == 1
                && if instruction.length == 5 {
                    operand.displacement == 0
                } else if instruction.length == 6 {
                    i8::try_from(operand.displacement).is_ok()
                } else {
                    i32::try_from(operand.displacement).is_ok()
                };
            let rbp_direct = matches!(instruction.length, 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                && operand.index.is_none()
                && operand.scale == 1
                && if instruction.length == 5 {
                    i8::try_from(operand.displacement).is_ok()
                } else {
                    i32::try_from(operand.displacement).is_ok()
                };
            let rbp_base_no_index_sib =
                is_address32_rbp_base_no_index_sib_operand(instruction, operand);
            let rsi_rdi_base_no_index_sib =
                is_address32_rsi_rdi_base_no_index_sib_operand(instruction, operand);
            let absolute_sib_disp32 = instruction.length == 9
                && matches!(operand.base, od_isa::long_decode::AddressBase::None)
                && operand.index.is_none()
                && operand.scale == 1
                && i32::try_from(operand.displacement).is_ok();
            let indexed_no_base_sib_disp32 = instruction.length == 9
                && matches!(operand.base, od_isa::long_decode::AddressBase::None)
                && operand.index.is_some_and(|index| (index as u8) < 4)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok();
            let indexed_no_base_rbp_addr32_sib_disp32 = instruction.length == 9
                && matches!(operand.base, od_isa::long_decode::AddressBase::None)
                && operand.index == Some(od_isa::Register::Rbp)
                && matches!(operand.scale, 1 | 2 | 4 | 8)
                && i32::try_from(operand.displacement).is_ok();
            let indexed_no_base_rsi_rdi_addr32_sib_disp32 =
                is_address32_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand);
            instruction.prefixes.address_override
                && (base_supported && direct
                    || rsi_rdi_direct
                    || sib
                    || rbp_indexed_sib
                    || rsp_base_rbp_indexed_sib
                    || rsp_base_rsi_rdi_indexed_sib
                    || rbp_base_rsi_rdi_indexed_sib
                    || rbp_base_rbp_indexed_sib
                    || rsi_rdi_base_rbp_indexed_sib
                    || low_base_rsi_rdi_index_sib
                    || low_base_no_index_sib
                    || rsi_rdi_base_rsi_rdi_index_sib
                    || rsp_no_index_sib
                    || rbp_base_no_index_sib
                    || rsi_rdi_base_no_index_sib
                    || rbp_direct
                    || absolute_sib_disp32
                    || indexed_no_base_sib_disp32
                    || indexed_no_base_rbp_addr32_sib_disp32
                    || indexed_no_base_rsi_rdi_addr32_sib_disp32)
        }
        OperandSize::Qword => {
            let rsi_rdi_direct = matches!(instruction.length, 3 | 4 | 7)
                && operand.index.is_none()
                && operand.scale == 1
                && match instruction.length {
                    3 => operand.displacement == 0,
                    4 => i8::try_from(operand.displacement).is_ok(),
                    7 => i32::try_from(operand.displacement).is_ok(),
                    _ => false,
                }
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                );
            let register_base = matches!(
                operand.base,
                od_isa::long_decode::AddressBase::Register(register) if (register as u8) < 4
            ) || instruction.length == 4
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                || instruction.length == 7
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                    )
                || operand.index.is_some()
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    )
                || operand.index.is_some()
                    && matches!(instruction.length, 5 | 8)
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                    )
                || matches!(instruction.length, 4 | 5 | 8)
                    && operand.index.is_none()
                    && operand.scale == 1
                    && (instruction.length == 4 && operand.displacement == 0
                        || instruction.length == 5
                            && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                        || instruction.length == 8
                            && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement))
                    && matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    );
            let indexed_displacement_valid = match instruction.length {
                4 => operand.displacement == 0,
                5 => (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement),
                8 => (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement),
                _ => false,
            };
            let rsi_rdi_indexed_profile = matches!(instruction.length, 4 | 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(
                        od_isa::Register::Rsi | od_isa::Register::Rdi
                    )
                )
                && operand.index.is_some_and(|index| (index as u8) < 4)
                && indexed_displacement_valid
                && matches!(operand.scale, 1 | 2 | 4 | 8);
            let rbp_indexed_profile = matches!(instruction.length, 4 | 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(register)
                        if (register as u8) < 4
                )
                && operand.index == Some(od_isa::Register::Rbp)
                && indexed_displacement_valid;
            let rsi_rdi_base_rbp_indexed_profile =
                is_rsi_rdi_base_rbp_index_qword_sib_data_operand(instruction, operand);
            let low_base_rsi_rdi_indexed_profile =
                is_low_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand);
            let rsp_base_rbp_indexed_profile = matches!(instruction.length, 4 | 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                )
                && operand.index == Some(od_isa::Register::Rbp)
                && indexed_displacement_valid;
            let rsp_base_rsi_rdi_indexed_profile =
                is_rsp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand);
            let rbp_base_rbp_indexed_profile = matches!(instruction.length, 5 | 8)
                && matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rbp)
                )
                && operand.index == Some(od_isa::Register::Rbp)
                && indexed_displacement_valid;
            let rbp_base_rsi_rdi_indexed_profile =
                is_rbp_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand);
            let rbp_base_no_index_sib_profile =
                is_rbp_base_no_index_qword_sib_data_operand(instruction, operand);
            let rsi_rdi_base_rsi_rdi_indexed_profile =
                is_rsi_rdi_base_rsi_rdi_index_qword_sib_data_operand(instruction, operand);
            let indexed_form = (operand.index.is_some_and(|index| (index as u8) < 4)
                || rbp_indexed_profile
                || rsi_rdi_base_rbp_indexed_profile
                || low_base_rsi_rdi_indexed_profile
                || rsp_base_rbp_indexed_profile
                || rsp_base_rsi_rdi_indexed_profile
                || rbp_base_rbp_indexed_profile
                || rbp_base_rsi_rdi_indexed_profile
                || rsi_rdi_base_rsi_rdi_indexed_profile)
                && indexed_displacement_valid
                && matches!(operand.scale, 1 | 2 | 4 | 8);
            let direct_form = operand.index.is_none()
                && operand.scale == 1
                && (direct_checked_displacement_matches_length(
                    instruction.length,
                    operand.displacement,
                ) || matches!(
                    operand.base,
                    od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                ) && instruction.length == 5
                    && (i8::MIN as i64..=i8::MAX as i64).contains(&operand.displacement)
                    || matches!(
                        operand.base,
                        od_isa::long_decode::AddressBase::Register(od_isa::Register::Rsp)
                    ) && instruction.length == 8
                        && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement));
            let low_base_no_index_sib =
                is_low_base_no_index_qword_sib_data_operand(instruction, operand);
            let rsi_rdi_base_no_index_sib =
                is_rsi_rdi_base_no_index_qword_sib_data_operand(instruction, operand);
            let rsp_base_no_index_sib =
                is_rsp_base_no_index_qword_sib_data_operand(instruction, operand);
            let rip_relative = matches!(operand.base, od_isa::long_decode::AddressBase::NextRip)
                && operand.index.is_none()
                && operand.scale == 1
                && instruction.length == 7
                && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
            let absolute_sib_disp32 =
                matches!(operand.base, od_isa::long_decode::AddressBase::None)
                    && operand.index.is_none()
                    && operand.scale == 1
                    && instruction.length == 8
                    && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
            let indexed_no_base_sib_disp32 =
                matches!(operand.base, od_isa::long_decode::AddressBase::None)
                    && operand.index.is_some_and(|index| (index as u8) < 4)
                    && matches!(operand.scale, 1 | 2 | 4 | 8)
                    && instruction.length == 8
                    && (i32::MIN as i64..=i32::MAX as i64).contains(&operand.displacement);
            let indexed_no_base_rbp_sib_disp32 =
                matches!(operand.base, od_isa::long_decode::AddressBase::None)
                    && operand.index == Some(od_isa::Register::Rbp)
                    && matches!(operand.scale, 1 | 2 | 4 | 8)
                    && instruction.length == 8
                    && i32::try_from(operand.displacement).is_ok();
            let indexed_no_base_rsi_rdi_sib_disp32 =
                is_indexed_no_base_rsi_rdi_sib_data_operand(instruction, operand);
            !instruction.prefixes.address_override
                && (register_base && (direct_form || indexed_form || low_base_no_index_sib)
                    || rsi_rdi_base_no_index_sib
                    || rsp_base_no_index_sib
                    || rsi_rdi_direct
                    || rsi_rdi_indexed_profile
                    || rsi_rdi_base_rbp_indexed_profile
                    || rsi_rdi_base_rsi_rdi_indexed_profile
                    || low_base_rsi_rdi_indexed_profile
                    || rsp_base_rsi_rdi_indexed_profile
                    || rbp_base_no_index_sib_profile
                    || rip_relative
                    || absolute_sib_disp32
                    || indexed_no_base_sib_disp32
                    || indexed_no_base_rbp_sib_disp32
                    || indexed_no_base_rsi_rdi_sib_disp32)
        }
        _ => false,
    }
}

fn exact_direct_qword_load_bytes(code: CodeView<'_>, guest_rip: u64, length: u8) -> bool {
    exact_direct_qword_memory_bytes(code, guest_rip, length, 0x8b)
}

pub(crate) fn exact_direct_qword_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let direct_mode = match bytes.len() {
        3 => bytes.get(2).is_some_and(|modrm| modrm >> 6 == 0),
        4 => {
            bytes.get(2).is_some_and(|modrm| modrm >> 6 == 1)
                || bytes.len() == 4
                    && bytes
                        .get(2)
                        .is_some_and(|modrm| modrm >> 6 == 0 && modrm & 7 == 4)
                    && bytes.get(3).is_some_and(|sib| {
                        *sib == 0x24
                            || sib & 7 < 4 && (sib >> 3) & 7 < 4
                            || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 < 4
                            || matches!(sib & 7, 6 | 7) && (sib >> 3) & 7 == 5
                    })
        }
        7 => bytes.get(2).is_some_and(|modrm| modrm >> 6 == 2),
        8 => bytes
            .get(2)
            .is_some_and(|modrm| modrm >> 6 == 2 && modrm & 7 == 4),
        _ => false,
    };
    (bytes.len() == 3
        && bytes[0..2] == [0x48, opcode]
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 < 4
        && matches!((bytes[2] >> 3) & 7, 4 | 5))
        || (bytes.len() == 3
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 0
            && matches!(bytes[2] & 7, 6 | 7)
            && (bytes[2] >> 3) & 7 <= 5)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 1
            && matches!(bytes[2] & 7, 6 | 7)
            && (bytes[2] >> 3) & 7 <= 5)
        || (bytes.len() == 7
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 2
            && matches!(bytes[2] & 7, 6 | 7)
            && (bytes[2] >> 3) & 7 <= 5)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 1
            && bytes[2] & 7 == 5
            && ((bytes[2] >> 3) & 7 < 4 || bytes[2] == 0x6d))
        || (bytes.len() == 4 && bytes[0..2] == [0x48, opcode] && bytes[2] == 0x65)
        || (bytes.len() == 7
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 2
            && bytes[2] & 7 == 5
            && ((bytes[2] >> 3) & 7 < 4 || bytes[2] == 0xad))
        || (bytes.len() == 7 && bytes[0..2] == [0x48, opcode] && bytes[2] == 0xa5)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] == 0x2c
            && bytes[3] == 0x24)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 0
            && bytes[2] & 7 == 4
            && matches!((bytes[2] >> 3) & 7, 6 | 7)
            && bytes[3] == 0x24)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 0
            && bytes[2] & 7 == 4
            && ((bytes[2] >> 3) & 7 < 4 || bytes[2] == 0x24)
            && bytes[3] == 0x24)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 0
            && bytes[2] & 7 == 4
            && (bytes[2] >> 3) & 7 <= 5
            && matches!(bytes[3] & 7, 6 | 7)
            && ((bytes[3] >> 3) & 7 < 4 || (bytes[3] >> 3) & 7 == 5))
        || (matches!(bytes.len(), 5 | 8)
            && bytes[0..2] == [0x48, opcode]
            && matches!(bytes[2] >> 6, 1 | 2)
            && bytes[2] & 7 == 4
            && (bytes[2] >> 3) & 7 <= 5
            && matches!(bytes[3] & 7, 6 | 7)
            && ((bytes[3] >> 3) & 7 < 4 || (bytes[3] >> 3) & 7 == 5)
            && (bytes.len() == 5 && bytes[2] >> 6 == 1 || bytes.len() == 8 && bytes[2] >> 6 == 2))
        || (bytes.len() == 5
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] == 0x6c
            && bytes[3] == 0x24)
        || (bytes.len() == 5
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 1
            && bytes[2] & 7 == 4
            && ((bytes[2] >> 3) & 7 < 4 || bytes[2] == 0x64)
            && bytes[3] == 0x24)
        || (bytes.len() == 8
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 2
            && bytes[2] & 7 == 4
            && ((bytes[2] >> 3) & 7 < 4 || bytes[2] == 0xa4)
            && bytes[3] == 0x24)
        || (bytes.len() == 8
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] == 0xac
            && bytes[3] == 0x24)
        || (bytes.len() == 5
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 1
            && bytes[2] & 7 == 4
            && matches!((bytes[2] >> 3) & 7, 6 | 7)
            && bytes[3] == 0x24)
        || (bytes.len() == 8
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 2
            && bytes[2] & 7 == 4
            && matches!((bytes[2] >> 3) & 7, 6 | 7)
            && bytes[3] == 0x24)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && matches!(bytes[2], 0x24 | 0x2c)
            && bytes[3] & 7 < 4
            && (bytes[3] >> 3) & 7 < 4)
        || (bytes.len() == 5
            && bytes[0..2] == [0x48, opcode]
            && matches!(bytes[2], 0x64 | 0x6c)
            && bytes[3] & 7 < 4
            && (bytes[3] >> 3) & 7 < 4)
        || (bytes.len() == 8
            && bytes[0..2] == [0x48, opcode]
            && matches!(bytes[2], 0xa4 | 0xac)
            && bytes[3] & 7 < 4
            && (bytes[3] >> 3) & 7 < 4)
        || (bytes.len() == 4
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 1
            && bytes[2] & 7 < 4
            && matches!((bytes[2] >> 3) & 7, 4 | 5))
        || (bytes.len() == 7
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 2
            && bytes[2] & 7 < 4
            && matches!((bytes[2] >> 3) & 7, 4 | 5))
        || (bytes.len() >= 3
            && bytes[0..2] == [0x48, opcode]
            && (bytes[2] & 0b111 <= 3
                || bytes.len() == 4
                    && bytes[2] >> 6 == 0
                    && bytes[2] & 0b111 == 4
                    && bytes[3] & 0b111 < 4
                    && (bytes[3] >> 3) & 0b111 < 4)
            && (bytes[2] >> 3) & 0b111 <= 3
            && direct_mode)
        || (bytes.len() == 7
            && bytes[0..2] == [0x48, opcode]
            && bytes[2] >> 6 == 0
            && bytes[2] & 0b111 == 5
            && matches!((bytes[2] >> 3) & 0b111, 0..=5))
}

pub(crate) fn exact_rip_relative_qword_rsi_rdi_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes.len() == 7
        && length == 7
        && bytes.get(0..2) == Some(&[0x48, opcode])
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 == 5
        && matches!((bytes[2] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_direct_qword_store_rsi_rdi_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes.len() == usize::from(length)
        && bytes.get(0..2) == Some(&[0x48, 0x89])
        && matches!(length, 3 | 4 | 7)
        && matches!(bytes[2] >> 6, 0 | 1 | 2)
        && (length == 3 && bytes[2] >> 6 == 0
            || length == 4 && bytes[2] >> 6 == 1
            || length == 7 && bytes[2] >> 6 == 2)
        && matches!((bytes[2] >> 3) & 7, 6 | 7)
        && (bytes[2] & 7 < 4 || matches!(length, 4 | 7) && bytes[2] & 7 == 5)
}

pub(crate) fn exact_direct_qword_load_rsi_rdi_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes.len() == usize::from(length)
        && bytes.get(0..2) == Some(&[0x48, 0x8b])
        && matches!(length, 3 | 4 | 7)
        && (length == 3 && bytes[2] >> 6 == 0
            || length == 4 && bytes[2] >> 6 == 1
            || length == 7 && bytes[2] >> 6 == 2)
        && matches!((bytes[2] >> 3) & 7, 6 | 7)
        && (bytes[2] & 7 < 4 || matches!(length, 4 | 7) && bytes[2] & 7 == 5)
}

pub(crate) fn exact_qword_address32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let expected_mode = match bytes.len() {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    bytes[0..3] == [0x67, 0x48, opcode]
        && bytes[3] >> 6 == expected_mode
        && ((bytes[3] >> 3) & 7 <= 5
            && (bytes[3] & 7 < 4
                || expected_mode != 0 && bytes[3] & 7 == 5
                || matches!(bytes[3] & 7, 6 | 7))
            || (bytes[3] >> 3) & 7 >= 6 && bytes[3] & 7 < 4)
}

pub(crate) fn exact_qword_address32_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let expected_mode = match bytes.len() {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    bytes[0..3] == [0x67, 0x48, opcode]
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && (bytes[4] == 0x24
            || bytes[4] & 7 < 4 && (bytes[4] >> 3) & 7 < 4
            || bytes[4] & 7 == 4 && (bytes[4] >> 3) & 7 < 4
            || matches!(bytes[4] & 7, 6 | 7) && (bytes[4] >> 3) & 7 < 4
            || matches!(expected_mode, 1 | 2) && bytes[4] & 7 == 5 && (bytes[4] >> 3) & 7 < 4)
}

pub(crate) fn exact_qword_address32_low_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && bytes[4] & 7 < 4
        && matches!((bytes[4] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_address32_low_base_no_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0..3] == [0x67, 0x48, opcode]
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && matches!(bytes[4], 0x20..=0x23)
}

pub(crate) fn exact_qword_address32_rsi_rdi_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && matches!(bytes[4] & 7, 6 | 7)
        && matches!((bytes[4] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_sib_displacement_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let expected_mode = match bytes.len() {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    bytes[0..2] == [0x48, opcode]
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && (bytes[3] & 7 <= 4
            || expected_mode != 0 && bytes[3] & 7 == 5
            || matches!(bytes[3] & 7, 6 | 7) && (bytes[3] >> 3) & 7 == 5)
        && ((bytes[3] >> 3) & 7 < 4 || matches!(bytes[3] & 7, 6 | 7) && (bytes[3] >> 3) & 7 == 5)
}

pub(crate) fn exact_qword_low_base_no_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && matches!(bytes[3], 0x20..=0x23)
}

pub(crate) fn exact_qword_rsi_rdi_base_no_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && matches!(bytes[3], 0x26 | 0x27)
}

pub(crate) fn exact_qword_rbp_base_no_index_sib_disp_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && bytes[3] == 0x25
}

pub(crate) fn exact_qword_address32_rbp_base_no_index_sib_disp_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && bytes[4] == 0x25
}

pub(crate) fn exact_qword_address32_rsi_rdi_base_no_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && matches!(bytes[4], 0x26 | 0x27)
}

pub(crate) fn exact_qword_absolute_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 8 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(8)) else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && bytes[3] == 0x25
}

pub(crate) fn exact_qword_address32_absolute_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 9 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(9)) else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == 0
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && bytes[4] == 0x25
}

pub(crate) fn exact_qword_address32_indexed_no_base_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 9 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(9)) else {
        return false;
    };
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == 0
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 < 4
}

pub(crate) fn exact_qword_address32_indexed_no_base_rbp_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 9 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(9)) else {
        return false;
    };
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == 0
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_address32_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 9 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(9)) else {
        return false;
    };
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == 0
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && sib & 7 == 5
        && matches!((sib >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_indexed_no_base_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 8 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(8)) else {
        return false;
    };
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 < 4
}

pub(crate) fn exact_qword_indexed_no_base_rbp_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 8 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(8)) else {
        return false;
    };
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_indexed_no_base_rsi_rdi_sib_disp32_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    if length != 8 {
        return false;
    }
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code.bytes.get(offset..offset.saturating_add(8)) else {
        return false;
    };
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == 0
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && sib & 7 == 5
        && matches!((sib >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[2];
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 < 4
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_rsp_base_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[2];
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 4
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_rsp_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[2];
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 4
        && matches!((sib >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_rbp_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[2];
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 5
        && matches!((sib >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_rbp_base_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[2];
    let sib = bytes[3];
    bytes[0] == 0x48
        && bytes[1] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_address32_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[3];
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 < 4
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_address32_rsi_rdi_base_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && matches!(bytes[4] & 7, 6 | 7)
        && (bytes[4] >> 3) & 7 == 5
}

pub(crate) fn exact_qword_low_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && bytes[3] & 7 < 4
        && matches!((bytes[3] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_rsi_rdi_base_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        4 => 0,
        5 => 1,
        8 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x48
        && bytes[1] == opcode
        && bytes[2] >> 6 == expected_mode
        && bytes[2] & 7 == 4
        && (bytes[2] >> 3) & 7 <= 5
        && matches!(bytes[3] & 7, 6 | 7)
        && matches!((bytes[3] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_address32_rsp_base_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[3];
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 4
        && (sib >> 3) & 7 == 5
}

pub(crate) fn exact_qword_address32_rsp_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        5 => 0,
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && bytes[3] >> 6 == expected_mode
        && bytes[3] & 7 == 4
        && (bytes[3] >> 3) & 7 <= 5
        && bytes[4] & 7 == 4
        && matches!((bytes[4] >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_address32_rbp_base_rsi_rdi_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[3];
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 5
        && matches!((sib >> 3) & 7, 6 | 7)
}

pub(crate) fn exact_qword_address32_rbp_base_rbp_index_sib_memory_bytes(
    code: CodeView<'_>,
    guest_rip: u64,
    length: u8,
    opcode: u8,
) -> bool {
    let expected_mode = match length {
        6 => 1,
        9 => 2,
        _ => return false,
    };
    let Some(offset) = guest_rip
        .checked_sub(code.base_rip)
        .and_then(|value| usize::try_from(value).ok())
    else {
        return false;
    };
    let Some(bytes) = code
        .bytes
        .get(offset..offset.saturating_add(usize::from(length)))
    else {
        return false;
    };
    let modrm = bytes[3];
    let sib = bytes[4];
    bytes[0] == 0x67
        && bytes[1] == 0x48
        && bytes[2] == opcode
        && modrm >> 6 == expected_mode
        && modrm & 7 == 4
        && (modrm >> 3) & 7 <= 5
        && sib & 7 == 5
        && (sib >> 3) & 7 == 5
}

fn address32_checked_displacement_matches_length(length: u8, displacement: i64) -> bool {
    match length {
        4 => displacement == 0,
        5 => (i8::MIN as i64..=i8::MAX as i64).contains(&displacement),
        8 => (i32::MIN as i64..=i32::MAX as i64).contains(&displacement),
        _ => false,
    }
}

fn direct_checked_displacement_matches_length(length: u8, displacement: i64) -> bool {
    match length {
        3 => displacement == 0,
        4 => (i8::MIN as i64..=i8::MAX as i64).contains(&displacement),
        7 => (i32::MIN as i64..=i32::MAX as i64).contains(&displacement),
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

pub(crate) struct CodeMemory<'a> {
    code: CodeView<'a>,
    read_start_rip: u64,
    read_end_rip: u64,
}

impl<'a> CodeMemory<'a> {
    fn limited(code: CodeView<'a>, read_start_rip: u64, read_end_rip: u64) -> Self {
        Self {
            code,
            read_start_rip,
            read_end_rip,
        }
    }

    pub(crate) fn full_code(code: CodeView<'a>) -> Self {
        Self::limited(code, code.base_rip(), code.code_end())
    }

    pub(crate) fn one_instruction(code: CodeView<'a>, rip: u64) -> Option<Self> {
        let read_end_rip = rip.checked_add(1)?;
        Self::bounded_range(code, rip, read_end_rip)
    }

    pub(crate) fn bounded_range(
        code: CodeView<'a>,
        read_start_rip: u64,
        read_end_rip: u64,
    ) -> Option<Self> {
        if read_start_rip >= read_end_rip
            || read_start_rip < code.base_rip()
            || read_end_rip > code.code_end()
        {
            return None;
        }
        Some(Self::limited(code, read_start_rip, read_end_rip))
    }

    fn unavailable(address: u64, access: AccessKind) -> MemoryError {
        MemoryError::Unmapped { address, access }
    }

    fn unsupported_data_access() -> MemoryError {
        MemoryError::Backend("block CodeView supports instruction fetch only".into())
    }
}

impl GuestMemory for CodeMemory<'_> {
    fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
        if address < self.read_start_rip || address >= self.read_end_rip {
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
