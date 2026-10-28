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

/// An encoded result and its mathematical rounding diagnostics.
///
/// `overflow` is true when rounding the exact value to the destination
/// precision with an unbounded exponent produces a magnitude greater than the
/// destination's maximum finite magnitude. `tiny_after_precision_rounding` is
/// true when that unbounded-exponent precision-rounded result is nonzero and
/// has magnitude below the minimum normal magnitude. `inexact` reports loss in
/// the final bounded encoding, including overflow results. These values are
/// mathematical diagnostics, not architectural exception flags; they do not
/// update CPU state or MXCSR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryRoundingResult<T> {
    value: T,
    inexact: bool,
    overflow: bool,
    tiny_after_precision_rounding: bool,
}

impl<T> BinaryRoundingResult<T> {
    const fn new(
        value: T,
        inexact: bool,
        overflow: bool,
        tiny_after_precision_rounding: bool,
    ) -> Self {
        Self {
            value,
            inexact,
            overflow,
            tiny_after_precision_rounding,
        }
    }

    /// Returns the destination-format encoding produced by the rounding.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns whether the final bounded encoding lost information.
    pub const fn inexact(&self) -> bool {
        self.inexact
    }

    /// Returns whether unbounded-exponent destination-precision rounding
    /// exceeds the destination's maximum finite magnitude.
    pub const fn overflow(&self) -> bool {
        self.overflow
    }

    /// Returns whether the nonzero unbounded-exponent destination-precision
    /// result has magnitude below the destination's minimum normal magnitude.
    pub const fn tiny_after_precision_rounding(&self) -> bool {
        self.tiny_after_precision_rounding
    }
}

/// Result of a same-format binary32/binary64 addition or subtraction.
///
/// The rounding diagnostics have the same local mathematical meaning as
/// [`BinaryRoundingResult`]. `invalid_operation` is a local operation outcome,
/// not an architectural exception flag or delivery decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryAddSubResult<T> {
    rounding: BinaryRoundingResult<T>,
    invalid_operation: bool,
}

impl<T> BinaryAddSubResult<T> {
    const fn new(rounding: BinaryRoundingResult<T>, invalid_operation: bool) -> Self {
        Self {
            rounding,
            invalid_operation,
        }
    }

    fn exact(value: T, invalid_operation: bool) -> Self {
        Self::new(
            BinaryRoundingResult::new(value, false, false, false),
            invalid_operation,
        )
    }

    /// Returns the destination-format result encoding.
    pub const fn value(&self) -> &T {
        self.rounding.value()
    }

    /// Returns whether the result lost information in bounded encoding.
    pub const fn inexact(&self) -> bool {
        self.rounding.inexact()
    }

    /// Returns whether precision rounding exceeds the destination's finite range.
    pub const fn overflow(&self) -> bool {
        self.rounding.overflow()
    }

    /// Returns whether the precision-rounded nonzero result is below normal range.
    pub const fn tiny_after_precision_rounding(&self) -> bool {
        self.rounding.tiny_after_precision_rounding()
    }

    /// Returns the local invalid-operation outcome for this add/sub operation.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }
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

    /// Rounds `(-1)^sign * (quotient + remainder/denominator) * 2^(exponent-63)` to binary32.
    ///
    /// Returns `None` for a zero denominator, out-of-range remainder, or a
    /// nonzero quotient that is not normalized. Exact zero is accepted and
    /// retains `sign`. The diagnostics describe the final bounded encoding,
    /// except `tiny_after_precision_rounding`, which measures the unbounded
    /// destination-precision result. No architectural state is updated.
    pub fn round_pack_quotient(
        sign: bool,
        exponent: i32,
        quotient: u64,
        remainder: u64,
        denominator: u64,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        let result = round_pack_quotient_bits(
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
            mode,
            24,
            -126,
            127,
            31,
            127,
            0x007f_ffff,
            0x7f80_0000,
            0x7f7f_ffff,
        )?;
        Some(BinaryRoundingResult::new(
            Self::from_bits(result.value as u32),
            result.inexact,
            result.overflow,
            result.tiny_after_precision_rounding,
        ))
    }

    /// Multiplies finite values and reports bounded binary32 rounding diagnostics.
    ///
    /// A zero input returns exact signed zero using the XOR sign. Nonzero
    /// inputs are multiplied exactly and rounded once; overflow uses the
    /// specified mode-dependent infinity or maximum-finite encoding.
    pub fn multiply_finite_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> BinaryRoundingResult<Self> {
        let sign = lhs.sign ^ rhs.sign;
        if lhs.significand == 0 || rhs.significand == 0 {
            return BinaryRoundingResult::new(
                Self::from_bits(u32::from(sign) << 31),
                false,
                false,
                false,
            );
        }
        let (sign, exponent, quotient, remainder, denominator) =
            finite_product_quotient(lhs, rhs).expect("normalized finite product bridge");
        Self::round_pack_quotient(sign, exponent, quotient, remainder, denominator, mode)
            .expect("normalized finite product quotient")
    }

    /// Divides finite values and reports bounded binary32 rounding diagnostics.
    ///
    /// A zero divisor returns `None`; a zero numerator returns exact signed
    /// zero using the XOR sign. Nonzero inputs are divided exactly and rounded
    /// once; overflow uses the specified mode-dependent infinity or
    /// maximum-finite encoding.
    pub fn divide_finite_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        if rhs.significand == 0 {
            return None;
        }
        let sign = lhs.sign ^ rhs.sign;
        if lhs.significand == 0 {
            return Some(BinaryRoundingResult::new(
                Self::from_bits(u32::from(sign) << 31),
                false,
                false,
                false,
            ));
        }
        let quotient =
            divide_finite_significands(lhs, rhs).expect("nonzero finite divisor has a quotient");
        Some(
            Self::round_pack_quotient(
                quotient.sign(),
                quotient.exponent(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                mode,
            )
            .expect("normalized finite quotient"),
        )
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

    /// Adds finite values and returns bounded binary32 rounding diagnostics.
    ///
    /// Zero operands are rounded from the original exact finite value so mixed
    /// source and destination formats are converted correctly. Exact zero uses
    /// the IEEE rounding-mode sign rule for cancellation; no architectural
    /// state or exception flags are updated.
    pub fn add_finite_aligned_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        if lhs.significand == 0 && rhs.significand == 0 {
            let sign = if lhs.sign == rhs.sign {
                lhs.sign
            } else {
                mode == RationalRoundingMode::TowardNegative
            };
            return Some(BinaryRoundingResult::new(
                Self::from_bits(u32::from(sign) << 31),
                false,
                false,
                false,
            ));
        }
        if lhs.significand == 0 || rhs.significand == 0 {
            let value = if lhs.significand == 0 { rhs } else { lhs };
            return Self::round_pack_quotient(
                value.sign,
                value.exponent,
                value.significand,
                0,
                1,
                mode,
            );
        }

        let (sign, exponent, quotient, remainder, denominator) =
            if finite_exponent_gap(lhs, rhs) <= 64 {
                let (sign, exponent, magnitude) = add_finite_aligned(lhs, rhs)?;
                if magnitude == 0 {
                    let sign = mode == RationalRoundingMode::TowardNegative;
                    return Some(BinaryRoundingResult::new(
                        Self::from_bits(u32::from(sign) << 31),
                        false,
                        false,
                        false,
                    ));
                }
                finite_sum_quotient(sign, exponent, magnitude)?
            } else {
                finite_large_gap_rounding_bridge(lhs, rhs)?
            };
        Self::round_pack_quotient(sign, exponent, quotient, remainder, denominator, mode)
    }

    /// Subtracts finite values by adding a copy of the right operand with its
    /// sign flipped, while returning bounded binary32 rounding diagnostics.
    pub fn subtract_finite_aligned_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        let negated_rhs =
            BinaryFinite::new(!rhs.sign, rhs.significand, rhs.exponent, rhs.source_class);
        Self::add_finite_aligned_result(lhs, negated_rhs, mode)
    }

    /// Adds same-format raw binary32 operands and returns result diagnostics.
    ///
    /// Finite operands use the finite result path. NaN propagation follows the
    /// Intel SDM Rev. 093 source-order rule: select the first NaN operand and
    /// quiet it only when that selected operand is signaling. The local
    /// invalid-operation result is set for any signaling NaN or invalid
    /// infinity cancellation; no architectural state is updated.
    pub fn add_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryAddSubResult<Self> {
        Self::add_sub_raw_result(lhs, rhs, mode, false)
    }

    /// Subtracts same-format raw binary32 operands and returns result diagnostics.
    ///
    /// NaN selection is performed from the original source operands before the
    /// finite/infinity subtraction sign is applied to the right operand.
    pub fn subtract_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryAddSubResult<Self> {
        Self::add_sub_raw_result(lhs, rhs, mode, true)
    }

    fn add_sub_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
        subtract: bool,
    ) -> BinaryAddSubResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        let lhs_nan = matches!(lhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        let rhs_nan = matches!(rhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        if lhs_nan || rhs_nan {
            let (selected, selected_class) = if lhs_nan {
                (lhs, lhs_class)
            } else {
                (rhs, rhs_class)
            };
            let value = if selected_class == BinaryClass::SignalingNaN {
                Self::from_bits(selected.0 | Self::QUIET_BIT)
            } else {
                selected
            };
            return BinaryAddSubResult::exact(
                value,
                lhs_class == BinaryClass::SignalingNaN || rhs_class == BinaryClass::SignalingNaN,
            );
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let effective_rhs_sign = rhs.sign() ^ subtract;
        if lhs_infinity && rhs_infinity {
            if lhs.sign() != effective_rhs_sign {
                return BinaryAddSubResult::exact(Self::from_bits(0xffc0_0000), true);
            }
            return BinaryAddSubResult::exact(lhs, false);
        }
        if lhs_infinity {
            return BinaryAddSubResult::exact(lhs, false);
        }
        if rhs_infinity {
            return BinaryAddSubResult::exact(
                Self::from_bits(
                    (Self::EXPONENT_MASK << 23) | (u32::from(effective_rhs_sign) << 31),
                ),
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary32 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary32 operand");
        let rounded = if subtract {
            Self::subtract_finite_aligned_result(lhs_finite, rhs_finite, mode)
        } else {
            Self::add_finite_aligned_result(lhs_finite, rhs_finite, mode)
        }
        .expect("finite binary32 add/sub result is supported");
        BinaryAddSubResult::new(rounded, false)
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

    /// Rounds `(-1)^sign * (quotient + remainder/denominator) * 2^(exponent-63)` to binary64.
    ///
    /// Returns `None` for a zero denominator, out-of-range remainder, or a
    /// nonzero quotient that is not normalized. Exact zero is accepted and
    /// retains `sign`. The diagnostics describe the final bounded encoding,
    /// except `tiny_after_precision_rounding`, which measures the unbounded
    /// destination-precision result. No architectural state is updated.
    pub fn round_pack_quotient(
        sign: bool,
        exponent: i32,
        quotient: u64,
        remainder: u64,
        denominator: u64,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        let result = round_pack_quotient_bits(
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
            mode,
            53,
            -1022,
            1023,
            63,
            1023,
            0x000f_ffff_ffff_ffff,
            0x7ff0_0000_0000_0000,
            0x7fef_ffff_ffff_ffff,
        )?;
        Some(BinaryRoundingResult::new(
            Self::from_bits(result.value),
            result.inexact,
            result.overflow,
            result.tiny_after_precision_rounding,
        ))
    }

    /// Multiplies finite values and reports bounded binary64 rounding diagnostics.
    ///
    /// A zero input returns exact signed zero using the XOR sign. Nonzero
    /// inputs are multiplied exactly and rounded once; overflow uses the
    /// specified mode-dependent infinity or maximum-finite encoding.
    pub fn multiply_finite_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> BinaryRoundingResult<Self> {
        let sign = lhs.sign ^ rhs.sign;
        if lhs.significand == 0 || rhs.significand == 0 {
            return BinaryRoundingResult::new(
                Self::from_bits(u64::from(sign) << 63),
                false,
                false,
                false,
            );
        }
        let (sign, exponent, quotient, remainder, denominator) =
            finite_product_quotient(lhs, rhs).expect("normalized finite product bridge");
        Self::round_pack_quotient(sign, exponent, quotient, remainder, denominator, mode)
            .expect("normalized finite product quotient")
    }

    /// Divides finite values and reports bounded binary64 rounding diagnostics.
    ///
    /// A zero divisor returns `None`; a zero numerator returns exact signed
    /// zero using the XOR sign. Nonzero inputs are divided exactly and rounded
    /// once; overflow uses the specified mode-dependent infinity or
    /// maximum-finite encoding.
    pub fn divide_finite_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        if rhs.significand == 0 {
            return None;
        }
        let sign = lhs.sign ^ rhs.sign;
        if lhs.significand == 0 {
            return Some(BinaryRoundingResult::new(
                Self::from_bits(u64::from(sign) << 63),
                false,
                false,
                false,
            ));
        }
        let quotient =
            divide_finite_significands(lhs, rhs).expect("nonzero finite divisor has a quotient");
        Some(
            Self::round_pack_quotient(
                quotient.sign(),
                quotient.exponent(),
                quotient.quotient(),
                quotient.remainder(),
                quotient.denominator(),
                mode,
            )
            .expect("normalized finite quotient"),
        )
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

    /// Adds finite values and returns bounded binary64 rounding diagnostics.
    ///
    /// Zero operands are rounded from the original exact finite value so mixed
    /// source and destination formats are converted correctly. Exact zero uses
    /// the IEEE rounding-mode sign rule for cancellation; no architectural
    /// state or exception flags are updated.
    pub fn add_finite_aligned_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        if lhs.significand == 0 && rhs.significand == 0 {
            let sign = if lhs.sign == rhs.sign {
                lhs.sign
            } else {
                mode == RationalRoundingMode::TowardNegative
            };
            return Some(BinaryRoundingResult::new(
                Self::from_bits(u64::from(sign) << 63),
                false,
                false,
                false,
            ));
        }
        if lhs.significand == 0 || rhs.significand == 0 {
            let value = if lhs.significand == 0 { rhs } else { lhs };
            return Self::round_pack_quotient(
                value.sign,
                value.exponent,
                value.significand,
                0,
                1,
                mode,
            );
        }

        let (sign, exponent, quotient, remainder, denominator) =
            if finite_exponent_gap(lhs, rhs) <= 64 {
                let (sign, exponent, magnitude) = add_finite_aligned(lhs, rhs)?;
                if magnitude == 0 {
                    let sign = mode == RationalRoundingMode::TowardNegative;
                    return Some(BinaryRoundingResult::new(
                        Self::from_bits(u64::from(sign) << 63),
                        false,
                        false,
                        false,
                    ));
                }
                finite_sum_quotient(sign, exponent, magnitude)?
            } else {
                finite_large_gap_rounding_bridge(lhs, rhs)?
            };
        Self::round_pack_quotient(sign, exponent, quotient, remainder, denominator, mode)
    }

    /// Subtracts finite values by adding a copy of the right operand with its
    /// sign flipped, while returning bounded binary64 rounding diagnostics.
    pub fn subtract_finite_aligned_result(
        lhs: BinaryFinite,
        rhs: BinaryFinite,
        mode: RationalRoundingMode,
    ) -> Option<BinaryRoundingResult<Self>> {
        let negated_rhs =
            BinaryFinite::new(!rhs.sign, rhs.significand, rhs.exponent, rhs.source_class);
        Self::add_finite_aligned_result(lhs, negated_rhs, mode)
    }

    /// Adds same-format raw binary64 operands and returns result diagnostics.
    ///
    /// Finite operands use the finite result path. NaN propagation follows the
    /// Intel SDM Rev. 093 source-order rule: select the first NaN operand and
    /// quiet it only when that selected operand is signaling. The local
    /// invalid-operation result is set for any signaling NaN or invalid
    /// infinity cancellation; no architectural state is updated.
    pub fn add_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryAddSubResult<Self> {
        Self::add_sub_raw_result(lhs, rhs, mode, false)
    }

    /// Subtracts same-format raw binary64 operands and returns result diagnostics.
    ///
    /// NaN selection is performed from the original source operands before the
    /// finite/infinity subtraction sign is applied to the right operand.
    pub fn subtract_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryAddSubResult<Self> {
        Self::add_sub_raw_result(lhs, rhs, mode, true)
    }

    fn add_sub_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
        subtract: bool,
    ) -> BinaryAddSubResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        let lhs_nan = matches!(lhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        let rhs_nan = matches!(rhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        if lhs_nan || rhs_nan {
            let (selected, selected_class) = if lhs_nan {
                (lhs, lhs_class)
            } else {
                (rhs, rhs_class)
            };
            let value = if selected_class == BinaryClass::SignalingNaN {
                Self::from_bits(selected.0 | Self::QUIET_BIT)
            } else {
                selected
            };
            return BinaryAddSubResult::exact(
                value,
                lhs_class == BinaryClass::SignalingNaN || rhs_class == BinaryClass::SignalingNaN,
            );
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let effective_rhs_sign = rhs.sign() ^ subtract;
        if lhs_infinity && rhs_infinity {
            if lhs.sign() != effective_rhs_sign {
                return BinaryAddSubResult::exact(Self::from_bits(0xfff8_0000_0000_0000), true);
            }
            return BinaryAddSubResult::exact(lhs, false);
        }
        if lhs_infinity {
            return BinaryAddSubResult::exact(lhs, false);
        }
        if rhs_infinity {
            return BinaryAddSubResult::exact(
                Self::from_bits(
                    (Self::EXPONENT_MASK << 52) | (u64::from(effective_rhs_sign) << 63),
                ),
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary64 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary64 operand");
        let rounded = if subtract {
            Self::subtract_finite_aligned_result(lhs_finite, rhs_finite, mode)
        } else {
            Self::add_finite_aligned_result(lhs_finite, rhs_finite, mode)
        }
        .expect("finite binary64 add/sub result is supported");
        BinaryAddSubResult::new(rounded, false)
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

fn finite_exponent_gap(lhs: BinaryFinite, rhs: BinaryFinite) -> u64 {
    (i64::from(lhs.exponent) - i64::from(rhs.exponent)).unsigned_abs()
}

fn finite_large_gap_rounding_bridge(
    lhs: BinaryFinite,
    rhs: BinaryFinite,
) -> Option<(bool, i32, u64, u64, u64)> {
    if lhs.significand == 0 || rhs.significand == 0 {
        return None;
    }
    let (dominant, small) = if lhs.exponent > rhs.exponent {
        (lhs, rhs)
    } else if rhs.exponent > lhs.exponent {
        (rhs, lhs)
    } else {
        return None;
    };
    let gap = u32::try_from(i64::from(dominant.exponent) - i64::from(small.exponent)).ok()?;
    if gap <= 64 {
        return None;
    }

    // With dominant sign s, exponent e, significand a and small significand b:
    // same-sign addition has E=e, Q=a, T=b/2^d; opposite-sign subtraction
    // with a>2^63 has E=e, Q=a-1, T=1-b/2^d; at a=2^63 it has
    // E=e-1, Q=2^64-1, T=1-b/2^(d-1). In every case Q is normalized and
    // 0<T<1, so cancellation is impossible at this gap.
    let (sign, exponent, quotient) = if dominant.sign == small.sign {
        (dominant.sign, dominant.exponent, dominant.significand)
    } else if dominant.significand > (1_u64 << 63) {
        (dominant.sign, dominant.exponent, dominant.significand - 1)
    } else {
        let exponent = dominant.exponent.checked_sub(1)?;
        (dominant.sign, exponent, u64::MAX)
    };

    // This is a rounding-equivalent bridge, not the exact sum quotient. Its
    // tail is 1/4 although the exact tail is another value strictly between
    // zero and one. For binary32/binary64 packing, normal precision is at most
    // 53 bits and subnormal rounding retains at most 53 bits, so the integer
    // quotient fixes every retained bit and the tail only distinguishes an
    // exact tie below/at/above a rounding midpoint. Any nonzero tail has the
    // same decisions for those precisions, including inexact, post-rounding
    // tininess, and overflow. This equivalence is not valid for 64-bit
    // precision rounding and must not be used as a generic exact quotient.
    Some((sign, exponent, quotient, 1, 4))
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

fn round_pack_quotient_bits(
    sign: bool,
    exponent: i32,
    quotient: u64,
    remainder: u64,
    denominator: u64,
    mode: RationalRoundingMode,
    precision: u32,
    minimum_normal_exponent: i32,
    maximum_normal_exponent: i32,
    sign_shift: u32,
    exponent_bias: i32,
    fraction_mask: u64,
    infinity_bits: u64,
    maximum_finite_bits: u64,
) -> Option<BinaryRoundingResult<u64>> {
    if denominator == 0 || remainder >= denominator {
        return None;
    }
    if quotient == 0 {
        if remainder != 0 {
            return None;
        }
        return Some(BinaryRoundingResult::new(
            u64::from(sign) << sign_shift,
            false,
            false,
            false,
        ));
    }
    if quotient < (1_u64 << 63) {
        return None;
    }

    let (magnitude, rounded_inexact) =
        round_quotient_to_precision(sign, quotient, remainder, denominator, precision, mode)?;
    let precision_limit = 1_u128 << precision;
    if magnitude < (precision_limit >> 1) || magnitude > precision_limit {
        return None;
    }

    let carried = magnitude == precision_limit;
    let rounded_exponent = i64::from(exponent) + i64::from(carried);
    let tiny_after_precision_rounding = rounded_exponent < i64::from(minimum_normal_exponent);
    let overflow = rounded_exponent > i64::from(maximum_normal_exponent);
    let sign_bits = u64::from(sign) << sign_shift;

    if overflow {
        let to_infinity = match mode {
            RationalRoundingMode::NearestTiesEven => true,
            RationalRoundingMode::TowardZero => false,
            RationalRoundingMode::TowardPositive => !sign,
            RationalRoundingMode::TowardNegative => sign,
        };
        return Some(BinaryRoundingResult::new(
            sign_bits
                | if to_infinity {
                    infinity_bits
                } else {
                    maximum_finite_bits
                },
            true,
            true,
            tiny_after_precision_rounding,
        ));
    }

    if tiny_after_precision_rounding {
        let (magnitude, inexact) = round_subnormal_quotient(
            sign,
            exponent,
            quotient,
            remainder,
            denominator,
            precision,
            minimum_normal_exponent,
            minimum_normal_exponent - (precision as i32 - 1),
            mode,
        )?;
        return Some(BinaryRoundingResult::new(
            sign_bits | magnitude,
            inexact,
            false,
            true,
        ));
    }

    let significand = if carried {
        precision_limit >> 1
    } else {
        magnitude
    } as u64;
    let biased_exponent = u64::try_from(rounded_exponent + i64::from(exponent_bias)).ok()?;
    let bits = sign_bits | (biased_exponent << (precision - 1)) | (significand & fraction_mask);
    Some(BinaryRoundingResult::new(
        bits,
        rounded_inexact,
        false,
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        Binary32, Binary64, BinaryClass, BinaryFinite, BinaryFiniteQuotient, BinaryRoundingResult,
        RationalRoundingMode, add_finite_aligned, compare_finite, divide_finite_significands,
        finite_exponent_gap, multiply_finite_significands, round_quotient_to_precision,
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
        // Convert the exact binary64 subtraction once, directly into binary32
        // subnormal units; an intermediate binary64 rounding would lose the tail.
        let result = Binary32::subtract_finite_aligned_result(
            lhs_subnormal_round,
            rhs_positive_tail,
            Nearest,
        )
        .unwrap();
        assert_eq!(result.value().to_bits(), 0x007f_ffff);
        assert!(result.inexact());
        assert!(result.tiny_after_precision_rounding());
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
    fn finite_aligned_result_zero_signs_and_cancellation_follow_rounding_mode() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        let zeros32 = [
            Binary32::from_bits(0).normalize_finite().unwrap(),
            Binary32::from_bits(0x8000_0000).normalize_finite().unwrap(),
        ];
        let zeros64 = [
            Binary64::from_bits(0).normalize_finite().unwrap(),
            Binary64::from_bits(0x8000_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
        ];

        for mode in modes {
            let negative_on_opposite = mode == Down;
            for (lhs_index, lhs) in zeros32.into_iter().enumerate() {
                for (rhs_index, rhs) in zeros64.into_iter().enumerate() {
                    let lhs_sign = lhs_index == 1;
                    let rhs_sign = rhs_index == 1;
                    let add_sign = if lhs_sign == rhs_sign {
                        lhs_sign
                    } else {
                        negative_on_opposite
                    };
                    let sub_rhs_sign = !rhs_sign;
                    let sub_sign = if lhs_sign == sub_rhs_sign {
                        lhs_sign
                    } else {
                        negative_on_opposite
                    };
                    assert_eq!(
                        Binary32::add_finite_aligned_result(lhs, rhs, mode)
                            .unwrap()
                            .value()
                            .to_bits(),
                        u32::from(add_sign) << 31,
                    );
                    assert_eq!(
                        Binary64::add_finite_aligned_result(lhs, rhs, mode)
                            .unwrap()
                            .value()
                            .to_bits(),
                        u64::from(add_sign) << 63,
                    );
                    assert_eq!(
                        Binary32::subtract_finite_aligned_result(lhs, rhs, mode)
                            .unwrap()
                            .value()
                            .to_bits(),
                        u32::from(sub_sign) << 31,
                    );
                    assert_eq!(
                        Binary64::subtract_finite_aligned_result(lhs, rhs, mode)
                            .unwrap()
                            .value()
                            .to_bits(),
                        u64::from(sub_sign) << 63,
                    );
                }
            }

            let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
            let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
            let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
                .normalize_finite()
                .unwrap();
            let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
                .normalize_finite()
                .unwrap();
            let expected_zero32 = u32::from(negative_on_opposite) << 31;
            let expected_zero64 = u64::from(negative_on_opposite) << 63;

            for (lhs, rhs) in [(one32, negative_one32), (negative_one32, one32)] {
                assert_eq!(
                    Binary32::add_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero32,
                );
                assert_eq!(
                    Binary64::add_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero64,
                );
            }
            for (lhs, rhs) in [(one64, negative_one64), (negative_one64, one64)] {
                assert_eq!(
                    Binary32::add_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero32,
                );
                assert_eq!(
                    Binary64::add_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero64,
                );
            }
            for (lhs, rhs) in [(one32, one32), (negative_one32, negative_one32)] {
                assert_eq!(
                    Binary32::subtract_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero32,
                );
                assert_eq!(
                    Binary64::subtract_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero64,
                );
            }
            for (lhs, rhs) in [(one64, one64), (negative_one64, negative_one64)] {
                assert_eq!(
                    Binary32::subtract_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero32,
                );
                assert_eq!(
                    Binary64::subtract_finite_aligned_result(lhs, rhs, mode)
                        .unwrap()
                        .value()
                        .to_bits(),
                    expected_zero64,
                );
            }
        }
    }

    #[test]
    fn finite_aligned_result_zero_operands_round_original_values_and_keep_diagnostics() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardZero as Zero};

        let positive_zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let tenth64 = Binary64::from_bits(0x3fb9_9999_9999_999a)
            .normalize_finite()
            .unwrap();
        let negative_tenth64 = Binary64::from_bits(0xbfb9_9999_9999_999a)
            .normalize_finite()
            .unwrap();

        let converted =
            Binary32::add_finite_aligned_result(positive_zero32, tenth64, Nearest).unwrap();
        assert_eq!(converted.value().to_bits(), 0x3dcc_cccd);
        assert!(converted.inexact());
        assert!(!converted.overflow());
        assert!(!converted.tiny_after_precision_rounding());

        let converted_sub =
            Binary32::subtract_finite_aligned_result(positive_zero32, negative_tenth64, Nearest)
                .unwrap();
        assert_eq!(converted_sub.value().to_bits(), 0x3dcc_cccd);
        assert!(converted_sub.inexact());

        let tiny64 = Binary64::from_bits(0x3690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_tiny64 = Binary64::from_bits(0xb690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let tiny64_subnormal = Binary64::from_bits(0x0008_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let tiny64_as_binary64 =
            Binary64::add_finite_aligned_result(negative_zero64, tiny64_subnormal, Nearest)
                .unwrap();
        assert_eq!(tiny64_as_binary64.value().to_bits(), 0x0008_0000_0000_0000);
        assert!(!tiny64_as_binary64.inexact());
        assert!(tiny64_as_binary64.tiny_after_precision_rounding());
        let negative_tiny64_as_binary64 =
            Binary64::subtract_finite_aligned_result(negative_zero64, tiny64_subnormal, Nearest)
                .unwrap();
        assert_eq!(
            negative_tiny64_as_binary64.value().to_bits(),
            0x8008_0000_0000_0000,
        );

        let rounded_zero =
            Binary32::add_finite_aligned_result(negative_zero64, tiny64, Zero).unwrap();
        assert_eq!(rounded_zero.value().to_bits(), 0);
        assert!(rounded_zero.inexact());
        assert!(rounded_zero.tiny_after_precision_rounding());
        let rounded_negative_zero =
            Binary32::subtract_finite_aligned_result(negative_zero64, tiny64, Zero).unwrap();
        assert_eq!(rounded_negative_zero.value().to_bits(), 0x8000_0000);
        assert!(rounded_negative_zero.inexact());
        assert!(rounded_negative_zero.tiny_after_precision_rounding());

        let rounded_negative_tiny =
            Binary32::add_finite_aligned_result(positive_zero32, negative_tiny64, Zero).unwrap();
        assert_eq!(rounded_negative_tiny.value().to_bits(), 0x8000_0000);
        assert!(rounded_negative_tiny.inexact());
        assert!(rounded_negative_tiny.tiny_after_precision_rounding());
        let rounded_positive_tiny_by_subtracting_negative =
            Binary32::subtract_finite_aligned_result(positive_zero32, negative_tiny64, Zero)
                .unwrap();
        assert_eq!(
            rounded_positive_tiny_by_subtracting_negative
                .value()
                .to_bits(),
            0,
        );
        assert!(rounded_positive_tiny_by_subtracting_negative.inexact());
        assert!(rounded_positive_tiny_by_subtracting_negative.tiny_after_precision_rounding());
        let rounded_negative_tiny_by_subtracting_positive =
            Binary32::subtract_finite_aligned_result(positive_zero32, tiny64, Zero).unwrap();
        assert_eq!(
            rounded_negative_tiny_by_subtracting_positive
                .value()
                .to_bits(),
            0x8000_0000,
        );
        assert!(rounded_negative_tiny_by_subtracting_positive.inexact());
        assert!(rounded_negative_tiny_by_subtracting_positive.tiny_after_precision_rounding());

        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let negative_max_subnormal32 = Binary32::from_bits(0x807f_ffff).normalize_finite().unwrap();
        let min_subnormal =
            Binary32::add_finite_aligned_result(min_normal32, negative_max_subnormal32, Nearest)
                .unwrap();
        assert_eq!(min_subnormal.value().to_bits(), 1);
        assert!(!min_subnormal.inexact());
        assert!(min_subnormal.tiny_after_precision_rounding());

        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_max_subnormal64 = Binary64::from_bits(0x800f_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let min_subnormal64 =
            Binary64::add_finite_aligned_result(min_normal64, negative_max_subnormal64, Nearest)
                .unwrap();
        assert_eq!(min_subnormal64.value().to_bits(), 1);
        assert!(!min_subnormal64.inexact());
        assert!(min_subnormal64.tiny_after_precision_rounding());
    }

    #[test]
    fn finite_aligned_result_nonzero_tiny_sums_round_to_signed_zero_or_min_subnormal() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let positive_full_tiny = Binary64::from_bits(0x36a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let positive_half_tiny = Binary64::from_bits(0x3690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_full_tiny = Binary64::from_bits(0xb6a0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_half_tiny = Binary64::from_bits(0xb690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let check = |result: BinaryRoundingResult<Binary32>, expected: u32| {
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(result.tiny_after_precision_rounding());
        };

        for (mode, positive_result, negative_result) in [
            (Nearest, 0, 0x8000_0000),
            (Zero, 0, 0x8000_0000),
            (Up, 1, 0x8000_0000),
            (Down, 0, 0x8000_0001),
        ] {
            // Nonzero subtraction in each operand order leaves +/-2^-150,
            // which is exactly halfway between zero and the least binary32 subnormal.
            check(
                Binary32::subtract_finite_aligned_result(
                    positive_full_tiny,
                    positive_half_tiny,
                    mode,
                )
                .unwrap(),
                positive_result,
            );
            check(
                Binary32::subtract_finite_aligned_result(
                    positive_half_tiny,
                    positive_full_tiny,
                    mode,
                )
                .unwrap(),
                negative_result,
            );

            // Adding the opposite-signed half-tiny produces the same exact
            // nonzero values and exercises the addition result API directly.
            check(
                Binary32::add_finite_aligned_result(positive_full_tiny, negative_half_tiny, mode)
                    .unwrap(),
                positive_result,
            );
            check(
                Binary32::add_finite_aligned_result(negative_full_tiny, positive_half_tiny, mode)
                    .unwrap(),
                negative_result,
            );
        }
    }

    #[test]
    fn finite_aligned_result_nonzero_sum_boundaries_and_legacy_apis() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let one_and_half32 = Binary32::from_bits(0x3fc0_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one_and_half64 = Binary64::from_bits(0x3ff8_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(
            Binary32::add_finite_aligned_result(one32, one_and_half32, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0x4020_0000,
        );
        assert_eq!(
            Binary64::add_finite_aligned_result(one64, one_and_half64, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0x4004_0000_0000_0000,
        );
        assert_eq!(
            Binary32::subtract_finite_aligned_result(one_and_half32, one32, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0x3f00_0000,
        );
        assert_eq!(
            Binary64::subtract_finite_aligned_result(one32, one_and_half64, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0xbfe0_0000_0000_0000,
        );

        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_64 = Binary64::from_bits(0x3bf0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let two_to_minus_65 = Binary64::from_bits(0x3be0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let lhs_before = one64;
        let rhs_before = two_to_minus_64;
        for (lhs, rhs, expected) in [
            (one64, two_to_minus_64, 0x3ff0_0000_0000_0000),
            (two_to_minus_64, one64, 0x3ff0_0000_0000_0000),
            (negative_one64, two_to_minus_64, 0xbff0_0000_0000_0000),
            (two_to_minus_64, negative_one64, 0xbff0_0000_0000_0000),
        ] {
            let result = Binary64::add_finite_aligned_result(lhs, rhs, Nearest).unwrap();
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
        }
        // The accepted 64-bit alignment tail must remain visible to directed rounding.
        let positive_tail_up =
            Binary64::add_finite_aligned_result(one64, two_to_minus_64, Up).unwrap();
        assert_eq!(positive_tail_up.value().to_bits(), 0x3ff0_0000_0000_0001);
        assert!(positive_tail_up.inexact());
        assert!(!positive_tail_up.overflow());
        assert!(!positive_tail_up.tiny_after_precision_rounding());

        for (mode, expected) in [
            (Up, 0xbfefffffffffffff),
            (Zero, 0xbfefffffffffffff),
            (Down, 0xbff0000000000000),
        ] {
            let negative_tail =
                Binary64::add_finite_aligned_result(negative_one64, two_to_minus_64, mode).unwrap();
            assert_eq!(negative_tail.value().to_bits(), expected);
            assert!(negative_tail.inexact());
            assert!(!negative_tail.overflow());
            assert!(!negative_tail.tiny_after_precision_rounding());
        }
        for (lhs, rhs) in [(one64, two_to_minus_65), (two_to_minus_65, one64)] {
            let result = Binary64::add_finite_aligned_result(lhs, rhs, Nearest).unwrap();
            assert_eq!(result.value().to_bits(), 0x3ff0_0000_0000_0000);
            assert!(result.inexact());
        }
        assert_eq!(
            Binary64::subtract_finite_aligned_result(one64, two_to_minus_65, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0x3ff0_0000_0000_0000,
        );
        assert_eq!(
            Binary64::subtract_finite_aligned_result(two_to_minus_65, one64, Nearest)
                .unwrap()
                .value()
                .to_bits(),
            0xbff0_0000_0000_0000,
        );
        assert!(Binary64::add_finite_aligned(one64, two_to_minus_65, Nearest).is_none());
        assert!(Binary64::subtract_finite_aligned(one64, two_to_minus_65, Nearest).is_none());
        assert_eq!(one64, lhs_before);
        assert_eq!(two_to_minus_64, rhs_before);
        let subtraction_lhs_before = one64;
        let subtraction_rhs_before = negative_one64;
        let _ = Binary64::subtract_finite_aligned_result(one64, negative_one64, Nearest).unwrap();
        assert_eq!(one64, subtraction_lhs_before);
        assert_eq!(negative_one64, subtraction_rhs_before);

        let max_finite32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let overflow =
            Binary32::add_finite_aligned_result(max_finite32, max_finite32, Nearest).unwrap();
        assert_eq!(overflow.value().to_bits(), 0x7f80_0000);
        assert!(overflow.inexact());
        assert!(overflow.overflow());
        assert!(!overflow.tiny_after_precision_rounding());
        let overflow_down =
            Binary32::add_finite_aligned_result(max_finite32, max_finite32, Down).unwrap();
        assert_eq!(overflow_down.value().to_bits(), 0x7f7f_ffff);
        assert!(overflow_down.overflow());

        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let overflow64 =
            Binary64::add_finite_aligned_result(max_finite64, max_finite64, Nearest).unwrap();
        assert_eq!(overflow64.value().to_bits(), 0x7ff0_0000_0000_0000);
        assert!(overflow64.inexact());
        assert!(overflow64.overflow());
        let overflow64_down =
            Binary64::add_finite_aligned_result(max_finite64, max_finite64, Down).unwrap();
        assert_eq!(overflow64_down.value().to_bits(), 0x7fef_ffff_ffff_ffff);
        assert!(overflow64_down.overflow());

        // Negative overflow selects infinity only for nearest and toward-negative.
        let negative_max_finite32 = Binary32::from_bits(0xff7f_ffff).normalize_finite().unwrap();
        let negative_max_finite64 = Binary64::from_bits(0xffef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let modes = [(Nearest, true), (Zero, false), (Up, false), (Down, true)];
        for (mode, infinity) in modes {
            let expected32 = if infinity { 0xff80_0000 } else { 0xff7f_ffff };
            let add32 = Binary32::add_finite_aligned_result(
                negative_max_finite32,
                negative_max_finite32,
                mode,
            )
            .unwrap();
            let subtract32 =
                Binary32::subtract_finite_aligned_result(negative_max_finite32, max_finite32, mode)
                    .unwrap();
            for result in [add32, subtract32] {
                assert_eq!(result.value().to_bits(), expected32);
                assert!(result.inexact());
                assert!(result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }

            let expected64 = if infinity {
                0xfff0_0000_0000_0000
            } else {
                0xffef_ffff_ffff_ffff
            };
            let add64 = Binary64::add_finite_aligned_result(
                negative_max_finite64,
                negative_max_finite64,
                mode,
            )
            .unwrap();
            let subtract64 =
                Binary64::subtract_finite_aligned_result(negative_max_finite64, max_finite64, mode)
                    .unwrap();
            for result in [add64, subtract64] {
                assert_eq!(result.value().to_bits(), expected64);
                assert!(result.inexact());
                assert!(result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }
        }

        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        let zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        assert_eq!(
            Binary32::add_finite_aligned(one32, negative_one32, Nearest),
            None
        );
        assert_eq!(
            Binary64::add_finite_aligned(one32, negative_one32, Nearest),
            None
        );
        assert_eq!(
            Binary32::subtract_finite_aligned(one32, one32, Nearest),
            None
        );
        assert_eq!(
            Binary64::subtract_finite_aligned(one32, one32, Nearest),
            None
        );
        assert_eq!(Binary32::add_finite_aligned(zero32, zero32, Nearest), None);
        assert_eq!(
            Binary64::subtract_finite_aligned(zero32, zero32, Nearest),
            None
        );
    }

    #[test]
    fn finite_aligned_result_large_gap_midpoints_cover_modes_signs_and_order() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        for (midpoint_bits, low, high) in [
            (0x3ff0_0000_1000_0000_u64, 0x3f80_0000_u32, 0x3f80_0001_u32),
            (0x3ff0_0000_3000_0000_u64, 0x3f80_0001_u32, 0x3f80_0002_u32),
        ] {
            for midpoint_negative in [false, true] {
                for tail_negative in [false, true] {
                    for mode in [Nearest, Zero, Up, Down] {
                        let midpoint = Binary64::from_bits(
                            midpoint_bits | (u64::from(midpoint_negative) << 63),
                        )
                        .normalize_finite()
                        .unwrap();
                        let tail = Binary64::from_bits(1 | (u64::from(tail_negative) << 63))
                            .normalize_finite()
                            .unwrap();
                        let exact_sign = midpoint_negative;
                        let magnitude_above_tie = midpoint_negative == tail_negative;
                        let round_magnitude_up = match mode {
                            Nearest => magnitude_above_tie,
                            Zero => false,
                            Up => !exact_sign,
                            Down => exact_sign,
                        };
                        let magnitude = if round_magnitude_up { high } else { low };
                        let expected = magnitude | (u32::from(exact_sign) << 31);

                        for (lhs, rhs) in [(midpoint, tail), (tail, midpoint)] {
                            let result =
                                Binary32::add_finite_aligned_result(lhs, rhs, mode).unwrap();
                            assert_eq!(result.value().to_bits(), expected);
                            assert!(result.inexact());
                            assert!(!result.overflow());
                            assert!(!result.tiny_after_precision_rounding());
                        }

                        let rhs_sign_for_same_effective_sum = !tail_negative;
                        let subtract_rhs = Binary64::from_bits(
                            1 | (u64::from(rhs_sign_for_same_effective_sum) << 63),
                        )
                        .normalize_finite()
                        .unwrap();
                        let subtract =
                            Binary32::subtract_finite_aligned_result(midpoint, subtract_rhs, mode)
                                .unwrap();
                        assert_eq!(subtract.value().to_bits(), expected);
                        assert!(subtract.inexact());

                        let reversed =
                            Binary32::subtract_finite_aligned_result(tail, midpoint, mode).unwrap();
                        let reverse_sign = !midpoint_negative;
                        let reverse_above_tie = !magnitude_above_tie;
                        let reverse_round_up = match mode {
                            Nearest => reverse_above_tie,
                            Zero => false,
                            Up => !reverse_sign,
                            Down => reverse_sign,
                        };
                        let reverse_magnitude = if reverse_round_up { high } else { low };
                        assert_eq!(
                            reversed.value().to_bits(),
                            reverse_magnitude | (u32::from(reverse_sign) << 31),
                        );
                        assert!(reversed.inexact());
                    }
                }
            }
        }
    }

    #[test]
    fn finite_aligned_result_large_gap_boundaries_and_extreme_significands() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        for (mode, positive_result, negative_result) in [
            (Nearest, 0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0000),
            (Zero, 0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0000),
            (Up, 0x3ff0_0000_0000_0001, 0xbff0_0000_0000_0000),
            (Down, 0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0001),
        ] {
            for (negative, expected_add) in [(false, positive_result), (true, negative_result)] {
                let dominant =
                    Binary64::from_bits(0x3ff0_0000_0000_0000 | (u64::from(negative) << 63))
                        .normalize_finite()
                        .unwrap();
                let tail = Binary64::from_bits(1 | (u64::from(negative) << 63))
                    .normalize_finite()
                    .unwrap();
                for (lhs, rhs) in [(dominant, tail), (tail, dominant)] {
                    let result = Binary64::add_finite_aligned_result(lhs, rhs, mode).unwrap();
                    assert_eq!(result.value().to_bits(), expected_add);
                    assert!(result.inexact());
                    assert!(!result.overflow());
                    assert!(!result.tiny_after_precision_rounding());
                }
            }
        }

        // The maximum finite exponent and minimum subnormal exponent differ by
        // 2,097; the legacy exact alignment API still rejects this pair.
        let max_finite64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        assert_eq!(finite_exponent_gap(max_finite64, min_subnormal64), 2097);
        assert!(Binary64::add_finite_aligned(max_finite64, min_subnormal64, Up).is_none());
        let extreme =
            Binary64::add_finite_aligned_result(max_finite64, min_subnormal64, Up).unwrap();
        assert_eq!(extreme.value().to_bits(), 0x7ff0_0000_0000_0000);
        assert!(extreme.inexact());
        assert!(extreme.overflow());
        assert!(!extreme.tiny_after_precision_rounding());

        // A binary64 tiny tail straddles the binary32 normal/subnormal
        // midpoint. Both rounded outputs report tininess after precision round.
        let boundary = Binary64::from_bits(0x380f_ffff_e000_0000)
            .normalize_finite()
            .unwrap();
        let rounded_normal =
            Binary32::add_finite_aligned_result(boundary, min_subnormal64, Nearest).unwrap();
        assert_eq!(rounded_normal.value().to_bits(), 0x0080_0000);
        assert!(rounded_normal.inexact());
        assert!(rounded_normal.tiny_after_precision_rounding());
        let rounded_subnormal =
            Binary32::subtract_finite_aligned_result(boundary, min_subnormal64, Nearest).unwrap();
        assert_eq!(rounded_subnormal.value().to_bits(), 0x007f_ffff);
        assert!(rounded_subnormal.inexact());
        assert!(rounded_subnormal.tiny_after_precision_rounding());

        // 2^-150 is the binary32 zero/min-subnormal midpoint. A binary64
        // min-subnormal on either side determines the exact side of the tie.
        let half_binary32_subnormal = Binary64::from_bits(0x3690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_half_binary32_subnormal = Binary64::from_bits(0xb690_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        for (mode, positive_above, positive_below, negative_above, negative_below) in [
            (Nearest, 1_u32, 0_u32, 0x8000_0001_u32, 0x8000_0000_u32),
            (Zero, 0, 0, 0x8000_0000, 0x8000_0000),
            (Up, 1, 1, 0x8000_0000, 0x8000_0000),
            (Down, 0, 0, 0x8000_0001, 0x8000_0001),
        ] {
            for (lhs, rhs) in [
                (half_binary32_subnormal, min_subnormal64),
                (min_subnormal64, half_binary32_subnormal),
            ] {
                let result = Binary32::add_finite_aligned_result(lhs, rhs, mode).unwrap();
                assert_eq!(result.value().to_bits(), positive_above);
                assert!(result.inexact());
                assert!(result.tiny_after_precision_rounding());
            }
            let positive_below_result = Binary32::subtract_finite_aligned_result(
                half_binary32_subnormal,
                min_subnormal64,
                mode,
            )
            .unwrap();
            assert_eq!(positive_below_result.value().to_bits(), positive_below);
            assert!(positive_below_result.inexact());
            assert!(positive_below_result.tiny_after_precision_rounding());

            let negative_above_result = Binary32::add_finite_aligned_result(
                negative_half_binary32_subnormal,
                Binary64::from_bits(0x8000_0000_0000_0001)
                    .normalize_finite()
                    .unwrap(),
                mode,
            )
            .unwrap();
            assert_eq!(negative_above_result.value().to_bits(), negative_above);
            assert!(negative_above_result.inexact());
            assert!(negative_above_result.tiny_after_precision_rounding());
            let negative_below_result = Binary32::add_finite_aligned_result(
                negative_half_binary32_subnormal,
                min_subnormal64,
                mode,
            )
            .unwrap();
            assert_eq!(negative_below_result.value().to_bits(), negative_below);
            assert!(negative_below_result.inexact());
            assert!(negative_below_result.tiny_after_precision_rounding());
        }

        let half_min_subnormal64 = Binary64::from_bits(0x3680_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let substage_above =
            Binary32::add_finite_aligned_result(boundary, half_min_subnormal64, Nearest).unwrap();
        assert_eq!(substage_above.value().to_bits(), 0x0080_0000);
        assert!(!substage_above.tiny_after_precision_rounding());
        let substage_below =
            Binary32::subtract_finite_aligned_result(boundary, half_min_subnormal64, Nearest)
                .unwrap();
        assert_eq!(substage_below.value().to_bits(), 0x007f_ffff);
        assert!(substage_below.tiny_after_precision_rounding());

        // The exact binary32 overflow midpoint is approached from either side.
        let overflow_midpoint = Binary64::from_bits(0x47ef_ffff_f000_0000)
            .normalize_finite()
            .unwrap();
        let rounded_infinity =
            Binary32::add_finite_aligned_result(overflow_midpoint, min_subnormal64, Nearest)
                .unwrap();
        assert_eq!(rounded_infinity.value().to_bits(), 0x7f80_0000);
        assert!(rounded_infinity.inexact());
        assert!(rounded_infinity.overflow());
        let rounded_max =
            Binary32::subtract_finite_aligned_result(overflow_midpoint, min_subnormal64, Nearest)
                .unwrap();
        assert_eq!(rounded_max.value().to_bits(), 0x7f7f_ffff);
        assert!(rounded_max.inexact());
        assert!(!rounded_max.overflow());

        // Values at and one integer step above 2^63 take the two different
        // normalized subtraction forms in the large-gap bridge.
        for (significand, positive_expected, negative_expected) in [
            (
                1_u64 << 63,
                [
                    0x3ff0_0000_0000_0000,
                    0x3fefffffffffffff,
                    0x3ff0_0000_0000_0000,
                    0x3fefffffffffffff,
                ],
                [
                    0xbff0_0000_0000_0000,
                    0xbfefffffffffffff,
                    0xbfefffffffffffff,
                    0xbff0_0000_0000_0000,
                ],
            ),
            (
                (1_u64 << 63) + 1,
                [
                    0x3ff0_0000_0000_0000,
                    0x3ff0_0000_0000_0000,
                    0x3ff0_0000_0000_0001,
                    0x3ff0_0000_0000_0000,
                ],
                [
                    0xbff0_0000_0000_0000,
                    0xbff0_0000_0000_0000,
                    0xbff0_0000_0000_0000,
                    0xbff0_0000_0000_0001,
                ],
            ),
        ] {
            for (sign, expected) in [(false, positive_expected), (true, negative_expected)] {
                let dominant = BinaryFinite::new(sign, significand, 0, BinaryClass::Normal);
                let small = BinaryFinite::new(!sign, 1_u64 << 63, -65, BinaryClass::Normal);
                for (mode, expected_bits) in [Nearest, Zero, Up, Down].into_iter().zip(expected) {
                    let result =
                        Binary64::add_finite_aligned_result(dominant, small, mode).unwrap();
                    assert_eq!(result.value().to_bits(), expected_bits);
                    assert!(result.inexact());
                    assert!(!result.overflow());
                    assert!(!result.tiny_after_precision_rounding());
                }
            }
        }
    }

    #[test]
    fn finite_aligned_result_gap_65_covers_binary32_binary64_and_subtraction_orders() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let tiny = Binary64::from_bits(0x3be0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        assert_eq!(finite_exponent_gap(one, tiny), 65);
        let negative_one = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_tiny = Binary64::from_bits(0xbbe0_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        for mode in [Nearest, Zero, Up, Down] {
            let one_plus_tiny = Binary64::add_finite_aligned_result(one, tiny, mode).unwrap();
            let one_plus_tiny_expected = if mode == Up {
                0x3ff0_0000_0000_0001
            } else {
                0x3ff0_0000_0000_0000
            };
            assert_eq!(one_plus_tiny.value().to_bits(), one_plus_tiny_expected);
            assert!(one_plus_tiny.inexact());

            let one_minus_tiny =
                Binary64::add_finite_aligned_result(one, negative_tiny, mode).unwrap();
            let one_minus_tiny_expected = if matches!(mode, Zero | Down) {
                0x3fefffffffffffff
            } else {
                0x3ff0_0000_0000_0000
            };
            assert_eq!(one_minus_tiny.value().to_bits(), one_minus_tiny_expected);
            assert!(one_minus_tiny.inexact());

            let negative_one_minus_tiny =
                Binary64::add_finite_aligned_result(negative_one, negative_tiny, mode).unwrap();
            let negative_one_minus_tiny_expected = if mode == Down {
                0xbff0_0000_0000_0001
            } else {
                0xbff0_0000_0000_0000
            };
            assert_eq!(
                negative_one_minus_tiny.value().to_bits(),
                negative_one_minus_tiny_expected,
            );
            assert!(negative_one_minus_tiny.inexact());

            let negative_one_plus_tiny =
                Binary64::add_finite_aligned_result(negative_one, tiny, mode).unwrap();
            let negative_one_plus_tiny_expected = if matches!(mode, Zero | Up) {
                0xbfefffffffffffff
            } else {
                0xbff0_0000_0000_0000
            };
            assert_eq!(
                negative_one_plus_tiny.value().to_bits(),
                negative_one_plus_tiny_expected,
            );
            assert!(negative_one_plus_tiny.inexact());

            // Subtraction is addition of a copied, sign-flipped right operand.
            let subtract_same_sign =
                Binary64::subtract_finite_aligned_result(one, negative_tiny, mode).unwrap();
            assert_eq!(subtract_same_sign.value().to_bits(), one_plus_tiny_expected);
            assert!(subtract_same_sign.inexact());
            let subtract_opposite_sign =
                Binary64::subtract_finite_aligned_result(one, tiny, mode).unwrap();
            assert_eq!(
                subtract_opposite_sign.value().to_bits(),
                one_minus_tiny_expected
            );
            assert!(subtract_opposite_sign.inexact());

            // Mixed destination precision has the same exact tail semantics.
            let result32 = Binary32::add_finite_aligned_result(one, tiny, mode).unwrap();
            assert_eq!(
                result32.value().to_bits(),
                if mode == Up { 0x3f80_0001 } else { 0x3f80_0000 },
            );
            assert!(result32.inexact());
        }

        for (lhs, rhs) in [(one, tiny), (tiny, one)] {
            let result = Binary64::add_finite_aligned_result(lhs, rhs, Nearest).unwrap();
            assert_eq!(result.value().to_bits(), 0x3ff0_0000_0000_0000);
            assert!(result.inexact());
        }
        let reverse_subtract =
            Binary64::subtract_finite_aligned_result(tiny, one, Nearest).unwrap();
        assert_eq!(reverse_subtract.value().to_bits(), 0xbff0_0000_0000_0000);
        assert!(reverse_subtract.inexact());
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

    #[test]
    fn bounded_quotient_rounding_covers_subnormal_precision_and_diagnostics() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardPositive};

        let exact_min_sub32 =
            Binary32::round_pack_quotient(false, -149, 1 << 63, 0, 1, Nearest).unwrap();
        assert_eq!(*exact_min_sub32.value(), Binary32::from_bits(1));
        assert!(!exact_min_sub32.inexact());
        assert!(exact_min_sub32.tiny_after_precision_rounding());
        let halfway_min_sub32 =
            Binary32::round_pack_quotient(false, -150, 1 << 63, 0, 1, Nearest).unwrap();
        assert_eq!(*halfway_min_sub32.value(), Binary32::from_bits(0));
        assert!(halfway_min_sub32.inexact());
        let away_min_sub32 =
            Binary32::round_pack_quotient(false, -150, 1 << 63, 0, 1, TowardPositive).unwrap();
        assert_eq!(*away_min_sub32.value(), Binary32::from_bits(1));
        assert!(away_min_sub32.inexact());

        let exact_min_sub64 =
            Binary64::round_pack_quotient(false, -1074, 1 << 63, 0, 1, Nearest).unwrap();
        assert_eq!(*exact_min_sub64.value(), Binary64::from_bits(1));
        assert!(!exact_min_sub64.inexact());
        let halfway_min_sub64 =
            Binary64::round_pack_quotient(false, -1075, 1 << 63, 0, 1, Nearest).unwrap();
        assert_eq!(*halfway_min_sub64.value(), Binary64::from_bits(0));
        assert!(halfway_min_sub64.inexact());
        let away_min_sub64 =
            Binary64::round_pack_quotient(false, -1075, 1 << 63, 0, 1, TowardPositive).unwrap();
        assert_eq!(*away_min_sub64.value(), Binary64::from_bits(1));
        assert!(away_min_sub64.inexact());

        // The exact value is halfway between max-subnormal and min-normal. Its
        // unbounded-precision result remains tiny, while direct subnormal rounding
        // produces the minimum normal encoding (no intermediate double rounding).
        let rounded_min32 =
            Binary32::round_pack_quotient(false, -127, 0xffff_ff00_0000_0000, 0, 1, Nearest)
                .unwrap();
        assert_eq!(*rounded_min32.value(), Binary32::from_bits(0x0080_0000));
        assert!(rounded_min32.inexact());
        assert!(rounded_min32.tiny_after_precision_rounding());
        let precision_rounds_min32 = Binary32::round_pack_quotient(
            false,
            -127,
            0xffff_ff00_0000_0000 + (1 << 39),
            0,
            1,
            Nearest,
        )
        .unwrap();
        assert_eq!(
            *precision_rounds_min32.value(),
            Binary32::from_bits(0x0080_0000)
        );
        assert!(precision_rounds_min32.inexact());
        assert!(!precision_rounds_min32.tiny_after_precision_rounding());

        let rounded_min64 =
            Binary64::round_pack_quotient(false, -1023, 0xffff_ffff_ffff_f800, 0, 1, Nearest)
                .unwrap();
        assert_eq!(
            *rounded_min64.value(),
            Binary64::from_bits(0x0010_0000_0000_0000)
        );
        assert!(rounded_min64.tiny_after_precision_rounding());
        let precision_rounds_min64 = Binary64::round_pack_quotient(
            false,
            -1023,
            0xffff_ffff_ffff_f800 + (1 << 10),
            0,
            1,
            Nearest,
        )
        .unwrap();
        assert_eq!(
            *precision_rounds_min64.value(),
            Binary64::from_bits(0x0010_0000_0000_0000)
        );
        assert!(!precision_rounds_min64.tiny_after_precision_rounding());
    }

    #[test]
    fn bounded_quotient_rounding_handles_overflow_zero_and_invalid_decompositions() {
        use RationalRoundingMode::{NearestTiesEven, TowardNegative, TowardPositive, TowardZero};

        for sign in [false, true] {
            let zero32 =
                Binary32::round_pack_quotient(sign, i32::MIN, 0, 0, 1, NearestTiesEven).unwrap();
            assert_eq!(*zero32.value(), Binary32::from_bits(u32::from(sign) << 31));
            assert!(
                !zero32.inexact() && !zero32.overflow() && !zero32.tiny_after_precision_rounding()
            );
            let zero64 =
                Binary64::round_pack_quotient(sign, i32::MAX, 0, 0, 1, NearestTiesEven).unwrap();
            assert_eq!(*zero64.value(), Binary64::from_bits(u64::from(sign) << 63));
        }

        assert!(Binary32::round_pack_quotient(false, 0, 1 << 63, 0, 0, NearestTiesEven).is_none());
        assert!(Binary32::round_pack_quotient(false, 0, 1 << 63, 1, 1, NearestTiesEven).is_none());
        assert!(
            Binary32::round_pack_quotient(false, 0, (1 << 63) - 1, 0, 1, NearestTiesEven).is_none()
        );
        assert!(Binary32::round_pack_quotient(false, 0, 0, 1, 2, NearestTiesEven).is_none());
        assert!(Binary64::round_pack_quotient(false, 0, 0, 0, 0, NearestTiesEven).is_none());

        let plus_exact_overflow =
            Binary32::round_pack_quotient(false, 128, 1 << 63, 0, 1, NearestTiesEven).unwrap();
        assert_eq!(
            *plus_exact_overflow.value(),
            Binary32::from_bits(0x7f80_0000)
        );
        assert!(plus_exact_overflow.overflow() && plus_exact_overflow.inexact());
        let minus_exact_overflow =
            Binary64::round_pack_quotient(true, 1024, 1 << 63, 0, 1, TowardZero).unwrap();
        assert_eq!(
            *minus_exact_overflow.value(),
            Binary64::from_bits(0xffef_ffff_ffff_ffff)
        );
        assert!(minus_exact_overflow.overflow() && minus_exact_overflow.inexact());

        for (mode, positive_bits, negative_bits) in [
            (NearestTiesEven, 0x7f80_0000, 0xff80_0000),
            (TowardZero, 0x7f7f_ffff, 0xff7f_ffff),
            (TowardPositive, 0x7f80_0000, 0xff7f_ffff),
            (TowardNegative, 0x7f7f_ffff, 0xff80_0000),
        ] {
            for sign in [false, true] {
                let result =
                    Binary32::round_pack_quotient(sign, 127, u64::MAX, 0, 1, mode).unwrap();
                assert_eq!(
                    result.value().to_bits(),
                    if sign { negative_bits } else { positive_bits }
                );
                assert!(result.inexact());
                assert_eq!(
                    result.overflow(),
                    mode != TowardZero
                        && match mode {
                            NearestTiesEven => true,
                            TowardPositive => !sign,
                            TowardNegative => sign,
                            TowardZero => false,
                        }
                );
            }
        }

        // A value just above max-finite rounds down under toward-zero and the
        // directed mode that points inward; its unbounded result does not carry.
        for (sign, mode, expected32) in [
            (false, TowardZero, 0x7f7f_ffff),
            (false, TowardNegative, 0x7f7f_ffff),
            (true, TowardZero, 0xff7f_ffff),
            (true, TowardPositive, 0xff7f_ffff),
        ] {
            let result = Binary32::round_pack_quotient(sign, 127, u64::MAX, 0, 1, mode).unwrap();
            assert_eq!(result.value().to_bits(), expected32);
            assert!(!result.overflow());
            assert!(result.inexact());
        }
        let extreme_tiny =
            Binary64::round_pack_quotient(false, i32::MIN, 1 << 63, 0, 1, NearestTiesEven).unwrap();
        assert_eq!(*extreme_tiny.value(), Binary64::from_bits(0));
        assert!(extreme_tiny.tiny_after_precision_rounding() && extreme_tiny.inexact());
        let extreme_overflow =
            Binary64::round_pack_quotient(false, i32::MAX, 1 << 63, 0, 1, NearestTiesEven).unwrap();
        assert_eq!(
            *extreme_overflow.value(),
            Binary64::from_bits(0x7ff0_0000_0000_0000)
        );
        assert!(extreme_overflow.overflow() && extreme_overflow.inexact());
    }

    #[test]
    fn finite_multiplication_result_preserves_zero_sign_and_reports_range() {
        use RationalRoundingMode::{NearestTiesEven as Nearest, TowardZero};

        let zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero32 = Binary32::from_bits(0x8000_0000).normalize_finite().unwrap();
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        for (lhs, rhs, expected) in [
            (zero32, zero32, 0),
            (negative_zero32, zero32, 0x8000_0000),
            (zero32, negative_zero32, 0x8000_0000),
            (negative_zero32, negative_zero32, 0),
        ] {
            let result = Binary32::multiply_finite_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), Binary32::from_bits(expected));
            assert!(
                !result.inexact() && !result.overflow() && !result.tiny_after_precision_rounding()
            );
        }
        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal32 = Binary32::from_bits(0x8000_0001).normalize_finite().unwrap();
        for (lhs, rhs, expected) in [
            (zero32, min_subnormal32, 0),
            (min_subnormal32, zero32, 0),
            (negative_zero32, min_subnormal32, 0x8000_0000),
            (min_subnormal32, negative_zero32, 0x8000_0000),
            (zero32, negative_min_subnormal32, 0x8000_0000),
            (negative_min_subnormal32, zero32, 0x8000_0000),
            (negative_zero32, negative_min_subnormal32, 0),
            (negative_min_subnormal32, negative_zero32, 0),
        ] {
            let result = Binary32::multiply_finite_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), Binary32::from_bits(expected));
            assert!(
                !result.inexact() && !result.overflow() && !result.tiny_after_precision_rounding()
            );
        }

        let zero64 = Binary64::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        for (lhs, rhs, expected) in [
            (zero64, zero64, 0),
            (negative_zero64, zero64, 0x8000_0000_0000_0000),
            (zero64, negative_zero64, 0x8000_0000_0000_0000),
            (negative_zero64, negative_zero64, 0),
        ] {
            let result = Binary64::multiply_finite_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), Binary64::from_bits(expected));
            assert!(
                !result.inexact() && !result.overflow() && !result.tiny_after_precision_rounding()
            );
        }
        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal64 = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        for (lhs, rhs, expected) in [
            (zero64, min_subnormal64, 0),
            (min_subnormal64, zero64, 0),
            (negative_zero64, min_subnormal64, 0x8000_0000_0000_0000),
            (min_subnormal64, negative_zero64, 0x8000_0000_0000_0000),
            (zero64, negative_min_subnormal64, 0x8000_0000_0000_0000),
            (negative_min_subnormal64, zero64, 0x8000_0000_0000_0000),
            (negative_zero64, negative_min_subnormal64, 0),
            (negative_min_subnormal64, negative_zero64, 0),
        ] {
            let result = Binary64::multiply_finite_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), Binary64::from_bits(expected));
            assert!(
                !result.inexact() && !result.overflow() && !result.tiny_after_precision_rounding()
            );
        }

        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let min_subnormal_scale32 = Binary32::from_bits(0x3400_0000).normalize_finite().unwrap();
        let exact_min_subnormal32 =
            Binary32::multiply_finite_result(min_normal32, min_subnormal_scale32, Nearest);
        assert_eq!(*exact_min_subnormal32.value(), Binary32::from_bits(1));
        assert!(!exact_min_subnormal32.inexact() && !exact_min_subnormal32.overflow());
        assert!(exact_min_subnormal32.tiny_after_precision_rounding());

        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min_subnormal_scale64 = Binary64::from_bits(0x3cb0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let exact_min_subnormal64 =
            Binary64::multiply_finite_result(min_normal64, min_subnormal_scale64, Nearest);
        assert_eq!(*exact_min_subnormal64.value(), Binary64::from_bits(1));
        assert!(!exact_min_subnormal64.inexact() && !exact_min_subnormal64.overflow());
        assert!(exact_min_subnormal64.tiny_after_precision_rounding());

        let max32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let max32_result = Binary32::multiply_finite_result(max32, one32, Nearest);
        assert_eq!(*max32_result.value(), Binary32::from_bits(0x7f7f_ffff));
        assert!(!max32_result.inexact() && !max32_result.overflow());
        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        let overflow32 = Binary32::multiply_finite_result(max32, two32, TowardZero);
        assert_eq!(*overflow32.value(), Binary32::from_bits(0x7f7f_ffff));
        assert!(overflow32.overflow() && overflow32.inexact());

        let max64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let max64_result = Binary64::multiply_finite_result(max64, one64, Nearest);
        assert_eq!(
            *max64_result.value(),
            Binary64::from_bits(0x7fef_ffff_ffff_ffff)
        );
        assert!(!max64_result.inexact() && !max64_result.overflow());
        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let overflow64 = Binary64::multiply_finite_result(max64, two64, TowardZero);
        assert_eq!(
            *overflow64.value(),
            Binary64::from_bits(0x7fef_ffff_ffff_ffff)
        );
        assert!(overflow64.overflow() && overflow64.inexact());
    }

    #[test]
    fn finite_division_result_rounds_once_with_signs_and_preserves_legacy_behavior() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        let three32 = Binary32::from_bits(0x4040_0000).normalize_finite().unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let three64 = Binary64::from_bits(0x4008_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_three64 = Binary64::from_bits(0xc008_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        // 1/3 takes the quotient-normalization branch where numerator < divisor;
        // 3/2 takes the branch where numerator >= divisor. Inputs are mixed
        // across source formats while the destination format stays explicit.
        let three_halves32 = Binary32::divide_finite_result(
            three32,
            Binary64::from_bits(0x4000_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            Nearest,
        )
        .unwrap();
        assert_eq!(*three_halves32.value(), Binary32::from_bits(0x3fc0_0000));
        assert!(!three_halves32.inexact() && !three_halves32.overflow());
        let one_third32 = Binary32::divide_finite_result(one32, three64, Nearest).unwrap();
        assert_eq!(*one_third32.value(), Binary32::from_bits(0x3eaa_aaab));
        assert!(one_third32.inexact() && !one_third32.overflow());
        let three_halves64 = Binary64::divide_finite_result(
            Binary32::from_bits(0x4040_0000).normalize_finite().unwrap(),
            Binary64::from_bits(0x4000_0000_0000_0000)
                .normalize_finite()
                .unwrap(),
            Nearest,
        )
        .unwrap();
        assert_eq!(
            *three_halves64.value(),
            Binary64::from_bits(0x3ff8_0000_0000_0000)
        );
        assert!(!three_halves64.inexact() && !three_halves64.overflow());
        let one_third64 = Binary64::divide_finite_result(one64, three32, Nearest).unwrap();
        assert_eq!(
            *one_third64.value(),
            Binary64::from_bits(0x3fd5_5555_5555_5555)
        );
        assert!(one_third64.inexact() && !one_third64.overflow());

        for (mode, positive32, negative32, positive64, negative64) in [
            (
                Nearest,
                0x3eaa_aaab,
                0xbeaa_aaab,
                0x3fd5_5555_5555_5555,
                0xbfd5_5555_5555_5555,
            ),
            (
                Zero,
                0x3eaa_aaaa,
                0xbeaa_aaaa,
                0x3fd5_5555_5555_5555,
                0xbfd5_5555_5555_5555,
            ),
            (
                Up,
                0x3eaa_aaab,
                0xbeaa_aaaa,
                0x3fd5_5555_5555_5556,
                0xbfd5_5555_5555_5555,
            ),
            (
                Down,
                0x3eaa_aaaa,
                0xbeaa_aaab,
                0x3fd5_5555_5555_5555,
                0xbfd5_5555_5555_5556,
            ),
        ] {
            for (lhs, rhs, expected32, expected64) in [
                (one32, three64, positive32, positive64),
                (negative_one32, three64, negative32, negative64),
                (one32, negative_three64, negative32, negative64),
                (negative_one32, negative_three64, positive32, positive64),
            ] {
                let result32 = Binary32::divide_finite_result(lhs, rhs, mode).unwrap();
                assert_eq!(*result32.value(), Binary32::from_bits(expected32));
                assert!(result32.inexact() && !result32.overflow());
                let result64 = Binary64::divide_finite_result(
                    Binary64::from_bits(if lhs.sign() {
                        0xbff0_0000_0000_0000
                    } else {
                        0x3ff0_0000_0000_0000
                    })
                    .normalize_finite()
                    .unwrap(),
                    rhs,
                    mode,
                )
                .unwrap();
                assert_eq!(*result64.value(), Binary64::from_bits(expected64));
                assert!(result64.inexact() && !result64.overflow());
            }
        }

        // Legacy division remains unchanged, including its zero-input rejection.
        assert_eq!(
            Binary32::divide_finite(one32, three32, Nearest),
            Some((Binary32::from_bits(0x3eaa_aaab), true))
        );
        assert_eq!(
            Binary64::divide_finite(one64, three64, Nearest),
            Some((Binary64::from_bits(0x3fd5_5555_5555_5555), true))
        );
        assert_eq!(
            Binary32::divide_finite(
                Binary32::from_bits(0x8000_0000).normalize_finite().unwrap(),
                one32,
                Nearest
            ),
            None
        );
        assert_eq!(
            Binary64::divide_finite(
                Binary64::from_bits(0x8000_0000_0000_0000)
                    .normalize_finite()
                    .unwrap(),
                one64,
                Nearest
            ),
            None
        );
    }

    #[test]
    fn finite_division_result_preserves_zero_sign_and_reports_tininess() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let zero32 = Binary32::from_bits(0).normalize_finite().unwrap();
        let negative_zero32 = Binary32::from_bits(0x8000_0000).normalize_finite().unwrap();
        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let negative_one32 = Binary32::from_bits(0xbf80_0000).normalize_finite().unwrap();
        let zero64 = Binary64::from_bits(0).normalize_finite().unwrap();
        let negative_zero64 = Binary64::from_bits(0x8000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let negative_one64 = Binary64::from_bits(0xbff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();

        for (lhs, rhs, expected32, expected64) in [
            (zero32, one32, 0, 0),
            (negative_zero32, one32, 0x8000_0000, 0x8000_0000_0000_0000),
            (zero32, negative_one32, 0x8000_0000, 0x8000_0000_0000_0000),
            (negative_zero32, negative_one32, 0, 0),
        ] {
            let result32 = Binary32::divide_finite_result(lhs, rhs, Nearest).unwrap();
            let result64 = Binary64::divide_finite_result(
                if lhs.sign() { negative_zero64 } else { zero64 },
                if rhs.sign() { negative_one64 } else { one64 },
                Nearest,
            )
            .unwrap();
            assert_eq!(*result32.value(), Binary32::from_bits(expected32));
            assert_eq!(*result64.value(), Binary64::from_bits(expected64));
            for (inexact, overflow, tiny) in [
                (
                    result32.inexact(),
                    result32.overflow(),
                    result32.tiny_after_precision_rounding(),
                ),
                (
                    result64.inexact(),
                    result64.overflow(),
                    result64.tiny_after_precision_rounding(),
                ),
            ] {
                assert!(!inexact && !overflow && !tiny);
            }
        }
        for (lhs, rhs) in [
            (zero32, zero32),
            (negative_zero32, zero32),
            (zero32, negative_zero32),
            (negative_zero32, negative_zero32),
            (one32, zero32),
            (one32, negative_zero32),
        ] {
            assert!(Binary32::divide_finite_result(lhs, rhs, Nearest).is_none());
        }
        for (lhs, rhs) in [
            (zero64, zero64),
            (negative_zero64, zero64),
            (zero64, negative_zero64),
            (negative_zero64, negative_zero64),
            (one64, zero64),
            (one64, negative_zero64),
        ] {
            assert!(Binary64::divide_finite_result(lhs, rhs, Nearest).is_none());
        }

        let two32 = Binary32::from_bits(0x4000_0000).normalize_finite().unwrap();
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let exact_tiny32 = Binary32::divide_finite_result(min_normal32, two32, Nearest).unwrap();
        assert_eq!(*exact_tiny32.value(), Binary32::from_bits(0x0040_0000));
        assert!(exact_tiny32.tiny_after_precision_rounding());
        assert!(!exact_tiny32.inexact() && !exact_tiny32.overflow());

        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal32 = Binary32::from_bits(0x8000_0001).normalize_finite().unwrap();
        for (mode, positive, negative) in [
            (Nearest, 0, 0x8000_0000),
            (Zero, 0, 0x8000_0000),
            (Up, 1, 0x8000_0000),
            (Down, 0, 0x8000_0001),
        ] {
            for (lhs, expected) in [
                (min_subnormal32, positive),
                (negative_min_subnormal32, negative),
            ] {
                let result = Binary32::divide_finite_result(lhs, two32, mode).unwrap();
                assert_eq!(*result.value(), Binary32::from_bits(expected));
                assert!(result.tiny_after_precision_rounding() && result.inexact());
                assert!(!result.overflow());
            }
        }

        let two64 = Binary64::from_bits(0x4000_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let exact_tiny64 = Binary64::divide_finite_result(min_normal64, two64, Nearest).unwrap();
        assert_eq!(
            *exact_tiny64.value(),
            Binary64::from_bits(0x0008_0000_0000_0000)
        );
        assert!(exact_tiny64.tiny_after_precision_rounding());
        assert!(!exact_tiny64.inexact() && !exact_tiny64.overflow());
        let min_subnormal64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let negative_min_subnormal64 = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        for (mode, positive, negative) in [
            (Nearest, 0, 0x8000_0000_0000_0000),
            (Zero, 0, 0x8000_0000_0000_0000),
            (Up, 1, 0x8000_0000_0000_0000),
            (Down, 0, 0x8000_0000_0000_0001),
        ] {
            for (lhs, expected) in [
                (min_subnormal64, positive),
                (negative_min_subnormal64, negative),
            ] {
                let result = Binary64::divide_finite_result(lhs, two64, mode).unwrap();
                assert_eq!(*result.value(), Binary64::from_bits(expected));
                assert!(result.tiny_after_precision_rounding() && result.inexact());
                assert!(!result.overflow());
            }
        }
    }

    #[test]
    fn finite_division_result_handles_boundaries_overflow_and_extreme_inputs() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let one32 = Binary32::from_bits(0x3f80_0000).normalize_finite().unwrap();
        let max32 = Binary32::from_bits(0x7f7f_ffff).normalize_finite().unwrap();
        let max32_exact = Binary32::divide_finite_result(max32, one32, Nearest).unwrap();
        assert_eq!(*max32_exact.value(), Binary32::from_bits(0x7f7f_ffff));
        assert!(!max32_exact.inexact() && !max32_exact.overflow());

        // A binary64 quotient just below the normal/subnormal boundary must be
        // rounded directly to binary32 subnormal units, without double rounding.
        let boundary = Binary64::from_bits(0x380f_ffff_dfff_ffff)
            .normalize_finite()
            .unwrap();
        let one64 = Binary64::from_bits(0x3ff0_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let boundary_result = Binary32::divide_finite_result(boundary, one64, Nearest).unwrap();
        assert_eq!(*boundary_result.value(), Binary32::from_bits(0x007f_ffff));
        assert!(boundary_result.tiny_after_precision_rounding() && boundary_result.inexact());
        assert!(!boundary_result.overflow());
        let min_normal32 = Binary32::from_bits(0x0080_0000).normalize_finite().unwrap();
        let below_min_normal_divisor = Binary32::from_bits(0x3f80_0001).normalize_finite().unwrap();
        let just_below =
            Binary32::divide_finite_result(min_normal32, below_min_normal_divisor, Nearest)
                .unwrap();
        assert_eq!(*just_below.value(), Binary32::from_bits(0x007f_ffff));
        assert!(just_below.tiny_after_precision_rounding() && just_below.inexact());

        let min_subnormal32 = Binary32::from_bits(1).normalize_finite().unwrap();
        for (lhs, sign) in [
            (max32, false),
            (
                Binary32::from_bits(0xff7f_ffff).normalize_finite().unwrap(),
                true,
            ),
        ] {
            let max_finite_bits = if sign { 0xff7f_ffff } else { 0x7f7f_ffff };
            let infinity_bits = if sign { 0xff80_0000 } else { 0x7f80_0000 };
            for (mode, expected, overflow) in [
                (Nearest, infinity_bits, true),
                (Zero, max_finite_bits, true),
                (Up, if sign { max_finite_bits } else { infinity_bits }, true),
                (
                    Down,
                    if sign { infinity_bits } else { max_finite_bits },
                    true,
                ),
            ] {
                let result = Binary32::divide_finite_result(lhs, min_subnormal32, mode).unwrap();
                assert_eq!(*result.value(), Binary32::from_bits(expected));
                assert_eq!(result.overflow(), overflow);
                assert!(result.inexact());
            }
        }
        let max64 = Binary64::from_bits(0x7fef_ffff_ffff_ffff)
            .normalize_finite()
            .unwrap();
        let min64 = Binary64::from_bits(1).normalize_finite().unwrap();
        let negative_min64 = Binary64::from_bits(0x8000_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let tiny_positive = Binary64::divide_finite_result(min64, max64, Nearest).unwrap();
        assert_eq!(*tiny_positive.value(), Binary64::from_bits(0));
        assert!(tiny_positive.tiny_after_precision_rounding() && tiny_positive.inexact());
        assert!(!tiny_positive.overflow());
        let tiny_up = Binary64::divide_finite_result(min64, max64, Up).unwrap();
        assert_eq!(*tiny_up.value(), Binary64::from_bits(1));
        assert!(tiny_up.tiny_after_precision_rounding() && tiny_up.inexact());
        let tiny_negative_down =
            Binary64::divide_finite_result(negative_min64, max64, Down).unwrap();
        assert_eq!(
            *tiny_negative_down.value(),
            Binary64::from_bits(0x8000_0000_0000_0001)
        );
        assert!(tiny_negative_down.tiny_after_precision_rounding() && tiny_negative_down.inexact());

        let min_normal64 = Binary64::from_bits(0x0010_0000_0000_0000)
            .normalize_finite()
            .unwrap();
        let next_one64 = Binary64::from_bits(0x3ff0_0000_0000_0001)
            .normalize_finite()
            .unwrap();
        let just_below64 =
            Binary64::divide_finite_result(min_normal64, next_one64, Nearest).unwrap();
        assert_eq!(
            *just_below64.value(),
            Binary64::from_bits(0x000f_ffff_ffff_ffff)
        );
        assert!(just_below64.tiny_after_precision_rounding() && just_below64.inexact());

        for (lhs, sign) in [
            (max64, false),
            (
                Binary64::from_bits(0xffef_ffff_ffff_ffff)
                    .normalize_finite()
                    .unwrap(),
                true,
            ),
        ] {
            let max_finite_bits = if sign {
                0xffef_ffff_ffff_ffff
            } else {
                0x7fef_ffff_ffff_ffff
            };
            let infinity_bits = if sign {
                0xfff0_0000_0000_0000
            } else {
                0x7ff0_0000_0000_0000
            };
            for (mode, expected) in [
                (Nearest, infinity_bits),
                (Zero, max_finite_bits),
                (Up, if sign { max_finite_bits } else { infinity_bits }),
                (Down, if sign { infinity_bits } else { max_finite_bits }),
            ] {
                let result = Binary64::divide_finite_result(lhs, min64, mode).unwrap();
                assert_eq!(*result.value(), Binary64::from_bits(expected));
                assert!(result.overflow() && result.inexact());
            }
        }

        let lhs_raw = Binary64::from_bits(0x0000_0000_0000_0001);
        let rhs_raw = Binary64::from_bits(0x7fef_ffff_ffff_ffff);
        let lhs_before = lhs_raw.to_bits();
        let rhs_before = rhs_raw.to_bits();
        let lhs = lhs_raw.normalize_finite().unwrap();
        let rhs = rhs_raw.normalize_finite().unwrap();
        let _ = Binary64::divide_finite_result(lhs, rhs, Nearest).unwrap();
        assert_eq!(lhs_raw.to_bits(), lhs_before);
        assert_eq!(rhs_raw.to_bits(), rhs_before);
        assert_eq!(lhs.significand(), 1 << 63);
        assert_eq!(lhs.exponent(), -1074);
        assert_eq!(rhs.exponent(), 1023);
    }

    #[test]
    fn raw_binary32_add_sub_nan_selection_and_invalid_are_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let qnan = Binary32::from_bits(0x7fc1_2345);
        let negative_qnan = Binary32::from_bits(0xffc2_3456);
        let snan = Binary32::from_bits(0x7f81_2345);
        let quieted_snan = Binary32::from_bits(0x7fc1_2345);
        let negative_snan = Binary32::from_bits(0xff81_3456);
        let quieted_negative_snan = Binary32::from_bits(0xffc1_3456);
        let one = Binary32::from_bits(0x3f80_0000);

        for result in [
            Binary32::add_raw_result(qnan, snan, Nearest),
            Binary32::subtract_raw_result(qnan, snan, Nearest),
        ] {
            assert_eq!(*result.value(), qnan);
            assert!(result.invalid_operation());
            assert!(!result.inexact() && !result.overflow());
        }
        for result in [
            Binary32::add_raw_result(snan, qnan, Nearest),
            Binary32::subtract_raw_result(snan, qnan, Nearest),
        ] {
            assert_eq!(*result.value(), quieted_snan);
            assert!(result.invalid_operation());
        }

        for result in [
            Binary32::add_raw_result(one, negative_qnan, Nearest),
            Binary32::subtract_raw_result(one, negative_qnan, Nearest),
        ] {
            assert_eq!(*result.value(), negative_qnan);
            assert!(!result.invalid_operation());
        }
        for result in [
            Binary32::add_raw_result(one, snan, Nearest),
            Binary32::subtract_raw_result(one, snan, Nearest),
        ] {
            assert_eq!(*result.value(), quieted_snan);
            assert!(result.invalid_operation());
        }
        assert_eq!(
            *Binary32::add_raw_result(qnan, negative_qnan, Nearest).value(),
            qnan,
        );
        assert_eq!(
            *Binary32::subtract_raw_result(negative_qnan, qnan, Nearest).value(),
            negative_qnan,
        );
        let negative_selected = Binary32::subtract_raw_result(negative_snan, qnan, Nearest);
        assert_eq!(*negative_selected.value(), quieted_negative_snan);
        assert!(negative_selected.invalid_operation());
        let qnan_selected_over_snan = Binary32::subtract_raw_result(qnan, negative_snan, Nearest);
        assert_eq!(*qnan_selected_over_snan.value(), qnan);
        assert!(qnan_selected_over_snan.invalid_operation());
    }

    #[test]
    fn raw_binary64_add_sub_nan_selection_and_invalid_are_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let qnan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let negative_qnan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let snan = Binary64::from_bits(0x7ff0_1234_5678_9abc);
        let quieted_snan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let negative_snan = Binary64::from_bits(0xfff0_2345_6789_abcd);
        let quieted_negative_snan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000);

        for result in [
            Binary64::add_raw_result(qnan, snan, Nearest),
            Binary64::subtract_raw_result(qnan, snan, Nearest),
        ] {
            assert_eq!(*result.value(), qnan);
            assert!(result.invalid_operation());
            assert!(!result.inexact() && !result.overflow());
        }
        for result in [
            Binary64::add_raw_result(snan, qnan, Nearest),
            Binary64::subtract_raw_result(snan, qnan, Nearest),
        ] {
            assert_eq!(*result.value(), quieted_snan);
            assert!(result.invalid_operation());
        }

        for result in [
            Binary64::add_raw_result(one, negative_qnan, Nearest),
            Binary64::subtract_raw_result(one, negative_qnan, Nearest),
        ] {
            assert_eq!(*result.value(), negative_qnan);
            assert!(!result.invalid_operation());
        }
        for result in [
            Binary64::add_raw_result(one, snan, Nearest),
            Binary64::subtract_raw_result(one, snan, Nearest),
        ] {
            assert_eq!(*result.value(), quieted_snan);
            assert!(result.invalid_operation());
        }
        assert_eq!(
            *Binary64::add_raw_result(qnan, negative_qnan, Nearest).value(),
            qnan,
        );
        assert_eq!(
            *Binary64::subtract_raw_result(negative_qnan, qnan, Nearest).value(),
            negative_qnan,
        );
        let negative_selected = Binary64::subtract_raw_result(negative_snan, qnan, Nearest);
        assert_eq!(*negative_selected.value(), quieted_negative_snan);
        assert!(negative_selected.invalid_operation());
        let qnan_selected_over_snan = Binary64::subtract_raw_result(qnan, negative_snan, Nearest);
        assert_eq!(*qnan_selected_over_snan.value(), qnan);
        assert!(qnan_selected_over_snan.invalid_operation());
    }

    #[test]
    fn raw_binary32_add_sub_infinity_table_uses_indefinite_on_cancellation() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let cases = [
            (0x7f80_0000, 0x7f80_0000, false, 0x7f80_0000, false),
            (0x7f80_0000, 0xff80_0000, false, 0xffc0_0000, true),
            (0xff80_0000, 0x7f80_0000, false, 0xffc0_0000, true),
            (0x7f80_0000, 0x7f80_0000, true, 0xffc0_0000, true),
            (0x7f80_0000, 0xff80_0000, true, 0x7f80_0000, false),
            (0xff80_0000, 0xff80_0000, false, 0xff80_0000, false),
            (0xff80_0000, 0xff80_0000, true, 0xffc0_0000, true),
            (0xff80_0000, 0x7f80_0000, true, 0xff80_0000, false),
            (0x3f80_0000, 0xff80_0000, false, 0xff80_0000, false),
            (0x3f80_0000, 0x7f80_0000, false, 0x7f80_0000, false),
            (0x3f80_0000, 0x7f80_0000, true, 0xff80_0000, false),
            (0x3f80_0000, 0xff80_0000, true, 0x7f80_0000, false),
            (0xff80_0000, 0x3f80_0000, true, 0xff80_0000, false),
        ];
        for (lhs, rhs, subtract, expected, invalid) in cases {
            let result = if subtract {
                Binary32::subtract_raw_result(
                    Binary32::from_bits(lhs),
                    Binary32::from_bits(rhs),
                    Nearest,
                )
            } else {
                Binary32::add_raw_result(
                    Binary32::from_bits(lhs),
                    Binary32::from_bits(rhs),
                    Nearest,
                )
            };
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
        }
    }

    #[test]
    fn raw_binary64_add_sub_infinity_table_uses_indefinite_on_cancellation() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let cases = [
            (
                0x7ff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                false,
                0x7ff0_0000_0000_0000,
                false,
            ),
            (
                0x7ff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                false,
                0xfff8_0000_0000_0000,
                true,
            ),
            (
                0xfff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                false,
                0xfff8_0000_0000_0000,
                true,
            ),
            (
                0x7ff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                true,
                0xfff8_0000_0000_0000,
                true,
            ),
            (
                0xfff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                true,
                0xfff8_0000_0000_0000,
                true,
            ),
            (
                0x7ff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                true,
                0x7ff0_0000_0000_0000,
                false,
            ),
            (
                0xfff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                true,
                0xfff0_0000_0000_0000,
                false,
            ),
            (
                0xfff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                false,
                0xfff0_0000_0000_0000,
                false,
            ),
            (
                0x3ff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                false,
                0x7ff0_0000_0000_0000,
                false,
            ),
            (
                0x3ff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                false,
                0xfff0_0000_0000_0000,
                false,
            ),
            (
                0x3ff0_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                true,
                0xfff0_0000_0000_0000,
                false,
            ),
            (
                0x3ff0_0000_0000_0000,
                0xfff0_0000_0000_0000,
                true,
                0x7ff0_0000_0000_0000,
                false,
            ),
            (
                0xfff0_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                true,
                0xfff0_0000_0000_0000,
                false,
            ),
        ];
        for (lhs, rhs, subtract, expected, invalid) in cases {
            let result = if subtract {
                Binary64::subtract_raw_result(
                    Binary64::from_bits(lhs),
                    Binary64::from_bits(rhs),
                    Nearest,
                )
            } else {
                Binary64::add_raw_result(
                    Binary64::from_bits(lhs),
                    Binary64::from_bits(rhs),
                    Nearest,
                )
            };
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
        }
    }

    #[test]
    fn raw_add_sub_signed_zero_literals_follow_effective_terms_and_rounding_mode() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        for (mode, add_expected, subtract_expected) in [
            (
                Nearest,
                [0x0000_0000, 0x0000_0000, 0x0000_0000, 0x8000_0000],
                [0x0000_0000, 0x0000_0000, 0x8000_0000, 0x0000_0000],
            ),
            (
                Zero,
                [0x0000_0000, 0x0000_0000, 0x0000_0000, 0x8000_0000],
                [0x0000_0000, 0x0000_0000, 0x8000_0000, 0x0000_0000],
            ),
            (
                Up,
                [0x0000_0000, 0x0000_0000, 0x0000_0000, 0x8000_0000],
                [0x0000_0000, 0x0000_0000, 0x8000_0000, 0x0000_0000],
            ),
            (
                Down,
                [0x0000_0000, 0x8000_0000, 0x8000_0000, 0x8000_0000],
                [0x8000_0000, 0x0000_0000, 0x8000_0000, 0x8000_0000],
            ),
        ] {
            let zeros32 = [
                Binary32::from_bits(0x0000_0000),
                Binary32::from_bits(0x8000_0000),
            ];
            let zeros64 = [
                Binary64::from_bits(0x0000_0000_0000_0000),
                Binary64::from_bits(0x8000_0000_0000_0000),
            ];
            let mut index = 0;
            for lhs_sign in 0..2 {
                for rhs_sign in 0..2 {
                    let add32 =
                        Binary32::add_raw_result(zeros32[lhs_sign], zeros32[rhs_sign], mode);
                    let sub32 =
                        Binary32::subtract_raw_result(zeros32[lhs_sign], zeros32[rhs_sign], mode);
                    let add64 =
                        Binary64::add_raw_result(zeros64[lhs_sign], zeros64[rhs_sign], mode);
                    let sub64 =
                        Binary64::subtract_raw_result(zeros64[lhs_sign], zeros64[rhs_sign], mode);
                    assert_eq!(add32.value().to_bits(), add_expected[index]);
                    assert_eq!(
                        sub32.value().to_bits(),
                        subtract_expected[index],
                        "binary32 subtraction zero mode={mode:?}, index={index}",
                    );
                    assert_eq!(
                        add64.value().to_bits(),
                        u64::from(add_expected[index]) << 32
                    );
                    assert_eq!(
                        sub64.value().to_bits(),
                        u64::from(subtract_expected[index]) << 32,
                    );
                    for result in [add32, sub32] {
                        assert!(!result.invalid_operation());
                        assert!(!result.inexact() && !result.overflow());
                        assert!(!result.tiny_after_precision_rounding());
                    }
                    for result in [add64, sub64] {
                        assert!(!result.invalid_operation());
                        assert!(!result.inexact() && !result.overflow());
                        assert!(!result.tiny_after_precision_rounding());
                    }
                    index += 1;
                }
            }
        }
    }

    #[test]
    fn raw_finite_add_sub_delegates_all_same_format_values_to_s2_36p() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        for (lhs_bits, rhs_bits) in [
            (0x3f80_0000_u32, 1_u32),
            (0x3f80_0000, 0xbf80_0000),
            (0x7f7f_ffff, 0x7f7f_ffff),
            (0x0080_0000, 0x807f_ffff),
            (0x0000_0000, 0xbf80_0000),
        ] {
            let lhs_raw = Binary32::from_bits(lhs_bits);
            let rhs_raw = Binary32::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                for (raw, finite) in [
                    (
                        Binary32::add_raw_result(lhs_raw, rhs_raw, mode),
                        Binary32::add_finite_aligned_result(lhs, rhs, mode).unwrap(),
                    ),
                    (
                        Binary32::subtract_raw_result(lhs_raw, rhs_raw, mode),
                        Binary32::subtract_finite_aligned_result(lhs, rhs, mode).unwrap(),
                    ),
                ] {
                    assert_eq!(raw.value(), finite.value());
                    assert_eq!(raw.inexact(), finite.inexact());
                    assert_eq!(raw.overflow(), finite.overflow());
                    assert_eq!(
                        raw.tiny_after_precision_rounding(),
                        finite.tiny_after_precision_rounding(),
                    );
                    assert!(!raw.invalid_operation());
                }
            }
        }

        for (lhs_bits, rhs_bits) in [
            (0x3ff0_0000_0000_0000_u64, 1_u64),
            (0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0000),
            (0x7fef_ffff_ffff_ffff, 0x7fef_ffff_ffff_ffff),
            (0x0010_0000_0000_0000, 0x800f_ffff_ffff_ffff),
            (0x0000_0000_0000_0000, 0xbff0_0000_0000_0000),
        ] {
            let lhs_raw = Binary64::from_bits(lhs_bits);
            let rhs_raw = Binary64::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                for (raw, finite) in [
                    (
                        Binary64::add_raw_result(lhs_raw, rhs_raw, mode),
                        Binary64::add_finite_aligned_result(lhs, rhs, mode).unwrap(),
                    ),
                    (
                        Binary64::subtract_raw_result(lhs_raw, rhs_raw, mode),
                        Binary64::subtract_finite_aligned_result(lhs, rhs, mode).unwrap(),
                    ),
                ] {
                    assert_eq!(raw.value(), finite.value());
                    assert_eq!(raw.inexact(), finite.inexact());
                    assert_eq!(raw.overflow(), finite.overflow());
                    assert_eq!(
                        raw.tiny_after_precision_rounding(),
                        finite.tiny_after_precision_rounding(),
                    );
                    assert!(!raw.invalid_operation());
                }
            }
        }
    }
}
