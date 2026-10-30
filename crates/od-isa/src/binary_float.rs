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

/// Result of a raw binary arithmetic or exact widening operation.
///
/// The rounding diagnostics have the same local mathematical meaning as
/// [`BinaryRoundingResult`]. They are false for exact widening conversions.
/// `invalid_operation` is a local operation outcome, including signaling-NaN
/// quieting during widening; it is not an architectural exception flag or
/// delivery decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryArithmeticResult<T> {
    rounding: BinaryRoundingResult<T>,
    invalid_operation: bool,
}

/// Result of a local raw binary32/binary64 to signed integer value conversion.
///
/// `invalid_operation` describes this helper's conversion outcome. It is not
/// an architectural exception flag or a decision about exception delivery.
/// The default integer type remains `i32` for source compatibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryIntegerResult<T = i32> {
    value: T,
    inexact: bool,
    invalid_operation: bool,
}

impl<T> BinaryIntegerResult<T> {
    const fn new(value: T, inexact: bool, invalid_operation: bool) -> Self {
        Self {
            value,
            inexact,
            invalid_operation,
        }
    }

    /// Returns the converted signed integer, or the operation's signed-indefinite
    /// sentinel when the conversion is invalid.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns whether a valid conversion discarded a fractional source part.
    pub const fn inexact(&self) -> bool {
        self.inexact
    }

    /// Returns whether the source was non-finite or the rounded integer was out of range.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }
}

impl<T> BinaryArithmeticResult<T> {
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

    /// Returns the local invalid-operation outcome for this operation.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }
}

/// Compatibility name for the same-format add/sub result type.
pub type BinaryAddSubResult<T> = BinaryArithmeticResult<T>;

/// Result of a same-format binary32/binary64 division.
///
/// Rounding diagnostics describe the bounded mathematical result. The two
/// booleans are local operation outcomes; neither is an architectural status
/// bit or an exception-delivery decision.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryDivisionResult<T> {
    rounding: BinaryRoundingResult<T>,
    invalid_operation: bool,
    divide_by_zero: bool,
}

/// Comparison mode that controls whether quiet NaNs signal a local invalid result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryComparisonMode {
    /// Invalid is reported only for signaling NaN operands.
    Quiet,
    /// Invalid is reported for either quiet or signaling NaN operands.
    Signaling,
}

/// Numerical relation produced by a same-format binary comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryComparisonRelation {
    Less,
    Equal,
    Greater,
    Unordered,
}

/// Relation and local invalid-operation outcome for a binary comparison.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinaryComparisonResult {
    relation: BinaryComparisonRelation,
    invalid_operation: bool,
}

impl BinaryComparisonResult {
    const fn new(relation: BinaryComparisonRelation, invalid_operation: bool) -> Self {
        Self {
            relation,
            invalid_operation,
        }
    }

    /// Returns the numerical relation, including `Unordered` for any NaN.
    pub const fn relation(&self) -> BinaryComparisonRelation {
        self.relation
    }

    /// Returns the local invalid-operation outcome.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }
}

/// Selected raw value and local invalid-operation outcome for MIN/MAX.
///
/// This is a value-only result. It does not update architectural state, deliver
/// exceptions, or report denormal-input status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BinarySelectionResult<T> {
    value: T,
    invalid_operation: bool,
}

impl<T> BinarySelectionResult<T> {
    const fn new(value: T, invalid_operation: bool) -> Self {
        Self {
            value,
            invalid_operation,
        }
    }

    /// Returns the selected value, preserving its raw encoding.
    pub const fn value(&self) -> &T {
        &self.value
    }

    /// Returns whether either source was a NaN.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }
}

impl<T> BinaryDivisionResult<T> {
    const fn new(
        rounding: BinaryRoundingResult<T>,
        invalid_operation: bool,
        divide_by_zero: bool,
    ) -> Self {
        Self {
            rounding,
            invalid_operation,
            divide_by_zero,
        }
    }

    fn exact(value: T, invalid_operation: bool, divide_by_zero: bool) -> Self {
        Self::new(
            BinaryRoundingResult::new(value, false, false, false),
            invalid_operation,
            divide_by_zero,
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

    /// Returns the local invalid-operation outcome.
    pub const fn invalid_operation(&self) -> bool {
        self.invalid_operation
    }

    /// Returns the local finite-nonzero divided by zero outcome.
    pub const fn divide_by_zero(&self) -> bool {
        self.divide_by_zero
    }
}

/// An immutable binary32 encoding.
///
/// Equality is raw-bit identity, not guest floating-point equality.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Binary32(u32);

/// Returns the exact normalized binary significand for a signed integer.
///
/// The zero tuple is canonical and signless. For nonzero values, `significand`
/// has bit 63 set and the exact magnitude is
/// `significand * 2^(exponent - 63)`.
fn normalize_i64(value: i64) -> (bool, i32, u64) {
    if value == 0 {
        return (false, 0, 0);
    }

    let sign = value < 0;
    let magnitude = value.unsigned_abs();
    let leading_bit = 63 - magnitude.leading_zeros() as i32;
    let significand = magnitude << (63 - leading_bit);
    (sign, leading_bit, significand)
}

/// Returns `floor(sqrt(value))` for the full `u128` domain.
///
/// This restoring digit-by-digit algorithm processes two input bits per step
/// and never forms a potentially overflowing square or trial product.
fn integer_sqrt_u128(value: u128) -> u64 {
    let mut remainder = value;
    let mut root = 0_u128;
    let mut bit = 1_u128 << 126;
    while bit > remainder {
        bit >>= 2;
    }
    while bit != 0 {
        let trial = root + bit;
        if remainder >= trial {
            remainder -= trial;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root as u64
}

/// Builds an integer radicand for a normalized nonzero finite value.
///
/// For `x = S * 2^(e-63)`, this returns `(E, q, has_tail)` with
/// `E=floor(e/2)` and `q=floor(sqrt(S*2^(63+b)))`, where `b=e-2E`.
/// The exact root is represented as `(q + δ) * 2^(E-63)`, with `δ=0` for an
/// exact integer root and `0<δ<1` otherwise. The caller passes a rounding-
/// equivalent representative tail (`0/1` or `1/2`) to the existing packer;
/// this is not the exact fractional root remainder. It is sufficient for
/// binary32 and binary64 because their discarded significand widths are 40
/// and 11 bits: a nonzero tail only changes rounding when `q` is exactly at
/// the halfway boundary, where every positive tail rounds above that tie.
fn sqrt_finite_parts(finite: BinaryFinite) -> (i32, u64, bool) {
    debug_assert_ne!(finite.significand(), 0);
    let exponent = finite.exponent();
    let result_exponent = exponent.div_euclid(2);
    let parity = exponent.rem_euclid(2) as u32;
    let radicand = u128::from(finite.significand()) << (63 + parity);
    let root = integer_sqrt_u128(radicand);
    let square = u128::from(root) * u128::from(root);
    let remainder = radicand - square;
    debug_assert!(remainder < (u128::from(root) * 2 + 1));
    (result_exponent, root, remainder != 0)
}

/// Rounds the magnitude of one exact finite binary value to an integer.
///
/// Exponents above 63 cannot fit any signed 64-bit destination and are
/// rejected by this shared bounded helper. Typed wrappers validate their own
/// destination range after rounding.
fn round_finite_integer_magnitude(
    finite: BinaryFinite,
    mode: RationalRoundingMode,
) -> Option<(u128, bool)> {
    let exponent = finite.exponent();
    let significand = finite.significand();

    if significand == 0 {
        return Some((0, false));
    }
    if exponent > 63 {
        return None;
    }
    if exponent < 0 {
        let increment = match mode {
            RationalRoundingMode::NearestTiesEven => exponent == -1 && significand > (1_u64 << 63),
            RationalRoundingMode::TowardZero => false,
            RationalRoundingMode::TowardPositive => !finite.sign(),
            RationalRoundingMode::TowardNegative => finite.sign(),
        };
        return Some((u128::from(increment), true));
    }

    crate::rational_rounding::round_quotient_to_precision(
        finite.sign(),
        significand,
        0,
        1,
        exponent as u32 + 1,
        mode,
    )
}

/// Rounds one exact finite binary value to an integer and validates i32 range.
fn round_finite_to_i32(
    finite: BinaryFinite,
    mode: RationalRoundingMode,
) -> BinaryIntegerResult<i32> {
    let sign = finite.sign();
    let Some((magnitude, inexact)) = round_finite_integer_magnitude(finite, mode) else {
        return BinaryIntegerResult::new(i32::MIN, false, true);
    };
    let limit = if sign { 1_u128 << 31 } else { i32::MAX as u128 };
    if magnitude > limit {
        return BinaryIntegerResult::new(i32::MIN, false, true);
    }
    let value = if sign {
        if magnitude == 1_u128 << 31 {
            i32::MIN
        } else {
            -(magnitude as i32)
        }
    } else {
        magnitude as i32
    };
    BinaryIntegerResult::new(value, inexact, false)
}

/// Rounds one exact finite binary value to an integer and validates i64 range.
fn round_finite_to_i64(
    finite: BinaryFinite,
    mode: RationalRoundingMode,
) -> BinaryIntegerResult<i64> {
    let sign = finite.sign();
    let Some((magnitude, inexact)) = round_finite_integer_magnitude(finite, mode) else {
        return BinaryIntegerResult::new(i64::MIN, false, true);
    };
    let limit = if sign { 1_u128 << 63 } else { i64::MAX as u128 };
    if magnitude > limit {
        return BinaryIntegerResult::new(i64::MIN, false, true);
    }
    let value = if sign {
        if magnitude == 1_u128 << 63 {
            i64::MIN
        } else {
            -(magnitude as i64)
        }
    } else {
        magnitude as i64
    };
    BinaryIntegerResult::new(value, inexact, false)
}

impl Binary32 {
    const SIGN_MASK: u32 = 1 << 31;
    const EXPONENT_MASK: u32 = 0xff;
    const FRACTION_MASK: u32 = (1 << 23) - 1;
    const QUIET_BIT: u32 = 1 << 22;

    /// Constructs a binary32 value without interpreting or changing its bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Converts a raw finite encoding to signed i32 using `mode`.
    ///
    /// Rounding is applied to the exact source value before the rounded
    /// integer is checked against the i32 range. This local value conversion
    /// does not modify the source, CPU state, or MXCSR and does not deliver
    /// architectural exceptions. Non-finite or out-of-range inputs return
    /// the `i32::MIN` sentinel with `invalid_operation` set.
    pub fn to_i32(self, mode: RationalRoundingMode) -> BinaryIntegerResult<i32> {
        match self.normalize_finite() {
            Some(finite) => round_finite_to_i32(finite, mode),
            None => BinaryIntegerResult::new(i32::MIN, false, true),
        }
    }

    /// Converts a raw finite encoding to signed i64 using `mode`.
    ///
    /// Rounding is applied to the exact source value before the rounded
    /// integer is checked against the i64 range. This local value conversion
    /// does not modify the source, CPU state, or MXCSR and does not deliver
    /// architectural exceptions. Non-finite or out-of-range inputs return the
    /// `i64::MIN` sentinel with `invalid_operation` set.
    pub fn to_i64(self, mode: RationalRoundingMode) -> BinaryIntegerResult<i64> {
        match self.normalize_finite() {
            Some(finite) => round_finite_to_i64(finite, mode),
            None => BinaryIntegerResult::new(i64::MIN, false, true),
        }
    }

    /// Converts a signed 64-bit integer directly to binary32 using `mode`.
    ///
    /// The integer is normalized exactly and rounded once to binary32
    /// precision. This value-only conversion uses no host floating point, does
    /// not modify MXCSR or CPU state, and does not model instruction exception
    /// delivery. Zero is always positive zero. Nonzero results are normal and
    /// finite; only `inexact` may be set in the rounding diagnostics.
    pub fn from_i64(value: i64, mode: RationalRoundingMode) -> BinaryRoundingResult<Self> {
        if value == 0 {
            return BinaryRoundingResult::new(Self::from_bits(0), false, false, false);
        }
        let (sign, exponent, significand) = normalize_i64(value);
        Self::round_pack_quotient(sign, exponent, significand, 0, 1, mode)
            .expect("normalized i64 significand is valid for binary32 rounding")
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

    /// Narrows a raw binary64 encoding to binary32 using the supplied mode.
    ///
    /// Finite values are rounded once from their exact normalized binary64
    /// significand directly to binary32 precision and range. Signed zero and
    /// infinity preserve their sign. NaN payload bits are truncated to the
    /// most significant 23 source fraction bits; signaling NaNs are quieted
    /// and report a local invalid-operation outcome. This is a value-only
    /// conversion: it does not use host floating point, update CPU/MXCSR state,
    /// or model instruction exception delivery, DAZ, or FTZ.
    pub fn narrow_from_binary64(
        source: Binary64,
        mode: RationalRoundingMode,
    ) -> BinaryArithmeticResult<Self> {
        let sign = (source.sign() as u32) << 31;
        let class = source.class();
        match class {
            BinaryClass::Zero => BinaryArithmeticResult::exact(Self::from_bits(sign), false),
            BinaryClass::Infinity => {
                BinaryArithmeticResult::exact(Self::from_bits(sign | 0x7f80_0000), false)
            }
            BinaryClass::QuietNaN | BinaryClass::SignalingNaN => {
                let signaling = class == BinaryClass::SignalingNaN;
                let mut fraction = (source.fraction() >> 29) as u32;
                if signaling {
                    fraction |= Self::QUIET_BIT;
                }
                BinaryArithmeticResult::exact(
                    Self::from_bits(sign | 0x7f80_0000 | fraction),
                    signaling,
                )
            }
            BinaryClass::Subnormal | BinaryClass::Normal => {
                let finite = source
                    .normalize_finite()
                    .expect("finite binary64 source has an exact normalized form");
                let rounded = Self::round_pack_quotient(
                    finite.sign(),
                    finite.exponent(),
                    finite.significand(),
                    0,
                    1,
                    mode,
                )
                .expect("normalized binary64 source is valid for binary32 rounding");
                BinaryArithmeticResult::new(
                    BinaryRoundingResult::new(
                        *rounded.value(),
                        rounded.inexact(),
                        rounded.overflow(),
                        rounded.tiny_after_precision_rounding(),
                    ),
                    false,
                )
            }
        }
    }

    /// Compares same-format raw binary32 operands without changing their bits.
    ///
    /// Either NaN produces `Unordered`. Quiet mode reports invalid only for a
    /// signaling NaN; signaling mode reports invalid for every NaN. This is a
    /// local comparison result and does not update flags or CPU state.
    pub fn compare_raw(lhs: Self, rhs: Self, mode: BinaryComparisonMode) -> BinaryComparisonResult {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        let lhs_nan = is_nan_class(lhs_class);
        let rhs_nan = is_nan_class(rhs_class);
        if lhs_nan || rhs_nan {
            let invalid = mode == BinaryComparisonMode::Signaling
                || lhs_class == BinaryClass::SignalingNaN
                || rhs_class == BinaryClass::SignalingNaN;
            return BinaryComparisonResult::new(BinaryComparisonRelation::Unordered, invalid);
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        if lhs_infinity && rhs_infinity {
            return BinaryComparisonResult::new(
                if lhs.sign() == rhs.sign() {
                    BinaryComparisonRelation::Equal
                } else if lhs.sign() {
                    BinaryComparisonRelation::Less
                } else {
                    BinaryComparisonRelation::Greater
                },
                false,
            );
        }
        if lhs_infinity {
            return BinaryComparisonResult::new(
                if lhs.sign() {
                    BinaryComparisonRelation::Less
                } else {
                    BinaryComparisonRelation::Greater
                },
                false,
            );
        }
        if rhs_infinity {
            return BinaryComparisonResult::new(
                if rhs.sign() {
                    BinaryComparisonRelation::Greater
                } else {
                    BinaryComparisonRelation::Less
                },
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary32 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary32 operand");
        BinaryComparisonResult::new(
            comparison_relation(compare_finite(lhs_finite, rhs_finite)),
            false,
        )
    }

    /// Selects the raw minimum of two binary32 values.
    ///
    /// NaN inputs select the right operand unchanged and report local invalid
    /// operation, including for quiet NaNs. Equal values select the right
    /// operand. Finite values, including subnormals, are compared by their raw
    /// numeric values without DAZ/FTZ or denormal-exception handling.
    pub fn min_sse_raw_result(lhs: Self, rhs: Self) -> BinarySelectionResult<Self> {
        Self::select_min_max_raw(lhs, rhs, true)
    }

    /// Selects the raw maximum of two binary32 values.
    ///
    /// NaN inputs select the right operand unchanged and report local invalid
    /// operation, including for quiet NaNs. Equal values select the right
    /// operand. Finite values, including subnormals, are compared by their raw
    /// numeric values without DAZ/FTZ or denormal-exception handling.
    pub fn max_sse_raw_result(lhs: Self, rhs: Self) -> BinarySelectionResult<Self> {
        Self::select_min_max_raw(lhs, rhs, false)
    }

    fn select_min_max_raw(lhs: Self, rhs: Self, minimum: bool) -> BinarySelectionResult<Self> {
        if is_nan_class(lhs.class()) || is_nan_class(rhs.class()) {
            return BinarySelectionResult::new(rhs, true);
        }
        let relation = Self::compare_raw(lhs, rhs, BinaryComparisonMode::Quiet).relation();
        let select_lhs = if minimum {
            relation == BinaryComparisonRelation::Less
        } else {
            relation == BinaryComparisonRelation::Greater
        };
        BinarySelectionResult::new(if select_lhs { lhs } else { rhs }, false)
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

    /// Computes a same-format raw binary32 square root with one rounding step.
    ///
    /// This pure value helper preserves signed zero and positive infinity.
    /// Negative nonzero values, including negative infinity, return the
    /// negative Intel indefinite QNaN and set the local invalid outcome. NaNs
    /// follow a local sign/payload-preserving policy: QNaN is returned as-is,
    /// while SNaN is quieted by setting its quiet bit; only SNaN sets invalid.
    /// Positive finite inputs use integer-only square root and one existing
    /// round-and-pack operation. The returned diagnostics are local and do not
    /// update MXCSR or deliver architectural exceptions.
    pub fn sqrt_raw_result(self, mode: RationalRoundingMode) -> BinaryArithmeticResult<Self> {
        match self.class() {
            BinaryClass::Zero => BinaryArithmeticResult::exact(self, false),
            BinaryClass::Infinity if !self.sign() => BinaryArithmeticResult::exact(self, false),
            BinaryClass::Infinity => {
                BinaryArithmeticResult::exact(Self::from_bits(0xffc0_0000), true)
            }
            BinaryClass::QuietNaN => BinaryArithmeticResult::exact(self, false),
            BinaryClass::SignalingNaN => {
                BinaryArithmeticResult::exact(Self::from_bits(self.0 | Self::QUIET_BIT), true)
            }
            BinaryClass::Subnormal | BinaryClass::Normal if self.sign() => {
                BinaryArithmeticResult::exact(Self::from_bits(0xffc0_0000), true)
            }
            BinaryClass::Subnormal | BinaryClass::Normal => {
                let finite = self.normalize_finite().expect("finite binary32 operand");
                let (exponent, quotient, has_tail) = sqrt_finite_parts(finite);
                let rounded = Self::round_pack_quotient(
                    false,
                    exponent,
                    quotient,
                    u64::from(has_tail),
                    if has_tail { 2 } else { 1 },
                    mode,
                )
                .expect("positive finite binary32 square root remains normal");
                BinaryArithmeticResult::new(rounded, false)
            }
        }
    }

    /// Computes the deterministic local raw binary32 SSE reciprocal reference.
    ///
    /// This policy treats signed zero and every signed subnormal input as a
    /// same-sign infinity, maps infinity to same-sign zero, returns QNaN
    /// unchanged, and quiets SNaN by setting its quiet bit. Normal finite
    /// values use exact same-format division of `1.0 / self`, rounded once to
    /// nearest-ties-even; a subnormal rounded result is flushed to same-sign
    /// zero. The helper is value-only: it does not consult or update MXCSR,
    /// report diagnostics, or deliver SIMD floating-point exceptions. Its
    /// finite result is a deterministic reference policy, not a promise of
    /// identical hardware bits.
    pub fn reciprocal_sse_raw(self) -> Self {
        let sign = self.0 & Self::SIGN_MASK;
        match self.class() {
            BinaryClass::Zero | BinaryClass::Subnormal => Self::from_bits(sign | 0x7f80_0000),
            BinaryClass::Infinity => Self::from_bits(sign),
            BinaryClass::QuietNaN => self,
            BinaryClass::SignalingNaN => Self::from_bits(self.0 | Self::QUIET_BIT),
            BinaryClass::Normal => {
                let numerator = Self::from_bits(0x3f80_0000)
                    .normalize_finite()
                    .expect("one is finite");
                let denominator = self.normalize_finite().expect("normal is finite");
                let rounded = *Self::divide_finite_result(
                    numerator,
                    denominator,
                    RationalRoundingMode::NearestTiesEven,
                )
                .expect("normal divisor is nonzero")
                .value();
                if rounded.class() == BinaryClass::Subnormal {
                    Self::from_bits(rounded.0 & Self::SIGN_MASK)
                } else {
                    rounded
                }
            }
        }
    }

    /// Computes a deterministic local raw binary32 SSE reciprocal-square-root reference.
    ///
    /// Signed zero and every signed subnormal map to same-sign infinity before
    /// the sign of a finite input is considered. Negative normal values and
    /// negative infinity map to the negative indefinite QNaN; positive
    /// infinity maps to positive zero, inferred from `1/sqrt(x)`. QNaN is
    /// preserved and SNaN is quieted by setting its quiet bit while preserving
    /// sign and payload. Positive normal values compose the existing
    /// nearest-ties-even square root and deterministic SSE reciprocal helpers.
    /// This is a pure value policy: it does not report diagnostics, mutate
    /// architectural state, consult MXCSR, or claim bitwise hardware matching.
    pub fn reciprocal_sqrt_sse_raw(self) -> Self {
        let sign = self.0 & Self::SIGN_MASK;
        match self.class() {
            BinaryClass::Zero | BinaryClass::Subnormal => Self::from_bits(sign | 0x7f80_0000),
            BinaryClass::Infinity if sign != 0 => Self::from_bits(0xffc0_0000),
            BinaryClass::Infinity => Self::from_bits(0),
            BinaryClass::QuietNaN => self,
            BinaryClass::SignalingNaN => Self::from_bits(self.0 | Self::QUIET_BIT),
            BinaryClass::Normal if sign != 0 => Self::from_bits(0xffc0_0000),
            BinaryClass::Normal => {
                let square_root = *self
                    .sqrt_raw_result(RationalRoundingMode::NearestTiesEven)
                    .value();
                square_root.reciprocal_sse_raw()
            }
        }
    }

    /// Multiplies same-format raw binary32 operands and returns result diagnostics.
    ///
    /// NaNs use the Intel SDM Rev. 093 first-source rule and signaling NaNs set
    /// the local invalid-operation outcome. Zero times infinity returns the
    /// Intel indefinite QNaN. Finite operands use the single-rounding finite
    /// result path; no architectural state or exception delivery is updated.
    pub fn multiply_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryArithmeticResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        if let Some(result) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return result;
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let lhs_zero = lhs_class == BinaryClass::Zero;
        let rhs_zero = rhs_class == BinaryClass::Zero;
        if (lhs_infinity && rhs_zero) || (lhs_zero && rhs_infinity) {
            return BinaryArithmeticResult::exact(Self::from_bits(0xffc0_0000), true);
        }

        let sign = lhs.sign() ^ rhs.sign();
        if lhs_infinity || rhs_infinity {
            return BinaryArithmeticResult::exact(
                Self::from_bits((Self::EXPONENT_MASK << 23) | (u32::from(sign) << 31)),
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary32 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary32 operand");
        BinaryArithmeticResult::new(
            Self::multiply_finite_result(lhs_finite, rhs_finite, mode),
            false,
        )
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

    /// Divides same-format raw binary32 operands and returns result diagnostics.
    ///
    /// NaNs are propagated before zero-divisor classification, so a NaN
    /// denominator never reports divide-by-zero. Only finite nonzero divided
    /// by zero sets the local divide-by-zero outcome. Finite pairs use the
    /// single-rounding finite result path; no architectural state or exception
    /// delivery is updated.
    pub fn divide_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryDivisionResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        if let Some(nan) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return BinaryDivisionResult::exact(*nan.value(), nan.invalid_operation(), false);
        }

        let sign = lhs.sign() ^ rhs.sign();
        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let lhs_zero = lhs_class == BinaryClass::Zero;
        let rhs_zero = rhs_class == BinaryClass::Zero;

        if (lhs_zero && rhs_zero) || (lhs_infinity && rhs_infinity) {
            return BinaryDivisionResult::exact(Self::from_bits(0xffc0_0000), true, false);
        }
        if rhs_zero {
            let infinity = Self::from_bits((Self::EXPONENT_MASK << 23) | (u32::from(sign) << 31));
            return BinaryDivisionResult::exact(infinity, false, !lhs_infinity);
        }
        if lhs_infinity {
            return BinaryDivisionResult::exact(
                Self::from_bits((Self::EXPONENT_MASK << 23) | (u32::from(sign) << 31)),
                false,
                false,
            );
        }
        if rhs_infinity {
            return BinaryDivisionResult::exact(
                Self::from_bits(u32::from(sign) << 31),
                false,
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary32 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary32 operand");
        let rounded = Self::divide_finite_result(lhs_finite, rhs_finite, mode)
            .expect("finite binary32 denominator is nonzero");
        BinaryDivisionResult::new(rounded, false, false)
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
        if let Some(result) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return result;
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

    fn propagate_nan(
        lhs: Self,
        rhs: Self,
        lhs_class: BinaryClass,
        rhs_class: BinaryClass,
    ) -> Option<BinaryArithmeticResult<Self>> {
        let lhs_nan = matches!(lhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        let rhs_nan = matches!(rhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        if !lhs_nan && !rhs_nan {
            return None;
        }
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
        Some(BinaryArithmeticResult::exact(
            value,
            lhs_class == BinaryClass::SignalingNaN || rhs_class == BinaryClass::SignalingNaN,
        ))
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

    /// Converts a raw finite encoding to signed i32 using `mode`.
    ///
    /// Rounding is applied to the exact source value before the rounded
    /// integer is checked against the i32 range. This local value conversion
    /// does not modify the source, CPU state, or MXCSR and does not deliver
    /// architectural exceptions. Non-finite or out-of-range inputs return
    /// the `i32::MIN` sentinel with `invalid_operation` set.
    pub fn to_i32(self, mode: RationalRoundingMode) -> BinaryIntegerResult<i32> {
        match self.normalize_finite() {
            Some(finite) => round_finite_to_i32(finite, mode),
            None => BinaryIntegerResult::new(i32::MIN, false, true),
        }
    }

    /// Converts a raw finite encoding to signed i64 using `mode`.
    ///
    /// Rounding is applied to the exact source value before the rounded
    /// integer is checked against the i64 range. This local value conversion
    /// does not modify the source, CPU state, or MXCSR and does not deliver
    /// architectural exceptions. Non-finite or out-of-range inputs return the
    /// `i64::MIN` sentinel with `invalid_operation` set.
    pub fn to_i64(self, mode: RationalRoundingMode) -> BinaryIntegerResult<i64> {
        match self.normalize_finite() {
            Some(finite) => round_finite_to_i64(finite, mode),
            None => BinaryIntegerResult::new(i64::MIN, false, true),
        }
    }

    /// Converts a signed 64-bit integer directly to binary64 using `mode`.
    ///
    /// The integer is normalized exactly and rounded once to binary64
    /// precision. This value-only conversion uses no host floating point, does
    /// not modify MXCSR or CPU state, and does not model instruction exception
    /// delivery. Zero is always positive zero. Nonzero results are normal and
    /// finite; only `inexact` may be set in the rounding diagnostics.
    pub fn from_i64(value: i64, mode: RationalRoundingMode) -> BinaryRoundingResult<Self> {
        if value == 0 {
            return BinaryRoundingResult::new(Self::from_bits(0), false, false, false);
        }
        let (sign, exponent, significand) = normalize_i64(value);
        Self::round_pack_quotient(sign, exponent, significand, 0, 1, mode)
            .expect("normalized i64 significand is valid for binary64 rounding")
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

    /// Widens a raw binary32 encoding exactly, without using host floating point.
    ///
    /// Every finite value is represented exactly in binary64. A signaling NaN
    /// is quieted in the widened result and reports a local invalid-operation
    /// outcome; a quiet NaN preserves its sign and payload. The rounding
    /// diagnostics are always false because this conversion is exact. This is
    /// a value conversion only: it does not model denormal-input exception
    /// delivery or modify CPU/MXCSR state.
    pub fn widen_from_binary32(source: Binary32) -> BinaryArithmeticResult<Self> {
        let sign = (source.sign() as u64) << 63;
        let fraction = source.fraction() as u64;
        let (biased_exponent, widened_fraction, invalid_operation) = match source.class() {
            BinaryClass::Zero => (0, 0, false),
            BinaryClass::Subnormal => {
                let leading_bit = 31 - source.fraction().leading_zeros();
                let unbiased_exponent = leading_bit as i32 - 149;
                let exponent = (unbiased_exponent + 1023) as u64;
                let normalized_fraction = (fraction << (52 - leading_bit)) & Self::FRACTION_MASK;
                (exponent, normalized_fraction, false)
            }
            BinaryClass::Normal => (
                source.biased_exponent() as u64 + (1023 - 127),
                fraction << (52 - 23),
                false,
            ),
            BinaryClass::Infinity => (Self::EXPONENT_MASK as u64, 0, false),
            BinaryClass::QuietNaN => (Self::EXPONENT_MASK as u64, fraction << (52 - 23), false),
            BinaryClass::SignalingNaN => (
                Self::EXPONENT_MASK as u64,
                (fraction << (52 - 23)) | Self::QUIET_BIT,
                true,
            ),
        };
        BinaryArithmeticResult::exact(
            Self::from_bits(sign | (biased_exponent << 52) | widened_fraction),
            invalid_operation,
        )
    }

    /// Compares same-format raw binary64 operands without changing their bits.
    ///
    /// Either NaN produces `Unordered`. Quiet mode reports invalid only for a
    /// signaling NaN; signaling mode reports invalid for every NaN. This is a
    /// local comparison result and does not update flags or CPU state.
    pub fn compare_raw(lhs: Self, rhs: Self, mode: BinaryComparisonMode) -> BinaryComparisonResult {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        let lhs_nan = is_nan_class(lhs_class);
        let rhs_nan = is_nan_class(rhs_class);
        if lhs_nan || rhs_nan {
            let invalid = mode == BinaryComparisonMode::Signaling
                || lhs_class == BinaryClass::SignalingNaN
                || rhs_class == BinaryClass::SignalingNaN;
            return BinaryComparisonResult::new(BinaryComparisonRelation::Unordered, invalid);
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        if lhs_infinity && rhs_infinity {
            return BinaryComparisonResult::new(
                if lhs.sign() == rhs.sign() {
                    BinaryComparisonRelation::Equal
                } else if lhs.sign() {
                    BinaryComparisonRelation::Less
                } else {
                    BinaryComparisonRelation::Greater
                },
                false,
            );
        }
        if lhs_infinity {
            return BinaryComparisonResult::new(
                if lhs.sign() {
                    BinaryComparisonRelation::Less
                } else {
                    BinaryComparisonRelation::Greater
                },
                false,
            );
        }
        if rhs_infinity {
            return BinaryComparisonResult::new(
                if rhs.sign() {
                    BinaryComparisonRelation::Greater
                } else {
                    BinaryComparisonRelation::Less
                },
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary64 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary64 operand");
        BinaryComparisonResult::new(
            comparison_relation(compare_finite(lhs_finite, rhs_finite)),
            false,
        )
    }

    /// Selects the raw minimum of two binary64 values.
    ///
    /// NaN inputs select the right operand unchanged and report local invalid
    /// operation, including for quiet NaNs. Equal values select the right
    /// operand. Finite values, including subnormals, are compared by their raw
    /// numeric values without DAZ/FTZ or denormal-exception handling.
    pub fn min_sse_raw_result(lhs: Self, rhs: Self) -> BinarySelectionResult<Self> {
        Self::select_min_max_raw(lhs, rhs, true)
    }

    /// Selects the raw maximum of two binary64 values.
    ///
    /// NaN inputs select the right operand unchanged and report local invalid
    /// operation, including for quiet NaNs. Equal values select the right
    /// operand. Finite values, including subnormals, are compared by their raw
    /// numeric values without DAZ/FTZ or denormal-exception handling.
    pub fn max_sse_raw_result(lhs: Self, rhs: Self) -> BinarySelectionResult<Self> {
        Self::select_min_max_raw(lhs, rhs, false)
    }

    fn select_min_max_raw(lhs: Self, rhs: Self, minimum: bool) -> BinarySelectionResult<Self> {
        if is_nan_class(lhs.class()) || is_nan_class(rhs.class()) {
            return BinarySelectionResult::new(rhs, true);
        }
        let relation = Self::compare_raw(lhs, rhs, BinaryComparisonMode::Quiet).relation();
        let select_lhs = if minimum {
            relation == BinaryComparisonRelation::Less
        } else {
            relation == BinaryComparisonRelation::Greater
        };
        BinarySelectionResult::new(if select_lhs { lhs } else { rhs }, false)
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

    /// Computes a same-format raw binary64 square root with one rounding step.
    ///
    /// This pure value helper preserves signed zero and positive infinity.
    /// Negative nonzero values, including negative infinity, return the
    /// negative Intel indefinite QNaN and set the local invalid outcome. NaNs
    /// follow a local sign/payload-preserving policy: QNaN is returned as-is,
    /// while SNaN is quieted by setting its quiet bit; only SNaN sets invalid.
    /// Positive finite inputs use integer-only square root and one existing
    /// round-and-pack operation. The returned diagnostics are local and do not
    /// update MXCSR or deliver architectural exceptions.
    pub fn sqrt_raw_result(self, mode: RationalRoundingMode) -> BinaryArithmeticResult<Self> {
        match self.class() {
            BinaryClass::Zero => BinaryArithmeticResult::exact(self, false),
            BinaryClass::Infinity if !self.sign() => BinaryArithmeticResult::exact(self, false),
            BinaryClass::Infinity => {
                BinaryArithmeticResult::exact(Self::from_bits(0xfff8_0000_0000_0000), true)
            }
            BinaryClass::QuietNaN => BinaryArithmeticResult::exact(self, false),
            BinaryClass::SignalingNaN => {
                BinaryArithmeticResult::exact(Self::from_bits(self.0 | Self::QUIET_BIT), true)
            }
            BinaryClass::Subnormal | BinaryClass::Normal if self.sign() => {
                BinaryArithmeticResult::exact(Self::from_bits(0xfff8_0000_0000_0000), true)
            }
            BinaryClass::Subnormal | BinaryClass::Normal => {
                let finite = self.normalize_finite().expect("finite binary64 operand");
                let (exponent, quotient, has_tail) = sqrt_finite_parts(finite);
                let rounded = Self::round_pack_quotient(
                    false,
                    exponent,
                    quotient,
                    u64::from(has_tail),
                    if has_tail { 2 } else { 1 },
                    mode,
                )
                .expect("positive finite binary64 square root remains normal");
                BinaryArithmeticResult::new(rounded, false)
            }
        }
    }

    /// Multiplies same-format raw binary64 operands and returns result diagnostics.
    ///
    /// NaNs use the Intel SDM Rev. 093 first-source rule and signaling NaNs set
    /// the local invalid-operation outcome. Zero times infinity returns the
    /// Intel indefinite QNaN. Finite operands use the single-rounding finite
    /// result path; no architectural state or exception delivery is updated.
    pub fn multiply_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryArithmeticResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        if let Some(result) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return result;
        }

        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let lhs_zero = lhs_class == BinaryClass::Zero;
        let rhs_zero = rhs_class == BinaryClass::Zero;
        if (lhs_infinity && rhs_zero) || (lhs_zero && rhs_infinity) {
            return BinaryArithmeticResult::exact(Self::from_bits(0xfff8_0000_0000_0000), true);
        }

        let sign = lhs.sign() ^ rhs.sign();
        if lhs_infinity || rhs_infinity {
            return BinaryArithmeticResult::exact(
                Self::from_bits((Self::EXPONENT_MASK << 52) | (u64::from(sign) << 63)),
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary64 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary64 operand");
        BinaryArithmeticResult::new(
            Self::multiply_finite_result(lhs_finite, rhs_finite, mode),
            false,
        )
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

    /// Divides same-format raw binary64 operands and returns result diagnostics.
    ///
    /// NaNs are propagated before zero-divisor classification, so a NaN
    /// denominator never reports divide-by-zero. Only finite nonzero divided
    /// by zero sets the local divide-by-zero outcome. Finite pairs use the
    /// single-rounding finite result path; no architectural state or exception
    /// delivery is updated.
    pub fn divide_raw_result(
        lhs: Self,
        rhs: Self,
        mode: RationalRoundingMode,
    ) -> BinaryDivisionResult<Self> {
        let lhs_class = lhs.class();
        let rhs_class = rhs.class();
        if let Some(nan) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return BinaryDivisionResult::exact(*nan.value(), nan.invalid_operation(), false);
        }

        let sign = lhs.sign() ^ rhs.sign();
        let lhs_infinity = lhs_class == BinaryClass::Infinity;
        let rhs_infinity = rhs_class == BinaryClass::Infinity;
        let lhs_zero = lhs_class == BinaryClass::Zero;
        let rhs_zero = rhs_class == BinaryClass::Zero;

        if (lhs_zero && rhs_zero) || (lhs_infinity && rhs_infinity) {
            return BinaryDivisionResult::exact(
                Self::from_bits(0xfff8_0000_0000_0000),
                true,
                false,
            );
        }
        if rhs_zero {
            let infinity = Self::from_bits((Self::EXPONENT_MASK << 52) | (u64::from(sign) << 63));
            return BinaryDivisionResult::exact(infinity, false, !lhs_infinity);
        }
        if lhs_infinity {
            return BinaryDivisionResult::exact(
                Self::from_bits((Self::EXPONENT_MASK << 52) | (u64::from(sign) << 63)),
                false,
                false,
            );
        }
        if rhs_infinity {
            return BinaryDivisionResult::exact(
                Self::from_bits(u64::from(sign) << 63),
                false,
                false,
            );
        }

        let lhs_finite = lhs.normalize_finite().expect("finite binary64 operand");
        let rhs_finite = rhs.normalize_finite().expect("finite binary64 operand");
        let rounded = Self::divide_finite_result(lhs_finite, rhs_finite, mode)
            .expect("finite binary64 denominator is nonzero");
        BinaryDivisionResult::new(rounded, false, false)
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
        if let Some(result) = Self::propagate_nan(lhs, rhs, lhs_class, rhs_class) {
            return result;
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

    fn propagate_nan(
        lhs: Self,
        rhs: Self,
        lhs_class: BinaryClass,
        rhs_class: BinaryClass,
    ) -> Option<BinaryArithmeticResult<Self>> {
        let lhs_nan = matches!(lhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        let rhs_nan = matches!(rhs_class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN);
        if !lhs_nan && !rhs_nan {
            return None;
        }
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
        Some(BinaryArithmeticResult::exact(
            value,
            lhs_class == BinaryClass::SignalingNaN || rhs_class == BinaryClass::SignalingNaN,
        ))
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

fn is_nan_class(class: BinaryClass) -> bool {
    matches!(class, BinaryClass::QuietNaN | BinaryClass::SignalingNaN)
}

fn comparison_relation(ordering: core::cmp::Ordering) -> BinaryComparisonRelation {
    match ordering {
        core::cmp::Ordering::Less => BinaryComparisonRelation::Less,
        core::cmp::Ordering::Equal => BinaryComparisonRelation::Equal,
        core::cmp::Ordering::Greater => BinaryComparisonRelation::Greater,
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
        Binary32, Binary64, BinaryClass, BinaryComparisonMode, BinaryComparisonRelation,
        BinaryFinite, BinaryFiniteQuotient, BinaryRoundingResult, RationalRoundingMode,
        add_finite_aligned, compare_finite, divide_finite_significands, finite_exponent_gap,
        integer_sqrt_u128, multiply_finite_significands, round_quotient_to_precision,
    };

    use core::cmp::Ordering;

    fn assert_sse_reciprocal_relative_bound(source_bits: u32, result_bits: u32) {
        let source = Binary32::from_bits(source_bits);
        let result = Binary32::from_bits(result_bits);
        assert_eq!(source.class(), BinaryClass::Normal);
        assert_eq!(result.class(), BinaryClass::Normal);
        assert_eq!(source.sign(), result.sign(), "reciprocal sign mismatch");

        let source_significand = u64::from((1 << 23) | source.fraction());
        let result_significand = u64::from((1 << 23) | result.fraction());
        let product = source_significand * result_significand;
        let source_exponent = i32::from(source.biased_exponent()) - 127;
        let result_exponent = i32::from(result.biased_exponent()) - 127;
        let product_shift = source_exponent + result_exponent - 46;
        let target = match product_shift {
            -47 => 1_u64 << 47,
            -46 => 1_u64 << 46,
            shift => panic!("unexpected reciprocal product shift: {shift}"),
        };
        let difference = product.abs_diff(target);
        assert!(
            u128::from(difference) * 8192 <= u128::from(target) * 3,
            "relative error exceeds 3/8192: source={source_bits:#010x} result={result_bits:#010x}"
        );
    }

    fn assert_sse_reciprocal_sqrt_relative_bound(source_bits: u32, result_bits: u32) {
        let source = Binary32::from_bits(source_bits);
        let result = Binary32::from_bits(result_bits);
        assert_eq!(source.class(), BinaryClass::Normal);
        assert_eq!(result.class(), BinaryClass::Normal);
        assert!(!source.sign() && !result.sign());

        let source_significand = u128::from((1 << 23) | source.fraction());
        let result_significand = u128::from((1 << 23) | result.fraction());
        let source_exponent = i32::from(source.biased_exponent()) - 127;
        let result_exponent = i32::from(result.biased_exponent()) - 127;
        let scale = source_exponent + 2 * result_exponent;
        let denominator_shift = u32::try_from(69 - scale)
            .expect("normal reciprocal-square-root result has bounded positive scale");
        let product = source_significand * result_significand * result_significand;
        let scaled_product = product * 8192 * 8192;
        let lower = u128::from(8192_u32 - 3).pow(2) << denominator_shift;
        let upper = u128::from(8192_u32 + 3).pow(2) << denominator_shift;
        assert!(
            lower <= scaled_product && scaled_product <= upper,
            "relative error exceeds 3/8192: source={source_bits:#010x} result={result_bits:#010x}"
        );
    }

    #[test]
    fn raw_sse_reciprocal_binary32_handles_specials_and_boundaries() {
        let literals = [
            (0x0000_0000, 0x7f80_0000),
            (0x8000_0000, 0xff80_0000),
            (0x7f80_0000, 0x0000_0000),
            (0xff80_0000, 0x8000_0000),
            (0x3f80_0000, 0x3f80_0000),
            (0xbf80_0000, 0xbf80_0000),
            (0x4000_0000, 0x3f00_0000),
            (0xc000_0000, 0xbf00_0000),
            (0x3f00_0000, 0x4000_0000),
            (0x3e80_0000, 0x4080_0000),
            (0x4040_0000, 0x3eaa_aaab),
            (0xc040_0000, 0xbeaa_aaab),
            (0x0080_0000, 0x7e80_0000),
            (0x8080_0000, 0xfe80_0000),
            (0x7e7f_ffff, 0x0080_0001),
            (0x7e80_0000, 0x0080_0000),
            (0x7e80_0001, 0x0000_0000),
            (0xfe80_0001, 0x8000_0000),
            (0x7f7f_ffff, 0x0000_0000),
            (0xff7f_ffff, 0x8000_0000),
            (0x7fc1_2345, 0x7fc1_2345),
            (0xffe5_4321, 0xffe5_4321),
            (0x7f81_2345, 0x7fc1_2345),
            (0xff85_4321, 0xffc5_4321),
        ];
        for (source_bits, expected_bits) in literals {
            let source = Binary32::from_bits(source_bits);
            assert_eq!(
                source.reciprocal_sse_raw().to_bits(),
                expected_bits,
                "source={source_bits:#010x}"
            );
            assert_eq!(source.to_bits(), source_bits, "source was mutated");
        }

        for leading_bit in 0..23 {
            for sign in [0, 0x8000_0000] {
                let source_bits = sign | (1 << leading_bit);
                let expected = sign | 0x7f80_0000;
                assert_eq!(
                    Binary32::from_bits(source_bits)
                        .reciprocal_sse_raw()
                        .to_bits(),
                    expected,
                    "subnormal source={source_bits:#010x}"
                );
            }
        }
    }

    #[test]
    fn raw_sse_reciprocal_binary32_normal_results_meet_integer_error_bound() {
        let mut checked = 0;
        for exponent in 1..=254_u32 {
            for fraction in [
                0,
                1,
                0x0000_0001,
                0x0012_3456,
                0x003f_ffff,
                0x007f_fffe,
                0x007f_ffff,
            ] {
                for sign in [0, 0x8000_0000] {
                    let source_bits = sign | (exponent << 23) | fraction;
                    let result_bits = Binary32::from_bits(source_bits)
                        .reciprocal_sse_raw()
                        .to_bits();
                    let result = Binary32::from_bits(result_bits);
                    if result.class() == BinaryClass::Normal {
                        assert_sse_reciprocal_relative_bound(source_bits, result_bits);
                        checked += 1;
                    } else {
                        assert_eq!(
                            result.class(),
                            BinaryClass::Zero,
                            "finite normal input may only produce normal or flushed-zero result: {source_bits:#010x} -> {result_bits:#010x}"
                        );
                    }
                }
            }
        }
        assert!(checked > 1_000);
    }

    #[test]
    fn raw_sse_reciprocal_sqrt_binary32_handles_specials_and_boundaries() {
        let literals = [
            (0x0000_0000, 0x7f80_0000),
            (0x8000_0000, 0xff80_0000),
            (0x0000_0001, 0x7f80_0000),
            (0x8000_0001, 0xff80_0000),
            (0x7f80_0000, 0x0000_0000),
            (0xff80_0000, 0xffc0_0000),
            (0x3f80_0000, 0x3f80_0000),
            (0x4080_0000, 0x3f00_0000),
            (0x4180_0000, 0x3e80_0000),
            (0x3e80_0000, 0x4000_0000),
            (0x0080_0000, 0x5f00_0000),
            (0xbf80_0000, 0xffc0_0000),
            (0xc080_0000, 0xffc0_0000),
            (0x7fc1_2345, 0x7fc1_2345),
            (0xffe5_4321, 0xffe5_4321),
            (0x7f81_2345, 0x7fc1_2345),
            (0xff85_4321, 0xffc5_4321),
        ];
        for (source_bits, expected_bits) in literals {
            let source = Binary32::from_bits(source_bits);
            assert_eq!(
                source.reciprocal_sqrt_sse_raw().to_bits(),
                expected_bits,
                "source={source_bits:#010x}"
            );
            assert_eq!(source.to_bits(), source_bits, "source was mutated");
        }

        for leading_bit in 0..23 {
            for sign in [0, 0x8000_0000] {
                let source_bits = sign | (1 << leading_bit);
                assert_eq!(
                    Binary32::from_bits(source_bits)
                        .reciprocal_sqrt_sse_raw()
                        .to_bits(),
                    sign | 0x7f80_0000,
                    "subnormal source={source_bits:#010x}"
                );
            }
        }
    }

    #[test]
    fn raw_sse_reciprocal_sqrt_binary32_normal_results_meet_integer_error_bound() {
        let fractions = [0, 1, 0x0040_0000, 0x0012_3456, 0x003f_ffff, 0x007f_ffff];
        let mut checked = 0;
        for exponent in 1..=254_u32 {
            for fraction in fractions {
                let source_bits = (exponent << 23) | fraction;
                let source = Binary32::from_bits(source_bits);
                let result_bits = source.reciprocal_sqrt_sse_raw().to_bits();
                assert_sse_reciprocal_sqrt_relative_bound(source_bits, result_bits);
                assert_eq!(source.to_bits(), source_bits, "source was mutated");
                checked += 1;
            }
        }
        assert_eq!(checked, 254 * fractions.len());
    }

    #[test]
    fn integer_square_root_has_overflow_safe_floor_invariant() {
        assert_eq!(integer_sqrt_u128(0), 0);
        assert_eq!(integer_sqrt_u128(1), 1);
        assert_eq!(integer_sqrt_u128(2), 1);
        assert_eq!(integer_sqrt_u128(4), 2);
        let check = |value: u128| {
            let root = u128::from(integer_sqrt_u128(value));
            let square = root * root;
            assert!(square <= value);
            assert!(value - square < 2 * root + 1);
        };
        let root = 12_345_u128;
        let square = root * root;
        for value in [square - 1, square, square + 1, u128::MAX] {
            check(value);
        }
        assert_eq!(integer_sqrt_u128(u128::MAX), u64::MAX);
    }

    #[test]
    fn raw_sqrt_binary32_is_literal_pure_and_handles_boundaries() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, D, Z, U];
        let source = Binary32::from_bits(0x4000_0000);
        for (mode, expected) in [
            (N, 0x3fb5_04f3),
            (D, 0x3fb5_04f3),
            (Z, 0x3fb5_04f3),
            (U, 0x3fb5_04f4),
        ] {
            let result = source.sqrt_raw_result(mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }
        let half = Binary32::from_bits(0x3f00_0000);
        for (mode, expected) in [
            (N, 0x3f35_04f3),
            (D, 0x3f35_04f3),
            (Z, 0x3f35_04f3),
            (U, 0x3f35_04f4),
        ] {
            let result = half.sqrt_raw_result(mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
        }
        for (bits, expected) in [
            (0x0000_0000, 0x0000_0000),
            (0x8000_0000, 0x8000_0000),
            (0x3f80_0000, 0x3f80_0000),
            (0x4080_0000, 0x4000_0000),
            (0x7f80_0000, 0x7f80_0000),
        ] {
            let value = Binary32::from_bits(bits);
            for mode in modes {
                let result = value.sqrt_raw_result(mode);
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.inexact());
                assert!(!result.invalid_operation());
            }
            assert_eq!(value.to_bits(), bits);
        }
        for (bits, expected, invalid) in [
            (0xff80_0000, 0xffc0_0000, true),
            (0xbf80_0000, 0xffc0_0000, true),
            (0x8000_0001, 0xffc0_0000, true),
            (0x7fc1_2345, 0x7fc1_2345, false),
            (0xffc1_2345, 0xffc1_2345, false),
            (0x7f81_2345, 0x7fc1_2345, true),
            (0xff81_2345, 0xffc1_2345, true),
        ] {
            let value = Binary32::from_bits(bits);
            let result = value.sqrt_raw_result(N);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact());
            assert_eq!(value.to_bits(), bits);
        }
        for bits in [0x0000_0001, 0x007f_ffff, 0x0080_0000, 0x7f7f_ffff] {
            let value = Binary32::from_bits(bits);
            let result = value.sqrt_raw_result(N);
            assert_eq!(result.value().class(), BinaryClass::Normal);
            assert_eq!(result.inexact(), bits != 0x0080_0000);
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
            assert_eq!(value.to_bits(), bits);
        }
        assert_eq!(
            Binary32::from_bits(0x3f00_0000)
                .sqrt_raw_result(N)
                .value()
                .to_bits(),
            0x3f35_04f3,
        );
        let min_normal = Binary32::from_bits(0x0080_0000).sqrt_raw_result(N);
        assert_eq!(min_normal.value().to_bits(), 0x2000_0000);
        assert!(!min_normal.inexact());
    }

    #[test]
    fn raw_sqrt_binary64_is_literal_pure_and_handles_boundaries() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, D, Z, U];
        let source = Binary64::from_bits(0x4000_0000_0000_0000);
        for (mode, expected) in [
            (N, 0x3ff6_a09e_667f_3bcd),
            (D, 0x3ff6_a09e_667f_3bcc),
            (Z, 0x3ff6_a09e_667f_3bcc),
            (U, 0x3ff6_a09e_667f_3bcd),
        ] {
            let result = source.sqrt_raw_result(mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }
        let half = Binary64::from_bits(0x3fe0_0000_0000_0000);
        for (mode, expected) in [
            (N, 0x3fe6_a09e_667f_3bcd),
            (D, 0x3fe6_a09e_667f_3bcc),
            (Z, 0x3fe6_a09e_667f_3bcc),
            (U, 0x3fe6_a09e_667f_3bcd),
        ] {
            let result = half.sqrt_raw_result(mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
        }
        for (bits, expected) in [
            (0x0000_0000_0000_0000, 0x0000_0000_0000_0000),
            (0x8000_0000_0000_0000, 0x8000_0000_0000_0000),
            (0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000),
            (0x4010_0000_0000_0000, 0x4000_0000_0000_0000),
            (0x7ff0_0000_0000_0000, 0x7ff0_0000_0000_0000),
        ] {
            let value = Binary64::from_bits(bits);
            for mode in modes {
                let result = value.sqrt_raw_result(mode);
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.inexact());
                assert!(!result.invalid_operation());
            }
            assert_eq!(value.to_bits(), bits);
        }
        for (bits, expected, invalid) in [
            (0xfff0_0000_0000_0000, 0xfff8_0000_0000_0000, true),
            (0xbff0_0000_0000_0000, 0xfff8_0000_0000_0000, true),
            (0x8000_0000_0000_0001, 0xfff8_0000_0000_0000, true),
            (0x7ff8_1234_5678_9abc, 0x7ff8_1234_5678_9abc, false),
            (0xfff8_1234_5678_9abc, 0xfff8_1234_5678_9abc, false),
            (0x7ff0_1234_5678_9abc, 0x7ff8_1234_5678_9abc, true),
            (0xfff0_1234_5678_9abc, 0xfff8_1234_5678_9abc, true),
        ] {
            let value = Binary64::from_bits(bits);
            let result = value.sqrt_raw_result(N);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact());
            assert_eq!(value.to_bits(), bits);
        }
        for bits in [
            0x0000_0000_0000_0001,
            0x000f_ffff_ffff_ffff,
            0x0010_0000_0000_0000,
            0x7fef_ffff_ffff_ffff,
        ] {
            let value = Binary64::from_bits(bits);
            let result = value.sqrt_raw_result(N);
            assert_eq!(result.value().class(), BinaryClass::Normal);
            assert_eq!(
                result.inexact(),
                bits != 0x0000_0000_0000_0001 && bits != 0x0010_0000_0000_0000
            );
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
            assert_eq!(value.to_bits(), bits);
        }
        let min_normal = Binary64::from_bits(0x0010_0000_0000_0000).sqrt_raw_result(N);
        assert_eq!(min_normal.value().to_bits(), 0x2000_0000_0000_0000);
        assert!(!min_normal.inexact());
        let min_subnormal = Binary64::from_bits(1).sqrt_raw_result(N);
        assert_eq!(min_subnormal.value().to_bits(), 0x1e60_0000_0000_0000);
        assert!(!min_subnormal.inexact());
    }

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
            for (lhs_bits, rhs_bits) in [
                (lhs_bits, rhs_bits),
                (lhs_bits ^ 0x8000_0000, rhs_bits),
                (lhs_bits, rhs_bits ^ 0x8000_0000),
                (lhs_bits ^ 0x8000_0000, rhs_bits ^ 0x8000_0000),
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
        }

        for (lhs_bits, rhs_bits) in [
            (0x3ff0_0000_0000_0000_u64, 1_u64),
            (0x3ff0_0000_0000_0000, 0xbff0_0000_0000_0000),
            (0x7fef_ffff_ffff_ffff, 0x7fef_ffff_ffff_ffff),
            (0x0010_0000_0000_0000, 0x800f_ffff_ffff_ffff),
            (0x0000_0000_0000_0000, 0xbff0_0000_0000_0000),
        ] {
            for (lhs_bits, rhs_bits) in [
                (lhs_bits, rhs_bits),
                (lhs_bits ^ 0x8000_0000_0000_0000, rhs_bits),
                (lhs_bits, rhs_bits ^ 0x8000_0000_0000_0000),
                (
                    lhs_bits ^ 0x8000_0000_0000_0000,
                    rhs_bits ^ 0x8000_0000_0000_0000,
                ),
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

    #[test]
    fn raw_binary32_multiply_special_case_matrix_is_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let qnan = Binary32::from_bits(0x7fc1_2345);
        let neg_qnan = Binary32::from_bits(0xffc2_3456);
        let snan = Binary32::from_bits(0x7f81_2345);
        let quiet_snan = Binary32::from_bits(0x7fc1_2345);
        let neg_snan = Binary32::from_bits(0xff81_3456);
        let quiet_neg_snan = Binary32::from_bits(0xffc1_3456);
        let one = Binary32::from_bits(0x3f80_0000);
        for (lhs, rhs, expected, invalid) in [
            (qnan, snan, qnan, true),
            (snan, qnan, quiet_snan, true),
            (one, neg_qnan, neg_qnan, false),
            (neg_qnan, one, neg_qnan, false),
            (one, snan, quiet_snan, true),
            (snan, one, quiet_snan, true),
            (neg_snan, qnan, quiet_neg_snan, true),
            (qnan, neg_snan, qnan, true),
            (snan, neg_snan, quiet_snan, true),
            (neg_snan, snan, quiet_neg_snan, true),
        ] {
            let result = Binary32::multiply_raw_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }

        let p_inf = Binary32::from_bits(0x7f80_0000);
        let n_inf = Binary32::from_bits(0xff80_0000);
        let p_zero = Binary32::from_bits(0x0000_0000);
        let n_zero = Binary32::from_bits(0x8000_0000);
        let n_one = Binary32::from_bits(0xbf80_0000);
        for (lhs, rhs, expected, invalid) in [
            (p_zero, p_inf, 0xffc0_0000, true),
            (n_inf, p_zero, 0xffc0_0000, true),
            (n_zero, n_inf, 0xffc0_0000, true),
            (p_inf, n_zero, 0xffc0_0000, true),
            (p_inf, p_inf, 0x7f80_0000, false),
            (p_inf, n_inf, 0xff80_0000, false),
            (n_inf, n_inf, 0x7f80_0000, false),
            (p_inf, n_one, 0xff80_0000, false),
            (n_one, p_inf, 0xff80_0000, false),
            (n_inf, n_one, 0x7f80_0000, false),
        ] {
            let result = Binary32::multiply_raw_result(lhs, rhs, Nearest);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }
    }

    #[test]
    fn raw_binary64_multiply_special_case_matrix_is_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let qnan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let neg_qnan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let snan = Binary64::from_bits(0x7ff0_1234_5678_9abc);
        let quiet_snan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let neg_snan = Binary64::from_bits(0xfff0_2345_6789_abcd);
        let quiet_neg_snan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000);
        for (lhs, rhs, expected, invalid) in [
            (qnan, snan, qnan, true),
            (snan, qnan, quiet_snan, true),
            (one, neg_qnan, neg_qnan, false),
            (neg_qnan, one, neg_qnan, false),
            (one, snan, quiet_snan, true),
            (snan, one, quiet_snan, true),
            (neg_snan, qnan, quiet_neg_snan, true),
            (qnan, neg_snan, qnan, true),
            (snan, neg_snan, quiet_snan, true),
            (neg_snan, snan, quiet_neg_snan, true),
        ] {
            let result = Binary64::multiply_raw_result(lhs, rhs, Nearest);
            assert_eq!(*result.value(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }

        let p_inf = Binary64::from_bits(0x7ff0_0000_0000_0000);
        let n_inf = Binary64::from_bits(0xfff0_0000_0000_0000);
        let p_zero = Binary64::from_bits(0x0000_0000_0000_0000);
        let n_zero = Binary64::from_bits(0x8000_0000_0000_0000);
        let n_one = Binary64::from_bits(0xbff0_0000_0000_0000);
        for (lhs, rhs, expected, invalid) in [
            (p_zero, p_inf, 0xfff8_0000_0000_0000, true),
            (n_inf, p_zero, 0xfff8_0000_0000_0000, true),
            (n_zero, n_inf, 0xfff8_0000_0000_0000, true),
            (p_inf, n_zero, 0xfff8_0000_0000_0000, true),
            (p_inf, p_inf, 0x7ff0_0000_0000_0000, false),
            (p_inf, n_inf, 0xfff0_0000_0000_0000, false),
            (n_inf, n_inf, 0x7ff0_0000_0000_0000, false),
            (p_inf, n_one, 0xfff0_0000_0000_0000, false),
            (n_one, p_inf, 0xfff0_0000_0000_0000, false),
            (n_inf, n_one, 0x7ff0_0000_0000_0000, false),
        ] {
            let result = Binary64::multiply_raw_result(lhs, rhs, Nearest);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }
    }

    #[test]
    fn raw_multiply_finite_results_delegate_all_diagnostics_to_s2_36m() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        for lhs_bits in [0x0000_0000_u32, 0x8000_0000] {
            for rhs_bits in [0x0000_0000_u32, 0x8000_0000] {
                let lhs = Binary32::from_bits(lhs_bits);
                let rhs = Binary32::from_bits(rhs_bits);
                for mode in modes {
                    let result = Binary32::multiply_raw_result(lhs, rhs, mode);
                    assert_eq!(result.value().to_bits(), lhs_bits ^ rhs_bits);
                    assert!(!result.invalid_operation());
                    assert!(!result.inexact() && !result.overflow());
                    assert!(!result.tiny_after_precision_rounding());
                }
            }
        }
        for lhs_bits in [0x0000_0000_0000_0000_u64, 0x8000_0000_0000_0000] {
            for rhs_bits in [0x0000_0000_0000_0000_u64, 0x8000_0000_0000_0000] {
                let lhs = Binary64::from_bits(lhs_bits);
                let rhs = Binary64::from_bits(rhs_bits);
                for mode in modes {
                    let result = Binary64::multiply_raw_result(lhs, rhs, mode);
                    assert_eq!(result.value().to_bits(), lhs_bits ^ rhs_bits);
                    assert!(!result.invalid_operation());
                    assert!(!result.inexact() && !result.overflow());
                    assert!(!result.tiny_after_precision_rounding());
                }
            }
        }
        for (lhs_bits, rhs_bits) in [
            (0x3f80_0000_u32, 0x4000_0000_u32), // exact normal
            (0x3f80_0001, 0x3f80_0001),         // inexact normal
            (0x0080_0000, 0x3400_0000),         // exact subnormal
            (0x0000_0001, 0x3f00_0000),         // underflow boundary
            (0x7f7f_ffff, 0x4000_0000),         // overflow
            (0x8000_0000, 0x0000_0001),         // signed zero
        ] {
            let lhs_raw = Binary32::from_bits(lhs_bits);
            let rhs_raw = Binary32::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                for (raw, finite) in [
                    (
                        Binary32::multiply_raw_result(lhs_raw, rhs_raw, mode),
                        Binary32::multiply_finite_result(lhs, rhs, mode),
                    ),
                    (
                        Binary32::multiply_raw_result(rhs_raw, lhs_raw, mode),
                        Binary32::multiply_finite_result(rhs, lhs, mode),
                    ),
                ] {
                    assert_eq!(raw.value(), finite.value());
                    assert_eq!(raw.inexact(), finite.inexact());
                    assert_eq!(raw.overflow(), finite.overflow());
                    assert_eq!(
                        raw.tiny_after_precision_rounding(),
                        finite.tiny_after_precision_rounding()
                    );
                    assert!(!raw.invalid_operation());
                }
            }
        }

        for (lhs_bits, rhs_bits) in [
            (0x3ff0_0000_0000_0000_u64, 0x4000_0000_0000_0000), // exact normal
            (0x3ff0_0000_0000_0001, 0x3ff0_0000_0000_0001),     // inexact normal
            (0x0010_0000_0000_0000, 0x3cb0_0000_0000_0000),     // exact subnormal
            (0x0000_0000_0000_0001, 0x3fe0_0000_0000_0000),     // underflow boundary
            (0x7fef_ffff_ffff_ffff, 0x4000_0000_0000_0000),     // overflow
            (0x8000_0000_0000_0000, 0x0000_0000_0000_0001),     // signed zero
        ] {
            let lhs_raw = Binary64::from_bits(lhs_bits);
            let rhs_raw = Binary64::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                for (raw, finite) in [
                    (
                        Binary64::multiply_raw_result(lhs_raw, rhs_raw, mode),
                        Binary64::multiply_finite_result(lhs, rhs, mode),
                    ),
                    (
                        Binary64::multiply_raw_result(rhs_raw, lhs_raw, mode),
                        Binary64::multiply_finite_result(rhs, lhs, mode),
                    ),
                ] {
                    assert_eq!(raw.value(), finite.value());
                    assert_eq!(raw.inexact(), finite.inexact());
                    assert_eq!(raw.overflow(), finite.overflow());
                    assert_eq!(
                        raw.tiny_after_precision_rounding(),
                        finite.tiny_after_precision_rounding()
                    );
                    assert!(!raw.invalid_operation());
                }
            }
        }

        let exact32 = Binary32::multiply_raw_result(
            Binary32::from_bits(0x3f80_0000),
            Binary32::from_bits(0x4000_0000),
            Nearest,
        );
        assert_eq!(exact32.value().to_bits(), 0x4000_0000);
        assert!(!exact32.inexact() && !exact32.overflow());
        let inexact32 = Binary32::multiply_raw_result(
            Binary32::from_bits(0x3f80_0001),
            Binary32::from_bits(0x3f80_0001),
            Nearest,
        );
        assert_eq!(inexact32.value().to_bits(), 0x3f80_0002);
        assert!(inexact32.inexact());
        let subnormal32 = Binary32::multiply_raw_result(
            Binary32::from_bits(0x0080_0000),
            Binary32::from_bits(0x3400_0000),
            Nearest,
        );
        assert_eq!(subnormal32.value().to_bits(), 1);
        assert!(!subnormal32.inexact() && subnormal32.tiny_after_precision_rounding());
        let underflow32 = Binary32::multiply_raw_result(
            Binary32::from_bits(1),
            Binary32::from_bits(0x3f00_0000),
            Nearest,
        );
        assert_eq!(underflow32.value().to_bits(), 0);
        assert!(underflow32.inexact() && underflow32.tiny_after_precision_rounding());
        let overflow32 = Binary32::multiply_raw_result(
            Binary32::from_bits(0x7f7f_ffff),
            Binary32::from_bits(0x4000_0000),
            Nearest,
        );
        assert_eq!(overflow32.value().to_bits(), 0x7f80_0000);
        assert!(overflow32.inexact() && overflow32.overflow());

        let exact64 = Binary64::multiply_raw_result(
            Binary64::from_bits(0x3ff0_0000_0000_0000),
            Binary64::from_bits(0x4000_0000_0000_0000),
            Nearest,
        );
        assert_eq!(exact64.value().to_bits(), 0x4000_0000_0000_0000);
        assert!(!exact64.inexact() && !exact64.overflow());
        let inexact64 = Binary64::multiply_raw_result(
            Binary64::from_bits(0x3ff0_0000_0000_0001),
            Binary64::from_bits(0x3ff0_0000_0000_0001),
            Nearest,
        );
        assert_eq!(inexact64.value().to_bits(), 0x3ff0_0000_0000_0002);
        assert!(inexact64.inexact());
        let subnormal64 = Binary64::multiply_raw_result(
            Binary64::from_bits(0x0010_0000_0000_0000),
            Binary64::from_bits(0x3cb0_0000_0000_0000),
            Nearest,
        );
        assert_eq!(subnormal64.value().to_bits(), 1);
        assert!(!subnormal64.inexact() && subnormal64.tiny_after_precision_rounding());
        let underflow64 = Binary64::multiply_raw_result(
            Binary64::from_bits(1),
            Binary64::from_bits(0x3fe0_0000_0000_0000),
            Nearest,
        );
        assert_eq!(underflow64.value().to_bits(), 0);
        assert!(underflow64.inexact() && underflow64.tiny_after_precision_rounding());
        let overflow64 = Binary64::multiply_raw_result(
            Binary64::from_bits(0x7fef_ffff_ffff_ffff),
            Binary64::from_bits(0x4000_0000_0000_0000),
            Nearest,
        );
        assert_eq!(overflow64.value().to_bits(), 0x7ff0_0000_0000_0000);
        assert!(overflow64.inexact() && overflow64.overflow());
    }

    #[test]
    fn multiply_tininess_uses_precision_rounded_unbounded_exponent_for_binary32_and_64() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let cases = [
            (Nearest, 0x0010_0000_0000_0000, false),
            (Down, 0x000f_ffff_ffff_ffff, true),
            (Up, 0x0010_0000_0000_0000, false),
            (Zero, 0x000f_ffff_ffff_ffff, true),
        ];
        for (mode, expected, tiny) in cases {
            let result = Binary64::multiply_raw_result(
                Binary64::from_bits(0x0010_0000_0000_0001),
                Binary64::from_bits(0x3fef_ffff_ffff_fffe),
                mode,
            );
            assert_eq!(result.value().to_bits(), expected, "mode={mode:?}");
            assert!(result.inexact(), "mode={mode:?}");
            assert_eq!(
                result.tiny_after_precision_rounding(),
                tiny,
                "mode={mode:?}"
            );

            let result = Binary32::multiply_raw_result(
                Binary32::from_bits(0x0080_0001),
                Binary32::from_bits(0x3f7f_fffe),
                mode,
            );
            let expected32 = if tiny { 0x007f_ffff } else { 0x0080_0000 };
            assert_eq!(result.value().to_bits(), expected32, "mode={mode:?}");
            assert!(result.inexact(), "mode={mode:?}");
            assert_eq!(
                result.tiny_after_precision_rounding(),
                tiny,
                "mode={mode:?}"
            );
        }
    }

    #[test]
    fn raw_binary32_division_special_class_matrix_is_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let p_zero = Binary32::from_bits(0x0000_0000);
        let n_zero = Binary32::from_bits(0x8000_0000);
        let p_one = Binary32::from_bits(0x3f80_0000);
        let n_one = Binary32::from_bits(0xbf80_0000);
        let p_inf = Binary32::from_bits(0x7f80_0000);
        let n_inf = Binary32::from_bits(0xff80_0000);
        let qnan = Binary32::from_bits(0x7fc1_2345);
        let neg_qnan = Binary32::from_bits(0xffc2_3456);
        let snan = Binary32::from_bits(0x7f81_2345);
        let quiet_snan = Binary32::from_bits(0x7fc1_2345);
        let neg_snan = Binary32::from_bits(0xff81_3456);
        let quiet_neg_snan = Binary32::from_bits(0xffc1_3456);
        for (lhs, rhs, expected, invalid, divide_by_zero) in [
            (p_zero, p_zero, 0xffc0_0000, true, false),
            (n_zero, p_zero, 0xffc0_0000, true, false),
            (p_zero, n_zero, 0xffc0_0000, true, false),
            (n_zero, n_zero, 0xffc0_0000, true, false),
            (p_inf, p_inf, 0xffc0_0000, true, false),
            (p_inf, n_inf, 0xffc0_0000, true, false),
            (n_inf, p_inf, 0xffc0_0000, true, false),
            (n_inf, n_inf, 0xffc0_0000, true, false),
            (p_one, p_zero, 0x7f80_0000, false, true),
            (p_one, n_zero, 0xff80_0000, false, true),
            (n_one, p_zero, 0xff80_0000, false, true),
            (n_one, n_zero, 0x7f80_0000, false, true),
            (p_inf, p_zero, 0x7f80_0000, false, false),
            (p_inf, n_zero, 0xff80_0000, false, false),
            (n_inf, p_zero, 0xff80_0000, false, false),
            (n_inf, n_zero, 0x7f80_0000, false, false),
            (p_zero, n_one, 0x8000_0000, false, false),
            (n_zero, n_one, 0x0000_0000, false, false),
            (p_one, n_inf, 0x8000_0000, false, false),
            (n_one, n_inf, 0x0000_0000, false, false),
            (p_inf, n_one, 0xff80_0000, false, false),
            (n_inf, n_one, 0x7f80_0000, false, false),
            (qnan, p_zero, qnan.to_bits(), false, false),
            (p_zero, qnan, qnan.to_bits(), false, false),
            (snan, p_zero, quiet_snan.to_bits(), true, false),
            (p_zero, neg_snan, quiet_neg_snan.to_bits(), true, false),
            (qnan, snan, qnan.to_bits(), true, false),
            (neg_qnan, p_zero, neg_qnan.to_bits(), false, false),
        ] {
            let result = Binary32::divide_raw_result(lhs, rhs, Nearest);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert_eq!(result.divide_by_zero(), divide_by_zero);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }
    }

    #[test]
    fn raw_binary64_division_special_class_matrix_is_literal() {
        use RationalRoundingMode::NearestTiesEven as Nearest;

        let p_zero = Binary64::from_bits(0x0000_0000_0000_0000);
        let n_zero = Binary64::from_bits(0x8000_0000_0000_0000);
        let p_one = Binary64::from_bits(0x3ff0_0000_0000_0000);
        let n_one = Binary64::from_bits(0xbff0_0000_0000_0000);
        let p_inf = Binary64::from_bits(0x7ff0_0000_0000_0000);
        let n_inf = Binary64::from_bits(0xfff0_0000_0000_0000);
        let qnan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let neg_qnan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let snan = Binary64::from_bits(0x7ff0_1234_5678_9abc);
        let quiet_snan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let neg_snan = Binary64::from_bits(0xfff0_2345_6789_abcd);
        let quiet_neg_snan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        for (lhs, rhs, expected, invalid, divide_by_zero) in [
            (p_zero, p_zero, 0xfff8_0000_0000_0000, true, false),
            (n_zero, p_zero, 0xfff8_0000_0000_0000, true, false),
            (p_zero, n_zero, 0xfff8_0000_0000_0000, true, false),
            (n_zero, n_zero, 0xfff8_0000_0000_0000, true, false),
            (p_inf, p_inf, 0xfff8_0000_0000_0000, true, false),
            (p_inf, n_inf, 0xfff8_0000_0000_0000, true, false),
            (n_inf, p_inf, 0xfff8_0000_0000_0000, true, false),
            (n_inf, n_inf, 0xfff8_0000_0000_0000, true, false),
            (p_one, p_zero, 0x7ff0_0000_0000_0000, false, true),
            (p_one, n_zero, 0xfff0_0000_0000_0000, false, true),
            (n_one, p_zero, 0xfff0_0000_0000_0000, false, true),
            (n_one, n_zero, 0x7ff0_0000_0000_0000, false, true),
            (p_inf, p_zero, 0x7ff0_0000_0000_0000, false, false),
            (p_inf, n_zero, 0xfff0_0000_0000_0000, false, false),
            (n_inf, p_zero, 0xfff0_0000_0000_0000, false, false),
            (n_inf, n_zero, 0x7ff0_0000_0000_0000, false, false),
            (p_zero, n_one, 0x8000_0000_0000_0000, false, false),
            (n_zero, n_one, 0x0000_0000_0000_0000, false, false),
            (p_one, n_inf, 0x8000_0000_0000_0000, false, false),
            (n_one, n_inf, 0x0000_0000_0000_0000, false, false),
            (p_inf, n_one, 0xfff0_0000_0000_0000, false, false),
            (n_inf, n_one, 0x7ff0_0000_0000_0000, false, false),
            (qnan, p_zero, qnan.to_bits(), false, false),
            (p_zero, qnan, qnan.to_bits(), false, false),
            (snan, p_zero, quiet_snan.to_bits(), true, false),
            (p_zero, neg_snan, quiet_neg_snan.to_bits(), true, false),
            (qnan, snan, qnan.to_bits(), true, false),
            (neg_qnan, p_zero, neg_qnan.to_bits(), false, false),
        ] {
            let result = Binary64::divide_raw_result(lhs, rhs, Nearest);
            assert_eq!(result.value().to_bits(), expected);
            assert_eq!(result.invalid_operation(), invalid);
            assert_eq!(result.divide_by_zero(), divide_by_zero);
            assert!(!result.inexact() && !result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }
    }

    #[test]
    fn raw_division_finite_results_delegate_and_match_literal_rounding() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let modes = [Nearest, Zero, Up, Down];
        for (lhs_bits, rhs_bits, expected) in [
            (0x0000_0000_u32, 0x3f80_0000_u32, 0x0000_0000_u32),
            (0x8000_0000, 0x3f80_0000, 0x8000_0000),
            (0x0000_0000, 0xbf80_0000, 0x8000_0000),
            (0x8000_0000, 0xbf80_0000, 0x0000_0000),
        ] {
            for mode in modes {
                let result = Binary32::divide_raw_result(
                    Binary32::from_bits(lhs_bits),
                    Binary32::from_bits(rhs_bits),
                    mode,
                );
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation() && !result.divide_by_zero());
                assert!(!result.inexact() && !result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }
        }
        for (lhs_bits, rhs_bits, expected) in [
            (
                0x0000_0000_0000_0000_u64,
                0x3ff0_0000_0000_0000_u64,
                0x0000_0000_0000_0000_u64,
            ),
            (
                0x8000_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x8000_0000_0000_0000,
            ),
            (
                0x0000_0000_0000_0000,
                0xbff0_0000_0000_0000,
                0x8000_0000_0000_0000,
            ),
            (
                0x8000_0000_0000_0000,
                0xbff0_0000_0000_0000,
                0x0000_0000_0000_0000,
            ),
        ] {
            for mode in modes {
                let result = Binary64::divide_raw_result(
                    Binary64::from_bits(lhs_bits),
                    Binary64::from_bits(rhs_bits),
                    mode,
                );
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation() && !result.divide_by_zero());
                assert!(!result.inexact() && !result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }
        }
        for (lhs_bits, rhs_bits) in [
            (0x0000_0000_u32, 0x3f80_0000_u32),
            (0x8000_0000, 0xbf80_0000),
        ] {
            let lhs_raw = Binary32::from_bits(lhs_bits);
            let rhs_raw = Binary32::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                let raw = Binary32::divide_raw_result(lhs_raw, rhs_raw, mode);
                let finite = Binary32::divide_finite_result(lhs, rhs, mode).unwrap();
                assert_eq!(raw.value(), finite.value());
                assert_eq!(raw.inexact(), finite.inexact());
                assert_eq!(raw.overflow(), finite.overflow());
                assert_eq!(
                    raw.tiny_after_precision_rounding(),
                    finite.tiny_after_precision_rounding()
                );
                assert!(!raw.invalid_operation() && !raw.divide_by_zero());
            }
        }
        for (lhs_bits, rhs_bits) in [
            (0x0000_0000_0000_0000_u64, 0x3ff0_0000_0000_0000_u64),
            (0x8000_0000_0000_0000, 0xbff0_0000_0000_0000),
        ] {
            let lhs_raw = Binary64::from_bits(lhs_bits);
            let rhs_raw = Binary64::from_bits(rhs_bits);
            let lhs = lhs_raw.normalize_finite().unwrap();
            let rhs = rhs_raw.normalize_finite().unwrap();
            for mode in modes {
                let raw = Binary64::divide_raw_result(lhs_raw, rhs_raw, mode);
                let finite = Binary64::divide_finite_result(lhs, rhs, mode).unwrap();
                assert_eq!(raw.value(), finite.value());
                assert_eq!(raw.inexact(), finite.inexact());
                assert_eq!(raw.overflow(), finite.overflow());
                assert_eq!(
                    raw.tiny_after_precision_rounding(),
                    finite.tiny_after_precision_rounding()
                );
                assert!(!raw.invalid_operation() && !raw.divide_by_zero());
            }
        }
        for (lhs_bits, rhs_bits, expected, divide_by_zero) in [
            (0x3f80_0000_u32, 0x0000_0000_u32, 0x7f80_0000_u32, true),
            (0x3f80_0000, 0x8000_0000, 0xff80_0000, true),
            (0xbf80_0000, 0x0000_0000, 0xff80_0000, true),
            (0xbf80_0000, 0x8000_0000, 0x7f80_0000, true),
            (0x7f80_0000, 0x8000_0000, 0xff80_0000, false),
            (0xff80_0000, 0x0000_0000, 0xff80_0000, false),
        ] {
            for mode in modes {
                let result = Binary32::divide_raw_result(
                    Binary32::from_bits(lhs_bits),
                    Binary32::from_bits(rhs_bits),
                    mode,
                );
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation());
                assert_eq!(result.divide_by_zero(), divide_by_zero);
                assert!(!result.inexact() && !result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }
        }
        for (lhs_bits, rhs_bits, expected, divide_by_zero) in [
            (
                0x3ff0_0000_0000_0000_u64,
                0x0000_0000_0000_0000_u64,
                0x7ff0_0000_0000_0000_u64,
                true,
            ),
            (
                0x3ff0_0000_0000_0000,
                0x8000_0000_0000_0000,
                0xfff0_0000_0000_0000,
                true,
            ),
            (
                0xbff0_0000_0000_0000,
                0x0000_0000_0000_0000,
                0xfff0_0000_0000_0000,
                true,
            ),
            (
                0xbff0_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x7ff0_0000_0000_0000,
                true,
            ),
            (
                0x7ff0_0000_0000_0000,
                0x8000_0000_0000_0000,
                0xfff0_0000_0000_0000,
                false,
            ),
            (
                0xfff0_0000_0000_0000,
                0x0000_0000_0000_0000,
                0xfff0_0000_0000_0000,
                false,
            ),
        ] {
            for mode in modes {
                let result = Binary64::divide_raw_result(
                    Binary64::from_bits(lhs_bits),
                    Binary64::from_bits(rhs_bits),
                    mode,
                );
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation());
                assert_eq!(result.divide_by_zero(), divide_by_zero);
                assert!(!result.inexact() && !result.overflow());
                assert!(!result.tiny_after_precision_rounding());
            }
        }
        for (lhs_base_bits, rhs_base_bits) in [
            (0x3f80_0000_u32, 0x4000_0000_u32),
            (0x3f80_0000, 0x4040_0000),
            (0x0080_0000, 0x4000_0000),
            (0x0000_0001, 0x4000_0000),
            (0x7f7f_ffff, 0x0000_0001),
        ] {
            for (lhs_bits, rhs_bits) in [
                (lhs_base_bits, rhs_base_bits),
                (lhs_base_bits ^ 0x8000_0000, rhs_base_bits),
                (lhs_base_bits, rhs_base_bits ^ 0x8000_0000),
                (lhs_base_bits ^ 0x8000_0000, rhs_base_bits ^ 0x8000_0000),
            ] {
                let lhs_raw = Binary32::from_bits(lhs_bits);
                let rhs_raw = Binary32::from_bits(rhs_bits);
                let lhs = lhs_raw.normalize_finite().unwrap();
                let rhs = rhs_raw.normalize_finite().unwrap();
                for mode in modes {
                    for (raw_a, raw_b, finite_a, finite_b) in
                        [(lhs_raw, rhs_raw, lhs, rhs), (rhs_raw, lhs_raw, rhs, lhs)]
                    {
                        let result = Binary32::divide_raw_result(raw_a, raw_b, mode);
                        let finite =
                            Binary32::divide_finite_result(finite_a, finite_b, mode).unwrap();
                        assert_eq!(result.value(), finite.value());
                        assert_eq!(result.inexact(), finite.inexact());
                        assert_eq!(result.overflow(), finite.overflow());
                        assert_eq!(
                            result.tiny_after_precision_rounding(),
                            finite.tiny_after_precision_rounding()
                        );
                        assert!(!result.invalid_operation() && !result.divide_by_zero());
                    }
                }
            }
        }

        for (lhs_base_bits, rhs_base_bits) in [
            (0x3ff0_0000_0000_0000_u64, 0x4000_0000_0000_0000),
            (0x3ff0_0000_0000_0000, 0x4008_0000_0000_0000),
            (0x0010_0000_0000_0000, 0x4000_0000_0000_0000),
            (0x0000_0000_0000_0001, 0x4000_0000_0000_0000),
            (0x7fef_ffff_ffff_ffff, 0x0000_0000_0000_0001),
        ] {
            for (lhs_bits, rhs_bits) in [
                (lhs_base_bits, rhs_base_bits),
                (lhs_base_bits ^ 0x8000_0000_0000_0000, rhs_base_bits),
                (lhs_base_bits, rhs_base_bits ^ 0x8000_0000_0000_0000),
                (
                    lhs_base_bits ^ 0x8000_0000_0000_0000,
                    rhs_base_bits ^ 0x8000_0000_0000_0000,
                ),
            ] {
                let lhs_raw = Binary64::from_bits(lhs_bits);
                let rhs_raw = Binary64::from_bits(rhs_bits);
                let lhs = lhs_raw.normalize_finite().unwrap();
                let rhs = rhs_raw.normalize_finite().unwrap();
                for mode in modes {
                    for (raw_a, raw_b, finite_a, finite_b) in
                        [(lhs_raw, rhs_raw, lhs, rhs), (rhs_raw, lhs_raw, rhs, lhs)]
                    {
                        let result = Binary64::divide_raw_result(raw_a, raw_b, mode);
                        let finite =
                            Binary64::divide_finite_result(finite_a, finite_b, mode).unwrap();
                        assert_eq!(result.value(), finite.value());
                        assert_eq!(result.inexact(), finite.inexact());
                        assert_eq!(result.overflow(), finite.overflow());
                        assert_eq!(
                            result.tiny_after_precision_rounding(),
                            finite.tiny_after_precision_rounding()
                        );
                        assert!(!result.invalid_operation() && !result.divide_by_zero());
                    }
                }
            }
        }

        let exact32 = Binary32::divide_raw_result(
            Binary32::from_bits(0x3f80_0000),
            Binary32::from_bits(0x4000_0000),
            Nearest,
        );
        assert_eq!(exact32.value().to_bits(), 0x3f00_0000);
        assert!(!exact32.inexact());
        let inexact32 = Binary32::divide_raw_result(
            Binary32::from_bits(0x3f80_0000),
            Binary32::from_bits(0x4040_0000),
            Nearest,
        );
        assert_eq!(inexact32.value().to_bits(), 0x3eaa_aaab);
        assert!(inexact32.inexact());
        let subnormal32 = Binary32::divide_raw_result(
            Binary32::from_bits(0x0080_0000),
            Binary32::from_bits(0x4000_0000),
            Nearest,
        );
        assert_eq!(subnormal32.value().to_bits(), 0x0040_0000);
        assert!(subnormal32.tiny_after_precision_rounding());
        let underflow32 = Binary32::divide_raw_result(
            Binary32::from_bits(1),
            Binary32::from_bits(0x4000_0000),
            Nearest,
        );
        assert_eq!(underflow32.value().to_bits(), 0);
        assert!(underflow32.inexact() && underflow32.tiny_after_precision_rounding());
        let overflow32 = Binary32::divide_raw_result(
            Binary32::from_bits(0x7f7f_ffff),
            Binary32::from_bits(1),
            Nearest,
        );
        assert_eq!(overflow32.value().to_bits(), 0x7f80_0000);
        assert!(overflow32.inexact() && overflow32.overflow());

        let exact64 = Binary64::divide_raw_result(
            Binary64::from_bits(0x3ff0_0000_0000_0000),
            Binary64::from_bits(0x4000_0000_0000_0000),
            Nearest,
        );
        assert_eq!(exact64.value().to_bits(), 0x3fe0_0000_0000_0000);
        assert!(!exact64.inexact());
        let inexact64 = Binary64::divide_raw_result(
            Binary64::from_bits(0x3ff0_0000_0000_0000),
            Binary64::from_bits(0x4008_0000_0000_0000),
            Nearest,
        );
        assert_eq!(inexact64.value().to_bits(), 0x3fd5_5555_5555_5555);
        assert!(inexact64.inexact());
        let subnormal64 = Binary64::divide_raw_result(
            Binary64::from_bits(0x0010_0000_0000_0000),
            Binary64::from_bits(0x4000_0000_0000_0000),
            Nearest,
        );
        assert_eq!(subnormal64.value().to_bits(), 0x0008_0000_0000_0000);
        assert!(subnormal64.tiny_after_precision_rounding());
        let underflow64 = Binary64::divide_raw_result(
            Binary64::from_bits(1),
            Binary64::from_bits(0x4000_0000_0000_0000),
            Nearest,
        );
        assert_eq!(underflow64.value().to_bits(), 0);
        assert!(underflow64.inexact() && underflow64.tiny_after_precision_rounding());
        let overflow64 = Binary64::divide_raw_result(
            Binary64::from_bits(0x7fef_ffff_ffff_ffff),
            Binary64::from_bits(1),
            Nearest,
        );
        assert_eq!(overflow64.value().to_bits(), 0x7ff0_0000_0000_0000);
        assert!(overflow64.inexact() && overflow64.overflow());
    }

    #[test]
    fn raw_binary32_comparison_nan_modes_are_literal_and_nonmutating() {
        use BinaryComparisonMode::{Quiet, Signaling};
        use BinaryComparisonRelation::Unordered;

        let qnan = Binary32::from_bits(0x7fc1_2345);
        let neg_qnan = Binary32::from_bits(0xffc2_3456);
        let snan = Binary32::from_bits(0x7f81_2345);
        let neg_snan = Binary32::from_bits(0xff81_3456);
        let one = Binary32::from_bits(0x3f80_0000);
        for (lhs, rhs, quiet_invalid) in [
            (qnan, one, false),
            (one, qnan, false),
            (qnan, neg_qnan, false),
            (qnan, snan, true),
            (snan, qnan, true),
            (snan, neg_snan, true),
            (neg_snan, one, true),
            (one, neg_snan, true),
        ] {
            let lhs_before = lhs.to_bits();
            let rhs_before = rhs.to_bits();
            for (mode, invalid) in [(Quiet, quiet_invalid), (Signaling, true)] {
                let result = Binary32::compare_raw(lhs, rhs, mode);
                assert_eq!(result.relation(), Unordered);
                assert_eq!(result.invalid_operation(), invalid);
                assert_eq!(lhs.to_bits(), lhs_before);
                assert_eq!(rhs.to_bits(), rhs_before);
            }
        }
    }

    #[test]
    fn raw_binary64_comparison_nan_modes_are_literal_and_nonmutating() {
        use BinaryComparisonMode::{Quiet, Signaling};
        use BinaryComparisonRelation::Unordered;

        let qnan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let neg_qnan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let snan = Binary64::from_bits(0x7ff0_1234_5678_9abc);
        let neg_snan = Binary64::from_bits(0xfff0_2345_6789_abcd);
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000);
        for (lhs, rhs, quiet_invalid) in [
            (qnan, one, false),
            (one, qnan, false),
            (qnan, neg_qnan, false),
            (qnan, snan, true),
            (snan, qnan, true),
            (snan, neg_snan, true),
            (neg_snan, one, true),
            (one, neg_snan, true),
        ] {
            let lhs_before = lhs.to_bits();
            let rhs_before = rhs.to_bits();
            for (mode, invalid) in [(Quiet, quiet_invalid), (Signaling, true)] {
                let result = Binary64::compare_raw(lhs, rhs, mode);
                assert_eq!(result.relation(), Unordered);
                assert_eq!(result.invalid_operation(), invalid);
                assert_eq!(lhs.to_bits(), lhs_before);
                assert_eq!(rhs.to_bits(), rhs_before);
            }
        }
    }

    #[test]
    fn raw_binary32_comparison_orders_infinities_zeros_and_finite_edges() {
        use BinaryComparisonMode::{Quiet, Signaling};
        use BinaryComparisonRelation::{Equal, Greater, Less};

        let pairs = [
            (0xff80_0000_u32, 0xff80_0000_u32, Equal),
            (0xff80_0000, 0x7f80_0000, Less),
            (0x7f80_0000, 0xff80_0000, Greater),
            (0xff80_0000, 0xff7f_ffff, Less),
            (0x7f7f_ffff, 0x7f80_0000, Less),
            (0x8000_0000, 0x0000_0000, Equal),
            (0x3f80_0000, 0x3f80_0000, Equal),
            (0x8000_0001, 0x8000_0000, Less),
            (0x0000_0000, 0x0000_0001, Less),
            (0x007f_ffff, 0x0080_0000, Less),
            (0x3f80_0000, 0x3f80_0001, Less),
            (0x4000_0000, 0x4000_0001, Less),
            (0xbf80_0001, 0xbf80_0000, Less),
        ];
        for (lhs_bits, rhs_bits, expected) in pairs {
            let lhs = Binary32::from_bits(lhs_bits);
            let rhs = Binary32::from_bits(rhs_bits);
            for mode in [Quiet, Signaling] {
                let result = Binary32::compare_raw(lhs, rhs, mode);
                let reversed = Binary32::compare_raw(rhs, lhs, mode);
                assert_eq!(result.relation(), expected);
                assert_eq!(result.invalid_operation(), false);
                assert_eq!(
                    reversed.relation(),
                    match expected {
                        Less => Greater,
                        Equal => Equal,
                        Greater => Less,
                        BinaryComparisonRelation::Unordered => unreachable!(),
                    }
                );
            }
        }
    }

    #[test]
    fn raw_binary64_comparison_orders_infinities_zeros_and_finite_edges() {
        use BinaryComparisonMode::{Quiet, Signaling};
        use BinaryComparisonRelation::{Equal, Greater, Less};

        let pairs = [
            (0xfff0_0000_0000_0000_u64, 0xfff0_0000_0000_0000_u64, Equal),
            (0xfff0_0000_0000_0000, 0x7ff0_0000_0000_0000, Less),
            (0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000, Greater),
            (0xfff0_0000_0000_0000, 0xffef_ffff_ffff_ffff, Less),
            (0x7fef_ffff_ffff_ffff, 0x7ff0_0000_0000_0000, Less),
            (0x8000_0000_0000_0000, 0x0000_0000_0000_0000, Equal),
            (0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0000, Equal),
            (0x8000_0000_0000_0001, 0x8000_0000_0000_0000, Less),
            (0x0000_0000_0000_0000, 0x0000_0000_0000_0001, Less),
            (0x000f_ffff_ffff_ffff, 0x0010_0000_0000_0000, Less),
            (0x3ff0_0000_0000_0000, 0x3ff0_0000_0000_0001, Less),
            (0x4000_0000_0000_0000, 0x4000_0000_0000_0001, Less),
            (0xbff0_0000_0000_0001, 0xbff0_0000_0000_0000, Less),
        ];
        for (lhs_bits, rhs_bits, expected) in pairs {
            let lhs = Binary64::from_bits(lhs_bits);
            let rhs = Binary64::from_bits(rhs_bits);
            for mode in [Quiet, Signaling] {
                let result = Binary64::compare_raw(lhs, rhs, mode);
                let reversed = Binary64::compare_raw(rhs, lhs, mode);
                assert_eq!(result.relation(), expected);
                assert_eq!(result.invalid_operation(), false);
                assert_eq!(
                    reversed.relation(),
                    match expected {
                        Less => Greater,
                        Equal => Equal,
                        Greater => Less,
                        BinaryComparisonRelation::Unordered => unreachable!(),
                    }
                );
            }
        }
    }

    #[test]
    fn raw_binary32_min_max_selection_is_literal_and_nonmutating() {
        let operations = [Binary32::min_sse_raw_result, Binary32::max_sse_raw_result];
        for (lhs_bits, rhs_bits, min_bits, max_bits) in [
            (0x3f80_0000, 0x4000_0000, 0x3f80_0000, 0x4000_0000),
            (0x4000_0000, 0x3f80_0000, 0x3f80_0000, 0x4000_0000),
            (0xc000_0000, 0xbf80_0000, 0xc000_0000, 0xbf80_0000),
            (0xbf80_0000, 0xc000_0000, 0xc000_0000, 0xbf80_0000),
            (0x0000_0001, 0x0000_0002, 0x0000_0001, 0x0000_0002),
            (0x8000_0002, 0x8000_0001, 0x8000_0002, 0x8000_0001),
            (0xff80_0000, 0x7f7f_ffff, 0xff80_0000, 0x7f7f_ffff),
            (0x0000_0000, 0x8000_0000, 0x8000_0000, 0x8000_0000),
            (0x8000_0000, 0x0000_0000, 0x0000_0000, 0x0000_0000),
            (0x3f80_0000, 0x3f80_0000, 0x3f80_0000, 0x3f80_0000),
        ] {
            let lhs = Binary32::from_bits(lhs_bits);
            let rhs = Binary32::from_bits(rhs_bits);
            for (operation, expected) in operations.into_iter().zip([min_bits, max_bits]) {
                let result = operation(lhs, rhs);
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation());
                assert_eq!(lhs.to_bits(), lhs_bits);
                assert_eq!(rhs.to_bits(), rhs_bits);
            }
        }

        let qnan = Binary32::from_bits(0xffc2_3456);
        let other_qnan = Binary32::from_bits(0x7fc1_2345);
        let snan = Binary32::from_bits(0x7f81_4321);
        let one = Binary32::from_bits(0x3f80_0000);
        for (lhs, rhs) in [
            (qnan, one),
            (one, qnan),
            (qnan, other_qnan),
            (snan, one),
            (one, snan),
            (qnan, snan),
            (snan, qnan),
        ] {
            let lhs_bits = lhs.to_bits();
            let rhs_bits = rhs.to_bits();
            for operation in operations {
                let result = operation(lhs, rhs);
                assert_eq!(result.value().to_bits(), rhs_bits);
                assert!(result.invalid_operation());
                assert_eq!(lhs.to_bits(), lhs_bits);
                assert_eq!(rhs.to_bits(), rhs_bits);
            }
        }

        let lhs_nan = Binary32::min_sse_raw_result(qnan, one);
        let rhs_nan = Binary32::min_sse_raw_result(one, qnan);
        assert_eq!(lhs_nan.value().to_bits(), one.to_bits());
        assert_eq!(rhs_nan.value().to_bits(), qnan.to_bits());
        assert_ne!(lhs_nan.value().to_bits(), rhs_nan.value().to_bits());
    }

    #[test]
    fn raw_binary64_min_max_selection_is_literal_and_nonmutating() {
        let operations = [Binary64::min_sse_raw_result, Binary64::max_sse_raw_result];
        for (lhs_bits, rhs_bits, min_bits, max_bits) in [
            (
                0x3ff0_0000_0000_0000,
                0x4000_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x4000_0000_0000_0000,
            ),
            (
                0x4000_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x4000_0000_0000_0000,
            ),
            (
                0xc000_0000_0000_0000,
                0xbff0_0000_0000_0000,
                0xc000_0000_0000_0000,
                0xbff0_0000_0000_0000,
            ),
            (
                0xbff0_0000_0000_0000,
                0xc000_0000_0000_0000,
                0xc000_0000_0000_0000,
                0xbff0_0000_0000_0000,
            ),
            (0x0000_0000_0000_0001, 0x0000_0000_0000_0002, 1, 2),
            (
                0x8000_0000_0000_0002,
                0x8000_0000_0000_0001,
                0x8000_0000_0000_0002,
                0x8000_0000_0000_0001,
            ),
            (
                0xfff0_0000_0000_0000,
                0x7fef_ffff_ffff_ffff,
                0xfff0_0000_0000_0000,
                0x7fef_ffff_ffff_ffff,
            ),
            (
                0x0000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
            ),
            (
                0x8000_0000_0000_0000,
                0x0000_0000_0000_0000,
                0x0000_0000_0000_0000,
                0x0000_0000_0000_0000,
            ),
            (
                0x3ff0_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x3ff0_0000_0000_0000,
                0x3ff0_0000_0000_0000,
            ),
        ] {
            let lhs = Binary64::from_bits(lhs_bits);
            let rhs = Binary64::from_bits(rhs_bits);
            for (operation, expected) in operations.into_iter().zip([min_bits, max_bits]) {
                let result = operation(lhs, rhs);
                assert_eq!(result.value().to_bits(), expected);
                assert!(!result.invalid_operation());
                assert_eq!(lhs.to_bits(), lhs_bits);
                assert_eq!(rhs.to_bits(), rhs_bits);
            }
        }

        let qnan = Binary64::from_bits(0xfff8_2345_6789_abcd);
        let other_qnan = Binary64::from_bits(0x7ff8_1234_5678_9abc);
        let snan = Binary64::from_bits(0x7ff0_4567_89ab_cdef);
        let one = Binary64::from_bits(0x3ff0_0000_0000_0000);
        for (lhs, rhs) in [
            (qnan, one),
            (one, qnan),
            (qnan, other_qnan),
            (snan, one),
            (one, snan),
            (qnan, snan),
            (snan, qnan),
        ] {
            let lhs_bits = lhs.to_bits();
            let rhs_bits = rhs.to_bits();
            for operation in operations {
                let result = operation(lhs, rhs);
                assert_eq!(result.value().to_bits(), rhs_bits);
                assert!(result.invalid_operation());
                assert_eq!(lhs.to_bits(), lhs_bits);
                assert_eq!(rhs.to_bits(), rhs_bits);
            }
        }

        let lhs_nan = Binary64::max_sse_raw_result(qnan, one);
        let rhs_nan = Binary64::max_sse_raw_result(one, qnan);
        assert_eq!(lhs_nan.value().to_bits(), one.to_bits());
        assert_eq!(rhs_nan.value().to_bits(), qnan.to_bits());
        assert_ne!(lhs_nan.value().to_bits(), rhs_nan.value().to_bits());
    }

    #[test]
    fn binary32_to_binary64_widening_preserves_zero_and_finite_literals() {
        for (source_bits, expected_bits) in [
            (0x0000_0000, 0x0000_0000_0000_0000),
            (0x8000_0000, 0x8000_0000_0000_0000),
            (0x3f80_0000, 0x3ff0_0000_0000_0000),
            (0xbf80_0000, 0xbff0_0000_0000_0000),
            (0x3f80_0001, 0x3ff0_0000_2000_0000),
            (0xbf80_0001, 0xbff0_0000_2000_0000),
            (0x0080_0000, 0x3810_0000_0000_0000),
            (0x0080_0001, 0x3810_0000_2000_0000),
            (0x8080_0001, 0xb810_0000_2000_0000),
            (0x7f7f_ffff, 0x47ef_ffff_e000_0000),
            (0xff7f_ffff, 0xc7ef_ffff_e000_0000),
            (0x4049_0fdb, 0x4009_21fb_6000_0000),
        ] {
            let source = Binary32::from_bits(source_bits);
            let result = Binary64::widen_from_binary32(source);
            assert_eq!(
                result.value().to_bits(),
                expected_bits,
                "{source_bits:#010x}"
            );
            assert!(!result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
            assert_eq!(source.to_bits(), source_bits);
        }
    }

    #[test]
    fn binary32_subnormal_widening_maps_every_leading_bit_position() {
        for (source_bits, expected_bits) in [
            (0x0000_0001, 0x36a0_0000_0000_0000),
            (0x0000_0002, 0x36b0_0000_0000_0000),
            (0x0000_0004, 0x36c0_0000_0000_0000),
            (0x0000_0008, 0x36d0_0000_0000_0000),
            (0x0000_0010, 0x36e0_0000_0000_0000),
            (0x0000_0020, 0x36f0_0000_0000_0000),
            (0x0000_0040, 0x3700_0000_0000_0000),
            (0x0000_0080, 0x3710_0000_0000_0000),
            (0x0000_0100, 0x3720_0000_0000_0000),
            (0x0000_0200, 0x3730_0000_0000_0000),
            (0x0000_0400, 0x3740_0000_0000_0000),
            (0x0000_0800, 0x3750_0000_0000_0000),
            (0x0000_1000, 0x3760_0000_0000_0000),
            (0x0000_2000, 0x3770_0000_0000_0000),
            (0x0000_4000, 0x3780_0000_0000_0000),
            (0x0000_8000, 0x3790_0000_0000_0000),
            (0x0001_0000, 0x37a0_0000_0000_0000),
            (0x0002_0000, 0x37b0_0000_0000_0000),
            (0x0004_0000, 0x37c0_0000_0000_0000),
            (0x0008_0000, 0x37d0_0000_0000_0000),
            (0x0010_0000, 0x37e0_0000_0000_0000),
            (0x0020_0000, 0x37f0_0000_0000_0000),
            (0x0040_0000, 0x3800_0000_0000_0000),
            (0x8000_0001, 0xb6a0_0000_0000_0000),
            (0x807f_ffff, 0xb80f_ffff_c000_0000),
        ] {
            let source = Binary32::from_bits(source_bits);
            let source_before = source.to_bits();
            let result = Binary64::widen_from_binary32(source);
            assert_eq!(
                result.value().to_bits(),
                expected_bits,
                "{source_bits:#010x}"
            );
            assert!(!result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
            assert_eq!(source.to_bits(), source_before);
        }
    }

    #[test]
    fn binary32_to_binary64_widening_preserves_infinity_and_nan_rules() {
        for (source_bits, expected_bits, invalid) in [
            (0x7f80_0000, 0x7ff0_0000_0000_0000, false),
            (0xff80_0000, 0xfff0_0000_0000_0000, false),
            (0x7fc1_2345, 0x7ff8_2468_a000_0000, false),
            (0xffc6_5432, 0xfff8_ca86_4000_0000, false),
            (0x7f81_2345, 0x7ff8_2468_a000_0000, true),
            (0xff80_0001, 0xfff8_0000_2000_0000, true),
            (0x7fff_ffff, 0x7fff_ffff_e000_0000, false),
        ] {
            let source = Binary32::from_bits(source_bits);
            let source_before = source.to_bits();
            let result = Binary64::widen_from_binary32(source);
            assert_eq!(
                result.value().to_bits(),
                expected_bits,
                "{source_bits:#010x}"
            );
            assert_eq!(result.invalid_operation(), invalid);
            assert!(!result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert_eq!(source.to_bits(), source_before);
        }
    }

    #[test]
    fn binary64_to_binary32_narrowing_preserves_special_values_and_nan_payload_rules() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        for (source_bits, expected_bits) in [
            (0x0000_0000_0000_0000, 0x0000_0000),
            (0x8000_0000_0000_0000, 0x8000_0000),
            (0x7ff0_0000_0000_0000, 0x7f80_0000),
            (0xfff0_0000_0000_0000, 0xff80_0000),
            (0x7ff8_0000_0000_0001, 0x7fc0_0000),
            (0xfff8_0000_2000_0001, 0xffc0_0001),
            (0x7ff0_0000_0000_0001, 0x7fc0_0000),
            (0xfff0_0000_1fff_ffff, 0xffc0_0000),
            (0x7ff0_0000_2000_0000, 0x7fc0_0001),
        ] {
            for mode in [Nearest, Zero, Up, Down] {
                let source = Binary64::from_bits(source_bits);
                let result = Binary32::narrow_from_binary64(source, mode);
                assert_eq!(
                    result.value().to_bits(),
                    expected_bits,
                    "{source_bits:#018x}"
                );
                let signaling = source.class() == BinaryClass::SignalingNaN;
                assert_eq!(result.invalid_operation(), signaling, "{source_bits:#018x}");
                assert!(!result.inexact(), "{source_bits:#018x}");
                assert!(!result.overflow(), "{source_bits:#018x}");
                assert!(
                    !result.tiny_after_precision_rounding(),
                    "{source_bits:#018x}"
                );
                assert_eq!(source.to_bits(), source_bits);
            }
        }
    }

    #[test]
    fn binary64_to_binary32_narrowing_rounds_exact_finite_inputs_once() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        for (source_bits, expected_bits) in [
            (0x3ff0_0000_0000_0000, 0x3f80_0000),
            (0xbff0_0000_0000_0000, 0xbf80_0000),
            (0x3ff8_0000_0000_0000, 0x3fc0_0000),
            (0x380f_ffff_c000_0000, 0x007f_ffff),
            (0x3810_0000_0000_0000, 0x0080_0000),
            (0x36a0_0000_0000_0000, 0x0000_0001),
            (0x47ef_ffff_e000_0000, 0x7f7f_ffff),
        ] {
            let source = Binary64::from_bits(source_bits);
            let result = Binary32::narrow_from_binary64(source, Nearest);
            assert_eq!(
                result.value().to_bits(),
                expected_bits,
                "{source_bits:#018x}"
            );
            assert!(!result.inexact(), "{source_bits:#018x}");
            assert!(!result.overflow(), "{source_bits:#018x}");
            assert_eq!(
                result.tiny_after_precision_rounding(),
                expected_bits != 0x0080_0000 && expected_bits & 0x7f80_0000 == 0,
                "{source_bits:#018x}"
            );
            assert!(!result.invalid_operation());
        }

        let positive_tie = Binary64::from_bits(0x3ff0_0000_1000_0000);
        let negative_tie = Binary64::from_bits(0xbff0_0000_1000_0000);
        for (source, mode, expected) in [
            (positive_tie, Nearest, 0x3f80_0000),
            (positive_tie, Zero, 0x3f80_0000),
            (positive_tie, Up, 0x3f80_0001),
            (positive_tie, Down, 0x3f80_0000),
            (negative_tie, Nearest, 0xbf80_0000),
            (negative_tie, Zero, 0xbf80_0000),
            (negative_tie, Up, 0xbf80_0000),
            (negative_tie, Down, 0xbf80_0001),
            (
                Binary64::from_bits(0x3ff0_0000_1000_0001),
                Nearest,
                0x3f80_0001,
            ),
        ] {
            let result = Binary32::narrow_from_binary64(source, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }

        let positive_normal_boundary_tie = Binary64::from_bits(0x380f_ffff_e000_0000);
        let negative_normal_boundary_tie = Binary64::from_bits(0xb80f_ffff_e000_0000);
        for (source, mode, expected) in [
            (positive_normal_boundary_tie, Nearest, 0x0080_0000),
            (positive_normal_boundary_tie, Zero, 0x007f_ffff),
            (positive_normal_boundary_tie, Up, 0x0080_0000),
            (positive_normal_boundary_tie, Down, 0x007f_ffff),
            (negative_normal_boundary_tie, Nearest, 0x8080_0000),
            (negative_normal_boundary_tie, Zero, 0x807f_ffff),
            (negative_normal_boundary_tie, Up, 0x807f_ffff),
            (negative_normal_boundary_tie, Down, 0x8080_0000),
        ] {
            let result = Binary32::narrow_from_binary64(source, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }
    }

    #[test]
    fn binary64_to_binary32_narrowing_reports_tiny_and_overflow_diagnostics() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        let half_min_subnormal = Binary64::from_bits(0x3690_0000_0000_0000);
        for (mode, expected) in [
            (Nearest, 0x0000_0000),
            (Zero, 0x0000_0000),
            (Up, 0x0000_0001),
            (Down, 0x0000_0000),
        ] {
            let result = Binary32::narrow_from_binary64(half_min_subnormal, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(!result.overflow());
            assert!(result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }

        for (source_bits, expected_toward_positive, expected_toward_negative) in [
            (0x0000_0000_0000_0001, 0x0000_0001, 0x0000_0000),
            (0x000f_ffff_ffff_ffff, 0x0000_0001, 0x0000_0000),
            (0x8000_0000_0000_0001, 0x8000_0000, 0x8000_0001),
            (0x800f_ffff_ffff_ffff, 0x8000_0000, 0x8000_0001),
        ] {
            let source = Binary64::from_bits(source_bits);
            for (mode, expected) in [
                (Nearest, u32::from(source.sign()) << 31),
                (Zero, u32::from(source.sign()) << 31),
                (Up, expected_toward_positive),
                (Down, expected_toward_negative),
            ] {
                let result = Binary32::narrow_from_binary64(source, mode);
                assert_eq!(result.value().to_bits(), expected);
                assert!(result.inexact());
                assert!(!result.overflow());
                assert!(result.tiny_after_precision_rounding());
            }
        }

        let negative_half_min_subnormal = Binary64::from_bits(0xb690_0000_0000_0000);
        for (mode, expected) in [
            (Nearest, 0x8000_0000),
            (Zero, 0x8000_0000),
            (Up, 0x8000_0000),
            (Down, 0x8000_0001),
        ] {
            let result = Binary32::narrow_from_binary64(negative_half_min_subnormal, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(result.tiny_after_precision_rounding());
        }

        let positive_overflow = Binary64::from_bits(0x47f0_0000_0000_0000);
        for (mode, expected) in [
            (Nearest, 0x7f80_0000),
            (Zero, 0x7f7f_ffff),
            (Up, 0x7f80_0000),
            (Down, 0x7f7f_ffff),
        ] {
            let result = Binary32::narrow_from_binary64(positive_overflow, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(result.overflow());
            assert!(!result.tiny_after_precision_rounding());
            assert!(!result.invalid_operation());
        }

        let negative_overflow = Binary64::from_bits(0xc7f0_0000_0000_0000);
        for (mode, expected) in [
            (Nearest, 0xff80_0000),
            (Zero, 0xff7f_ffff),
            (Up, 0xff7f_ffff),
            (Down, 0xff80_0000),
        ] {
            let result = Binary32::narrow_from_binary64(negative_overflow, mode);
            assert_eq!(result.value().to_bits(), expected);
            assert!(result.inexact());
            assert!(result.overflow());
            assert!(!result.tiny_after_precision_rounding());
        }
    }

    #[test]
    fn signed_i64_to_binary_formats_covers_every_leading_bit_position() {
        for (value, expected32, expected64) in [
            (1_i64 << 0, 0x3f800000, 0x3ff0000000000000),
            (1_i64 << 1, 0x40000000, 0x4000000000000000),
            (1_i64 << 2, 0x40800000, 0x4010000000000000),
            (1_i64 << 3, 0x41000000, 0x4020000000000000),
            (1_i64 << 4, 0x41800000, 0x4030000000000000),
            (1_i64 << 5, 0x42000000, 0x4040000000000000),
            (1_i64 << 6, 0x42800000, 0x4050000000000000),
            (1_i64 << 7, 0x43000000, 0x4060000000000000),
            (1_i64 << 8, 0x43800000, 0x4070000000000000),
            (1_i64 << 9, 0x44000000, 0x4080000000000000),
            (1_i64 << 10, 0x44800000, 0x4090000000000000),
            (1_i64 << 11, 0x45000000, 0x40a0000000000000),
            (1_i64 << 12, 0x45800000, 0x40b0000000000000),
            (1_i64 << 13, 0x46000000, 0x40c0000000000000),
            (1_i64 << 14, 0x46800000, 0x40d0000000000000),
            (1_i64 << 15, 0x47000000, 0x40e0000000000000),
            (1_i64 << 16, 0x47800000, 0x40f0000000000000),
            (1_i64 << 17, 0x48000000, 0x4100000000000000),
            (1_i64 << 18, 0x48800000, 0x4110000000000000),
            (1_i64 << 19, 0x49000000, 0x4120000000000000),
            (1_i64 << 20, 0x49800000, 0x4130000000000000),
            (1_i64 << 21, 0x4a000000, 0x4140000000000000),
            (1_i64 << 22, 0x4a800000, 0x4150000000000000),
            (1_i64 << 23, 0x4b000000, 0x4160000000000000),
            (1_i64 << 24, 0x4b800000, 0x4170000000000000),
            (1_i64 << 25, 0x4c000000, 0x4180000000000000),
            (1_i64 << 26, 0x4c800000, 0x4190000000000000),
            (1_i64 << 27, 0x4d000000, 0x41a0000000000000),
            (1_i64 << 28, 0x4d800000, 0x41b0000000000000),
            (1_i64 << 29, 0x4e000000, 0x41c0000000000000),
            (1_i64 << 30, 0x4e800000, 0x41d0000000000000),
            (1_i64 << 31, 0x4f000000, 0x41e0000000000000),
            (1_i64 << 32, 0x4f800000, 0x41f0000000000000),
            (1_i64 << 33, 0x50000000, 0x4200000000000000),
            (1_i64 << 34, 0x50800000, 0x4210000000000000),
            (1_i64 << 35, 0x51000000, 0x4220000000000000),
            (1_i64 << 36, 0x51800000, 0x4230000000000000),
            (1_i64 << 37, 0x52000000, 0x4240000000000000),
            (1_i64 << 38, 0x52800000, 0x4250000000000000),
            (1_i64 << 39, 0x53000000, 0x4260000000000000),
            (1_i64 << 40, 0x53800000, 0x4270000000000000),
            (1_i64 << 41, 0x54000000, 0x4280000000000000),
            (1_i64 << 42, 0x54800000, 0x4290000000000000),
            (1_i64 << 43, 0x55000000, 0x42a0000000000000),
            (1_i64 << 44, 0x55800000, 0x42b0000000000000),
            (1_i64 << 45, 0x56000000, 0x42c0000000000000),
            (1_i64 << 46, 0x56800000, 0x42d0000000000000),
            (1_i64 << 47, 0x57000000, 0x42e0000000000000),
            (1_i64 << 48, 0x57800000, 0x42f0000000000000),
            (1_i64 << 49, 0x58000000, 0x4300000000000000),
            (1_i64 << 50, 0x58800000, 0x4310000000000000),
            (1_i64 << 51, 0x59000000, 0x4320000000000000),
            (1_i64 << 52, 0x59800000, 0x4330000000000000),
            (1_i64 << 53, 0x5a000000, 0x4340000000000000),
            (1_i64 << 54, 0x5a800000, 0x4350000000000000),
            (1_i64 << 55, 0x5b000000, 0x4360000000000000),
            (1_i64 << 56, 0x5b800000, 0x4370000000000000),
            (1_i64 << 57, 0x5c000000, 0x4380000000000000),
            (1_i64 << 58, 0x5c800000, 0x4390000000000000),
            (1_i64 << 59, 0x5d000000, 0x43a0000000000000),
            (1_i64 << 60, 0x5d800000, 0x43b0000000000000),
            (1_i64 << 61, 0x5e000000, 0x43c0000000000000),
            (1_i64 << 62, 0x5e800000, 0x43d0000000000000),
        ] {
            for (input, negative) in [(value, false), (-value, true)] {
                let expected32 = expected32 | if negative { 0x8000_0000 } else { 0 };
                let expected64 = expected64 | if negative { 0x8000_0000_0000_0000 } else { 0 };
                let result32 = Binary32::from_i64(input, RationalRoundingMode::NearestTiesEven);
                assert_eq!(result32.value().to_bits(), expected32, "{input}");
                assert_eq!(result32.value().class(), BinaryClass::Normal);
                assert!(!result32.inexact());
                assert!(!result32.overflow());
                assert!(!result32.tiny_after_precision_rounding());

                let result64 = Binary64::from_i64(input, RationalRoundingMode::NearestTiesEven);
                assert_eq!(result64.value().to_bits(), expected64, "{input}");
                assert_eq!(result64.value().class(), BinaryClass::Normal);
                assert!(!result64.inexact());
                assert!(!result64.overflow());
                assert!(!result64.tiny_after_precision_rounding());
            }
        }

        for mode in [
            RationalRoundingMode::NearestTiesEven,
            RationalRoundingMode::TowardZero,
            RationalRoundingMode::TowardPositive,
            RationalRoundingMode::TowardNegative,
        ] {
            let zero32 = Binary32::from_i64(0, mode);
            let zero64 = Binary64::from_i64(0, mode);
            assert_eq!(zero32.value().to_bits(), 0x0000_0000);
            assert_eq!(zero64.value().to_bits(), 0x0000_0000_0000_0000);
            assert!(
                !zero32.inexact() && !zero32.overflow() && !zero32.tiny_after_precision_rounding()
            );
            assert!(
                !zero64.inexact() && !zero64.overflow() && !zero64.tiny_after_precision_rounding()
            );
        }

        for (input, expected32, expected64) in [(i64::MIN, 0xdf00_0000, 0xc3e0_0000_0000_0000)] {
            let result32 = Binary32::from_i64(input, RationalRoundingMode::NearestTiesEven);
            let result64 = Binary64::from_i64(input, RationalRoundingMode::NearestTiesEven);
            assert_eq!(result32.value().to_bits(), expected32);
            assert_eq!(result64.value().to_bits(), expected64);
            assert!(!result32.inexact() && !result64.inexact());
        }
    }

    #[test]
    fn signed_i64_to_binary_formats_round_boundaries_and_ties_by_mode() {
        use RationalRoundingMode::{
            NearestTiesEven as Nearest, TowardNegative as Down, TowardPositive as Up,
            TowardZero as Zero,
        };

        for (input, expected32, expected64, inexact32, inexact64) in [
            (1, 0x3f80_0000, 0x3ff0_0000_0000_0000, false, false),
            (-1, 0xbf80_0000, 0xbff0_0000_0000_0000, false, false),
            (
                i32::MIN as i64,
                0xcf00_0000,
                0xc1e0_0000_0000_0000,
                false,
                false,
            ),
            (
                i32::MAX as i64,
                0x4f00_0000,
                0x41df_ffff_ffc0_0000,
                true,
                false,
            ),
            (i64::MIN, 0xdf00_0000, 0xc3e0_0000_0000_0000, false, false),
            (i64::MIN + 1, 0xdf00_0000, 0xc3e0_0000_0000_0000, true, true),
            (i64::MAX - 1, 0x5f00_0000, 0x43e0_0000_0000_0000, true, true),
            (i64::MAX, 0x5f00_0000, 0x43e0_0000_0000_0000, true, true),
            (16_777_215, 0x4b7f_ffff, 0x416f_ffff_e000_0000, false, false),
            (16_777_216, 0x4b80_0000, 0x4170_0000_0000_0000, false, false),
            (16_777_217, 0x4b80_0000, 0x4170_0000_1000_0000, true, false),
            (16_777_218, 0x4b80_0001, 0x4170_0000_2000_0000, false, false),
            (16_777_219, 0x4b80_0002, 0x4170_0000_3000_0000, true, false),
            (
                9_007_199_254_740_991,
                0x5a00_0000,
                0x433f_ffff_ffff_ffff,
                true,
                false,
            ),
            (
                9_007_199_254_740_992,
                0x5a00_0000,
                0x4340_0000_0000_0000,
                false,
                false,
            ),
            (
                9_007_199_254_740_993,
                0x5a00_0000,
                0x4340_0000_0000_0000,
                true,
                true,
            ),
            (
                9_007_199_254_740_994,
                0x5a00_0000,
                0x4340_0000_0000_0001,
                true,
                false,
            ),
            (
                9_007_199_254_740_995,
                0x5a00_0000,
                0x4340_0000_0000_0002,
                true,
                true,
            ),
            // Direct binary32 rounding goes above the midpoint; a binary64
            // intermediate would erase the final +1 and tie back down.
            (
                (1_i64 << 62) + (1_i64 << 38) + 1,
                0x5e80_0001,
                0x43d0_0000_1000_0000,
                true,
                true,
            ),
        ] {
            let result32 = Binary32::from_i64(input, Nearest);
            assert_eq!(result32.value().to_bits(), expected32, "{input}");
            assert_eq!(result32.inexact(), inexact32, "{input}");
            assert!(!result32.overflow());
            assert!(!result32.tiny_after_precision_rounding());

            let result64 = Binary64::from_i64(input, Nearest);
            assert_eq!(result64.value().to_bits(), expected64, "{input}");
            assert_eq!(result64.inexact(), inexact64, "{input}");
            assert!(!result64.overflow());
            assert!(!result64.tiny_after_precision_rounding());
        }

        for (input, expected32, expected64, inexact32, inexact64) in [
            (
                16_777_217,
                [0x4b80_0000, 0x4b80_0000, 0x4b80_0001, 0x4b80_0000],
                [0x4170_0000_1000_0000; 4],
                true,
                false,
            ),
            (
                -16_777_217,
                [0xcb80_0000, 0xcb80_0000, 0xcb80_0000, 0xcb80_0001],
                [0xc170_0000_1000_0000; 4],
                true,
                false,
            ),
            (
                9_007_199_254_740_993,
                [0x5a00_0000, 0x5a00_0000, 0x5a00_0001, 0x5a00_0000],
                [
                    0x4340_0000_0000_0000,
                    0x4340_0000_0000_0000,
                    0x4340_0000_0000_0001,
                    0x4340_0000_0000_0000,
                ],
                true,
                true,
            ),
            (
                -9_007_199_254_740_993,
                [0xda00_0000, 0xda00_0000, 0xda00_0000, 0xda00_0001],
                [
                    0xc340_0000_0000_0000,
                    0xc340_0000_0000_0000,
                    0xc340_0000_0000_0000,
                    0xc340_0000_0000_0001,
                ],
                true,
                true,
            ),
            (
                i64::MAX,
                [0x5f00_0000, 0x5eff_ffff, 0x5f00_0000, 0x5eff_ffff],
                [
                    0x43e0_0000_0000_0000,
                    0x43df_ffff_ffff_ffff,
                    0x43e0_0000_0000_0000,
                    0x43df_ffff_ffff_ffff,
                ],
                true,
                true,
            ),
            (
                -i64::MAX,
                [0xdf00_0000, 0xdeff_ffff, 0xdeff_ffff, 0xdf00_0000],
                [
                    0xc3e0_0000_0000_0000,
                    0xc3df_ffff_ffff_ffff,
                    0xc3df_ffff_ffff_ffff,
                    0xc3e0_0000_0000_0000,
                ],
                true,
                true,
            ),
        ] {
            for (index, mode) in [Nearest, Zero, Up, Down].into_iter().enumerate() {
                let result32 = Binary32::from_i64(input, mode);
                assert_eq!(
                    result32.value().to_bits(),
                    expected32[index],
                    "{input} {mode:?}"
                );
                assert_eq!(result32.inexact(), inexact32);
                assert!(!result32.overflow());
                assert!(!result32.tiny_after_precision_rounding());

                let result64 = Binary64::from_i64(input, mode);
                assert_eq!(
                    result64.value().to_bits(),
                    expected64[index],
                    "{input} {mode:?}"
                );
                assert_eq!(result64.inexact(), inexact64);
                assert!(!result64.overflow());
                assert!(!result64.tiny_after_precision_rounding());
            }
        }

        for mode in [Nearest, Zero, Up, Down] {
            for (input, expected32, expected64) in [(i64::MIN, 0xdf00_0000, 0xc3e0_0000_0000_0000)]
            {
                let result32 = Binary32::from_i64(input, mode);
                let result64 = Binary64::from_i64(input, mode);
                assert_eq!(result32.value().to_bits(), expected32);
                assert_eq!(result64.value().to_bits(), expected64);
                assert!(!result32.inexact());
                assert!(!result64.inexact());
                assert!(!result32.overflow() && !result32.tiny_after_precision_rounding());
                assert!(!result64.overflow() && !result64.tiny_after_precision_rounding());
            }
        }
    }

    #[test]
    fn raw_to_i32_binary32_fraction_rounding_matrix() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases = [
            (0x0000_0000, [0, 0, 0, 0], false),
            (0x8000_0000, [0, 0, 0, 0], false),
            (0x0000_0001, [0, 0, 1, 0], true),
            (0x8000_0001, [0, 0, 0, -1], true),
            (0x3eff_ffff, [0, 0, 1, 0], true),
            (0x3f00_0000, [0, 0, 1, 0], true),
            (0x3f00_0001, [1, 0, 1, 0], true),
            (0xbf00_0001, [-1, 0, 0, -1], true),
            (0x3fc0_0000, [2, 1, 2, 1], true),
            (0x4020_0000, [2, 2, 3, 2], true),
            (0xbfc0_0000, [-2, -1, -1, -2], true),
            (0xc020_0000, [-2, -2, -2, -3], true),
            (0x4040_0000, [3, 3, 3, 3], false),
            (0xc040_0000, [-3, -3, -3, -3], false),
        ];
        for (bits, expected, inexact) in cases {
            for (i, mode) in modes.into_iter().enumerate() {
                let source = Binary32::from_bits(bits);
                let result = source.to_i32(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (expected[i], inexact, false),
                    "{bits:#010x} {mode:?}"
                );
                assert_eq!(source.to_bits(), bits);
            }
        }
    }

    #[test]
    fn raw_to_i32_binary64_fraction_rounding_matrix() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases = [
            (0x0000_0000_0000_0000, [0, 0, 0, 0], false),
            (0x8000_0000_0000_0000, [0, 0, 0, 0], false),
            (0x3fdf_ffff_ffff_ffff, [0, 0, 1, 0], true),
            (0x3fe0_0000_0000_0000, [0, 0, 1, 0], true),
            (0x3fe0_0000_0000_0001, [1, 0, 1, 0], true),
            (0xbfdf_ffff_ffff_ffff, [0, 0, 0, -1], true),
            (0xbfe0_0000_0000_0000, [0, 0, 0, -1], true),
            (0xbfe0_0000_0000_0001, [-1, 0, 0, -1], true),
            (0x3ff8_0000_0000_0000, [2, 1, 2, 1], true),
            (0xbff8_0000_0000_0000, [-2, -1, -1, -2], true),
            (0x4004_0000_0000_0000, [2, 2, 3, 2], true),
            (0xc004_0000_0000_0000, [-2, -2, -2, -3], true),
        ];
        for (bits, expected, inexact) in cases {
            for (index, mode) in modes.into_iter().enumerate() {
                let source = Binary64::from_bits(bits);
                let result = source.to_i32(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (expected[index], inexact, false),
                    "{bits:#018x} {mode:?}"
                );
                assert_eq!(source.to_bits(), bits);
            }
        }
    }

    #[test]
    fn raw_to_i32_binary64_boundary_rounding_precedes_range_validation() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases = [
            (
                0x41df_ffff_ffd0_0000,
                [i32::MAX, i32::MAX, i32::MIN, i32::MAX],
                [true, true, false, true],
                [false, false, true, false],
            ),
            (
                0x41df_ffff_ffe0_0000,
                [i32::MIN, i32::MAX, i32::MIN, i32::MAX],
                [false, true, false, true],
                [true, false, true, false],
            ),
            (
                0x41df_ffff_fff0_0000,
                [i32::MIN, i32::MAX, i32::MIN, i32::MAX],
                [false, true, false, true],
                [true, false, true, false],
            ),
            (
                0xc1e0_0000_0008_0000,
                [i32::MIN; 4],
                [true, true, true, false],
                [false, false, false, true],
            ),
            (
                0xc1e0_0000_0010_0000,
                [i32::MIN; 4],
                [true, true, true, false],
                [false, false, false, true],
            ),
            (
                0xc1e0_0000_0018_0000,
                [i32::MIN; 4],
                [false, true, true, false],
                [true, false, false, true],
            ),
            (0x41e0_0000_0000_0000, [i32::MIN; 4], [false; 4], [true; 4]),
            (0xc1e0_0000_0000_0000, [i32::MIN; 4], [false; 4], [false; 4]),
            (0x41df_ffff_ffc0_0000, [i32::MAX; 4], [false; 4], [false; 4]),
        ];
        for (bits, values, inexact, invalid) in cases {
            for (i, mode) in modes.into_iter().enumerate() {
                let source = Binary64::from_bits(bits);
                let result = source.to_i32(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (values[i], inexact[i], invalid[i]),
                    "{bits:#018x} {mode:?}"
                );
                assert_eq!(source.to_bits(), bits);
            }
        }
    }

    #[test]
    fn raw_to_i32_binary32_edges_and_binary64_non_finite_values() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        for (bits, value, invalid) in [
            (0x4eff_ffff, 2_147_483_520, false),
            (0x4f00_0001, i32::MIN, true),
            (0xceff_ffff, -2_147_483_520, false),
            (0xcf00_0000, i32::MIN, false),
            (0x4f00_0000, i32::MIN, true),
            (0xcf00_0001, i32::MIN, true),
            (0x7f80_0000, i32::MIN, true),
            (0xff80_0000, i32::MIN, true),
            (0x7fc0_0001, i32::MIN, true),
            (0xffc0_0001, i32::MIN, true),
            (0x7f80_0001, i32::MIN, true),
            (0xff80_0001, i32::MIN, true),
            (0x7f7f_ffff, i32::MIN, true),
            (0xff7f_ffff, i32::MIN, true),
        ] {
            for mode in modes {
                let source = Binary32::from_bits(bits);
                let result = source.to_i32(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (value, false, invalid),
                    "{bits:#010x} {mode:?}"
                );
                assert_eq!(source.to_bits(), bits);
            }
        }
        for (bits, values, invalid) in [
            (0x0000_0000_0000_0001, [0, 0, 1, 0], false),
            (0x8000_0000_0000_0001, [0, 0, 0, -1], false),
            (0x7ff0_0000_0000_0000, [i32::MIN; 4], true),
            (0xfff0_0000_0000_0000, [i32::MIN; 4], true),
            (0x7ff8_0000_0000_0001, [i32::MIN; 4], true),
            (0xfff8_0000_0000_0001, [i32::MIN; 4], true),
            (0x7ff0_0000_0000_0001, [i32::MIN; 4], true),
            (0xfff0_0000_0000_0001, [i32::MIN; 4], true),
            (0x7fef_ffff_ffff_ffff, [i32::MIN; 4], true),
            (0xffef_ffff_ffff_ffff, [i32::MIN; 4], true),
        ] {
            for (i, mode) in modes.into_iter().enumerate() {
                let source = Binary64::from_bits(bits);
                let result = source.to_i32(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (values[i], !invalid, invalid),
                    "{bits:#018x} {mode:?}"
                );
                assert_eq!(source.to_bits(), bits);
            }
        }
    }

    #[test]
    fn raw_to_i64_fraction_rounding_matrix_for_both_source_widths() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases32 = [
            (0x0000_0000, [0, 0, 0, 0], false),
            (0x8000_0000, [0, 0, 0, 0], false),
            (0x0000_0001, [0, 0, 1, 0], true),
            (0x8000_0001, [0, 0, 0, -1], true),
            (0x3f00_0000, [0, 0, 1, 0], true),
            (0xbf00_0000, [0, 0, 0, -1], true),
            (0x3fc0_0000, [2, 1, 2, 1], true),
            (0xbfc0_0000, [-2, -1, -1, -2], true),
            (0x4020_0000, [2, 2, 3, 2], true),
            (0xc020_0000, [-2, -2, -2, -3], true),
            (0x4f00_0000, [2_147_483_648; 4], false),
        ];
        for (bits, values, inexact) in cases32 {
            let source = Binary32::from_bits(bits);
            for (index, mode) in modes.into_iter().enumerate() {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (values[index], inexact, false),
                    "{bits:#010x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }

        let cases64 = [
            (0x0000_0000_0000_0000, [0, 0, 0, 0], false),
            (0x8000_0000_0000_0000, [0, 0, 0, 0], false),
            (0x0000_0000_0000_0001, [0, 0, 1, 0], true),
            (0x8000_0000_0000_0001, [0, 0, 0, -1], true),
            (0x3fe0_0000_0000_0000, [0, 0, 1, 0], true),
            (0xbfe0_0000_0000_0000, [0, 0, 0, -1], true),
            (0x3ff8_0000_0000_0000, [2, 1, 2, 1], true),
            (0xbff8_0000_0000_0000, [-2, -1, -1, -2], true),
            (0x4004_0000_0000_0000, [2, 2, 3, 2], true),
            (0xc004_0000_0000_0000, [-2, -2, -2, -3], true),
        ];
        for (bits, values, inexact) in cases64 {
            let source = Binary64::from_bits(bits);
            for (index, mode) in modes.into_iter().enumerate() {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (values[index], inexact, false),
                    "{bits:#018x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }
    }

    #[test]
    fn raw_to_i64_limits_and_adjacent_source_encodings() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        let cases32 = [
            (0x5eff_ffff, 9_223_371_487_098_961_920, false),
            (0xdeff_ffff, -9_223_371_487_098_961_920, false),
            (0x5f00_0000, i64::MIN, true),
            (0xdf00_0000, i64::MIN, false),
            (0x5f00_0001, i64::MIN, true),
            (0xdf00_0001, i64::MIN, true),
        ];
        for (bits, value, invalid) in cases32 {
            let source = Binary32::from_bits(bits);
            for mode in modes {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (value, false, invalid),
                    "{bits:#010x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }

        let cases64 = [
            (0x43df_ffff_ffff_ffff, 9_223_372_036_854_774_784, false),
            (0xc3df_ffff_ffff_ffff, -9_223_372_036_854_774_784, false),
            (0x43e0_0000_0000_0000, i64::MIN, true),
            (0xc3e0_0000_0000_0000, i64::MIN, false),
            (0x43e0_0000_0000_0001, i64::MIN, true),
            (0xc3e0_0000_0000_0001, i64::MIN, true),
        ];
        for (bits, value, invalid) in cases64 {
            let source = Binary64::from_bits(bits);
            for mode in modes {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (value, false, invalid),
                    "{bits:#018x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }

        let powers32 = [
            (0x4eff_ffff, 2_147_483_520),
            (0x4f00_0000, 2_147_483_648),
            (0x5980_0000, 4_503_599_627_370_496),
            (0x5a00_0000, 9_007_199_254_740_992),
            (0x5e80_0000, 4_611_686_018_427_387_904),
        ];
        for (bits, value) in powers32 {
            for (positive_bits, signed_value) in [(bits, value), (bits | 0x8000_0000, -value)] {
                let source = Binary32::from_bits(positive_bits);
                for mode in modes {
                    let result = source.to_i64(mode);
                    assert_eq!(
                        (
                            *result.value(),
                            result.inexact(),
                            result.invalid_operation()
                        ),
                        (signed_value, false, false),
                        "{positive_bits:#010x} {mode:?}"
                    );
                }
                assert_eq!(source.to_bits(), positive_bits);
            }
        }

        let powers64 = [
            (0x41e0_0000_0000_0000, 2_147_483_648),
            (0x41f0_0000_0000_0000, 4_294_967_296),
            (0x4330_0000_0000_0000, 4_503_599_627_370_496),
            (0x4340_0000_0000_0000, 9_007_199_254_740_992),
            (0x43d0_0000_0000_0000, 4_611_686_018_427_387_904),
        ];
        for (bits, value) in powers64 {
            for (source_bits, signed_value) in
                [(bits, value), (bits | 0x8000_0000_0000_0000, -value)]
            {
                let source = Binary64::from_bits(source_bits);
                for mode in modes {
                    let result = source.to_i64(mode);
                    assert_eq!(
                        (
                            *result.value(),
                            result.inexact(),
                            result.invalid_operation()
                        ),
                        (signed_value, false, false),
                        "{source_bits:#018x} {mode:?}"
                    );
                }
                assert_eq!(source.to_bits(), source_bits);
            }
        }
    }

    #[test]
    fn raw_to_i64_invalid_non_finite_and_huge_values_suppress_inexact() {
        use RationalRoundingMode::{
            NearestTiesEven as N, TowardNegative as D, TowardPositive as U, TowardZero as Z,
        };
        let modes = [N, Z, U, D];
        for bits in [
            0x7f80_0000,
            0xff80_0000,
            0x7fc0_0001,
            0xffc0_0001,
            0x7f80_0001,
            0xff80_0001,
            0x5f80_0000,
            0xdf80_0000,
            0x7f7f_ffff,
        ] {
            let source = Binary32::from_bits(bits);
            for mode in modes {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (i64::MIN, false, true),
                    "{bits:#010x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }
        for bits in [
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
            0x7ff8_0000_0000_0001,
            0xfff8_0000_0000_0001,
            0x7ff0_0000_0000_0001,
            0xfff0_0000_0000_0001,
            0x43f0_0000_0000_0000,
            0xc3f0_0000_0000_0000,
            0x7fef_ffff_ffff_ffff,
            0xffef_ffff_ffff_ffff,
        ] {
            let source = Binary64::from_bits(bits);
            for mode in modes {
                let result = source.to_i64(mode);
                assert_eq!(
                    (
                        *result.value(),
                        result.inexact(),
                        result.invalid_operation()
                    ),
                    (i64::MIN, false, true),
                    "{bits:#018x} {mode:?}"
                );
            }
            assert_eq!(source.to_bits(), bits);
        }
    }
}
