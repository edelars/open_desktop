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
