//! Small, project-owned ARM64 lowering for the S0-09 x86 fragment profile.
//!
//! Arithmetic and control flow are emitted as native ARM64 instructions.
//! A small host helper materializes the defined x86 arithmetic flags.

use std::collections::HashMap;

use od_isa::{DecodeError, FragmentState as CpuState, Instruction, Operation, decode};

pub mod allocator;
pub mod arm64;
pub mod atomic_adapter;
pub mod block;
pub mod block_compile;
pub mod block_emitter;
pub mod dispatch;
pub mod emitter;
pub mod ir;
pub mod lowering;
pub mod memory_bridge;
pub mod memory_cache;
pub mod memory_dispatch;
pub mod memory_load_v2;
pub mod phase_observation;
pub mod runtime;
pub mod statepoints;
pub mod translation;

const RIP_OFFSET: u32 = std::mem::offset_of!(CpuState, rip) as u32;
const RFLAGS_OFFSET: u32 = std::mem::offset_of!(CpuState, rflags) as u32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JitError {
    Decode(DecodeError),
    CodeTooLarge,
    InvalidEntry,
    Encoding(arm64::EncodeError),
}

impl From<arm64::EncodeError> for JitError {
    fn from(error: arm64::EncodeError) -> Self {
        match error {
            arm64::EncodeError::BranchOutOfRange { .. } => Self::CodeTooLarge,
            error => Self::Encoding(error),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JitOutput {
    pub host_code: Vec<u32>,
    /// Maps each decoded guest byte offset to the first generated ARM64 word.
    pub guest_to_host: Vec<(usize, usize)>,
}

pub fn compile(code: &[u8]) -> Result<JitOutput, JitError> {
    let instructions = decode(code).map_err(JitError::Decode)?;
    compile_decoded(code.len(), &instructions)
}

fn compile_decoded(
    code_length: usize,
    instructions: &[Instruction],
) -> Result<JitOutput, JitError> {
    use arm64::{AddressBase, Condition, Encoder, PairIndexMode, WReg, XRegOrSp, XRegOrZero};

    let offsets: HashMap<usize, usize> = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.offset, index))
        .collect();
    let mut encoder = Encoder::new();
    let guest_labels: Vec<_> = (0..=instructions.len())
        .map(|_| encoder.new_label())
        .collect::<Result<_, _>>()?;
    let code_end_label = guest_labels[instructions.len()];
    let budget_exit = encoder.new_label()?;
    let stop_exit = encoder.new_label()?;
    let invalid_entry = encoder.new_label()?;

    // Preserve the legacy fragment prologue and its generated word sequence.
    encoder.stp_x(
        xr(19)?,
        xr(20)?,
        AddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.stp_x(
        xr(21)?,
        xr(30)?,
        AddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.stp_x(
        xr(22)?,
        xr(30)?,
        AddressBase::Sp,
        -16,
        PairIndexMode::PreIndex,
    )?;
    encoder.mov_x(xr(19)?, XRegOrZero::Reg(xr(0)?));
    encoder.mov_x(xr(15)?, XRegOrZero::Reg(xr(1)?));
    encoder.mov_x(xr(21)?, XRegOrZero::Reg(xr(2)?));
    encoder.mov_x(xr(14)?, XRegOrZero::Zero);
    encoder.mov_x(xr(22)?, XRegOrZero::Reg(xr(3)?));

    for (index, instruction) in instructions.iter().enumerate() {
        encoder.ldr_x(
            XRegOrZero::Reg(xr(9)?),
            AddressBase::Reg(xr(19)?),
            RIP_OFFSET,
        )?;
        emit_mov_imm64(&mut encoder, 10, instruction.offset as u64)?;
        encoder.subs_x(XRegOrZero::Zero, xr(9)?, XRegOrZero::Reg(xr(10)?));
        encoder.b_cond(Condition::Equal, guest_labels[index])?;
    }
    encoder.ldr_x(
        XRegOrZero::Reg(xr(9)?),
        AddressBase::Reg(xr(19)?),
        RIP_OFFSET,
    )?;
    emit_mov_imm64(&mut encoder, 10, code_length as u64)?;
    encoder.subs_x(XRegOrZero::Zero, xr(9)?, XRegOrZero::Reg(xr(10)?));
    encoder.b_cond(Condition::Equal, code_end_label)?;
    encoder.b(invalid_entry)?;

    let mut guest_to_host = Vec::with_capacity(instructions.len());
    for (index, instruction) in instructions.iter().enumerate() {
        encoder.bind(guest_labels[index])?;
        guest_to_host.push((instruction.offset, encoder.position()));
        emit_mov_imm64(&mut encoder, 9, instruction.offset as u64)?;
        encoder.str_x(
            XRegOrZero::Reg(xr(9)?),
            AddressBase::Reg(xr(19)?),
            RIP_OFFSET,
        )?;
        encoder.subs_imm_x(XRegOrZero::Reg(xr(15)?), XRegOrSp::Reg(xr(15)?), 1)?;
        encoder.b_cond(Condition::Lower, budget_exit)?;
        encoder.ldarb_w(WReg::new(9)?, AddressBase::Reg(xr(22)?));
        encoder.cbnz_w(WReg::new(9)?, stop_exit)?;
        encoder.adds_imm_x(XRegOrZero::Reg(xr(14)?), XRegOrSp::Reg(xr(14)?), 1)?;

        match instruction.operation {
            Operation::MovImm { dst, value } => {
                emit_mov_imm64(&mut encoder, 9, value)?;
                encoder.str_x(
                    XRegOrZero::Reg(xr(9)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(dst as u8) * 8,
                )?;
            }
            Operation::Add { dst, src } | Operation::Sub { dst, src } => {
                encoder.ldr_x(
                    XRegOrZero::Reg(xr(9)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(dst as u8) * 8,
                )?;
                encoder.ldr_x(
                    XRegOrZero::Reg(xr(10)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(src as u8) * 8,
                )?;
                if matches!(instruction.operation, Operation::Add { .. }) {
                    encoder.add_x(xr(11)?, xr(9)?, XRegOrZero::Reg(xr(10)?));
                } else {
                    encoder.sub_x(xr(11)?, xr(9)?, XRegOrZero::Reg(xr(10)?));
                }
                encoder.str_x(
                    XRegOrZero::Reg(xr(11)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(dst as u8) * 8,
                )?;
                emit_flag_helper_call(
                    &mut encoder,
                    matches!(instruction.operation, Operation::Add { .. }) as u32,
                )?;
            }
            Operation::Cmp { lhs, rhs } => {
                encoder.ldr_x(
                    XRegOrZero::Reg(xr(9)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(lhs as u8) * 8,
                )?;
                encoder.ldr_x(
                    XRegOrZero::Reg(xr(10)?),
                    AddressBase::Reg(xr(19)?),
                    u32::from(rhs as u8) * 8,
                )?;
                encoder.sub_x(xr(11)?, xr(9)?, XRegOrZero::Reg(xr(10)?));
                emit_flag_helper_call(&mut encoder, 2)?;
            }
            Operation::Jz { displacement } | Operation::Jnz { displacement } => {
                let target = (instruction.next_offset() as i64 + i64::from(displacement)) as usize;
                let label = if target == code_length {
                    code_end_label
                } else {
                    let index = *offsets.get(&target).ok_or(JitError::InvalidEntry)?;
                    guest_labels[index]
                };
                encoder.ldr_x(
                    XRegOrZero::Reg(xr(9)?),
                    AddressBase::Reg(xr(19)?),
                    RFLAGS_OFFSET,
                )?;
                emit_mov_imm64(&mut encoder, 10, 1 << 6)?;
                encoder.and_x(xr(9)?, xr(9)?, XRegOrZero::Reg(xr(10)?));
                if matches!(instruction.operation, Operation::Jz { .. }) {
                    encoder.cbnz_x(XRegOrZero::Reg(xr(9)?), label)?;
                } else {
                    encoder.cbz_x(XRegOrZero::Reg(xr(9)?), label)?;
                }
            }
        }
    }

    encoder.bind(code_end_label)?;
    emit_mov_imm64(&mut encoder, 9, code_length as u64)?;
    encoder.str_x(
        XRegOrZero::Reg(xr(9)?),
        AddressBase::Reg(xr(19)?),
        RIP_OFFSET,
    )?;
    encoder.movz_x(xr(0)?, 0, 0)?;
    encoder.str_x(XRegOrZero::Reg(xr(14)?), AddressBase::Reg(xr(21)?), 0)?;
    emit_epilogue(&mut encoder)?;

    encoder.bind(budget_exit)?;
    emit_mov_imm64(&mut encoder, 0, 1)?;
    encoder.str_x(XRegOrZero::Reg(xr(14)?), AddressBase::Reg(xr(21)?), 0)?;
    emit_epilogue(&mut encoder)?;

    encoder.bind(stop_exit)?;
    emit_mov_imm64(&mut encoder, 0, 3)?;
    encoder.str_x(XRegOrZero::Reg(xr(14)?), AddressBase::Reg(xr(21)?), 0)?;
    emit_epilogue(&mut encoder)?;

    encoder.bind(invalid_entry)?;
    emit_mov_imm64(&mut encoder, 0, 2)?;
    encoder.str_x(XRegOrZero::Reg(xr(14)?), AddressBase::Reg(xr(21)?), 0)?;
    emit_epilogue(&mut encoder)?;

    Ok(JitOutput {
        host_code: encoder.finish()?.into_words(),
        guest_to_host,
    })
}

fn xr(number: u8) -> Result<arm64::XReg, JitError> {
    Ok(arm64::XReg::new(number)?)
}

fn emit_mov_imm64(encoder: &mut arm64::Encoder, dst: u8, value: u64) -> Result<(), JitError> {
    let dst = xr(dst)?;
    encoder.movz_x(dst, value as u16, 0)?;
    for half in 1..4 {
        let immediate = ((value >> (half * 16)) & 0xffff) as u16;
        if immediate != 0 {
            encoder.movk_x(dst, immediate, half as u8)?;
        }
    }
    Ok(())
}

fn emit_epilogue(encoder: &mut arm64::Encoder) -> Result<(), JitError> {
    use arm64::{AddressBase, PairIndexMode};
    encoder.ldp_x(
        xr(22)?,
        xr(30)?,
        AddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ldp_x(
        xr(21)?,
        xr(30)?,
        AddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ldp_x(
        xr(19)?,
        xr(20)?,
        AddressBase::Sp,
        16,
        PairIndexMode::PostIndex,
    )?;
    encoder.ret(xr(30)?);
    Ok(())
}

fn emit_flag_helper_call(encoder: &mut arm64::Encoder, operation: u32) -> Result<(), JitError> {
    use arm64::{XReg, XRegOrZero};
    encoder.mov_x(xr(0)?, XRegOrZero::Reg(xr(19)?));
    encoder.mov_x(xr(1)?, XRegOrZero::Reg(xr(9)?));
    encoder.mov_x(xr(2)?, XRegOrZero::Reg(xr(10)?));
    encoder.mov_x(xr(3)?, XRegOrZero::Reg(xr(11)?));
    encoder.movz_x(xr(4)?, operation as u16, 0)?;
    let address = update_arithmetic_flags as *const () as usize as u64;
    for half in 0..4 {
        let immediate = ((address >> (half * 16)) & 0xffff) as u16;
        if half == 0 {
            encoder.movz_x(xr(16)?, immediate, 0)?;
        } else {
            encoder.movk_x(xr(16)?, immediate, half as u8)?;
        }
    }
    encoder.blr(XReg::new(16)?);
    encoder.mov_x(xr(0)?, XRegOrZero::Reg(xr(19)?));
    Ok(())
}

unsafe extern "C" fn update_arithmetic_flags(
    state: *mut CpuState,
    lhs: u64,
    rhs: u64,
    result: u64,
    operation: u64,
) {
    let state = unsafe { &mut *state };
    const FLAGS: u64 = (1 << 0) | (1 << 2) | (1 << 4) | (1 << 6) | (1 << 7) | (1 << 11);
    let add = operation == 1;
    let carry = if add { result < lhs } else { lhs < rhs };
    let overflow = if add {
        (!(lhs ^ rhs) & (lhs ^ result)) >> 63 != 0
    } else {
        ((lhs ^ rhs) & (lhs ^ result)) >> 63 != 0
    };
    let mut flags = state.rflags & !FLAGS;
    if carry {
        flags |= 1 << 0;
    }
    if (result as u8).count_ones() % 2 == 0 {
        flags |= 1 << 2;
    }
    if (lhs ^ rhs ^ result) & 0x10 != 0 {
        flags |= 1 << 4;
    }
    if result == 0 {
        flags |= 1 << 6;
    }
    if result >> 63 != 0 {
        flags |= 1 << 7;
    }
    if overflow {
        flags |= 1 << 11;
    }
    state.rflags = flags | (1 << 1);
}

/// Invoke a compiled fragment in an executable mapping owned by the caller.
///
/// # Safety
/// `entry` must point to live executable memory containing the entry point of
/// the `JitOutput` produced by `compile`, and the mapping must remain alive for
/// the duration of this call.
pub unsafe fn execute_at(
    entry: *const u8,
    state: &mut CpuState,
    budget: u64,
    stop_requested: *const u8,
) -> Result<od_isa::ExecutionResult, JitError> {
    if entry.is_null() || stop_requested.is_null() {
        return Err(JitError::InvalidEntry);
    }
    type Entry = unsafe extern "C" fn(*mut CpuState, u64, *mut u64, *const u8) -> u32;
    let function: Entry = unsafe { std::mem::transmute(entry) };
    let mut instructions_executed = 0;
    let reason = unsafe { function(state, budget, &mut instructions_executed, stop_requested) };
    if reason == 2 {
        return Err(JitError::InvalidEntry);
    }
    Ok(od_isa::ExecutionResult {
        state: state.clone(),
        reason: match reason {
            0 => od_isa::ExitReason::ReachedCodeEnd,
            1 => od_isa::ExitReason::InstructionBudgetExhausted,
            3 => od_isa::ExitReason::StopRequested,
            _ => return Err(JitError::InvalidEntry),
        },
        instructions_executed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragment_layout_matches_generated_loads_and_stores() {
        assert_eq!(std::mem::offset_of!(CpuState, gpr), 0);
        assert_eq!(std::mem::size_of::<CpuState>(), 48);
        assert_eq!(RIP_OFFSET, 32);
        assert_eq!(RFLAGS_OFFSET, 40);
        assert_eq!(std::mem::size_of::<usize>(), 8);
        let output = compile(&[0x48, 0x03, 0xc1, 0x74, 0]).unwrap();
        assert!(output.host_code.contains(&0xF940_1269)); // LDR X9, [X19, #32]
        assert!(output.host_code.contains(&0xF900_1269)); // STR X9, [X19, #32]
        assert!(output.host_code.contains(&0xF940_1669)); // LDR X9, [X19, #40]
    }

    #[test]
    fn extended_register_forms_are_still_outside_fragment_jit() {
        for code in [
            &[0x49, 0xb8, 0, 0, 0, 0, 0, 0, 0, 0][..],
            &[0x48, 0x03, 0xc4],
            &[0xdb, 0x00],
            &[0xd9, 0x10],
            &[0xd9, 0x18],
            &[0xdd, 0x10],
            &[0xdd, 0x18],
            &[0xdb, 0x38],
            &[0xf3, 0xae],
            &[0xf3, 0xa6],
        ] {
            assert!(matches!(compile(code), Err(JitError::Decode(_))));
        }
        assert_eq!(CpuState::default().register(od_isa::Register::R15), None);
    }

    #[test]
    fn bounded_fcmovnu_is_not_admitted_to_the_fragment_jit() {
        assert!(matches!(compile(&[0xdb, 0xd9]), Err(JitError::Decode(_))));
    }

    #[test]
    fn bounded_x87_fsqrt_is_not_admitted_to_the_fragment_jit() {
        assert!(matches!(compile(&[0xd9, 0xfa]), Err(JitError::Decode(_))));
    }

    #[test]
    fn bounded_x87_fprem_is_not_admitted_to_the_fragment_jit() {
        assert!(matches!(compile(&[0xd9, 0xf8]), Err(JitError::Decode(_))));
    }

    #[test]
    fn bounded_x87_fprem1_is_not_admitted_to_the_fragment_jit() {
        assert!(matches!(compile(&[0xd9, 0xf5]), Err(JitError::Decode(_))));
    }

    #[test]
    fn swapgs_is_not_admitted_to_the_fragment_jit() {
        assert!(matches!(
            compile(&[0x0f, 0x01, 0xf8]),
            Err(JitError::Decode(_))
        ));
    }

    #[test]
    fn emits_guest_offset_map_and_native_words() {
        let code = [0x48, 0xB8, 7, 0, 0, 0, 0, 0, 0, 0];
        let output = compile(&code).unwrap();
        assert_eq!(output.guest_to_host.len(), 1);
        assert_eq!(output.guest_to_host[0].0, 0);
        assert!(!output.host_code.is_empty());
        assert_eq!(output.host_code[0], 0xA9BF_53F3);
    }

    #[test]
    fn legacy_mov_fragment_output_matches_the_existing_word_vector() {
        let code = [0x48, 0xb8, 1, 0, 0, 0, 0, 0, 0, 0];
        let output = compile(&code).unwrap();
        assert_eq!(output.guest_to_host, vec![(0, 17)]);
        assert_eq!(
            output.host_code,
            vec![
                0xa9bf_53f3,
                0xa9bf_7bf5,
                0xa9bf_7bf6,
                0xaa00_03f3,
                0xaa01_03ef,
                0xaa02_03f5,
                0xaa1f_03ee,
                0xaa03_03f6,
                0xf940_1269,
                0xd280_000a,
                0xeb0a_013f,
                0x5400_00c0,
                0xf940_1269,
                0xd280_014a,
                0xeb0a_013f,
                0x5400_0160,
                0x1400_001e,
                0xd280_0009,
                0xf900_1269,
                0xf100_05ef,
                0x5400_01c3,
                0x08df_fec9,
                0x3500_0249,
                0xb100_05ce,
                0xd280_0029,
                0xf900_0269,
                0xd280_0149,
                0xf900_1269,
                0xd280_0000,
                0xf900_02ae,
                0xa8c1_7bf6,
                0xa8c1_7bf5,
                0xa8c1_53f3,
                0xd65f_03c0,
                0xd280_0020,
                0xf900_02ae,
                0xa8c1_7bf6,
                0xa8c1_7bf5,
                0xa8c1_53f3,
                0xd65f_03c0,
                0xd280_0060,
                0xf900_02ae,
                0xa8c1_7bf6,
                0xa8c1_7bf5,
                0xa8c1_53f3,
                0xd65f_03c0,
                0xd280_0040,
                0xf900_02ae,
                0xa8c1_7bf6,
                0xa8c1_7bf5,
                0xa8c1_53f3,
                0xd65f_03c0,
            ]
        );
    }

    #[test]
    fn compile_rejects_invalid_control_flow() {
        assert!(matches!(
            compile(&[0x74, 0x01]),
            Err(JitError::Decode(DecodeError::InvalidBranchTarget { .. }))
        ));
    }
}
