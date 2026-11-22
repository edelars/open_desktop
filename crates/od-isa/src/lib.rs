//! A deliberately small x86-64 fragment decoder and reference interpreter.
//!
//! This crate implements only the register-only S0-09 forms documented in
//! `docs/specs/CPU_CONTRACT.md`. It is not an x86 emulator or a boot CPU.

use std::collections::HashMap;

pub mod binary_float;
pub mod cli;
pub mod cpuid;
pub mod cr8;
pub mod execute;
mod finite_arithmetic;
pub mod hlt;
pub mod integer_flags;
pub mod interrupt;
pub mod iretq;
pub mod legacy_mode;
pub mod long_decode;
pub mod long_mode_entry;
pub mod lretq;
pub mod memory;
pub mod mmx_int;
pub mod msr;
pub mod paging;
pub mod port_io;
pub mod protected_exec;
mod rational_rounding;
pub mod real_mode_exec;
pub mod sep;
pub mod sse2_int;
mod sse_fp;
pub mod state;
pub mod sti;
pub mod swapgs;
pub mod syscall;
pub mod x87;
pub mod x87_transcend;
pub use cli::CliProfile;
pub use execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, Cpl0ProfileSetExit, Cpl0ProfileSetInputError,
    Cpl0ProfileSetResult, Cr0Profile, FlatLidtLong64V1, GuestFpState, InvlpgExit, InvlpgLong64V1,
    InvlpgResult, InvlpgWpLong64V1, IretqUserEntry, MovCr0WpLong64V1, MovCr3Exit, MovCr3InputError,
    MovCr3Long64V1, MovCr3Result, MovCr3WpLong64V1, PagedCpl0Cr0WpMovV1, PagedCpl0Cr0WpProbeV1,
    PagedCpl0Cr3ProbeV1, PagedCpl0PageTableRepairMovV1, PagedHltLong64V1, PagedLgdtLong64V1,
    PagedLidtLong64V1, PagedMovDwordStoreLong64V1, PagedProbeExit, PagedProbeRejection,
    PagedProbeResult, PagedStiLong64V1, TranslationContextChange, TranslationInvalidation,
    step_flat_lidt, step_invlpg, step_invlpg_wp, step_mov_cr0_wp, step_mov_cr3, step_mov_cr3_wp,
    step_paged_cr0_wp_mov, step_paged_cr0_wp_page_table_repair_mov, step_paged_cr0_wp_probe,
    step_paged_cr3_probe, step_paged_hlt, step_paged_lgdt, step_paged_lidt,
    step_paged_mov_dword_store, step_paged_sti,
};
pub use hlt::HaltProfile;
pub use interrupt::{
    Cpl0ExternalInterruptDelivery, Cpl0ExternalInterruptPhase, Cpl0ExternalInterruptRefusal,
    Cpl0GeneralProtectionDelivery, Cpl0GeneralProtectionPhase, Cpl0GeneralProtectionRefusal,
    Cpl0Int3Delivery, Cpl0Int3Phase, Cpl0Int3Refusal, Cpl0PageFaultDelivery,
    Cpl0PageFaultDeliveryPhase, Cpl0PageFaultRefusal, InterruptDeliveryPhase,
    InterruptDeliveryResult, InterruptFrameSlot, InterruptOutsideProfile, NMI_VECTOR,
    PageFaultDeliveryOutsideProfile, PageFaultDeliveryPhase, PageFaultDeliveryResult,
    PageFaultFrameSlot, deliver_double_fault, deliver_external_interrupt_cpl0_set,
    deliver_general_protection_cpl0_set, deliver_int3_cpl0_set, deliver_maskable_interrupt,
    deliver_nmi_cpl0_set, deliver_page_fault, deliver_page_fault_cpl0_set,
    deliver_paged_maskable_interrupt,
};
pub use iretq::{
    IretqProfile, IretqReadPhase, IretqResult, IretqRunExit, IretqRunResult,
    run_with_iretq_profile, step_with_iretq_profile,
};
pub use lretq::{
    LretqProfile, LretqReadPhase, LretqResult, LretqRunExit, LretqRunResult,
    run_with_lretq_profile, step_with_lretq_profile,
};
pub use memory::{
    AccessContext, AccessKind, AccessOrigin, ArchitecturalFault, DoubleFaultRecord, FenceKind,
    GuestMemory, InstructionMode, MemoryError, PageFaultRecord, PrivilegeLevel,
    page_fault_error_code,
};
pub use port_io::PortIo;
pub use state::{
    CodeSegmentDescriptorCache, CpuActivity, DataSegmentDescriptorCache, DsDescriptorCache,
    FragmentAdapterError, FsDescriptorCache, LongModeState, OperandSize, RegisterError,
    RegisterView, StackSegmentDescriptorCache, StiShadow, TaskRegisterCache,
    validate_tss_descriptor,
};
pub use sti::StiProfile;
pub use swapgs::SwapgsProfile;

const CF: u64 = 1 << 0;
const PF: u64 = 1 << 2;
const AF: u64 = 1 << 4;
const ZF: u64 = 1 << 6;
const SF: u64 = 1 << 7;
const OF: u64 = 1 << 11;
const DF: u64 = 1 << 10;
const ARITHMETIC_FLAGS: u64 = CF | PF | AF | ZF | SF | OF;
/// RFLAGS bits admitted and observed by the current long-mode integer profile:
/// the six status flags, DF, and the architecturally fixed bit 1.
pub const INTEGER_FLAGS: u64 = ARITHMETIC_FLAGS | DF | 2;
/// Profile flags writable by the implemented long-mode `POPFQ` slice.
pub const POPFQ_INTEGER_FLAGS: u64 = ARITHMETIC_FLAGS | DF;
/// MXCSR_MASK of the selected synthetic Generic amd64 target, stored in plain
/// FXSAVE images and used by LDMXCSR and FXRSTOR64 to raise `#GP(0)` for a set bit
/// outside it. By the owner's "main priority" rule (merge 56c60c9) DAZ (bit 6) is
/// NOT in the mask: 0xFFBF is the S7-31i value and ADR-0014's 0xFFFF is
/// superseded (the oracle i5-3210M accepts DAZ; see KNOWN_ISSUES ISSUE-680). This is
/// intentionally distinct from testkit diagnostic metadata.
pub const TARGET_FXSAVE_MXCSR_MASK: u32 = 0x0000_ffbf;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Register {
    Rax = 0,
    Rcx = 1,
    Rdx = 2,
    Rbx = 3,
    Rsp = 4,
    Rbp = 5,
    Rsi = 6,
    Rdi = 7,
    R8 = 8,
    R9 = 9,
    R10 = 10,
    R11 = 11,
    R12 = 12,
    R13 = 13,
    R14 = 14,
    R15 = 15,
}

impl Register {
    pub const fn from_encoding(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Rax),
            1 => Some(Self::Rcx),
            2 => Some(Self::Rdx),
            3 => Some(Self::Rbx),
            4 => Some(Self::Rsp),
            5 => Some(Self::Rbp),
            6 => Some(Self::Rsi),
            7 => Some(Self::Rdi),
            8 => Some(Self::R8),
            9 => Some(Self::R9),
            10 => Some(Self::R10),
            11 => Some(Self::R11),
            12 => Some(Self::R12),
            13 => Some(Self::R13),
            14 => Some(Self::R14),
            15 => Some(Self::R15),
            _ => None,
        }
    }

    fn from_fragment_encoding(value: u8) -> Option<Self> {
        if value < 4 {
            Self::from_encoding(value)
        } else {
            None
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    MovImm { dst: Register, value: u64 },
    Add { dst: Register, src: Register },
    Sub { dst: Register, src: Register },
    Cmp { lhs: Register, rhs: Register },
    Jz { displacement: i8 },
    Jnz { displacement: i8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Instruction {
    pub offset: usize,
    pub length: usize,
    pub operation: Operation,
}

impl Instruction {
    pub fn next_offset(self) -> usize {
        self.offset + self.length
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeError {
    Truncated { offset: usize },
    UnsupportedOpcode { offset: usize, opcode: u8 },
    UnsupportedEncoding { offset: usize },
    InvalidBranchTarget { offset: usize, target: i64 },
}

pub fn decode(code: &[u8]) -> Result<Vec<Instruction>, DecodeError> {
    let mut result = Vec::new();
    let mut offset = 0;
    while offset < code.len() {
        let start = offset;
        let first = code[offset];
        let instruction = if first == 0x48 {
            offset += 1;
            let opcode = *code
                .get(offset)
                .ok_or(DecodeError::Truncated { offset: start })?;
            match opcode {
                0xB8..=0xBB => {
                    let dst = Register::from_fragment_encoding(opcode - 0xB8)
                        .ok_or(DecodeError::UnsupportedEncoding { offset: start })?;
                    offset += 1;
                    let immediate = code
                        .get(offset..offset + 8)
                        .ok_or(DecodeError::Truncated { offset: start })?;
                    let value = u64::from_le_bytes(immediate.try_into().expect("eight-byte slice"));
                    offset += 8;
                    Operation::MovImm { dst, value }
                }
                0x03 | 0x2B | 0x3B => {
                    offset += 1;
                    let modrm = *code
                        .get(offset)
                        .ok_or(DecodeError::Truncated { offset: start })?;
                    offset += 1;
                    if modrm >> 6 != 0b11 {
                        return Err(DecodeError::UnsupportedEncoding { offset: start });
                    }
                    let dst = Register::from_fragment_encoding((modrm >> 3) & 7)
                        .ok_or(DecodeError::UnsupportedEncoding { offset: start })?;
                    let src = Register::from_fragment_encoding(modrm & 7)
                        .ok_or(DecodeError::UnsupportedEncoding { offset: start })?;
                    match opcode {
                        0x03 => Operation::Add { dst, src },
                        0x2B => Operation::Sub { dst, src },
                        _ => Operation::Cmp { lhs: dst, rhs: src },
                    }
                }
                _ => {
                    return Err(DecodeError::UnsupportedOpcode {
                        offset: start,
                        opcode,
                    });
                }
            }
        } else {
            match first {
                0x74 | 0x75 => {
                    offset += 1;
                    let displacement = *code
                        .get(offset)
                        .ok_or(DecodeError::Truncated { offset: start })?
                        as i8;
                    offset += 1;
                    if first == 0x74 {
                        Operation::Jz { displacement }
                    } else {
                        Operation::Jnz { displacement }
                    }
                }
                opcode => {
                    return Err(DecodeError::UnsupportedOpcode {
                        offset: start,
                        opcode,
                    });
                }
            }
        };
        result.push(Instruction {
            offset: start,
            length: offset - start,
            operation: instruction,
        });
    }

    validate_branch_targets(&result, code.len())?;
    Ok(result)
}

fn branch_target(instruction: Instruction, displacement: i8) -> i64 {
    instruction.next_offset() as i64 + i64::from(displacement)
}

fn validate_branch_targets(
    instructions: &[Instruction],
    code_length: usize,
) -> Result<(), DecodeError> {
    let boundaries: HashMap<usize, ()> = instructions.iter().map(|i| (i.offset, ())).collect();
    for instruction in instructions {
        let displacement = match instruction.operation {
            Operation::Jz { displacement } | Operation::Jnz { displacement } => displacement,
            _ => continue,
        };
        let target = branch_target(*instruction, displacement);
        if target < 0
            || (target as usize != code_length && !boundaries.contains_key(&(target as usize)))
        {
            return Err(DecodeError::InvalidBranchTarget {
                offset: instruction.offset,
                target,
            });
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct FragmentState {
    pub gpr: [u64; 4],
    pub rip: usize,
    /// Only CF/PF/AF/ZF/SF/OF are modeled. Reserved bit 1 is always set.
    pub rflags: u64,
}

/// Compatibility name for the S0-09 buffer API. New integer state uses
/// `LongModeState`; conversion back to this four-register profile is fallible.
pub type CpuState = FragmentState;

impl Default for FragmentState {
    fn default() -> Self {
        Self {
            gpr: [0; 4],
            rip: 0,
            rflags: 1 << 1,
        }
    }
}

impl FragmentState {
    pub fn register(&self, register: Register) -> Option<u64> {
        self.gpr.get(register as usize).copied()
    }

    pub fn zero_flag(&self) -> bool {
        self.rflags & ZF != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitReason {
    ReachedCodeEnd,
    InstructionBudgetExhausted,
    StopRequested,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionResult {
    pub state: CpuState,
    pub reason: ExitReason,
    pub instructions_executed: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExecuteError {
    InvalidInstructionPointer {
        rip: usize,
    },
    /// The validated compatibility profile and the core disagreed; this is an
    /// implementation error, not a manufactured guest exception.
    UnexpectedCoreExit {
        exit: execute::RunExit,
    },
}

pub fn interpret(
    code: &[u8],
    state: &mut CpuState,
    budget: u64,
) -> Result<ExecutionResult, DecodeErrorOrExecute> {
    let instructions = decode(code).map_err(DecodeErrorOrExecute::Decode)?;
    interpret_decoded(code, &instructions, state, budget)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DecodeErrorOrExecute {
    Decode(DecodeError),
    Execute(ExecuteError),
}

fn interpret_decoded(
    code: &[u8],
    instructions: &[Instruction],
    state: &mut CpuState,
    budget: u64,
) -> Result<ExecutionResult, DecodeErrorOrExecute> {
    // Preserve S0's static boundaries and its budget-before-entry-validation rule.
    if state.rip != code.len() && budget != 0 && !instructions.iter().any(|i| i.offset == state.rip)
    {
        return Err(DecodeErrorOrExecute::Execute(
            ExecuteError::InvalidInstructionPointer { rip: state.rip },
        ));
    }
    let mut core = LongModeState::from(&*state);
    let result = execute::run(
        &mut core,
        &mut FragmentCode(code),
        execute::RunConfig {
            instruction_budget: budget,
            stop_rip: Some(code.len() as u64),
            context: memory::AccessContext::default(),
        },
        || false,
    );
    // Only the statically validated four-register forms can reach this adapter;
    // all original flag bits are retained, including caller-supplied unmodeled bits.
    state.gpr.copy_from_slice(&core.gpr[..4]);
    state.rip = usize::try_from(core.rip).map_err(|_| {
        DecodeErrorOrExecute::Execute(ExecuteError::UnexpectedCoreExit {
            exit: result.exit.clone(),
        })
    })?;
    state.rflags = core.rflags;
    let reason = match result.exit {
        execute::RunExit::StopAddress => ExitReason::ReachedCodeEnd,
        execute::RunExit::InstructionBudgetExhausted => ExitReason::InstructionBudgetExhausted,
        exit => {
            return Err(DecodeErrorOrExecute::Execute(
                ExecuteError::UnexpectedCoreExit { exit },
            ));
        }
    };
    Ok(ExecutionResult {
        state: state.clone(),
        reason,
        instructions_executed: result.instructions_executed,
    })
}

struct FragmentCode<'a>(&'a [u8]);

impl memory::GuestMemory for FragmentCode<'_> {
    fn fetch(
        &mut self,
        address: u64,
        _context: memory::AccessContext,
    ) -> Result<u8, memory::MemoryError> {
        usize::try_from(address)
            .ok()
            .and_then(|index| self.0.get(index))
            .copied()
            .ok_or(memory::MemoryError::Unmapped {
                address,
                access: memory::AccessKind::Fetch,
            })
    }

    fn read(
        &mut self,
        _address: u64,
        _context: memory::AccessContext,
        _destination: &mut [u8],
    ) -> Result<(), memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code cannot read data memory".into(),
        ))
    }

    fn write(
        &mut self,
        _address: u64,
        _context: memory::AccessContext,
        _source: &[u8],
    ) -> Result<(), memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code cannot write data memory".into(),
        ))
    }

    fn atomic_exchange_integer(
        &mut self,
        _address: u64,
        _context: memory::AccessContext,
        _size: OperandSize,
        _replacement: u64,
    ) -> Result<u64, memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code cannot access data memory".into(),
        ))
    }

    fn atomic_fetch_add_integer(
        &mut self,
        _address: u64,
        _context: memory::AccessContext,
        _size: OperandSize,
        _addend: u64,
    ) -> Result<u64, memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code cannot access data memory".into(),
        ))
    }

    fn atomic_compare_exchange_integer(
        &mut self,
        _address: u64,
        _context: memory::AccessContext,
        _size: OperandSize,
        _expected: u64,
        _replacement: u64,
    ) -> Result<u64, memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code cannot access data memory".into(),
        ))
    }

    fn mfence(&mut self, _context: memory::AccessContext) -> Result<(), memory::MemoryError> {
        Err(memory::MemoryError::Backend(
            "validated S0 code has no memory fence backend".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arithmetic_flags_cover_carry_zero_and_signed_overflow() {
        let mut state = CpuState::default();
        state.gpr[0] = u64::MAX;
        state.gpr[1] = 1;
        let add = [0x48, 0x03, 0xC1];
        interpret(&add, &mut state, 4).unwrap();
        assert_eq!(state.gpr[0], 0);
        assert_eq!(state.rflags & (CF | PF | ZF), CF | PF | ZF);

        let mut state = CpuState::default();
        state.gpr[0] = i64::MAX as u64;
        state.gpr[1] = 1;
        interpret(&add, &mut state, 4).unwrap();
        assert_eq!(state.rflags & (SF | OF), SF | OF);

        let mut state = CpuState::default();
        state.gpr[0] = 0xFF;
        state.gpr[1] = 1;
        interpret(&add, &mut state, 4).unwrap();
        assert_eq!(state.gpr[0], 0x100);
        assert_ne!(state.rflags & PF, 0, "PF uses only the low result byte");
    }

    #[test]
    fn dependent_sub_jnz_loop_stops_at_budget() {
        let code = [0x48, 0x2B, 0xC1, 0x75, 0xFB]; // sub rax, rcx; jnz -5
        let mut state = CpuState::default();
        state.gpr[0] = 3;
        state.gpr[1] = 1;
        let result = interpret(&code, &mut state, 8).unwrap();
        assert_eq!(result.reason, ExitReason::ReachedCodeEnd);
        assert_eq!(result.state.gpr[0], 0);
        assert_eq!(result.state.rip, code.len());
        assert_eq!(result.instructions_executed, 6);
    }

    #[test]
    fn unsupported_and_truncated_forms_are_distinct() {
        assert_eq!(
            decode(&[0x48, 0xB8, 1]),
            Err(DecodeError::Truncated { offset: 0 })
        );
        assert_eq!(
            decode(&[0x90]),
            Err(DecodeError::UnsupportedOpcode {
                offset: 0,
                opcode: 0x90
            })
        );
    }

    #[test]
    fn every_truncated_prefix_of_supported_encodings_is_rejected_before_execution() {
        let mov_imm64 = [0x48, 0xB8, 1, 2, 3, 4, 5, 6, 7, 8];
        for length in 1..mov_imm64.len() {
            let mut state = CpuState {
                gpr: [11, 22, 33, 44],
                rip: 0,
                rflags: 1 << 1,
            };
            let before = state.clone();
            assert_eq!(
                interpret(&mov_imm64[..length], &mut state, 8),
                Err(DecodeErrorOrExecute::Decode(DecodeError::Truncated {
                    offset: 0
                }))
            );
            assert_eq!(state, before, "truncated length {length} changed state");
        }

        for code in [&[0x48][..], &[0x48, 0x03][..], &[0x74][..]] {
            assert_eq!(
                decode(code),
                Err(DecodeError::Truncated { offset: 0 }),
                "truncated bytes {code:02x?}"
            );
        }
        assert_eq!(decode(&mov_imm64).unwrap()[0].length, 10);
    }

    #[test]
    fn empty_input_and_unsupported_max_length_prefix_have_defined_results() {
        assert_eq!(decode(&[]), Ok(Vec::new()));
        let mut state = CpuState::default();
        let result = interpret(&[], &mut state, 1).unwrap();
        assert_eq!(result.reason, ExitReason::ReachedCodeEnd);
        assert_eq!(result.instructions_executed, 0);

        let fifteen_byte_legacy_prefix_sequence = [0x66; 15];
        assert_eq!(
            decode(&fifteen_byte_legacy_prefix_sequence),
            Err(DecodeError::UnsupportedOpcode {
                offset: 0,
                opcode: 0x66,
            })
        );
    }

    #[test]
    fn rejects_memory_modrm_invalid_branch_targets_and_non_boundary_entry_rip() {
        assert_eq!(
            decode(&[0x48, 0x03, 0x00]),
            Err(DecodeError::UnsupportedEncoding { offset: 0 })
        );
        assert_eq!(
            decode(&[0x74, 0x01]),
            Err(DecodeError::InvalidBranchTarget {
                offset: 0,
                target: 3,
            })
        );
        assert!(decode(&[0x74, 0xFE]).is_ok());

        let mut state = CpuState {
            rip: 1,
            ..CpuState::default()
        };
        assert_eq!(
            interpret(&[0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0], &mut state, 1),
            Err(DecodeErrorOrExecute::Execute(
                ExecuteError::InvalidInstructionPointer { rip: 1 }
            ))
        );
    }

    #[test]
    fn branch_into_instruction_body_is_rejected() {
        let code = [0x74, 0x01, 0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(matches!(
            decode(&code),
            Err(DecodeError::InvalidBranchTarget { .. })
        ));
    }

    #[test]
    fn cmp_jz_program_exercises_conditional_branch() {
        let code = [
            0x48, 0xB8, 5, 0, 0, 0, 0, 0, 0, 0, 0x48, 0xB9, 5, 0, 0, 0, 0, 0, 0, 0, 0x48, 0x3B,
            0xC1, 0x74, 0x00,
        ];
        let mut state = CpuState::default();
        let result = interpret(&code, &mut state, 8).unwrap();
        assert_eq!(result.reason, ExitReason::ReachedCodeEnd);
        assert_eq!(result.instructions_executed, 4);
        assert!(result.state.zero_flag());
        assert_eq!(result.state.gpr[0], 5);
    }

    #[test]
    fn backward_branch_respects_instruction_budget() {
        let code = [0x75, 0xFE]; // jnz to itself; initial ZF is clear.
        let mut state = CpuState::default();
        let result = interpret(&code, &mut state, 5).unwrap();
        assert_eq!(result.reason, ExitReason::InstructionBudgetExhausted);
        assert_eq!(result.instructions_executed, 5);
        assert_eq!(result.state.rip, 0);
    }
}
