//! Small, project-owned ARM64 lowering for the S0-09 x86 fragment profile.
//!
//! Arithmetic and control flow are emitted as native ARM64 instructions.
//! A small host helper materializes the defined x86 arithmetic flags.

use std::collections::HashMap;

use od_isa::{DecodeError, FragmentState as CpuState, Instruction, Operation, decode};

const RIP_OFFSET: u32 = std::mem::offset_of!(CpuState, rip) as u32;
const RFLAGS_OFFSET: u32 = std::mem::offset_of!(CpuState, rflags) as u32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JitError {
    Decode(DecodeError),
    CodeTooLarge,
    InvalidEntry,
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
    let offsets: HashMap<usize, usize> = instructions
        .iter()
        .enumerate()
        .map(|(index, instruction)| (instruction.offset, index))
        .collect();
    let mut emitter = Emitter::default();
    // Preserve the one callee-saved register holding CpuState and establish
    // the budget and instruction counters. The generated function is
    // fn(*mut CpuState, u64, *mut u64)->u32.
    emitter.emit(0xA9BF_53F3); // STP X19, X20, [SP, #-16]!
    emitter.emit(0xA9BF_7BF5); // STP X21, X30, [SP, #-16]!
    emitter.emit(0xA9BF_7BF6); // STP X22, X30, [SP, #-16]!
    emitter.emit(mov_reg(19, 0));
    emitter.emit(mov_reg(15, 1));
    emitter.emit(mov_reg(21, 2));
    emitter.emit(mov_reg(14, 31));
    emitter.emit(mov_reg(22, 3));

    let mut start_patches = Vec::with_capacity(instructions.len() + 1);
    for instruction in instructions {
        emitter.emit(ldr_x(9, 19, RIP_OFFSET));
        emitter.emit_mov_imm64(10, instruction.offset as u64);
        emitter.emit(subs_x(31, 9, 10)); // CMP X9, X10
        let patch = emitter.emit(0x5400_0000); // B.EQ guest block
        start_patches.push((patch, instruction.offset));
    }
    emitter.emit(ldr_x(9, 19, RIP_OFFSET));
    emitter.emit_mov_imm64(10, code_length as u64);
    emitter.emit(subs_x(31, 9, 10));
    let start_at_end = emitter.emit(0x5400_0000); // B.EQ complete
    start_patches.push((start_at_end, code_length));
    let invalid_start_branch = emitter.emit(0x1400_0000); // B invalid_entry

    let mut guest_to_host = Vec::with_capacity(instructions.len());
    let mut branch_patches = Vec::new();
    for instruction in instructions {
        let host_word = emitter.words.len();
        guest_to_host.push((instruction.offset, host_word));
        emitter.emit_mov_imm64(9, instruction.offset as u64);
        emitter.emit(str_x(9, 19, RIP_OFFSET));
        emitter.emit(subs_imm(15, 15, 1));
        let budget_patch = emitter.emit(0x5400_0003); // B.LO budget_exit
        branch_patches.push((budget_patch, PatchKind::Budget));
        emitter.emit(ldarb_w(9, 22));
        let stop_patch = emitter.emit(0x3500_0009); // CBNZ W9 stop_exit
        branch_patches.push((stop_patch, PatchKind::Stopped));
        emitter.emit(adds_imm(14, 14, 1));

        match instruction.operation {
            Operation::MovImm { dst, value } => {
                emitter.emit_mov_imm64(9, value);
                emitter.emit(str_x(9, 19, dst as u32 * 8));
            }
            Operation::Add { dst, src } | Operation::Sub { dst, src } => {
                let lhs = dst as u32;
                let rhs = src as u32;
                emitter.emit(ldr_x(9, 19, lhs * 8));
                emitter.emit(ldr_x(10, 19, rhs * 8));
                emitter.emit(if matches!(instruction.operation, Operation::Add { .. }) {
                    add_x(11, 9, 10)
                } else {
                    sub_x(11, 9, 10)
                });
                emitter.emit(str_x(11, 19, lhs * 8));
                emit_flag_helper_call(
                    &mut emitter,
                    matches!(instruction.operation, Operation::Add { .. }) as u32,
                );
            }
            Operation::Cmp { lhs, rhs } => {
                emitter.emit(ldr_x(9, 19, lhs as u32 * 8));
                emitter.emit(ldr_x(10, 19, rhs as u32 * 8));
                emitter.emit(sub_x(11, 9, 10));
                emit_flag_helper_call(&mut emitter, 2);
            }
            Operation::Jz { displacement } | Operation::Jnz { displacement } => {
                let target = (instruction.next_offset() as i64 + i64::from(displacement)) as usize;
                emitter.emit(ldr_x(9, 19, RFLAGS_OFFSET));
                emitter.emit_mov_imm64(10, 1 << 6);
                emitter.emit(and_x(9, 9, 10));
                let is_jz = matches!(instruction.operation, Operation::Jz { .. });
                let patch = emitter.emit(if is_jz { 0xB500_0009 } else { 0xB400_0009 }); // CBNZ/CBZ X9
                branch_patches.push((patch, PatchKind::Guest(target)));
            }
        }
    }

    let complete = emitter.words.len();
    emitter.emit_mov_imm64(9, code_length as u64);
    emitter.emit(str_x(9, 19, RIP_OFFSET));
    emitter.emit_mov_imm64(0, 0);
    emitter.emit(str_x(14, 21, 0));
    let epilogue = emitter.words.len();
    emitter.emit(0xA8C1_7BF6); // LDP X22, X30, [SP], #16
    emitter.emit(0xA8C1_7BF5); // LDP X21, X30, [SP], #16
    emitter.emit(0xA8C1_53F3); // LDP X19, X20, [SP], #16
    emitter.emit(0xD65F_03C0); // RET
    let budget_exit = emitter.words.len();
    emitter.emit_mov_imm64(0, 1);
    emitter.emit(str_x(14, 21, 0));
    emitter.emit(0xA8C1_7BF6);
    emitter.emit(0xA8C1_7BF5);
    emitter.emit(0xA8C1_53F3);
    emitter.emit(0xD65F_03C0);
    let stop_exit = emitter.words.len();
    emitter.emit_mov_imm64(0, 3);
    emitter.emit(str_x(14, 21, 0));
    emitter.emit(0xA8C1_7BF6);
    emitter.emit(0xA8C1_7BF5);
    emitter.emit(0xA8C1_53F3);
    emitter.emit(0xD65F_03C0);
    let invalid_entry = emitter.words.len();
    emitter.emit_mov_imm64(0, 2);
    emitter.emit(str_x(14, 21, 0));
    emitter.emit(0xA8C1_7BF6);
    emitter.emit(0xA8C1_7BF5);
    emitter.emit(0xA8C1_53F3);
    emitter.emit(0xD65F_03C0);

    for (patch, guest_offset) in start_patches {
        let target = if guest_offset == code_length {
            complete
        } else {
            let index = *offsets.get(&guest_offset).ok_or(JitError::InvalidEntry)?;
            guest_to_host[index].1
        };
        emitter.patch_conditional(patch, target)?;
    }
    emitter.patch_unconditional(invalid_start_branch, invalid_entry)?;

    for (patch, kind) in branch_patches {
        let target = match kind {
            PatchKind::Budget => budget_exit,
            PatchKind::Stopped => stop_exit,
            PatchKind::Guest(target) if target == code_length => complete,
            PatchKind::Guest(target) => {
                let index = *offsets.get(&target).ok_or(JitError::InvalidEntry)?;
                // Each guest block starts with saving its RIP to CpuState.
                guest_to_host[index].1
            }
        };
        emitter.patch_conditional(patch, target)?;
    }

    // Keep the epilogue's location represented for debugging even when no
    // guest branch targets it directly.
    let _ = epilogue;
    Ok(JitOutput {
        host_code: emitter.words,
        guest_to_host,
    })
}

#[derive(Clone, Copy)]
enum PatchKind {
    Budget,
    Stopped,
    Guest(usize),
}

#[derive(Default)]
struct Emitter {
    words: Vec<u32>,
}

impl Emitter {
    fn emit(&mut self, word: u32) -> usize {
        let index = self.words.len();
        self.words.push(word);
        index
    }

    fn emit_mov_imm64(&mut self, dst: u32, value: u64) {
        self.emit(mov_imm16(dst, value as u16, 0, false));
        for half in 1..4 {
            let imm = ((value >> (half * 16)) & 0xFFFF) as u16;
            if imm != 0 {
                self.emit(mov_imm16(dst, imm, half as u32, true));
            }
        }
    }

    fn patch_conditional(&mut self, at: usize, target: usize) -> Result<(), JitError> {
        let delta = target as i64 - at as i64;
        if !(-(1 << 18)..(1 << 18)).contains(&delta) {
            return Err(JitError::CodeTooLarge);
        }
        self.words[at] = (self.words[at] & !0x00FF_FFE0) | (((delta as u32) & 0x7_FFFF) << 5);
        Ok(())
    }

    fn patch_unconditional(&mut self, at: usize, target: usize) -> Result<(), JitError> {
        let delta = target as i64 - at as i64;
        if !(-(1 << 25)..(1 << 25)).contains(&delta) {
            return Err(JitError::CodeTooLarge);
        }
        self.words[at] = 0x1400_0000 | ((delta as u32) & 0x03FF_FFFF);
        Ok(())
    }
}

fn mov_reg(dst: u32, src: u32) -> u32 {
    0xAA00_03E0 | (src << 16) | dst // ORR Xd, XZR, Xm
}

fn mov_imm16(dst: u32, value: u16, half: u32, keep: bool) -> u32 {
    (if keep { 0xF280_0000 } else { 0xD280_0000 }) | (half << 21) | ((value as u32) << 5) | dst
}

fn emit_flag_helper_call(emitter: &mut Emitter, operation: u32) {
    emitter.emit(mov_reg(0, 19));
    emitter.emit(mov_reg(1, 9));
    emitter.emit(mov_reg(2, 10));
    emitter.emit(mov_reg(3, 11));
    emitter.emit(mov_imm16(4, operation as u16, 0, false));
    let address = update_arithmetic_flags as *const () as usize as u64;
    for half in 0..4 {
        let imm = ((address >> (half * 16)) & 0xFFFF) as u32;
        emitter.emit(mov_imm16(16, imm as u16, half, half != 0));
    }
    emitter.emit(0xD63F_0200); // BLR X16
    emitter.emit(mov_reg(0, 19)); // restore state pointer for following code
}

fn ldr_x(rt: u32, rn: u32, byte_offset: u32) -> u32 {
    0xF940_0000 | ((byte_offset / 8) << 10) | (rn << 5) | rt
}

fn ldarb_w(rt: u32, rn: u32) -> u32 {
    0x08DF_FC00 | (rn << 5) | rt
}

fn str_x(rt: u32, rn: u32, byte_offset: u32) -> u32 {
    0xF900_0000 | ((byte_offset / 8) << 10) | (rn << 5) | rt
}

fn add_x(rd: u32, rn: u32, rm: u32) -> u32 {
    0x8B00_0000 | (rm << 16) | (rn << 5) | rd
}

fn sub_x(rd: u32, rn: u32, rm: u32) -> u32 {
    0xCB00_0000 | (rm << 16) | (rn << 5) | rd
}

fn and_x(rd: u32, rn: u32, rm: u32) -> u32 {
    0x8A00_0000 | (rm << 16) | (rn << 5) | rd
}

fn subs_imm(rd: u32, rn: u32, imm12: u32) -> u32 {
    0xF100_0000 | (imm12 << 10) | (rn << 5) | rd
}

fn subs_x(rd: u32, rn: u32, rm: u32) -> u32 {
    0xEB00_0000 | (rm << 16) | (rn << 5) | rd
}

fn adds_imm(rd: u32, rn: u32, imm12: u32) -> u32 {
    0xB100_0000 | (imm12 << 10) | (rn << 5) | rd
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
        assert!(output.host_code.contains(&ldr_x(9, 19, RIP_OFFSET)));
        assert!(output.host_code.contains(&str_x(9, 19, RIP_OFFSET)));
        assert!(output.host_code.contains(&ldr_x(9, 19, RFLAGS_OFFSET)));
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
    fn compile_rejects_invalid_control_flow() {
        assert!(matches!(
            compile(&[0x74, 0x01]),
            Err(JitError::Decode(DecodeError::InvalidBranchTarget { .. }))
        ));
    }
}
