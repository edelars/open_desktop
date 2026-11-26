//! S6-41b: ARM64 code for one guest instruction made of the atomic-family IR
//! effects (`AtomicRmw`, `AtomicFlags`, `Fence`, `WriteGprIfNotEqual`, plus
//! `Constant`, `ReadGpr` and `WriteGpr`), including the six arithmetic flags.
//!
//! This is the part of the inline atomic emitter that needs no knowledge of
//! guest address translation. Its contract with the caller (the future inline
//! memory emitter, S6-41b (б), which does not exist yet):
//!
//! - `env.address` holds the host pointer of the memory operand, already
//!   translated, naturally aligned for the operand width, permission-checked
//!   and known to be writable. Nothing in this module can fault after the
//!   atomic operation, so the guest instruction cannot be left half done
//!   (`VerifyError::AtomicRmwOrder`);
//! - the caller bumps the page generation and reports the committed write
//!   after this sequence (S6-40, obligation 1); this module does neither;
//! - `env.state` points at the guest `LongModeState` (GPRs and RFLAGS are read
//!   and written at `LONG_MODE_*_OFFSET`);
//! - registers `x2..=x16` are clobbered; `env.state` and `env.address` must lie
//!   outside that range.
//!
//! Flags: `lower_atomic_flags` computes the six defined arithmetic flags
//! inline from the old memory value and the operand with plain integer
//! instructions (no helper call), for the widths 8, 16, 32 and 64, with the
//! semantics of `od_isa`'s `arithmetic_result`/`logic_result`. The sequence
//! emitter merges them into RFLAGS under `AtomicFlagsKind::written_mask`.

use std::collections::HashMap;

use crate::arm64::memory_order::{
    HostOperands, MemoryOrderError, MemoryOrderMode, lower_memory_effect,
};
use crate::arm64::{
    AccessWidth, AddressBase, Condition, EncodeError, Encoder, Shift, XReg, XRegOrZero,
};
use crate::ir::{self, AtomicFlagsKind, GuestGpr, IntWidth, Operation, Program, TempId};
use crate::runtime::{LONG_MODE_GPR_OFFSET, LONG_MODE_RFLAGS_OFFSET};

/// Registers and mode of one emitted sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SequenceEnv {
    pub mode: MemoryOrderMode,
    /// Pointer to the guest `LongModeState`.
    pub state: XReg,
    /// Host address of the memory operand.
    pub address: XReg,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SequenceError {
    Encode(EncodeError),
    MemoryOrder(MemoryOrderError),
    InvalidProgram(ir::VerifyError),
    /// The program has not exactly one guest instruction.
    NotOneInstruction,
    /// An effect this emitter does not lower (explicit refusal, never a skip).
    UnsupportedEffect {
        operation: usize,
    },
    /// A register write of a width other than `W32`/`W64`.
    UnsupportedWidth {
        operation: usize,
        width: IntWidth,
    },
    /// More temporaries than host registers in the pool.
    TooManyTemporaries,
    UnknownTemporary(TempId),
    /// `env.state` or `env.address` is in the clobbered range `x2..=x16`.
    EnvOverlapsScratch,
}

impl From<EncodeError> for SequenceError {
    fn from(error: EncodeError) -> Self {
        Self::Encode(error)
    }
}

impl From<MemoryOrderError> for SequenceError {
    fn from(error: MemoryOrderError) -> Self {
        Self::MemoryOrder(error)
    }
}

/// Registers of one flag derivation. `lhs`/`rhs` hold the operands
/// zero-extended to the operation width; `out` receives the flag bits (only
/// `kind.written_mask()` bits can be set); `scratch` is clobbered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FlagsRegs {
    pub lhs: XReg,
    pub rhs: Option<XReg>,
    pub out: XReg,
    pub scratch: [XReg; 5],
}

const fn width_bits(width: IntWidth) -> u8 {
    match width {
        IntWidth::W8 => 8,
        IntWidth::W16 => 16,
        IntWidth::W32 => 32,
        IntWidth::W64 => 64,
    }
}

const fn access_width(width: IntWidth) -> AccessWidth {
    match width {
        IntWidth::W8 => AccessWidth::Byte,
        IntWidth::W16 => AccessWidth::Half,
        IntWidth::W32 => AccessWidth::Word,
        IntWidth::W64 => AccessWidth::Dword,
    }
}

fn reg(register: XReg) -> XRegOrZero {
    XRegOrZero::Reg(register)
}

/// Emit the flag derivation of `kind` into `regs.out`.
///
/// Add: `CF = carry out`, `OF = overflow`, `AF = (a ^ b ^ r) bit 4`,
/// `PF = even parity of the low byte of r`, `ZF`, `SF`, with
/// `r = (a + b) mod 2^width`; subtraction uses the borrow and the subtraction
/// overflow; logic kinds clear `CF` and `OF` and leave `AF` out of the mask.
pub fn lower_atomic_flags(
    encoder: &mut Encoder,
    kind: AtomicFlagsKind,
    width: IntWidth,
    regs: &FlagsRegs,
) -> Result<(), SequenceError> {
    let bits = width_bits(width);
    let top = bits - 1;
    let a = regs.lhs;
    let [res, s0, s1, s2, one] = regs.scratch;
    let out = regs.out;
    encoder.mov_x(out, XRegOrZero::Zero);

    if let AtomicFlagsKind::BitTest { bit } = kind {
        // CF = bit `bit % width` of the old value.
        encoder.ubfx_x(s0, a, bit % bits, 1)?;
        encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 0)?;
        return Ok(());
    }
    let b = regs.rhs.expect("a kind with an rhs is given one");

    match kind {
        AtomicFlagsKind::Add | AtomicFlagsKind::AddKeepCarry => {
            encoder.add_x(res, a, reg(b));
        }
        AtomicFlagsKind::Subtract | AtomicFlagsKind::SubtractKeepCarry => {
            encoder.sub_x(res, a, reg(b));
        }
        AtomicFlagsKind::And => encoder.and_x(res, a, reg(b)),
        AtomicFlagsKind::Or => encoder.orr_x(res, reg(a), reg(b), Shift::Lsl, 0)?,
        AtomicFlagsKind::BitTest { .. } => unreachable!("handled above"),
    }
    if bits < 64 {
        encoder.lsl_imm_x(res, res, 64 - bits)?;
        encoder.lsr_imm_x(res, res, 64 - bits)?;
    }

    // SF (bit 7).
    encoder.ubfx_x(s0, res, top, 1)?;
    encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 7)?;
    // ZF (bit 6).
    encoder.subs_imm_x(XRegOrZero::Zero, crate::arm64::XRegOrSp::Reg(res), 0)?;
    encoder.cset_x(s0, Condition::Equal);
    encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 6)?;
    // PF (bit 2): set when the low byte of the result has even parity.
    encoder.ubfx_x(s0, res, 0, 8)?;
    encoder.eor_x(s0, reg(s0), reg(s0), Shift::Lsr, 4)?;
    encoder.eor_x(s0, reg(s0), reg(s0), Shift::Lsr, 2)?;
    encoder.eor_x(s0, reg(s0), reg(s0), Shift::Lsr, 1)?;
    encoder.ubfx_x(s0, s0, 0, 1)?;
    encoder.movz_x(one, 1, 0)?;
    encoder.eor_x(s0, reg(s0), reg(one), Shift::Lsl, 0)?;
    encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 2)?;

    let arithmetic = matches!(
        kind,
        AtomicFlagsKind::Add
            | AtomicFlagsKind::AddKeepCarry
            | AtomicFlagsKind::Subtract
            | AtomicFlagsKind::SubtractKeepCarry
    );
    if !arithmetic {
        return Ok(());
    }
    let subtract = matches!(
        kind,
        AtomicFlagsKind::Subtract | AtomicFlagsKind::SubtractKeepCarry
    );

    // AF (bit 4): bit 4 of a ^ b ^ r.
    encoder.eor_x(s0, reg(a), reg(b), Shift::Lsl, 0)?;
    encoder.eor_x(s0, reg(s0), reg(res), Shift::Lsl, 0)?;
    encoder.ubfx_x(s0, s0, 4, 1)?;
    encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 4)?;

    // CF (bit 0), when the kind writes it.
    if kind.written_mask() & 1 != 0 {
        if subtract {
            // borrow = (~a & b) | (~(a ^ b) & r), taken at the top bit.
            encoder.orn_x(s0, XRegOrZero::Zero, reg(a), Shift::Lsl, 0)?;
            encoder.and_x(s0, s0, reg(b));
            encoder.eor_x(s1, reg(a), reg(b), Shift::Lsl, 0)?;
            encoder.orn_x(s1, XRegOrZero::Zero, reg(s1), Shift::Lsl, 0)?;
            encoder.and_x(s1, s1, reg(res));
            encoder.orr_x(s0, reg(s0), reg(s1), Shift::Lsl, 0)?;
        } else {
            // carry = (a & b) | ((a | b) & ~r), taken at the top bit.
            encoder.and_x(s0, a, reg(b));
            encoder.orr_x(s1, reg(a), reg(b), Shift::Lsl, 0)?;
            encoder.orn_x(s2, XRegOrZero::Zero, reg(res), Shift::Lsl, 0)?;
            encoder.and_x(s1, s1, reg(s2));
            encoder.orr_x(s0, reg(s0), reg(s1), Shift::Lsl, 0)?;
        }
        encoder.ubfx_x(s0, s0, top, 1)?;
        encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 0)?;
    }

    // OF (bit 11).
    if subtract {
        // overflow = ((a ^ b) & (a ^ r)) at the top bit.
        encoder.eor_x(s0, reg(a), reg(b), Shift::Lsl, 0)?;
        encoder.eor_x(s1, reg(a), reg(res), Shift::Lsl, 0)?;
    } else {
        // overflow = ((a ^ r) & (b ^ r)) at the top bit.
        encoder.eor_x(s0, reg(a), reg(res), Shift::Lsl, 0)?;
        encoder.eor_x(s1, reg(b), reg(res), Shift::Lsl, 0)?;
    }
    encoder.and_x(s0, s0, reg(s1));
    encoder.ubfx_x(s0, s0, top, 1)?;
    encoder.orr_x(out, reg(out), reg(s0), Shift::Lsl, 11)?;
    Ok(())
}

// Register plan of `emit_atomic_program`.
const TEMP_POOL: [u8; 7] = [2, 3, 4, 5, 6, 7, 8];
const FLAGS_OUT: u8 = 9;
const FLAGS_SCRATCH: [u8; 5] = [10, 11, 12, 13, 14];
const RFLAGS_TMP: u8 = 15;
const MASK_TMP: u8 = 16;

fn x(number: u8) -> XReg {
    XReg::new(number).expect("register plan uses registers below 31")
}

/// Emit the sequence of the single guest instruction of `program`. The program
/// is verified first; every effect outside the supported set is refused.
pub fn emit_atomic_program(
    encoder: &mut Encoder,
    env: &SequenceEnv,
    program: &Program,
) -> Result<(), SequenceError> {
    ir::verify(program).map_err(SequenceError::InvalidProgram)?;
    if program.guest_instructions.len() != 1 {
        return Err(SequenceError::NotOneInstruction);
    }
    for register in [env.state, env.address] {
        if (2..=16).contains(&register.number()) {
            return Err(SequenceError::EnvOverlapsScratch);
        }
    }

    let mut temps: HashMap<TempId, XReg> = HashMap::new();
    let mut next_temp = 0usize;
    let mut define = |temp: TempId, temps: &mut HashMap<TempId, XReg>| {
        let number = *TEMP_POOL
            .get(next_temp)
            .ok_or(SequenceError::TooManyTemporaries)?;
        next_temp += 1;
        let register = x(number);
        temps.insert(temp, register);
        Ok::<XReg, SequenceError>(register)
    };
    let lookup = |temp: TempId, temps: &HashMap<TempId, XReg>| {
        temps
            .get(&temp)
            .copied()
            .ok_or(SequenceError::UnknownTemporary(temp))
    };
    let gpr_offset =
        |register: GuestGpr| -> u32 { LONG_MODE_GPR_OFFSET as u32 + 8 * u32::from(register.0) };

    for (index, operation) in program.operations.iter().enumerate() {
        match operation {
            Operation::BeginInstruction { .. }
            | Operation::CommitInstruction { .. }
            | Operation::Exit { .. } => {}
            Operation::Constant { dst, value, width } => {
                let register = define(*dst, &mut temps)?;
                let value = match width {
                    IntWidth::W64 => *value,
                    other => *value & ((1u64 << width_bits(*other)) - 1),
                };
                materialize(encoder, register, value)?;
            }
            Operation::ReadGpr {
                dst,
                register,
                width,
            } => {
                let target = define(*dst, &mut temps)?;
                encoder.ldr_sized(
                    access_width(*width),
                    reg(target),
                    AddressBase::Reg(env.state),
                    gpr_offset(*register),
                )?;
            }
            Operation::AtomicRmw {
                value,
                expected,
                dst,
                ..
            } => {
                let value_reg = lookup(*value, &temps)?;
                let expected_reg = match expected {
                    Some(expected) => Some(lookup(*expected, &temps)?),
                    None => None,
                };
                let dst_reg = define(*dst, &mut temps)?;
                lower_memory_effect(
                    encoder,
                    env.mode,
                    operation,
                    &HostOperands {
                        address: AddressBase::Reg(env.address),
                        dst: Some(dst_reg),
                        value: Some(reg(value_reg)),
                        expected: expected_reg,
                    },
                )?;
            }
            Operation::Fence { .. } => {
                lower_memory_effect(
                    encoder,
                    env.mode,
                    operation,
                    &HostOperands {
                        address: AddressBase::Reg(env.address),
                        dst: None,
                        value: None,
                        expected: None,
                    },
                )?;
            }
            Operation::AtomicFlags {
                kind,
                width,
                lhs,
                rhs,
            } => {
                let regs = FlagsRegs {
                    lhs: lookup(*lhs, &temps)?,
                    rhs: match rhs {
                        Some(rhs) => Some(lookup(*rhs, &temps)?),
                        None => None,
                    },
                    out: x(FLAGS_OUT),
                    scratch: FLAGS_SCRATCH.map(x),
                };
                lower_atomic_flags(encoder, *kind, *width, &regs)?;
                // RFLAGS = (RFLAGS & !mask) | out
                let rflags = x(RFLAGS_TMP);
                let mask = x(MASK_TMP);
                let scratch = x(FLAGS_SCRATCH[0]);
                encoder.ldr_x(
                    reg(rflags),
                    AddressBase::Reg(env.state),
                    LONG_MODE_RFLAGS_OFFSET as u32,
                )?;
                encoder.movz_x(mask, kind.written_mask() as u16, 0)?;
                encoder.and_x(scratch, rflags, reg(mask));
                encoder.eor_x(rflags, reg(rflags), reg(scratch), Shift::Lsl, 0)?;
                encoder.orr_x(rflags, reg(rflags), reg(x(FLAGS_OUT)), Shift::Lsl, 0)?;
                encoder.str_x(
                    reg(rflags),
                    AddressBase::Reg(env.state),
                    LONG_MODE_RFLAGS_OFFSET as u32,
                )?;
            }
            Operation::WriteGpr {
                register,
                src,
                width,
            } => {
                if !matches!(width, IntWidth::W32 | IntWidth::W64) {
                    return Err(SequenceError::UnsupportedWidth {
                        operation: index,
                        width: *width,
                    });
                }
                // A `W32` write zero-extends: every temporary is already
                // zero-extended to its width.
                encoder.str_x(
                    reg(lookup(*src, &temps)?),
                    AddressBase::Reg(env.state),
                    gpr_offset(*register),
                )?;
            }
            Operation::WriteGprIfNotEqual {
                register,
                src,
                compare,
                ..
            } => {
                let skip = encoder.new_label()?;
                encoder.subs_x(
                    XRegOrZero::Zero,
                    lookup(*src, &temps)?,
                    reg(lookup(*compare, &temps)?),
                );
                encoder.b_cond(Condition::Equal, skip)?;
                encoder.str_x(
                    reg(lookup(*src, &temps)?),
                    AddressBase::Reg(env.state),
                    gpr_offset(*register),
                )?;
                encoder.bind(skip)?;
            }
            _ => return Err(SequenceError::UnsupportedEffect { operation: index }),
        }
    }
    Ok(())
}

/// Load a 64-bit constant with `MOVZ` and `MOVK`.
pub(crate) fn materialize(encoder: &mut Encoder, dst: XReg, value: u64) -> Result<(), EncodeError> {
    let mut first = true;
    for halfword in 0..4u8 {
        let part = ((value >> (16 * halfword)) & 0xffff) as u16;
        if part == 0 && !(halfword == 3 && first) {
            continue;
        }
        if first {
            encoder.movz_x(dst, part, halfword)?;
            first = false;
        } else {
            encoder.movk_x(dst, part, halfword)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{AtomicRmwKind, FenceKind, GuestInstruction};
    use od_isa::OperandSize;
    use od_isa::long_decode::{AddressBase as GuestBase, MemoryOperand};

    const RIP: u64 = 0x2000;

    fn operand(size: OperandSize) -> MemoryOperand {
        MemoryOperand {
            segment: od_isa::long_decode::MemorySegment::Flat,
            base: GuestBase::None,
            index: None,
            scale: 1,
            displacement: 0x1000,
            address_size: OperandSize::Qword,
            size,
        }
    }

    fn program(operations: Vec<Operation>) -> Program {
        let mut all = vec![Operation::BeginInstruction { index: 0 }];
        all.extend(operations);
        all.push(Operation::CommitInstruction { index: 0 });
        all.push(Operation::Exit {
            kind: ir::ExitKind::BlockComplete,
            target_rip: RIP + 4,
        });
        Program {
            guest_instructions: vec![GuestInstruction {
                guest_rip: RIP,
                next_rip: RIP + 4,
            }],
            operations: all,
            code_end: RIP + 4,
        }
    }

    fn env(mode: MemoryOrderMode) -> SequenceEnv {
        SequenceEnv {
            mode,
            state: x(0),
            address: x(1),
        }
    }

    fn emit(mode: MemoryOrderMode, program: &Program) -> Result<Vec<u32>, SequenceError> {
        let mut encoder = Encoder::new();
        emit_atomic_program(&mut encoder, &env(mode), program)?;
        Ok(encoder.finish().unwrap().into_words())
    }

    fn lock_add_m32_r32() -> Program {
        program(vec![
            Operation::ReadGpr {
                dst: TempId(0),
                register: GuestGpr(3),
                width: IntWidth::W32,
            },
            Operation::AtomicRmw {
                kind: AtomicRmwKind::Add,
                width: IntWidth::W32,
                operand: operand(OperandSize::Dword),
                value: TempId(0),
                expected: None,
                dst: TempId(1),
            },
            Operation::AtomicFlags {
                kind: AtomicFlagsKind::Add,
                width: IntWidth::W32,
                lhs: TempId(1),
                rhs: Some(TempId(0)),
            },
        ])
    }

    #[test]
    fn the_modes_differ_only_in_the_ordering_bits_of_the_atomic_word() {
        let interleaved = emit(MemoryOrderMode::Interleaved, &lock_add_m32_r32()).unwrap();
        let parallel = emit(MemoryOrderMode::Parallel, &lock_add_m32_r32()).unwrap();
        assert_eq!(interleaved.len(), parallel.len());
        let differing: Vec<usize> = (0..parallel.len())
            .filter(|&i| interleaved[i] != parallel[i])
            .collect();
        assert_eq!(differing, [1], "only the LDADD word differs");
        // LDADD W: x2 (value) -> x3 (old), [x1]; AL sets bits 23 and 22.
        assert_eq!(interleaved[1], 0xb822_0023);
        assert_eq!(parallel[1], 0xb8e2_0023);
    }

    #[test]
    fn a_fence_alone_is_a_barrier_in_parallel_mode_and_nothing_when_interleaved() {
        let fence = program(vec![Operation::Fence {
            kind: FenceKind::Full,
        }]);
        assert_eq!(
            emit(MemoryOrderMode::Parallel, &fence).unwrap(),
            [0xd503_3bbf]
        );
        assert!(
            emit(MemoryOrderMode::Interleaved, &fence)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn unsupported_effects_and_bad_programs_are_refused_not_skipped() {
        let checked_load = program(vec![Operation::CheckedLoad {
            dst: TempId(0),
            operand: MemoryOperand {
                segment: od_isa::long_decode::MemorySegment::Flat,
                base: GuestBase::Register(od_isa::Register::Rax),
                index: None,
                scale: 1,
                displacement: 0,
                address_size: OperandSize::Qword,
                size: OperandSize::Qword,
            },
        }]);
        assert!(matches!(
            emit(MemoryOrderMode::Parallel, &checked_load),
            Err(SequenceError::InvalidProgram(_) | SequenceError::UnsupportedEffect { .. })
        ));
        let mut two = lock_add_m32_r32();
        two.guest_instructions.push(GuestInstruction {
            guest_rip: RIP + 4,
            next_rip: RIP + 8,
        });
        assert!(emit(MemoryOrderMode::Parallel, &two).is_err());
        let narrow_write = program(vec![
            Operation::Constant {
                dst: TempId(0),
                width: IntWidth::W8,
                value: 1,
            },
            Operation::WriteGpr {
                register: GuestGpr(0),
                src: TempId(0),
                width: IntWidth::W8,
            },
        ]);
        assert!(matches!(
            emit(MemoryOrderMode::Parallel, &narrow_write),
            Err(SequenceError::UnsupportedWidth { .. })
        ));
        let mut encoder = Encoder::new();
        let overlapping = SequenceEnv {
            mode: MemoryOrderMode::Parallel,
            state: x(0),
            address: x(9),
        };
        assert_eq!(
            emit_atomic_program(&mut encoder, &overlapping, &lock_add_m32_r32()),
            Err(SequenceError::EnvOverlapsScratch)
        );
    }

    #[test]
    fn flag_sequences_have_a_fixed_shape_per_kind() {
        let regs = FlagsRegs {
            lhs: x(2),
            rhs: Some(x(3)),
            out: x(9),
            scratch: [x(10), x(11), x(12), x(13), x(14)],
        };
        let length = |kind, width| {
            let mut encoder = Encoder::new();
            lower_atomic_flags(&mut encoder, kind, width, &regs).unwrap();
            encoder.position()
        };
        // Narrow widths pay two instructions to zero-extend the result.
        assert_eq!(
            length(AtomicFlagsKind::Add, IntWidth::W32),
            length(AtomicFlagsKind::Add, IntWidth::W64) + 2
        );
        // INC drops the CF part, logic kinds drop AF, CF and OF.
        assert!(
            length(AtomicFlagsKind::AddKeepCarry, IntWidth::W64)
                < length(AtomicFlagsKind::Add, IntWidth::W64)
        );
        assert!(
            length(AtomicFlagsKind::Or, IntWidth::W64)
                < length(AtomicFlagsKind::AddKeepCarry, IntWidth::W64)
        );
        let mut encoder = Encoder::new();
        let bit_regs = FlagsRegs { rhs: None, ..regs };
        lower_atomic_flags(
            &mut encoder,
            AtomicFlagsKind::BitTest { bit: 70 },
            IntWidth::W64,
            &bit_regs,
        )
        .unwrap();
        // out = 0; UBFX (70 % 64 = 6); ORR.
        assert_eq!(encoder.position(), 3);
    }
}
