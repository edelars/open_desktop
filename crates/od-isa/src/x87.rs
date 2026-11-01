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

pub use crate::rational_rounding::RationalRoundingMode;

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
    crate::rational_rounding::round_quotient_to_precision(sign, q, r, d, precision, mode)
        .map(|(magnitude, _)| magnitude)
}

/// Pack an already-rounded, nonzero normal significand into binary80.
///
/// The input represents `(-1)^sign * magnitude * 2^(exponent + 1 - precision)`.
/// A carry to `2^precision` is normalized by incrementing the exponent once.
/// This helper accepts only normal finite results, performs no rounding, and
/// defines no architectural x87 range, status, or exception behavior.
pub fn pack_normal_rounded_significand(
    sign: bool,
    exponent: i32,
    magnitude: u128,
    precision: u32,
) -> Option<X87Binary80> {
    if !(1..=64).contains(&precision) {
        return None;
    }

    let precision_limit = 1_u128 << precision;
    if magnitude < (precision_limit >> 1) || magnitude > precision_limit {
        return None;
    }

    let (significand, exponent) = if magnitude == precision_limit {
        (1_u64 << 63, exponent.checked_add(1)?)
    } else {
        ((magnitude << (64 - precision)) as u64, exponent)
    };
    if !(-16382..=16383).contains(&exponent) {
        return None;
    }

    let biased_exponent = (exponent + 16383) as u16;
    let sign_exponent = biased_exponent | if sign { 0x8000 } else { 0 };
    let significand_bytes = significand.to_le_bytes();
    let sign_exponent_bytes = sign_exponent.to_le_bytes();
    Some(X87Binary80::from_bytes([
        significand_bytes[0],
        significand_bytes[1],
        significand_bytes[2],
        significand_bytes[3],
        significand_bytes[4],
        significand_bytes[5],
        significand_bytes[6],
        significand_bytes[7],
        sign_exponent_bytes[0],
        sign_exponent_bytes[1],
    ]))
}

/// Square-root a positive, nonzero normalized finite binary80 value and round
/// once to the binary80 significand precision.
///
/// This helper accepts only normalized finite inputs with a nonnegative sign;
/// zero, negative values, and malformed non-normalized significands return
/// `None`. The exact radicand/root remainder are computed with integer math.
/// It defines no x87 instruction, FCW, exception, or status behavior.
pub fn sqrt_finite_to_binary80(
    value: X87Finite,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if value.sign || value.is_zero() || value.significand < (1_u64 << 63) {
        return None;
    }

    let (exponent, root, remainder) =
        crate::finite_arithmetic::sqrt_finite_parts(value.significand, value.exponent)?;
    let increment = match mode {
        // R > q is exactly the condition that sqrt(N) is above q + 1/2.
        // Equality at the half boundary is impossible because N and q are
        // integers while (q + 1/2)^2 has a fractional quarter.
        RationalRoundingMode::NearestTiesEven => remainder > u128::from(root),
        RationalRoundingMode::TowardZero | RationalRoundingMode::TowardNegative => false,
        RationalRoundingMode::TowardPositive => remainder != 0,
    };
    let rounded_significand = u128::from(root) + u128::from(increment);
    pack_normal_rounded_significand(false, exponent, rounded_significand, 64)
}

/// Return the exact truncated finite remainder coefficient and exponent for
/// normalized nonzero binary80 values.
///
/// The returned pair `(exponent, coefficient)` represents
/// `coefficient * 2^(exponent - 63)`. For exponent gaps from zero through 64,
/// the helper computes the remainder of `(lhs.significand << gap)` divided by
/// `rhs.significand`. A negative gap returns the left operand unchanged. Gaps
/// above 64 are outside this bounded helper and return `None`. Only normal,
/// subnormal, and pseudo-denormal provenance with a normalized significand is
/// accepted. This is exact integer arithmetic only: it does not implement an
/// x87 remainder instruction, remainder rounding variant, status flags,
/// exceptions, or state changes.
pub fn remainder_finite_trunc(lhs: X87Finite, rhs: X87Finite) -> Option<(i32, i128)> {
    let valid = |value: X87Finite| {
        matches!(
            value.source_class,
            X87Binary80Class::Normal
                | X87Binary80Class::Subnormal
                | X87Binary80Class::PseudoDenormal
        ) && value.significand & (1_u64 << 63) != 0
    };
    if !valid(lhs) || !valid(rhs) {
        return None;
    }

    let signed = |magnitude: u64, sign: bool| {
        let magnitude = i128::from(magnitude);
        if sign { -magnitude } else { magnitude }
    };
    let gap = i64::from(lhs.exponent) - i64::from(rhs.exponent);
    if gap < 0 {
        return Some((lhs.exponent, signed(lhs.significand, lhs.sign)));
    }
    if gap > 64 {
        return None;
    }

    // `lhs.significand` has at most 64 bits, so shifting by at most 64 keeps
    // the exact numerator within u128. The remainder is strictly below the
    // 64-bit divisor and therefore safely representable as a signed i128.
    let numerator = u128::from(lhs.significand) << (gap as u32);
    let remainder = (numerator % u128::from(rhs.significand)) as u64;
    Some((rhs.exponent, signed(remainder, lhs.sign)))
}

/// Return the exact finite nearest-even remainder coefficient and exponent.
///
/// The returned pair `(exponent, coefficient)` represents
/// `coefficient * 2^(exponent - 63)` and equals
/// `lhs - nearest_integer_ties_even(lhs / rhs) * rhs`. Only normalized
/// nonzero Normal, Subnormal, and PseudoDenormal inputs are accepted. The
/// bounded integer decomposition handles exponent gaps from -1 through 64;
/// smaller-magnitude lhs values are returned unchanged and larger gaps are
/// rejected. This helper does not implement FPREM1 or any x87 instruction,
/// status, exception, or quotient-bit behavior.
pub fn remainder_finite_nearest_even(lhs: X87Finite, rhs: X87Finite) -> Option<(i32, i128)> {
    let valid = |value: X87Finite| {
        matches!(
            value.source_class,
            X87Binary80Class::Normal
                | X87Binary80Class::Subnormal
                | X87Binary80Class::PseudoDenormal
        ) && value.significand & (1_u64 << 63) != 0
    };
    if !valid(lhs) || !valid(rhs) {
        return None;
    }

    let apply_sign = |coefficient: i128, sign: bool| {
        if sign { -coefficient } else { coefficient }
    };
    let gap = i64::from(lhs.exponent) - i64::from(rhs.exponent);
    if gap <= -2 {
        return Some((
            lhs.exponent,
            apply_sign(i128::from(lhs.significand), lhs.sign),
        ));
    }
    if gap == -1 {
        if lhs.significand <= rhs.significand {
            // The ratio is at most one half; at equality, zero is even.
            return Some((
                lhs.exponent,
                apply_sign(i128::from(lhs.significand), lhs.sign),
            ));
        }
        let coefficient = i128::from(lhs.significand) - (i128::from(rhs.significand) << 1);
        return Some((lhs.exponent, apply_sign(coefficient, lhs.sign)));
    }
    if gap > 64 {
        return None;
    }

    // With a gap no larger than 64, the exact numerator fits in u128. Keep
    // both quotient and doubled remainder wide through the tie decision.
    let numerator = u128::from(lhs.significand) << (gap as u32);
    let denominator = u128::from(rhs.significand);
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    let twice_remainder = remainder * 2;
    let increment =
        twice_remainder > denominator || (twice_remainder == denominator && quotient & 1 != 0);
    let coefficient = if increment {
        i128::try_from(remainder).ok()? - i128::from(rhs.significand)
    } else {
        i128::try_from(remainder).ok()?
    };
    Some((rhs.exponent, apply_sign(coefficient, lhs.sign)))
}

/// Round an exact normalized quotient directly into the binary80 subnormal
/// range, preserving the quotient's rational tail until the final rounding.
///
/// The exact value is `(-1)^sign * (q + r / d) * 2^(e - 63)`. The quotient
/// must be normalized (`q >= 2^63`), `d` must be nonzero, `r < d`, and `e`
/// must be at most -16383. A rounded magnitude below `2^63` is encoded with
/// exponent field zero; `2^63` is the minimum normal encoding. Underflow to
/// zero retains the input sign. This helper defines mathematical rounding and
/// binary80 encoding only; it does not model architectural flags, exceptions,
/// or x87 control-word behavior.
pub fn round_pack_subnormal_quotient(
    sign: bool,
    e: i32,
    q: u64,
    r: u64,
    d: u64,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if e > -16383 || q < (1_u64 << 63) || d == 0 || r >= d {
        return None;
    }

    let k = -16382_i64 - i64::from(e);
    let magnitude = if k <= 63 {
        round_quotient_to_precision(sign, q, r, d, (64 - k) as u32, mode)?
    } else if k == 64 {
        let above_half = q > (1_u64 << 63) || (q == (1_u64 << 63) && r != 0);
        let increment = match mode {
            RationalRoundingMode::NearestTiesEven => above_half,
            RationalRoundingMode::TowardZero => false,
            // The scaled value is always strictly between zero and one unit,
            // including the exact half-way input.
            RationalRoundingMode::TowardPositive => !sign,
            RationalRoundingMode::TowardNegative => sign,
        };
        u128::from(increment)
    } else {
        u128::from(match mode {
            RationalRoundingMode::TowardPositive => !sign,
            RationalRoundingMode::TowardNegative => sign,
            RationalRoundingMode::NearestTiesEven | RationalRoundingMode::TowardZero => false,
        })
    };

    let significand = if magnitude == (1_u128 << 63) {
        1_u64 << 63
    } else {
        magnitude as u64
    };
    let exponent = if magnitude == (1_u128 << 63) {
        1_u16
    } else {
        0_u16
    };
    let sign_exponent = exponent | if sign { 0x8000 } else { 0 };
    let significand_bytes = significand.to_le_bytes();
    let sign_exponent_bytes = sign_exponent.to_le_bytes();
    Some(X87Binary80::from_bytes([
        significand_bytes[0],
        significand_bytes[1],
        significand_bytes[2],
        significand_bytes[3],
        significand_bytes[4],
        significand_bytes[5],
        significand_bytes[6],
        significand_bytes[7],
        sign_exponent_bytes[0],
        sign_exponent_bytes[1],
    ]))
}

/// Divide two nonzero finite values and round once to a bounded binary80 value.
///
/// The exact divider result is rounded to 64 significant bits when its
/// unrounded exponent is normal (`e >= -16382`). Values below that boundary
/// are rounded directly into the subnormal lattice from the original exact
/// quotient and remainder. Zero operands are rejected before division because
/// this helper does not choose an operation's signed-zero result. `None`
/// reports only a bounded-helper rejection, including zero operands and
/// results outside the finite binary80 exponent range; it does not represent a
/// guest exception. This helper defines no x87 status flags, exceptions,
/// control-word semantics, or instruction behavior.
pub fn divide_finite_to_binary80(
    lhs: X87Finite,
    rhs: X87Finite,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if lhs.is_zero() || rhs.is_zero() {
        return None;
    }

    let quotient = divide_finite_significands(lhs, rhs)?;
    let sign = lhs.sign ^ rhs.sign;
    let e = quotient.exponent();
    let q = quotient.quotient();
    let r = quotient.remainder();
    let d = quotient.denominator();
    if e >= -16382 {
        let magnitude = round_quotient_to_precision(sign, q, r, d, 64, mode)?;
        pack_normal_rounded_significand(sign, e, magnitude, 64)
    } else {
        round_pack_subnormal_quotient(sign, e, q, r, d, mode)
    }
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

/// Add two nonzero finite values with unequal exponents using an exact
/// coefficient at the lower exponent.
///
/// The returned pair represents `coefficient * 2^(exponent - 63)`. Exponent
/// gaps from one through 62 fit the aligned signed coefficient in `i128`;
/// larger gaps are rejected rather than rounded or jammed. Equal exponents
/// and zero operands are outside this helper. `None` is bounded-helper
/// rejection, not an architectural result or guest exception.
pub fn add_finite_unequal_exponents(lhs: X87Finite, rhs: X87Finite) -> Option<(i32, i128)> {
    if lhs.is_zero() || rhs.is_zero() || lhs.exponent == rhs.exponent {
        return None;
    }

    let (higher, lower) = if lhs.exponent > rhs.exponent {
        (lhs, rhs)
    } else {
        (rhs, lhs)
    };
    let gap = i64::from(higher.exponent) - i64::from(lower.exponent);
    if !(1..=62).contains(&gap) {
        return None;
    }

    let scale = 1_i128.checked_shl(gap as u32)?;
    let coefficient = signed_significand(higher)
        .checked_mul(scale)?
        .checked_add(signed_significand(lower))?;
    Some((lower.exponent, coefficient))
}

/// Add finite binary80 values with matching nonzero exponents and round once.
///
/// Zero operands are neutral; two zeros and exact cancellation are rejected
/// because this helper does not choose an architectural zero sign. Unequal
/// nonzero exponents are outside this exact, unaligned path. `None` means only
/// bounded-helper rejection, not a guest exception or architectural result.
pub fn add_finite_same_exponent_to_binary80(
    lhs: X87Finite,
    rhs: X87Finite,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    let (exponent, coefficient) = add_finite_same_exponent(lhs, rhs)?;
    round_pack_exact_coefficient(exponent, coefficient, mode)
}

/// Add finite values with exact alignment, then round and pack once into binary80.
///
/// A single zero is neutral in either operand position. Two zeros and exact
/// cancellation return `None` because no signed-zero choice is made here.
/// Nonzero operands with exponent gaps through 64 are aligned losslessly into
/// `u128`; wider gaps use a p=64-aware quotient-tail bridge. The exact path
/// uses the magnitude packer once without an `i128` narrowing intermediate.
/// `None` also includes checked-exponent or finite-binary80 range rejection
/// and is not a guest exception.
pub fn add_finite_to_binary80(
    lhs: X87Finite,
    rhs: X87Finite,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    let exact = crate::finite_arithmetic::add_finite_aligned(
        lhs.sign,
        lhs.significand,
        lhs.exponent,
        rhs.sign,
        rhs.significand,
        rhs.exponent,
    );
    if let Some((sign, exponent, magnitude)) = exact {
        if magnitude == 0 {
            return None;
        }
        return round_pack_exact_magnitude(sign, exponent.checked_add(64)?, magnitude, mode);
    }

    let (sign, exponent, quotient, tail) =
        crate::finite_arithmetic::finite_large_gap_rounding_bridge(
            lhs.sign,
            lhs.significand,
            lhs.exponent,
            rhs.sign,
            rhs.significand,
            rhs.exponent,
        )?;
    let (remainder, denominator) = tail.representative();
    if exponent >= -16382 {
        let rounded =
            round_quotient_to_precision(sign, quotient, remainder, denominator, 64, mode)?;
        pack_normal_rounded_significand(sign, exponent, rounded, 64)
    } else {
        round_pack_subnormal_quotient(sign, exponent, quotient, remainder, denominator, mode)
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

/// Round an exact signed coefficient `coefficient * 2^(exponent - 63)` once
/// into finite binary80 normal or subnormal encoding.
///
/// Zero is rejected because this helper does not choose a signed-zero result.
/// For nonzero coefficients, normalization preserves the exact magnitude. Its
/// even 128-bit result is decomposed as `q + r / 2^63`, then rounded directly
/// at the final normal or subnormal boundary. `None` reports only rejection by
/// these bounded numeric helpers, including exponent overflow or a result
/// outside the finite binary80 range. No x87 control-word, status, exception,
/// instruction, or signed-zero operation behavior is defined here.
pub fn round_pack_exact_coefficient(
    exponent: i32,
    coefficient: i128,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if coefficient == 0 {
        return None;
    }

    let (sign, normalized_exponent, magnitude) =
        normalize_exact_coefficient(exponent, coefficient)?;
    let q = (magnitude >> 64) as u64;
    let r = (magnitude as u64) >> 1;
    const D: u64 = 1_u64 << 63;

    if normalized_exponent >= -16382 {
        let rounded = round_quotient_to_precision(sign, q, r, D, 64, mode)?;
        pack_normal_rounded_significand(sign, normalized_exponent, rounded, 64)
    } else {
        round_pack_subnormal_quotient(sign, normalized_exponent, q, r, D, mode)
    }
}

/// Scale a normalized nonzero finite binary80 value by an exact power of two
/// and round once into finite binary80.
///
/// The input value is `(-1)^sign * significand * 2^(exponent - 63)`. Only
/// normalized finite values with the leading significand bit set are
/// accepted; normal, subnormal, and pseudo-denormal provenance is retained
/// by the immutable input and does not affect the numerical result. The
/// exponent adjustment is checked before the exact signed coefficient is
/// passed once to `round_pack_exact_coefficient`. `None` means bounded helper
/// rejection (invalid input, checked exponent overflow, or a result outside
/// the finite binary80 range). No x87 control-word, status, exception,
/// instruction, or signed-zero operation behavior is defined here.
pub fn scale_finite_to_binary80(
    value: X87Finite,
    power: i32,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if value.is_zero()
        || value.significand < (1_u64 << 63)
        || !matches!(
            value.source_class,
            X87Binary80Class::Normal
                | X87Binary80Class::Subnormal
                | X87Binary80Class::PseudoDenormal
        )
    {
        return None;
    }

    let exponent = value.exponent.checked_add(power)?;
    let coefficient = if value.sign {
        -i128::from(value.significand)
    } else {
        i128::from(value.significand)
    };
    round_pack_exact_coefficient(exponent, coefficient, mode)
}

/// Round an exact signed unsigned magnitude once into finite binary80.
///
/// The exact input is `(-1)^sign * magnitude * 2^(exponent - 127)`. Unlike the
/// coefficient rounder, the magnitude may be odd in its low bit. After exact
/// normalization, the low 64 bits are compressed into a 63-bit remainder plus
/// sticky bit. This preserves zero/nonzero, below-half, exact-half, and
/// above-half distinctions as well as the tail information needed by normal
/// and subnormal rounding. `None` is only helper rejection (zero input,
/// exponent overflow, or a result outside finite binary80 range); it does not
/// define architectural signed-zero or exception behavior.
pub fn round_pack_exact_magnitude(
    sign: bool,
    exponent: i32,
    magnitude: u128,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if magnitude == 0 {
        return None;
    }

    let (sign, normalized_exponent, normalized_magnitude) =
        normalize_exact_magnitude(sign, exponent, magnitude)?;
    let q = (normalized_magnitude >> 64) as u64;
    let low = normalized_magnitude as u64;
    let r = (low >> 1) | (low & 1);
    const D: u64 = 1_u64 << 63;

    if normalized_exponent >= -16382 {
        let rounded = round_quotient_to_precision(sign, q, r, D, 64, mode)?;
        pack_normal_rounded_significand(sign, normalized_exponent, rounded, 64)
    } else {
        round_pack_subnormal_quotient(sign, normalized_exponent, q, r, D, mode)
    }
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

/// Multiply two nonzero finite values and round their exact product once to
/// finite binary80.
///
/// The full unsigned 128-bit product is passed directly to
/// `round_pack_exact_magnitude`, so odd low product bits remain available as a
/// sticky tail. Zero operands are rejected because this helper does not choose
/// an architectural signed-zero result. `None` means bounded helper rejection
/// (zero input, checked exponent failure, or finite-range overflow), not a
/// guest exception. No x87 flags, control-word behavior, or instruction
/// semantics are defined here.
pub fn multiply_finite_to_binary80(
    lhs: X87Finite,
    rhs: X87Finite,
    mode: RationalRoundingMode,
) -> Option<X87Binary80> {
    if lhs.is_zero() || rhs.is_zero() {
        return None;
    }

    let (sign, exponent, magnitude) = multiply_finite_significands(lhs, rhs)?;
    round_pack_exact_magnitude(sign, exponent, magnitude, mode)
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
        add_finite_same_exponent, add_finite_same_exponent_to_binary80, add_finite_to_binary80,
        add_finite_unequal_exponents, align_finite_pair, divide_finite_significands,
        divide_finite_to_binary80, multiply_finite_significands, multiply_finite_to_binary80,
        normalize_exact_coefficient, normalize_exact_magnitude, pack_normal_rounded_significand,
        remainder_finite_nearest_even, remainder_finite_trunc, round_pack_exact_coefficient,
        round_pack_exact_magnitude, round_pack_subnormal_quotient, round_quotient_magnitude,
        round_quotient_ties_even, round_quotient_to_precision, scale_finite_to_binary80,
        shift_right_jam, shift_right_jam_u128, sqrt_finite_to_binary80,
    };
    use crate::state::{X87State, X87Tag};

    #[test]
    fn normal_significand_packing_matches_literal_binary80_encodings() {
        let cases = [
            (
                false,
                0,
                1_u128,
                1,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
            ),
            (
                true,
                0,
                3,
                2,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xc0, 0xff, 0xbf],
            ),
            (
                false,
                0,
                2,
                1,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0x40],
            ),
            (
                false,
                7,
                0x00c0_1234,
                24,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x34, 0x12, 0xc0, 0x06, 0x40],
            ),
            (
                false,
                0,
                0x0010_0000_0000_0001,
                53,
                [0x00, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
            ),
            (
                false,
                0,
                0x4000_0000_0000_0001,
                63,
                [0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
            ),
            (
                false,
                0,
                0x8000_0000_0000_0001,
                64,
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0xff, 0x3f],
            ),
            (
                true,
                0,
                1_u128 << 64,
                64,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x00, 0xc0],
            ),
            (
                false,
                -16382,
                1_u128 << 23,
                24,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00],
            ),
            (
                false,
                -16383,
                1_u128 << 24,
                24,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00],
            ),
            (
                false,
                16383,
                (1_u128 << 64) - 1,
                64,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f],
            ),
        ];

        for (sign, exponent, magnitude, precision, expected) in cases {
            let packed = pack_normal_rounded_significand(sign, exponent, magnitude, precision)
                .expect("literal normal value should pack");
            assert_eq!(packed.to_bytes(), expected);
            assert_eq!(packed.classify(), X87Binary80Class::Normal);
        }
    }

    #[test]
    fn subnormal_quotient_rounding_matches_literal_boundaries_and_modes() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let half_minimum_cases = [
            (false, Nearest, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (false, Zero, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (false, Up, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (false, Down, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (true, Nearest, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (true, Zero, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (true, Up, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (true, Down, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
        ];
        for (sign, mode, expected) in half_minimum_cases {
            assert_eq!(
                round_pack_subnormal_quotient(sign, -16446, 1_u64 << 63, 0, 1, mode)
                    .unwrap()
                    .to_bytes(),
                expected,
                "half-minimum sign={sign} mode={mode:?}"
            );
        }

        let cases = [
            // k=1: half of minimum normal is exactly subnormal.
            (
                -16383,
                0x8000_0000_0000_0000,
                0,
                1,
                Nearest,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00],
            ),
            // k=63 and k=64 exercise the precision and integer-round paths.
            (
                -16445,
                0x8000_0000_0000_0000,
                0,
                1,
                Nearest,
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ),
            (
                -16446,
                0xc000_0000_0000_0000,
                0,
                1,
                Nearest,
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ),
            // k=65 is below half the minimum subnormal, but directed rounding
            // away from zero still produces one unit.
            (
                -16447,
                0x8000_0000_0000_0000,
                0,
                1,
                Up,
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
            ),
            // The largest exact subnormal stays below the normal boundary.
            (
                -16383,
                u64::MAX - 1,
                0,
                1,
                Nearest,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0x00, 0x00],
            ),
            // A tie at the top of the subnormal range rounds to min normal.
            (
                -16383,
                u64::MAX,
                0,
                1,
                Nearest,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x00],
            ),
            // Directed positive rounding of a tiny value increments one unit.
            (
                -16383,
                0x8000_0000_0000_0000,
                1,
                2,
                Up,
                [0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00],
            ),
        ];
        for (e, q, r, d, mode, expected) in cases {
            let packed = round_pack_subnormal_quotient(false, e, q, r, d, mode).unwrap();
            assert_eq!(
                packed.to_bytes(),
                expected,
                "e={e} q={q:#018x} mode={mode:?}"
            );
        }

        assert_eq!(
            round_pack_subnormal_quotient(true, i32::MIN, 1_u64 << 63, 0, 1, Down)
                .unwrap()
                .to_bytes(),
            [0x01, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );
        assert_eq!(
            round_pack_subnormal_quotient(false, i32::MIN, 1_u64 << 63, 0, 1, Nearest)
                .unwrap()
                .to_bytes(),
            [0; 10]
        );
    }

    #[test]
    fn subnormal_quotient_rounding_validates_domain_and_avoids_double_rounding() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        for (e, q, r, d) in [
            (-16382, 1_u64 << 63, 0, 1),
            (-16383, (1_u64 << 63) - 1, 0, 1),
            (-16383, 1_u64 << 63, 0, 0),
            (-16383, 1_u64 << 63, 1, 1),
        ] {
            assert_eq!(
                round_pack_subnormal_quotient(false, e, q, r, d, Nearest),
                None
            );
        }

        let direct =
            round_pack_subnormal_quotient(false, -16383, (1_u64 << 63) + 1, 1, 3, Nearest).unwrap();
        assert_eq!(direct.to_bytes(), [0x01, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]);
    }

    #[test]
    fn subnormal_quotient_rounding_composes_with_division() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let min_normal = finite(-16382, 1_u64 << 63, false, X87Binary80Class::Normal);
        let two = finite(1, 1_u64 << 63, false, X87Binary80Class::Normal);
        let three = finite(1, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);

        for (denominator, expected) in [
            (
                two,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40, 0x00, 0x00],
            ),
            (
                three,
                [0xab, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0x2a, 0x00, 0x00],
            ),
        ] {
            let quotient = divide_finite_significands(min_normal, denominator).unwrap();
            let packed = round_pack_subnormal_quotient(
                quotient.sign(),
                quotient.exponent(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                Nearest,
            )
            .unwrap();
            assert_eq!(packed.to_bytes(), expected);
            assert_eq!(packed.classify(), X87Binary80Class::Subnormal);
        }
    }

    #[test]
    fn finite_division_composes_with_single_rounding_and_binary80_packing() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let normal_one = finite(0, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let normal_half = finite(1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(normal_one, normal_half, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfe, 0x3f]
        );

        let third_numerator = finite(-1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let third_denominator = finite(0, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
        for (sign, mode, significand) in [
            (false, Nearest, 0xaaaa_aaaa_aaaa_aaab_u64),
            (false, Zero, 0xaaaa_aaaa_aaaa_aaaa),
            (false, Up, 0xaaaa_aaaa_aaaa_aaab),
            (false, Down, 0xaaaa_aaaa_aaaa_aaaa),
            (true, Nearest, 0xaaaa_aaaa_aaaa_aaab),
            (true, Zero, 0xaaaa_aaaa_aaaa_aaaa),
            (true, Up, 0xaaaa_aaaa_aaaa_aaaa),
            (true, Down, 0xaaaa_aaaa_aaaa_aaab),
        ] {
            let numerator = finite(
                third_numerator.exponent(),
                third_numerator.significand(),
                sign,
                X87Binary80Class::Normal,
            );
            let expected_sign_exponent: u16 = if sign { 0xbf_fd } else { 0x3f_fd };
            let mut expected = [0; 10];
            expected[..8].copy_from_slice(&significand.to_le_bytes());
            expected[8..].copy_from_slice(&expected_sign_exponent.to_le_bytes());
            assert_eq!(
                divide_finite_to_binary80(numerator, third_denominator, mode)
                    .unwrap()
                    .to_bytes(),
                expected,
                "sign={sign} mode={mode:?}"
            );
        }

        let min_normal = finite(-16382, 1_u64 << 63, false, X87Binary80Class::Normal);
        let one = finite(0, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(min_normal, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]
        );
        let two = finite(1, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(min_normal, two, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]
        );
        let three = finite(1, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(min_normal, three, Nearest)
                .unwrap()
                .to_bytes(),
            [0xab, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0x2a, 0, 0]
        );

        // The exact largest subnormal is adjacent to the minimum normal.
        let largest_subnormal = finite(-16383, u64::MAX - 1, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(largest_subnormal, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0]
        );
        // The exact halfway point rounds to minimum normal (ties to even).
        let halfway_to_normal = finite(-16383, u64::MAX, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(halfway_to_normal, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]
        );

        let min_subnormal = finite(-16445, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(min_subnormal, one, Nearest)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let half_min_subnormal = finite(-16446, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(half_min_subnormal, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0; 10]
        );
        let negative_half = finite(-16446, 1_u64 << 63, true, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(negative_half, one, Zero)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );
        assert_eq!(
            divide_finite_to_binary80(half_min_subnormal, one, Up)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        assert_eq!(
            divide_finite_to_binary80(negative_half, one, Down)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );
        let above_half = finite(
            -16446,
            0xc000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        assert_eq!(
            divide_finite_to_binary80(above_half, one, Nearest)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );
        let below_half = finite(-16447, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(below_half, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0; 10]
        );
        assert_eq!(
            divide_finite_to_binary80(below_half, one, Up)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );

        let max_finite = finite(16383, u64::MAX, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(max_finite, one, Nearest)
                .unwrap()
                .to_bytes(),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f]
        );
        let above_finite_range = finite(16384, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(
            divide_finite_to_binary80(above_finite_range, one, Nearest),
            None
        );

        for (lhs_sign, rhs_sign) in [(false, false), (false, true), (true, false), (true, true)] {
            let lhs = finite(0, 1_u64 << 63, lhs_sign, X87Binary80Class::Normal);
            let rhs = finite(1, 1_u64 << 63, rhs_sign, X87Binary80Class::Normal);
            let result = divide_finite_to_binary80(lhs, rhs, Nearest).unwrap();
            assert_eq!(result.sign(), lhs_sign ^ rhs_sign);
        }

        let positive_zero = finite(0, 0, false, X87Binary80Class::Zero);
        let negative_zero = finite(0, 0, true, X87Binary80Class::Zero);
        assert_eq!(divide_finite_to_binary80(positive_zero, one, Nearest), None);
        assert_eq!(divide_finite_to_binary80(one, negative_zero, Nearest), None);
        assert_eq!(
            divide_finite_to_binary80(negative_zero, positive_zero, Nearest),
            None
        );
    }

    #[test]
    fn finite_division_composition_uses_original_tail_for_subnormal_rounding() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let lhs = finite(
            -16382,
            0x8000_0000_0000_0002,
            false,
            X87Binary80Class::Normal,
        );
        let rhs = finite(0, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
        let lhs_before = lhs;
        let rhs_before = rhs;
        let quotient = divide_finite_significands(lhs, rhs).unwrap();
        assert_eq!(quotient.exponent(), -16383);
        assert_eq!(quotient.quotient(), 0xaaaa_aaaa_aaaa_aaad);
        assert_eq!(quotient.remainder(), 0x4000_0000_0000_0000);
        assert_eq!(quotient.denominator(), 0xc000_0000_0000_0000);

        let result = divide_finite_to_binary80(lhs, rhs, Nearest).unwrap();
        assert_eq!(
            result.to_bytes(),
            [0x57, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55, 0, 0]
        );
        assert_eq!((lhs, rhs), (lhs_before, rhs_before));
        assert_eq!(lhs.source_class(), X87Binary80Class::Normal);
        assert_eq!(rhs.source_class(), X87Binary80Class::Normal);
    }

    #[test]
    fn finite_division_composition_preserves_raw_finite_provenance() {
        let raw_subnormal = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let raw_pseudo = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80]);
        let lhs = raw_subnormal.normalize_finite().unwrap();
        let rhs = raw_pseudo.normalize_finite().unwrap();
        let before = (raw_subnormal, raw_pseudo, lhs, rhs);

        let result =
            divide_finite_to_binary80(lhs, rhs, RationalRoundingMode::NearestTiesEven).unwrap();
        assert_eq!(result.sign(), true);
        assert_eq!((raw_subnormal, raw_pseudo, lhs, rhs), before);
        assert_eq!(lhs.source_class(), X87Binary80Class::Subnormal);
        assert_eq!(rhs.source_class(), X87Binary80Class::PseudoDenormal);
    }

    #[test]
    fn normal_significand_packing_composes_with_one_third_rounding() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let numerator = finite(-1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let denominator = finite(0, 0xc000_0000_0000_0000, false, X87Binary80Class::Normal);
        let quotient = divide_finite_significands(numerator, denominator).unwrap();
        let cases = [
            (
                24,
                0x00aa_aaab_u128,
                [0x00, 0x00, 0x00, 0x00, 0x00, 0xab, 0xaa, 0xaa, 0xfd, 0x3f],
            ),
            (
                53,
                0x0015_5555_5555_5555,
                [0x00, 0xa8, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xfd, 0x3f],
            ),
            (
                64,
                0xaaaa_aaaa_aaaa_aaab,
                [0xab, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xaa, 0xfd, 0x3f],
            ),
        ];
        for (precision, magnitude, expected) in cases {
            let rounded = round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                precision,
                Nearest,
            )
            .unwrap();
            assert_eq!(rounded, magnitude);
            assert_eq!(
                pack_normal_rounded_significand(
                    quotient.sign(),
                    quotient.exponent(),
                    rounded,
                    precision,
                )
                .unwrap()
                .to_bytes(),
                expected
            );
        }

        let negative_numerator = finite(-1, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal);
        let negative_quotient =
            divide_finite_significands(negative_numerator, denominator).unwrap();
        for (mode, magnitude) in [
            (Nearest, 0x00aa_aaab_u128),
            (Zero, 0x00aa_aaaa),
            (Up, 0x00aa_aaaa),
            (Down, 0x00aa_aaab),
        ] {
            let rounded = round_quotient_to_precision(
                negative_quotient.sign(),
                negative_quotient.quotient(),
                negative_quotient.remainder(),
                negative_quotient.denominator(),
                24,
                mode,
            )
            .unwrap();
            assert_eq!(rounded, magnitude);
            let packed = pack_normal_rounded_significand(
                negative_quotient.sign(),
                negative_quotient.exponent(),
                rounded,
                24,
            )
            .unwrap();
            assert_eq!(packed.sign(), true);
            assert_eq!(packed.exponent(), 0x3ffd);
            assert_eq!(packed.classify(), X87Binary80Class::Normal);
        }
    }

    #[test]
    fn normal_significand_packing_rejects_out_of_slice_inputs_and_exponents() {
        for precision in [0, 65, u32::MAX] {
            assert_eq!(
                pack_normal_rounded_significand(false, 0, 1, precision),
                None
            );
        }
        for magnitude in [0, 1, 5] {
            assert_eq!(
                pack_normal_rounded_significand(false, 0, magnitude, 2),
                None
            );
        }
        assert_eq!(
            pack_normal_rounded_significand(false, 0, (1_u128 << 24) + 1, 24),
            None
        );
        for exponent in [i32::MIN, -16383, 16384, i32::MAX] {
            assert_eq!(
                pack_normal_rounded_significand(false, exponent, 1_u128 << 63, 64),
                None
            );
        }
        assert_eq!(
            pack_normal_rounded_significand(false, i32::MAX, 1_u128 << 64, 64),
            None
        );
        assert_eq!(
            pack_normal_rounded_significand(false, 16383, 1_u128 << 64, 64),
            None
        );
    }

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
            instruction_selector: None,
            data_selector: None,
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
    fn finite_scale_exact_powers_and_low_significand_bit_match_literal_encodings() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let high = 0x8000_0000_0000_0000;
        let exact = [
            (0, false, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]),
            (3, false, [0, 0, 0, 0, 0, 0, 0, 0x80, 0x02, 0x40]),
            (-3, false, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfc, 0x3f]),
            (0, true, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0xbf]),
            (3, true, [0, 0, 0, 0, 0, 0, 0, 0x80, 0x02, 0xc0]),
            (-3, true, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfc, 0xbf]),
        ];
        for (power, sign, expected) in exact {
            let value = finite(0, high, sign, X87Binary80Class::Normal);
            for mode in [Nearest, Zero, Up, Down] {
                assert_eq!(
                    scale_finite_to_binary80(value, power, mode)
                        .unwrap()
                        .to_bytes(),
                    expected
                );
            }
            assert_eq!(value, finite(0, high, sign, X87Binary80Class::Normal));
        }

        let low_bit = finite(0, 0x8000_0000_0000_0001, false, X87Binary80Class::Normal);
        assert_eq!(
            scale_finite_to_binary80(low_bit, 1, Nearest)
                .unwrap()
                .to_bytes(),
            [0x01, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x40]
        );
        let negative_low_bit = finite(0, 0x8000_0000_0000_0001, true, X87Binary80Class::Normal);
        assert_eq!(
            scale_finite_to_binary80(negative_low_bit, 1, Nearest)
                .unwrap()
                .to_bytes(),
            [0x01, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0]
        );
    }

    #[test]
    fn finite_scale_normal_subnormal_and_provenance_boundaries_match_literals() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let min_normal = finite(
            -16382,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        assert_eq!(
            scale_finite_to_binary80(min_normal, -1, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]
        );
        let negative_min_normal = finite(
            -16382,
            0x8000_0000_0000_0000,
            true,
            X87Binary80Class::Normal,
        );
        assert_eq!(
            scale_finite_to_binary80(negative_min_normal, -1, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0x80]
        );

        let min_subnormal = finite(
            -16445,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            scale_finite_to_binary80(min_subnormal, 63, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]
        );
        let max_subnormal = finite(
            -16383,
            0xffff_ffff_ffff_fffe,
            false,
            X87Binary80Class::Subnormal,
        );
        assert_eq!(
            scale_finite_to_binary80(max_subnormal, 1, Nearest)
                .unwrap()
                .to_bytes(),
            [0xfe, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 1, 0]
        );

        let raw_pseudo = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        let pseudo = raw_pseudo.normalize_finite().unwrap();
        assert_eq!(
            scale_finite_to_binary80(pseudo, 0, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]
        );
        assert_eq!(pseudo.source_class(), X87Binary80Class::PseudoDenormal);
        assert_eq!(raw_pseudo.to_bytes(), [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
    }

    #[test]
    fn finite_scale_underflow_keeps_sign_and_rounds_all_modes_once() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let vectors = [
            (
                false,
                -1,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                true,
                -1,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
            (
                false,
                -2,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                true,
                -2,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
        ];
        for (sign, power, nearest, zero, up, down) in vectors {
            let value = finite(
                -16445,
                0x8000_0000_0000_0000,
                sign,
                X87Binary80Class::Subnormal,
            );
            for (mode, expected) in [(Nearest, nearest), (Zero, zero), (Up, up), (Down, down)] {
                assert_eq!(
                    scale_finite_to_binary80(value, power, mode)
                        .unwrap()
                        .to_bytes(),
                    expected,
                    "sign={sign}, power={power}, mode={mode:?}"
                );
            }
        }
    }

    #[test]
    fn finite_scale_subnormal_half_ties_and_negative_directed_results_are_literal() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let vectors = [
            (
                3,
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
            (
                5,
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [2, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
            (
                7,
                [4, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [4, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [4, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [3, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [4, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
        ];
        for (
            raw_significand,
            pos_nearest,
            pos_zero,
            pos_up,
            pos_down,
            neg_nearest,
            neg_zero,
            neg_up,
            neg_down,
        ) in vectors
        {
            for (sign, expected) in [
                (false, [pos_nearest, pos_zero, pos_up, pos_down]),
                (true, [neg_nearest, neg_zero, neg_up, neg_down]),
            ] {
                let value = raw_finite(0, raw_significand, sign)
                    .normalize_finite()
                    .unwrap();
                for (mode, expected) in [
                    (Nearest, expected[0]),
                    (Zero, expected[1]),
                    (Up, expected[2]),
                    (Down, expected[3]),
                ] {
                    assert_eq!(
                        scale_finite_to_binary80(value, -1, mode)
                            .unwrap()
                            .to_bytes(),
                        expected,
                        "sign={sign}, raw_significand={raw_significand}, mode={mode:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn finite_scale_minimum_normal_predecessor_boundary_and_finite_range_are_literal() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let predecessor = finite(
            -16383,
            0xffff_ffff_ffff_fffe,
            false,
            X87Binary80Class::Subnormal,
        );
        for (mode, expected) in [
            (Nearest, [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]),
            (Zero, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0, 0]),
            (Up, [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]),
            (Down, [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0, 0]),
        ] {
            assert_eq!(
                scale_finite_to_binary80(predecessor, -1, mode)
                    .unwrap()
                    .to_bytes(),
                expected
            );
        }
        let negative_predecessor = finite(
            -16383,
            0xffff_ffff_ffff_fffe,
            true,
            X87Binary80Class::Subnormal,
        );
        for (mode, expected) in [
            (Nearest, [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0x80]),
            (
                Zero,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0, 0x80],
            ),
            (
                Up,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f, 0, 0x80],
            ),
            (Down, [0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0x80]),
        ] {
            assert_eq!(
                scale_finite_to_binary80(negative_predecessor, -1, mode)
                    .unwrap()
                    .to_bytes(),
                expected
            );
        }

        // This exact midpoint lies between the largest subnormal and minimum
        // normal after scaling; nearest-even and directed-away modes carry.
        for (sign, expected) in [
            (
                false,
                [
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0],
                ],
            ),
            (
                true,
                [
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0x80],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0x80],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0x80],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0x80],
                ],
            ),
        ] {
            let value = finite(-16382, u64::MAX, sign, X87Binary80Class::Normal);
            for (mode, expected) in [
                (Nearest, expected[0]),
                (Zero, expected[1]),
                (Up, expected[2]),
                (Down, expected[3]),
            ] {
                assert_eq!(
                    scale_finite_to_binary80(value, -1, mode)
                        .unwrap()
                        .to_bytes(),
                    expected,
                    "sign={sign}, mode={mode:?}"
                );
            }
        }

        let max_finite = finite(16383, u64::MAX, false, X87Binary80Class::Normal);
        let negative_max_finite = finite(16383, u64::MAX, true, X87Binary80Class::Normal);
        for (value, expected) in [
            (
                max_finite,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f],
            ),
            (
                negative_max_finite,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff],
            ),
        ] {
            for mode in [Nearest, Zero, Up, Down] {
                assert_eq!(
                    scale_finite_to_binary80(value, 0, mode).unwrap().to_bytes(),
                    expected
                );
            }
        }
        for sign in [false, true] {
            let value = finite(16383, u64::MAX, sign, X87Binary80Class::Normal);
            for mode in [Nearest, Zero, Up, Down] {
                assert_eq!(scale_finite_to_binary80(value, 1, mode), None);
            }
        }
    }

    #[test]
    fn finite_scale_checks_exponent_overflow_validation_and_input_immutability() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let min = finite(-1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let max = finite(1, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        for mode in [Nearest, Zero, Up, Down] {
            assert_eq!(scale_finite_to_binary80(min, i32::MIN, mode), None);
            assert_eq!(scale_finite_to_binary80(max, i32::MAX, mode), None);
        }

        let tiny_positive = finite(0, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal);
        let tiny_negative = finite(0, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal);
        for (value, mode, expected) in [
            (tiny_positive, Nearest, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (tiny_positive, Zero, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (tiny_positive, Up, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (tiny_positive, Down, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (tiny_negative, Nearest, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (tiny_negative, Zero, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (tiny_negative, Up, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (tiny_negative, Down, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
        ] {
            assert_eq!(
                scale_finite_to_binary80(value, i32::MIN, mode)
                    .unwrap()
                    .to_bytes(),
                expected
            );
        }

        let zero = finite(0, 0, false, X87Binary80Class::Zero);
        let malformed_significand =
            finite(0, 0x7fff_ffff_ffff_ffff, false, X87Binary80Class::Normal);
        let malformed_class = finite(0, 0x8000_0000_0000_0000, false, X87Binary80Class::Infinity);
        for mode in [Nearest, Zero, Up, Down] {
            assert_eq!(scale_finite_to_binary80(zero, 0, mode), None);
            assert_eq!(
                scale_finite_to_binary80(malformed_significand, 0, mode),
                None
            );
            assert_eq!(scale_finite_to_binary80(malformed_class, 0, mode), None);
        }
        assert_eq!(
            tiny_positive,
            finite(0, 0x8000_0000_0000_0000, false, X87Binary80Class::Normal)
        );
        assert_eq!(
            tiny_negative,
            finite(0, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal)
        );
    }

    fn raw_finite(exponent_field: u16, significand: u64, sign: bool) -> X87Binary80 {
        let mut bytes = [0; 10];
        bytes[..8].copy_from_slice(&significand.to_le_bytes());
        let sign_exponent = exponent_field | if sign { 0x8000 } else { 0 };
        bytes[8..].copy_from_slice(&sign_exponent.to_le_bytes());
        X87Binary80::from_bytes(bytes)
    }

    #[test]
    fn finite_sqrt_matches_literal_binary80_rounding_vectors() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases = [
            (
                0x8000_0000_0000_0000,
                0,
                [
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                ],
            ),
            (
                0x9000_0000_0000_0000,
                3,
                [
                    [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40],
                    [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40],
                    [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40],
                    [0, 0, 0, 0, 0, 0, 0, 0xc0, 0x00, 0x40],
                ],
            ),
            (
                0x8000_0000_0000_0000,
                1,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x85, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                ],
            ),
            (
                0x8000_0000_0000_0001,
                0,
                [
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [1, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f],
                ],
            ),
            (
                0xffff_ffff_ffff_ffff,
                0,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x83, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                    [0x83, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xff, 0x3f],
                ],
            ),
            (
                0xffff_ffff_ffff_ffff,
                1,
                [
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f],
                    [0, 0, 0, 0, 0, 0, 0, 0x80, 0x00, 0x40],
                    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x3f],
                ],
            ),
        ];

        for (significand, exponent, expected_by_mode) in cases {
            let value = finite(exponent, significand, false, X87Binary80Class::Normal);
            for (index, (mode, expected)) in modes.into_iter().zip(expected_by_mode).enumerate() {
                assert_eq!(
                    sqrt_finite_to_binary80(value, mode).unwrap().to_bytes(),
                    expected,
                    "S={significand:#018x}, e={exponent}, mode index={index}"
                );
            }
        }
    }

    #[test]
    fn finite_sqrt_covers_exponent_boundaries_provenance_and_rejections() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let boundary_vectors = [
            (
                -1,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x3f],
                    [0x85, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x3f],
                ],
            ),
            (
                -3,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfd, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfd, 0x3f],
                    [0x85, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfd, 0x3f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfd, 0x3f],
                ],
            ),
            (
                -16445,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xe0, 0x1f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xe0, 0x1f],
                    [0x85, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xe0, 0x1f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xe0, 0x1f],
                ],
            ),
            (
                16383,
                [
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x5f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x5f],
                    [0x85, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x5f],
                    [0x84, 0x64, 0xde, 0xf9, 0x33, 0xf3, 0x04, 0xb5, 0xfe, 0x5f],
                ],
            ),
        ];
        for (exponent, expected_by_mode) in boundary_vectors {
            let value = finite(
                exponent,
                0x8000_0000_0000_0000,
                false,
                X87Binary80Class::Subnormal,
            );
            for (mode, expected) in modes.into_iter().zip(expected_by_mode) {
                let result = sqrt_finite_to_binary80(value, mode).unwrap();
                assert_eq!(result.classify(), X87Binary80Class::Normal);
                assert_eq!(
                    result.to_bytes(),
                    expected,
                    "exponent={exponent}, mode={mode:?}"
                );
            }
        }

        let pseudo = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0])
            .normalize_finite()
            .unwrap();
        let normal = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0])
            .normalize_finite()
            .unwrap();
        for mode in modes {
            assert_eq!(
                sqrt_finite_to_binary80(pseudo, mode).unwrap().to_bytes(),
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x20]
            );
            assert_eq!(
                sqrt_finite_to_binary80(pseudo, mode),
                sqrt_finite_to_binary80(normal, mode)
            );
        }

        let positive_zero = finite(0, 0, false, X87Binary80Class::Zero);
        let negative_zero = finite(0, 0, true, X87Binary80Class::Zero);
        let negative = finite(0, 0x8000_0000_0000_0000, true, X87Binary80Class::Normal);
        let unnormalized = finite(0, 0x7fff_ffff_ffff_ffff, false, X87Binary80Class::Normal);
        for mode in modes {
            assert_eq!(sqrt_finite_to_binary80(positive_zero, mode), None);
            assert_eq!(sqrt_finite_to_binary80(negative_zero, mode), None);
            assert_eq!(sqrt_finite_to_binary80(negative, mode), None);
            assert_eq!(sqrt_finite_to_binary80(unnormalized, mode), None);
        }
    }

    #[test]
    fn finite_truncated_remainder_matches_literal_signed_and_gap_vectors() {
        let normal = X87Binary80Class::Normal;
        let high = 0x8000_0000_0000_0000;

        // 5.5 % 2 is exactly 1.5 in units of 2^(rhs exponent - 63).
        for (lhs_sign, rhs_sign, expected) in [
            (false, false, 0x6000_0000_0000_0000_i128),
            (false, true, 0x6000_0000_0000_0000_i128),
            (true, false, -0x6000_0000_0000_0000_i128),
            (true, true, -0x6000_0000_0000_0000_i128),
        ] {
            let lhs = finite(2, 0xb000_0000_0000_0000, lhs_sign, normal);
            let rhs = finite(1, high, rhs_sign, normal);
            let before = (lhs, rhs);
            assert_eq!(remainder_finite_trunc(lhs, rhs), Some((1, expected)));
            assert_eq!((lhs, rhs), before);
        }

        // Exact multiples have a mathematical zero coefficient regardless of
        // operand signs; architectural zero sign is outside this helper.
        for lhs_sign in [false, true] {
            for rhs_sign in [false, true] {
                assert_eq!(
                    remainder_finite_trunc(
                        finite(2, 0xc000_0000_0000_0000, lhs_sign, normal),
                        finite(1, high, rhs_sign, normal),
                    ),
                    Some((1, 0))
                );
            }
        }

        // A smaller exponent returns the signed left coefficient unchanged.
        assert_eq!(
            remainder_finite_trunc(
                finite(0, high, true, normal),
                finite(1, high, false, normal),
            ),
            Some((0, -0x8000_0000_0000_0000_i128))
        );

        let max = u64::MAX;
        let edge_vectors = [
            (0, 0x8000_0000_0000_0001, high, 1),
            (1, high, max, 1),
            (63, max, 0x8000_0000_0000_0001, 3),
            (64, max, 0x8000_0000_0000_0001, 6),
            (64, max, max, 0),
        ];
        for (gap, lhs_significand, rhs_significand, expected) in edge_vectors {
            let lhs = finite(gap, lhs_significand, false, normal);
            let rhs = finite(0, rhs_significand, false, normal);
            assert_eq!(
                remainder_finite_trunc(lhs, rhs),
                Some((0, expected)),
                "gap={gap}, lhs={lhs_significand:#018x}, rhs={rhs_significand:#018x}"
            );
        }
        assert_eq!(
            remainder_finite_trunc(
                finite(65, high, false, normal),
                finite(0, high, false, normal)
            ),
            None
        );
    }

    #[test]
    fn finite_truncated_remainder_covers_provenance_boundaries_and_rejections() {
        let high = 0x8000_0000_0000_0000;
        let normal = X87Binary80Class::Normal;

        // Minimum normal modulo maximum subnormal is the minimum subnormal.
        assert_eq!(
            remainder_finite_trunc(
                finite(-16382, high, false, normal),
                finite(
                    -16383,
                    0xffff_ffff_ffff_fffe,
                    false,
                    X87Binary80Class::Subnormal
                ),
            ),
            Some((-16383, 2))
        );

        // Raw subnormal integers 7 % 3 == 1 after normalization.
        assert_eq!(
            remainder_finite_trunc(
                finite(
                    -16443,
                    0xe000_0000_0000_0000,
                    false,
                    X87Binary80Class::Subnormal,
                ),
                finite(
                    -16444,
                    0xc000_0000_0000_0000,
                    false,
                    X87Binary80Class::Subnormal,
                ),
            ),
            Some((-16444, 0x4000_0000_0000_0000_i128))
        );

        let pseudo_min = finite(-16382, high, true, X87Binary80Class::PseudoDenormal);
        let normal_min = finite(-16382, high, true, normal);
        let rhs = finite(-16383, 0xc000_0000_0000_0000, false, normal);
        assert_eq!(
            remainder_finite_trunc(pseudo_min, rhs),
            remainder_finite_trunc(normal_min, rhs)
        );

        // Widen before subtracting exponents: both i32 endpoints are safe.
        assert_eq!(
            remainder_finite_trunc(
                finite(i32::MIN, high, true, normal),
                finite(i32::MAX, high, false, normal),
            ),
            Some((i32::MIN, -0x8000_0000_0000_0000_i128))
        );
        assert_eq!(
            remainder_finite_trunc(
                finite(i32::MAX, high, false, normal),
                finite(i32::MIN, high, false, normal),
            ),
            None
        );

        let valid = finite(0, high, false, normal);
        let invalid = [
            finite(0, 0, false, normal),
            finite(0, high - 1, false, normal),
            finite(0, 0, false, X87Binary80Class::Zero),
            finite(0, high, false, X87Binary80Class::Infinity),
            finite(0, high, false, X87Binary80Class::QuietNaN),
            finite(0, high, false, X87Binary80Class::Unnormal),
        ];
        for value in invalid {
            assert_eq!(remainder_finite_trunc(value, valid), None);
            assert_eq!(remainder_finite_trunc(valid, value), None);
        }
        assert_eq!(
            remainder_finite_trunc(valid, finite(0, 0, false, normal)),
            None
        );
    }

    #[test]
    fn finite_truncated_remainder_composes_to_literal_exact_binary80_values() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };
        let modes = [Nearest, Zero, Up, Down];
        let normal = X87Binary80Class::Normal;
        let high = 0x8000_0000_0000_0000;

        let (exponent, coefficient) = remainder_finite_trunc(
            finite(2, 0xb000_0000_0000_0000, false, normal),
            finite(1, high, true, normal),
        )
        .unwrap();
        assert_eq!((exponent, coefficient), (1, 0x6000_0000_0000_0000));
        for mode in modes {
            assert_eq!(
                round_pack_exact_coefficient(exponent, coefficient, mode)
                    .unwrap()
                    .to_bytes(),
                [0, 0, 0, 0, 0, 0, 0, 0xc0, 0xff, 0x3f]
            );
        }

        let (exponent, coefficient) = remainder_finite_trunc(
            finite(-16382, high, false, normal),
            finite(
                -16383,
                0xffff_ffff_ffff_fffe,
                false,
                X87Binary80Class::Subnormal,
            ),
        )
        .unwrap();
        assert_eq!((exponent, coefficient), (-16383, 2));
        for mode in modes {
            assert_eq!(
                round_pack_exact_coefficient(exponent, coefficient, mode)
                    .unwrap()
                    .to_bytes(),
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
            );
        }
    }

    #[test]
    fn finite_nearest_even_remainder_matches_literal_gap_matrix() {
        let normal = X87Binary80Class::Normal;
        let high = 0x8000_0000_0000_0000;
        let max = u64::MAX;
        let cases = [
            (
                0,
                0xc000_0000_0000_0000,
                high,
                0,
                -0x4000_0000_0000_0000_i128,
            ),
            (
                1,
                0xa000_0000_0000_0000,
                high,
                0,
                0x4000_0000_0000_0000_i128,
            ),
            (
                1,
                0xe000_0000_0000_0000,
                high,
                0,
                -0x4000_0000_0000_0000_i128,
            ),
            (-1, high, high, -1, 0x8000_0000_0000_0000_i128),
            (-1, high + 1, high, -1, 1 - 0x8000_0000_0000_0000_i128),
            (-1, high, high + 1, -1, 0x8000_0000_0000_0000_i128),
            (-2, max, high, -2, i128::from(max)),
            (63, high, high + 1, 0, 1),
            (64, high, high + 1, 0, 2),
            (64, max, high + 1, 0, 6),
            (64, max, max, 0, 0),
        ];
        for (gap, lhs_sig, rhs_sig, rhs_exp, expected) in cases {
            let lhs = finite(gap + rhs_exp, lhs_sig, false, normal);
            let rhs = finite(rhs_exp, rhs_sig, false, normal);
            let expected_exp = if gap < 0 {
                lhs.exponent()
            } else {
                rhs.exponent()
            };
            assert_eq!(
                remainder_finite_nearest_even(lhs, rhs),
                Some((expected_exp, expected)),
                "gap={gap}, lhs={lhs_sig:#018x}, rhs={rhs_sig:#018x}"
            );
        }
        assert_eq!(
            remainder_finite_nearest_even(
                finite(65, high, false, normal),
                finite(0, high, false, normal),
            ),
            None
        );

        // Exact 6 % 2 has zero regardless of either sign.
        for lhs_sign in [false, true] {
            for rhs_sign in [false, true] {
                assert_eq!(
                    remainder_finite_nearest_even(
                        finite(2, 0xc000_0000_0000_0000, lhs_sign, normal),
                        finite(1, high, rhs_sign, normal),
                    ),
                    Some((1, 0))
                );
            }
        }
    }

    #[test]
    fn finite_nearest_even_remainder_handles_signs_ties_and_normalized_subnormals() {
        let normal = X87Binary80Class::Normal;
        let subnormal = X87Binary80Class::Subnormal;
        let high = 0x8000_0000_0000_0000;

        // Below, exactly at, and above one half for an exponent gap of -1.
        for (lhs_sig, rhs_sig, expected) in [
            (high, high + 1, i128::from(high)),
            (high, high, i128::from(high)),
            (high + 1, high, 1 - i128::from(high)),
        ] {
            assert_eq!(
                remainder_finite_nearest_even(
                    finite(0, lhs_sig, false, normal),
                    finite(1, rhs_sig, false, normal),
                ),
                Some((0, expected))
            );
        }

        // The midpoint q=1 is odd and rounds away; q=2 is even and stays.
        assert_eq!(
            remainder_finite_nearest_even(
                finite(0, 0xc000_0000_0000_0000, false, normal),
                finite(0, high, false, normal),
            ),
            Some((0, -0x4000_0000_0000_0000))
        );
        assert_eq!(
            remainder_finite_nearest_even(
                finite(1, 0xa000_0000_0000_0000, false, normal),
                finite(0, high, false, normal),
            ),
            Some((0, 0x4000_0000_0000_0000))
        );

        // For a -1 coefficient at the nearest remainder, only lhs sign applies.
        for (lhs_sign, rhs_sign, expected) in [
            (false, false, -0x4000_0000_0000_0000_i128),
            (false, true, -0x4000_0000_0000_0000_i128),
            (true, false, 0x4000_0000_0000_0000_i128),
            (true, true, 0x4000_0000_0000_0000_i128),
        ] {
            assert_eq!(
                remainder_finite_nearest_even(
                    finite(0, 0xc000_0000_0000_0000, lhs_sign, normal),
                    finite(0, high, rhs_sign, normal),
                ),
                Some((0, expected))
            );
        }

        // Minimum normal % maximum subnormal is minimum subnormal; raw 7 % 4
        // subnormal units rounds the quotient 1.75 to 2 and leaves -1 unit.
        assert_eq!(
            remainder_finite_nearest_even(
                finite(-16382, high, false, normal),
                finite(-16383, 0xffff_ffff_ffff_fffe, false, subnormal),
            ),
            Some((-16383, 2))
        );
        assert_eq!(
            remainder_finite_nearest_even(
                finite(-16443, 0xe000_0000_0000_0000, false, subnormal),
                finite(-16443, high, false, subnormal),
            ),
            Some((-16443, -0x2000_0000_0000_0000))
        );

        let pseudo = finite(-16382, high, true, X87Binary80Class::PseudoDenormal);
        let canonical = finite(-16382, high, true, normal);
        let divisor = finite(-16383, 0xc000_0000_0000_0000, false, normal);
        assert_eq!(
            remainder_finite_nearest_even(pseudo, divisor),
            remainder_finite_nearest_even(canonical, divisor)
        );
    }

    #[test]
    fn finite_nearest_even_remainder_rejects_invalid_inputs_and_preserves_them() {
        let normal = X87Binary80Class::Normal;
        let high = 0x8000_0000_0000_0000;
        let valid = finite(0, high, false, normal);
        let invalid = [
            finite(0, 0, false, normal),
            finite(0, high - 1, false, normal),
            finite(0, 0, false, X87Binary80Class::Zero),
            finite(0, high, false, X87Binary80Class::Infinity),
            finite(0, high, false, X87Binary80Class::QuietNaN),
            finite(0, high, false, X87Binary80Class::Unnormal),
        ];
        for value in invalid {
            assert_eq!(remainder_finite_nearest_even(value, valid), None);
            assert_eq!(remainder_finite_nearest_even(valid, value), None);
        }
        assert_eq!(
            remainder_finite_nearest_even(valid, finite(0, 0, false, normal)),
            None
        );

        // Subtract exponents only after widening, and preserve the i32-extreme
        // lhs unchanged when its exponent gap is far below the divisor.
        assert_eq!(
            remainder_finite_nearest_even(
                finite(i32::MIN, high, true, normal),
                finite(i32::MAX, high, false, normal),
            ),
            Some((i32::MIN, -i128::from(high)))
        );
        assert_eq!(
            remainder_finite_nearest_even(
                finite(i32::MAX, high, false, normal),
                finite(i32::MIN, high, false, normal),
            ),
            None
        );

        let lhs = finite(0, 0xc000_0000_0000_0000, true, normal);
        let rhs = finite(0, high, false, normal);
        let before = (lhs, rhs);
        let _ = remainder_finite_nearest_even(lhs, rhs);
        assert_eq!((lhs, rhs), before);
    }

    #[test]
    fn finite_nearest_even_remainder_composes_to_literal_binary80_outputs() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };
        let modes = [Nearest, Zero, Up, Down];
        let normal = X87Binary80Class::Normal;
        let high = 0x8000_0000_0000_0000;

        let (exponent, coefficient) = remainder_finite_nearest_even(
            finite(0, 0xc000_0000_0000_0000, false, normal),
            finite(0, high, false, normal),
        )
        .unwrap();
        assert_eq!((exponent, coefficient), (0, -0x4000_0000_0000_0000));
        for mode in modes {
            assert_eq!(
                round_pack_exact_coefficient(exponent, coefficient, mode)
                    .unwrap()
                    .to_bytes(),
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0xfe, 0xbf]
            );
        }

        let (exponent, coefficient) = remainder_finite_nearest_even(
            finite(-16382, high, true, normal),
            finite(
                -16383,
                0xffff_ffff_ffff_fffe,
                false,
                X87Binary80Class::Subnormal,
            ),
        )
        .unwrap();
        assert_eq!((exponent, coefficient), (-16383, -2));
        for mode in modes {
            assert_eq!(
                round_pack_exact_coefficient(exponent, coefficient, mode)
                    .unwrap()
                    .to_bytes(),
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
            );
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
    fn unequal_exponent_sum_is_exact_at_the_lower_exponent_and_bounded_by_gap() {
        let make = |exponent, significand, sign| {
            finite(exponent, significand, sign, X87Binary80Class::Normal)
        };
        let one_e0 = make(0, 0x8000_0000_0000_0000, false);
        let two_e1 = make(1, 0x8000_0000_0000_0000, false);
        let negative_two_e1 = make(1, 0x8000_0000_0000_0000, true);
        let negative_one_e0 = make(0, 0x8000_0000_0000_0000, true);

        assert_eq!(
            add_finite_unequal_exponents(two_e1, one_e0),
            Some((0, 0x1_8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(one_e0, two_e1),
            Some((0, 0x1_8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(negative_two_e1, one_e0),
            Some((0, -0x8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(one_e0, negative_two_e1),
            Some((0, -0x8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(two_e1, negative_one_e0),
            Some((0, 0x8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(negative_one_e0, two_e1),
            Some((0, 0x8000_0000_0000_0000))
        );
        assert_eq!(
            add_finite_unequal_exponents(negative_two_e1, negative_one_e0),
            Some((0, -0x1_8000_0000_0000_0000))
        );

        let high_gap = make(62, 0x8000_0000_0000_0000, false);
        let low_gap = make(0, 0x8000_0000_0000_0000, false);
        assert_eq!(
            add_finite_unequal_exponents(high_gap, low_gap),
            Some((0, (1_i128 << 125) + (1_i128 << 63)))
        );
        assert_eq!(
            add_finite_unequal_exponents(make(63, 0x8000_0000_0000_0000, false), low_gap),
            None
        );
        assert_eq!(add_finite_unequal_exponents(two_e1, two_e1), None);
        assert_eq!(
            add_finite_unequal_exponents(one_e0, finite(0, 0, false, X87Binary80Class::Zero)),
            None
        );
    }

    #[test]
    fn unequal_exponent_addition_composes_once_with_rounding_and_binary80_packing() {
        let make = |exponent, significand, sign| {
            finite(exponent, significand, sign, X87Binary80Class::Normal)
        };
        let one = make(0, 0x8000_0000_0000_0000, false);
        let two = make(1, 0x8000_0000_0000_0000, false);
        let negative_two = make(1, 0x8000_0000_0000_0000, true);
        let zero = finite(0, 0, false, X87Binary80Class::Zero);
        let before = (one, two, negative_two, zero);

        assert_eq!(
            add_finite_to_binary80(two, one, RationalRoundingMode::NearestTiesEven)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0xc0, 0, 0x40]
        );
        assert_eq!(
            add_finite_to_binary80(negative_two, one, RationalRoundingMode::NearestTiesEven)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0xbf]
        );
        assert_eq!(
            add_finite_to_binary80(zero, one, RationalRoundingMode::NearestTiesEven)
                .unwrap()
                .to_bytes(),
            raw_finite(0x3fff, 0x8000_0000_0000_0000, false).to_bytes()
        );
        assert_eq!(
            add_finite_to_binary80(one, zero, RationalRoundingMode::NearestTiesEven)
                .unwrap()
                .to_bytes(),
            raw_finite(0x3fff, 0x8000_0000_0000_0000, false).to_bytes()
        );
        assert_eq!(
            add_finite_to_binary80(zero, zero, RationalRoundingMode::NearestTiesEven),
            None
        );
        assert_eq!(
            add_finite_to_binary80(
                one,
                make(0, 0x8000_0000_0000_0000, true),
                RationalRoundingMode::NearestTiesEven
            ),
            None
        );

        let high = make(62, 0x8000_0000_0000_0000, false);
        let low = make(0, 0x8000_0000_0000_0001, false);
        let expected_sig = |mode| match mode {
            RationalRoundingMode::TowardPositive => 0x8000_0000_0000_0003,
            RationalRoundingMode::NearestTiesEven
            | RationalRoundingMode::TowardZero
            | RationalRoundingMode::TowardNegative => 0x8000_0000_0000_0002,
        };
        for mode in [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ] {
            assert_eq!(
                add_finite_to_binary80(high, low, mode).unwrap().to_bytes(),
                raw_finite(0x403d, expected_sig(mode), false).to_bytes()
            );
        }
        let negative_high = make(62, 0x8000_0000_0000_0000, true);
        let negative_low = make(0, 0x8000_0000_0000_0001, true);
        assert_eq!(
            add_finite_to_binary80(
                negative_high,
                negative_low,
                RationalRoundingMode::TowardNegative
            )
            .unwrap()
            .to_bytes(),
            raw_finite(0x403d, 0x8000_0000_0000_0003, true).to_bytes()
        );

        let max_finite = make(16383, u64::MAX, false);
        let tiny_positive = make(16321, 0x8000_0000_0000_0000, false);
        assert_eq!(
            add_finite_to_binary80(
                max_finite,
                tiny_positive,
                RationalRoundingMode::NearestTiesEven
            ),
            None
        );
        assert_eq!((one, two, negative_two, zero), before);
    }

    #[test]
    fn aligned_addition_packs_exact_gaps_ties_boundaries_and_overflow() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let make = |exponent, significand, sign| {
            finite(exponent, significand, sign, X87Binary80Class::Normal)
        };
        let sig = 1_u64 << 63;

        let one = make(0, sig, false);
        let high62 = make(62, sig, false);
        let old_coeff = add_finite_unequal_exponents(high62, one).unwrap();
        assert_eq!(
            add_finite_to_binary80(high62, one, Nearest),
            round_pack_exact_coefficient(old_coeff.0, old_coeff.1, Nearest)
        );

        let high63 = make(63, sig, false);
        for (lhs, rhs) in [(high63, one), (one, high63)] {
            assert_eq!(
                add_finite_to_binary80(lhs, rhs, Nearest),
                Some(raw_finite(0x403e, sig + 1, false)),
                "gap 63 contributes exactly one ulp"
            );
        }

        // Gap 64 is a half ulp. Exact ties use the retained parity; directed
        // rounding increments only when its direction is away from zero. An
        // odd low significand adds a sticky tail above the exact tie.
        for high_significand in [sig, sig + 1] {
            for low_significand in [sig, sig + 1] {
                for sign in [false, true] {
                    for mode in [Nearest, Zero, Up, Down] {
                        let high = make(64, high_significand, sign);
                        let low = make(0, low_significand, sign);
                        let above_tie = low_significand & 1 != 0;
                        let increment = match mode {
                            Nearest => above_tie || high_significand & 1 != 0,
                            Zero => false,
                            Up => !sign,
                            Down => sign,
                        };
                        let expected = Some(raw_finite(
                            0x403f,
                            high_significand + u64::from(increment),
                            sign,
                        ));
                        assert_eq!(add_finite_to_binary80(high, low, mode), expected);
                        assert_eq!(add_finite_to_binary80(low, high, mode), expected);
                    }
                }
            }
        }

        // 1 - 2^-64 is exactly the predecessor of one.
        let negative_tiny = make(-64, sig, true);
        for (lhs, rhs) in [(one, negative_tiny), (negative_tiny, one)] {
            assert_eq!(
                add_finite_to_binary80(lhs, rhs, Nearest),
                Some(raw_finite(0x3ffe, u64::MAX, false))
            );
        }

        // Opposite-sign low terms make exact half-ulp values just below the
        // odd retained significand; parity and result sign remain observable.
        for sign in [false, true] {
            for mode in [Nearest, Zero, Up, Down] {
                let high = make(64, sig + 1, sign);
                let low = make(0, sig, !sign);
                let increment = match mode {
                    Nearest | Zero => false,
                    Up => !sign,
                    Down => sign,
                };
                let expected = Some(raw_finite(0x403f, sig + u64::from(increment), sign));
                assert_eq!(add_finite_to_binary80(high, low, mode), expected);
                assert_eq!(add_finite_to_binary80(low, high, mode), expected);
            }
        }

        let gap65 = make(65, sig, false);
        assert_eq!(
            add_finite_to_binary80(gap65, one, Nearest),
            Some(raw_finite(0x4040, sig, false))
        );
        assert_eq!(
            add_finite_to_binary80(one, gap65, Nearest),
            Some(raw_finite(0x4040, sig, false))
        );

        let maximum = make(16383, u64::MAX, false);
        assert_eq!(add_finite_to_binary80(maximum, maximum, Nearest), None);
        assert_eq!(add_finite_to_binary80(maximum, maximum, Zero), None);
        let checked_exponent_overflow_low = make(i32::MAX - 63, sig, false);
        let checked_exponent_overflow_high = make(i32::MAX, sig, false);
        assert_eq!(
            add_finite_to_binary80(
                checked_exponent_overflow_high,
                checked_exponent_overflow_low,
                Nearest
            ),
            None
        );

        let zero = finite(0, 0, true, X87Binary80Class::Zero);
        let negative_one = make(0, sig, true);
        let before = (
            one,
            high62,
            high63,
            negative_tiny,
            gap65,
            maximum,
            checked_exponent_overflow_low,
            checked_exponent_overflow_high,
            zero,
        );
        assert_eq!(
            add_finite_to_binary80(zero, one, Nearest),
            Some(raw_finite(0x3fff, sig, false))
        );
        assert_eq!(add_finite_to_binary80(one, negative_one, Nearest), None);
        assert_eq!(add_finite_to_binary80(zero, zero, Nearest), None);
        assert_eq!(
            (
                one,
                high62,
                high63,
                negative_tiny,
                gap65,
                maximum,
                checked_exponent_overflow_low,
                checked_exponent_overflow_high,
                zero,
            ),
            before,
            "inputs and source-class metadata are immutable"
        );
    }

    #[test]
    fn large_gap_addition_rounds_binary80_tail_categories_in_all_modes() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let make = |exponent, significand, sign| {
            finite(exponent, significand, sign, X87Binary80Class::Normal)
        };
        let sig = 1_u64 << 63;
        let one = make(0, sig, false);
        let predecessor = raw_finite(0x3ffe, u64::MAX, false);
        let negative_one = make(0, sig, true);

        let half_below_one = make(-65, sig, true);
        assert_eq!(
            add_finite_to_binary80(one, half_below_one, Nearest),
            Some(raw_finite(0x3fff, sig, false)),
            "1 - 2^-65 is an exact tie to even one"
        );
        let just_below_half = make(-65, sig + 1, true);
        assert_eq!(
            add_finite_to_binary80(one, just_below_half, Nearest),
            Some(predecessor),
            "1 - (2^-65 + 2^-128) is below the tie"
        );
        let below_half = make(-66, sig, true);
        assert_eq!(
            add_finite_to_binary80(one, below_half, Nearest),
            Some(raw_finite(0x3fff, sig, false)),
            "1 - 2^-66 is closer to one"
        );

        // Gap 65 is an exact path boundary: it is still rejected by the old
        // signed-coefficient API, while the composed helper uses its bridge.
        assert_eq!(add_finite_unequal_exponents(one, half_below_one), None);

        // Same-sign tails are below half. Check both signs, operand orders,
        // and every rounding direction.
        for sign in [false, true] {
            for mode in [Nearest, Zero, Up, Down] {
                let high = make(0, sig, sign);
                let low = make(-65, sig, sign);
                let increment = match mode {
                    Up => !sign,
                    Down => sign,
                    Nearest | Zero => false,
                };
                let expected = raw_finite(0x3fff, sig + u64::from(increment), sign);
                assert_eq!(add_finite_to_binary80(high, low, mode), Some(expected));
                assert_eq!(add_finite_to_binary80(low, high, mode), Some(expected));
            }
        }

        // Opposite signs with a dominant significand above 2^63 create a
        // greater-than-half tail after borrowing one retained unit.
        for sign in [false, true] {
            for mode in [Nearest, Zero, Up, Down] {
                let high = make(0, sig + 1, sign);
                let low = make(-65, sig, !sign);
                let increment = match mode {
                    Nearest => true,
                    Zero => false,
                    Up => !sign,
                    Down => sign,
                };
                let expected = raw_finite(0x3fff, sig + u64::from(increment), sign);
                assert_eq!(add_finite_to_binary80(high, low, mode), Some(expected));
                assert_eq!(add_finite_to_binary80(low, high, mode), Some(expected));
            }
        }

        // Opposite-sign tails at the leading significand exercise exact-half
        // (gap 65), below-half (gap 65, b > 2^63), and above-half (gap >= 66).
        for (gap, low_sig, nearest_increments) in [
            (65, sig, true),
            (65, sig + 1, false),
            (66, sig, true),
            (127, sig, true),
            (128, sig, true),
        ] {
            for sign in [false, true] {
                for mode in [Nearest, Zero, Up, Down] {
                    let high = make(0, sig, sign);
                    let low = make(-gap, low_sig, !sign);
                    let increment = match mode {
                        Nearest => nearest_increments,
                        Zero => false,
                        Up => !sign,
                        Down => sign,
                    };
                    let expected = if increment {
                        raw_finite(0x3fff, sig, sign)
                    } else {
                        raw_finite(0x3ffe, u64::MAX, sign)
                    };
                    assert_eq!(add_finite_to_binary80(high, low, mode), Some(expected));
                    assert_eq!(add_finite_to_binary80(low, high, mode), Some(expected));
                }
            }
        }

        let max_finite = make(16383, u64::MAX, false);
        let min_subnormal = finite(-16445, sig, false, X87Binary80Class::Subnormal);
        let negative_max = make(16383, u64::MAX, true);
        let negative_min_subnormal = finite(-16445, sig, true, X87Binary80Class::Subnormal);
        let max_raw = raw_finite(0x7ffe, u64::MAX, false);
        let previous_max_raw = raw_finite(0x7ffe, u64::MAX - 1, false);
        let negative_max_raw = raw_finite(0x7ffe, u64::MAX, true);
        let negative_previous_max_raw = raw_finite(0x7ffe, u64::MAX - 1, true);

        for (lhs, rhs, expected) in [
            (
                max_finite,
                min_subnormal,
                [Some(max_raw), Some(max_raw), None, Some(max_raw)],
            ),
            (
                max_finite,
                negative_min_subnormal,
                [
                    Some(max_raw),
                    Some(previous_max_raw),
                    Some(max_raw),
                    Some(previous_max_raw),
                ],
            ),
            (
                negative_max,
                min_subnormal,
                [
                    Some(negative_max_raw),
                    Some(negative_previous_max_raw),
                    Some(negative_previous_max_raw),
                    Some(negative_max_raw),
                ],
            ),
            (
                negative_max,
                negative_min_subnormal,
                [
                    Some(negative_max_raw),
                    Some(negative_max_raw),
                    Some(negative_max_raw),
                    None,
                ],
            ),
        ] {
            for (mode, expected) in [
                (Nearest, expected[0]),
                (Zero, expected[1]),
                (Up, expected[2]),
                (Down, expected[3]),
            ] {
                assert_eq!(add_finite_to_binary80(lhs, rhs, mode), expected);
                assert_eq!(add_finite_to_binary80(rhs, lhs, mode), expected);
            }
        }

        // Gap 64 remains on exact full-u128 alignment and keeps its earlier
        // one-round result. The pair is nonzero and same sign.
        let gap64 = make(-64, sig, false);
        assert_eq!(
            add_finite_to_binary80(one, gap64, Nearest),
            Some(raw_finite(0x3fff, sig, false))
        );

        let zero = finite(0, 0, true, X87Binary80Class::Zero);
        let before = (
            one,
            half_below_one,
            just_below_half,
            below_half,
            max_finite,
            min_subnormal,
            negative_max,
            negative_min_subnormal,
            zero,
        );
        assert_eq!(
            add_finite_to_binary80(zero, one, Nearest),
            Some(raw_finite(0x3fff, sig, false))
        );
        assert_eq!(add_finite_to_binary80(one, negative_one, Nearest), None);
        assert_eq!(add_finite_to_binary80(zero, zero, Nearest), None);
        assert_eq!(
            (
                one,
                half_below_one,
                just_below_half,
                below_half,
                max_finite,
                min_subnormal,
                negative_max,
                negative_min_subnormal,
                zero,
            ),
            before,
            "input values and class provenance remain unchanged"
        );
        assert_eq!(negative_one.sign(), true);
    }

    #[test]
    fn same_exponent_sum_composes_to_canonical_binary80() {
        let add = |lhs: X87Binary80, rhs: X87Binary80| {
            add_finite_same_exponent_to_binary80(
                lhs.normalize_finite()?,
                rhs.normalize_finite()?,
                RationalRoundingMode::NearestTiesEven,
            )
        };
        let one = raw_finite(0x3fff, 0x8000_0000_0000_0000, false);
        let negative_one = raw_finite(0x3fff, 0x8000_0000_0000_0000, true);
        assert_eq!(
            add(one, one).unwrap().to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x40]
        );
        assert_eq!(
            add(negative_one, negative_one).unwrap().to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0]
        );

        let min_normal = raw_finite(1, 0x8000_0000_0000_0000, false);
        let adjacent_min_normal = raw_finite(1, 0x8000_0000_0000_0001, false);
        let negative_adjacent_min_normal = raw_finite(1, 0x8000_0000_0000_0001, true);
        let minsub = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let negative_minsub = X87Binary80::from_bytes([1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]);
        assert_eq!(
            add(
                adjacent_min_normal,
                X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x80,])
            )
            .unwrap()
            .to_bytes(),
            minsub.to_bytes()
        );
        assert_eq!(
            add(negative_adjacent_min_normal, min_normal)
                .unwrap()
                .to_bytes(),
            negative_minsub.to_bytes()
        );

        let half_min_normal = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0]);
        assert_eq!(
            add(half_min_normal, half_min_normal).unwrap().to_bytes(),
            min_normal.to_bytes()
        );
        let minsub_twice = X87Binary80::from_bytes([2, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(
            add(minsub, minsub).unwrap().to_bytes(),
            minsub_twice.to_bytes()
        );

        let max_subnormal =
            X87Binary80::from_bytes([0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0]);
        let positive_zero = X87Binary80::from_bytes([0; 10]);
        let negative_zero = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]);
        let pseudo_denormal = X87Binary80::from_bytes([0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0]);
        for (lhs, rhs, value) in [
            (positive_zero, one, one),
            (one, positive_zero, one),
            (negative_zero, max_subnormal, max_subnormal),
            (max_subnormal, negative_zero, max_subnormal),
            (positive_zero, pseudo_denormal, pseudo_denormal),
            (pseudo_denormal, positive_zero, pseudo_denormal),
        ] {
            let result = add(lhs, rhs).unwrap();
            let normalized_value = value.normalize_finite().unwrap();
            let expected = round_pack_exact_coefficient(
                normalized_value.exponent(),
                if normalized_value.sign() {
                    -(normalized_value.significand() as i128)
                } else {
                    normalized_value.significand() as i128
                },
                RationalRoundingMode::NearestTiesEven,
            )
            .unwrap();
            assert_eq!(result.to_bytes(), expected.to_bytes());
        }

        let max_finite = raw_finite(0x7ffe, u64::MAX, false);
        assert_eq!(
            add(positive_zero, max_finite).unwrap().to_bytes(),
            max_finite.to_bytes()
        );
        assert_eq!(add(max_finite, max_finite), None);
        assert_eq!(add(one, negative_one), None);
        assert_eq!(
            add(min_normal, raw_finite(0x4000, 0x8000_0000_0000_0000, false)),
            None
        );

        for (left_sign, right_sign) in [(false, false), (false, true), (true, false), (true, true)]
        {
            let left_zero = X87Binary80::from_bytes(if left_sign {
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
            } else {
                [0; 10]
            });
            let right_zero = X87Binary80::from_bytes(if right_sign {
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
            } else {
                [0; 10]
            });
            assert_eq!(add(left_zero, right_zero), None);
        }
    }

    #[test]
    fn same_exponent_sum_rounds_positive_and_negative_normal_ties_in_all_modes() {
        let modes = [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ];
        let cases = [
            (false, false, [0, 0, 1, 0]),
            (false, true, [2, 1, 2, 1]),
            (true, false, [0, 0, 0, 1]),
            (true, true, [2, 1, 1, 2]),
        ];

        for (sign, odd_retained, expected_lsb) in cases {
            let lhs_significand = if odd_retained {
                0x8000_0000_0000_0001
            } else {
                0x8000_0000_0000_0000
            };
            let lhs = raw_finite(0x3fff, lhs_significand, sign)
                .normalize_finite()
                .unwrap();
            let rhs_sig = if odd_retained {
                0x8000_0000_0000_0002
            } else {
                0x8000_0000_0000_0001
            };
            let rhs = raw_finite(0x3fff, rhs_sig, sign)
                .normalize_finite()
                .unwrap();
            let input_before = (lhs, rhs);
            for (mode, lsb) in modes.into_iter().zip(expected_lsb) {
                let result = add_finite_same_exponent_to_binary80(lhs, rhs, mode).unwrap();
                let expected_sign_exponent: u16 = if sign { 0xc000 } else { 0x4000 };
                let mut expected = [0; 10];
                let significand = (0x8000_0000_0000_0000_u64 | lsb as u64).to_le_bytes();
                expected[..8].copy_from_slice(&significand);
                expected[8..].copy_from_slice(&expected_sign_exponent.to_le_bytes());
                assert_eq!(result.to_bytes(), expected, "sign={sign} mode={mode:?}");
            }
            assert_eq!((lhs, rhs), input_before);
            assert_eq!(lhs.source_class(), X87Binary80Class::Normal);
            assert_eq!(rhs.source_class(), X87Binary80Class::Normal);
        }
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
    fn exact_coefficient_rounding_matches_normal_and_subnormal_literal_encodings() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let cases = [
            (0, 1_i128 << 63, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]),
            (0, -(1_i128 << 63), [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0xbf]),
            (0, (1_i128 << 64) + 1, [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x40]),
            (0, (1_i128 << 64) + 3, [2, 0, 0, 0, 0, 0, 0, 0x80, 0, 0x40]),
            (0, (1_i128 << 65) - 1, [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0x40]),
            (-16382, 1, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (-16382, -1, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
            (
                -16385,
                (1_i128 << 64) + 11,
                [1, 0, 0, 0, 0, 0, 0, 0x20, 0, 0],
            ),
            (
                -16382,
                (1_i128 << 63) - 1,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f, 0, 0],
            ),
            (
                -16383,
                (1_i128 << 64) - 1,
                [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0],
            ),
            (
                16383,
                (1_i128 << 64) - 1,
                [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x7f],
            ),
            (-64, i128::MIN, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0xbf]),
            (-64, i128::MAX, [0, 0, 0, 0, 0, 0, 0, 0x80, 0xff, 0x3f]),
        ];
        for (exponent, coefficient, expected) in cases {
            assert_eq!(
                round_pack_exact_coefficient(exponent, coefficient, Nearest)
                    .unwrap()
                    .to_bytes(),
                expected,
                "exponent={exponent}, coefficient={coefficient}"
            );
        }

        let negative_normal_tie = -(1_i128 << 64) - 1;
        for (mode, expected) in [
            (Nearest, [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0]),
            (
                RationalRoundingMode::TowardZero,
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0],
            ),
            (
                RationalRoundingMode::TowardPositive,
                [0, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0],
            ),
            (
                RationalRoundingMode::TowardNegative,
                [1, 0, 0, 0, 0, 0, 0, 0x80, 0, 0xc0],
            ),
        ] {
            assert_eq!(
                round_pack_exact_coefficient(0, negative_normal_tie, mode)
                    .unwrap()
                    .to_bytes(),
                expected
            );
        }

        // The directed result for i128::MAX retains the value below one.
        assert_eq!(
            round_pack_exact_coefficient(-64, i128::MAX, RationalRoundingMode::TowardZero)
                .unwrap()
                .to_bytes(),
            [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xfe, 0x3f]
        );
    }

    #[test]
    fn exact_coefficient_rounding_handles_modes_boundaries_and_rejection() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let half_subnormal_modes = [
            (
                1_i128,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
            ),
            (
                -1_i128,
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
                [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80],
            ),
        ];
        for (coefficient, expected) in [
            (1_i128, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (-1_i128, [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]),
        ] {
            for mode in [Nearest, Zero, Up, Down] {
                assert_eq!(
                    round_pack_exact_coefficient(-16382, coefficient, mode)
                        .unwrap()
                        .to_bytes(),
                    expected
                );
            }
        }
        for (coefficient, nearest, zero, up, down) in half_subnormal_modes {
            for (mode, expected) in [(Nearest, nearest), (Zero, zero), (Up, up), (Down, down)] {
                assert_eq!(
                    round_pack_exact_coefficient(-16383, coefficient, mode)
                        .unwrap()
                        .to_bytes(),
                    expected
                );
            }
        }

        assert_eq!(round_pack_exact_coefficient(i32::MIN, 1, Nearest), None);
        assert_eq!(
            round_pack_exact_coefficient(i32::MAX, i128::MIN, Nearest),
            None
        );
        assert_eq!(
            round_pack_exact_coefficient(16383, 1_i128 << 64, Nearest),
            None
        );
        for mode in [Nearest, Zero, Up, Down] {
            assert_eq!(round_pack_exact_coefficient(i32::MIN, 0, mode), None);
            assert_eq!(round_pack_exact_coefficient(i32::MAX, 0, mode), None);
        }
    }

    #[test]
    fn exact_coefficient_rounding_composes_with_same_exponent_sum() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let min_normal = finite(
            -16382,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let adjacent = finite(
            -16382,
            0x8000_0000_0000_0001,
            false,
            X87Binary80Class::Normal,
        );
        let adjacent_negative = finite(
            -16382,
            0x8000_0000_0000_0001,
            true,
            X87Binary80Class::Normal,
        );
        let zero = finite(0, 0, true, X87Binary80Class::Zero);
        let unequal = finite(
            -16381,
            0x8000_0000_0000_0000,
            false,
            X87Binary80Class::Normal,
        );
        let operands_before = (min_normal, adjacent, adjacent_negative, zero, unequal);

        let (cancel_exponent, positive_one) = add_finite_same_exponent(
            adjacent,
            finite(
                -16382,
                0x8000_0000_0000_0000,
                true,
                X87Binary80Class::Normal,
            ),
        )
        .unwrap();
        assert_eq!(positive_one, 1);
        assert_eq!(
            round_pack_exact_coefficient(cancel_exponent, positive_one, Nearest)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );

        let (negative_exponent, negative_one) =
            add_finite_same_exponent(min_normal, adjacent_negative).unwrap();
        assert_eq!(negative_one, -1);
        assert_eq!(
            round_pack_exact_coefficient(negative_exponent, negative_one, Nearest)
                .unwrap()
                .to_bytes(),
            [1, 0, 0, 0, 0, 0, 0, 0, 0, 0x80]
        );

        let (carry_exponent, carry) = add_finite_same_exponent(min_normal, adjacent).unwrap();
        assert_eq!(carry, 0x1_0000_0000_0000_0001);
        assert_eq!(
            round_pack_exact_coefficient(carry_exponent, carry, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 2, 0]
        );

        let (one_zero_exponent, one_zero_coefficient) =
            add_finite_same_exponent(zero, min_normal).unwrap();
        assert_eq!(
            round_pack_exact_coefficient(one_zero_exponent, one_zero_coefficient, Nearest)
                .unwrap()
                .to_bytes(),
            [0, 0, 0, 0, 0, 0, 0, 0x80, 1, 0]
        );
        assert_eq!(add_finite_same_exponent(min_normal, unequal), None);
        assert_eq!(
            (min_normal, adjacent, adjacent_negative, zero, unequal),
            operands_before
        );
    }

    #[test]
    fn exact_magnitude_rounding_preserves_odd_tail_for_normal_ties() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        let tails = [
            ((1_u64 << 63) - 1, false),
            (1_u64 << 63, false),
            ((1_u64 << 63) + 1, true),
        ];
        for sign in [false, true] {
            for (low, above_tie) in tails {
                let magnitude = (1_u128 << 127) | u128::from(low);
                for mode in modes {
                    let increment = match mode {
                        Nearest => above_tie,
                        Zero => false,
                        Up => !sign,
                        Down => sign,
                    };
                    let expected_significand = (1_u64 << 63) + u64::from(increment);
                    assert_eq!(
                        round_pack_exact_magnitude(sign, 0, magnitude, mode),
                        Some(raw_finite(0x3fff, expected_significand, sign)),
                        "sign={sign}, low={low:#018x}, mode={mode:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn exact_magnitude_rounding_covers_subnormal_boundaries_and_even_regression() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let normalized = 1_u128 << 127;
        assert_eq!(
            round_pack_exact_magnitude(false, -16382, normalized, Nearest),
            Some(raw_finite(1, 1_u64 << 63, false)),
            "minimum normal"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, -16383, normalized, Nearest),
            Some(raw_finite(0, 1_u64 << 62, false)),
            "half minimum normal is an exact subnormal"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, -16445, normalized, Nearest),
            Some(raw_finite(0, 1, false)),
            "minimum subnormal"
        );

        let largest_subnormal_magnitude = u128::from(u64::MAX - 1) << 64;
        assert_eq!(
            round_pack_exact_magnitude(false, -16383, largest_subnormal_magnitude, Nearest),
            Some(raw_finite(0, u64::MAX >> 1, false)),
            "largest subnormal"
        );

        // At this exponent, q = u64::MAX is exactly halfway between the
        // largest subnormal and the minimum normal. Its odd retained value
        // rounds up for ties-to-even; an odd low product bit moves it above tie.
        let half_min_normal = u128::from(u64::MAX) << 64;
        for sign in [false, true] {
            for mode in [Nearest, Zero, Up, Down] {
                let rounds_to_minimum_normal = match mode {
                    Nearest => true,
                    Zero => false,
                    Up => !sign,
                    Down => sign,
                };
                let expected = if rounds_to_minimum_normal {
                    raw_finite(1, 1_u64 << 63, sign)
                } else {
                    raw_finite(0, u64::MAX >> 1, sign)
                };
                assert_eq!(
                    round_pack_exact_magnitude(sign, -16383, half_min_normal, mode),
                    Some(expected),
                    "subnormal boundary sign={sign}, mode={mode:?}"
                );
            }
        }
        assert_eq!(
            round_pack_exact_magnitude(false, -16383, half_min_normal | 1, Nearest),
            Some(raw_finite(1, 1_u64 << 63, false)),
            "odd low bit nudges subnormal boundary above the tie"
        );

        let half_min_subnormal = round_pack_exact_magnitude(true, -16446, normalized, Nearest);
        assert_eq!(half_min_subnormal, Some(raw_finite(0, 0, true)));
        assert_eq!(
            round_pack_exact_magnitude(true, -16446, normalized, Down),
            Some(raw_finite(0, 1, true))
        );
        assert_eq!(
            round_pack_exact_magnitude(true, -16446, normalized | 1, Nearest),
            Some(raw_finite(0, 1, true)),
            "odd low bit moves an exact half-minsub tail above half"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, -16446, normalized, Up),
            Some(raw_finite(0, 1, false))
        );

        for significand in [1_u64 << 63, (1_u64 << 63) + 1, u64::MAX] {
            let magnitude = u128::from(significand) << 64;
            for sign in [false, true] {
                let coefficient = if sign {
                    -i128::from(significand)
                } else {
                    i128::from(significand)
                };
                for exponent in [-16445, -16383, -16382, 0, 16383] {
                    for mode in [Nearest, Zero, Up, Down] {
                        assert_eq!(
                            round_pack_exact_magnitude(sign, exponent, magnitude, mode),
                            round_pack_exact_coefficient(exponent, coefficient, mode),
                            "even magnitude regression: sign={sign}, exponent={exponent}, significand={significand:#018x}, mode={mode:?}"
                        );
                    }
                }
            }
        }

        assert_eq!(round_pack_exact_magnitude(false, 0, 0, Nearest), None);
        assert_eq!(
            round_pack_exact_magnitude(false, i32::MIN, 1, Nearest),
            None,
            "checked normalization exponent underflow"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, 16384, normalized, Nearest),
            None,
            "outside finite exponent range"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, 16383, u128::MAX, Nearest),
            None,
            "rounding carry beyond maximum finite"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, 0, u128::MAX, Nearest),
            Some(raw_finite(0x4000, 1_u64 << 63, false)),
            "normal rounding carry increments the exponent"
        );
        assert_eq!(
            round_pack_exact_magnitude(false, 0, u128::MAX, Zero),
            Some(raw_finite(0x3fff, u64::MAX, false)),
            "toward-zero does not carry"
        );
    }

    #[test]
    fn finite_product_rounds_full_u128_once_and_rejects_zero_or_overflow() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one = finite(0, 1_u64 << 63, false, X87Binary80Class::Normal);
        let maximum = finite(0, u64::MAX, false, X87Binary80Class::Normal);
        let maximum_negative = finite(0, u64::MAX, true, X87Binary80Class::PseudoDenormal);
        let minimum_subnormal = finite(-16445, 1_u64 << 63, false, X87Binary80Class::Subnormal);
        let negative_minimum_subnormal =
            finite(-16445, 1_u64 << 63, true, X87Binary80Class::Normal);
        let zero = finite(0, 0, false, X87Binary80Class::Zero);
        let before = (
            one,
            maximum,
            maximum_negative,
            minimum_subnormal,
            negative_minimum_subnormal,
            zero,
        );

        for mode in [Nearest, Zero, Up, Down] {
            assert_eq!(
                multiply_finite_to_binary80(one, one, mode),
                Some(raw_finite(0x3fff, 1_u64 << 63, false))
            );
        }

        // The product's low 64-bit word is one. Nearest must retain the even
        // high word; directed rounding still observes that odd sticky tail.
        for (lhs, rhs) in [(maximum, maximum), (maximum_negative, maximum)] {
            let sign = lhs.sign() ^ rhs.sign();
            for mode in [Nearest, Zero, Up, Down] {
                let increment = match mode {
                    Nearest | Zero => false,
                    Up => !sign,
                    Down => sign,
                };
                let significand = (u64::MAX - 1) + u64::from(increment);
                assert_eq!(
                    multiply_finite_to_binary80(lhs, rhs, mode),
                    Some(raw_finite(0x4000, significand, sign)),
                    "mode={mode:?}, sign={sign}"
                );
            }
        }

        assert_eq!(
            multiply_finite_to_binary80(minimum_subnormal, negative_minimum_subnormal, Zero),
            Some(raw_finite(0, 0, true)),
            "nonzero negative underflow preserves the sign of zero"
        );
        assert_eq!(
            multiply_finite_to_binary80(minimum_subnormal, negative_minimum_subnormal, Down),
            Some(raw_finite(0, 1, true)),
            "directed negative underflow rounds to negative minimum subnormal"
        );
        assert_eq!(
            multiply_finite_to_binary80(minimum_subnormal, minimum_subnormal, Up),
            Some(raw_finite(0, 1, false)),
            "directed positive underflow rounds to minimum subnormal"
        );

        let largest = finite(16383, u64::MAX, false, X87Binary80Class::Normal);
        assert_eq!(
            multiply_finite_to_binary80(largest, one, Nearest),
            Some(raw_finite(0x7ffe, u64::MAX, false)),
            "largest finite multiplied by one remains finite"
        );
        let two = finite(1, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(multiply_finite_to_binary80(largest, two, Nearest), None);
        assert_eq!(multiply_finite_to_binary80(zero, one, Nearest), None);

        let extreme = finite(i32::MAX, 1_u64 << 63, false, X87Binary80Class::Normal);
        assert_eq!(multiply_finite_to_binary80(extreme, extreme, Nearest), None);
        assert_eq!(
            (
                one,
                maximum,
                maximum_negative,
                minimum_subnormal,
                negative_minimum_subnormal,
                zero
            ),
            before,
            "inputs and source-class provenance are immutable"
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
