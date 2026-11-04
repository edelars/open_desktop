//! Defined 64-bit ADD/SUB/CMP flags for the initial long-mode integer slice.
//!
//! This is shared x86 arithmetic semantics. Host code generators call the
//! narrow JIT ABI adapter rather than reproducing these formulas.

pub const CARRY_FLAG: u64 = 1 << 0;
pub const PARITY_FLAG: u64 = 1 << 2;
pub const AUXILIARY_CARRY_FLAG: u64 = 1 << 4;
pub const ZERO_FLAG: u64 = 1 << 6;
pub const SIGN_FLAG: u64 = 1 << 7;
pub const OVERFLOW_FLAG: u64 = 1 << 11;
const SIGN_BIT: u64 = 1 << 63;
pub const DEFINED_ARITHMETIC_FLAGS: u64 =
    CARRY_FLAG | PARITY_FLAG | AUXILIARY_CARRY_FLAG | ZERO_FLAG | SIGN_FLAG | OVERFLOW_FLAG;

/// Operation tags shared with `od-jit`'s scalar flags-helper ABI.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArithmeticFlagOperation {
    Add = 1,
    Subtract = 2,
    Compare = 3,
}

impl TryFrom<u32> for ArithmeticFlagOperation {
    type Error = u32;

    fn try_from(raw: u32) -> Result<Self, Self::Error> {
        match raw {
            1 => Ok(Self::Add),
            2 => Ok(Self::Subtract),
            3 => Ok(Self::Compare),
            _ => Err(raw),
        }
    }
}

/// Replace only the six flags defined by 64-bit ADD/SUB/CMP.
///
/// `Compare` uses subtraction flags and never implies a destination write.
/// Every RFLAGS bit outside [`DEFINED_ARITHMETIC_FLAGS`] is copied verbatim
/// from `old_flags`.
pub fn updated_arithmetic_flags(
    operation: ArithmeticFlagOperation,
    lhs: u64,
    rhs: u64,
    old_flags: u64,
) -> u64 {
    let (result, carry, overflow) = match operation {
        ArithmeticFlagOperation::Add => {
            let wide = u128::from(lhs) + u128::from(rhs);
            let result = lhs.wrapping_add(rhs);
            let overflow = (!(lhs ^ rhs) & (lhs ^ result) & SIGN_BIT) != 0;
            (result, wide > u128::from(u64::MAX), overflow)
        }
        ArithmeticFlagOperation::Subtract | ArithmeticFlagOperation::Compare => {
            let result = lhs.wrapping_sub(rhs);
            let overflow = ((lhs ^ rhs) & (lhs ^ result) & SIGN_BIT) != 0;
            (result, lhs < rhs, overflow)
        }
    };

    let mut computed = 0;
    if carry {
        computed |= CARRY_FLAG;
    }
    if result.to_le_bytes()[0].count_ones() % 2 == 0 {
        computed |= PARITY_FLAG;
    }
    if (lhs ^ rhs ^ result) & AUXILIARY_CARRY_FLAG != 0 {
        computed |= AUXILIARY_CARRY_FLAG;
    }
    if result == 0 {
        computed |= ZERO_FLAG;
    }
    if result & SIGN_BIT != 0 {
        computed |= SIGN_FLAG;
    }
    if overflow {
        computed |= OVERFLOW_FLAG;
    }

    (old_flags & !DEFINED_ARITHMETIC_FLAGS) | computed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LongModeState;
    use crate::execute::{self, StepOutcome};
    use crate::memory::{AccessContext, GuestMemory, MemoryError};

    const BASE_RIP: u64 = 0x4000;

    struct CodeMemory {
        base: u64,
        code: Vec<u8>,
    }

    impl GuestMemory for CodeMemory {
        fn fetch(&mut self, address: u64, _context: AccessContext) -> Result<u8, MemoryError> {
            let offset = address
                .checked_sub(self.base)
                .and_then(|offset| usize::try_from(offset).ok())
                .ok_or(MemoryError::Unmapped {
                    address,
                    access: crate::memory::AccessKind::Fetch,
                })?;
            self.code.get(offset).copied().ok_or(MemoryError::Unmapped {
                address,
                access: crate::memory::AccessKind::Fetch,
            })
        }

        fn read(
            &mut self,
            address: u64,
            _context: AccessContext,
            _destination: &mut [u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: crate::memory::AccessKind::Read,
            })
        }

        fn write(
            &mut self,
            address: u64,
            _context: AccessContext,
            _source: &[u8],
        ) -> Result<(), MemoryError> {
            Err(MemoryError::Unmapped {
                address,
                access: crate::memory::AccessKind::Write,
            })
        }

        fn atomic_exchange_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: crate::OperandSize,
            _replacement: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend(format!(
                "unexpected atomic access at {address:#x}"
            )))
        }

        fn atomic_fetch_add_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: crate::OperandSize,
            _addend: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend(format!(
                "unexpected atomic access at {address:#x}"
            )))
        }

        fn atomic_compare_exchange_integer(
            &mut self,
            address: u64,
            _context: AccessContext,
            _size: crate::OperandSize,
            _expected: u64,
            _replacement: u64,
        ) -> Result<u64, MemoryError> {
            Err(MemoryError::Backend(format!(
                "unexpected atomic access at {address:#x}"
            )))
        }

        fn mfence(&mut self, _context: AccessContext) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[test]
    fn w64_flags_match_literal_vectors_and_preserve_other_rflags_bits() {
        let old_flags = 0xA5A5_F0F0_5A5A_0A0A;
        let signed_min = 1_u64 << 63;
        let signed_max = signed_min - 1;
        let vectors = [
            (ArithmeticFlagOperation::Add, 0, 0, 0x044),
            (ArithmeticFlagOperation::Add, u64::MAX, 1, 0x055),
            (ArithmeticFlagOperation::Add, signed_max, 1, 0x894),
            (ArithmeticFlagOperation::Add, 15, 1, 0x010),
            (ArithmeticFlagOperation::Subtract, 0, 1, 0x095),
            (ArithmeticFlagOperation::Subtract, signed_min, 1, 0x814),
            (
                ArithmeticFlagOperation::Subtract,
                signed_max,
                u64::MAX,
                0x885,
            ),
            (
                ArithmeticFlagOperation::Compare,
                signed_min,
                signed_min,
                0x044,
            ),
        ];

        for (operation, lhs, rhs, expected_defined_flags) in vectors {
            let actual = updated_arithmetic_flags(operation, lhs, rhs, old_flags);
            assert_eq!(
                actual,
                (old_flags & !DEFINED_ARITHMETIC_FLAGS) | expected_defined_flags,
                "{operation:?} {lhs:#018x} {rhs:#018x}"
            );
            assert_eq!(
                actual & !DEFINED_ARITHMETIC_FLAGS,
                old_flags & !DEFINED_ARITHMETIC_FLAGS,
                "non-arithmetic flags must be preserved"
            );
        }
        assert_eq!(old_flags & 2, 2, "the fixed RFLAGS bit 1 is preserved");
    }

    #[test]
    fn w64_flags_match_interpreter_for_independent_literal_vectors() {
        let old_flags = 0xA5A5_F0F0_5A5A_0A0A;
        let signed_min = 1_u64 << 63;
        let signed_max = signed_min - 1;
        let vectors = [
            (ArithmeticFlagOperation::Add, 0, 0, 0x044, 0x03),
            (ArithmeticFlagOperation::Add, u64::MAX, 1, 0x055, 0x03),
            (ArithmeticFlagOperation::Add, signed_max, 1, 0x894, 0x03),
            (ArithmeticFlagOperation::Add, 15, 1, 0x010, 0x03),
            (ArithmeticFlagOperation::Subtract, 0, 1, 0x095, 0x2B),
            (
                ArithmeticFlagOperation::Subtract,
                signed_min,
                1,
                0x814,
                0x2B,
            ),
            (
                ArithmeticFlagOperation::Subtract,
                signed_max,
                u64::MAX,
                0x885,
                0x2B,
            ),
            (
                ArithmeticFlagOperation::Compare,
                signed_min,
                signed_min,
                0x044,
                0x3B,
            ),
        ];

        for (operation, lhs, rhs, expected_flags, opcode) in vectors {
            let opcode_form = match opcode {
                0x03 => [0x48, 0x03, 0xC1],
                0x2B => [0x48, 0x2B, 0xC1],
                0x3B => [0x48, 0x3B, 0xC1],
                _ => unreachable!(),
            };
            let mut memory = CodeMemory {
                base: BASE_RIP,
                code: opcode_form.to_vec(),
            };
            let mut state = LongModeState::default();
            state.gpr[0] = lhs;
            state.gpr[1] = rhs;
            state.rflags = old_flags;
            state.rip = BASE_RIP;
            let gprs_before = state.gpr;

            assert_eq!(
                execute::step(&mut state, &mut memory, AccessContext::default()).unwrap(),
                StepOutcome::Advanced
            );
            assert_eq!(
                updated_arithmetic_flags(operation, lhs, rhs, old_flags),
                state.rflags
            );
            assert_eq!(
                state.rflags,
                (old_flags & !DEFINED_ARITHMETIC_FLAGS) | expected_flags
            );
            if operation == ArithmeticFlagOperation::Compare {
                assert_eq!(state.gpr, gprs_before, "CMP must preserve all GPRs");
            } else {
                let expected_result = match operation {
                    ArithmeticFlagOperation::Add => lhs.wrapping_add(rhs),
                    ArithmeticFlagOperation::Subtract => lhs.wrapping_sub(rhs),
                    ArithmeticFlagOperation::Compare => unreachable!(),
                };
                assert_eq!(state.gpr[0], expected_result);
                assert_eq!(state.gpr[1], gprs_before[1]);
            }
            assert_eq!(state.rip, BASE_RIP + 3);
        }
    }

    #[test]
    fn successive_arithmetic_replaces_all_six_defined_bits() {
        let old_flags = 0xFFFF_FFFF_FFFF_FFFF;
        let after_carry =
            updated_arithmetic_flags(ArithmeticFlagOperation::Add, u64::MAX, 1, old_flags);
        let after_zero = updated_arithmetic_flags(ArithmeticFlagOperation::Add, 0, 0, after_carry);
        assert_eq!(after_carry & DEFINED_ARITHMETIC_FLAGS, 0x055);
        assert_eq!(after_zero & DEFINED_ARITHMETIC_FLAGS, 0x044);
        assert_eq!(
            after_zero & !DEFINED_ARITHMETIC_FLAGS,
            old_flags & !DEFINED_ARITHMETIC_FLAGS
        );
    }
}
