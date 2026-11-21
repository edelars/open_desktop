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
//! - S2-55 adds ROUND* and DPPS/DPPD on the same lane core (Intel SDM Vol. 2B pp. 4-583..4-591, Vol. 2A pp. 3-277..3-281).
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
                    SseFpArithKind::AddSub | SseFpArithKind::HAdd | SseFpArithKind::HSub => {
                        unreachable!("the caller resolves these to Add or Sub for each element")
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

/// The post-computation response to one lane (see the module documentation): the delivered bits, the flags it
/// contributes and whether an unmasked overflow or underflow stops the instruction (the unmasked precision
/// exception is decided by the caller from the final flags, as it faults after the masked response).
struct Settled {
    bits: u64,
    flags: u32,
    fault: bool,
}

/// A lane whose overflow or underflow is unmasked reports OE or UE and, as the precision flag, the inexactness
/// of the unbounded-exponent rounding (no bounded result exists). Any other lane gets the masked response:
/// FTZ, the flags and the delivered result.
fn settle(lane: &Lane, masks: u32, mxcsr: u32, sign: u64) -> Settled {
    let overflow_masked = masks & MXCSR_OE != 0;
    let underflow_masked = masks & MXCSR_UE != 0;
    let flush = mxcsr & MXCSR_FTZ != 0 && underflow_masked;
    let mut bits = lane.bits;
    let mut flags = 0;
    if (lane.overflow && !overflow_masked) || (lane.tiny && !underflow_masked) {
        flags |= if lane.overflow { MXCSR_OE } else { MXCSR_UE };
        if lane.unbounded_inexact {
            flags |= MXCSR_PE;
        }
        return Settled {
            bits,
            flags,
            fault: true,
        };
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
    Settled {
        bits,
        flags,
        fault: false,
    }
}

arith_lane!(arith_lane32, Binary32, u32, 1 << 31);
arith_lane!(arith_lane64, Binary64, u64, 1 << 63);

/// Executes `kind` on `destination` (first source) and `source` (second source) under `mxcsr`.
///
/// S2-54: the SSE3 packed forms (ADDSUB, HADD, HSUB) run the same lane arithmetic with a different pairing of
/// the source elements (see the loop below); their exceptions, flags and MXCSR policy are those of ADD and SUB.
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
        // The operands and the operation of each result element. The horizontal forms combine the two
        // elements of an adjacent pair (the pairs of the destination fill the low half of the result, those of
        // the source the high half): HADD is `low + high`, HSUB `low - high`. The SDM pseudocode writes HADD with
        // the high element first (`SRC1[63:32] + SRC1[31:0]`); the order here deliberately differs from that text,
        // because the hardware takes the low element as the first operand (checked on the oracle), and the first
        // operand decides which NaN wins. Precondition: HAdd/HSub only come with a packed shape (count is 2 or 4,
        // so `half` is at least 1); the decoder never builds them with a scalar shape, and `index % half` would
        // divide by zero for a hand-built scalar one.
        // ADDSUB subtracts in the even elements and adds in the odd ones.
        let (kind, lhs, rhs) = match kind {
            SseFpArithKind::HAdd | SseFpArithKind::HSub => {
                let half = count / 2;
                let vector = if index < half { &destination } else { &source };
                let pair = (index % half) * 2;
                let operation = if kind == SseFpArithKind::HAdd {
                    SseFpArithKind::Add
                } else {
                    SseFpArithKind::Sub
                };
                (operation, element(vector, pair), element(vector, pair + 1))
            }
            SseFpArithKind::AddSub => (
                if index % 2 == 0 {
                    SseFpArithKind::Sub
                } else {
                    SseFpArithKind::Add
                },
                element(&destination, index),
                element(&source, index),
            ),
            kind => (kind, element(&destination, index), element(&source, index)),
        };
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
    let sign = if double { 1u64 << 63 } else { 1 << 31 };
    let mut flags = pre;
    let mut fault = false;
    let mut result = destination;
    for (index, lane) in lanes.iter().enumerate() {
        let Some(lane) = lane else { break };
        let settled = settle(lane, masks, mxcsr, sign);
        flags |= settled.flags;
        if settled.fault {
            fault = true;
            continue;
        }
        result[index * width..(index + 1) * width]
            .copy_from_slice(&settled.bits.to_le_bytes()[..width]);
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
/// S2-37 (remainder): CVTPS2PD, CVTPD2PS, CVTSS2SD and CVTSD2SS (`shape` is the one the prefix selects, see
/// [`convert`]) and RCPPS, RCPSS, RSQRTPS and RSQRTSS (see [`reciprocal`]).
pub(crate) fn misc(
    kind: SseFpMiscKind,
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    immediate: u8,
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    match kind {
        SseFpMiscKind::Cvtps2pd
        | SseFpMiscKind::Cvtpd2ps
        | SseFpMiscKind::Cvtss2sd
        | SseFpMiscKind::Cvtsd2ss
        | SseFpMiscKind::Cvtdq2pd
        | SseFpMiscKind::Cvtpd2dq
        | SseFpMiscKind::Cvttpd2dq
        | SseFpMiscKind::Cvtpi2ps
        | SseFpMiscKind::Cvtpi2pd
        | SseFpMiscKind::Cvtps2pi
        | SseFpMiscKind::Cvttps2pi
        | SseFpMiscKind::Cvtpd2pi
        | SseFpMiscKind::Cvttpd2pi => return convert(kind, destination, source, mxcsr),
        SseFpMiscKind::Rcp | SseFpMiscKind::Rsqrt => {
            return reciprocal(kind, shape, destination, source, mxcsr);
        }
        _ => {}
    }
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
                float_to_int_lane(rhs, (false, false), mode, mxcsr)
            }
            SseFpMiscKind::Comi | SseFpMiscKind::Ucomi => {
                unreachable!("COMISS and UCOMISS write EFLAGS: see compare_flags")
            }
            SseFpMiscKind::Cvtps2pd
            | SseFpMiscKind::Cvtpd2ps
            | SseFpMiscKind::Cvtss2sd
            | SseFpMiscKind::Cvtsd2ss
            | SseFpMiscKind::Cvtdq2pd
            | SseFpMiscKind::Cvtpd2dq
            | SseFpMiscKind::Cvttpd2dq
            | SseFpMiscKind::Cvtpi2ps
            | SseFpMiscKind::Cvtpi2pd
            | SseFpMiscKind::Cvtps2pi
            | SseFpMiscKind::Cvttps2pi
            | SseFpMiscKind::Cvtpd2pi
            | SseFpMiscKind::Cvttpd2pi
            | SseFpMiscKind::Rcp
            | SseFpMiscKind::Rsqrt => unreachable!("handled above"),
        });
    }
    commit(lanes, width, shape.is_double(), destination, mxcsr)
}

/// DAZ: a denormal source is a zero of the same sign (the denormal-operand exception is not reported then).
fn daz32(bits: u32, mxcsr: u32) -> u32 {
    if mxcsr & MXCSR_DAZ != 0 && Binary32::from_bits(bits).class() == BinaryClass::Subnormal {
        bits & (1 << 31)
    } else {
        bits
    }
}

fn daz64(bits: u64, mxcsr: u32) -> u64 {
    if mxcsr & MXCSR_DAZ != 0 && Binary64::from_bits(bits).class() == BinaryClass::Subnormal {
        bits & (1 << 63)
    } else {
        bits
    }
}

/// One CVTPS2PD/CVTSS2SD element: the exact widening of a binary32. A signaling NaN is quieted and invalid, a
/// denormal source (without DAZ) is a denormal operand; nothing rounds.
fn widen_lane(bits: u32, mxcsr: u32) -> Lane {
    let class = Binary32::from_bits(bits).class();
    let widened = Binary64::widen_from_binary32(Binary32::from_bits(daz32(bits, mxcsr)));
    let denormal = class == BinaryClass::Subnormal && mxcsr & MXCSR_DAZ == 0;
    Lane {
        bits: widened.value().to_bits(),
        pre: if widened.invalid_operation() {
            MXCSR_IE
        } else {
            0
        } | if denormal { MXCSR_DE } else { 0 },
        overflow: false,
        inexact: false,
        unbounded_inexact: false,
        tiny: false,
    }
}

/// One CVTPD2PS/CVTSD2SS element: the binary64 rounded to binary32 under MXCSR.RC (overflow, underflow and
/// precision as for the arithmetic forms; a NaN keeps the top 23 bits of its payload, a signaling NaN is quieted
/// and invalid).
fn narrow_lane(bits: u64, mxcsr: u32) -> Lane {
    let class = Binary64::from_bits(bits).class();
    let narrowed = Binary32::narrow_from_binary64(
        Binary64::from_bits(daz64(bits, mxcsr)),
        rounding_mode(mxcsr),
    );
    let denormal = class == BinaryClass::Subnormal && mxcsr & MXCSR_DAZ == 0;
    Lane {
        bits: u64::from(narrowed.value().to_bits()),
        pre: if narrowed.invalid_operation() {
            MXCSR_IE
        } else {
            0
        } | if denormal { MXCSR_DE } else { 0 },
        overflow: narrowed.overflow(),
        inexact: narrowed.inexact(),
        unbounded_inexact: narrowed.unbounded_inexact(),
        tiny: narrowed.tiny_after_precision_rounding(),
    }
}

/// CVTPS2PD (two low binary32 of `source` to a binary64 pair), CVTPD2PS (a binary64 pair to two binary32 in the
/// low half, the high half zero), CVTSS2SD and CVTSD2SS (element 0 only, the bits above the result element are the
/// destination's). Intel SDM Vol. 2A CVTPD2PS pp. 3-219..3-222, CVTPS2PD pp. 3-228..3-230, CVTSD2SS pp. 3-234..3-235,
/// CVTSS2SD pp. 3-240..3-241. Exceptions: CVTPS2PD and CVTSS2SD invalid and denormal only; CVTPD2PS and CVTSD2SS also
/// overflow, underflow and precision.
fn convert(kind: SseFpMiscKind, destination: [u8; 16], source: [u8; 16], mxcsr: u32) -> FpOutcome {
    let low32 = |index: usize| {
        u32::from_le_bytes(
            source[index * 4..index * 4 + 4]
                .try_into()
                .expect("four bytes"),
        )
    };
    let low64 = |index: usize| {
        u64::from_le_bytes(
            source[index * 8..index * 8 + 8]
                .try_into()
                .expect("eight bytes"),
        )
    };
    let mut lanes = [None; 4];
    // `width` and `double` describe the destination elements (the FTZ sign and the lane stride).
    let (width, double) = match kind {
        SseFpMiscKind::Cvtps2pd => {
            lanes[0] = Some(widen_lane(low32(0), mxcsr));
            lanes[1] = Some(widen_lane(low32(1), mxcsr));
            (8, true)
        }
        SseFpMiscKind::Cvtss2sd => {
            lanes[0] = Some(widen_lane(low32(0), mxcsr));
            (8, true)
        }
        SseFpMiscKind::Cvtpd2ps => {
            lanes[0] = Some(narrow_lane(low64(0), mxcsr));
            lanes[1] = Some(narrow_lane(low64(1), mxcsr));
            (4, false)
        }
        SseFpMiscKind::Cvtsd2ss => {
            lanes[0] = Some(narrow_lane(low64(0), mxcsr));
            (4, false)
        }
        // CVTDQ2PD: the two low signed doublewords, converted exactly (no flag exists).
        SseFpMiscKind::Cvtdq2pd => {
            lanes[0] = Some(int_to_float_lane(i64::from(low32(0) as i32), true, mxcsr));
            lanes[1] = Some(int_to_float_lane(i64::from(low32(1) as i32), true, mxcsr));
            (8, true)
        }
        // CVTPD2DQ / CVTTPD2DQ: the two binary64 to signed doublewords in the low half. CVTPD2PI / CVTTPD2PI give the
        // same two doublewords; the executor takes them from the low half of the result for the MMX register.
        SseFpMiscKind::Cvtpd2dq
        | SseFpMiscKind::Cvttpd2dq
        | SseFpMiscKind::Cvtpd2pi
        | SseFpMiscKind::Cvttpd2pi => {
            let mode = if matches!(kind, SseFpMiscKind::Cvttpd2dq | SseFpMiscKind::Cvttpd2pi) {
                RationalRoundingMode::TowardZero
            } else {
                rounding_mode(mxcsr)
            };
            lanes[0] = Some(float_to_int_lane(low64(0), (true, false), mode, mxcsr));
            lanes[1] = Some(float_to_int_lane(low64(1), (true, false), mode, mxcsr));
            (4, false)
        }
        // CVTPS2PI / CVTTPS2PI: the two low binary32 to signed doublewords (the MMX register is the low half).
        SseFpMiscKind::Cvtps2pi | SseFpMiscKind::Cvttps2pi => {
            let mode = if kind == SseFpMiscKind::Cvttps2pi {
                RationalRoundingMode::TowardZero
            } else {
                rounding_mode(mxcsr)
            };
            lanes[0] = Some(float_to_int_lane(
                u64::from(low32(0)),
                (false, false),
                mode,
                mxcsr,
            ));
            lanes[1] = Some(float_to_int_lane(
                u64::from(low32(1)),
                (false, false),
                mode,
                mxcsr,
            ));
            (4, false)
        }
        // CVTPI2PS: the two signed doublewords of the MMX source to two binary32 in the low half; the high half of the
        // destination is kept. CVTPI2PD: to two binary64 (exact).
        SseFpMiscKind::Cvtpi2ps => {
            lanes[0] = Some(int_to_float_lane(i64::from(low32(0) as i32), false, mxcsr));
            lanes[1] = Some(int_to_float_lane(i64::from(low32(1) as i32), false, mxcsr));
            (4, false)
        }
        SseFpMiscKind::Cvtpi2pd => {
            lanes[0] = Some(int_to_float_lane(i64::from(low32(0) as i32), true, mxcsr));
            lanes[1] = Some(int_to_float_lane(i64::from(low32(1) as i32), true, mxcsr));
            (8, true)
        }
        _ => unreachable!("convert handles the precision and the packed integer conversions"),
    };
    // CVTPD2PS, CVTPD2DQ and CVTTPD2DQ write the whole destination: the high quadword becomes zero.
    let mut base = destination;
    if matches!(
        kind,
        SseFpMiscKind::Cvtpd2ps
            | SseFpMiscKind::Cvtpd2dq
            | SseFpMiscKind::Cvttpd2dq
            | SseFpMiscKind::Cvtpd2pi
            | SseFpMiscKind::Cvttpd2pi
    ) {
        base[8..].fill(0);
    }
    let mut outcome = commit(lanes, width, double, base, mxcsr);
    if outcome.fault {
        outcome.result = destination;
    }
    outcome
}

/// One signed-integer-to-binary lane (CVTDQ2PD, CVTSI2SS, CVTSI2SD): `value` rounded to binary64 (`double`) or
/// binary32 under MXCSR.RC. Only the precision flag can arise: a binary32 holds every magnitude of a 64-bit
/// integer without overflow, and a binary64 holds every 32-bit one exactly.
fn int_to_float_lane(value: i64, double: bool, mxcsr: u32) -> Lane {
    let (bits, inexact) = if double {
        let converted = Binary64::from_i64(value, rounding_mode(mxcsr));
        (converted.value().to_bits(), converted.inexact())
    } else {
        let converted = Binary32::from_i64(value, rounding_mode(mxcsr));
        (u64::from(converted.value().to_bits()), converted.inexact())
    };
    Lane {
        bits,
        pre: 0,
        overflow: false,
        inexact,
        unbounded_inexact: false,
        tiny: false,
    }
}
/// One floating-point-to-signed-integer lane (CVTPS2DQ, CVTTPS2DQ, CVTPD2DQ, CVTTPD2DQ, CVT[T]SS2SI,
/// CVT[T]SD2SI): `bits` is a binary64 (`double`) or binary32 source; `wide` selects a 64-bit result, else the
/// result is 32 bits (zero-extended in the lane). A NaN or an out-of-range value is the integer indefinite
/// value and invalid. DAZ: a denormal source is a zero of the same sign, and none of these instructions reports
/// a denormal-operand exception (SDM: "Invalid, Precision").
fn float_to_int_lane(
    bits: u64,
    (double, wide): (bool, bool),
    mode: RationalRoundingMode,
    mxcsr: u32,
) -> Lane {
    let (value, invalid, inexact) = match (double, wide) {
        (false, false) => {
            let converted = Binary32::from_bits(daz32(bits as u32, mxcsr)).to_i32(mode);
            (
                u64::from(*converted.value() as u32),
                converted.invalid_operation(),
                converted.inexact(),
            )
        }
        (false, true) => {
            let converted = Binary32::from_bits(daz32(bits as u32, mxcsr)).to_i64(mode);
            (
                *converted.value() as u64,
                converted.invalid_operation(),
                converted.inexact(),
            )
        }
        (true, false) => {
            let converted = Binary64::from_bits(daz64(bits, mxcsr)).to_i32(mode);
            (
                u64::from(*converted.value() as u32),
                converted.invalid_operation(),
                converted.inexact(),
            )
        }
        (true, true) => {
            let converted = Binary64::from_bits(daz64(bits, mxcsr)).to_i64(mode);
            (
                *converted.value() as u64,
                converted.invalid_operation(),
                converted.inexact(),
            )
        }
    };
    Lane {
        bits: value,
        pre: if invalid { MXCSR_IE } else { 0 },
        overflow: false,
        inexact,
        unbounded_inexact: false,
        tiny: false,
    }
}

/// The outcome of CVTSS2SI, CVTTSS2SI, CVTSD2SI or CVTTSD2SI: the integer for the general register (the 32-bit
/// result in the low half, zero above), the MXCSR flag bits to OR in and whether an unmasked exception faults
/// (the register is not written then).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct IntOutcome {
    pub(crate) value: u64,
    pub(crate) flags: u32,
    pub(crate) fault: bool,
}

/// CVTSS2SI, CVTTSS2SI (`double` false) and CVTSD2SI, CVTTSD2SI (`double` true) (Intel SDM Vol. 2A pp. 3-232,
/// 3-242, 3-253, 3-255): element 0 of `source` to a 32-bit (`wide` false) or 64-bit signed integer, rounded by
/// MXCSR.RC or, for `truncate`, toward zero. `mxcsr` must not carry reserved bits.
pub(crate) fn float_to_int(
    (double, wide, truncate): (bool, bool, bool),
    source: [u8; 16],
    mxcsr: u32,
) -> IntOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    let bits = if double {
        u64::from_le_bytes(source[..8].try_into().expect("eight bytes"))
    } else {
        u64::from(u32::from_le_bytes(
            source[..4].try_into().expect("four bytes"),
        ))
    };
    let mode = if truncate {
        RationalRoundingMode::TowardZero
    } else {
        rounding_mode(mxcsr)
    };
    let lane = float_to_int_lane(bits, (double, wide), mode, mxcsr);
    let outcome = commit(
        [Some(lane), None, None, None],
        if wide { 8 } else { 4 },
        false,
        [0; 16],
        mxcsr,
    );
    IntOutcome {
        value: u64::from_le_bytes(outcome.result[..8].try_into().expect("eight bytes")),
        flags: outcome.flags,
        fault: outcome.fault,
    }
}

/// CVTSI2SS (`double` false) and CVTSI2SD (`double` true) (Intel SDM Vol. 2A pp. 3-236, 3-238): the signed
/// `value` (already sign-extended from 32 or 64 bits) becomes element 0 of `destination`; the bits above the
/// element are kept. Only the precision exception exists. `mxcsr` must not carry reserved bits.
pub(crate) fn int_to_float(
    double: bool,
    destination: [u8; 16],
    value: i64,
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    commit(
        [
            Some(int_to_float_lane(value, double, mxcsr)),
            None,
            None,
            None,
        ],
        if double { 8 } else { 4 },
        double,
        destination,
        mxcsr,
    )
}
/// RCPPS, RCPSS, RSQRTPS and RSQRTSS (Intel SDM Vol. 2B RCPPS pp. 4-538..4-539, RCPSS pp. 4-540..4-541, RSQRTPS
/// pp. 4-594..4-595, RSQRTSS pp. 4-596..4-597): approximations with a relative error of at most 1.5 * 2^-12. The
/// value of each lane is the deterministic reference of [`Binary32::reciprocal_sse_raw`] and
/// [`Binary32::reciprocal_sqrt_sse_raw`] (ISSUE-025: not a claim of bit equality with the hardware table); a
/// denormal source is a zero whatever DAZ says, MXCSR.RC and FTZ do not matter, and no exception or flag exists.
/// For the scalar shape the bits above element 0 are the destination's.
fn reciprocal(
    kind: SseFpMiscKind,
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    mxcsr: u32,
) -> FpOutcome {
    let count = if shape.is_packed() { 4 } else { 1 };
    let mut lanes = [None; 4];
    for (index, slot) in lanes.iter_mut().enumerate().take(count) {
        let operand = Binary32::from_bits(u32::from_le_bytes(
            source[index * 4..index * 4 + 4]
                .try_into()
                .expect("four bytes"),
        ));
        let result = if kind == SseFpMiscKind::Rcp {
            operand.reciprocal_sse_raw()
        } else {
            operand.reciprocal_sqrt_sse_raw()
        };
        *slot = Some(Lane {
            bits: u64::from(result.to_bits()),
            pre: 0,
            overflow: false,
            inexact: false,
            unbounded_inexact: false,
            tiny: false,
        });
    }
    commit(lanes, 4, false, destination, mxcsr)
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
/// Intel SDM Vol. 2B ROUNDPS/ROUNDPD/ROUNDSS/ROUNDSD pp. 4-583..4-591 (S2-55): the rounding mode is MXCSR.RC
/// when imm8[2] = 1 and imm8[1:0] otherwise (the same encoding as RC); imm8[3] = 1 suppresses the precision
/// exception whatever the MXCSR precision mask says.
fn round_mode(imm8: u8, mxcsr: u32) -> RationalRoundingMode {
    if imm8 & 4 != 0 {
        rounding_mode(mxcsr)
    } else {
        rounding_mode(u32::from(imm8 & 3) << 13)
    }
}

/// Rounds a finite encoding to an integral value in the given mode: the encoding of the result (a zero keeps
/// the sign of the source) and whether the rounding discarded a nonzero fraction. Integer arithmetic on the
/// raw fields only. `frac_bits` is 23 or 52.
fn round_to_integral(bits: u64, frac_bits: u32, mode: RationalRoundingMode) -> (u64, bool) {
    let exp_bits = if frac_bits == 23 { 8 } else { 11 };
    let bias = (1i32 << (exp_bits - 1)) - 1;
    let sign_mask = 1u64 << (frac_bits + exp_bits);
    let sign = bits & sign_mask != 0;
    let exponent = ((bits >> frac_bits) & ((1 << exp_bits) - 1)) as i32;
    let fraction = bits & ((1 << frac_bits) - 1);
    let (significand, unbiased) = if exponent == 0 {
        (fraction, 1 - bias)
    } else {
        (fraction | (1 << frac_bits), exponent - bias)
    };
    // The value is significand * 2^(unbiased - frac_bits): at or above 2^frac_bits it is integral already.
    if significand == 0 || unbiased >= frac_bits as i32 {
        return (bits, false);
    }
    // At 120 or more discarded bits the value is below one half whatever the exact shift is.
    let shift = (frac_bits as i32 - unbiased).min(120) as u32;
    let significand = u128::from(significand);
    let integral = significand >> shift;
    let remainder = significand & ((1u128 << shift) - 1);
    let half = 1u128 << (shift - 1);
    let inexact = remainder != 0;
    let up = match mode {
        RationalRoundingMode::NearestTiesEven => {
            remainder > half || (remainder == half && integral & 1 == 1)
        }
        RationalRoundingMode::TowardNegative => sign && inexact,
        RationalRoundingMode::TowardPositive => !sign && inexact,
        RationalRoundingMode::TowardZero => false,
    };
    let integral = integral + u128::from(up);
    let sign_bits = if sign { sign_mask } else { 0 };
    if integral == 0 {
        return (sign_bits, inexact);
    }
    // integral <= 2^frac_bits, so the leading bit is at most at frac_bits.
    let leading = 127 - integral.leading_zeros();
    let mantissa = ((integral << (frac_bits - leading)) as u64) & ((1 << frac_bits) - 1);
    let exponent = u64::from(leading) + bias as u64;
    (sign_bits | (exponent << frac_bits) | mantissa, inexact)
}

/// One ROUND* lane: the delivered bits, the invalid-operation flag (a signaling NaN, which is quieted) and
/// the precision candidate (the rounding discarded a fraction). Denormal is never signaled; DAZ turns a
/// denormal source into a zero of the same sign before the rounding. Infinities, zeros and quiet NaNs are
/// returned unchanged.
fn round_lane(bits: u64, double: bool, imm8: u8, mxcsr: u32) -> (u64, bool, bool) {
    let (frac_bits, quiet) = if double {
        (52, 1u64 << 51)
    } else {
        (23, 1 << 22)
    };
    let class = if double {
        Binary64::from_bits(bits).class()
    } else {
        Binary32::from_bits(u32::try_from(bits).expect("a binary32 lane")).class()
    };
    match class {
        BinaryClass::SignalingNaN => (bits | quiet, true, false),
        BinaryClass::QuietNaN | BinaryClass::Infinity | BinaryClass::Zero => (bits, false, false),
        BinaryClass::Subnormal if mxcsr & MXCSR_DAZ != 0 => (
            bits & (1 << (frac_bits + if double { 11 } else { 8 })),
            false,
            false,
        ),
        _ => {
            let (rounded, inexact) = round_to_integral(bits, frac_bits, round_mode(imm8, mxcsr));
            (rounded, false, inexact)
        }
    }
}

/// ROUNDPS/ROUNDPD (packed) and ROUNDSS/ROUNDSD (scalar, the bits above the element are the destination's;
/// `source` supplies the element). Invalid (signaling NaN) and precision are the only exceptions; the
/// precision exception exists only when imm8[3] = 0. An unmasked exception leaves the destination unchanged
/// and sets the detected flags.
pub(crate) fn round(
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    imm8: u8,
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    let width = if shape.is_double() { 8 } else { 4 };
    let count = if shape.is_packed() { 16 / width } else { 1 };
    let masks = mxcsr >> MXCSR_MASK_SHIFT & 0x3f;
    let mut result = destination;
    let mut flags = 0;
    let mut precision = false;
    for index in 0..count {
        let mut raw = [0u8; 8];
        raw[..width].copy_from_slice(&source[index * width..(index + 1) * width]);
        let (bits, invalid, inexact) =
            round_lane(u64::from_le_bytes(raw), shape.is_double(), imm8, mxcsr);
        if invalid {
            flags |= MXCSR_IE;
        }
        precision |= inexact && imm8 & 8 == 0;
        result[index * width..(index + 1) * width].copy_from_slice(&bits.to_le_bytes()[..width]);
    }
    // The invalid operation is a pre-computation exception: unmasked, it stops the instruction before the
    // precision exception is evaluated, so only IE is reported (confirmed against hardware).
    if flags & !masks != 0 {
        return FpOutcome {
            result: destination,
            flags,
            fault: true,
        };
    }
    if precision {
        flags |= MXCSR_PE;
    }
    let fault = flags & !masks != 0;
    FpOutcome {
        result: if fault { destination } else { result },
        flags,
        fault,
    }
}
/// DPPS and DPPD (Intel SDM Vol. 2A pp. 3-277..3-281): the products selected by imm8[7:4] (DPPS) or
/// imm8[5:4] (DPPD) are multiplied, the others are +0.0; the products are summed pairwise (DPPS: (0+1)+(2+3))
/// and the sum is broadcast to the elements selected by imm8[3:0] (DPPD: imm8[1:0]), the others become +0.0.
/// Every multiply and add is one IEEE operation of the general core under the MXCSR (rounding, DAZ, FTZ,
/// masks).
pub(crate) fn dot_product(
    shape: SseFpShape,
    destination: [u8; 16],
    source: [u8; 16],
    imm8: u8,
    mxcsr: u32,
) -> FpOutcome {
    debug_assert_eq!(mxcsr & MXCSR_RESERVED, 0);
    debug_assert!(shape.is_packed());
    let double = shape.is_double();
    let width = if double { 8 } else { 4 };
    let count = 16 / width;
    let masks = mxcsr >> MXCSR_MASK_SHIFT & 0x3f;
    let sign = if double { 1u64 << 63 } else { 1 << 31 };
    let (mut flags, mut fault) = (0, false);
    // One stage of the datapath: the operations of a stage are independent. The pre-computation exceptions of
    // all of them are gathered first and, unmasked, stop the instruction with only those flags; then the
    // masked or unmasked responses of all of them (an unmasked overflow, underflow or precision exception
    // stops the instruction before the next stage, which then reports nothing). Confirmed against hardware.
    let mut stage = |operations: &[Option<(SseFpArithKind, u64, u64)>]| -> [u64; 4] {
        let mut out = [0u64; 4];
        if fault {
            return out;
        }
        let mut lanes = [None; 4];
        let mut pre = 0;
        for (slot, operation) in lanes.iter_mut().zip(operations) {
            if let Some((kind, lhs, rhs)) = *operation {
                let lane = if double {
                    arith_lane64(kind, lhs, rhs, mxcsr)
                } else {
                    arith_lane32(kind, lhs as u32, rhs as u32, mxcsr)
                };
                pre |= lane.pre;
                *slot = Some(lane);
            }
        }
        flags |= pre;
        if pre & !masks != 0 {
            fault = true;
            return out;
        }
        for (index, lane) in lanes.iter().enumerate() {
            let Some(lane) = lane else { continue };
            let settled = settle(lane, masks, mxcsr, sign);
            flags |= settled.flags;
            fault |= settled.fault;
            out[index] = settled.bits;
        }
        fault |= flags & MXCSR_PE != 0 && masks & MXCSR_PE == 0;
        out
    };
    let element = |vector: &[u8; 16], index: usize| {
        let mut raw = [0u8; 8];
        raw[..width].copy_from_slice(&vector[index * width..(index + 1) * width]);
        u64::from_le_bytes(raw)
    };
    let mut multiplies = [None; 4];
    for (index, operation) in multiplies.iter_mut().enumerate().take(count) {
        if imm8 >> (4 + index) & 1 != 0 {
            *operation = Some((
                SseFpArithKind::Mul,
                element(&destination, index),
                element(&source, index),
            ));
        }
    }
    let products = stage(&multiplies);
    let add = SseFpArithKind::Add;
    // The sum feeding the low half of the destination and the one feeding the high half are separate adders
    // with swapped operands; the results differ only in which NaN wins (the first operand), which is
    // implementation specific in the SDM. Orders confirmed against hardware: DPPD adds (t0, t1) for the low
    // element and (t1, t0) for the high one; DPPS adds the pairs as (p1, p0) and (p3, p2), then (pair01,
    // pair23) for the low elements and (pair23, pair01) for the high ones.
    let (low_total, high_total) = if double {
        let sums = stage(&[
            Some((add, products[0], products[1])),
            Some((add, products[1], products[0])),
        ]);
        (sums[0], sums[1])
    } else {
        let pairs = stage(&[
            Some((add, products[1], products[0])),
            Some((add, products[3], products[2])),
        ]);
        let sums = stage(&[
            Some((add, pairs[0], pairs[1])),
            Some((add, pairs[1], pairs[0])),
        ]);
        (sums[0], sums[1])
    };
    let mut result = [0u8; 16];
    for index in 0..count {
        if imm8 >> index & 1 != 0 {
            let total = if index < count / 2 {
                low_total
            } else {
                high_total
            };
            result[index * width..(index + 1) * width]
                .copy_from_slice(&total.to_le_bytes()[..width]);
        }
    }
    FpOutcome {
        result: if fault { destination } else { result },
        flags,
        fault,
    }
}
