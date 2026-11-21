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

use crate::binary_float::{Binary32, Binary64, BinaryClass, RationalRoundingMode};
use crate::long_decode::{SseFpArithKind, SseFpShape};

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
            let denormal = !nan(left_class)
                && !nan(right_class)
                && !invalid
                && !divide_by_zero
                && (left_class == BinaryClass::Subnormal || right_class == BinaryClass::Subnormal);
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
    let sign = if shape.is_double() {
        1u64 << 63
    } else {
        1 << 31
    };
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
