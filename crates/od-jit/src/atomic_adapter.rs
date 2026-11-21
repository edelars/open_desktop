//! S6-41b (а): decoder adapter that turns the decoded `LOCK`/`XCHG`/`XADD`/
//! `CMPXCHG`/`MFENCE` forms of `od_isa::long_decode` into the S6-41 atomic IR
//! effects (`AtomicRmw`, `AtomicFlags`, `Fence`, `WriteGprIfNotEqual`).
//!
//! The adapter reads the public decoded `Instruction`; it does not decode x86
//! again and does not define the arithmetic or flag semantics (those are the
//! shared interpreter helpers; the lowering in `arm64::atomic_sequence` mirrors
//! them and is checked against the interpreter by differential tests).
//!
//! Output is one verified `Program`: one instruction (`lower_atomic_instruction`)
//! or a contiguous run of them (`lower_atomic_sequence`, S6-41d). The block that
//! executes it is `atomic_block::compile_atomic_block`; the general block
//! emitters and their allocator still refuse these effects explicitly
//! (S6-41b (в)/(е), S6-41d).
//!
//! Not adapted, each with an explicit `AtomicAdapterError`:
//! - forms whose address is not a static `MemoryOperand`: `LOCK BTS/BTR m, r`
//!   with a register bit offset (the element address depends on the register);
//! - forms whose data register is outside RAX..RBX (the IR's initial GPR
//!   profile) or is not a full register view;
//! - segment-override (FS/GS) forms, which decode to separate operations;
//! - any operation that is not an atomic-family form.

use od_isa::OperandSize;
use od_isa::RegisterView;
use od_isa::long_decode::{BitKind, Instruction, MemoryOperand, Operation as Decoded};

use crate::ir::{
    self, AtomicFlagsKind, AtomicRmwKind, ExitKind, FenceKind, GuestGpr, GuestInstruction,
    IntWidth, Operation, Program, TempId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AtomicAdapterError {
    /// The decoded operation is not an atomic-family form.
    NotAnAtomicForm,
    /// `LOCK BTS/BTR m, r` with a register bit offset.
    RegisterBitOffset,
    /// The memory operand width is not a width the form allows.
    UnsupportedWidth(OperandSize),
    /// A data register outside RAX, RCX, RDX, RBX or not a full register view.
    UnsupportedRegister,
    /// `BitKind` other than `Set`/`Reset`.
    UnsupportedBitKind,
    /// The produced program failed IR verification.
    Verification(ir::VerifyError),
}

const fn int_width(size: OperandSize) -> IntWidth {
    match size {
        OperandSize::Byte => IntWidth::W8,
        OperandSize::Word => IntWidth::W16,
        OperandSize::Dword => IntWidth::W32,
        OperandSize::Qword => IntWidth::W64,
    }
}

const fn width_mask(size: OperandSize) -> u64 {
    match size {
        OperandSize::Byte => 0xff,
        OperandSize::Word => 0xffff,
        OperandSize::Dword => 0xffff_ffff,
        OperandSize::Qword => u64::MAX,
    }
}

struct Builder {
    operations: Vec<Operation>,
    next_temp: u32,
}

impl Builder {
    fn temp(&mut self) -> TempId {
        let temp = TempId(self.next_temp);
        self.next_temp += 1;
        temp
    }

    fn constant(&mut self, size: OperandSize, value: u64) -> TempId {
        let dst = self.temp();
        self.operations.push(Operation::Constant {
            dst,
            width: int_width(size),
            value: value & width_mask(size),
        });
        dst
    }

    fn read(
        &mut self,
        view: RegisterView,
        size: OperandSize,
    ) -> Result<TempId, AtomicAdapterError> {
        let register = gpr(view, size)?;
        let dst = self.temp();
        self.operations.push(Operation::ReadGpr {
            dst,
            register,
            width: int_width(size),
        });
        Ok(dst)
    }

    fn rmw(
        &mut self,
        kind: AtomicRmwKind,
        operand: MemoryOperand,
        value: TempId,
        expected: Option<TempId>,
    ) -> TempId {
        let dst = self.temp();
        self.operations.push(Operation::AtomicRmw {
            kind,
            width: int_width(operand.size),
            operand,
            value,
            expected,
            dst,
        });
        dst
    }

    fn flags(
        &mut self,
        kind: AtomicFlagsKind,
        size: OperandSize,
        lhs: TempId,
        rhs: Option<TempId>,
    ) {
        self.operations.push(Operation::AtomicFlags {
            kind,
            width: int_width(size),
            lhs,
            rhs,
        });
    }
}

/// The data register view must be a full dword/qword register of the initial
/// RAX..RBX profile whose size equals the operand size.
fn gpr(view: RegisterView, size: OperandSize) -> Result<GuestGpr, AtomicAdapterError> {
    let number = view.register() as u8;
    if view.size() != size
        || number >= 4
        || !matches!(size, OperandSize::Dword | OperandSize::Qword)
    {
        return Err(AtomicAdapterError::UnsupportedRegister);
    }
    Ok(GuestGpr(number))
}

fn check_size(size: OperandSize, allowed: &[OperandSize]) -> Result<(), AtomicAdapterError> {
    if allowed.contains(&size) {
        Ok(())
    } else {
        Err(AtomicAdapterError::UnsupportedWidth(size))
    }
}

const DWORD_QWORD: [OperandSize; 2] = [OperandSize::Dword, OperandSize::Qword];
const ANY_SIZE: [OperandSize; 4] = [
    OperandSize::Byte,
    OperandSize::Word,
    OperandSize::Dword,
    OperandSize::Qword,
];

/// Lower one decoded atomic-family instruction located at `guest_rip` to a
/// verified single-instruction program (terminated by `Exit::BlockComplete` at
/// the next RIP).
pub fn lower_atomic_instruction(
    instruction: &Instruction,
    guest_rip: u64,
) -> Result<Program, AtomicAdapterError> {
    lower_atomic_sequence(&[instruction], guest_rip)
}

/// Cheap rejection of a partial fetch from the ordinary forms probe. The
/// full decoder and `lower_atomic_instruction` still decide admission.
pub fn could_start_atomic(bytes: &[u8]) -> bool {
    let mut bytes = bytes.iter().copied().skip_while(|byte| {
        matches!(
            byte,
            0x26 | 0x2e | 0x36 | 0x3e | 0x40..=0x4f | 0x64..=0x67 | 0xf2 | 0xf3
        )
    });
    match bytes.next() {
        None | Some(0x86 | 0x87 | 0xf0) => true,
        Some(0x0f) => matches!(bytes.next(), None | Some(0xae | 0xb0 | 0xb1 | 0xc0 | 0xc1)),
        _ => false,
    }
}

/// S6-41d: lower a contiguous run of decoded atomic-family instructions, the first at `guest_rip`,
/// to one verified multi-instruction program (terminated by `Exit::BlockComplete` at the RIP after
/// the last instruction). Temporaries are numbered across the whole program; every instruction
/// keeps its own `BeginInstruction`/`CommitInstruction` pair, so the order rules of `ir::verify`
/// (`AtomicRmwOrder`) apply per guest instruction. An instruction the adapter refuses refuses the
/// whole run, with the error of that instruction.
pub fn lower_atomic_sequence(
    instructions: &[&Instruction],
    guest_rip: u64,
) -> Result<Program, AtomicAdapterError> {
    let mut b = Builder {
        operations: Vec::new(),
        next_temp: 0,
    };
    let mut guest_instructions = Vec::with_capacity(instructions.len());
    let mut rip = guest_rip;
    for (index, instruction) in instructions.iter().enumerate() {
        let next_rip = rip + u64::from(instruction.length);
        b.operations.push(Operation::BeginInstruction { index });
        lower_one(instruction, &mut b)?;
        b.operations.push(Operation::CommitInstruction { index });
        guest_instructions.push(GuestInstruction {
            guest_rip: rip,
            next_rip,
        });
        rip = next_rip;
    }
    b.operations.push(Operation::Exit {
        kind: ExitKind::BlockComplete,
        target_rip: rip,
    });
    let program = Program {
        guest_instructions,
        operations: b.operations,
        code_end: rip,
    };
    ir::verify(&program).map_err(AtomicAdapterError::Verification)?;
    Ok(program)
}

/// The effects of one instruction, between its `BeginInstruction` and `CommitInstruction`.
fn lower_one(instruction: &Instruction, b: &mut Builder) -> Result<(), AtomicAdapterError> {
    match instruction.operation {
        // LOCK ADD m, r  (F0 [REX.W] 01 /r)
        Decoded::LockAddMemReg {
            destination,
            source,
        } => {
            check_size(destination.size, &DWORD_QWORD)?;
            let value = b.read(source, destination.size)?;
            let old = b.rmw(AtomicRmwKind::Add, destination, value, None);
            b.flags(AtomicFlagsKind::Add, destination.size, old, Some(value));
        }
        // LOCK ADD m, imm  (F0 [REX.W] 80/81/83 /0)
        Decoded::LockAddMem {
            destination,
            immediate,
        } => {
            check_size(destination.size, &ANY_SIZE)?;
            let value = b.constant(destination.size, immediate);
            let old = b.rmw(AtomicRmwKind::Add, destination, value, None);
            b.flags(AtomicFlagsKind::Add, destination.size, old, Some(value));
        }
        // LOCK INC m
        Decoded::LockIncMem { destination } => {
            check_size(destination.size, &ANY_SIZE)?;
            let one = b.constant(destination.size, 1);
            let old = b.rmw(AtomicRmwKind::Add, destination, one, None);
            b.flags(
                AtomicFlagsKind::AddKeepCarry,
                destination.size,
                old,
                Some(one),
            );
        }
        // LOCK DEC m: memory += -1, flags of old - 1 (CF kept)
        Decoded::LockDecMem { destination } => {
            check_size(destination.size, &ANY_SIZE)?;
            let minus_one = b.constant(destination.size, u64::MAX);
            let one = b.constant(destination.size, 1);
            let old = b.rmw(AtomicRmwKind::Add, destination, minus_one, None);
            b.flags(
                AtomicFlagsKind::SubtractKeepCarry,
                destination.size,
                old,
                Some(one),
            );
        }
        // [LOCK] XADD m, r (also the DS-override form, executed as LOCK)
        Decoded::LockXaddMem {
            destination,
            source,
        }
        | Decoded::XaddDsMem {
            destination,
            source,
        }
        | Decoded::XaddMemory {
            destination,
            source,
        } => {
            check_size(destination.size, &DWORD_QWORD)?;
            let value = b.read(source, destination.size)?;
            let register = gpr(source, destination.size)?;
            let old = b.rmw(AtomicRmwKind::Add, destination, value, None);
            b.flags(AtomicFlagsKind::Add, destination.size, old, Some(value));
            b.operations.push(Operation::WriteGpr {
                register,
                src: old,
                width: int_width(destination.size),
            });
        }
        // XCHG m, r (implicitly locked)
        Decoded::XchgMemory { register, memory } => {
            check_size(memory.size, &DWORD_QWORD)?;
            let value = b.read(register, memory.size)?;
            let gpr = gpr(register, memory.size)?;
            let old = b.rmw(AtomicRmwKind::Exchange, memory, value, None);
            b.operations.push(Operation::WriteGpr {
                register: gpr,
                src: old,
                width: int_width(memory.size),
            });
        }
        // [LOCK] CMPXCHG m, r: flags of accumulator - memory; the accumulator
        // receives the observed value only on a mismatch.
        Decoded::LockCmpxchgMem {
            destination,
            source,
        }
        | Decoded::CmpxchgMemory {
            destination,
            source,
        } => {
            check_size(destination.size, &DWORD_QWORD)?;
            let size = destination.size;
            let expected = b.temp();
            b.operations.push(Operation::ReadGpr {
                dst: expected,
                register: GuestGpr(0),
                width: int_width(size),
            });
            let value = b.read(source, size)?;
            let old = b.rmw(
                AtomicRmwKind::CompareExchange,
                destination,
                value,
                Some(expected),
            );
            b.flags(AtomicFlagsKind::Subtract, size, expected, Some(old));
            b.operations.push(Operation::WriteGprIfNotEqual {
                register: GuestGpr(0),
                src: old,
                compare: expected,
                width: int_width(size),
            });
        }
        // LOCK OR m, imm
        Decoded::LockOrMem {
            destination,
            immediate,
        } => {
            check_size(destination.size, &ANY_SIZE)?;
            let value = b.constant(destination.size, immediate);
            let old = b.rmw(AtomicRmwKind::Or, destination, value, None);
            b.flags(AtomicFlagsKind::Or, destination.size, old, Some(value));
        }
        // LOCK AND m, imm: LDCLR with the complement of the width-normalized immediate.
        Decoded::LockAndMem {
            destination,
            immediate,
        } => {
            check_size(destination.size, &ANY_SIZE)?;
            let size = destination.size;
            let operand = b.constant(size, immediate);
            let complement = b.constant(size, !immediate);
            let old = b.rmw(AtomicRmwKind::ClearBits, destination, complement, None);
            b.flags(AtomicFlagsKind::And, size, old, Some(operand));
        }
        // LOCK BTS/BTR m, imm8
        Decoded::LockBitMemImm {
            kind,
            destination,
            bit,
        } => {
            check_size(destination.size, &DWORD_QWORD)?;
            let size = destination.size;
            let bits = (size.bytes() * 8) as u64;
            let mask = b.constant(size, 1u64 << (u64::from(bit) % bits));
            let rmw_kind = match kind {
                BitKind::Set => AtomicRmwKind::Or,
                BitKind::Reset => AtomicRmwKind::ClearBits,
                BitKind::Test | BitKind::Complement => {
                    return Err(AtomicAdapterError::UnsupportedBitKind);
                }
            };
            let old = b.rmw(rmw_kind, destination, mask, None);
            b.flags(AtomicFlagsKind::BitTest { bit }, size, old, None);
        }
        // LOCK BTS/BTR m, r: the register selects the element, so the address
        // is not a static operand.
        Decoded::LockBitMem { .. } => return Err(AtomicAdapterError::RegisterBitOffset),
        Decoded::Mfence => b.operations.push(Operation::Fence {
            kind: FenceKind::Full,
        }),
        Decoded::Sfence => b.operations.push(Operation::Fence {
            kind: FenceKind::Store,
        }),
        Decoded::Lfence => b.operations.push(Operation::Fence {
            kind: FenceKind::Load,
        }),
        _ => return Err(AtomicAdapterError::NotAnAtomicForm),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use od_isa::long_decode;
    use od_isa::memory::{AccessContext, AccessKind, GuestMemory, MemoryError};

    struct Code(Vec<u8>);

    impl GuestMemory for Code {
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: AccessKind::Fetch,
                })
        }
        fn read(
            &mut self,
            address: u64,
            _: AccessContext,
            _: &mut [u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Read,
            })
        }
        fn write(&mut self, address: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: AccessKind::Write,
            })
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn lower(bytes: &[u8]) -> Result<Program, AtomicAdapterError> {
        let mut memory = Code(bytes.to_vec());
        let decoded = long_decode::decode(&mut memory, 0, AccessContext::default())
            .expect("the form decodes");
        let program = lower_atomic_instruction(&decoded, 0x2000);
        if program.is_ok() {
            for length in 1..=2.min(bytes.len()) {
                assert!(could_start_atomic(&bytes[..length]), "{bytes:02x?}");
            }
        }
        program
    }

    fn effects(program: &Program) -> Vec<&Operation> {
        program
            .operations
            .iter()
            .filter(|operation| {
                !matches!(
                    operation,
                    Operation::BeginInstruction { .. }
                        | Operation::CommitInstruction { .. }
                        | Operation::Exit { .. }
                        | Operation::Constant { .. }
                        | Operation::ReadGpr { .. }
                )
            })
            .collect()
    }

    fn rmw_kinds(program: &Program) -> Vec<AtomicRmwKind> {
        program
            .operations
            .iter()
            .filter_map(|operation| match operation {
                Operation::AtomicRmw { kind, .. } => Some(*kind),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_lock_form_the_s6_41_differential_covers_lowers_to_a_verified_program() {
        // (bytes, expected atomic kind, flag kind or None, writes a register)
        let cases: &[(&[u8], AtomicRmwKind, Option<AtomicFlagsKind>, bool)] = &[
            (
                &[0xf0, 0x0f, 0xc1, 0x1c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::Add),
                true,
            ),
            (
                &[0xf0, 0x48, 0x0f, 0xc1, 0x1c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::Add),
                true,
            ),
            (
                &[0xf0, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::Add),
                false,
            ),
            (
                &[0xf0, 0x80, 0x04, 0x25, 0, 0x10, 0, 0, 0xf5],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::Add),
                false,
            ),
            (
                &[0xf0, 0xff, 0x04, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::AddKeepCarry),
                false,
            ),
            (
                &[0xf0, 0x48, 0xff, 0x0c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Add,
                Some(AtomicFlagsKind::SubtractKeepCarry),
                false,
            ),
            (
                &[0xf0, 0x0f, 0xb1, 0x1c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::CompareExchange,
                Some(AtomicFlagsKind::Subtract),
                false,
            ),
            (
                &[0x87, 0x1c, 0x25, 0, 0x10, 0, 0],
                AtomicRmwKind::Exchange,
                None,
                true,
            ),
            (
                &[0xf0, 0x80, 0x0c, 0x25, 0, 0x10, 0, 0, 0x0f],
                AtomicRmwKind::Or,
                Some(AtomicFlagsKind::Or),
                false,
            ),
            (
                &[0xf0, 0x80, 0x24, 0x25, 0, 0x10, 0, 0, 0x0f],
                AtomicRmwKind::ClearBits,
                Some(AtomicFlagsKind::And),
                false,
            ),
            (
                &[0xf0, 0x0f, 0xba, 0x2c, 0x25, 0, 0x10, 0, 0, 0x05],
                AtomicRmwKind::Or,
                Some(AtomicFlagsKind::BitTest { bit: 5 }),
                false,
            ),
            (
                &[0xf0, 0x0f, 0xba, 0x34, 0x25, 0, 0x10, 0, 0, 0x05],
                AtomicRmwKind::ClearBits,
                Some(AtomicFlagsKind::BitTest { bit: 5 }),
                false,
            ),
        ];
        for (bytes, kind, flags, writes) in cases {
            let program = lower(bytes).unwrap_or_else(|error| panic!("{bytes:02x?}: {error:?}"));
            assert_eq!(rmw_kinds(&program), [*kind], "{bytes:02x?}");
            let flag_kinds: Vec<AtomicFlagsKind> = program
                .operations
                .iter()
                .filter_map(|operation| match operation {
                    Operation::AtomicFlags { kind, .. } => Some(*kind),
                    _ => None,
                })
                .collect();
            assert_eq!(
                flag_kinds,
                flags.iter().copied().collect::<Vec<_>>(),
                "{bytes:02x?}"
            );
            let register_writes = program
                .operations
                .iter()
                .filter(|operation| {
                    matches!(
                        operation,
                        Operation::WriteGpr { .. } | Operation::WriteGprIfNotEqual { .. }
                    )
                })
                .count();
            // CMPXCHG always has the conditional accumulator write.
            let expected_writes =
                usize::from(*writes) + usize::from(*kind == AtomicRmwKind::CompareExchange);
            assert_eq!(register_writes, expected_writes, "{bytes:02x?}");
            assert_eq!(program.guest_instructions.len(), 1);
            assert_eq!(
                program.guest_instructions[0].next_rip,
                0x2000 + bytes.len() as u64
            );
        }
    }

    #[test]
    fn lock_and_qword_imm8_lowers_with_the_sign_extended_mask() {
        let program = lower(&[0xf0, 0x49, 0x83, 0x26, 0xcf]).unwrap();
        assert_eq!(rmw_kinds(&program), [AtomicRmwKind::ClearBits]);
        for value in [0xffff_ffff_ffff_ffcf, 0x30] {
            assert!(program.operations.iter().any(|op| matches!(
                op,
                Operation::Constant { width: IntWidth::W64, value: actual, .. } if *actual == value
            )));
        }
        assert!(matches!(
            effects(&program).as_slice(),
            [
                Operation::AtomicRmw {
                    width: IntWidth::W64,
                    ..
                },
                Operation::AtomicFlags {
                    kind: AtomicFlagsKind::And,
                    width: IntWidth::W64,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn cmpxchg_has_the_accumulator_compare_and_the_conditional_write() {
        let program = lower(&[0xf0, 0x48, 0x0f, 0xb1, 0x1c, 0x25, 0, 0x10, 0, 0]).unwrap();
        let ops = effects(&program);
        assert!(matches!(
            ops[0],
            Operation::AtomicRmw {
                kind: AtomicRmwKind::CompareExchange,
                width: IntWidth::W64,
                expected: Some(_),
                ..
            }
        ));
        // Flags of accumulator - memory: lhs is the expected temporary, rhs the old value.
        let (
            Operation::AtomicRmw {
                expected: Some(expected),
                dst: old,
                ..
            },
            Operation::AtomicFlags { lhs, rhs, .. },
        ) = (ops[0], ops[1])
        else {
            panic!("unexpected shape {ops:?}");
        };
        assert_eq!(lhs, expected);
        assert_eq!(*rhs, Some(*old));
        assert!(matches!(
            ops[2],
            Operation::WriteGprIfNotEqual {
                register: GuestGpr(0),
                width: IntWidth::W64,
                ..
            }
        ));
    }

    #[test]
    fn fences_become_fence_effects_and_nothing_else() {
        for (bytes, kind) in [
            (&[0x0f, 0xae, 0xf0][..], FenceKind::Full),
            (&[0x0f, 0xae, 0xe8][..], FenceKind::Load),
            (&[0x0f, 0xae, 0xf8][..], FenceKind::Store),
        ] {
            let program = lower(bytes).unwrap();
            assert_eq!(effects(&program), [&Operation::Fence { kind }]);
        }
    }

    #[test]
    fn refusals_are_explicit() {
        // LOCK BTS m64, r64: the register selects the element.
        assert_eq!(
            lower(&[0xf0, 0x48, 0x0f, 0xab, 0x1c, 0x25, 0, 0x10, 0, 0]),
            Err(AtomicAdapterError::RegisterBitOffset)
        );
        // MOV RAX, RBX is not an atomic form.
        assert_eq!(
            lower(&[0x48, 0x89, 0xd8]),
            Err(AtomicAdapterError::NotAnAtomicForm)
        );
        // S2-51: CMPXCHG16B m128 (with and without LOCK) is not an atomic form of this adapter: the
        // IR has no 128-bit atomic effect, so the form is refused, not lowered to two 64-bit ones.
        assert_eq!(
            lower(&[0x48, 0x0f, 0xc7, 0x0f]),
            Err(AtomicAdapterError::NotAnAtomicForm)
        );
        assert_eq!(
            lower(&[0xf0, 0x48, 0x0f, 0xc7, 0x0f]),
            Err(AtomicAdapterError::NotAnAtomicForm)
        );
        // LOCK XADD with RSI as the data register is outside the RAX..RBX profile.
        assert_eq!(
            lower(&[0xf0, 0x48, 0x0f, 0xc1, 0x34, 0x25, 0, 0x10, 0, 0]),
            Err(AtomicAdapterError::UnsupportedRegister)
        );
    }

    #[test]
    fn atomic_programs_are_refused_by_the_block_emitters() {
        // The general allocator keeps its explicit refusal (S6-41b (е), S6-41d): atomic programs
        // are compiled by `atomic_block`, which has its own register plan.
        let program = lower(&[0xf0, 0x48, 0x0f, 0xc1, 0x1c, 0x25, 0, 0x10, 0, 0]).unwrap();
        assert!(matches!(
            crate::allocator::Allocation::build(
                &program,
                crate::allocator::AllocatorConfig::default(),
                &[]
            ),
            Err(crate::allocator::AllocatorError::UnsupportedEffect { .. })
        ));
    }

    fn decode_run(forms: &[&[u8]]) -> Vec<Instruction> {
        forms
            .iter()
            .map(|bytes| {
                let mut memory = Code(bytes.to_vec());
                long_decode::decode(&mut memory, 0, AccessContext::default())
                    .expect("the form decodes")
            })
            .collect()
    }

    /// S6-41d: a run of decoded forms is one multi-instruction program with contiguous RIPs,
    /// temporaries numbered across it and one `Begin`/`Commit` pair per instruction.
    #[test]
    fn s6_41d_a_run_of_atomic_forms_lowers_to_one_verified_multi_instruction_program() {
        let decoded = decode_run(&[
            // lock add m32, ebx
            &[0xf0, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0],
            // xchg m64, rcx
            &[0x48, 0x87, 0x0c, 0x25, 0x40, 0x10, 0, 0],
            // mfence
            &[0x0f, 0xae, 0xf0],
            // lock cmpxchg m32, ebx
            &[0xf0, 0x0f, 0xb1, 0x1c, 0x25, 0x80, 0x10, 0, 0],
        ]);
        let refs: Vec<&Instruction> = decoded.iter().collect();
        let program = lower_atomic_sequence(&refs, 0x2000).unwrap();
        assert_eq!(ir::verify(&program), Ok(()));
        assert_eq!(program.guest_instructions.len(), 4);
        let mut rip = 0x2000;
        for (instruction, decoded) in program.guest_instructions.iter().zip(&decoded) {
            assert_eq!(instruction.guest_rip, rip);
            rip += u64::from(decoded.length);
            assert_eq!(instruction.next_rip, rip);
        }
        assert_eq!(program.code_end, rip);
        assert_eq!(
            rmw_kinds(&program),
            [
                AtomicRmwKind::Add,
                AtomicRmwKind::Exchange,
                AtomicRmwKind::CompareExchange
            ]
        );
        // One instruction of the run that the adapter refuses refuses the run.
        let decoded = decode_run(&[
            &[0xf0, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0],
            &[0x48, 0x89, 0xd8],
        ]);
        let refs: Vec<&Instruction> = decoded.iter().collect();
        assert_eq!(
            lower_atomic_sequence(&refs, 0x2000),
            Err(AtomicAdapterError::NotAnAtomicForm)
        );
        // A run of one is the old single-instruction lowering.
        let single = decode_run(&[&[0xf0, 0x01, 0x1c, 0x25, 0, 0x10, 0, 0]]);
        assert_eq!(
            lower_atomic_sequence(&[&single[0]], 0x2000).unwrap(),
            lower_atomic_instruction(&single[0], 0x2000).unwrap()
        );
    }
}
