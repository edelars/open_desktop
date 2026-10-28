//! Lossless binary32 and binary64 fields and exact finite-value normalization.
//!
//! These types inspect encoded bits without performing guest floating-point
//! operations. The module also provides mathematical rounding and normal-packing
//! helpers; they do not implement instruction rounding state or exception
//! handling. Equality on [`Binary32`] and
//! [`Binary64`] compares raw encodings, including NaN sign and payload; it does
//! not model guest floating-point comparison.

pub use crate::rational_rounding::RationalRoundingMode;

/// The six value classes shared by IEEE 754 binary32 and binary64 encodings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryClass {
    Zero,
    Subnormal,
    Normal,
    Infinity,
    QuietNaN,
    SignalingNaN,
}

/// An immutable binary32 encoding.
///
/// Equality is raw-bit identity, not guest floating-point equality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binary32(u32);

impl Binary32 {
    const SIGN_MASK: u32 = 1 << 31;
    const EXPONENT_MASK: u32 = 0xff;
    const FRACTION_MASK: u32 = (1 << 23) - 1;
    const QUIET_BIT: u32 = 1 << 22;

    /// Constructs a binary32 value without interpreting or changing its bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the exact stored encoding.
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    /// Returns whether the sign bit is set.
    pub const fn sign(self) -> bool {
        self.0 & Self::SIGN_MASK != 0
    }

    /// Returns the unsigned, biased exponent field.
    pub const fn biased_exponent(self) -> u16 {
        ((self.0 >> 23) & Self::EXPONENT_MASK) as u16
    }

    /// Returns the fraction field, without an implicit leading bit.
    pub const fn fraction(self) -> u32 {
        self.0 & Self::FRACTION_MASK
    }

    /// Classifies the encoding while retaining all raw bits in `self`.
    pub const fn class(self) -> BinaryClass {
        let exponent = self.biased_exponent();
        let fraction = self.fraction();
        if exponent == 0 {
            if fraction == 0 {
                BinaryClass::Zero
            } else {
                BinaryClass::Subnormal
            }
        } else if exponent != Self::EXPONENT_MASK as u16 {
            BinaryClass::Normal
        } else if fraction == 0 {
            BinaryClass::Infinity
        } else if fraction & Self::QUIET_BIT != 0 {
            BinaryClass::QuietNaN
        } else {
            BinaryClass::SignalingNaN
        }
    }

    /// Converts a finite encoding to an exact normalized integer significand.
    ///
    /// For nonzero values, `value = (-1)^sign * significand * 2^(exponent-63)`
    /// and bit 63 of `significand` is set. Zero retains its sign with a canonical
    /// zero significand and exponent. Infinite and NaN encodings return `None`.
    pub const fn normalize_finite(self) -> Option<BinaryFinite> {
        let class = self.class();
        let sign = self.sign();
        let fraction = self.fraction() as u64;
        let biased_exponent = self.biased_exponent();
        match class {
            BinaryClass::Zero => Some(BinaryFinite::new(sign, 0, 0, class)),
            BinaryClass::Subnormal => {
                let leading_bit = 63 - fraction.leading_zeros() as i32;
                let significand = fraction << (63 - leading_bit);
                let exponent = leading_bit - 149;
                Some(BinaryFinite::new(sign, significand, exponent, class))
            }
            BinaryClass::Normal => {
                let significand = (((1 << 23) | fraction as u32) as u64) << 40;
                let exponent = biased_exponent as i32 - 127;
                Some(BinaryFinite::new(sign, significand, exponent, class))
            }
            BinaryClass::Infinity | BinaryClass::QuietNaN | BinaryClass::SignalingNaN => None,
        }
    }

    /// Packs an already-rounded, normal binary32 significand.
    ///
    /// `magnitude` represents `(-1)^sign * magnitude * 2^(exponent+1-24)`.
    /// A carry to `2^24` is normalized by incrementing the exponent. This method
    /// does not round, handle subnormals, or define overflow behavior.
    pub fn pack_normal_rounded_significand(
        sign: bool,
        exponent: i32,
        magnitude: u128,
    ) -> Option<Self> {
        const PRECISION: u32 = 24;
        const EMIN: i32 = -126;
        const EMAX: i32 = 127;

        let precision_limit = 1_u128 << PRECISION;
        if magnitude < (precision_limit >> 1) || magnitude > precision_limit {
            return None;
        }
        let (significand, exponent) = if magnitude == precision_limit {
            (1_u32 << (PRECISION - 1), exponent.checked_add(1)?)
        } else {
            (magnitude as u32, exponent)
        };
        if !(EMIN..=EMAX).contains(&exponent) {
            return None;
        }

        let biased_exponent = (exponent + 127) as u32;
        let bits =
            ((sign as u32) << 31) | (biased_exponent << 23) | (significand & Self::FRACTION_MASK);
        Some(Self::from_bits(bits))
    }

    /// Rounds an exact normalized quotient directly to binary32 subnormal units.
    ///
    /// The input represents `(-1)^sign * (quotient + remainder/denominator) *
    /// 2^(exponent-63)`. The result may be zero or the minimum normal value;
    /// `inexact` reports discarded information only and does not update flags.
    /// This helper accepts only nonzero normalized finite quotients below the
    /// normal exponent range.
    pub fn round_pack_subnormal_quotient(
        sign: bool,
        exponent: i32,
        quotient: u64,
        remainder: u64,
        denominator: u64,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let (magnitude, inexact) = round_subnormal_quotient(
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
            24,
            -126,
            -149,
            mode,
        )?;
        Some((
            Self::from_bits((u32::from(sign) << 31) | magnitude as u32),
            inexact,
        ))
    }

    /// Divides two finite values and rounds once to binary32 precision.
    ///
    /// Finite inputs are represented by their exact normalized integer forms.
    /// The `inexact` result reports only mathematical rounding; this helper does
    /// not update floating-point status or choose an architectural exception.
    pub fn divide_finite(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        if lhs.significand == 0 || rhs.significand == 0 {
            return None;
        }
        let quotient = divide_finite_significands(lhs, rhs)?;
        if quotient.exponent() >= -126 {
            let (magnitude, inexact) = round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                24,
                mode,
            )?;
            let value = Self::pack_normal_rounded_significand(
                quotient.sign(),
                quotient.exponent(),
                magnitude,
            )?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                quotient.sign(),
                quotient.exponent(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                mode,
            )
        }
    }

    /// Multiplies two finite values and rounds once to binary32 precision.
    ///
    /// The exact integer product is normalized and bridged to a rational
    /// significand without discarding bits. `inexact` reports only final target
    /// precision loss; no floating-point status or exception is updated.
    pub fn multiply_finite(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        if lhs.significand == 0 || rhs.significand == 0 {
            return None;
        }
        let (sign, exponent, quotient, remainder, denominator) = finite_product_quotient(lhs, rhs)?;
        if exponent >= -126 {
            let (magnitude, inexact) =
                round_quotient_to_precision(sign, quotient, remainder, denominator, 24, mode)?;
            let value = Self::pack_normal_rounded_significand(sign, exponent, magnitude)?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                sign,
                exponent,
                quotient,
                remainder,
                denominator,
                mode,
            )
        }
    }

    /// Adds two finite values and rounds once to binary32 precision.
    ///
    /// The exact aligned sum is bridged without discarding bits before target
    /// rounding. Both-zero and exact-cancellation cases return `None`, leaving
    /// their guest zero sign policy to a later layer.
    pub fn add_finite_aligned(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let (sign, exponent, magnitude) = add_finite_aligned(lhs, rhs)?;
        let (sign, exponent, quotient, remainder, denominator) =
            finite_sum_quotient(sign, exponent, magnitude)?;
        if exponent >= -126 {
            let (rounded, inexact) =
                round_quotient_to_precision(sign, quotient, remainder, denominator, 24, mode)?;
            let value = Self::pack_normal_rounded_significand(sign, exponent, rounded)?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                sign,
                exponent,
                quotient,
                remainder,
                denominator,
                mode,
            )
        }
    }

    /// Subtracts two finite values by negating the copied right operand.
    ///
    /// Exact cancellation and two-zero inputs return `None`, matching the
    /// bounded addition helper's deferred zero-sign policy.
    pub fn subtract_finite_aligned(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let negated_rhs =
            BinaryFinite::new(!rhs.sign, rhs.significand, rhs.exponent, rhs.source_class);
        Self::add_finite_aligned(lhs, negated_rhs, mode)
    }
}

/// An immutable binary64 encoding.
///
/// Equality is raw-bit identity, not guest floating-point equality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binary64(u64);

impl Binary64 {
    const SIGN_MASK: u64 = 1 << 63;
    const EXPONENT_MASK: u64 = 0x7ff;
    const FRACTION_MASK: u64 = (1 << 52) - 1;
    const QUIET_BIT: u64 = 1 << 51;

    /// Constructs a binary64 value without interpreting or changing its bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the exact stored encoding.
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// Returns whether the sign bit is set.
    pub const fn sign(self) -> bool {
        self.0 & Self::SIGN_MASK != 0
    }

    /// Returns the unsigned, biased exponent field.
    pub const fn biased_exponent(self) -> u16 {
        ((self.0 >> 52) & Self::EXPONENT_MASK) as u16
    }

    /// Returns the fraction field, without an implicit leading bit.
    pub const fn fraction(self) -> u64 {
        self.0 & Self::FRACTION_MASK
    }

    /// Classifies the encoding while retaining all raw bits in `self`.
    pub const fn class(self) -> BinaryClass {
        let exponent = self.biased_exponent();
        let fraction = self.fraction();
        if exponent == 0 {
            if fraction == 0 {
                BinaryClass::Zero
            } else {
                BinaryClass::Subnormal
            }
        } else if exponent != Self::EXPONENT_MASK as u16 {
            BinaryClass::Normal
        } else if fraction == 0 {
            BinaryClass::Infinity
        } else if fraction & Self::QUIET_BIT != 0 {
            BinaryClass::QuietNaN
        } else {
            BinaryClass::SignalingNaN
        }
    }

    /// Converts a finite encoding to an exact normalized integer significand.
    ///
    /// For nonzero values, `value = (-1)^sign * significand * 2^(exponent-63)`
    /// and bit 63 of `significand` is set. Zero retains its sign with a canonical
    /// zero significand and exponent. Infinite and NaN encodings return `None`.
    pub const fn normalize_finite(self) -> Option<BinaryFinite> {
        let class = self.class();
        let sign = self.sign();
        let fraction = self.fraction();
        let biased_exponent = self.biased_exponent();
        match class {
            BinaryClass::Zero => Some(BinaryFinite::new(sign, 0, 0, class)),
            BinaryClass::Subnormal => {
                let leading_bit = 63 - fraction.leading_zeros() as i32;
                let significand = fraction << (63 - leading_bit);
                let exponent = leading_bit - 1074;
                Some(BinaryFinite::new(sign, significand, exponent, class))
            }
            BinaryClass::Normal => {
                let significand = ((1 << 52) | fraction) << 11;
                let exponent = biased_exponent as i32 - 1023;
                Some(BinaryFinite::new(sign, significand, exponent, class))
            }
            BinaryClass::Infinity | BinaryClass::QuietNaN | BinaryClass::SignalingNaN => None,
        }
    }

    /// Packs an already-rounded, normal binary64 significand.
    ///
    /// `magnitude` represents `(-1)^sign * magnitude * 2^(exponent+1-53)`.
    /// A carry to `2^53` is normalized by incrementing the exponent. This method
    /// does not round, handle subnormals, or define overflow behavior.
    pub fn pack_normal_rounded_significand(
        sign: bool,
        exponent: i32,
        magnitude: u128,
    ) -> Option<Self> {
        const PRECISION: u32 = 53;
        const EMIN: i32 = -1022;
        const EMAX: i32 = 1023;

        let precision_limit = 1_u128 << PRECISION;
        if magnitude < (precision_limit >> 1) || magnitude > precision_limit {
            return None;
        }
        let (significand, exponent) = if magnitude == precision_limit {
            (1_u64 << (PRECISION - 1), exponent.checked_add(1)?)
        } else {
            (magnitude as u64, exponent)
        };
        if !(EMIN..=EMAX).contains(&exponent) {
            return None;
        }

        let biased_exponent = (exponent + 1023) as u64;
        let bits =
            ((sign as u64) << 63) | (biased_exponent << 52) | (significand & Self::FRACTION_MASK);
        Some(Self::from_bits(bits))
    }

    /// Rounds an exact normalized quotient directly to binary64 subnormal units.
    ///
    /// The input represents `(-1)^sign * (quotient + remainder/denominator) *
    /// 2^(exponent-63)`. The result may be zero or the minimum normal value;
    /// `inexact` reports discarded information only and does not update flags.
    /// This helper accepts only nonzero normalized finite quotients below the
    /// normal exponent range.
    pub fn round_pack_subnormal_quotient(
        sign: bool,
        exponent: i32,
        quotient: u64,
        remainder: u64,
        denominator: u64,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let (magnitude, inexact) = round_subnormal_quotient(
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
            53,
            -1022,
            -1074,
            mode,
        )?;
        Some((
            Self::from_bits((u64::from(sign) << 63) | magnitude),
            inexact,
        ))
    }

    /// Divides two finite values and rounds once to binary64 precision.
    ///
    /// Finite inputs are represented by their exact normalized integer forms.
    /// The `inexact` result reports only mathematical rounding; this helper does
    /// not update floating-point status or choose an architectural exception.
    pub fn divide_finite(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        if lhs.significand == 0 || rhs.significand == 0 {
            return None;
        }
        let quotient = divide_finite_significands(lhs, rhs)?;
        if quotient.exponent() >= -1022 {
            let (magnitude, inexact) = round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                53,
                mode,
            )?;
            let value = Self::pack_normal_rounded_significand(
                quotient.sign(),
                quotient.exponent(),
                magnitude,
            )?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                quotient.sign(),
                quotient.exponent(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                mode,
            )
        }
    }

    /// Multiplies two finite values and rounds once to binary64 precision.
    ///
    /// The exact integer product is normalized and bridged to a rational
    /// significand without discarding bits. `inexact` reports only final target
    /// precision loss; no floating-point status or exception is updated.
    pub fn multiply_finite(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        if lhs.significand == 0 || rhs.significand == 0 {
            return None;
        }
        let (sign, exponent, quotient, remainder, denominator) = finite_product_quotient(lhs, rhs)?;
        if exponent >= -1022 {
            let (magnitude, inexact) =
                round_quotient_to_precision(sign, quotient, remainder, denominator, 53, mode)?;
            let value = Self::pack_normal_rounded_significand(sign, exponent, magnitude)?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                sign,
                exponent,
                quotient,
                remainder,
                denominator,
                mode,
            )
        }
    }

    /// Adds two finite values and rounds once to binary64 precision.
    ///
    /// The exact aligned sum is bridged without discarding bits before target
    /// rounding. Both-zero and exact-cancellation cases return `None`, leaving
    /// their guest zero sign policy to a later layer.
    pub fn add_finite_aligned(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let (sign, exponent, magnitude) = add_finite_aligned(lhs, rhs)?;
        let (sign, exponent, quotient, remainder, denominator) =
            finite_sum_quotient(sign, exponent, magnitude)?;
        if exponent >= -1022 {
            let (rounded, inexact) =
                round_quotient_to_precision(sign, quotient, remainder, denominator, 53, mode)?;
            let value = Self::pack_normal_rounded_significand(sign, exponent, rounded)?;
            Some((value, inexact))
        } else {
            Self::round_pack_subnormal_quotient(
                sign,
                exponent,
                quotient,
                remainder,
                denominator,
                mode,
            )
        }
    }

    /// Subtracts two finite values by negating the copied right operand.
    ///
    /// Exact cancellation and two-zero inputs return `None`, matching the
    /// bounded addition helper's deferred zero-sign policy.
    pub fn subtract_finite_aligned(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<(Self, bool)> {
        let negated_rhs =
            BinaryFinite::new(!rhs.sign, rhs.significand, rhs.exponent, rhs.source_class);
        Self::add_finite_aligned(lhs, negated_rhs, mode)
    }
}

/// Exact normalized representation shared by finite binary32 and binary64 values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryFinite {
    sign: bool,
    significand: u64,
    exponent: i32,
    source_class: BinaryClass,
}

impl BinaryFinite {
    const fn new(sign: bool, significand: u64, exponent: i32, source_class: BinaryClass) -> Self {
        Self {
            sign,
            significand,
            exponent,
            source_class,
        }
    }

    /// Returns whether the original encoding was negative.
    pub const fn sign(self) -> bool {
        self.sign
    }

    /// Returns the normalized integer significand (zero for either signed zero).
    pub const fn significand(self) -> u64 {
        self.significand
    }

    /// Returns the power in `significand * 2^(exponent-63)`.
    pub const fn exponent(self) -> i32 {
        self.exponent
    }

    /// Returns the finite class of the original encoding.
    pub const fn source_class(self) -> BinaryClass {
        self.source_class
    }
}

/// Compares two finite values numerically without changing raw-encoding identity.
///
/// Signed zeros compare equal. Nonzero values compare by sign and then by their
/// normalized `(exponent, significand)` pair; negative magnitudes reverse that
/// order. This helper does not define NaN, infinity, or guest comparison flags.
pub fn compare_finite(lhs: BinaryFinite, rhs: BinaryFinite) -> core::cmp::Ordering {
    use core::cmp::Ordering;

    let lhs_zero = lhs.significand == 0;
    let rhs_zero = rhs.significand == 0;
    match (lhs_zero, rhs_zero) {
        (true, true) => return Ordering::Equal,
        (true, false) => {
            return if rhs.sign {
                Ordering::Greater
            } else {
                Ordering::Less
            };
        }
        (false, true) => {
            return if lhs.sign {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
        (false, false) => {}
    }

    if lhs.sign != rhs.sign {
        return if lhs.sign {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }

    let magnitude_order = (lhs.exponent, lhs.significand).cmp(&(rhs.exponent, rhs.significand));
    if lhs.sign {
        magnitude_order.reverse()
    } else {
        magnitude_order
    }
}

/// Multiplies two normalized finite significands without rounding or packing.
///
/// For nonzero inputs, returns `(sign, exponent, magnitude)` such that the exact
/// mathematical product is `(-1)^sign * magnitude * 2^(exponent-127)`. The
/// magnitude is the full 128-bit product, with no shifting or discarded bits.
/// If either input is zero, returns the signless mathematical-zero sentinel
/// `(false, 0, 0)`; this does not choose a guest floating-point zero sign.
pub fn multiply_finite_significands(lhs: BinaryFinite, rhs: BinaryFinite) -> (bool, i32, u128) {
    if lhs.significand == 0 || rhs.significand == 0 {
        return (false, 0, 0);
    }

    let sign = lhs.sign ^ rhs.sign;
    let exponent = lhs.exponent + rhs.exponent + 1;
    let magnitude = u128::from(lhs.significand) * u128::from(rhs.significand);
    (sign, exponent, magnitude)
}

fn finite_product_quotient(
    lhs: BinaryFinite,
    rhs: BinaryFinite,
) -> Option<(bool, i32, u64, u64, u64)> {
    let (sign, product_exponent, magnitude) = multiply_finite_significands(lhs, rhs);
    if magnitude == 0 {
        return None;
    }

    let shift = magnitude.leading_zeros();
    if shift > 1 {
        return None;
    }
    let normalized = magnitude << shift;
    let low = normalized as u64;
    if low & 1 != 0 {
        return None;
    }
    let exponent = product_exponent.checked_sub(shift as i32)?;
    let quotient = (normalized >> 64) as u64;
    let remainder = low >> 1;
    let denominator = 1_u64 << 63;
    if quotient < (1_u64 << 63) || remainder >= denominator {
        return None;
    }
    Some((sign, exponent, quotient, remainder, denominator))
}

/// Adds two normalized finite values after exact alignment to a shared exponent.
///
/// Returns `(sign, exponent, magnitude)` for the exact sum
/// `(-1)^sign * magnitude * 2^(exponent-63)`. The larger exponent is aligned by
/// a left shift of at most 64 bits; wider gaps are outside this helper's bounded
/// profile and return `None`. Zero operands are mathematical identities, and
/// exact cancellation uses the signless zero sentinel `(false, 0, 0)`.
pub fn add_finite_aligned(lhs: BinaryFinite, rhs: BinaryFinite) -> Option<(bool, i32, u128)> {
    if lhs.significand == 0 {
        return Some(if rhs.significand == 0 {
            (false, 0, 0)
        } else {
            (rhs.sign, rhs.exponent, u128::from(rhs.significand))
        });
    }
    if rhs.significand == 0 {
        return Some((lhs.sign, lhs.exponent, u128::from(lhs.significand)));
    }

    let exponent = lhs.exponent.min(rhs.exponent);
    let lhs_gap = u32::try_from(i64::from(lhs.exponent) - i64::from(exponent)).ok()?;
    let rhs_gap = u32::try_from(i64::from(rhs.exponent) - i64::from(exponent)).ok()?;
    if lhs_gap > 64 || rhs_gap > 64 {
        return None;
    }

    let lhs_magnitude = u128::from(lhs.significand) << lhs_gap;
    let rhs_magnitude = u128::from(rhs.significand) << rhs_gap;
    if lhs.sign == rhs.sign {
        return Some((lhs.sign, exponent, lhs_magnitude + rhs_magnitude));
    }

    if lhs_magnitude == rhs_magnitude {
        Some((false, 0, 0))
    } else if lhs_magnitude > rhs_magnitude {
        Some((lhs.sign, exponent, lhs_magnitude - rhs_magnitude))
    } else {
        Some((rhs.sign, exponent, rhs_magnitude - lhs_magnitude))
    }
}

fn finite_sum_quotient(
    sign: bool,
    exponent: i32,
    magnitude: u128,
) -> Option<(bool, i32, u64, u64, u64)> {
    if magnitude == 0 {
        return None;
    }
    let shift = magnitude.leading_zeros();
    let normalized = magnitude << shift;
    let low = normalized as u64;
    if low & 1 != 0 {
        return None;
    }
    let normalized_exponent = i32::try_from(i64::from(exponent) + 64 - i64::from(shift)).ok()?;
    let quotient = (normalized >> 64) as u64;
    let remainder = low >> 1;
    let denominator = 1_u64 << 63;
    if quotient < (1_u64 << 63) || remainder >= denominator {
        return None;
    }
    Some((sign, normalized_exponent, quotient, remainder, denominator))
}

/// Exact quotient and remainder decomposition for two normalized finite values.
///
/// For nonzero operands, `lhs / rhs` is represented exactly as
/// `(-1)^sign * (quotient + remainder / denominator) * 2^(exponent-63)`.
/// The denominator is the original normalized divisor significand; it is not
/// reduced, and this representation is not rounded or packed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryFiniteQuotient {
    sign: bool,
    exponent: i32,
    quotient: u64,
    remainder: u64,
    denominator: u64,
}

impl BinaryFiniteQuotient {
    const fn new(
        sign: bool,
        exponent: i32,
        quotient: u64,
        remainder: u64,
        denominator: u64,
    ) -> Self {
        Self {
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
        }
    }

    const fn zero(denominator: u64) -> Self {
        Self::new(false, 0, 0, 0, denominator)
    }

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

/// Divides two finite values into an exact quotient and remainder decomposition.
///
/// Returns `None` when the divisor is zero. A zero dividend with a nonzero
/// divisor returns signless mathematical zero `(false, 0, 0, 0, d)`, retaining
/// the divisor's normalized significand as `d`; this does not choose a guest
/// floating-point zero sign. Nonzero results preserve all quotient and remainder
/// bits without rounding, jamming, or packing.
pub fn divide_finite_significands(
    lhs: BinaryFinite,
    rhs: BinaryFinite,
) -> Option<BinaryFiniteQuotient> {
    let numerator = lhs.significand;
    let denominator = rhs.significand;
    if denominator == 0 {
        return None;
    }
    if numerator == 0 {
        return Some(BinaryFiniteQuotient::zero(denominator));
    }

    let (scaled_numerator, exponent) = if numerator >= denominator {
        (u128::from(numerator) << 63, lhs.exponent - rhs.exponent)
    } else {
        (u128::from(numerator) << 64, lhs.exponent - rhs.exponent - 1)
    };
    let divisor = u128::from(denominator);
    let quotient = (scaled_numerator / divisor) as u64;
    let remainder = (scaled_numerator % divisor) as u64;
    Some(BinaryFiniteQuotient::new(
        lhs.sign ^ rhs.sign,
        exponent,
        quotient,
        remainder,
        denominator,
    ))
}

/// Rounds an exact rational quotient to binary precision, preserving inexactness.
///
/// The result is the unsigned rounded magnitude and whether discarded bits or a
/// rational remainder made the value inexact. A carry to `2^precision` is
/// preserved; this does not pack a binary32/binary64 value or update flags.
pub fn round_quotient_to_precision(
    sign: bool,
    q: u64,
    r: u64,
    d: u64,
    precision: u32,
    mode: RationalRoundingMode,
) -> Option<(u128, bool)> {
    crate::rational_rounding::round_quotient_to_precision(sign, q, r, d, precision, mode)
}

fn round_subnormal_quotient(
    sign: bool,
    exponent: i32,
    quotient: u64,
    remainder: u64,
    denominator: u64,
    precision: u32,
    minimum_normal_exponent: i32,
    subnormal_unit_exponent: i32,
    mode: RationalRoundingMode,
) -> Option<(u64, bool)> {
    if exponent >= minimum_normal_exponent
        || quotient < (1_u64 << 63)
        || denominator == 0
        || remainder >= denominator
    {
        return None;
    }

    let k = 63_i64 + i64::from(subnormal_unit_exponent) - i64::from(exponent);
    let (magnitude, inexact) = if k <= 63 {
        let retained_precision = u32::try_from(64_i64 - k).ok()?;
        crate::rational_rounding::round_quotient_to_precision(
            sign,
            quotient,
            remainder,
            denominator,
            retained_precision,
            mode,
        )?
    } else if k == 64 {
        let increment = match mode {
            RationalRoundingMode::NearestTiesEven => quotient > (1_u64 << 63) || remainder != 0,
            RationalRoundingMode::TowardZero => false,
            RationalRoundingMode::TowardPositive => !sign,
            RationalRoundingMode::TowardNegative => sign,
        };
        (u128::from(increment), true)
    } else {
        let magnitude = match mode {
            RationalRoundingMode::NearestTiesEven | RationalRoundingMode::TowardZero => 0,
            RationalRoundingMode::TowardPositive => u64::from(!sign),
            RationalRoundingMode::TowardNegative => u64::from(sign),
        };
        (u128::from(magnitude), true)
    };

    let minimum_normal = 1_u64 << (precision - 1);
    if magnitude > u128::from(minimum_normal) {
        return None;
    }
    Some((magnitude as u64, inexact))
}

#[cfg(test)]
mod tests {
    use super::{
        Binary32, Binary64, BinaryClass, BinaryFinite, BinaryFiniteQuotient, RationalRoundingMode,
        add_finite_aligned, compare_finite, divide_finite_significands,
        multiply_finite_significands, round_quotient_to_precision,
    };

    use core::cmp::Ordering;

    fn normalized(
        value: BinaryFinite,
        sign: bool,
        significand: u64,
        exponent: i32,
        class: BinaryClass,
    ) {
        assert_eq!(value.sign(), sign);
        assert_eq!(value.significand(), significand);
        assert_eq!(value.exponent(), exponent);
        assert_eq!(value.source_class(), class);
        if significand != 0 {
            assert_ne!(significand & (1 << 63), 0);
        }
    }

    fn assert_division_identity(lhs: BinaryFinite, rhs: BinaryFinite) {
        let result = divide_finite_significands(lhs, rhs).unwrap();
        assert_ne!(rhs.significand(), 0);
        assert_eq!(result.denominator(), rhs.significand());
        assert!(result.denominator() >= 1 << 63);

        if lhs.significand() == 0 {
            assert_eq!(
                (
                    result.sign(),
                    result.exponent(),
                    result.quotient(),
                    result.remainder(),
                ),
                (false, 0, 0, 0),
            );
            return;
        }

        let (scaled_numerator, expected_exponent) = if lhs.significand() >= rhs.significand() {
            (
                u128::from(lhs.significand()) << 63,
                lhs.exponent() - rhs.exponent(),
            )
        } else {
            (
                u128::from(lhs.significand()) << 64,
                lhs.exponent() - rhs.exponent() - 1,
            )
        };
        let quotient = result.quotient();
        let remainder = result.remainder();
        let denominator = result.denominator();
        assert_eq!(result.sign(), lhs.sign() ^ rhs.sign());
        assert_eq!(result.exponent(), expected_exponent);
        assert!(quotient >= 1 << 63);
        assert!(u128::from(quotient) < 1u128 << 64);
        assert!(remainder < denominator);
        assert_eq!(
            u128::from(quotient) * u128::from(denominator) + u128::from(remainder),
            scaled_numerator,
        );
    }

    #[test]
    fn finite_comparison_orders_signed_zeros_and_binary32_values() {
        let positive_zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero32 = Binary32::from_bits(0x8000_0000).normalize_finite().unwrap();
        let positive_min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal32 = Binary32::from_bits(0x8000_0001).normalize_finite().unwrap();

        for lhs in [positive_zero32, negative_zero32] {
            for rhs in [positive_zero32, negative_zero32] {
                assert_eq!(compare_finite(lhs, rhs), Ordering::Equal);
            }
        }
        assert_ne!(Binary32::from_bits(0), Binary32::from_bits(0x8000_0000));
        assert_eq!(
            compare_finite(negative_min_subnormal32, positive_zero32),
            Ordering::Less
        );
        assert_eq!(
            compare_finite(negative_min_subnormal32, negative_zero32),
            Ordering::Less
        );
        assert_eq!(
            compare_finite(positive_zero32, positive_min_subnormal32),
            Ordering::Less
        );
        assert_eq!(
            compare_finite(negative_zero32, positive_min_subnormal32),
            Ordering::Less
        );

        let negative_two32 = Binary32::from_bits(0xc000_0000).normalize_finite().unwrap();
        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        let positive_one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let positive_two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        let chain32 = [
            negative_two32,
            negative_one32,
            positive_zero32,
            positive_one32,
            positive_two32,
        ];
        assert_strict_comparison_chain(&chain32);

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let next_one32 = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        let negative_one32_next = Binary32::from_bits(0xbf80_0001).normalize_finite().unwrap();
        let negative_one32_exact = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        assert_comparison_pair(one32, next_one32, Ordering::Less);
        assert_comparison_pair(negative_one32_next, negative_one32_exact, Ordering::Less);

        let largest_below_two32 = Binary32::from_bits(0x3fff_ffff).normalize_finite().unwrap();
        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        assert_comparison_pair(largest_below_two32, two32, Ordering::Less);
        let largest_subnormal32 = Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap();
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        assert_comparison_pair(largest_subnormal32, min_normal32, Ordering::Less);
        let max_finite32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        assert_comparison_pair(positive_min_subnormal32, max_finite32, Ordering::Less);
        let negative_max_finite32 = Binary32::from_bits(0xff7f_ffff).normalize_finite().unwrap();
        assert_comparison_pair(
            negative_max_finite32,
            negative_min_subnormal32,
            Ordering::Less,
        );
    }

    #[test]
    fn finite_comparison_orders_binary64_values_and_mixed_precisions() {
        let positive_zero64 = Binary64::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal64 = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        for lhs in [positive_zero64, negative_zero64] {
            for rhs in [positive_zero64, negative_zero64] {
                assert_eq!(compare_finite(lhs, rhs), Ordering::Equal);
            }
        }
        assert_ne!(
            Binary64::from_bits(0),
            Binary64::from_bits(0x8000_0000_0000_0000)
        );
        assert_comparison_pair(negative_min_subnormal64, negative_zero64, Ordering::Less);
        assert_comparison_pair(negative_zero64, positive_min_subnormal64, Ordering::Less);

        let negative_two64 = Binary64::from_bits(0xc000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let chain64 = [
            negative_two64,
            negative_one64,
            positive_zero64,
            positive_one64,
            positive_two64,
        ];
        assert_strict_comparison_chain(&chain64);

        let next_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let negative_next_one64 = Binary64::from_bits(0xbff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(positive_one64, next_one64, Ordering::Less);
        assert_comparison_pair(negative_next_one64, negative_one64, Ordering::Less);

        let largest_below_two64 = Binary64::from_bits(0x3fff_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(largest_below_two64, two64, Ordering::Less);
        let largest_subnormal64 = Binary64::from_bits(0x000f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(largest_subnormal64, min_normal64, Ordering::Less);
        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(positive_min_subnormal64, max_finite64, Ordering::Less);
        let negative_max_finite64 = Binary64::from_bits(0xffef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(
            negative_max_finite64,
            negative_min_subnormal64,
            Ordering::Less,
        );

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let next_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let next_one32 = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        assert_comparison_pair(one32, one64, Ordering::Equal);
        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let same_min_subnormal64 = Binary64::from_bits(0x36a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(min_subnormal32, same_min_subnormal64, Ordering::Equal);
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let same_min_normal64 = Binary64::from_bits(0x3810_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_comparison_pair(min_normal32, same_min_normal64, Ordering::Equal);
        assert_comparison_pair(one32, next_one64, Ordering::Less);
        assert_comparison_pair(next_one64, next_one32, Ordering::Less);

        assert_eq!(min_subnormal32.source_class(), BinaryClass::Subnormal);
        assert_eq!(same_min_subnormal64.source_class(), BinaryClass::Normal);
        assert_eq!(min_normal32.source_class(), BinaryClass::Normal);
        assert_eq!(same_min_normal64.source_class(), BinaryClass::Normal);
        assert_eq!(
            compare_finite(min_subnormal32, min_subnormal32),
            Ordering::Equal
        );
    }

    fn assert_comparison_pair(lhs: BinaryFinite, rhs: BinaryFinite, expected: Ordering) {
        let lhs_before = lhs;
        let rhs_before = rhs;
        assert_eq!(compare_finite(lhs, rhs), expected);
        assert_eq!(compare_finite(rhs, lhs), expected.reverse());
        assert_eq!(compare_finite(lhs, lhs), Ordering::Equal);
        assert_eq!(compare_finite(rhs, rhs), Ordering::Equal);
        assert_eq!(lhs, lhs_before);
        assert_eq!(rhs, rhs_before);
    }

    fn assert_strict_comparison_chain(values: &[BinaryFinite]) {
        for pair in values.windows(2) {
            assert_eq!(compare_finite(pair[0], pair[1]), Ordering::Less);
        }
        for triple in values.windows(3) {
            assert_eq!(compare_finite(triple[0], triple[2]), Ordering::Less);
            assert_eq!(
                compare_finite(triple[0], triple[2]),
                compare_finite(triple[0], triple[1]).then(compare_finite(triple[1], triple[2]))
            );
        }
    }

    #[test]
    fn binary32_fields_classification_and_raw_identity() {
        for bit in 0..32 {
            let bits = 1u32 << bit;
            let value = Binary32::from_bits(bits);
            assert_eq!(value.to_bits(), bits);
            assert_eq!(value.sign(), bit == 31);
            assert_eq!(Binary32::from_bits(bits), value);
        }
        assert_eq!(Binary32::from_bits(0).class(), BinaryClass::Zero);
        assert_eq!(Binary32::from_bits(1).class(), BinaryClass::Subnormal);
        assert_eq!(
            Binary32::from_bits(0x007f_ffff).class(),
            BinaryClass::Subnormal
        );
        assert_eq!(
            Binary32::from_bits(0x0080_0000).class(),
            BinaryClass::Normal
        );
        assert_eq!(
            Binary32::from_bits(0x7f7f_ffff).class(),
            BinaryClass::Normal
        );
        assert_eq!(
            Binary32::from_bits(0x7f80_0000).class(),
            BinaryClass::Infinity
        );
        assert_eq!(
            Binary32::from_bits(0xff80_0000).class(),
            BinaryClass::Infinity
        );
        assert_eq!(
            Binary32::from_bits(0x7fc0_0000).class(),
            BinaryClass::QuietNaN
        );
        assert_eq!(
            Binary32::from_bits(0x7fff_ffff).class(),
            BinaryClass::QuietNaN
        );
        assert_eq!(
            Binary32::from_bits(0x7f80_0001).class(),
            BinaryClass::SignalingNaN
        );
        assert_eq!(
            Binary32::from_bits(0x7fbf_ffff).class(),
            BinaryClass::SignalingNaN
        );
        assert_eq!(
            Binary32::from_bits(0xff80_0001).class(),
            BinaryClass::SignalingNaN
        );
        assert_ne!(
            Binary32::from_bits(0x7fc0_0001),
            Binary32::from_bits(0xffc0_0001)
        );

        let mixed = Binary32::from_bits(0xd5a5_4321);
        assert_eq!(mixed.to_bits(), 0xd5a5_4321);
        assert!(mixed.sign());
        assert_eq!(mixed.biased_exponent(), 0xab);
        assert_eq!(mixed.fraction(), 0x25_4321);
        assert_eq!(mixed.class(), BinaryClass::Normal);
    }

    #[test]
    fn binary64_fields_classification_and_raw_identity() {
        for bit in 0..64 {
            let bits = 1u64 << bit;
            let value = Binary64::from_bits(bits);
            assert_eq!(value.to_bits(), bits);
            assert_eq!(value.sign(), bit == 63);
            assert_eq!(Binary64::from_bits(bits), value);
        }
        assert_eq!(Binary64::from_bits(0).class(), BinaryClass::Zero);
        assert_eq!(Binary64::from_bits(1).class(), BinaryClass::Subnormal);
        assert_eq!(
            Binary64::from_bits(0x000f_ffff_ffff_ffff).class(),
            BinaryClass::Subnormal
        );
        assert_eq!(
            Binary64::from_bits(0x0010_0000_0000_0000).class(),
            BinaryClass::Normal
        );
        assert_eq!(
            Binary64::from_bits(0x7fef_ffff_ffff_ffff).class(),
            BinaryClass::Normal
        );
        assert_eq!(
            Binary64::from_bits(0x7ff0_0000_0000_0000).class(),
            BinaryClass::Infinity
        );
        assert_eq!(
            Binary64::from_bits(0xfff0_0000_0000_0000).class(),
            BinaryClass::Infinity
        );
        assert_eq!(
            Binary64::from_bits(0x7ff8_0000_0000_0000).class(),
            BinaryClass::QuietNaN
        );
        assert_eq!(
            Binary64::from_bits(0x7fff_ffff_ffff_ffff).class(),
            BinaryClass::QuietNaN
        );
        assert_eq!(
            Binary64::from_bits(0x7ff0_0000_0000_0001).class(),
            BinaryClass::SignalingNaN
        );
        assert_eq!(
            Binary64::from_bits(0x7ff7_ffff_ffff_ffff).class(),
            BinaryClass::SignalingNaN
        );
        assert_eq!(
            Binary64::from_bits(0xfff0_0000_0000_0001).class(),
            BinaryClass::SignalingNaN
        );
        assert_ne!(
            Binary64::from_bits(0x7ff8_0000_0000_0001),
            Binary64::from_bits(0xfff8_0000_0000_0001)
        );

        let mixed = Binary64::from_bits(0xd5a5_4321_8765_4321);
        assert_eq!(mixed.to_bits(), 0xd5a5_4321_8765_4321);
        assert!(mixed.sign());
        assert_eq!(mixed.biased_exponent(), 0x55a);
        assert_eq!(mixed.fraction(), 0x5_4321_8765_4321);
        assert_eq!(mixed.class(), BinaryClass::Normal);
    }

    #[test]
    fn binary32_normalizes_finite_edges_exactly() {
        normalized(
            Binary32::from_bits(0x0000_0001).normalize_finite().unwrap(),
            false,
            1 << 63,
            -149,
            BinaryClass::Subnormal,
        );
        normalized(
            Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap(),
            false,
            0xffff_fe00_0000_0000,
            -127,
            BinaryClass::Subnormal,
        );
        normalized(
            Binary32::from_bits(0x0080_0000).normalize_finite().unwrap(),
            false,
            1 << 63,
            -126,
            BinaryClass::Normal,
        );
        normalized(
            Binary32::from_bits(0x3f7f_ffff).normalize_finite().unwrap(),
            false,
            0xffff_ff00_0000_0000,
            -1,
            BinaryClass::Normal,
        );
        normalized(
            Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap(),
            false,
            1 << 63,
            0,
            BinaryClass::Normal,
        );
        normalized(
            Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap(),
            false,
            0x8000_0100_0000_0000,
            0,
            BinaryClass::Normal,
        );
        normalized(
            Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap(),
            false,
            0xffff_ff00_0000_0000,
            127,
            BinaryClass::Normal,
        );
        normalized(
            Binary32::from_bits(0x8000_0000).normalize_finite().unwrap(),
            true,
            0,
            0,
            BinaryClass::Zero,
        );
        for leading_bit in 0..23 {
            let fraction = 1u32 << leading_bit;
            let value = Binary32::from_bits(fraction).normalize_finite().unwrap();
            let expected_exponent = leading_bit as i32 - 149;
            normalized(
                value,
                false,
                1u64 << 63,
                expected_exponent,
                BinaryClass::Subnormal,
            );
        }
        assert!(
            Binary32::from_bits(0x7f80_0000)
                .normalize_finite()
                .is_none()
        );
        assert!(
            Binary32::from_bits(0x7fc0_0001)
                .normalize_finite()
                .is_none()
        );
        assert!(
            Binary32::from_bits(0x7f80_0001)
                .normalize_finite()
                .is_none()
        );
        let value = Binary32::from_bits(0x0000_0001);
        let before = value.to_bits();
        let _ = value.normalize_finite();
        assert_eq!(value.to_bits(), before);
    }

    #[test]
    fn binary64_normalizes_finite_edges_exactly() {
        normalized(
            Binary64::from_bits(0x0000_0000_0000_0001)
                .normalize_finite()
                .unwrap(),
            false,
            1 << 63,
            -1074,
            BinaryClass::Subnormal,
        );
        normalized(
            Binary64::from_bits(0x000f_ffff_ffff_ffff)
                .normalize_finite()
                .unwrap(),
            false,
            0xffff_ffff_ffff_f000,
            -1023,
            BinaryClass::Subnormal,
        );
        normalized(
            Binary64::from_bits(0x0010_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            false,
            1 << 63,
            -1022,
            BinaryClass::Normal,
        );
        normalized(
            Binary64::from_bits(0x3fef_ffff_ffff_ffff)
                .normalize_finite()
                .unwrap(),
            false,
            0xffff_ffff_ffff_f800,
            -1,
            BinaryClass::Normal,
        );
        normalized(
            Binary64::from_bits(0x3ff0_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            false,
            1 << 63,
            0,
            BinaryClass::Normal,
        );
        normalized(
            Binary64::from_bits(0x3ff0_0000_0000_0001)
                .normalize_finite()
                .unwrap(),
            false,
            0x8000_0000_0000_0800,
            0,
            BinaryClass::Normal,
        );
        normalized(
            Binary64::from_bits(0x7fef_ffff_ffff_ffff)
                .normalize_finite()
                .unwrap(),
            false,
            0xffff_ffff_ffff_f800,
            1023,
            BinaryClass::Normal,
        );
        normalized(
            Binary64::from_bits(0x8000_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            true,
            0,
            0,
            BinaryClass::Zero,
        );
        for leading_bit in 0..52 {
            let fraction = 1u64 << leading_bit;
            let value = Binary64::from_bits(fraction).normalize_finite().unwrap();
            let expected_exponent = leading_bit as i32 - 1074;
            normalized(
                value,
                false,
                1u64 << 63,
                expected_exponent,
                BinaryClass::Subnormal,
            );
        }
        assert!(
            Binary64::from_bits(0x7ff0_0000_0000_0000)
                .normalize_finite()
                .is_none()
        );
        assert!(
            Binary64::from_bits(0x7ff8_0000_0000_0001)
                .normalize_finite()
                .is_none()
        );
        assert!(
            Binary64::from_bits(0x7ff0_0000_0000_0001)
                .normalize_finite()
                .is_none()
        );
        let value = Binary64::from_bits(0x0000_0000_0000_0001);
        let before = value.to_bits();
        let _ = value.normalize_finite();
        assert_eq!(value.to_bits(), before);
    }

    #[test]
    fn finite_multiplication_preserves_full_significand_product() {
        let one = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one_and_half = Binary32::from_bits(0x3fc0_0000).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(one, one),
            (false, 1, 0x4000_0000_0000_0000_0000_0000_0000_0000),
        );
        assert_eq!(
            multiply_finite_significands(one_and_half, one_and_half),
            (false, 1, 0x9000_0000_0000_0000_0000_0000_0000_0000),
        );

        let b32_next_one = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(b32_next_one, b32_next_one),
            (false, 1, 0x4000_0100_0001_0000_0000_0000_0000_0000),
        );
        let b64_next_one = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            multiply_finite_significands(b64_next_one, b64_next_one),
            (false, 1, 0x4000_0000_0000_0800_0000_0000_0040_0000),
        );

        let min_b32 = Binary32::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(min_b32, min_b32),
            (false, -297, 0x4000_0000_0000_0000_0000_0000_0000_0000),
        );
        let min_b64 = Binary64::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(min_b64, min_b64),
            (false, -2147, 0x4000_0000_0000_0000_0000_0000_0000_0000),
        );

        let max_b32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(max_b32, max_b32),
            (false, 255, 0xffff_fe00_0001_0000_0000_0000_0000_0000),
        );
        let max_b64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            multiply_finite_significands(max_b64, max_b64),
            (false, 2047, 0xffff_ffff_ffff_f000_0000_0000_0040_0000),
        );
    }

    #[test]
    fn finite_addition_matches_exact_aligned_literal_vectors() {
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let half = Binary64::from_bits(0x3fe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_half = Binary64::from_bits(0xbfe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one_and_half = Binary64::from_bits(0x3ff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        assert_eq!(
            add_finite_aligned(one, one),
            Some((false, 0, 0x1_0000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(one, half),
            Some((false, -1, 0x1_8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(one, negative_half),
            Some((false, -1, 0x8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(one_and_half, negative_one),
            Some((false, 0, 0x4000_0000_0000_0000)),
        );

        let next_one = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(next_one, negative_one),
            Some((false, 0, 0x800)),
        );

        let min_normal = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_largest_subnormal = Binary64::from_bits(0x800f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(min_normal, negative_largest_subnormal),
            Some((false, -1023, 0x1000)),
        );

        let min_subnormal = Binary64::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            add_finite_aligned(min_subnormal, min_subnormal),
            Some((false, -1074, 0x1_0000_0000_0000_0000)),
        );
        let max_finite = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(max_finite, max_finite),
            Some((false, 1023, 0x1_ffff_ffff_ffff_f000)),
        );

        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_64 = Binary64::from_bits(0xbbf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(one, two_to_minus_64),
            Some((false, -64, 0x8000_0000_0000_0000_8000_0000_0000_0000,)),
        );
        assert_eq!(
            add_finite_aligned(one, negative_two_to_minus_64),
            Some((false, -64, 0x7fff_ffff_ffff_ffff_8000_0000_0000_0000,)),
        );
        assert_eq!(add_finite_aligned(one, negative_one), Some((false, 0, 0)));

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        assert_eq!(
            add_finite_aligned(one32, next_one),
            Some((false, 0, 0x1_0000_0000_0000_0800)),
        );
    }

    #[test]
    fn finite_addition_handles_sign_order_gaps_and_cancellation() {
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_next_one = Binary64::from_bits(0xbff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let half = Binary64::from_bits(0x3fe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_half = Binary64::from_bits(0xbfe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_63 = Binary64::from_bits(0x3c00_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_63 = Binary64::from_bits(0xbc00_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_64 = Binary64::from_bits(0xbbf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_65 = Binary64::from_bits(0x3be0_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        for (lhs, rhs, expected) in [
            (one, half, (false, -1, 0x1_8000_0000_0000_0000)),
            (half, one, (false, -1, 0x1_8000_0000_0000_0000)),
            (one, negative_half, (false, -1, 0x8000_0000_0000_0000)),
            (negative_half, one, (false, -1, 0x8000_0000_0000_0000)),
            (negative_one, half, (true, -1, 0x8000_0000_0000_0000)),
            (half, negative_one, (true, -1, 0x8000_0000_0000_0000)),
            (
                negative_one,
                negative_half,
                (true, -1, 0x1_8000_0000_0000_0000),
            ),
            (
                negative_half,
                negative_one,
                (true, -1, 0x1_8000_0000_0000_0000),
            ),
        ] {
            assert_eq!(add_finite_aligned(lhs, rhs), Some(expected));
        }
        assert_eq!(
            add_finite_aligned(one, negative_next_one),
            Some((true, 0, 0x800))
        );
        assert_eq!(
            add_finite_aligned(negative_next_one, one),
            Some((true, 0, 0x800))
        );
        assert_eq!(
            add_finite_aligned(one, two_to_minus_63),
            Some((false, -63, 0x4000_0000_0000_0000_8000_0000_0000_0000,)),
        );
        assert_eq!(
            add_finite_aligned(one, negative_two_to_minus_63),
            Some((false, -63, 0x3fff_ffff_ffff_ffff_8000_0000_0000_0000,)),
        );
        assert_eq!(
            add_finite_aligned(one, two_to_minus_63),
            add_finite_aligned(two_to_minus_63, one),
        );
        assert_eq!(
            add_finite_aligned(one, negative_two_to_minus_63),
            add_finite_aligned(negative_two_to_minus_63, one),
        );
        assert_eq!(
            add_finite_aligned(one, two_to_minus_64),
            add_finite_aligned(two_to_minus_64, one),
        );
        assert_eq!(
            add_finite_aligned(one, negative_two_to_minus_64),
            add_finite_aligned(negative_two_to_minus_64, one),
        );
        assert_eq!(add_finite_aligned(one, two_to_minus_65), None);
        assert_eq!(add_finite_aligned(two_to_minus_65, one), None);

        assert_eq!(add_finite_aligned(one, negative_one), Some((false, 0, 0)));
        let max_finite = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let negative_max_finite = Binary64::from_bits(0xffef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(max_finite, negative_max_finite),
            Some((false, 0, 0)),
        );
        let min_subnormal = Binary64::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            add_finite_aligned(min_subnormal, negative_min_subnormal),
            Some((false, 0, 0)),
        );
    }

    #[test]
    fn finite_addition_treats_zeros_as_neutral_and_preserves_inputs() {
        let one = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_zero = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        assert_eq!(
            add_finite_aligned(positive_zero, one),
            Some((false, 0, 0x8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(one, negative_zero),
            Some((false, 0, 0x8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(negative_zero, negative_one),
            Some((true, 0, 0x8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(negative_zero, positive_zero),
            Some((false, 0, 0))
        );

        let extreme_zero = BinaryFinite::new(true, 0, i32::MAX, BinaryClass::Zero);
        assert_eq!(
            add_finite_aligned(extreme_zero, one),
            Some((false, 0, 0x8000_0000_0000_0000)),
        );
        assert_eq!(
            add_finite_aligned(one, extreme_zero),
            Some((false, 0, 0x8000_0000_0000_0000)),
        );

        let subnormal = Binary64::from_bits(1).normalize_finite().unwrap();
        let normal = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        let subnormal_before = subnormal;
        let normal_before = normal;
        assert_eq!(subnormal.source_class(), BinaryClass::Subnormal);
        assert_eq!(normal.source_class(), BinaryClass::Normal);
        let _ = add_finite_aligned(subnormal, normal);
        assert_eq!(subnormal, subnormal_before);
        assert_eq!(normal, normal_before);
    }

    #[test]
    fn finite_multiplication_handles_signs_zeros_and_mixed_formats() {
        let positive = Binary32::from_bits(0x3fc0_0000).normalize_finite().unwrap();
        let negative = Binary32::from_bits(0xbfc0_0000).normalize_finite().unwrap();
        let product = 0x9000_0000_0000_0000_0000_0000_0000_0000;
        assert_eq!(
            multiply_finite_significands(positive, positive),
            (false, 1, product)
        );
        assert_eq!(
            multiply_finite_significands(positive, negative),
            (true, 1, product)
        );
        assert_eq!(
            multiply_finite_significands(negative, positive),
            (true, 1, product)
        );
        assert_eq!(
            multiply_finite_significands(negative, negative),
            (false, 1, product)
        );

        let positive_zero = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero = Binary64::from_bits(1 << 63).normalize_finite().unwrap();
        let one_b64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            multiply_finite_significands(positive_zero, one_b64),
            (false, 0, 0)
        );
        assert_eq!(
            multiply_finite_significands(one_b64, negative_zero),
            (false, 0, 0)
        );
        assert_eq!(
            multiply_finite_significands(positive_zero, negative_zero),
            (false, 0, 0)
        );

        let two_b64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            multiply_finite_significands(positive, two_b64),
            (false, 2, 0x6000_0000_0000_0000_0000_0000_0000_0000),
        );
        let min_subnormal_b32 = Binary32::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            multiply_finite_significands(min_subnormal_b32, positive),
            (false, -148, 0x6000_0000_0000_0000_0000_0000_0000_0000),
        );
    }

    #[test]
    fn finite_division_returns_exact_quotient_and_remainder() {
        let one_b32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let three_b32 = Binary32::from_bits(0x4040_0000).normalize_finite().unwrap();
        let two_b64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        assert_eq!(
            divide_finite_significands(one_b32, one_b32),
            Some(BinaryFiniteQuotient::new(
                false,
                0,
                0x8000_0000_0000_0000,
                0,
                0x8000_0000_0000_0000,
            )),
        );
        assert_eq!(
            divide_finite_significands(one_b32, three_b32),
            Some(BinaryFiniteQuotient::new(
                false,
                -2,
                0xaaaaaaaa_aaaaaaaa,
                0x8000_0000_0000_0000,
                0xc000_0000_0000_0000,
            )),
        );
        assert_eq!(
            divide_finite_significands(three_b32, two_b64),
            Some(BinaryFiniteQuotient::new(
                false,
                0,
                0xc000_0000_0000_0000,
                0,
                0x8000_0000_0000_0000,
            )),
        );

        let next_one_b64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            divide_finite_significands(one_b32, next_one_b64),
            Some(BinaryFiniteQuotient::new(
                false,
                -1,
                0xffff_ffff_ffff_f000,
                0x0000_0000_0080_0000,
                0x8000_0000_0000_0800,
            )),
        );

        let min_b64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let max_b64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            divide_finite_significands(min_b64, max_b64),
            Some(BinaryFiniteQuotient::new(
                false,
                -2098,
                0x8000_0000_0000_0400,
                0x0000_0000_0020_0000,
                0xffff_ffff_ffff_f800,
            )),
        );
        assert_eq!(
            divide_finite_significands(max_b64, min_b64),
            Some(BinaryFiniteQuotient::new(
                false,
                2097,
                0xffff_ffff_ffff_f800,
                0,
                0x8000_0000_0000_0000,
            )),
        );

        assert_division_identity(one_b32, one_b32);
        assert_division_identity(one_b32, three_b32);
        assert_division_identity(three_b32, two_b64);
        assert_division_identity(one_b32, next_one_b64);
        assert_division_identity(min_b64, max_b64);
        assert_division_identity(max_b64, min_b64);
    }

    #[test]
    fn finite_division_handles_signs_zeros_and_preserves_inputs() {
        let one = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive = divide_finite_significands(one, one).unwrap();
        let negative = divide_finite_significands(one, negative_one).unwrap();
        let negative_numerator = divide_finite_significands(negative_one, one).unwrap();
        let both_negative = divide_finite_significands(negative_one, negative_one).unwrap();
        assert!(!positive.sign());
        assert!(negative.sign());
        assert!(negative_numerator.sign());
        assert!(!both_negative.sign());
        assert_eq!(negative.exponent(), 0);
        assert_eq!(negative.quotient(), 0x8000_0000_0000_0000);
        assert_eq!(negative.remainder(), 0);
        assert_eq!(negative.denominator(), 0x8000_0000_0000_0000);
        assert_division_identity(one, one);
        assert_division_identity(one, negative_one);
        assert_division_identity(negative_one, one);
        assert_division_identity(negative_one, negative_one);

        let positive_zero = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero = Binary64::from_bits(1 << 63).normalize_finite().unwrap();
        let zero_over_one = divide_finite_significands(negative_zero, one).unwrap();
        assert_eq!(
            (
                zero_over_one.sign(),
                zero_over_one.exponent(),
                zero_over_one.quotient(),
                zero_over_one.remainder(),
                zero_over_one.denominator(),
            ),
            (false, 0, 0, 0, 0x8000_0000_0000_0000),
        );
        assert_division_identity(negative_zero, one);
        assert_eq!(
            divide_finite_significands(positive_zero, negative_zero),
            None
        );
        assert_eq!(divide_finite_significands(one, positive_zero), None);

        let raw_numerator = Binary64::from_bits(0x0000_0000_0000_0001);
        let raw_denominator = Binary32::from_bits(0x4040_0000);
        let numerator_before = raw_numerator.to_bits();
        let denominator_before = raw_denominator.to_bits();
        let finite_numerator = raw_numerator.normalize_finite().unwrap();
        let finite_denominator = raw_denominator.normalize_finite().unwrap();
        let _ = divide_finite_significands(finite_numerator, finite_denominator);
        assert_eq!(raw_numerator.to_bits(), numerator_before);
        assert_eq!(raw_denominator.to_bits(), denominator_before);
        assert_eq!(finite_numerator.exponent(), -1074);
        assert_eq!(finite_denominator.exponent(), 1);
    }

    #[test]
    fn finite_division_composes_once_to_normal_binary32_and_binary64() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let three32 = Binary32::from_bits(0x4040_0000).normalize_finite().unwrap();
        let three64 = Binary64::from_bits(0x4008_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        assert_eq!(
            Binary32::divide_finite(one32, one32, Nearest),
            Some((Binary32::from_bits(0x3f80_0000), false)),
        );
        assert_eq!(
            Binary64::divide_finite(one64, one64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), false)),
        );
        assert_eq!(
            Binary32::divide_finite(one32, three64, Nearest),
            Some((Binary32::from_bits(0x3eaa_aaab), true)),
        );
        assert_eq!(
            Binary64::divide_finite(one64, three32, Nearest),
            Some((Binary64::from_bits(0x3fd5_5555_5555_5555), true)),
        );
        assert_eq!(
            Binary64::divide_finite(one64, three64, Up),
            Some((Binary64::from_bits(0x3fd5_5555_5555_5556), true)),
        );

        for (mode, positive, negative) in [
            (Nearest, 0x3eaa_aaab, 0xbeaa_aaab),
            (Zero, 0x3eaa_aaaa, 0xbeaa_aaaa),
            (Up, 0x3eaa_aaab, 0xbeaa_aaaa),
            (Down, 0x3eaa_aaaa, 0xbeaa_aaab),
        ] {
            assert_eq!(
                Binary32::divide_finite(one32, three32, mode),
                Some((Binary32::from_bits(positive), true)),
            );
            assert_eq!(
                Binary32::divide_finite(negative_one32, three32, mode),
                Some((Binary32::from_bits(negative), true)),
            );
        }
        for (mode, positive, negative) in [
            (Nearest, 0x3fd5_5555_5555_5555, 0xbfd5_5555_5555_5555),
            (Zero, 0x3fd5_5555_5555_5555, 0xbfd5_5555_5555_5555),
            (Up, 0x3fd5_5555_5555_5556, 0xbfd5_5555_5555_5555),
            (Down, 0x3fd5_5555_5555_5555, 0xbfd5_5555_5555_5556),
        ] {
            assert_eq!(
                Binary64::divide_finite(one64, three64, mode),
                Some((Binary64::from_bits(positive), true)),
            );
            assert_eq!(
                Binary64::divide_finite(negative_one64, three64, mode),
                Some((Binary64::from_bits(negative), true)),
            );
        }
    }

    #[test]
    fn finite_division_composes_directly_to_subnormal_and_preserves_signed_zero() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        assert_eq!(
            Binary32::divide_finite(min_normal32, two32, Nearest),
            Some((Binary32::from_bits(0x0040_0000), false)),
        );
        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        for (mode, positive, negative) in [
            (Nearest, 0, 0x8000_0000),
            (Zero, 0, 0x8000_0000),
            (Up, 1, 0x8000_0000),
            (Down, 0, 0x8000_0001),
        ] {
            assert_eq!(
                Binary32::divide_finite(min_subnormal32, two32, mode),
                Some((Binary32::from_bits(positive), true)),
            );
            assert_eq!(
                Binary32::divide_finite(
                    Binary32::from_bits(0x8000_0001).normalize_finite().unwrap(),
                    two32,
                    mode,
                ),
                Some((Binary32::from_bits(negative), true)),
            );
        }
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let max_subnormal32 = Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap();
        assert_eq!(
            Binary32::divide_finite(max_subnormal32, one32, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), false)),
        );

        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::divide_finite(min_normal64, two64, Nearest),
            Some((Binary64::from_bits(0x0008_0000_0000_0000), false)),
        );
        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        for (mode, positive, negative) in [
            (Nearest, 0, 0x8000_0000_0000_0000),
            (Zero, 0, 0x8000_0000_0000_0000),
            (Up, 1, 0x8000_0000_0000_0000),
            (Down, 0, 0x8000_0000_0000_0001),
        ] {
            assert_eq!(
                Binary64::divide_finite(min_subnormal64, two64, mode),
                Some((Binary64::from_bits(positive), true)),
            );
            assert_eq!(
                Binary64::divide_finite(
                    Binary64::from_bits(0x8000_0000_0000_0001)
                        .normalize_finite()
                        .unwrap(),
                    two64,
                    mode,
                ),
                Some((Binary64::from_bits(negative), true)),
            );
        }
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let max_subnormal64 = Binary64::from_bits(0x000f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::divide_finite(max_subnormal64, one64, Nearest),
            Some((Binary64::from_bits(0x000f_ffff_ffff_ffff), false)),
        );
    }

    #[test]
    fn finite_division_rejects_zero_and_out_of_normal_range_without_mutation() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        let zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        for mode in modes {
            assert_eq!(Binary32::divide_finite(zero32, zero32, mode), None);
            assert_eq!(Binary32::divide_finite(one32, zero32, mode), None);
            assert_eq!(Binary64::divide_finite(zero32, negative_zero64, mode), None);
            assert_eq!(Binary64::divide_finite(one64, negative_zero64, mode), None);
        }

        let max32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let half32 = Binary32::from_bits(0x3f00_0000).normalize_finite().unwrap();
        let max64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let half64 = Binary64::from_bits(0x3fe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let min64 = Binary64::from_bits(1).normalize_finite().unwrap();
        for mode in modes {
            assert_eq!(Binary32::divide_finite(max32, half32, mode), None);
            assert_eq!(Binary64::divide_finite(max64, half64, mode), None);
            assert_eq!(
                Binary32::divide_finite(max32, one32, mode),
                Some((Binary32::from_bits(0x7f7f_ffff), false)),
            );
            assert_eq!(
                Binary64::divide_finite(max64, one64, mode),
                Some((Binary64::from_bits(0x7fef_ffff_ffff_ffff), false)),
            );
        }
        assert_eq!(
            Binary32::divide_finite(min32, max64, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary32::divide_finite(min32, max64, Up),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary64::divide_finite(min64, max32, Nearest),
            Some((Binary64::from_bits(0), true)),
        );
        assert_eq!(
            Binary64::divide_finite(min64, max32, Up),
            Some((Binary64::from_bits(1), true)),
        );
        assert_eq!(Binary32::divide_finite(max64, min32, Nearest), None,);
        assert_eq!(Binary64::divide_finite(max32, min64, Nearest), None,);

        let lhs_raw = Binary64::from_bits(0x380f_ffff_dfff_ffff);
        let rhs_raw = Binary64::from_bits(0x3ff0_0000_0000_0000);
        let lhs_before = lhs_raw.to_bits();
        let rhs_before = rhs_raw.to_bits();
        let lhs = lhs_raw.normalize_finite().unwrap();
        let rhs = rhs_raw.normalize_finite().unwrap();
        assert_eq!(
            Binary32::divide_finite(lhs, rhs, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), true)),
        );
        assert_eq!(lhs_raw.to_bits(), lhs_before);
        assert_eq!(rhs_raw.to_bits(), rhs_before);
    }

    #[test]
    fn finite_multiplication_composes_once_to_normal_precision() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardPositive as Up};

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::multiply_finite(one32, one32, Nearest),
            Some((Binary32::from_bits(0x3f80_0000), false)),
        );
        assert_eq!(
            Binary64::multiply_finite(one64, one64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), false)),
        );
        assert_eq!(
            Binary32::multiply_finite(one32, one64, Nearest),
            Some((Binary32::from_bits(0x3f80_0000), false)),
        );
        assert_eq!(
            Binary64::multiply_finite(one32, one64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), false)),
        );

        let one_and_half32 = Binary32::from_bits(0x3fc0_0000).normalize_finite().unwrap();
        let negative_one_and_half32 = Binary32::from_bits(0xbfc0_0000).normalize_finite().unwrap();
        let one_and_half64 = Binary64::from_bits(0x3ff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one_and_half64 = Binary64::from_bits(0xbff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::multiply_finite(one_and_half32, one_and_half32, Nearest),
            Some((Binary32::from_bits(0x4010_0000), false)),
        );
        assert_eq!(
            Binary64::multiply_finite(one_and_half64, one_and_half64, Nearest),
            Some((Binary64::from_bits(0x4002_0000_0000_0000), false)),
        );
        for (lhs, rhs, sign) in [
            (one_and_half32, one_and_half32, false),
            (one_and_half32, negative_one_and_half32, true),
            (negative_one_and_half32, one_and_half32, true),
            (negative_one_and_half32, negative_one_and_half32, false),
        ] {
            let (result, inexact) = Binary32::multiply_finite(lhs, rhs, Nearest).unwrap();
            assert_eq!(
                result.to_bits(),
                if sign { 0xc010_0000 } else { 0x4010_0000 }
            );
            assert!(!inexact);
        }
        for (lhs, rhs, sign) in [
            (one_and_half64, one_and_half64, false),
            (one_and_half64, negative_one_and_half64, true),
            (negative_one_and_half64, one_and_half64, true),
            (negative_one_and_half64, negative_one_and_half64, false),
        ] {
            let (result, inexact) = Binary64::multiply_finite(lhs, rhs, Nearest).unwrap();
            assert_eq!(
                result.to_bits(),
                if sign {
                    0xc002_0000_0000_0000
                } else {
                    0x4002_0000_0000_0000
                }
            );
            assert!(!inexact);
        }

        let next_one32 = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(next_one32, next_one32, Nearest),
            Some((Binary32::from_bits(0x3f80_0002), true)),
        );
        assert_eq!(
            Binary32::multiply_finite(next_one32, next_one32, Up),
            Some((Binary32::from_bits(0x3f80_0003), true)),
        );
        let next_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(next_one64, next_one64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0002), true)),
        );
        assert_eq!(
            Binary64::multiply_finite(next_one64, next_one64, Up),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0003), true)),
        );

        let lhs = Binary64::from_bits(0x3ff0_0000_01ff_ffff)
            .normalize_finite()
            .unwrap();
        let rhs = Binary64::from_bits(0x3ff0_0000_0400_0002)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(lhs, rhs, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0600_0001), true)),
        );
    }

    #[test]
    fn finite_multiplication_composes_directly_to_subnormal_precision() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
        };

        let half32 = Binary32::from_bits(0x3f00_0000).normalize_finite().unwrap();
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(min_normal32, half32, Nearest),
            Some((Binary32::from_bits(0x0040_0000), false)),
        );
        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(min_subnormal32, half32, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        let negative_min_subnormal32 = Binary32::from_bits(0x8000_0001).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(negative_min_subnormal32, half32, Nearest),
            Some((Binary32::from_bits(0x8000_0000), true)),
        );
        assert_eq!(
            Binary32::multiply_finite(min_subnormal32, half32, Up),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::multiply_finite(negative_min_subnormal32, half32, Down),
            Some((Binary32::from_bits(0x8000_0001), true)),
        );

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let max_subnormal32 = Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(max_subnormal32, one32, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), false)),
        );
        let max_finite32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(max_finite32, one32, Nearest),
            Some((Binary32::from_bits(0x7f7f_ffff), false)),
        );
        assert_eq!(
            Binary32::multiply_finite(max_finite32, two32, Nearest),
            None,
        );

        let half64 = Binary64::from_bits(0x3fe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(min_normal64, half64, Nearest),
            Some((Binary64::from_bits(0x0008_0000_0000_0000), false)),
        );
        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        assert_eq!(
            Binary64::multiply_finite(min_subnormal64, half64, Nearest),
            Some((Binary64::from_bits(0), true)),
        );
        let negative_min_subnormal64 = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(negative_min_subnormal64, half64, Nearest),
            Some((Binary64::from_bits(0x8000_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::multiply_finite(min_subnormal64, half64, Up),
            Some((Binary64::from_bits(1), true)),
        );
        assert_eq!(
            Binary64::multiply_finite(negative_min_subnormal64, half64, Down),
            Some((Binary64::from_bits(0x8000_0000_0000_0001), true)),
        );

        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let max_subnormal64 = Binary64::from_bits(0x000f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(max_subnormal64, one64, Nearest),
            Some((Binary64::from_bits(0x000f_ffff_ffff_ffff), false)),
        );
        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::multiply_finite(max_finite64, one64, Nearest),
            Some((Binary64::from_bits(0x7fef_ffff_ffff_ffff), false)),
        );
        assert_eq!(
            Binary64::multiply_finite(max_finite64, two64, Nearest),
            None,
        );

        assert_eq!(
            Binary32::multiply_finite(min_subnormal32, min_subnormal32, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary64::multiply_finite(min_subnormal64, min_subnormal64, Nearest),
            Some((Binary64::from_bits(0), true)),
        );
    }

    #[test]
    fn finite_multiplication_rejects_zero_preserves_inputs_and_avoids_subnormal_double_rounding() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(Binary32::multiply_finite(zero32, zero32, Nearest), None);
        assert_eq!(Binary32::multiply_finite(one32, zero32, Nearest), None);
        assert_eq!(
            Binary64::multiply_finite(zero32, negative_zero64, Nearest),
            None
        );
        assert_eq!(
            Binary64::multiply_finite(one64, negative_zero64, Nearest),
            None
        );
        assert_eq!(
            Binary64::multiply_finite(negative_zero64, one32, Nearest),
            None
        );

        let subnormal_edge_raw = Binary64::from_bits(0x380f_ffff_dfff_ffff);
        let one_raw = Binary64::from_bits(0x3ff0_0000_0000_0000);
        let edge_before = subnormal_edge_raw.to_bits();
        let one_before = one_raw.to_bits();
        let edge = subnormal_edge_raw.normalize_finite().unwrap();
        let one = one_raw.normalize_finite().unwrap();
        assert_eq!(
            Binary32::multiply_finite(edge, one, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), true)),
        );

        let next_one32_raw = Binary32::from_bits(0x3f80_0001);
        let next_one64_raw = Binary64::from_bits(0x3ff0_0000_0000_0001);
        let lhs_raw = Binary64::from_bits(0x3ff0_0000_01ff_ffff);
        let rhs_raw = Binary64::from_bits(0x3ff0_0000_0400_0002);
        let before = [
            next_one32_raw.to_bits() as u64,
            next_one64_raw.to_bits(),
            lhs_raw.to_bits(),
            rhs_raw.to_bits(),
        ];
        let _ = Binary32::multiply_finite(
            next_one32_raw.normalize_finite().unwrap(),
            next_one32_raw.normalize_finite().unwrap(),
            Nearest,
        );
        let _ = Binary64::multiply_finite(
            next_one64_raw.normalize_finite().unwrap(),
            next_one64_raw.normalize_finite().unwrap(),
            Nearest,
        );
        let _ = Binary64::multiply_finite(
            lhs_raw.normalize_finite().unwrap(),
            rhs_raw.normalize_finite().unwrap(),
            Nearest,
        );
        assert_eq!(
            [
                next_one32_raw.to_bits() as u64,
                next_one64_raw.to_bits(),
                lhs_raw.to_bits(),
                rhs_raw.to_bits(),
            ],
            before,
        );
        assert_eq!(subnormal_edge_raw.to_bits(), edge_before);
        assert_eq!(one_raw.to_bits(), one_before);
    }

    #[test]
    fn finite_addition_composes_once_to_normal_binary32_and_binary64() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(one32, one32, Nearest),
            Some((Binary32::from_bits(0x4000_0000), false)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(one64, one64, Nearest),
            Some((Binary64::from_bits(0x4000_0000_0000_0000), false)),
        );

        let one_and_half = Binary64::from_bits(0x3ff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(one_and_half, negative_one64, Nearest),
            Some((Binary32::from_bits(0x3f00_0000), false)),
        );

        let successor_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let negative_successor_one64 = Binary64::from_bits(0xbff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::add_finite_aligned(successor_one64, negative_one64, Nearest),
            Some((Binary64::from_bits(0x3cb0_0000_0000_0000), false)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(negative_successor_one64, one64, Nearest),
            Some((Binary64::from_bits(0xbcb0_0000_0000_0000), false)),
        );

        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_64 = Binary64::from_bits(0xbbf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        for (mode, expected_positive, expected_below) in [
            (Nearest, 0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000),
            (Zero, 0x3ff0_0000_0000_0000, 0x3fef_ffff_ffff_ffff),
            (Up, 0x3ff0_0000_0000_0001, 0x3ff0_0000_0000_0000),
            (Down, 0x3ff0_0000_0000_0000, 0x3fef_ffff_ffff_ffff),
        ] {
            assert_eq!(
                Binary64::add_finite_aligned(one64, two_to_minus_64, mode),
                Some((Binary64::from_bits(expected_positive), true)),
            );
            assert_eq!(
                Binary64::add_finite_aligned(one64, negative_two_to_minus_64, mode),
                Some((Binary64::from_bits(expected_below), true)),
            );
        }

        let tail_above_binary32_midpoint = Binary64::from_bits(0x3e70_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(one64, tail_above_binary32_midpoint, Nearest),
            Some((Binary32::from_bits(0x3f80_0001), true)),
        );

        let min_normal_plus_halfway = Binary64::from_bits(0x3810_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_halfway = Binary64::from_bits(0xb690_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(min_normal_plus_halfway, negative_halfway, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), true)),
        );

        let two_to_minus_149 = Binary64::from_bits(0x36a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_150 = Binary64::from_bits(0xb690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_sum =
            Binary32::add_finite_aligned(two_to_minus_149, negative_two_to_minus_150, Nearest)
                .unwrap();
        assert_eq!(positive_sum, (Binary32::from_bits(0), true));
        let negative_sum = Binary32::add_finite_aligned(
            Binary64::from_bits(0xb6a0_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            Binary64::from_bits(0x3690_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            Nearest,
        )
        .unwrap();
        assert_eq!(negative_sum, (Binary32::from_bits(0x8000_0000), true));

        // Directed modes remain mathematical rounding only; the returned bit
        // sign follows each nonzero exact sum even when its magnitude is zero.
        assert_eq!(
            Binary32::add_finite_aligned(two_to_minus_149, negative_two_to_minus_150, Up),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::add_finite_aligned(
                Binary64::from_bits(0xb6a0_0000_0000_0000)
                    .normalize_finite()
                    .unwrap(),
                Binary64::from_bits(0x3690_0000_0000_0000)
                    .normalize_finite()
                    .unwrap(),
                Down,
            ),
            Some((Binary32::from_bits(0x8000_0001), true)),
        );

        let one32_raw = Binary32::from_bits(0x3f80_0000);
        assert_eq!(
            Binary64::add_finite_aligned(
                one32_raw.normalize_finite().unwrap(),
                successor_one64,
                Nearest,
            ),
            Some((Binary64::from_bits(0x4000_0000_0000_0000), true)),
        );
    }

    #[test]
    fn finite_addition_composes_directly_at_subnormal_boundaries() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let negative_max_subnormal32 = Binary32::from_bits(0x807f_ffff).normalize_finite().unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(min_normal32, negative_max_subnormal32, Nearest),
            Some((Binary32::from_bits(1), false)),
        );

        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_max_subnormal64 = Binary64::from_bits(0x800f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::add_finite_aligned(min_normal64, negative_max_subnormal64, Nearest),
            Some((Binary64::from_bits(1), false)),
        );
    }

    #[test]
    fn finite_addition_composition_rejects_zero_cancellation_and_wide_gaps() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardPositive as Up};

        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_zero = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(positive_zero, negative_zero, Nearest),
            None
        );
        assert_eq!(
            Binary32::add_finite_aligned(one64, negative_one64, Nearest),
            None
        );
        let successor_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(positive_zero, successor_one64, Nearest),
            Some((Binary32::from_bits(0x3f80_0000), true)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(negative_zero, successor_one64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0001), false)),
        );

        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_two_to_minus_64 = Binary64::from_bits(0xbbf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_65 = Binary64::from_bits(0x3be0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::add_finite_aligned(one64, two_to_minus_64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(one64, two_to_minus_64, Up),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0001), true)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(one64, negative_two_to_minus_64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(one64, negative_two_to_minus_64, Up),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::add_finite_aligned(one64, two_to_minus_65, Nearest),
            None
        );
        assert_eq!(
            Binary64::add_finite_aligned(two_to_minus_65, one64, Nearest),
            None
        );

        let max_finite32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(max_finite32, max_finite32, Nearest),
            None
        );
        assert_eq!(
            Binary64::add_finite_aligned(max_finite64, max_finite64, Nearest),
            None
        );

        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(negative_zero, negative_one32, Nearest),
            Some((Binary32::from_bits(0xbf80_0000), false)),
        );

        let lhs_raw = Binary64::from_bits(0x3ff0_0000_0000_0001);
        let rhs_raw = Binary64::from_bits(0x3c9f_ffff_ffff_ffff);
        let lhs_before = lhs_raw.to_bits();
        let rhs_before = rhs_raw.to_bits();
        assert_eq!(
            Binary64::add_finite_aligned(
                lhs_raw.normalize_finite().unwrap(),
                rhs_raw.normalize_finite().unwrap(),
                Nearest,
            ),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0001), true)),
        );
        assert_eq!(lhs_raw.to_bits(), lhs_before);
        assert_eq!(rhs_raw.to_bits(), rhs_before);
    }

    #[test]
    fn finite_subtraction_composes_exact_normal_results() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one_and_half32 = Binary32::from_bits(0x3fc0_0000).normalize_finite().unwrap();
        let one_and_half64 = Binary64::from_bits(0x3ff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::subtract_finite_aligned(one_and_half32, one32, Nearest),
            Some((Binary32::from_bits(0x3f00_0000), false)),
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(one_and_half64, one64, Nearest),
            Some((Binary64::from_bits(0x3fe0_0000_0000_0000), false)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(one32, one_and_half32, Nearest),
            Some((Binary32::from_bits(0xbf00_0000), false)),
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(one64, one_and_half64, Nearest),
            Some((Binary64::from_bits(0xbfe0_0000_0000_0000), false)),
        );

        let successor_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(successor_one64, one64, Nearest),
            Some((Binary64::from_bits(0x3cb0_0000_0000_0000), false)),
        );
        let negative_successor_one64 = Binary64::from_bits(0xbff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(negative_one64, negative_successor_one64, Nearest),
            Some((Binary64::from_bits(0x3cb0_0000_0000_0000), false)),
        );

        assert_eq!(
            Binary32::subtract_finite_aligned(one32, negative_one64, Nearest),
            Some((Binary32::from_bits(0x4000_0000), false)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(
                Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap(),
                one64,
                Nearest,
            ),
            Some((Binary32::from_bits(0xc000_0000), false)),
        );
    }

    #[test]
    fn finite_subtraction_composes_at_subnormal_boundaries_and_avoids_double_rounding() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let max_subnormal32 = Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap();
        assert_eq!(
            Binary32::subtract_finite_aligned(min_normal32, max_subnormal32, Nearest),
            Some((Binary32::from_bits(1), false)),
        );
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let max_subnormal64 = Binary64::from_bits(0x000f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(min_normal64, max_subnormal64, Nearest),
            Some((Binary64::from_bits(1), false)),
        );

        let lhs_normal_round = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let rhs_negative_tail = Binary64::from_bits(0xbc9f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(lhs_normal_round, rhs_negative_tail, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0001), true)),
        );

        let lhs_subnormal_round = Binary64::from_bits(0x3810_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let rhs_positive_tail = Binary64::from_bits(0x3690_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::subtract_finite_aligned(lhs_subnormal_round, rhs_positive_tail, Nearest),
            Some((Binary32::from_bits(0x007f_ffff), true)),
        );
    }

    #[test]
    fn finite_subtraction_handles_zero_signs_rounding_modes_gaps_and_inputs() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        let positive_zero = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();

        for mode in modes {
            for (lhs, rhs) in [(one32, one32), (negative_one32, negative_one32)] {
                assert_eq!(Binary32::subtract_finite_aligned(lhs, rhs, mode), None);
                assert_eq!(Binary64::subtract_finite_aligned(lhs, rhs, mode), None);
            }
            for (lhs, rhs) in [(one64, one64), (negative_one64, negative_one64)] {
                assert_eq!(Binary32::subtract_finite_aligned(lhs, rhs, mode), None);
                assert_eq!(Binary64::subtract_finite_aligned(lhs, rhs, mode), None);
            }
            for (lhs, rhs) in [
                (positive_zero, positive_zero),
                (positive_zero, negative_zero),
                (negative_zero, positive_zero),
                (negative_zero, negative_zero),
            ] {
                assert_eq!(Binary32::subtract_finite_aligned(lhs, rhs, mode), None);
                assert_eq!(Binary64::subtract_finite_aligned(lhs, rhs, mode), None);
            }
            assert_eq!(
                Binary32::subtract_finite_aligned(one32, positive_zero, mode),
                Some((Binary32::from_bits(0x3f80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(one32, negative_zero, mode),
                Some((Binary32::from_bits(0x3f80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(negative_one32, negative_zero, mode),
                Some((Binary32::from_bits(0xbf80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(positive_zero, one64, mode),
                Some((Binary32::from_bits(0xbf80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(negative_zero, one64, mode),
                Some((Binary32::from_bits(0xbf80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(positive_zero, negative_one64, mode),
                Some((Binary32::from_bits(0x3f80_0000), false)),
            );
            assert_eq!(
                Binary32::subtract_finite_aligned(negative_zero, negative_one64, mode),
                Some((Binary32::from_bits(0x3f80_0000), false)),
            );
        }

        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_65 = Binary64::from_bits(0x3be0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(one64, two_to_minus_64, Nearest),
            Some((Binary64::from_bits(0x3ff0_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(two_to_minus_64, one64, Nearest),
            Some((Binary64::from_bits(0xbff0_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(one64, two_to_minus_65, Nearest),
            None
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(two_to_minus_65, one64, Nearest),
            None
        );

        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let negative_max_finite64 = Binary64::from_bits(0xffef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary64::subtract_finite_aligned(max_finite64, negative_max_finite64, Nearest),
            None,
        );

        let positive_tiny = Binary64::from_bits(0x36a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_half_tiny = Binary64::from_bits(0xb690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_half_tiny = Binary64::from_bits(0x3690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_tiny = Binary64::from_bits(0xb6a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::subtract_finite_aligned(positive_tiny, positive_half_tiny, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(positive_half_tiny, positive_tiny, Nearest),
            Some((Binary32::from_bits(0x8000_0000), true)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(positive_tiny, positive_half_tiny, Zero),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(positive_half_tiny, positive_tiny, Zero),
            Some((Binary32::from_bits(0x8000_0000), true)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(positive_tiny, positive_half_tiny, Up),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(negative_tiny, negative_half_tiny, Down),
            Some((Binary32::from_bits(0x8000_0001), true)),
        );

        let lhs_raw = Binary64::from_bits(0x3ff0_0000_0000_0001);
        let rhs_raw = Binary64::from_bits(0x3c9f_ffff_ffff_ffff);
        let lhs_before = lhs_raw.to_bits();
        let rhs_before = rhs_raw.to_bits();
        let _ = Binary64::subtract_finite_aligned(
            lhs_raw.normalize_finite().unwrap(),
            rhs_raw.normalize_finite().unwrap(),
            Nearest,
        );
        assert_eq!(lhs_raw.to_bits(), lhs_before);
        assert_eq!(rhs_raw.to_bits(), rhs_before);
    }

    #[test]
    fn quotient_precision_rounding_matches_literal_vectors() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let cases = [
            (0, 0, 1, 24, 0, false),
            (0x8000_0000_0000_0000, 0, 1, 24, 0x0080_0000, false),
            (0x8000_0080_0000_0000, 0, 1, 24, 0x0080_0000, true),
            (0x8000_0180_0000_0000, 0, 1, 24, 0x0080_0002, true),
            (0x8000_0000_0000_0400, 0, 1, 53, 0x0010_0000_0000_0000, true),
            (0x8000_0000_0000_0c00, 0, 1, 53, 0x0010_0000_0000_0002, true),
            (0x8000_0000_0000_0001, 1, 4, 63, 0x4000_0000_0000_0001, true),
            (0x8000_0000_0000_0002, 1, 2, 64, 0x8000_0000_0000_0002, true),
            (0x8000_0000_0000_0003, 1, 2, 64, 0x8000_0000_0000_0004, true),
            (u64::MAX, 1, 2, 64, 1_u128 << 64, true),
            (0xbfff_ffff_ffff_ffff, u64::MAX - 1, u64::MAX, 1, 1, true),
            (0xc000_0000_0000_0000, 0, 1, 1, 2, true),
        ];

        for (q, r, d, precision, expected_magnitude, expected_inexact) in cases {
            assert_eq!(
                round_quotient_to_precision(false, q, r, d, precision, Nearest),
                Some((expected_magnitude, expected_inexact)),
                "q={q:#018x} r={r} d={d} p={precision}"
            );
        }
    }

    #[test]
    fn quotient_precision_rounding_handles_modes_tail_and_invalid_inputs() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        for sign in [false, true] {
            for (mode, expected) in [
                (Nearest, 2),
                (Zero, 2),
                (Up, if sign { 2 } else { 3 }),
                (Down, if sign { 3 } else { 2 }),
            ] {
                assert_eq!(
                    round_quotient_to_precision(sign, 0xa000_0000_0000_0000, 0, 1, 2, mode,),
                    Some((expected, true)),
                    "sign={sign} mode={mode:?}"
                );
            }
        }

        let q = 0x8000_0000_0000_0000;
        assert_eq!(
            round_quotient_to_precision(false, q, 1, 4, 64, Nearest),
            Some((u128::from(q), true)),
        );
        assert_eq!(
            round_quotient_to_precision(false, q, 0, 4, 24, Nearest),
            Some((0x0080_0000, false)),
        );
        for mode in modes {
            for precision in [0, 65, u32::MAX] {
                assert_eq!(
                    round_quotient_to_precision(false, 0, 0, 1, precision, mode),
                    None,
                );
            }
            assert_eq!(round_quotient_to_precision(false, 7, 0, 0, 24, mode), None);
            assert_eq!(round_quotient_to_precision(false, 7, 1, 1, 24, mode), None);
            assert_eq!(round_quotient_to_precision(false, 1, 0, 1, 24, mode), None);
            assert_eq!(round_quotient_to_precision(false, 0, 1, 2, 24, mode), None);
        }
    }

    #[test]
    fn quotient_precision_rounding_composes_with_binary_division() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let one = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let three = Binary64::from_bits(0x4008_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let quotient = divide_finite_significands(one, three).unwrap();
        assert_eq!(
            round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                24,
                Nearest,
            ),
            Some((0x00aa_aaab, true)),
        );
        assert_eq!(
            round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                53,
                Nearest,
            ),
            Some((0x0015_5555_5555_5555, true)),
        );
        assert_eq!(
            round_quotient_to_precision(
                quotient.sign(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                64,
                Nearest,
            ),
            Some((0xaaaa_aaaa_aaaa_aaab, true)),
        );
    }

    #[test]
    fn normal_packers_match_binary32_and_binary64_literal_vectors() {
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, -126, 1 << 23)
                .unwrap()
                .to_bits(),
            0x0080_0000,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, 0, 1 << 23)
                .unwrap()
                .to_bits(),
            0x3f80_0000,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(true, 0, 1 << 23)
                .unwrap()
                .to_bits(),
            0xbf80_0000,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, 127, (1 << 24) - 1)
                .unwrap()
                .to_bits(),
            0x7f7f_ffff,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, -127, 1 << 24)
                .unwrap()
                .to_bits(),
            0x0080_0000,
        );

        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, -1022, 1 << 52)
                .unwrap()
                .to_bits(),
            0x0010_0000_0000_0000,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, 0, 1 << 52)
                .unwrap()
                .to_bits(),
            0x3ff0_0000_0000_0000,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(true, 0, 1 << 52)
                .unwrap()
                .to_bits(),
            0xbff0_0000_0000_0000,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, 1023, (1 << 53) - 1)
                .unwrap()
                .to_bits(),
            0x7fef_ffff_ffff_ffff,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, -1023, 1 << 53)
                .unwrap()
                .to_bits(),
            0x0010_0000_0000_0000,
        );
    }

    #[test]
    fn normal_packers_reject_out_of_profile_inputs_without_overflow() {
        for magnitude in [0, (1 << 23) - 1, (1 << 24) + 1, u128::MAX] {
            assert_eq!(
                Binary32::pack_normal_rounded_significand(false, 0, magnitude),
                None,
            );
        }
        for magnitude in [0, (1 << 52) - 1, (1 << 53) + 1, u128::MAX] {
            assert_eq!(
                Binary64::pack_normal_rounded_significand(false, 0, magnitude),
                None,
            );
        }

        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, -127, 1 << 23),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, -128, 1 << 24),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, 127, 1 << 24),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, 128, (1 << 24) - 1),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, i32::MIN, 1 << 23),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, i32::MIN, 1 << 24),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, i32::MAX, 1 << 23),
            None,
        );
        assert_eq!(
            Binary32::pack_normal_rounded_significand(false, i32::MAX, 1 << 24),
            None,
        );

        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, -1023, 1 << 52),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, -1024, 1 << 53),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, 1023, 1 << 53),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, 1024, (1 << 53) - 1),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, i32::MIN, 1 << 52),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, i32::MIN, 1 << 53),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, i32::MAX, 1 << 52),
            None,
        );
        assert_eq!(
            Binary64::pack_normal_rounded_significand(false, i32::MAX, 1 << 53),
            None,
        );
    }

    #[test]
    fn normal_packers_compose_with_rounding_without_consuming_inexactness() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardNegative as Down};

        let one = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let three_b32 = Binary32::from_bits(0x4040_0000).normalize_finite().unwrap();
        let one_quotient = divide_finite_significands(one, one).unwrap();
        let (one_magnitude, one_inexact) = round_quotient_to_precision(
            one_quotient.sign(),
            one_quotient.quotient(),
            one_quotient.remainder(),
            one_quotient.denominator(),
            24,
            Nearest,
        )
        .unwrap();
        assert!(!one_inexact);
        assert_eq!(
            Binary32::pack_normal_rounded_significand(
                one_quotient.sign(),
                one_quotient.exponent(),
                one_magnitude,
            )
            .unwrap()
            .to_bits(),
            0x3f80_0000,
        );

        let three_b64 = Binary64::from_bits(0x4008_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let quotient = divide_finite_significands(one, three_b32).unwrap();
        let (magnitude32, inexact32) = round_quotient_to_precision(
            quotient.sign(),
            quotient.quotient(),
            quotient.remainder(),
            quotient.denominator(),
            24,
            Nearest,
        )
        .unwrap();
        assert!(inexact32);
        assert_eq!(
            Binary32::pack_normal_rounded_significand(
                quotient.sign(),
                quotient.exponent(),
                magnitude32,
            )
            .unwrap()
            .to_bits(),
            0x3eaa_aaab,
        );

        let quotient = divide_finite_significands(one, three_b64).unwrap();
        let (magnitude64, inexact64) = round_quotient_to_precision(
            quotient.sign(),
            quotient.quotient(),
            quotient.remainder(),
            quotient.denominator(),
            53,
            Nearest,
        )
        .unwrap();
        assert!(inexact64);
        assert_eq!(
            Binary64::pack_normal_rounded_significand(
                quotient.sign(),
                quotient.exponent(),
                magnitude64,
            )
            .unwrap()
            .to_bits(),
            0x3fd5_5555_5555_5555,
        );

        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_quotient = divide_finite_significands(negative_one, three_b64).unwrap();
        let (negative_magnitude, negative_inexact) = round_quotient_to_precision(
            negative_quotient.sign(),
            negative_quotient.quotient(),
            negative_quotient.remainder(),
            negative_quotient.denominator(),
            24,
            Down,
        )
        .unwrap();
        assert!(negative_quotient.sign());
        assert!(negative_inexact);
        assert_eq!(
            Binary32::pack_normal_rounded_significand(
                negative_quotient.sign(),
                negative_quotient.exponent(),
                negative_magnitude,
            )
            .unwrap()
            .to_bits(),
            0xbeaa_aaab,
        );
    }

    #[test]
    fn subnormal_quotient_packers_match_boundary_vectors() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let q = 0x8000_0000_0000_0000;
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -149, q, 0, 1, Nearest),
            Some((Binary32::from_bits(1), false)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -150, q, 0, 1, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -150, q, 1, 2, Nearest),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -151, u64::MAX, 0, 1, Nearest),
            Some((Binary32::from_bits(0), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -149,
                0xc000_0000_0000_0000,
                0,
                1,
                Nearest
            ),
            Some((Binary32::from_bits(2), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -148,
                0xa000_0000_0000_0000,
                0,
                1,
                Nearest
            ),
            Some((Binary32::from_bits(2), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -127,
                0xffff_fe00_0000_0000,
                0,
                1,
                Nearest
            ),
            Some((Binary32::from_bits(0x007f_ffff), false)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -127,
                0xffff_ff00_0000_0000,
                0,
                1,
                Nearest
            ),
            Some((Binary32::from_bits(0x0080_0000), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -127,
                0xffff_feff_ffff_ffff,
                0,
                1,
                Nearest
            ),
            Some((Binary32::from_bits(0x007f_ffff), true)),
        );

        assert_eq!(
            Binary64::round_pack_subnormal_quotient(false, -1074, q, 0, 1, Nearest),
            Some((Binary64::from_bits(1), false)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(false, -1075, q, 0, 1, Nearest),
            Some((Binary64::from_bits(0), true)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(false, -1075, q, 1, 2, Nearest),
            Some((Binary64::from_bits(1), true)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                false,
                -1023,
                0xffff_ffff_ffff_f000,
                0,
                1,
                Nearest
            ),
            Some((Binary64::from_bits(0x000f_ffff_ffff_ffff), false)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                false,
                -1023,
                0xffff_ffff_ffff_f800,
                0,
                1,
                Nearest
            ),
            Some((Binary64::from_bits(0x0010_0000_0000_0000), true)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                false,
                -1023,
                0xffff_ffff_ffff_f7ff,
                0,
                1,
                Nearest
            ),
            Some((Binary64::from_bits(0x000f_ffff_ffff_ffff), true)),
        );
    }

    #[test]
    fn subnormal_quotient_packers_handle_modes_ties_and_tails() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let q = 0x8000_0000_0000_0000;
        for (mode, positive_half, negative_half, positive_far, negative_far) in [
            (Nearest, 0, 0, 0, 0),
            (Zero, 0, 0, 0, 0),
            (Up, 1, 0, 1, 0),
            (Down, 0, 1, 0, 1),
        ] {
            assert_eq!(
                Binary32::round_pack_subnormal_quotient(false, -150, q, 0, 1, mode),
                Some((Binary32::from_bits(positive_half), true)),
            );
            assert_eq!(
                Binary32::round_pack_subnormal_quotient(true, -150, q, 0, 1, mode),
                Some((Binary32::from_bits(0x8000_0000 | negative_half), true)),
            );
            assert_eq!(
                Binary64::round_pack_subnormal_quotient(false, -1075, q, 0, 1, mode),
                Some((Binary64::from_bits(u64::from(positive_half)), true)),
            );
            assert_eq!(
                Binary64::round_pack_subnormal_quotient(true, -1075, q, 0, 1, mode),
                Some((
                    Binary64::from_bits(0x8000_0000_0000_0000 | u64::from(negative_half)),
                    true
                )),
            );

            assert_eq!(
                Binary32::round_pack_subnormal_quotient(false, -151, q, 0, 1, mode),
                Some((Binary32::from_bits(positive_far), true)),
            );
            assert_eq!(
                Binary32::round_pack_subnormal_quotient(true, -151, q, 0, 1, mode),
                Some((Binary32::from_bits(0x8000_0000 | negative_far), true)),
            );
        }

        for (exponent, expected) in [(-149, 2), (-148, 2)] {
            let tie_q = if exponent == -149 {
                0xc000_0000_0000_0000
            } else {
                0xa000_0000_0000_0000
            };
            assert_eq!(
                Binary32::round_pack_subnormal_quotient(false, exponent, tie_q, 0, 1, Nearest),
                Some((Binary32::from_bits(expected), true)),
            );
        }
        for (exponent, tie_q, expected) in [
            (-1074, 0xc000_0000_0000_0000, 2),
            (-1073, 0xa000_0000_0000_0000, 2),
        ] {
            assert_eq!(
                Binary64::round_pack_subnormal_quotient(false, exponent, tie_q, 0, 1, Nearest),
                Some((Binary64::from_bits(expected), true)),
            );
        }

        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -150, q, 1, 2, Nearest),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(false, -150, q, 1, u64::MAX, Nearest),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                false,
                -150,
                q,
                u64::MAX - 1,
                u64::MAX,
                Nearest
            ),
            Some((Binary32::from_bits(1), true)),
        );
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(false, i32::MIN, q, 0, 1, Nearest),
            Some((Binary64::from_bits(0), true)),
        );
    }

    #[test]
    fn subnormal_quotient_packers_reject_invalid_inputs_and_preserve_inputs() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        for (exponent, q, r, d) in [
            (-126, 0x8000_0000_0000_0000, 0, 1),
            (i32::MAX, 0x8000_0000_0000_0000, 0, 1),
            (-127, 0, 0, 1),
            (-127, 1, 0, 1),
            (-127, 0x8000_0000_0000_0000, 0, 0),
            (-127, 0x8000_0000_0000_0000, 1, 1),
        ] {
            assert_eq!(
                Binary32::round_pack_subnormal_quotient(false, exponent, q, r, d, Nearest),
                None
            );
        }
        for (exponent, q, r, d) in [
            (-1022, 0x8000_0000_0000_0000, 0, 1),
            (i32::MAX, 0x8000_0000_0000_0000, 0, 1),
            (-1023, 0, 0, 1),
            (-1023, 1, 0, 1),
            (-1023, 0x8000_0000_0000_0000, 0, 0),
            (-1023, 0x8000_0000_0000_0000, 1, 1),
        ] {
            assert_eq!(
                Binary64::round_pack_subnormal_quotient(false, exponent, q, r, d, Nearest),
                None
            );
        }

        let numerator = Binary64::from_bits(1);
        let denominator = Binary32::from_bits(0x4040_0000);
        let numerator_bits = numerator.to_bits();
        let denominator_bits = denominator.to_bits();
        let lhs = numerator.normalize_finite().unwrap();
        let rhs = denominator.normalize_finite().unwrap();
        let exact = divide_finite_significands(lhs, rhs).unwrap();
        let _ = Binary64::round_pack_subnormal_quotient(
            exact.sign(),
            exact.exponent(),
            exact.quotient(),
            exact.remainder(),
            exact.denominator(),
            Nearest,
        );
        assert_eq!(numerator.to_bits(), numerator_bits);
        assert_eq!(denominator.to_bits(), denominator_bits);
    }

    #[test]
    fn subnormal_quotient_packers_compose_with_exact_binary_division() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let half_min_normal32 = divide_finite_significands(min_normal32, two32).unwrap();
        assert_eq!(half_min_normal32.exponent(), -127);
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                half_min_normal32.sign(),
                half_min_normal32.exponent(),
                half_min_normal32.quotient(),
                half_min_normal32.remainder(),
                half_min_normal32.denominator(),
                Nearest,
            ),
            Some((Binary32::from_bits(0x0040_0000), false)),
        );
        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let half_min_subnormal32 = divide_finite_significands(min_subnormal32, two32).unwrap();
        assert_eq!(half_min_subnormal32.exponent(), -150);
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                half_min_subnormal32.sign(),
                half_min_subnormal32.exponent(),
                half_min_subnormal32.quotient(),
                half_min_subnormal32.remainder(),
                half_min_subnormal32.denominator(),
                Nearest,
            ),
            Some((Binary32::from_bits(0), true)),
        );
        let max_subnormal32 = Binary32::from_bits(0x007f_ffff).normalize_finite().unwrap();
        let max_subnormal_quotient32 = divide_finite_significands(max_subnormal32, one32).unwrap();
        assert_eq!(
            Binary32::round_pack_subnormal_quotient(
                max_subnormal_quotient32.sign(),
                max_subnormal_quotient32.exponent(),
                max_subnormal_quotient32.quotient(),
                max_subnormal_quotient32.remainder(),
                max_subnormal_quotient32.denominator(),
                Nearest,
            ),
            Some((Binary32::from_bits(0x007f_ffff), false)),
        );

        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let half_min_normal64 = divide_finite_significands(min_normal64, two64).unwrap();
        assert_eq!(half_min_normal64.exponent(), -1023);
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                half_min_normal64.sign(),
                half_min_normal64.exponent(),
                half_min_normal64.quotient(),
                half_min_normal64.remainder(),
                half_min_normal64.denominator(),
                Nearest,
            ),
            Some((Binary64::from_bits(0x0008_0000_0000_0000), false)),
        );
        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let half_min_subnormal64 = divide_finite_significands(min_subnormal64, two64).unwrap();
        assert_eq!(half_min_subnormal64.exponent(), -1075);
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                half_min_subnormal64.sign(),
                half_min_subnormal64.exponent(),
                half_min_subnormal64.quotient(),
                half_min_subnormal64.remainder(),
                half_min_subnormal64.denominator(),
                Nearest,
            ),
            Some((Binary64::from_bits(0), true)),
        );
        let max_subnormal64 = Binary64::from_bits(0x000f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let max_subnormal_quotient64 = divide_finite_significands(max_subnormal64, one64).unwrap();
        assert_eq!(
            Binary64::round_pack_subnormal_quotient(
                max_subnormal_quotient64.sign(),
                max_subnormal_quotient64.exponent(),
                max_subnormal_quotient64.quotient(),
                max_subnormal_quotient64.remainder(),
                max_subnormal_quotient64.denominator(),
                Nearest,
            ),
            Some((Binary64::from_bits(0x000f_ffff_ffff_ffff), false)),
        );
    }
}
