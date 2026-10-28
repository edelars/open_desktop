//! Shared exact rounding for rational significands.

/// Mathematical rounding direction for an exact rational coefficient.
///
/// These modes do not represent any instruction state or architectural control
/// register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RationalRoundingMode {
    NearestTiesEven,
    TowardZero,
    TowardPositive,
    TowardNegative,
}

/// Rounds a normalized rational quotient to the requested precision.
///
/// Returns the rounded unsigned magnitude and whether any discarded or rational
/// tail was inexact. A carry to `2^precision` is preserved as magnitude `2^64`
/// when precision is 64.
pub(crate) fn round_quotient_to_precision(
    sign: bool,
    q: u64,
    r: u64,
    d: u64,
    precision: u32,
    mode: RationalRoundingMode,
) -> Option<(u128, bool)> {
    if !(1..=64).contains(&precision) || d == 0 || r >= d {
        return None;
    }
    if (q == 0 && r != 0) || (q != 0 && q < (1_u64 << 63)) {
        return None;
    }

    if q == 0 {
        return Some((0, false));
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

    Some((u128::from(t) + u128::from(increment), inexact))
}
