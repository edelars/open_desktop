//! General SSE floating-point core (S2-37), shared by the instruction forms that follow it (S2-38).
//!
//! The module is pure: it takes the two raw source vectors and the MXCSR value and returns the raw result
//! vector, the MXCSR flag bits to set and whether the instruction faults. It reads no CPU state, touches no
//! memory and knows no instruction encoding beyond the shape (packed or scalar, binary32 or binary64), so the
//! executor owns operand access, the CR0/CR4 gates and the exception vector. The exact, correctly rounded
//! values for all four rounding modes come from [`crate::binary_float`] (integer arithmetic on the raw
//! encodings; no host floating point, no `libm`, no `fesetround`), so the result never depends on the host.
//!
//! Semantics (Intel SDM edition 325462-093US):
//! - Vol. 1 §10.2.3 (MXCSR: sticky flags bits 0..5, DAZ bit 6, masks bits 7..12, RC bits 13..14, FTZ bit 15),
//!   §11.5.2 (conditions and masked responses), §11.5.3 (pre- and post-computation exceptions, masked and
//!   unmasked handling), §4.8.3.5 Table 4-8 (SSE NaN rules: the first source operand wins, a signaling NaN
//!   is quieted), §4.9.1 and §4.9.2 (exception conditions and priority).
//! - Pre-computation exceptions (invalid, divide-by-zero, denormal operand) are detected on the sources
//!   before anything is computed; if one of them is unmasked the instruction faults, the destination is not
//!   written and the detected pre-computation flags are set. Post-computation exceptions (overflow,
//!   underflow, inexact) are detected on the rounded result; if one of them is unmasked the instruction
//!   faults and the destination is not written (§4.9.2: SSE does not update the destination).
//! - DAZ turns a denormal source into a zero of the same sign before the operation and suppresses the
//!   denormal-operand exception. FTZ turns a tiny result into a zero of the sign of the true result and sets
//!   UE and PE, only when the underflow exception is masked.
//! - Underflow is tiny-after-rounding (the result rounded to the destination precision with an unbounded
//!   exponent is nonzero and below the minimum normal); a masked underflow sets UE only if the result is
//!   also inexact, an unmasked one is reported for every nonzero tiny result.

use crate::binary_float::{
    Binary32, Binary64, BinaryClass, BinaryComparisonMode, BinaryComparisonRelation,
    RationalRoundingMode,
};
use crate::long_decode::{SseFpArithKind, SseFpMiscKind, SseFpShape};

/// MXCSR flag bits 0..5 (sticky): invalid, denormal operand, divide-by-zero, overflow, underflow, inexact.
pub(crate) const MXCSR_IE: u32 = 1 << 0;
pub(crate) const MXCSR_DE: u32 = 1 << 1;
pub(crate) const MXCSR_ZE: u32 = 1 << 2;
pub(crate) const MXCSR_OE: u32 = 1 << 3;
pub(crate) const MXCSR_UE: u32 = 1 << 4;
pub(crate) const MXCSR_PE: u32 = 1 << 5;
pub(crate) const MXCSR_DAZ: u32 = 1 << 6;
/// The six mask bits 7..12, in the order of the flag bits shifted left by 7.
const MXCSR_MASK_SHIFT: u32 = 7;
pub(crate) const MXCSR_FTZ: u32 = 1 << 15;
/// Bits 31:16 of MXCSR are reserved: LDMXCSR faults on them, so no architectural MXCSR carries them.
pub(crate) const MXCSR_RESERVED: u32 = 0xffff_0000;

/// The result of one arithmetic instruction on the raw operand vectors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FpOutcome {
    /// The destination vector. Equals the first source when `fault` is set (the destination is not written);
    /// for a scalar shape the bits above the element are the first source's.
    pub(crate) result: [u8; 16],
    /// MXCSR flag bits (0..5) to OR into MXCSR. Set also when `fault` is.
    pub(crate) flags: u32,
    /// An unmasked exception was detected: the instruction raises #XM (or #UD without CR4.OSXMMEXCPT).
    pub(crate) fault: bool,
}

/// One element's exact outcome before the MXCSR policy is applied.
#[derive(Clone, Copy, Debug)]
struct Lane {
    bits: u64,
    /// IE, DE and ZE bits detected before the computation.
    pre: u32,
    overflow: bool,
    inexact: bool,
    /// Rounding to the destination precision with an unbounded exponent range lost information: the
    /// inexactness an unmasked overflow or underflow reports (the delivered result does not exist then).
    unbounded_inexact: bool,
    tiny: bool,
}

pub(crate) const fn rounding_mode(mxcsr: u32) -> RationalRoundingMode {
    match (mxcsr >> 13) & 3 {
        0 => RationalRoundingMode::NearestTiesEven,
        1 => RationalRoundingMode::TowardNegative,
        2 => RationalRoundingMode::TowardPositive,
        _ => RationalRoundingMode::TowardZero,
    }
}

macro_rules! arith_lane {
    ($name:ident, $float:ident, $bits:ty, $sign:expr) => {
        fn $name(kind: SseFpArithKind, lhs: $bits, rhs: $bits, mxcsr: u32) -> Lane {
            let mode = rounding_mode(mxcsr);
            // DAZ: a denormal source is a zero of the same sign before anything else happens.
            let source = |bits: $bits| {
                if mxcsr & MXCSR_DAZ != 0
                    && $float::from_bits(bits).class() == BinaryClass::Subnormal
                {
                    bits & $sign
                } else {
                    bits
                }
            };
            let (lhs, rhs) = (source(lhs), source(rhs));
            let (left, right) = ($float::from_bits(lhs), $float::from_bits(rhs));
            let nan = |class| matches!(class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
            let (left_class, right_class) = (left.class(), right.class());
            // SQRT works on the second source alone: the first source takes no part in any exception.
            let uses_left = kind != SseFpArithKind::Sqrt;
            let (bits, invalid, divide_by_zero, overflow, inexact, unbounded_inexact, tiny) =
                match kind {
                    SseFpArithKind::Add | SseFpArithKind::Sub | SseFpArithKind::Mul => {
                        let result = match kind {
                            SseFpArithKind::Add => $float::add_raw_result(left, right, mode),
                            SseFpArithKind::Sub => $float::subtract_raw_result(left, right, mode),
                            _ => $float::multiply_raw_result(left, right, mode),
                        };
                        (
                            result.value().to_bits(),
                            result.invalid_operation(),
                            false,
                            result.overflow(),
                            result.inexact(),
                            result.unbounded_inexact(),
                            result.tiny_after_precision_rounding(),
                        )
                    }
                    SseFpArithKind::Sqrt => {
                        let result = $float::sqrt_raw_result(right, mode);
                        (
                            result.value().to_bits(),
                            result.invalid_operation(),
                            false,
                            result.overflow(),
                            result.inexact(),
                            result.unbounded_inexact(),
                            result.tiny_after_precision_rounding(),
                        )
                    }
                    SseFpArithKind::Min | SseFpArithKind::Max => {
                        // The second source is the result when either operand is a NaN (an SNaN is not
                        // quieted) or the operands compare equal; a NaN, quiet or signaling, is invalid.
                        let result = if kind == SseFpArithKind::Min {
                            $float::min_sse_raw_result(left, right)
                        } else {
                            $float::max_sse_raw_result(left, right)
                        };
                        (
                            result.value().to_bits(),
                            result.invalid_operation(),
                            false,
                            false,
                            false,
                            false,
                            false,
                        )
                    }
                    SseFpArithKind::Div => {
                        let result = $float::divide_raw_result(left, right, mode);
                        (
                            result.value().to_bits(),
                            result.invalid_operation(),
                            result.divide_by_zero(),
                            result.overflow(),
                            result.inexact(),
                            result.unbounded_inexact(),
                            result.tiny_after_precision_rounding(),
                        )
                    }
                };
            // A NaN operand is handled before any lower-priority condition (Vol. 1 §4.9.2), and an invalid
            // operation (priority 3 with divide-by-zero) before the denormal-operand exception (priority 4).
            let denormal = !(uses_left && nan(left_class))
                && !nan(right_class)
                && !invalid
                && !divide_by_zero
                && (uses_left && left_class == BinaryClass::Subnormal
                    || right_class == BinaryClass::Subnormal);
            Lane {
                bits: u64::from(bits),
                pre: if invalid { MXCSR_IE } else { 0 }
                    | if divide_by_zero { MXCSR_ZE } else { 0 }
                    | if denormal { MXCSR_DE } else { 0 },
                overflow,
                inexact,
                unbounded_inexact,
                tiny,
            }
        }
    };
}

arith_lane!(arith_lane32, Binary32, u32, 1 << 31);
arith_lane!(arith_lane64, Binary64, u64, 1 << 63);

/// Executes `kind` on `destination` (first source) and `source` (second source) under `mxcsr`.
///
/// For a scalar shape only element 0 is computed and the bits above it are the first source's; the bits of
/// `source` above the element are ignored. `mxcsr` must not carry reserved bits (the caller refuses such a
/// value).
pub(crate) fn arith(
    kind: SseFpArithKind,
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    let (width, count) = match shape {
        SseFpShape::Packed32 => (4, 4),
        SseFpShape::Packed64 => (8, 2),
        SseFpShape::Scalar32 => (4, 1),
        SseFpShape::Scalar64 => (8, 1),
    };
    let element = |vector: &[u8; 16], index: usize| {
        let mut raw = [0u8; 8];
        raw[..width].copy_from_slice(&vector[index * width..(index + 1) * width]);
        u64::from_le_bytes(raw)
    };
    let mut lanes = [None; 4];
    for (index, slot) in lanes.iter_mut().enumerate().take(count) {
        let (lhs, rhs) = (element(&destination, index), element(&source, index));
        *slot = Some(if shape.is_double() {
            arith_lane64(kind, lhs, rhs, mxcsr)
        } else {
            arith_lane32(kind, lhs as u32, rhs as u32, mxcsr)
        });
    }
    commit(lanes, width, shape.is_double(), destination, mxcsr)
}

/// Applies the MXCSR policy to the per-element outcomes and assembles the destination (shared by every
/// form of this module). `width` is the element size in bytes; `lanes` holds the computed elements, `None`
/// past the element count.
fn commit(
    lanes: [Option<Lane>; 4],
    width: usize,
    double: bool,
    destination: [u8; 16],
    mxcsr: u32,
) -> FpOutcome {
    let masks = mxcsr >> MXCSR_MASK_SHIFT & 0x3f;
    // Pre-computation exceptions: all detected ones set their flags; an unmasked one stops the instruction.
    let pre = lanes
        .iter()
        .flatten()
        .fold(0, |flags, lane| flags | lane.pre);
    if pre & !masks != 0 {
        return FpOutcome {
            result: destination,
            flags: pre,
            fault: true,
        };
    }
    // Post-computation exceptions on the rounded results, lane by lane. A lane whose overflow or underflow is
    // unmasked stops the instruction and reports OE or UE and, as the precision flag, the inexactness of the
    // unbounded-exponent rounding (no bounded result exists). Any other lane gets the masked response: the
    // flags and the delivered result; the masked precision flag then faults the instruction only if PE is
    // unmasked.
    let overflow_masked = masks & MXCSR_OE != 0;
    let underflow_masked = masks & MXCSR_UE != 0;
    let flush = mxcsr & MXCSR_FTZ != 0 && underflow_masked;
    let sign = if double { 1u64 << 63 } else { 1 << 31 };
    let mut flags = pre;
    let mut fault = false;
    let mut result = destination;
    for (index, lane) in lanes.iter().enumerate() {
        let Some(lane) = lane else { break };
        let mut bits = lane.bits;
        if (lane.overflow && !overflow_masked) || (lane.tiny && !underflow_masked) {
            fault = true;
            flags |= if lane.overflow { MXCSR_OE } else { MXCSR_UE };
            if lane.unbounded_inexact {
                flags |= MXCSR_PE;
            }
            continue;
        }
        if lane.overflow {
            flags |= MXCSR_OE;
        }
        if lane.tiny && flush {
            bits &= sign;
            flags |= MXCSR_UE | MXCSR_PE;
        } else if lane.tiny && lane.inexact {
            flags |= MXCSR_UE;
        }
        if lane.inexact {
            flags |= MXCSR_PE;
        }
        result[index * width..(index + 1) * width].copy_from_slice(&bits.to_le_bytes()[..width]);
    }
    let fault = fault || flags & MXCSR_PE != 0 && masks & MXCSR_PE == 0;
    FpOutcome {
        result: if fault { destination } else { result },
        flags,
        fault,
    }
}
macro_rules! compare_lane {
    ($name:ident, $float:ident, $bits:ty, $sign:expr) => {
        /// The relation of one pair and its pre-computation flags under DAZ. `signaling`: a quiet NaN is an
        /// invalid operand too (COMISS, and the LT/LE/NLT/NLE predicates), otherwise only a signaling NaN is.
        /// The denormal-operand exception needs two numbers: a NaN operand takes priority over it.
        fn $name(
            signaling: bool,
            lhs: $bits,
            rhs: $bits,
            mxcsr: u32,
        ) -> (BinaryComparisonRelation, u32) {
            let source = |bits: $bits| {
                if mxcsr & MXCSR_DAZ != 0
                    && $float::from_bits(bits).class() == BinaryClass::Subnormal
                {
                    bits & $sign
                } else {
                    bits
                }
            };
            let (left, right) = (
                $float::from_bits(source(lhs)),
                $float::from_bits(source(rhs)),
            );
            let result = $float::compare_raw(
                left,
                right,
                if signaling {
                    BinaryComparisonMode::Signaling
                } else {
                    BinaryComparisonMode::Quiet
                },
            );
            let relation = result.relation();
            let denormal = relation != BinaryComparisonRelation::Unordered
                && (left.class() == BinaryClass::Subnormal
                    || right.class() == BinaryClass::Subnormal);
            (
                relation,
                if result.invalid_operation() {
                    MXCSR_IE
                } else {
                    0
                } | if denormal { MXCSR_DE } else { 0 },
            )
        }
    };
}

compare_lane!(compare_lane32, Binary32, u32, 1 << 31);
compare_lane!(compare_lane64, Binary64, u64, 1 << 63);

/// Whether the CMPxx predicate (imm8 bits 2:0: EQ, LT, LE, UNORD, NEQ, NLT, NLE, ORD) holds for a relation.
fn predicate_holds(predicate: u8, relation: BinaryComparisonRelation) -> bool {
    use BinaryComparisonRelation::{Equal, Less, Unordered};
    match predicate & 7 {
        0 => relation == Equal,
        1 => relation == Less,
        2 => matches!(relation, Less | Equal),
        3 => relation == Unordered,
        4 => relation != Equal,
        5 => relation != Less,
        6 => !matches!(relation, Less | Equal),
        _ => relation != Unordered,
    }
}

/// Whether a predicate signals invalid for a quiet NaN operand too (the "S" predicates LT, LE, NLT, NLE).
const fn predicate_signals(predicate: u8) -> bool {
    matches!(predicate & 7, 1 | 2 | 5 | 6)
}

/// The instruction forms outside the four arithmetic operations that write a vector: CMPPS/CMPPD/CMPSS/CMPSD
/// (`immediate` bits 2:0 select the predicate; the caller refuses the reserved bits 7:3), CVTDQ2PS, CVTPS2DQ
/// and CVTTPS2DQ (`shape` is the packed binary32 shape: the elements are 32 bits wide in both formats).
pub(crate) fn misc(
    kind: SseFpMiscKind,
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    immediate: u8,
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    let (width, count) = match shape {
        SseFpShape::Packed32 => (4, 4),
        SseFpShape::Packed64 => (8, 2),
        SseFpShape::Scalar32 => (4, 1),
        SseFpShape::Scalar64 => (8, 1),
    };
    let element = |vector: &[u8; 16], index: usize| {
        let mut raw = [0u8; 8];
        raw[..width].copy_from_slice(&vector[index * width..(index + 1) * width]);
        u64::from_le_bytes(raw)
    };
    let mut lanes = [None; 4];
    for (index, slot) in lanes.iter_mut().enumerate().take(count) {
        let (lhs, rhs) = (element(&destination, index), element(&source, index));
        *slot = Some(match kind {
            SseFpMiscKind::Cmp => compare_mask_lane(shape.is_double(), immediate, lhs, rhs, mxcsr),
            SseFpMiscKind::Cvtdq2ps => {
                let converted =
                    Binary32::from_i64(i64::from(rhs as u32 as i32), rounding_mode(mxcsr));
                Lane {
                    bits: u64::from(converted.value().to_bits()),
                    pre: 0,
                    overflow: false,
                    inexact: converted.inexact(),
                    unbounded_inexact: false,
                    tiny: false,
                }
            }
            SseFpMiscKind::Cvtps2dq | SseFpMiscKind::Cvttps2dq => {
                let mode = if kind == SseFpMiscKind::Cvttps2dq {
                    RationalRoundingMode::TowardZero
                } else {
                    rounding_mode(mxcsr)
                };
                // DAZ: a denormal source is a zero of the same sign. The instruction reports no denormal-operand
                // exception at all (SDM: "Invalid, Precision").
                let mut bits = rhs as u32;
                if mxcsr & MXCSR_DAZ != 0
                    && Binary32::from_bits(bits).class() == BinaryClass::Subnormal
                {
                    bits &= 1 << 31;
                }
                let converted = Binary32::from_bits(bits).to_i32(mode);
                Lane {
                    bits: u64::from(*converted.value() as u32),
                    pre: if converted.invalid_operation() {
                        MXCSR_IE
                    } else {
                        0
                    },
                    overflow: false,
                    inexact: converted.inexact(),
                    unbounded_inexact: false,
                    tiny: false,
                }
            }
            SseFpMiscKind::Comi | SseFpMiscKind::Ucomi => {
                unreachable!("COMISS and UCOMISS write EFLAGS: see compare_flags")
            }
        });
    }
    commit(lanes, width, shape.is_double(), destination, mxcsr)
}

/// One CMPxx element: an all-ones or all-zero mask of the element width.
fn compare_mask_lane(double: bool, predicate: u8, lhs: u64, rhs: u64, mxcsr: u32) -> Lane {
    let signaling = predicate_signals(predicate);
    let (relation, pre) = if double {
        compare_lane64(signaling, lhs, rhs, mxcsr)
    } else {
        compare_lane32(signaling, lhs as u32, rhs as u32, mxcsr)
    };
    let ones = if double {
        u64::MAX
    } else {
        u64::from(u32::MAX)
    };
    Lane {
        bits: if predicate_holds(predicate, relation) {
            ones
        } else {
            0
        },
        pre,
        overflow: false,
        inexact: false,
        unbounded_inexact: false,
        tiny: false,
    }
}

/// EFLAGS bits CF, PF, ZF (the only ones COMISS/UCOMISS set from the comparison).
pub(crate) const EFLAGS_CF: u64 = 1 << 0;
pub(crate) const EFLAGS_PF: u64 = 1 << 2;
pub(crate) const EFLAGS_ZF: u64 = 1 << 6;
/// The six arithmetic flags COMISS/UCOMISS define: OF, SF, AF are cleared, ZF, PF, CF come from the relation.
pub(crate) const EFLAGS_COMPARE_MASK: u64 = 0x8d5;

/// The result of COMISS/COMISD/UCOMISS/UCOMISD.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CompareOutcome {
    /// The new value of the six arithmetic flags (EFLAGS bits in [`EFLAGS_COMPARE_MASK`]); only meaningful when
    /// `fault` is not set (EFLAGS is not updated then).
    pub(crate) eflags: u64,
    /// MXCSR flag bits to OR into MXCSR (also when `fault` is set).
    pub(crate) flags: u32,
    /// An unmasked invalid-operation or denormal-operand exception was detected.
    pub(crate) fault: bool,
}

/// COMISS/COMISD (`ordered`: a quiet NaN signals invalid) and UCOMISS/UCOMISD on the low element of
/// `destination` (operand 1) and `source` (operand 2). Unordered sets ZF, PF, CF; greater clears all three;
/// less sets CF; equal sets ZF.
pub(crate) fn compare_flags(
    ordered: bool,
    double: bool,
    destination: [u8; 16],
    source: [u8; 16],
    mxcsr: u32,
) -> CompareOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    let low = |vector: &[u8; 16]| {
        let mut raw = [0u8; 8];
        raw.copy_from_slice(&vector[..8]);
        u64::from_le_bytes(raw)
    };
    let (lhs, rhs) = (low(&destination), low(&source));
    let (relation, flags) = if double {
        compare_lane64(ordered, lhs, rhs, mxcsr)
    } else {
        compare_lane32(ordered, lhs as u32, rhs as u32, mxcsr)
    };
    let masks = mxcsr >> MXCSR_MASK_SHIFT & 0x3f;
    let eflags = match relation {
        BinaryComparisonRelation::Unordered => EFLAGS_ZF | EFLAGS_PF | EFLAGS_CF,
        BinaryComparisonRelation::Greater => 0,
        BinaryComparisonRelation::Less => EFLAGS_CF,
        BinaryComparisonRelation::Equal => EFLAGS_ZF,
    };
    CompareOutcome {
        eflags,
        flags,
        fault: flags & !masks != 0,
    }
}
