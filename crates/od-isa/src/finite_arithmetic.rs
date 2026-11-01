//! Bounded exact arithmetic shared by normalized finite significands.

/// Return `floor(sqrt(value))` for the full `u128` domain.
///
/// This restoring digit-by-digit algorithm processes two input bits per step
/// and never forms a potentially overflowing square or trial product.
pub(crate) fn integer_sqrt_u128(value: u128) -> u64 {
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

/// Decompose the square root of a normalized finite binary operand.
///
/// For `x = significand * 2^(exponent-63)`, returns `(E, q, R)` where
/// `E = floor(exponent/2)`, `q = floor(sqrt(N))`, `R = N-q²`, and
/// `N = significand * 2^(63 + exponent.rem_euclid(2))`. The exact root is
/// `(q + δ) * 2^(E-63)` with `δ=0` iff `R=0`. This helper does not round or
/// pack a destination value.
pub(crate) fn sqrt_finite_parts(significand: u64, exponent: i32) -> Option<(i32, u64, u128)> {
    if significand < (1_u64 << 63) {
        return None;
    }
    let result_exponent = exponent.div_euclid(2);
    let parity = exponent.rem_euclid(2) as u32;
    let radicand = u128::from(significand) << (63 + parity);
    let root = integer_sqrt_u128(radicand);
    let square = u128::from(root) * u128::from(root);
    let remainder = radicand - square;
    debug_assert!(remainder < (u128::from(root) * 2 + 1));
    Some((result_exponent, root, remainder))
}

/// A nonzero tail classified at the p=64 rounding boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RoundingTail {
    BelowHalf,
    ExactHalf,
    AboveHalf,
}

impl RoundingTail {
    /// Return a compact rational representative with the same p=64 rounding
    /// classification. Lower precisions only need to know that this tail is
    /// nonzero, while p=64 distinguishes below, at, and above half.
    pub(crate) const fn representative(self) -> (u64, u64) {
        match self {
            Self::BelowHalf => (1, 4),
            Self::ExactHalf => (1, 2),
            Self::AboveHalf => (3, 4),
        }
    }
}

/// Add two normalized finite significands at their lower exponent without
/// rounding or discarding bits.
///
/// Each input represents `(-1)^sign * significand * 2^(exponent - 63)`.
/// Nonzero significands must be normalized (`2^63 <= significand`). Their
/// exact sum is returned as `(sign, min_exponent, magnitude)` in the same
/// exponent scale. Gaps through 64 are safe in `u128`; larger gaps are outside
/// this bounded helper. A zero input is neutral, while two zeros and exact
/// cancellation return the signless sentinel `(false, 0, 0)`.
pub(crate) fn add_finite_aligned(
    lhs_sign: bool,
    lhs_significand: u64,
    lhs_exponent: i32,
    rhs_sign: bool,
    rhs_significand: u64,
    rhs_exponent: i32,
) -> Option<(bool, i32, u128)> {
    if lhs_significand == 0 {
        return Some(if rhs_significand == 0 {
            (false, 0, 0)
        } else {
            if rhs_significand < (1_u64 << 63) {
                return None;
            }
            (rhs_sign, rhs_exponent, u128::from(rhs_significand))
        });
    }
    if rhs_significand == 0 {
        if lhs_significand < (1_u64 << 63) {
            return None;
        }
        return Some((lhs_sign, lhs_exponent, u128::from(lhs_significand)));
    }
    if lhs_significand < (1_u64 << 63) || rhs_significand < (1_u64 << 63) {
        return None;
    }

    let exponent = lhs_exponent.min(rhs_exponent);
    let lhs_gap = u32::try_from(i64::from(lhs_exponent) - i64::from(exponent)).ok()?;
    let rhs_gap = u32::try_from(i64::from(rhs_exponent) - i64::from(exponent)).ok()?;
    if lhs_gap > 64 || rhs_gap > 64 {
        return None;
    }

    let lhs_magnitude = u128::from(lhs_significand) << lhs_gap;
    let rhs_magnitude = u128::from(rhs_significand) << rhs_gap;
    if lhs_sign == rhs_sign {
        // The worst case is gap 64 with both significands at `u64::MAX`:
        // `(2^64 - 1) * 2^64 + (2^64 - 1) == u128::MAX`.
        Some((lhs_sign, exponent, lhs_magnitude + rhs_magnitude))
    } else if lhs_magnitude == rhs_magnitude {
        Some((false, 0, 0))
    } else if lhs_magnitude > rhs_magnitude {
        Some((lhs_sign, exponent, lhs_magnitude - rhs_magnitude))
    } else {
        Some((rhs_sign, exponent, rhs_magnitude - lhs_magnitude))
    }
}

/// Describe the rounding-equivalent normalized quotient for a sum whose
/// nonzero finite operands have an exponent gap greater than 64.
///
/// The result `(sign, exponent, quotient, tail)` represents the exact result's
/// retained 64-bit quotient and the tail's relation to one half. The tail
/// itself is compressed because p=64 rounding observes only that relation;
/// all returned tails are nonzero. Gaps through 64 must use exact alignment.
pub(crate) fn finite_large_gap_rounding_bridge(
    lhs_sign: bool,
    lhs_significand: u64,
    lhs_exponent: i32,
    rhs_sign: bool,
    rhs_significand: u64,
    rhs_exponent: i32,
) -> Option<(bool, i32, u64, RoundingTail)> {
    if lhs_significand == 0
        || rhs_significand == 0
        || lhs_significand < (1_u64 << 63)
        || rhs_significand < (1_u64 << 63)
    {
        return None;
    }

    let (dominant, small) = if lhs_exponent > rhs_exponent {
        (
            (lhs_sign, lhs_significand, lhs_exponent),
            (rhs_sign, rhs_significand, rhs_exponent),
        )
    } else if rhs_exponent > lhs_exponent {
        (
            (rhs_sign, rhs_significand, rhs_exponent),
            (lhs_sign, lhs_significand, lhs_exponent),
        )
    } else {
        return None;
    };
    let (dominant_sign, dominant_significand, dominant_exponent) = dominant;
    let (small_sign, small_significand, small_exponent) = small;
    let gap = u32::try_from(i64::from(dominant_exponent) - i64::from(small_exponent)).ok()?;
    if gap <= 64 {
        return None;
    }

    if dominant_sign == small_sign {
        // T = b / 2^gap, strictly less than one half for gap > 64.
        return Some((
            dominant_sign,
            dominant_exponent,
            dominant_significand,
            RoundingTail::BelowHalf,
        ));
    }

    if dominant_significand > (1_u64 << 63) {
        // T = 1 - b / 2^gap, strictly greater than one half.
        return Some((
            dominant_sign,
            dominant_exponent,
            dominant_significand - 1,
            RoundingTail::AboveHalf,
        ));
    }

    let exponent = dominant_exponent.checked_sub(1)?;
    let tail = if gap == 65 {
        if small_significand == 1_u64 << 63 {
            RoundingTail::ExactHalf
        } else {
            // T = 1 - b / 2^64, below one half once b exceeds 2^63.
            RoundingTail::BelowHalf
        }
    } else {
        // For gap >= 66, b / 2^(gap-1) < one half, so its complement is
        // strictly greater than one half.
        RoundingTail::AboveHalf
    };
    Some((dominant_sign, exponent, u64::MAX, tail))
}

#[cfg(test)]
mod tests {
    use super::{
        RoundingTail, add_finite_aligned, finite_large_gap_rounding_bridge, integer_sqrt_u128,
        sqrt_finite_parts,
    };

    #[test]
    fn integer_sqrt_has_overflow_safe_floor_invariant_for_deterministic_samples() {
        let check = |value: u128| {
            let root = u128::from(integer_sqrt_u128(value));
            let square = root * root;
            assert!(square <= value);
            assert!(value - square < 2 * root + 1);
        };
        for value in [0, 1, 2, 4, u128::MAX] {
            check(value);
        }
        let root = 12_345_u128;
        let square = root * root;
        for value in [square - 1, square, square + 1] {
            check(value);
        }

        let mut state = 0x6a09_e667_f3bc_c909_bb67_ae85_84ca_a73b_u128;
        for _ in 0..2048 {
            state ^= state << 23;
            state ^= state >> 17;
            state ^= state << 26;
            check(state);
        }
        assert_eq!(integer_sqrt_u128(u128::MAX), u64::MAX);
    }

    #[test]
    fn finite_sqrt_decomposition_preserves_exact_u128_remainders() {
        let cases = [
            (0x8000_0000_0000_0000, 0, 0, 0x8000_0000_0000_0000, 0),
            (0x9000_0000_0000_0000, 3, 1, 0xc000_0000_0000_0000, 0),
            (
                0x8000_0000_0000_0000,
                1,
                0,
                0xb504_f333_f9de_6484,
                0x7e8e_faac_bb98_9bf0,
            ),
            (
                0x8000_0000_0000_0001,
                0,
                0,
                0x8000_0000_0000_0000,
                0x8000_0000_0000_0000,
            ),
            (
                u64::MAX,
                0,
                0,
                0xb504_f333_f9de_6483,
                0x1689_8e11_4af5_564f7,
            ),
            (u64::MAX, 1, 0, u64::MAX, u128::from(u64::MAX)),
        ];
        for (significand, exponent, expected_exponent, expected_root, expected_remainder) in cases {
            assert_eq!(
                sqrt_finite_parts(significand, exponent),
                Some((expected_exponent, expected_root, expected_remainder))
            );
        }
        assert_eq!(sqrt_finite_parts(0, 0), None);
        assert_eq!(sqrt_finite_parts(1_u64 << 62, 0), None);
    }

    #[test]
    fn exact_aligned_addition_covers_gap_64_full_u128_and_sign_order() {
        let sig = u64::MAX;
        let maximum = u128::MAX;
        for (lhs_sign, rhs_sign) in [(false, false), (true, true)] {
            for (lhs_exp, rhs_exp) in [(64, 0), (0, 64)] {
                assert_eq!(
                    add_finite_aligned(lhs_sign, sig, lhs_exp, rhs_sign, sig, rhs_exp),
                    Some((lhs_sign, 0, maximum))
                );
            }
        }
        assert_eq!(
            add_finite_aligned(false, 1 << 63, 0, true, 1 << 63, 0),
            Some((false, 0, 0))
        );
        assert_eq!(
            add_finite_aligned(false, 1 << 63, 65, false, 1 << 63, 0),
            None
        );
        assert_eq!(
            add_finite_aligned(false, 1 << 62, 0, false, 1 << 63, 0),
            None
        );
    }

    #[test]
    fn large_gap_bridge_classifies_binary80_half_tails() {
        let sig = 1_u64 << 63;
        let bridge = |dominant_sig: u64, small_sig: u64, gap: i32, same_sign: bool| {
            finite_large_gap_rounding_bridge(false, dominant_sig, 0, !same_sign, small_sig, -gap)
        };

        assert_eq!(
            bridge(sig, sig, 65, true),
            Some((false, 0, sig, RoundingTail::BelowHalf))
        );
        assert_eq!(
            bridge(sig, sig, 65, false),
            Some((false, -1, u64::MAX, RoundingTail::ExactHalf))
        );
        assert_eq!(
            bridge(sig, sig + 1, 65, false),
            Some((false, -1, u64::MAX, RoundingTail::BelowHalf))
        );
        assert_eq!(
            bridge(sig, sig, 66, false),
            Some((false, -1, u64::MAX, RoundingTail::AboveHalf))
        );
        assert_eq!(
            bridge(sig + 1, sig, 65, false),
            Some((false, 0, sig, RoundingTail::AboveHalf))
        );
        assert_eq!(bridge(sig, sig, 64, false), None);
    }
}
