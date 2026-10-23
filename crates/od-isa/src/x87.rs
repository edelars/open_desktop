//! Lossless binary80 fields, raw x87 encoding classification, and finite
//! integer-only helper representations.
//!
//! This module does not execute x87 instructions. Its bounded integer helpers
//! preserve exact intermediate values. Pure rational helpers round integer
//! quotients either to an integer lattice or a requested precision under
//! mathematical rounding modes; they do not define architectural x87 rounding,
//! packing, or exception behavior. The byte array
//! follows memory order, least significant byte first (Intel SDM revision 093,
//! Vol. 1 §§4.2.2 and
//! 8.2.2, Tables 4-3 and 8-3).

/// An immutable, lossless 80-bit x87 double-extended encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87Binary80([u8; 10]);

/// Classification of the raw exponent/integer-bit/fraction encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X87Binary80Class {
    Zero,
    Subnormal,
    Normal,
    Infinity,
    QuietNaN,
    SignalingNaN,
    PseudoDenormal,
    Unnormal,
    PseudoInfinity,
    PseudoQuietNaN,
    PseudoSignalingNaN,
}

/// A finite binary80 value expressed with its leading significand bit at bit
/// 63. `exponent` is the power of two associated with that bit. For zero,
/// `significand` and `exponent` are both zero. `source_class` retains the raw
/// encoding class, including subnormal and pseudo-denormal provenance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87Finite {
    sign: bool,
    exponent: i32,
    significand: u64,
    source_class: X87Binary80Class,
}

/// Exact quotient decomposition of two nonzero normalized finite significands.
///
/// The represented value is `(-1)^sign * (quotient + remainder / denominator)
/// * 2^(exponent - 63)`. Keeping the original denominator makes the fractional
/// tail exact; this is not a rounded or packed x87 result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87FiniteQuotient {
    sign: bool,
    exponent: i32,
    quotient: u64,
    remainder: u64,
    denominator: u64,
}

/// Mathematical rounding direction for an exact rational coefficient.
///
/// These modes do not represent x87 control-word values or instruction state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RationalRoundingMode {
    NearestTiesEven,
    TowardZero,
    TowardPositive,
    TowardNegative,
}

impl X87FiniteQuotient {
    pub const fn sign(self) -> bool {
        self.sign
    }

    pub const fn exponent(self) -> i32 {
        self.exponent
    }

    pub const fn quotient(self) -> u64 {
        self.quotient
    }

    pub const fn remainder(self) -> u64 {
        self.remainder
    }

    pub const fn denominator(self) -> u64 {
        self.denominator
    }
}

impl X87Finite {
    pub const fn sign(self) -> bool {
        self.sign
    }

    pub const fn exponent(self) -> i32 {
        self.exponent
    }

    pub const fn significand(self) -> u64 {
        self.significand
    }

    pub const fn source_class(self) -> X87Binary80Class {
        self.source_class
    }

    pub const fn is_zero(self) -> bool {
        match self.source_class {
            X87Binary80Class::Zero => true,
            _ => false,
        }
    }
}

/// Divide finite significands exactly, retaining the quotient's fractional tail.
///
/// For nonzero inputs, the result satisfies
/// `lhs / rhs = (-1)^sign * (quotient + remainder / denominator) *
/// 2^(exponent - 63)`. The quotient is normalized to `[2^63, 2^64)`, and
/// `remainder < denominator`. Zero numerator with a nonzero denominator
/// canonicalizes to an unsigned zero decomposition. A zero denominator,
/// including `0 / 0`, returns `None`; this helper defines no guest exception.
/// No alignment, jam, GRS, rounding, or packing is performed.
pub fn divide_finite_significands(lhs: X87Finite, rhs: X87Finite) -> Option<X87FiniteQuotient> {
    if rhs.is_zero() {
        return None;
    }

    let denominator = rhs.significand;
    if lhs.is_zero() {
        return Some(X87FiniteQuotient {
            sign: false,
            exponent: 0,
            quotient: 0,
            remainder: 0,
            denominator,
        });
    }

    let lhs_significand = lhs.significand;
    let (shift, exponent_adjustment) = if lhs_significand >= denominator {
        (63, 0)
    } else {
        (64, -1)
    };
    let dividend = (lhs_significand as u128) << shift;
    let quotient = (dividend / denominator as u128) as u64;
    let remainder = (dividend % denominator as u128) as u64;
    let exponent = i64::from(lhs.exponent) - i64::from(rhs.exponent) + exponent_adjustment;
    let exponent = i32::try_from(exponent).ok()?;

    Some(X87FiniteQuotient {
        sign: lhs.sign ^ rhs.sign,
        exponent,
        quotient,
        remainder,
        denominator,
    })
}

/// Round the exact rational coefficient `(-1)^sign * (q + r / d)` to an
/// integer lattice and return its unsigned magnitude.
///
/// Returns `None` when `d == 0` or `r >= d`. The sign affects only directed
/// rounding; it does not produce signed zero or adjust an exponent. This is a
/// mathematical helper and defines no x87 control-word, status, packing, or
/// instruction behavior.
pub fn round_quotient_magnitude(
    sign: bool,
    q: u64,
    r: u64,
    d: u64,
    mode: RationalRoundingMode,
) -> Option<u128> {
    if d == 0 || r >= d {
        return None;
    }

    let increment = if r == 0 {
        false
    } else {
        match mode {
            RationalRoundingMode::NearestTiesEven => {
                let twice_remainder = u128::from(r) * 2;
                let denominator = u128::from(d);
                twice_remainder > denominator || (twice_remainder == denominator && q & 1 != 0)
            }
            RationalRoundingMode::TowardZero => false,
            RationalRoundingMode::TowardPositive => !sign,
            RationalRoundingMode::TowardNegative => sign,
        }
    };

    Some(u128::from(q) + u128::from(increment))
}

/// Round an exact normalized rational quotient to `precision` significant bits.
///
/// The exact coefficient is `(-1)^sign * (q + r / d)`. For nonzero inputs,
/// `q` must be normalized with bit 63 set. The returned `u128` is the rounded
/// significand magnitude; a carry to `2^precision` is preserved without
/// changing an exponent. When consumed with a divider result whose exponent is
/// `e`, the represented value is
/// `(-1)^sign * magnitude * 2^(e + 1 - precision)`. This helper does not adjust
/// that exponent, pack binary80, or define x87 control-word/exception behavior.
///
/// The quotient tail is compared exactly: discarded quotient bits and the
/// rational remainder jointly determine nearest rounding, so no intermediate
/// rounding or multiplication is needed.
pub fn round_quotient_to_precision(
    sign: bool,
    q: u64,
    r: u64,
    d: u64,
    precision: u32,
    mode: RationalRoundingMode,
) -> Option<u128> {
    if !(1..=64).contains(&precision) || d == 0 || r >= d {
        return None;
    }
    if (q == 0 && r != 0) || (q != 0 && q < (1_u64 << 63)) {
        return None;
    }

    if q == 0 {
        return Some(0);
    }

    let k = 64 - precision;
    let t = q >> k;
    let lo = q - (t << k);
    let inexact = lo != 0 || r != 0;
    let increment = match mode {
        RationalRoundingMode::NearestTiesEven => {
            if k == 0 {
                let complement = d - r;
                r > complement || (r == complement && t & 1 != 0)
            } else {
                let halfway = 1_u64 << (k - 1);
                lo > halfway || (lo == halfway && (r != 0 || t & 1 != 0))
            }
        }
        RationalRoundingMode::TowardZero => false,
        RationalRoundingMode::TowardPositive => inexact && !sign,
        RationalRoundingMode::TowardNegative => inexact && sign,
    };

    Some(u128::from(t) + u128::from(increment))
}

/// Round the unsigned rational value `q + r / d` to nearest with ties to even.
///
/// This compatibility wrapper is equivalent to a positive coefficient using
/// [`RationalRoundingMode::NearestTiesEven`].
pub fn round_quotient_ties_even(q: u64, r: u64, d: u64) -> Option<u128> {
    round_quotient_magnitude(false, q, r, d, RationalRoundingMode::NearestTiesEven)
}

/// Add two finite values exactly when their nonzero exponents match.
///
/// The returned coefficient represents `coefficient * 2^(exponent - 63)`.
/// Zero operands are neutral and do not select a signed-zero result. The
/// coefficient is deliberately not normalized, so carry and cancellation
/// information remains exact. Unequal nonzero exponents require a separate
/// arithmetic path and return `None` here.
pub fn add_finite_same_exponent(lhs: X87Finite, rhs: X87Finite) -> Option<(i32, i128)> {
    match (lhs.is_zero(), rhs.is_zero()) {
        (true, true) => Some((0, 0)),
        (true, false) => Some((rhs.exponent, signed_significand(rhs))),
        (false, true) => Some((lhs.exponent, signed_significand(lhs))),
        (false, false) if lhs.exponent == rhs.exponent => Some((
            lhs.exponent,
            signed_significand(lhs) + signed_significand(rhs),
        )),
        (false, false) => None,
    }
}

/// Normalize the exact signed coefficient of `coefficient * 2^(exponent - 63)`.
///
/// Nonzero results place the leading one of the unsigned magnitude at bit 127.
/// This operation shifts only left and preserves every coefficient bit. A zero
/// result has no architectural sign and returns `(false, 0, 0)`. `None` means
/// only that the normalized exponent does not fit in `i32`.
pub fn normalize_exact_coefficient(exponent: i32, coefficient: i128) -> Option<(bool, i32, u128)> {
    if coefficient == 0 {
        return Some((false, 0, 0));
    }

    let sign = coefficient < 0;
    let magnitude = coefficient.unsigned_abs();
    let highest_bit = u128::BITS - 1 - magnitude.leading_zeros();
    let exponent_adjustment = highest_bit as i32 - 63;
    let normalized_exponent = exponent.checked_add(exponent_adjustment)?;
    let normalized_magnitude = magnitude << (u128::BITS - 1 - highest_bit);

    Some((sign, normalized_exponent, normalized_magnitude))
}

/// Multiply two finite significands without alignment, jam bits, or
/// normalization.
///
/// For nonzero inputs, the result `(sign, exponent, magnitude)` represents
/// `(-1)^sign * magnitude * 2^(exponent - 127)`, where `sign` is the XOR of
/// the input signs and `magnitude` is the full unsigned 128-bit product. The
/// leading one may be at bit 126 or 127. `None` means only that the summed
/// exponent does not fit in `i32`. Any zero pair returns `(false, 0, 0)` with
/// no architectural signed-zero choice.
pub fn multiply_finite_significands(lhs: X87Finite, rhs: X87Finite) -> Option<(bool, i32, u128)> {
    if lhs.is_zero() || rhs.is_zero() {
        return Some((false, 0, 0));
    }

    let exponent = i64::from(lhs.exponent)
        .checked_add(i64::from(rhs.exponent))?
        .checked_add(1)?;
    let exponent = i32::try_from(exponent).ok()?;
    let magnitude = (lhs.significand as u128) * (rhs.significand as u128);

    Some((lhs.sign ^ rhs.sign, exponent, magnitude))
}

/// Normalize an exact unsigned magnitude representing
/// `(-1)^sign * magnitude * 2^(exponent - 127)`.
///
/// Nonzero results place the leading one at bit 127. This shifts only left
/// and preserves every magnitude bit. A zero result has no architectural sign
/// and returns `(false, 0, 0)`. `None` means only that the normalized exponent
/// does not fit in `i32`.
pub fn normalize_exact_magnitude(
    sign: bool,
    exponent: i32,
    magnitude: u128,
) -> Option<(bool, i32, u128)> {
    if magnitude == 0 {
        return Some((false, 0, 0));
    }

    let shift = magnitude.leading_zeros();
    let normalized_exponent = exponent.checked_sub(shift as i32)?;
    let normalized_magnitude = magnitude << shift;

    Some((sign, normalized_exponent, normalized_magnitude))
}

fn signed_significand(value: X87Finite) -> i128 {
    let significand = value.significand as i128;
    if value.sign {
        -significand
    } else {
        significand
    }
}

/// Align two finite values to a shared exponent without changing their order
/// or metadata. Nonzero significands are extended with three low GRS bits and
/// right-shift-jammed to `max(lhs.exponent, rhs.exponent)`. Zero operands
/// contribute a zero significand; two zeros use exponent zero.
///
/// The jammed low bit records lost-tail information. It is not an exact
/// magnitude comparison or an architectural inexact indication. Any future
/// subtraction or left renormalization must separately prove that this
/// representation retains the information that operation needs.
pub fn align_finite_pair(lhs: X87Finite, rhs: X87Finite) -> (i32, u128, u128) {
    let common_exponent = match (lhs.is_zero(), rhs.is_zero()) {
        (true, true) => 0,
        (true, false) => rhs.exponent,
        (false, true) => lhs.exponent,
        (false, false) => lhs.exponent.max(rhs.exponent),
    };

    let align = |value: X87Finite| {
        if value.is_zero() {
            0
        } else {
            let significand = (value.significand as u128) << 3;
            shift_right_jam_u128(significand, (common_exponent - value.exponent) as u32)
        }
    };

    (common_exponent, align(lhs), align(rhs))
}

/// The three low-order bits used as guard, round, and sticky information.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87Grs {
    guard: bool,
    round: bool,
    sticky: bool,
}

impl X87Grs {
    /// Interpret bits 2, 1, and 0 as guard, round, and sticky respectively.
    pub const fn from_low_bits(value: u64) -> Self {
        Self {
            guard: value & 0b100 != 0,
            round: value & 0b010 != 0,
            sticky: value & 0b001 != 0,
        }
    }

    pub const fn guard(self) -> bool {
        self.guard
    }

    pub const fn round(self) -> bool {
        self.round
    }

    pub const fn sticky(self) -> bool {
        self.sticky
    }
}

/// Shift right while OR-ing every discarded one bit into the result's least
/// significant bit. Shifts at least the word width produce only that sticky
/// bit. This helper performs no rounding.
pub const fn shift_right_jam(value: u64, distance: u32) -> u64 {
    if distance == 0 {
        value
    } else if distance < u64::BITS {
        let discarded_mask = (1_u64 << distance) - 1;
        (value >> distance) | if value & discarded_mask != 0 { 1 } else { 0 }
    } else {
        if value != 0 { 1 } else { 0 }
    }
}

/// Shift a `u128` right while OR-ing every discarded one bit into bit zero.
/// Distance zero is identity; distances at least 128 produce only the sticky
/// bit. Repeated right shifts preserve an existing sticky bit. Left shifts
/// cannot reconstruct bits discarded by this operation. This helper does no
/// rounding and defines no floating-point arithmetic.
pub const fn shift_right_jam_u128(value: u128, distance: u32) -> u128 {
    if distance == 0 {
        value
    } else if distance < u128::BITS {
        let discarded_mask = (1_u128 << distance) - 1;
        (value >> distance) | if value & discarded_mask != 0 { 1 } else { 0 }
    } else if value != 0 {
        1
    } else {
        0
    }
}

impl X87Binary80 {
    pub const BYTE_LEN: usize = 10;

    /// Preserve exactly the ten bytes as they appear in memory.
    pub const fn from_bytes(bytes: [u8; 10]) -> Self {
        Self(bytes)
    }

    /// Return exactly the original ten memory-order bytes.
    pub const fn to_bytes(self) -> [u8; 10] {
        self.0
    }

    pub const fn sign(self) -> bool {
        self.0[9] & 0x80 != 0
    }

    pub const fn exponent(self) -> u16 {
        (self.0[8] as u16) | (((self.0[9] & 0x7f) as u16) << 8)
    }

    /// Return all 64 significand bits, including the explicit integer bit.
    pub const fn significand(self) -> u64 {
        u64::from_le_bytes([
            self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6], self.0[7],
        ])
    }

    pub const fn integer_bit(self) -> bool {
        self.significand() & (1 << 63) != 0
    }

    /// Return the 63-bit fraction, excluding the explicit integer bit.
    pub const fn fraction(self) -> u64 {
        self.significand() & !(1 << 63)
    }

    /// The quiet/signaling discriminator is significand bit 62.
    pub const fn quiet_bit(self) -> bool {
        self.significand() & (1 << 62) != 0
    }

    pub const fn classify(self) -> X87Binary80Class {
        let exponent = self.exponent();
        let integer = self.integer_bit();
        let fraction = self.fraction();
        if exponent == 0 {
            if integer {
                X87Binary80Class::PseudoDenormal
            } else if fraction == 0 {
                X87Binary80Class::Zero
            } else {
                X87Binary80Class::Subnormal
            }
        } else if exponent == 0x7fff {
            if integer {
                if fraction == 0 {
                    X87Binary80Class::Infinity
                } else if self.quiet_bit() {
                    X87Binary80Class::QuietNaN
                } else {
                    X87Binary80Class::SignalingNaN
                }
            } else if fraction == 0 {
                X87Binary80Class::PseudoInfinity
            } else if self.quiet_bit() {
                X87Binary80Class::PseudoQuietNaN
            } else {
                X87Binary80Class::PseudoSignalingNaN
            }
        } else if integer {
            X87Binary80Class::Normal
        } else {
            X87Binary80Class::Unnormal
        }
    }

    /// Normalize a finite raw encoding into a 64-bit significand and an
    /// associated binary exponent. Nonfinite and unsupported encodings return
    /// `None`; the raw wrapper is never modified.
    pub fn normalize_finite(self) -> Option<X87Finite> {
        let source_class = self.classify();
        let (exponent, significand) = match source_class {
            X87Binary80Class::Zero => (0, 0),
            X87Binary80Class::Subnormal => {
                let significand = self.significand();
                let shift = significand.leading_zeros();
                (-16382 - shift as i32, significand << shift)
            }
            X87Binary80Class::PseudoDenormal => (-16382, self.significand()),
            X87Binary80Class::Normal => (self.exponent() as i32 - 16383, self.significand()),
            _ => return None,
        };
        Some(X87Finite {
            sign: self.sign(),
            exponent,
            significand,
            source_class,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{
        RationalRoundingMode, X87Binary80, X87Binary80Class, X87Finite, X87Grs,
        add_finite_same_exponent, align_finite_pair, divide_finite_significands,
        multiply_finite_significands, normalize_exact_coefficient, normalize_exact_magnitude,
        round_quotient_magnitude, round_quotient_ties_even, round_quotient_to_precision,
        shift_right_jam, shift_right_jam_u128,
    };
    use crate::state::{X87State, X87Tag};

    #[test]
    fn literal_encodings_preserve_fields_and_classification() {
        let cases: [([u8; 10], bool, u16, u64, X87Binary80Class); 27] = [
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                false,
                0x0000,
                0x0000_0000_0000_0000,
                X87Binary80Class::Zero,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80],
                true,
                0x0000,
                0x0000_0000_0000_0000,
                X87Binary80Class::Zero,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
                false,
                0x0000,
                0x0000_0000_0000_0001,
                X87Binary80Class::Subnormal,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00],
                false,
                0x0000,
                0x7fff_ffff_ffff_ffff,
                X87Binary80Class::Subnormal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00],
                false,
                0x0001,
                0x8000_0000_0000_0000,
                X87Binary80Class::Normal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
                false,
                0x3fff,
                0x8000_0000_0000_0000,
                X87Binary80Class::Normal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0xbf],
                true,
                0x3fff,
                0x8000_0000_0000_0000,
                X87Binary80Class::Normal,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f],
                false,
                0x7ffe,
                0xffff_ffff_ffff_ffff,
                X87Binary80Class::Normal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x7f],
                false,
                0x7fff,
                0x8000_0000_0000_0000,
                X87Binary80Class::Infinity,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff],
                true,
                0x7fff,
                0x8000_0000_0000_0000,
                X87Binary80Class::Infinity,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0xff, 0x7f],
                false,
                0x7fff,
                0xc000_0000_0000_0001,
                X87Binary80Class::QuietNaN,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
                false,
                0x7fff,
                0xffff_ffff_ffff_ffff,
                X87Binary80Class::QuietNaN,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x7f],
                false,
                0x7fff,
                0x8000_0000_0000_0001,
                X87Binary80Class::SignalingNaN,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xbf, 0xff, 0x7f],
                false,
                0x7fff,
                0xbfff_ffff_ffff_ffff,
                X87Binary80Class::SignalingNaN,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0xff, 0xff],
                true,
                0x7fff,
                0xc000_0000_0000_0000,
                X87Binary80Class::QuietNaN,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0xff],
                true,
                0x7fff,
                0x8000_0000_0000_0000,
                X87Binary80Class::Infinity,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x00],
                false,
                0x0000,
                0x8000_0000_0000_0000,
                X87Binary80Class::PseudoDenormal,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0x00, 0x80],
                true,
                0x0000,
                0xc000_0000_0000_0001,
                X87Binary80Class::PseudoDenormal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00],
                false,
                0x0001,
                0x0000_0000_0000_0000,
                X87Binary80Class::Unnormal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xfe, 0x7f],
                false,
                0x7ffe,
                0x0000_0000_0000_0000,
                X87Binary80Class::Unnormal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x7f],
                false,
                0x7fff,
                0x0000_0000_0000_0000,
                X87Binary80Class::PseudoInfinity,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0xff, 0x7f],
                false,
                0x7fff,
                0x4000_0000_0000_0001,
                X87Binary80Class::PseudoQuietNaN,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0xff, 0xff],
                true,
                0x7fff,
                0x7fff_ffff_ffff_ffff,
                X87Binary80Class::PseudoQuietNaN,
            ),
            (
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff, 0x7f],
                false,
                0x7fff,
                0x0000_0000_0000_0001,
                X87Binary80Class::PseudoSignalingNaN,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0xff, 0xff],
                true,
                0x7fff,
                0x3fff_ffff_ffff_ffff,
                X87Binary80Class::PseudoSignalingNaN,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x80],
                true,
                0x0001,
                0x0000_0000_0000_0000,
                X87Binary80Class::Unnormal,
            ),
            (
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xfe, 0xff],
                true,
                0x7ffe,
                0x0000_0000_0000_0000,
                X87Binary80Class::Unnormal,
            ),
        ];

        for (bytes, sign, exponent, significand, class) in cases {
            let value = X87Binary80::from_bytes(bytes);
            assert_eq!(value.to_bytes(), bytes);
            assert_eq!(value.sign(), sign, "bytes={bytes:02x?}");
            assert_eq!(value.exponent(), exponent, "bytes={bytes:02x?}");
            assert_eq!(value.significand(), significand, "bytes={bytes:02x?}");
            assert_eq!(value.integer_bit(), significand & (1 << 63) != 0);
            assert_eq!(value.fraction(), significand & !(1 << 63));
            assert_eq!(value.quiet_bit(), significand & (1 << 62) != 0);
            assert_eq!(value.classify(), class, "bytes={bytes:02x?}");
        }
    }

    #[test]
    fn walking_one_checks_all_eighty_memory_bits() {
        for bit in 0..80 {
            let mut bytes = [0; 10];
            bytes[bit / 8] = 1 << (bit % 8);
            let value = X87Binary80::from_bytes(bytes);
            assert_eq!(value.to_bytes(), bytes);
            assert_eq!(value.sign(), bit == 79);
            assert_eq!(
                value.exponent(),
                if (64..79).contains(&bit) {
                    1 << (bit - 64)
                } else {
                    0
                }
            );
            assert_eq!(value.significand(), if bit < 64 { 1 << bit } else { 0 });
        }
    }

    #[test]
    fn classification_does_not_mutate_x87_state_or_tags() {
        let state = X87State {
            registers: [
                [0x01, 0x00, 0, 0, 0, 0, 0, 0, 0, 0],
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f],
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
                [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
                [0, 0, 0, 0, 0, 0, 0, 0, 0xff, 0x7f],
                [0xde, 0xad, 0xbe, 0xef, 0xca, 0xfe, 0xba, 0x00, 0x01, 0x80],
            ],
            control_word: 0x027f,
            status_word: 0x3805,
            tag_word: 0xfff1,
            instruction_pointer: 0x0123_4567_89ab_cdef,
            data_pointer: 0xfedc_ba98_7654_3210,
            last_opcode: 0x05aa,
        };
        let before = state.clone();
        let classes = state
            .registers
            .map(|bytes| X87Binary80::from_bytes(bytes).classify());
        assert_eq!(classes[0], X87Binary80Class::Subnormal);
        assert_eq!(classes[1], X87Binary80Class::QuietNaN);
        assert_eq!(classes[2], X87Binary80Class::PseudoDenormal);
        assert_eq!(classes[3], X87Binary80Class::Zero);
        assert_eq!(classes[4], X87Binary80Class::Infinity);
        assert_eq!(classes[5], X87Binary80Class::QuietNaN);
        assert_eq!(classes[6], X87Binary80Class::PseudoInfinity);
        assert_eq!(classes[7], X87Binary80Class::Unnormal);
        assert_eq!(state, before);
        assert_eq!(state.tag(0), Some(X87Tag::Zero));
        assert_eq!(state.tag(1), Some(X87Tag::Valid));
        assert_eq!(state.tag(7), Some(X87Tag::Empty));
    }

    #[test]
    fn finite_normalization_preserves_raw_class_and_provenance() {
        let cases = [
            (
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                X87Binary80Class::Zero,
                0,
                0,
                true,
            ),
            (
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                X87Binary80Class::Subnormal,
                -16445,
                1 << 63,
                false,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0],
                X87Binary80Class::Subnormal,
                -16383,
                u64::MAX << 1,
                false,
            ),
            (
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0],
                X87Binary80Class::PseudoDenormal,
                -16382,
                1 << 63,
                false,
            ),
            (
                [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0],
                X87Binary80Class::Normal,
                -16382,
                1 << 63,
                false,
            ),
            (
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f],
                X87Binary80Class::Normal,
                16383,
                u64::MAX,
                false,
            ),
        ];

        for (bytes, expected_class, expected_exponent, expected_significand, expected_sign) in cases
        {
            let raw = X87Binary80::from_bytes(bytes);
            let before = raw.to_bytes();
            let finite = raw.normalize_finite().expect("finite encoding");
            assert_eq!(finite.source_class(), expected_class);
            assert_eq!(finite.exponent(), expected_exponent);
            assert_eq!(finite.significand(), expected_significand);
            assert_eq!(finite.sign(), expected_sign);
            assert_eq!(finite.is_zero(), expected_class == X87Binary80Class::Zero);
            assert_eq!(raw.classify(), expected_class);
            assert_eq!(raw.to_bytes(), before);
        }

        let signed_subnormal = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]);
        let negative_pseudo_denormal =
            X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80]);
        assert!(signed_subnormal.normalize_finite().unwrap().sign());
        assert!(negative_pseudo_denormal.normalize_finite().unwrap().sign());

        for bytes in [
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x7f],
            [1, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x7f],
            [0, 0, 0, 0, 0, 0, 0, 0, 1, 0],
        ] {
            assert!(X87Binary80::from_bytes(bytes).normalize_finite().is_none());
        }
    }

    fn finite(
        exponent: i32,
        significand: u64,
        sign: bool,
        source_class: X87Binary80Class,
    ) -> X87Finite {
        X87Finite {
            sign,
            exponent,
            significand,
            source_class,
        }
    }

    #[test]
    fn exact_finite_quotient_matches_literal_normalized_vectors() {
        let cases = [
            (
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0,
                0,
            ),
            (
                0xc000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0xc000_0000_0000_0000,
                0,
                0,
            ),
            (
                0x8000_0000_0000_0000,
                0xc000_0000_0000_0000,
                0xaaaa_aaaa_aaaa_aaaa,
                0x8000_0000_0000_0000,
                -1,
            ),
            (
                0x8000_0000_0000_0000,
                0xffff_ffff_ffff_ffff,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                -1,
            ),
            (
                0xffff_ffff_ffff_ffff,
                0x8000_0000_0000_0000,
                0xffff_ffff_ffff_ffff,
                0,
                0,
            ),
            (
                0xffff_ffff_ffff_ffff,
                0xc000_0000_0000_0000,
                0xaaaa_aaaa_aaaa_aaaa,
                0,
                0,
            ),
            (
                0xffff_ffff_ffff_ffff,
                0x8000_0000_0000_0001,
                0xffff_ffff_ffff_fffd,
                3,
                0,
            ),
            (
                0x8000_0000_0000_0001,
                0x8000_0000_0000_0002,
                0xffff_ffff_ffff_fffe,
                4,
                -1,
            ),
        ];

        for (lhs_sig, rhs_sig, quotient, remainder, exponent) in cases {
            let lhs = finite(0, lhs_sig, false, X87Binary80Class::Normal);
            let rhs = finite(0, rhs_sig, false, X87Binary80Class::Normal);
            let result = divide_finite_significands(lhs, rhs).unwrap();
            assert_eq!(result.sign(), false);
            assert_eq!(result.exponent(), exponent);
            assert_eq!(result.quotient(), quotient);
            assert_eq!(result.remainder(), remainder);
            assert_eq!(result.denominator(), rhs_sig);
            assert!(result.quotient() >= (1_u64 << 63));
            assert!(result.remainder() < result.denominator());
            assert_eq!(
                (lhs_sig as u128) << if lhs_sig >= rhs_sig { 63 } else { 64 },
                result.quotient() as u128 * rhs_sig as u128 + result.remainder() as u128
            );
        }
    }

    #[test]
    fn unsigned_quotient_rounding_matches_literal_ties_even_vectors() {
        let cases = [
            (2, 4, 10, Some(2)),                  // below half
            (2, 5, 10, Some(2)),                  // tie, even quotient
            (3, 5, 10, Some(4)),                  // tie, odd quotient
            (3, 6, 10, Some(4)),                  // above half
            (u64::MAX, 1, 2, Some(1_u128 << 64)), // carry past u64::MAX
            (
                0,
                1_u64 << 63,
                u64::MAX,
                Some(1), // doubling r requires more than u64 width
            ),
            (7, 0, 1, Some(7)), // exact integer
            (7, 0, 0, None),
            (7, 1, 1, None),
        ];

        for (q, r, d, expected) in cases {
            assert_eq!(round_quotient_ties_even(q, r, d), expected);
        }
    }

    #[test]
    fn rational_rounding_modes_match_literal_signed_magnitude_vectors() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let cases = [
            // Positive 2 + 1/4: only toward positive increments.
            (false, 2, 1, 4, Nearest, 2),
            (false, 2, 1, 4, Zero, 2),
            (false, 2, 1, 4, Up, 3),
            (false, 2, 1, 4, Down, 2),
            // Negative 2 + 1/4: only toward negative increments magnitude.
            (true, 2, 1, 4, Nearest, 2),
            (true, 2, 1, 4, Zero, 2),
            (true, 2, 1, 4, Up, 2),
            (true, 2, 1, 4, Down, 3),
            // Positive and negative 2 + 3/4.
            (false, 2, 3, 4, Nearest, 3),
            (false, 2, 3, 4, Zero, 2),
            (false, 2, 3, 4, Up, 3),
            (false, 2, 3, 4, Down, 2),
            (true, 2, 3, 4, Nearest, 3),
            (true, 2, 3, 4, Zero, 2),
            (true, 2, 3, 4, Up, 2),
            (true, 2, 3, 4, Down, 3),
            // Ties exercise even and odd quotient parity and the u64 carry.
            (false, 2, 1, 2, Nearest, 2),
            (false, 2, 1, 2, Zero, 2),
            (false, 2, 1, 2, Up, 3),
            (false, 2, 1, 2, Down, 2),
            (true, 2, 1, 2, Nearest, 2),
            (true, 2, 1, 2, Zero, 2),
            (true, 2, 1, 2, Up, 2),
            (true, 2, 1, 2, Down, 3),
            (false, 3, 1, 2, Nearest, 4),
            (false, 3, 1, 2, Zero, 3),
            (false, 3, 1, 2, Up, 4),
            (false, 3, 1, 2, Down, 3),
            (true, 3, 1, 2, Nearest, 4),
            (true, 3, 1, 2, Zero, 3),
            (true, 3, 1, 2, Up, 3),
            (true, 3, 1, 2, Down, 4),
            (false, u64::MAX, 1, 2, Nearest, 1_u128 << 64),
            (true, u64::MAX, 1, 2, Down, 1_u128 << 64),
            // Small fractions around zero and exact values in every mode.
            (false, 0, 1, 4, Nearest, 0),
            (false, 0, 1, 4, Up, 1),
            (true, 0, 1, 4, Down, 1),
            (true, 0, 1, 4, Up, 0),
            (true, 7, 0, 1, Nearest, 7),
            (true, 7, 0, 1, Zero, 7),
            (true, 7, 0, 1, Up, 7),
            (true, 7, 0, 1, Down, 7),
            // Widening makes 2*r > d here; a u64 doubling would wrap.
            (false, 2, 1_u64 << 63, u64::MAX, Nearest, 3),
        ];

        for (sign, q, r, d, mode, expected) in cases {
            assert_eq!(
                round_quotient_magnitude(sign, q, r, d, mode),
                Some(expected),
                "sign={sign} q={q} r={r} d={d} mode={mode:?}"
            );
        }
    }

    #[test]
    fn quotient_precision_rounding_matches_literal_vectors() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let cases = [
            // Synthetic p=64 tie and carry cases: divider results cannot make
            // these exact half tails, but the generic helper must preserve them.
            (0x8000_0000_0000_0002, 1, 2, 64, 0x8000_0000_0000_0002),
            (0x8000_0000_0000_0003, 1, 2, 64, 0x8000_0000_0000_0004),
            (u64::MAX, 1, 2, 64, 1_u128 << 64),
            // At p=1, a remainder just below one cannot bridge lo=H-1.
            (0xbfff_ffff_ffff_ffff, u64::MAX - 1, u64::MAX, 1, 1),
            (0xc000_0000_0000_0000, 0, 1, 1, 2), // exact halfway, odd t
            (0xa000_0000_0000_0000, 0, 1, 2, 2), // exact halfway, even t
            (0xe000_0000_0000_0000, 0, 1, 2, 4), // exact halfway, odd t
            // Direct exact rounding avoids the double-rounding result 0x...000.
            (0x8000_0000_0000_0001, 1, 4, 63, 0x4000_0000_0000_0001),
            // d=MAX comparisons on either side of one half (p=64 synthetic).
            (
                0x8000_0000_0000_0000,
                0x7fff_ffff_ffff_ffff,
                u64::MAX,
                64,
                0x8000_0000_0000_0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                u64::MAX,
                64,
                0x8000_0000_0000_0001,
            ),
        ];

        for (q, r, d, precision, expected) in cases {
            assert_eq!(
                round_quotient_to_precision(false, q, r, d, precision, Nearest),
                Some(u128::from(expected)),
                "q={q:#018x} r={r} d={d} p={precision}"
            );
        }
    }

    #[test]
    fn quotient_precision_rounding_applies_all_modes_and_signs() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        for sign in [false, true] {
            for (mode, expected) in [
                (Nearest, 0x4000_0000_0000_0000_u128),
                (Zero, 0x4000_0000_0000_0000),
                (
                    Up,
                    if sign {
                        0x4000_0000_0000_0000
                    } else {
                        0x4000_0000_0000_0001
                    },
                ),
                (
                    Down,
                    if sign {
                        0x4000_0000_0000_0001
                    } else {
                        0x4000_0000_0000_0000
                    },
                ),
            ] {
                assert_eq!(
                    round_quotient_to_precision(sign, 0x8000_0000_0000_0000, 1, 2, 63, mode,),
                    Some(expected),
                    "sign={sign} mode={mode:?}"
                );
            }

            // A discarded bit with no rational remainder is still inexact,
            // while an exact halfway value follows ties-to-even.
            assert_eq!(
                round_quotient_to_precision(sign, 0xa000_0000_0000_0000, 0, 1, 2, Nearest,),
                Some(2)
            );
            assert_eq!(
                round_quotient_to_precision(sign, 0xa000_0000_0000_0000, 0, 1, 2, Up,),
                Some(if sign { 2 } else { 3 })
            );
            assert_eq!(
                round_quotient_to_precision(sign, 0xa000_0000_0000_0000, 0, 1, 2, Down,),
                Some(if sign { 3 } else { 2 })
            );
        }

        // Every mode and sign accepts the exact zero decomposition.
        for mode in modes {
            for sign in [false, true] {
                assert_eq!(
                    round_quotient_to_precision(sign, 0, 0, 1, 64, mode),
                    Some(0)
                );
            }
        }
    }

    #[test]
    fn quotient_precision_rounding_composes_with_division_without_double_rounding() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let numerator = finite(-1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let denominator = finite(0, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
        let quotient = divide_finite_significands(numerator, denominator).unwrap();
        assert_eq!(quotient.sign(), false);
        assert_eq!(quotient.exponent(), -2);
        assert_eq!(quotient.quotient(), 0xaaaa_aaaa_aaaa_aaaa);
        assert_eq!(quotient.remainder(), 0x8000_0000_0000_0000);
        assert_eq!(quotient.denominator(), 0xc000_0000_0000_0000);
        for (precision, nearest) in [
            (24, 0x00aa_aaab_u128),
            (53, 0x0015_5555_5555_5555),
            (64, 0xaaaa_aaaa_aaaa_aaab),
        ] {
            assert_eq!(
                round_quotient_to_precision(
                    quotient.sign(),
                    quotient.quotient(),
                    quotient.remainder(),
                    quotient.denominator(),
                    precision,
                    Nearest,
                ),
                Some(nearest)
            );
        }

        let negative_numerator = finite(-1, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal);
        let negative_quotient =
            divide_finite_significands(negative_numerator, denominator).unwrap();
        assert!(negative_quotient.sign());
        let p24 = [
            (Nearest, 0x00aa_aaab_u128),
            (Zero, 0x00aa_aaaa),
            (Up, 0x00aa_aaaa),
            (Down, 0x00aa_aaab),
        ];
        for (mode, expected) in p24 {
            assert_eq!(
                round_quotient_to_precision(
                    negative_quotient.sign(),
                    negative_quotient.quotient(),
                    negative_quotient.remainder(),
                    negative_quotient.denominator(),
                    24,
                    mode,
                ),
                Some(expected),
                "mode={mode:?}"
            );
        }
    }

    #[test]
    fn quotient_precision_rounding_rejects_invalid_inputs() {
        let modes = [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ];
        for mode in modes {
            for precision in [0, 65, u32::MAX] {
                assert_eq!(
                    round_quotient_to_precision(false, 0, 0, 1, precision, mode),
                    None
                );
            }
            assert_eq!(round_quotient_to_precision(false, 7, 0, 0, 1, mode), None);
            assert_eq!(round_quotient_to_precision(false, 7, 1, 1, 1, mode), None);
            assert_eq!(round_quotient_to_precision(false, 1, 0, 1, 1, mode), None);
            assert_eq!(round_quotient_to_precision(false, 0, 1, 2, 1, mode), None);
        }
    }

    #[test]
    fn rational_rounding_rejects_invalid_denominator_inputs_in_every_mode() {
        let modes = [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ];
        for mode in modes {
            assert_eq!(round_quotient_magnitude(false, 7, 0, 0, mode), None);
            assert_eq!(round_quotient_magnitude(true, 7, 1, 1, mode), None);
        }
    }

    #[test]
    fn unsigned_quotient_rounding_composes_with_s2_29i_division_vectors() {
        let cases = [
            (
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
            ),
            (
                0x8000_0000_0000_0000,
                0xc000_0000_0000_0000,
                0xaaaa_aaaa_aaaa_aaab,
            ),
            (0xffff_ffff_ffff_ffff, 0x8000_0000_0000_0000, u64::MAX),
            (
                0x8000_0000_0000_0001,
                0x8000_0000_0000_0002,
                0xffff_ffff_ffff_fffe,
            ),
        ];

        for (lhs_significand, rhs_significand, expected_rounded) in cases {
            let lhs = finite(0, lhs_significand, false, X87Binary80Class::Normal);
            let rhs = finite(0, rhs_significand, false, X87Binary80Class::Normal);
            let quotient = divide_finite_significands(lhs, rhs).unwrap();
            assert_eq!(
                round_quotient_ties_even(
                    quotient.quotient(),
                    quotient.remainder(),
                    quotient.denominator()
                ),
                Some(u128::from(expected_rounded))
            );
        }
    }

    #[test]
    fn signed_rational_rounding_composes_with_s2_29i_division_getters() {
        let modes = [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ];
        for sign in [false, true] {
            let lhs = finite(0, 0x8000_0000_0000_0000, sign, X87Binary80Class::Normal);
            let rhs = finite(0, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
            let quotient = divide_finite_significands(lhs, rhs).unwrap();
            assert_eq!(quotient.sign(), sign);
            assert_eq!(quotient.quotient(), 0xaaaa_aaaa_aaaa_aaaa);
            assert_eq!(quotient.remainder(), 0x8000_0000_0000_0000);
            assert_eq!(quotient.denominator(), 0xc000_0000_0000_0000);
            for mode in modes {
                let expected = match (mode, sign) {
                    (RationalRoundingMode::TowardZero, _)
                    | (RationalRoundingMode::TowardPositive, true)
                    | (RationalRoundingMode::TowardNegative, false) => 0xaaaa_aaaa_aaaa_aaaa_u128,
                    _ => 0xaaaa_aaaa_aaaa_aaab_u128,
                };
                assert_eq!(
                    round_quotient_magnitude(
                        quotient.sign(),
                        quotient.quotient(),
                        quotient.remainder(),
                        quotient.denominator(),
                        mode,
                    ),
                    Some(expected)
                );
            }
        }
    }

    #[test]
    fn exact_finite_quotient_handles_sign_zero_denominator_and_exponent_boundaries() {
        let lhs = finite(7, 0xc000_0000_0000_0000, true, X87Binary80Class::Normal);
        let rhs = finite(2, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let result = divide_finite_significands(lhs, rhs).unwrap();
        assert!(result.sign());
        assert_eq!(result.exponent(), 5);
        assert_eq!(result.quotient(), 0xc000_0000_0000_0000);
        assert_eq!(result.remainder(), 0);
        assert_eq!(result.denominator(), rhs.significand());
        let negative_rhs = finite(2, rhs.significand(), true, X87Binary80Class::Normal);
        assert!(
            !divide_finite_significands(lhs, negative_rhs)
                .unwrap()
                .sign()
        );

        for sign in [false, true] {
            let zero = finite(123, 0, sign, X87Binary80Class::Zero);
            let result = divide_finite_significands(zero, rhs).unwrap();
            assert_eq!(
                (
                    result.sign(),
                    result.exponent(),
                    result.quotient(),
                    result.remainder()
                ),
                (false, 0, 0, 0)
            );
            assert_eq!(result.denominator(), rhs.significand());
        }
        let zero = finite(0, 0, false, X87Binary80Class::Zero);
        assert_eq!(divide_finite_significands(lhs, zero), None);
        assert_eq!(divide_finite_significands(zero, zero), None);

        let min = finite(
            i32::MIN,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let max = finite(
            i32::MAX,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        assert_eq!(divide_finite_significands(min, max), None);
        assert_eq!(divide_finite_significands(max, min), None);
        assert_eq!(
            divide_finite_significands(
                finite(
                    i32::MAX,
                    0x8000_0000_0000_0000,
                    false,
                    X87Binary80Class::Normal
                ),
                finite(0, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal)
            )
            .unwrap()
            .exponent(),
            i32::MAX
        );
        assert_eq!(
            divide_finite_significands(
                finite(
                    i32::MIN + 1,
                    0x8000_0000_0000_0000,
                    false,
                    X87Binary80Class::Normal
                ),
                finite(0, 0xffff_ffff_ffff_ffff, false, X87Binary80Class::Normal)
            )
            .unwrap()
            .exponent(),
            i32::MIN
        );
    }

    #[test]
    fn exact_finite_quotient_composes_with_raw_finite_values_without_mutation() {
        let raw_subnormal = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let raw_pseudo = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80]);
        let numerator = raw_subnormal.normalize_finite().unwrap();
        let denominator = raw_pseudo.normalize_finite().unwrap();
        let before = (raw_subnormal, raw_pseudo, numerator, denominator);
        assert_eq!(numerator.source_class(), X87Binary80Class::Subnormal);
        assert_eq!(denominator.source_class(), X87Binary80Class::PseudoDenormal);
        let result = divide_finite_significands(numerator, denominator).unwrap();
        assert_eq!(result.sign(), true);
        assert_eq!(result.exponent(), -63);
        assert_eq!(result.quotient(), 1_u64 << 63);
        assert_eq!(result.remainder(), 0);
        assert_eq!(result.denominator(), denominator.significand());
        assert_eq!((raw_subnormal, raw_pseudo, numerator, denominator), before);
        assert_eq!(raw_subnormal.to_bytes(), [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(raw_pseudo.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80]);
    }

    #[test]
    fn finite_pair_alignment_matches_literal_shift_gap_vectors_in_input_order() {
        let lhs = finite(
            -16445,
            0x8000_0000_0000_0001,
            true,
            X87Binary80Class::Normal,
        );
        let gaps = [
            (0, 0x0000_0000_0000_0004_0000_0000_0000_0008),
            (1, 0x0000_0000_0000_0002_0000_0000_0000_0004),
            (2, 0x0000_0000_0000_0001_0000_0000_0000_0002),
            (3, 0x0000_0000_0000_0000_8000_0000_0000_0001),
            (4, 0x0000_0000_0000_0000_4000_0000_0000_0001),
            (63, 0x0000_0000_0000_0000_0000_0000_0000_0009),
            (64, 0x0000_0000_0000_0000_0000_0000_0000_0005),
            (65, 0x0000_0000_0000_0000_0000_0000_0000_0003),
            (66, 0x0000_0000_0000_0000_0000_0000_0000_0001),
            (67, 0x0000_0000_0000_0000_0000_0000_0000_0001),
            (127, 0x0000_0000_0000_0000_0000_0000_0000_0001),
            (128, 0x0000_0000_0000_0000_0000_0000_0000_0001),
            (129, 0x0000_0000_0000_0000_0000_0000_0000_0001),
        ];

        for (gap, expected_lhs) in gaps {
            let rhs = finite(
                lhs.exponent() + gap,
                0x8000_0000_0000_0001,
                false,
                X87Binary80Class::Subnormal,
            );
            assert_eq!(
                align_finite_pair(lhs, rhs),
                (
                    rhs.exponent(),
                    expected_lhs,
                    0x0000_0000_0000_0004_0000_0000_0000_0008,
                ),
                "gap={gap}"
            );
            assert_eq!(
                align_finite_pair(rhs, lhs),
                (
                    rhs.exponent(),
                    0x0000_0000_0000_0004_0000_0000_0000_0008,
                    expected_lhs,
                ),
                "reversed gap={gap}"
            );
            assert!(lhs.sign());
            assert!(!rhs.sign());
            assert_eq!(lhs.source_class(), X87Binary80Class::Normal);
            assert_eq!(rhs.source_class(), X87Binary80Class::Subnormal);
            assert_eq!(lhs.significand(), 0x8000_0000_0000_0001);
            assert_eq!(rhs.significand(), 0x8000_0000_0000_0001);
        }
    }

    #[test]
    fn finite_pair_alignment_covers_finite_boundaries_and_provenance() {
        let min_subnormal = finite(
            -16445,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Subnormal,
        );
        let min_normal = finite(
            -16382,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Normal,
        );
        assert_eq!(
            align_finite_pair(min_subnormal, min_normal),
            (
                -16382,
                0x0000_0000_0000_0000_0000_0000_0000_0008,
                0x0000_0000_0000_0004_0000_0000_0000_0000,
            )
        );

        let max_finite = finite(16383, u64::MAX, true, X87Binary80Class::Normal);
        assert_eq!(
            align_finite_pair(min_subnormal, max_finite),
            (16383, 1, 0x0000_0000_0000_0007_ffff_ffff_ffff_fff8,)
        );

        let max_subnormal = finite(
            -16383,
            0xffff_ffff_ffff_fffe,
            false,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            align_finite_pair(max_subnormal, min_normal),
            (
                -16382,
                0x0000_0000_0000_0000_0003_ffff_ffff_ffff_fff8,
                0x0000_0000_0000_0004_0000_0000_0000_0000,
            )
        );

        let pseudo_denormal = finite(
            -16382,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::PseudoDenormal,
        );
        assert_eq!(
            align_finite_pair(pseudo_denormal, min_normal),
            (
                -16382,
                0x0000_0000_0000_0004_0000_0000_0000_0000,
                0x0000_0000_0000_0004_0000_0000_0000_0000,
            )
        );
        assert_eq!(
            pseudo_denormal.source_class(),
            X87Binary80Class::PseudoDenormal
        );
        assert!(!pseudo_denormal.sign());
        assert_eq!(pseudo_denormal.significand(), 0x8000_0000_0000_0000);
        assert_eq!(min_normal.source_class(), X87Binary80Class::Normal);
        assert!(min_normal.sign());
        assert_eq!(min_normal.significand(), 0x8000_0000_0000_0000);
    }

    #[test]
    fn finite_pair_alignment_handles_signed_zeros_and_negative_exponents() {
        let positive_zero = finite(0, 0, false, X87Binary80Class::Zero);
        let negative_zero = finite(0, 0, true, X87Binary80Class::Zero);
        let negative_exponent = finite(-7, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal);
        assert_eq!(align_finite_pair(positive_zero, negative_zero), (0, 0, 0));
        assert_eq!(align_finite_pair(negative_zero, positive_zero), (0, 0, 0));
        assert_eq!(
            align_finite_pair(positive_zero, negative_exponent),
            (-7, 0, 0x0000_0000_0000_0004_0000_0000_0000_0000)
        );
        assert_eq!(
            align_finite_pair(negative_exponent, negative_zero),
            (-7, 0x0000_0000_0000_0004_0000_0000_0000_0000, 0)
        );

        let negative_nonzero = finite(-3, 0x8000_0000_0000_0000, true, X87Binary80Class::Subnormal);
        assert_eq!(
            align_finite_pair(negative_nonzero, negative_exponent),
            (
                -3,
                0x0000_0000_0000_0004_0000_0000_0000_0000,
                0x4000_0000_0000_0000
            )
        );
        assert_eq!(
            align_finite_pair(negative_exponent, negative_nonzero),
            (
                -3,
                0x4000_0000_0000_0000,
                0x0000_0000_0000_0004_0000_0000_0000_0000
            )
        );

        for (lhs, rhs) in [
            (positive_zero, positive_zero),
            (positive_zero, negative_zero),
            (negative_zero, positive_zero),
            (negative_zero, negative_zero),
        ] {
            assert_eq!(align_finite_pair(lhs, rhs), (0, 0, 0));
        }
    }

    #[test]
    fn exact_same_exponent_sum_preserves_carry_cancellation_and_inputs() {
        let exponent = -7;
        let positive_max = finite(exponent, u64::MAX, false, X87Binary80Class::Normal);
        let negative_max = finite(exponent, u64::MAX, true, X87Binary80Class::Subnormal);
        assert_eq!(
            add_finite_same_exponent(positive_max, positive_max),
            Some((exponent, 0x1_ffff_ffff_ffff_fffe))
        );
        assert_eq!(
            add_finite_same_exponent(negative_max, negative_max),
            Some((exponent, -0x1_ffff_ffff_ffff_fffe))
        );

        let positive_half = finite(
            exponent,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let negative_half = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::PseudoDenormal,
        );
        assert_eq!(
            add_finite_same_exponent(positive_half, positive_half),
            Some((exponent, 0x1_0000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_same_exponent(negative_half, negative_half),
            Some((exponent, -0x1_0000_0000_0000_0000))
        );

        let all_ones = finite(exponent, u64::MAX, false, X87Binary80Class::Normal);
        let leading_bit = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            add_finite_same_exponent(all_ones, leading_bit),
            Some((exponent, 0x7fff_ffff_ffff_ffff))
        );
        assert_eq!(
            add_finite_same_exponent(leading_bit, all_ones),
            Some((exponent, 0x7fff_ffff_ffff_ffff))
        );
        let negative_all_ones = finite(exponent, u64::MAX, true, X87Binary80Class::Normal);
        let positive_leading_bit = finite(
            exponent,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            add_finite_same_exponent(negative_all_ones, positive_leading_bit),
            Some((exponent, -0x7fff_ffff_ffff_ffff))
        );
        assert_eq!(
            add_finite_same_exponent(positive_leading_bit, negative_all_ones),
            Some((exponent, -0x7fff_ffff_ffff_ffff))
        );
        assert_eq!(
            add_finite_same_exponent(positive_half, negative_half),
            Some((exponent, 0))
        );

        let adjacent_positive = finite(
            exponent,
            0x8000_0000_0000_0001,
            false,
            X87Binary80Class::Normal,
        );
        let adjacent_negative = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            add_finite_same_exponent(adjacent_positive, adjacent_negative),
            Some((exponent, 1))
        );
        assert_eq!(
            add_finite_same_exponent(adjacent_negative, adjacent_positive),
            Some((exponent, 1))
        );
        let adjacent_negative_large = finite(
            exponent,
            0x8000_0000_0000_0001,
            true,
            X87Binary80Class::Normal,
        );
        let adjacent_positive_small = finite(
            exponent,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            add_finite_same_exponent(adjacent_negative_large, adjacent_positive_small),
            Some((exponent, -1))
        );
        assert_eq!(
            add_finite_same_exponent(adjacent_positive_small, adjacent_negative_large),
            Some((exponent, -1))
        );

        let before = (all_ones, leading_bit, positive_half, negative_half);
        let _ = add_finite_same_exponent(all_ones, leading_bit);
        let _ = add_finite_same_exponent(positive_half, negative_half);
        assert_eq!(
            (all_ones, leading_bit, positive_half, negative_half),
            before
        );
        assert_eq!(all_ones.source_class(), X87Binary80Class::Normal);
        assert_eq!(leading_bit.source_class(), X87Binary80Class::Subnormal);
        assert_eq!(positive_half.source_class(), X87Binary80Class::Normal);
        assert_eq!(
            negative_half.source_class(),
            X87Binary80Class::PseudoDenormal
        );
    }

    #[test]
    fn exact_same_exponent_sum_handles_zero_provenance_and_rejects_unequal_exponents() {
        let positive_zero = finite(0, 0, false, X87Binary80Class::Zero);
        let negative_zero = finite(0, 0, true, X87Binary80Class::Zero);
        let value = finite(
            -11,
            0x8123_4567_89ab_cdef,
            true,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            add_finite_same_exponent(positive_zero, negative_zero),
            Some((0, 0))
        );
        assert_eq!(
            add_finite_same_exponent(negative_zero, positive_zero),
            Some((0, 0))
        );
        assert_eq!(
            add_finite_same_exponent(positive_zero, value),
            Some((-11, -0x8123_4567_89ab_cdef))
        );
        assert_eq!(
            add_finite_same_exponent(value, negative_zero),
            Some((-11, -0x8123_4567_89ab_cdef))
        );
        assert_eq!(
            add_finite_same_exponent(
                value,
                finite(-10, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal)
            ),
            None
        );

        let pseudo_raw = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        let pseudo = pseudo_raw.normalize_finite().unwrap();
        let normal = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        let normal_finite = normal.normalize_finite().unwrap();
        assert_eq!(pseudo.exponent(), -16382);
        assert_eq!(normal_finite.exponent(), -16382);
        assert_eq!(pseudo.significand(), normal_finite.significand());
        assert_eq!(pseudo.source_class(), X87Binary80Class::PseudoDenormal);
        assert_eq!(normal_finite.source_class(), X87Binary80Class::Normal);
        let raw_before = (pseudo_raw, normal);
        let finite_before = (pseudo, normal_finite);
        assert_eq!(
            add_finite_same_exponent(pseudo, normal_finite),
            Some((-16382, 0x1_0000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_same_exponent(normal_finite, pseudo),
            Some((-16382, 0x1_0000_0000_0000_0000))
        );
        assert_eq!((pseudo, normal_finite), finite_before);
        assert_eq!(pseudo_raw.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        assert_eq!(normal, raw_before.1);
        assert_eq!(normal.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        assert_eq!(
            X87Binary80::from_bytes(raw_before.1.to_bytes()).classify(),
            X87Binary80Class::Normal
        );
    }

    #[test]
    fn exact_coefficient_normalization_preserves_all_bits_and_sign() {
        let cases = [
            (
                63,
                1_i128,
                (false, 0, 0x8000_0000_0000_0000_0000_0000_0000_0000),
            ),
            (
                63,
                -1_i128,
                (true, 0, 0x8000_0000_0000_0000_0000_0000_0000_0000),
            ),
            (
                63,
                1_i128 << 63,
                (false, 63, 0x8000_0000_0000_0000_0000_0000_0000_0000),
            ),
            (
                63,
                1_i128 << 64,
                (false, 64, 0x8000_0000_0000_0000_0000_0000_0000_0000),
            ),
            (
                63,
                (1_i128 << 64) + 1,
                (false, 64, 0x8000_0000_0000_0000_8000_0000_0000_0000),
            ),
            (
                63,
                (1_i128 << 65) - 2,
                (false, 64, 0xffff_ffff_ffff_ffff_0000_0000_0000_0000),
            ),
            (
                0,
                i128::MAX,
                (false, 63, 0xffff_ffff_ffff_ffff_ffff_ffff_ffff_fffe),
            ),
            (
                0,
                i128::MIN,
                (true, 64, 0x8000_0000_0000_0000_0000_0000_0000_0000),
            ),
        ];

        for (exponent, coefficient, expected) in cases {
            assert_eq!(
                normalize_exact_coefficient(exponent, coefficient),
                Some(expected)
            );
        }
    }

    #[test]
    fn exact_coefficient_normalization_checks_only_final_exponent_overflow() {
        assert_eq!(
            normalize_exact_coefficient(i32::MAX, 1),
            Some((
                false,
                i32::MAX - 63,
                0x8000_0000_0000_0000_0000_0000_0000_0000
            ))
        );
        assert_eq!(
            normalize_exact_coefficient(i32::MIN, 1_i128 << 63),
            Some((false, i32::MIN, 0x8000_0000_0000_0000_0000_0000_0000_0000))
        );
        assert_eq!(
            normalize_exact_coefficient(i32::MIN, 1_i128 << 64),
            Some((
                false,
                i32::MIN + 1,
                0x8000_0000_0000_0000_0000_0000_0000_0000
            ))
        );
        assert_eq!(normalize_exact_coefficient(i32::MAX, 1_i128 << 64), None);
        assert_eq!(normalize_exact_coefficient(i32::MIN, 1), None);
        assert_eq!(normalize_exact_coefficient(i32::MAX, i128::MIN), None);
        assert_eq!(
            normalize_exact_coefficient(i32::MIN, i128::MIN),
            Some((true, i32::MIN + 64, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_coefficient(i32::MAX, 0),
            Some((false, 0, 0))
        );
        assert_eq!(
            normalize_exact_coefficient(i32::MIN, 0),
            Some((false, 0, 0))
        );
    }

    #[test]
    fn exact_finite_significand_product_preserves_full_u128_magnitude_and_sign() {
        let exponent = 17;
        let minimum = finite(
            exponent,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let minimum_negative = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Subnormal,
        );
        let maximum = finite(exponent, u64::MAX, false, X87Binary80Class::Normal);
        let mixed = finite(
            exponent,
            0x8000_0000_0000_0001,
            true,
            X87Binary80Class::PseudoDenormal,
        );

        assert_eq!(
            multiply_finite_significands(minimum, minimum),
            Some((false, 35, 0x4000_0000_0000_0000_0000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(maximum, maximum),
            Some((false, 35, 0xffff_ffff_ffff_fffe_0000_0000_0000_0001))
        );
        assert_eq!(
            multiply_finite_significands(minimum, maximum),
            Some((false, 35, 0x7fff_ffff_ffff_ffff_8000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(minimum, mixed),
            Some((true, 35, 0x4000_0000_0000_0000_8000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(mixed, minimum),
            Some((true, 35, 0x4000_0000_0000_0000_8000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(minimum_negative, mixed),
            Some((false, 35, 0x4000_0000_0000_0000_8000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(mixed, minimum_negative),
            Some((false, 35, 0x4000_0000_0000_0000_8000_0000_0000_0000))
        );

        assert_eq!(maximum.source_class(), X87Binary80Class::Normal);
        assert_eq!(minimum_negative.source_class(), X87Binary80Class::Subnormal);
        assert_eq!(mixed.source_class(), X87Binary80Class::PseudoDenormal);
    }

    #[test]
    fn exact_finite_significand_product_handles_zero_and_exponent_boundaries() {
        let positive_zero = finite(0, 0, false, X87Binary80Class::Zero);
        let negative_zero = finite(0, 0, true, X87Binary80Class::Zero);
        let minimum = finite(
            i32::MIN,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Normal,
        );
        let maximum = finite(i32::MAX, u64::MAX, false, X87Binary80Class::Normal);
        let negative_one_exponent =
            finite(-1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);

        assert_eq!(
            multiply_finite_significands(positive_zero, maximum),
            Some((false, 0, 0))
        );
        assert_eq!(
            multiply_finite_significands(minimum, negative_zero),
            Some((false, 0, 0))
        );
        assert_eq!(
            multiply_finite_significands(negative_zero, positive_zero),
            Some((false, 0, 0))
        );
        assert_eq!(
            multiply_finite_significands(minimum, minimum),
            None,
            "i32::MIN + i32::MIN + 1 underflows"
        );
        assert_eq!(
            multiply_finite_significands(maximum, maximum),
            None,
            "i32::MAX + i32::MAX + 1 overflows"
        );
        assert_eq!(
            multiply_finite_significands(minimum, maximum),
            Some((true, 0, 0x7fff_ffff_ffff_ffff_8000_0000_0000_0000))
        );
        assert_eq!(
            multiply_finite_significands(minimum, negative_one_exponent),
            Some((true, i32::MIN, 0x4000_0000_0000_0000_0000_0000_0000_0000))
        );

        let raw_pseudo = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        let raw_normal = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        let pseudo = raw_pseudo.normalize_finite().unwrap();
        let normal = raw_normal.normalize_finite().unwrap();
        let finite_before = (pseudo, normal);
        let raw_before = (raw_pseudo, raw_normal);
        assert_eq!(pseudo.exponent(), normal.exponent());
        assert_eq!(pseudo.source_class(), X87Binary80Class::PseudoDenormal);
        assert_eq!(normal.source_class(), X87Binary80Class::Normal);
        assert_eq!(
            multiply_finite_significands(pseudo, normal),
            Some((false, -32763, 0x4000_0000_0000_0000_8000_0000_0000_0000))
        );
        assert_eq!((pseudo, normal), finite_before);
        assert_eq!((raw_pseudo, raw_normal), raw_before);
        assert_eq!(raw_pseudo.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        assert_eq!(raw_normal.to_bytes(), [1, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
    }

    #[test]
    fn exact_unsigned_magnitude_normalization_preserves_bits_and_sign() {
        assert_eq!(
            normalize_exact_magnitude(false, 127, 1),
            Some((false, 0, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_magnitude(true, 1, 1_u128 << 126),
            Some((true, 0, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_magnitude(false, 0, 1_u128 << 127),
            Some((false, 0, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_magnitude(true, 0, u128::MAX),
            Some((true, 0, u128::MAX))
        );
        assert_eq!(
            normalize_exact_magnitude(false, 63, (1_u128 << 64) + 1),
            Some((false, 0, (1_u128 << 127) + (1_u128 << 63)))
        );

        for (sign, exponent, magnitude) in [
            (false, 127, 1),
            (true, 1, 1_u128 << 126),
            (false, 0, 1_u128 << 127),
            (true, 0, u128::MAX),
            (false, 63, (1_u128 << 64) + 1),
        ] {
            let normalized = normalize_exact_magnitude(sign, exponent, magnitude).unwrap();
            assert_eq!(
                normalize_exact_magnitude(normalized.0, normalized.1, normalized.2),
                Some(normalized)
            );
        }
    }

    #[test]
    fn exact_unsigned_magnitude_normalization_handles_zero_and_exponent_boundaries() {
        assert_eq!(
            normalize_exact_magnitude(false, i32::MIN, 0),
            Some((false, 0, 0))
        );
        assert_eq!(
            normalize_exact_magnitude(true, i32::MAX, 0),
            Some((false, 0, 0))
        );
        assert_eq!(
            normalize_exact_magnitude(false, i32::MIN + 127, 1),
            Some((false, i32::MIN, 1_u128 << 127))
        );
        assert_eq!(normalize_exact_magnitude(true, i32::MIN, 1), None);
        assert_eq!(
            normalize_exact_magnitude(true, i32::MIN + 1, 1_u128 << 126),
            Some((true, i32::MIN, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_magnitude(false, i32::MIN, 1_u128 << 126),
            None
        );
        assert_eq!(
            normalize_exact_magnitude(false, i32::MAX, 1_u128 << 126),
            Some((false, i32::MAX - 1, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_magnitude(true, i32::MAX, 1_u128 << 127),
            Some((true, i32::MAX, 1_u128 << 127))
        );
    }

    #[test]
    fn exact_magnitude_normalization_composes_with_significand_product() {
        let minimum_raw = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        let maximum_pseudo_raw =
            X87Binary80::from_bytes([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0x80]);
        let minimum = minimum_raw.normalize_finite().unwrap();
        let maximum = maximum_pseudo_raw.normalize_finite().unwrap();
        let exponent = minimum.exponent();
        let raw_before = (minimum_raw, maximum_pseudo_raw);
        let finite_before = (minimum, maximum);
        assert_eq!(exponent, maximum.exponent());
        assert_eq!(minimum.source_class(), X87Binary80Class::Normal);
        assert_eq!(maximum.source_class(), X87Binary80Class::PseudoDenormal);

        let minimum_product = multiply_finite_significands(minimum, minimum).unwrap();
        assert_eq!(minimum_product, (false, exponent * 2 + 1, 1_u128 << 126));
        assert_eq!(
            normalize_exact_magnitude(minimum_product.0, minimum_product.1, minimum_product.2),
            Some((false, exponent * 2, 1_u128 << 127))
        );

        let maximum_product = multiply_finite_significands(maximum, maximum).unwrap();
        assert_eq!(
            maximum_product,
            (
                false,
                exponent * 2 + 1,
                0xffff_ffff_ffff_fffe_0000_0000_0000_0001
            )
        );
        assert_eq!(
            normalize_exact_magnitude(maximum_product.0, maximum_product.1, maximum_product.2),
            Some(maximum_product)
        );

        let mixed_product = multiply_finite_significands(minimum, maximum).unwrap();
        assert_eq!(
            mixed_product,
            (
                true,
                exponent * 2 + 1,
                0x7fff_ffff_ffff_ffff_8000_0000_0000_0000
            )
        );
        assert_eq!(
            normalize_exact_magnitude(mixed_product.0, mixed_product.1, mixed_product.2),
            Some((
                true,
                exponent * 2,
                0xffff_ffff_ffff_ffff_0000_0000_0000_0000
            ))
        );

        assert_eq!((minimum_raw, maximum_pseudo_raw), raw_before);
        assert_eq!((minimum, maximum), finite_before);
        assert_eq!(minimum_raw.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        assert_eq!(
            maximum_pseudo_raw.to_bytes(),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0, 0x80]
        );
        assert_eq!(minimum.source_class(), X87Binary80Class::Normal);
        assert_eq!(maximum.source_class(), X87Binary80Class::PseudoDenormal);
    }

    #[test]
    fn exact_coefficient_normalization_composes_with_same_exponent_sum() {
        let exponent = -16382;
        let min_normal_raw = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        let adjacent_raw = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]);
        let min_normal = min_normal_raw.normalize_finite().unwrap();
        let adjacent = adjacent_raw.normalize_finite().unwrap();
        let raw_before = (min_normal_raw, adjacent_raw);
        let finite_before = (min_normal, adjacent);
        let (sum_exponent, coefficient) = add_finite_same_exponent(min_normal, adjacent).unwrap();
        assert_eq!(coefficient, 0x1_0000_0000_0000_0001);
        assert_eq!(
            normalize_exact_coefficient(sum_exponent, coefficient),
            Some((
                false,
                exponent + 1,
                0x8000_0000_0000_0000_8000_0000_0000_0000
            ))
        );

        let adjacent_negative_raw = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0x80, 1, 0x80]);
        let adjacent_negative = adjacent_negative_raw.normalize_finite().unwrap();
        let raw_difference_before = (min_normal_raw, adjacent_negative_raw);
        let (difference_exponent, difference) =
            add_finite_same_exponent(min_normal, adjacent_negative).unwrap();
        assert_eq!(difference, -1);
        assert_eq!(
            normalize_exact_coefficient(difference_exponent, difference),
            Some((true, exponent - 63, 1_u128 << 127))
        );
        assert_eq!(
            (min_normal_raw, adjacent_negative_raw),
            raw_difference_before
        );
        assert_eq!(adjacent_negative.source_class(), X87Binary80Class::Normal);

        let leading = finite(
            exponent,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let adjacent_large = finite(
            exponent,
            0x8000_0000_0000_0001,
            false,
            X87Binary80Class::Normal,
        );
        let leading_negative = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Normal,
        );
        let (_, positive_one) = add_finite_same_exponent(adjacent_large, leading_negative).unwrap();
        let adjacent_large_negative = finite(
            exponent,
            0x8000_0000_0000_0001,
            true,
            X87Binary80Class::Normal,
        );
        let (_, negative_one) = add_finite_same_exponent(leading, adjacent_large_negative).unwrap();
        assert_eq!(positive_one, 1);
        assert_eq!(negative_one, -1);
        assert_eq!(
            normalize_exact_coefficient(exponent, positive_one),
            Some((false, exponent - 63, 1_u128 << 127))
        );
        assert_eq!(
            normalize_exact_coefficient(exponent, negative_one),
            Some((true, exponent - 63, 1_u128 << 127))
        );

        let opposite = finite(
            exponent,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Normal,
        );
        let (_, zero) = add_finite_same_exponent(leading, opposite).unwrap();
        assert_eq!(zero, 0);
        assert_eq!(
            normalize_exact_coefficient(exponent, zero),
            Some((false, 0, 0))
        );

        let maximum = finite(16383, u64::MAX, false, X87Binary80Class::Normal);
        let (sum_exponent, coefficient) = add_finite_same_exponent(maximum, maximum).unwrap();
        assert_eq!(
            normalize_exact_coefficient(sum_exponent, coefficient),
            Some((false, 16384, 0xffff_ffff_ffff_ffff_0000_0000_0000_0000))
        );

        assert_eq!((min_normal_raw, adjacent_raw), raw_before);
        assert_eq!((min_normal, adjacent), finite_before);
        assert_eq!(min_normal.source_class(), X87Binary80Class::Normal);
        assert_eq!(adjacent.source_class(), X87Binary80Class::Normal);
    }

    #[test]
    fn right_shift_jam_covers_word_boundaries_and_sticky_bits() {
        assert_eq!(shift_right_jam(0, 0), 0);
        assert_eq!(shift_right_jam(0x35, 0), 0x35);
        assert_eq!(shift_right_jam(0b1101, 1), 0b0111);
        assert_eq!(shift_right_jam(1_u64 << 63, 63), 1);
        assert_eq!(shift_right_jam(1_u64 << 63, 64), 1);
        assert_eq!(shift_right_jam(1_u64 << 63, 65), 1);
        assert_eq!(shift_right_jam(0, 64), 0);
        assert_eq!(shift_right_jam(0, 65), 0);
        assert_eq!(shift_right_jam(1_u64 << 63, 1), 1_u64 << 62);

        let grs = X87Grs::from_low_bits(0b101);
        assert!(grs.guard());
        assert!(!grs.round());
        assert!(grs.sticky());
        let exact = X87Grs::from_low_bits(0b010);
        assert!(!exact.guard());
        assert!(exact.round());
        assert!(!exact.sticky());
    }

    #[test]
    fn u128_right_shift_jam_matches_literal_vectors_at_all_boundaries() {
        let distances = [0, 1, 2, 3, 63, 64, 65, 66, 67, 127, 128, 129, u32::MAX];
        let max_expected = [
            u128::MAX,
            0x7fff_ffff_ffff_ffff_ffff_ffff_ffff_ffff,
            0x3fff_ffff_ffff_ffff_ffff_ffff_ffff_ffff,
            0x1fff_ffff_ffff_ffff_ffff_ffff_ffff_ffff,
            0x0000_0000_0000_0001_ffff_ffff_ffff_ffff,
            0x0000_0000_0000_0000_ffff_ffff_ffff_ffff,
            0x0000_0000_0000_0000_7fff_ffff_ffff_ffff,
            0x0000_0000_0000_0000_3fff_ffff_ffff_ffff,
            0x0000_0000_0000_0000_1fff_ffff_ffff_ffff,
            1,
            1,
            1,
            1,
        ];
        let high_bit_expected = [
            0x8000_0000_0000_0000_0000_0000_0000_0000,
            0x4000_0000_0000_0000_0000_0000_0000_0000,
            0x2000_0000_0000_0000_0000_0000_0000_0000,
            0x1000_0000_0000_0000_0000_0000_0000_0000,
            0x0000_0000_0000_0001_0000_0000_0000_0000,
            0x0000_0000_0000_0000_8000_0000_0000_0000,
            0x0000_0000_0000_0000_4000_0000_0000_0000,
            0x0000_0000_0000_0000_2000_0000_0000_0000,
            0x0000_0000_0000_0000_1000_0000_0000_0000,
            1,
            1,
            1,
            1,
        ];

        for ((distance, expected_max), expected_high) in distances
            .into_iter()
            .zip(max_expected)
            .zip(high_bit_expected)
        {
            assert_eq!(shift_right_jam_u128(0, distance), 0);
            assert_eq!(shift_right_jam_u128(u128::MAX, distance), expected_max);
            assert_eq!(shift_right_jam_u128(1_u128 << 127, distance), expected_high);
        }
    }

    #[test]
    fn u128_right_shift_jam_preserves_sticky_across_repeated_shifts() {
        assert_eq!(shift_right_jam_u128(0x11, 1), 0x09);
        assert_eq!(shift_right_jam_u128(0x09, 1), 0x05);
        assert_eq!(shift_right_jam_u128(0x05, 1), 0x03);
        assert_eq!(shift_right_jam_u128(0x03, 1), 0x01);
        assert_eq!(shift_right_jam_u128(0x11, 4), 0x01);
    }

    #[test]
    fn normalized_finite_significands_bridge_to_u128_grs_without_losing_metadata() {
        let cases = [
            (
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                true,
                X87Binary80Class::Zero,
                0,
                0,
                0,
                0b000,
            ),
            (
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                true,
                X87Binary80Class::Subnormal,
                0x8000_0000_0000_0000,
                0,
                0x8000_0000_0000_0000,
                0b000,
            ),
            (
                [5, 0, 0, 0, 0, 0, 0, 0x80, 1, 0],
                false,
                X87Binary80Class::Normal,
                0x8000_0000_0000_0005,
                3,
                0x1000_0000_0000_0000,
                0b101,
            ),
            (
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80],
                true,
                X87Binary80Class::PseudoDenormal,
                0x8000_0000_0000_0000,
                0,
                0x8000_0000_0000_0000,
                0b000,
            ),
        ];

        for (
            bytes,
            expected_sign,
            expected_class,
            expected_significand,
            distance,
            expected_retained,
            expected_grs_bits,
        ) in cases
        {
            let finite = X87Binary80::from_bytes(bytes)
                .normalize_finite()
                .expect("finite class");
            assert_eq!(finite.sign(), expected_sign);
            assert_eq!(finite.source_class(), expected_class);
            assert_eq!(finite.significand(), expected_significand);

            let aligned = (finite.significand() as u128) << 3;
            let shifted = shift_right_jam_u128(aligned, distance);
            let grs = X87Grs::from_low_bits((shifted & 0b111) as u64);
            let retained = (shifted >> 3) as u64;

            assert_eq!(retained, expected_retained);
            assert_eq!(grs.guard(), expected_grs_bits & 0b100 != 0);
            assert_eq!(grs.round(), expected_grs_bits & 0b010 != 0);
            assert_eq!(grs.sticky(), expected_grs_bits & 0b001 != 0);
        }
    }
}
