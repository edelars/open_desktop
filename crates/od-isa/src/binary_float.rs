//! Lossless binary32 and binary64 fields and exact finite-value normalization.
//!
//! These types inspect encoded bits only. They do not perform guest floating-point
//! operations, rounding, or exception handling. Equality on [`Binary32`] and
//! [`Binary64`] compares raw encodings, including NaN sign and payload; it does
//! not model guest floating-point comparison.

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

#[cfg(test)]
mod tests {
    use super::{Binary32, Binary64, BinaryClass, BinaryFinite, multiply_finite_significands};

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
}
