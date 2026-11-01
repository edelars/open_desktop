//! Bounded exact arithmetic shared by normalized finite significands.

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

#[cfg(test)]
mod tests {
    use super::add_finite_aligned;

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
}
