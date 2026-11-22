//! Modelled x87 transcendental and scaling helpers (S2-32): FSIN, FCOS, FSINCOS, FPTAN, F2XM1,
//! FYL2X, FYL2XP1, FPATAN, FSCALE and FXTRACT on canonical binary80 operands.
//!
//! Every function takes raw 10-byte binary80 operands (memory order) and answers [`None`]
//! outside the bounded domain it models (non-canonical, denormal, NaN, infinite or out-of-domain
//! operands, results outside the normal range), so the caller can refuse the instruction instead of
//! guessing. Values are evaluated in a 256-bit software float ([`Big`]) and then rounded once to the 64-bit
//! significand under the requested rounding mode, which is the architectural model: the x87
//! transcendental microcode ignores the precision control (PC), honours the rounding control (RC)
//! and reports the round-up indicator in C1 and inexactness in PE.
//!
//! Accuracy. The mathematical function is evaluated to about 2^-250 relative error and rounded
//! correctly, so the result is the correctly rounded value. Physical x87 microcode (Core i5-3210M,
//! the project oracle) is not correctly rounded: it agrees on most operands and differs by one
//! unit in the last place on the rest. The only reproduced hardware property beyond rounding is the
//! argument reduction of FSIN/FCOS/FSINCOS/FPTAN, which subtracts multiples of the 66-bit
//! approximation of pi/2 (Intel SDM Vol. 1 section 8.3.8 "Approximation of Pi", 8.3.10 and Vol. 2A "FSIN"); the
//! 66-bit constant is the first 66 significant bits of pi. Operands with |x| >= 2^63 are outside the reduction range
//! (C2 is set by the caller and the operand is left unchanged).

use crate::rational_rounding::RationalRoundingMode;
use std::cmp::Ordering;

/// A binary80 value rounded from a 256-bit intermediate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87Rounded {
    /// Result bytes in memory order (canonical Normal, or +0/-0).
    pub bytes: [u8; 10],
    /// The result differs from the exact mathematical value (PE).
    pub inexact: bool,
    /// The magnitude was rounded away from zero (the round-up indicator C1).
    pub rounded_up: bool,
}

/// Result of a one- or two-result trigonometric instruction on a reducible operand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum X87Trig<T> {
    /// |x| >= 2^63: the operand cannot be reduced (C2 = 1, operand unchanged).
    OutOfRange,
    Done(T),
}

/// Sine and cosine of one operand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct X87SinCos {
    pub sin: X87Rounded,
    pub cos: X87Rounded,
}

// ---------------------------------------------------------------------------------------------
// 256-bit software float
// ---------------------------------------------------------------------------------------------

/// value = (m / 2^256) * 2^exp, m little-endian limbs with the top bit of m[3] set; zero has m = 0.
#[derive(Clone, Copy, Debug)]
struct Big {
    neg: bool,
    exp: i32,
    m: [u64; 4],
}

/// What is known about the true magnitude relative to a [`Big`] that is exact only to its last bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Adj {
    Exact,
    JustBelow,
    JustAbove,
}

fn shr5(x: [u64; 5], sh: u32) -> [u64; 5] {
    if sh >= 320 {
        return [0; 5];
    }
    let words = (sh / 64) as usize;
    let bits = sh % 64;
    let mut out = [0u64; 5];
    for (i, slot) in out.iter_mut().enumerate() {
        let src = i + words;
        if src >= 5 {
            break;
        }
        *slot = x[src] >> bits;
        if bits != 0 && src + 1 < 5 {
            *slot |= x[src + 1] << (64 - bits);
        }
    }
    out
}

fn shl5(x: [u64; 5], sh: u32) -> [u64; 5] {
    if sh >= 320 {
        return [0; 5];
    }
    let words = (sh / 64) as usize;
    let bits = sh % 64;
    let mut out = [0u64; 5];
    for i in (0..5).rev() {
        if i < words {
            break;
        }
        let src = i - words;
        out[i] = x[src] << bits;
        if bits != 0 && src >= 1 {
            out[i] |= x[src - 1] >> (64 - bits);
        }
    }
    out
}

fn add5(a: [u64; 5], b: [u64; 5]) -> ([u64; 5], bool) {
    let mut out = [0u64; 5];
    let mut carry = false;
    for i in 0..5 {
        let (s1, c1) = a[i].overflowing_add(b[i]);
        let (s2, c2) = s1.overflowing_add(u64::from(carry));
        out[i] = s2;
        carry = c1 || c2;
    }
    (out, carry)
}

fn sub5(a: [u64; 5], b: [u64; 5]) -> [u64; 5] {
    let mut out = [0u64; 5];
    let mut borrow = false;
    for i in 0..5 {
        let (s1, b1) = a[i].overflowing_sub(b[i]);
        let (s2, b2) = s1.overflowing_sub(u64::from(borrow));
        out[i] = s2;
        borrow = b1 || b2;
    }
    out
}

fn lead5(x: &[u64; 5]) -> u32 {
    let mut total = 0;
    for limb in x.iter().rev() {
        if *limb == 0 {
            total += 64;
        } else {
            return total + limb.leading_zeros();
        }
    }
    total
}

impl Big {
    const ZERO: Big = Big {
        neg: false,
        exp: 0,
        m: [0; 4],
    };

    fn is_zero(&self) -> bool {
        self.m[3] == 0
    }

    /// v * 2^pow2.
    fn from_u128(neg: bool, v: u128, pow2: i32) -> Big {
        if v == 0 {
            return Big::ZERO;
        }
        let lz = v.leading_zeros();
        let sh = v << lz;
        Big {
            neg,
            exp: 128 - lz as i32 + pow2,
            m: [0, 0, sh as u64, (sh >> 64) as u64],
        }
    }

    fn from_i64(v: i64) -> Big {
        Big::from_u128(v < 0, u128::from(v.unsigned_abs()), 0)
    }

    fn negated(mut self) -> Big {
        if !self.is_zero() {
            self.neg = !self.neg;
        }
        self
    }

    fn abs(mut self) -> Big {
        self.neg = false;
        self
    }

    fn scaled(mut self, pow2: i32) -> Big {
        if !self.is_zero() {
            self.exp += pow2;
        }
        self
    }

    fn cmp_mag(&self, other: &Big) -> Ordering {
        match (self.is_zero(), other.is_zero()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Less,
            (false, true) => return Ordering::Greater,
            _ => {}
        }
        self.exp
            .cmp(&other.exp)
            .then_with(|| self.m[3].cmp(&other.m[3]))
            .then_with(|| self.m[2].cmp(&other.m[2]))
            .then_with(|| self.m[1].cmp(&other.m[1]))
            .then_with(|| self.m[0].cmp(&other.m[0]))
    }

    fn add(self, other: Big) -> Big {
        if self.is_zero() {
            return other;
        }
        if other.is_zero() {
            return self;
        }
        if self.neg == other.neg {
            let (hi, lo) = if self.cmp_mag(&other) == Ordering::Less {
                (other, self)
            } else {
                (self, other)
            };
            let sh = (hi.exp - lo.exp) as u32;
            let x = [0, hi.m[0], hi.m[1], hi.m[2], hi.m[3]];
            let y = shr5([0, lo.m[0], lo.m[1], lo.m[2], lo.m[3]], sh);
            let (mut s, carry) = add5(x, y);
            let mut exp = hi.exp;
            if carry {
                s = shr5(s, 1);
                s[4] |= 1 << 63;
                exp += 1;
            }
            return Big {
                neg: hi.neg,
                exp,
                m: [s[1], s[2], s[3], s[4]],
            };
        }
        match self.cmp_mag(&other) {
            Ordering::Equal => Big::ZERO,
            Ordering::Greater => Big::sub_mag(self, other),
            Ordering::Less => Big::sub_mag(other, self),
        }
    }

    /// |hi| > |lo|; the result carries hi's sign.
    fn sub_mag(hi: Big, lo: Big) -> Big {
        let sh = (hi.exp - lo.exp) as u32;
        let x = [0, hi.m[0], hi.m[1], hi.m[2], hi.m[3]];
        let y = shr5([0, lo.m[0], lo.m[1], lo.m[2], lo.m[3]], sh);
        let d = sub5(x, y);
        let lz = lead5(&d);
        if lz >= 320 {
            return Big::ZERO;
        }
        let s = shl5(d, lz);
        Big {
            neg: hi.neg,
            exp: hi.exp - lz as i32,
            m: [s[1], s[2], s[3], s[4]],
        }
    }

    fn sub(self, other: Big) -> Big {
        self.add(other.negated())
    }

    fn mul(self, other: Big) -> Big {
        if self.is_zero() || other.is_zero() {
            return Big::ZERO;
        }
        let mut prod = [0u64; 8];
        for i in 0..4 {
            let mut carry = 0u128;
            for j in 0..4 {
                let cur = u128::from(prod[i + j])
                    + u128::from(self.m[i]) * u128::from(other.m[j])
                    + carry;
                prod[i + j] = cur as u64;
                carry = cur >> 64;
            }
            prod[i + 4] = carry as u64;
        }
        let mut exp = self.exp + other.exp;
        if prod[7] >> 63 == 0 {
            for i in (1..8).rev() {
                prod[i] = (prod[i] << 1) | (prod[i - 1] >> 63);
            }
            prod[0] <<= 1;
            exp -= 1;
        }
        Big {
            neg: self.neg != other.neg,
            exp,
            m: [prod[4], prod[5], prod[6], prod[7]],
        }
    }

    /// self / other, other != 0; truncated quotient of 256 significant bits.
    fn div(self, other: Big) -> Big {
        assert!(
            !other.is_zero(),
            "x87 transcendental model: division by zero"
        );
        if self.is_zero() {
            return Big::ZERO;
        }
        let b = [other.m[0], other.m[1], other.m[2], other.m[3], 0];
        let mut r = [self.m[0], self.m[1], self.m[2], self.m[3], 0];
        let ge = |r: &[u64; 5], b: &[u64; 5]| {
            for i in (0..5).rev() {
                if r[i] != b[i] {
                    return r[i] > b[i];
                }
            }
            true
        };
        let mut q = [0u64; 5]; // 257 quotient bits: bit 256 is the integer bit
        for bit in (0..=256u32).rev() {
            if ge(&r, &b) {
                r = sub5(r, b);
                q[(bit / 64) as usize] |= 1 << (bit % 64);
            }
            r = shl5(r, 1);
        }
        // q = floor(a.m * 2^256 / b.m) in (2^255, 2^257)
        let mut exp = self.exp - other.exp;
        let m = if q[4] & 1 == 1 {
            let s = shr5(q, 1);
            exp += 1;
            [s[0], s[1], s[2], s[3]]
        } else {
            [q[0], q[1], q[2], q[3]]
        };
        Big {
            neg: self.neg != other.neg,
            exp,
            m,
        }
    }

    /// self / d for a small positive integer d (256 significant bits kept).
    fn div_small(self, d: u64) -> Big {
        if self.is_zero() {
            return self;
        }
        let d = u128::from(d);
        let src = [0, self.m[0], self.m[1], self.m[2], self.m[3]];
        let mut q = [0u64; 5];
        let mut rem = 0u128;
        for i in (0..5).rev() {
            let cur = (rem << 64) | u128::from(src[i]);
            q[i] = (cur / d) as u64;
            rem = cur % d;
        }
        let lz = lead5(&q);
        let s = shl5(q, lz);
        Big {
            neg: self.neg,
            exp: self.exp - lz as i32,
            m: [s[1], s[2], s[3], s[4]],
        }
    }

    fn to_f64(self) -> f64 {
        if self.is_zero() {
            return 0.0;
        }
        let top = (self.m[3] >> 11) as f64;
        let v = top * 2f64.powi(self.exp - 53);
        if self.neg { -v } else { v }
    }

    fn from_f64(f: f64) -> Big {
        if f == 0.0 {
            return Big::ZERO;
        }
        let bits = f.to_bits();
        let neg = bits >> 63 != 0;
        let e = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1 << 52) - 1);
        assert!(
            e != 0 && e != 0x7ff,
            "seed conversion expects normal finite f64"
        );
        Big::from_u128(neg, u128::from(frac | 1 << 52), e - 1075)
    }

    /// Square root of a positive value (Newton iterations from an f64 seed).
    fn sqrt(self) -> Big {
        let mut x = Big::from_f64(self.to_f64().sqrt());
        for _ in 0..4 {
            x = x.add(self.div(x)).scaled(-1);
        }
        x
    }
}

// ---------------------------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------------------------

/// pi to 256 bits: value = m / 2^256 * 2^2.
fn big_pi() -> Big {
    Big {
        neg: false,
        exp: 2,
        m: [
            0x020b_bea6_3b13_9b22,
            0x2902_4e08_8a67_cc74,
            0xc4c6_628b_80dc_1cd1,
            0xc90f_daa2_2168_c234,
        ],
    }
}

/// ln 2 to 256 bits.
fn big_ln2() -> Big {
    Big {
        neg: false,
        exp: 0,
        m: [
            0x8a0d_175b_8baa_fa2b,
            0x40f3_4326_7298_b62d,
            0xc9e3_b398_03f2_f6af,
            0xb172_17f7_d1cf_79ab,
        ],
    }
}

/// The 66-bit approximation of pi/2 used by the hardware argument reduction: the first 66
/// significant bits of pi, divided by two.
fn big_half_pi_66() -> Big {
    let mant66 = (0xc90f_daa2_2168_c234u128 << 2) | 0b11;
    Big::from_u128(false, mant66, -65)
}

fn big_one() -> Big {
    Big::from_i64(1)
}

// ---------------------------------------------------------------------------------------------
// Series
// ---------------------------------------------------------------------------------------------

fn converged(term: &Big, sum: &Big) -> bool {
    term.is_zero() || (!sum.is_zero() && term.exp < sum.exp - 262)
}

/// (sin r, cos r) for |r| <= 0.8, r != 0.
fn sin_cos_series(r: Big) -> (Big, Big) {
    let r2 = r.mul(r);
    let mut ts = r;
    let mut tc = big_one();
    let mut sin = r;
    let mut cos = tc;
    let mut k = 1u64;
    loop {
        tc = tc.mul(r2).div_small((2 * k - 1) * (2 * k)).negated();
        ts = ts.mul(r2).div_small((2 * k) * (2 * k + 1)).negated();
        cos = cos.add(tc);
        sin = sin.add(ts);
        k += 1;
        if (converged(&tc, &cos) && converged(&ts, &sin)) || k > 120 {
            break;
        }
    }
    (sin, cos)
}

/// atanh(z) = z + z^3/3 + z^5/5 + ..., |z| < 0.2, z != 0.
fn atanh_series(z: Big) -> Big {
    let z2 = z.mul(z);
    let mut pow = z;
    let mut sum = z;
    let mut k = 1u64;
    loop {
        pow = pow.mul(z2);
        k += 2;
        let term = pow.div_small(k);
        sum = sum.add(term);
        if converged(&term, &sum) || k > 600 {
            break;
        }
    }
    sum
}

/// atan(t) = t - t^3/3 + t^5/5 - ..., |t| < 0.2, t != 0.
fn atan_series(t: Big) -> Big {
    let t2 = t.mul(t);
    let mut pow = t;
    let mut sum = t;
    let mut k = 1u64;
    let mut sign = true;
    loop {
        pow = pow.mul(t2);
        k += 2;
        let mut term = pow.div_small(k);
        if sign {
            term = term.negated();
        }
        sign = !sign;
        sum = sum.add(term);
        if converged(&term, &sum) || k > 600 {
            break;
        }
    }
    sum
}

/// atan of a positive value t <= 1.
fn atan_unit(t: Big) -> Big {
    let mut t = t;
    let mut scale = 0;
    for _ in 0..3 {
        let root = big_one().add(t.mul(t)).sqrt();
        t = t.div(big_one().add(root));
        scale += 1;
    }
    atan_series(t).scaled(scale)
}

/// exp(u) - 1 for |u| <= 0.7, u != 0.
fn expm1_series(u: Big) -> Big {
    let mut term = u;
    let mut sum = u;
    let mut k = 1u64;
    loop {
        k += 1;
        term = term.mul(u).div_small(k);
        sum = sum.add(term);
        if converged(&term, &sum) || k > 200 {
            break;
        }
    }
    sum
}

/// log2(x) for a positive normal x = mant * 2^(e-63): returns (value, exact).
fn log2_of(e: i32, mant: u64) -> (Big, bool) {
    if mant == 1 << 63 {
        return (Big::from_i64(i64::from(e)), true);
    }
    let mut e = e;
    let mut m = Big::from_u128(false, u128::from(mant), -63);
    if m.to_f64() > std::f64::consts::SQRT_2 {
        m = m.scaled(-1);
        e += 1;
    }
    let z = m.sub(big_one()).div(m.add(big_one()));
    let ln_m = atanh_series(z).scaled(1);
    let frac = ln_m.div(big_ln2());
    (Big::from_i64(i64::from(e)).add(frac), false)
}

// ---------------------------------------------------------------------------------------------
// Parsing and rounding
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
struct Operand {
    neg: bool,
    /// Unbiased exponent; unused for zero.
    exp: i32,
    mant: u64,
}

impl Operand {
    fn is_zero(&self) -> bool {
        self.mant == 0
    }

    fn big(&self) -> Big {
        Big::from_u128(self.neg, u128::from(self.mant), self.exp - 63)
    }
}

/// Canonical Normal or zero; everything else is outside the modelled domain.
fn parse(raw: [u8; 10], allow_zero: bool) -> Option<Operand> {
    let mant = u64::from_le_bytes(raw[..8].try_into().ok()?);
    let se = u16::from_le_bytes([raw[8], raw[9]]);
    let neg = se & 0x8000 != 0;
    let biased = i32::from(se & 0x7fff);
    if biased == 0 && mant == 0 {
        return allow_zero.then_some(Operand {
            neg,
            exp: 0,
            mant: 0,
        });
    }
    if biased == 0 || biased == 0x7fff || mant >> 63 == 0 {
        return None;
    }
    Some(Operand {
        neg,
        exp: biased - 16383,
        mant,
    })
}

fn pack(neg: bool, exp: i32, mant: u64) -> Option<[u8; 10]> {
    let biased = exp + 16383;
    if !(1..=0x7ffe).contains(&biased) {
        return None;
    }
    let mut bytes = [0u8; 10];
    bytes[..8].copy_from_slice(&mant.to_le_bytes());
    let se = (u16::from(neg) << 15) | biased as u16;
    bytes[8..].copy_from_slice(&se.to_le_bytes());
    Some(bytes)
}

fn zero_bytes(neg: bool) -> [u8; 10] {
    let mut bytes = [0u8; 10];
    bytes[9] = u8::from(neg) << 7;
    bytes
}

fn exact(bytes: [u8; 10]) -> X87Rounded {
    X87Rounded {
        bytes,
        inexact: false,
        rounded_up: false,
    }
}

/// Round |v| to a 64-bit significand under the mode; None when the exponent leaves the normal range.
fn round_big(v: Big, adj: Adj, mode: RationalRoundingMode) -> Option<X87Rounded> {
    if v.is_zero() {
        return Some(exact(zero_bytes(false)));
    }
    let neg = v.neg;
    let mut e = v.exp - 1;
    let mut mant = v.m[3];
    let rest = [v.m[0], v.m[1], v.m[2]];
    let rest_zero = rest == [0, 0, 0];
    let half = rest == [0, 0, 1 << 63];
    // Remainder class relative to one ulp: 0 = exactly zero, 1 = below half, 2 = half, 3 = above.
    let mut class = if rest_zero {
        0
    } else if half {
        2
    } else if rest[2] >> 63 == 1 {
        3
    } else {
        1
    };
    match adj {
        Adj::Exact => {}
        Adj::JustAbove => {
            class = match class {
                0 => 1,
                2 => 3,
                other => other,
            };
        }
        Adj::JustBelow => match class {
            0 => {
                // the true value is just below mant: floor is mant - 1, remainder just below an ulp.
                if mant == 1 << 63 {
                    mant = u64::MAX;
                    e -= 1;
                } else {
                    mant -= 1;
                }
                class = 3;
            }
            2 => class = 1,
            _ => {}
        },
    }
    let inexact = class != 0;
    let inc = match mode {
        RationalRoundingMode::NearestTiesEven => class == 3 || (class == 2 && mant & 1 == 1),
        RationalRoundingMode::TowardZero => false,
        RationalRoundingMode::TowardPositive => inexact && !neg,
        RationalRoundingMode::TowardNegative => inexact && neg,
    };
    if inc {
        let (m2, carry) = mant.overflowing_add(1);
        if carry {
            mant = 1 << 63;
            e += 1;
        } else {
            mant = m2;
        }
    }
    Some(X87Rounded {
        bytes: pack(neg, e, mant)?,
        inexact,
        rounded_up: inc,
    })
}

// ---------------------------------------------------------------------------------------------
// Trigonometry
// ---------------------------------------------------------------------------------------------

struct Reduced {
    quadrant: u8,
    /// |r| < 2^-68: the hardware returns r (sine, tangent) and 1.0 (cosine) with PE set and C1 clear
    /// in every rounding mode (observed on the oracle, exponents <= -69).
    tiny: bool,
    /// |r| < 2^-33 (and not tiny): the hardware tangent behaves as if tan(r) lay just below r.
    near_tiny: bool,
    sin: Big,
    cos: Big,
    r: Big,
}

fn reduce(x: Operand) -> Option<X87Trig<Reduced>> {
    if x.exp >= 63 {
        return Some(X87Trig::OutOfRange);
    }
    let xb = x.big();
    let hp = big_half_pi_66();
    let q = xb.div(hp);
    // n = nearest integer to q (|q| < 2^63).
    let n: i128 = if q.exp <= 0 {
        if q.exp == 0 {
            if q.neg { -1 } else { 1 }
        } else {
            0
        }
    } else {
        let top = (u128::from(q.m[3]) << 64) | u128::from(q.m[2]);
        let exp = q.exp as u32;
        let ip = (top >> (128 - exp)) as i128;
        let round = ((top >> (128 - exp - 1)) & 1) as i128;
        let mag = ip + round;
        if q.neg { -mag } else { mag }
    };
    let prod = Big::from_u128(n < 0, n.unsigned_abs(), 0).mul(hp);
    let r = xb.sub(prod);
    let quadrant = n.rem_euclid(4) as u8;
    if r.is_zero() {
        return None;
    }
    // r.exp - 1 is the binary exponent of |r|.
    let tiny = r.exp <= -68;
    let near_tiny = !tiny && r.exp <= -33;
    let (sin, cos) = if tiny {
        (r, big_one())
    } else {
        sin_cos_series(r)
    };
    Some(X87Trig::Done(Reduced {
        quadrant,
        tiny,
        near_tiny,
        sin,
        cos,
        r,
    }))
}

impl Reduced {
    /// (sin x, cos x) before rounding.
    fn sin_cos_x(&self) -> (Big, Big) {
        let (s, c) = (self.sin, self.cos);
        match self.quadrant {
            0 => (s, c),
            1 => (c, s.negated()),
            2 => (s.negated(), c.negated()),
            _ => (c.negated(), s),
        }
    }

    /// Rounds a result of this reduction; a tiny reduced argument reports PE although the
    /// returned value is the operand itself (or 1.0).
    fn finish(&self, value: Big, adj: Adj, mode: RationalRoundingMode) -> Option<X87Rounded> {
        let mut out = round_big(value, adj, mode)?;
        if self.tiny {
            out.inexact = true;
        }
        Some(out)
    }

    fn tan_x(&self) -> (Big, Adj) {
        let odd = self.quadrant & 1 == 1;
        if self.tiny {
            if odd {
                (big_one().div(self.r).negated(), Adj::JustBelow)
            } else {
                (self.r, Adj::Exact)
            }
        } else if self.near_tiny && !odd {
            (self.r, Adj::JustBelow)
        } else if odd {
            (self.cos.div(self.sin).negated(), Adj::Exact)
        } else {
            (self.sin.div(self.cos), Adj::Exact)
        }
    }
}

fn one_bytes() -> [u8; 10] {
    pack(false, 0, 1 << 63).expect("1.0 is a normal binary80")
}

pub fn fsin(raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Trig<X87Rounded>> {
    let x = parse(raw, true)?;
    if x.is_zero() {
        return Some(X87Trig::Done(exact(raw)));
    }
    Some(match reduce(x)? {
        X87Trig::OutOfRange => X87Trig::OutOfRange,
        X87Trig::Done(red) => {
            let (s, _) = red.sin_cos_x();
            X87Trig::Done(red.finish(s, Adj::Exact, mode)?)
        }
    })
}

pub fn fcos(raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Trig<X87Rounded>> {
    let x = parse(raw, true)?;
    if x.is_zero() {
        return Some(X87Trig::Done(exact(one_bytes())));
    }
    Some(match reduce(x)? {
        X87Trig::OutOfRange => X87Trig::OutOfRange,
        X87Trig::Done(red) => {
            let (_, c) = red.sin_cos_x();
            X87Trig::Done(red.finish(c, Adj::Exact, mode)?)
        }
    })
}

pub fn fsincos(raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Trig<X87SinCos>> {
    let x = parse(raw, true)?;
    if x.is_zero() {
        return Some(X87Trig::Done(X87SinCos {
            sin: exact(raw),
            cos: exact(one_bytes()),
        }));
    }
    Some(match reduce(x)? {
        X87Trig::OutOfRange => X87Trig::OutOfRange,
        X87Trig::Done(red) => {
            let (s, c) = red.sin_cos_x();
            X87Trig::Done(X87SinCos {
                sin: red.finish(s, Adj::Exact, mode)?,
                cos: red.finish(c, Adj::Exact, mode)?,
            })
        }
    })
}

/// FPTAN's tangent (the pushed 1.0 is the caller's).
pub fn fptan(raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Trig<X87Rounded>> {
    let x = parse(raw, true)?;
    if x.is_zero() {
        return Some(X87Trig::Done(exact(raw)));
    }
    Some(match reduce(x)? {
        X87Trig::OutOfRange => X87Trig::OutOfRange,
        X87Trig::Done(red) => {
            let (t, adj) = red.tan_x();
            X87Trig::Done(red.finish(t, adj, mode)?)
        }
    })
}

// ---------------------------------------------------------------------------------------------
// Exponential, logarithms, arctangent
// ---------------------------------------------------------------------------------------------

/// 2^x - 1 for -1 <= x <= 1.
pub fn f2xm1(raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Rounded> {
    let x = parse(raw, true)?;
    if x.is_zero() {
        return Some(exact(raw));
    }
    if x.exp > 0 {
        return None;
    }
    if x.exp == 0 {
        if x.mant != 1 << 63 {
            return None;
        }
        // 2^1 - 1 = 1 and 2^-1 - 1 = -1/2, exactly; the hardware still reports PE.
        let bytes = if x.neg {
            pack(true, -1, 1 << 63)?
        } else {
            one_bytes()
        };
        return Some(X87Rounded {
            bytes,
            inexact: true,
            rounded_up: false,
        });
    }
    let u = x.big().mul(big_ln2());
    round_big(expm1_series(u), Adj::Exact, mode)
}

/// y * log2(x) for x > 0 (FYL2X pops; the result replaces ST(1)).
pub fn fyl2x(x_raw: [u8; 10], y_raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Rounded> {
    let x = parse(x_raw, false)?;
    let y = parse(y_raw, false)?;
    if x.neg {
        return None;
    }
    let (lg, power_of_two) = log2_of(x.exp, x.mant);
    if lg.is_zero() {
        // log2(1) = +0: the product is a zero with the sign of y, no exception.
        return Some(exact(zero_bytes(y.neg)));
    }
    let mut out = round_big(y.big().mul(lg), Adj::Exact, mode)?;
    if power_of_two {
        // The microcode evaluates log2 approximately even for 2^k: PE is set, C1 is not modelled.
        out.inexact = true;
    }
    Some(out)
}

/// The largest binary80 significand (explicit integer bit, exponent -2) not above 1 - sqrt(2)/2, i.e.
/// floor((4 - 2*sqrt(2)) * 2^63); 1 - sqrt(2)/2 is irrational, so the SDM range bound is never reached exactly.
const FYL2XP1_LIMIT_MANT: u64 = 0x95f6_1998_0c43_36f7;

/// y * log2(x + 1) for -(1 - sqrt(2)/2) <= x <= 1 - sqrt(2)/2 (FYL2XP1 pops; Intel SDM Vol. 2A, FYL2XP1,
/// the range of ST(0)). Outside it the SDM calls the result undefined, so the model refuses the operand.
pub fn fyl2xp1(x_raw: [u8; 10], y_raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Rounded> {
    let x = parse(x_raw, true)?;
    let y = parse(y_raw, false)?;
    if x.is_zero() {
        return Some(exact(zero_bytes(x.neg != y.neg)));
    }
    // Exact comparison of |x| with 1 - sqrt(2)/2 = 0x1.95f6..p-2 (no floating-point rounding at the bound).
    if x.exp > -2 || (x.exp == -2 && x.mant > FYL2XP1_LIMIT_MANT) {
        return None;
    }
    let xb = x.big();
    let two = Big::from_i64(2);
    let z = xb.div(two.add(xb));
    let ln1p = atanh_series(z).scaled(1);
    let lg = ln1p.div(big_ln2());
    round_big(y.big().mul(lg), Adj::Exact, mode)
}

/// atan2(y, x) for nonzero normal operands (FPATAN pops; the result replaces ST(1)).
pub fn fpatan(y_raw: [u8; 10], x_raw: [u8; 10], mode: RationalRoundingMode) -> Option<X87Rounded> {
    let y = parse(y_raw, false)?;
    let x = parse(x_raw, false)?;
    let (ay, ax) = (y.big().abs(), x.big().abs());
    let ratio_swapped = ay.cmp_mag(&ax) == Ordering::Greater;
    let t = if ratio_swapped {
        ax.div(ay)
    } else {
        ay.div(ax)
    };
    // Below 2^-40 the hardware returns the rounded quotient itself, with PE.
    let tiny = !ratio_swapped && !x.neg && t.exp <= -40;
    let (angle, adj) = if tiny {
        (t, Adj::Exact)
    } else {
        (atan_unit(t), Adj::Exact)
    };
    let pi = big_pi();
    let half_pi = pi.scaled(-1);
    // |y|/|x| angle in [0, pi/2]
    let (mut value, mut adj) = if ratio_swapped {
        (
            half_pi.sub(angle),
            if adj == Adj::JustBelow {
                Adj::JustAbove
            } else {
                adj
            },
        )
    } else {
        (angle, adj)
    };
    if x.neg {
        value = pi.sub(value);
        adj = match adj {
            Adj::JustBelow => Adj::JustAbove,
            Adj::JustAbove => Adj::JustBelow,
            Adj::Exact => Adj::Exact,
        };
    }
    if y.neg {
        value = value.negated();
    }
    let mut out = round_big(value, adj, mode)?;
    if tiny {
        out.inexact = true;
    }
    Some(out)
}

// ---------------------------------------------------------------------------------------------
// Exact scaling and extraction
// ---------------------------------------------------------------------------------------------

/// ST(0) * 2^trunc(ST(1)) for a normal or zero ST(0) and a normal or zero ST(1); None when the
/// result leaves the normal range.
pub fn fscale(x_raw: [u8; 10], scale_raw: [u8; 10]) -> Option<[u8; 10]> {
    let x = parse(x_raw, true)?;
    let s = parse(scale_raw, true)?;
    if x.is_zero() {
        return Some(x_raw);
    }
    let n: i32 = if s.is_zero() || s.exp < 0 {
        0
    } else if s.exp > 20 {
        return None;
    } else {
        let mag = (s.mant >> (63 - s.exp)) as i32;
        if s.neg { -mag } else { mag }
    };
    pack(x.neg, x.exp + n, x.mant)
}

/// FXTRACT: (significand with the sign of x and exponent 0, unbiased exponent as binary80).
pub fn fxtract(raw: [u8; 10]) -> Option<([u8; 10], [u8; 10])> {
    let x = parse(raw, false)?;
    let significand = pack(x.neg, 0, x.mant)?;
    let exponent = if x.exp == 0 {
        zero_bytes(false)
    } else {
        let mag = u64::from(x.exp.unsigned_abs());
        let lz = mag.leading_zeros();
        pack(x.exp < 0, 63 - lz as i32, mag << lz)?
    };
    Some((significand, exponent))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b80(v: f64) -> [u8; 10] {
        let bits = v.to_bits();
        let neg = bits >> 63 != 0;
        let e = ((bits >> 52) & 0x7ff) as i32 - 1023;
        let mant = ((bits & ((1 << 52) - 1)) | 1 << 52) << 11;
        pack(neg, e, mant).unwrap()
    }

    fn to_f64(raw: [u8; 10]) -> f64 {
        let o = parse(raw, true).unwrap();
        if o.is_zero() {
            return 0.0;
        }
        o.big().to_f64()
    }

    const RN: RationalRoundingMode = RationalRoundingMode::NearestTiesEven;

    #[test]
    fn big_arithmetic_matches_f64_on_small_cases() {
        let a = Big::from_f64(1.5);
        let b = Big::from_f64(-0.3);
        assert!((a.add(b).to_f64() - 1.2).abs() < 1e-15);
        assert!((a.mul(b).to_f64() + 0.45).abs() < 1e-15);
        assert!((a.div(b).to_f64() + 5.0).abs() < 1e-14);
        assert!((Big::from_f64(2.0).sqrt().to_f64() - 2f64.sqrt()).abs() < 1e-15);
        assert!((Big::from_f64(7.0).div_small(3).to_f64() - 7.0 / 3.0).abs() < 1e-15);
    }

    #[test]
    fn trig_values_match_f64() {
        for v in [0.1f64, 0.5, 1.0, 2.0, 3.0, 10.0, 100.0, -0.7, -5.5] {
            let Some(X87Trig::Done(s)) = fsin(b80(v), RN) else {
                panic!()
            };
            assert!((to_f64(s.bytes) - v.sin()).abs() < 1e-15, "sin {v}");
            let Some(X87Trig::Done(c)) = fcos(b80(v), RN) else {
                panic!()
            };
            assert!((to_f64(c.bytes) - v.cos()).abs() < 1e-15, "cos {v}");
            let Some(X87Trig::Done(t)) = fptan(b80(v), RN) else {
                panic!()
            };
            assert!(
                (to_f64(t.bytes) - v.tan()).abs() < 1e-13 * v.tan().abs().max(1.0),
                "tan {v}"
            );
        }
    }

    #[test]
    fn exp_log_atan_match_f64() {
        for v in [0.1f64, 0.5, 1.0, -0.5, -0.9] {
            let r = f2xm1(b80(v), RN).unwrap();
            assert!(
                (to_f64(r.bytes) - (v.exp2() - 1.0)).abs() < 1e-15,
                "f2xm1 {v}"
            );
        }
        let r = fyl2x(b80(10.0), b80(3.0), RN).unwrap();
        assert!((to_f64(r.bytes) - 3.0 * 10f64.log2()).abs() < 1e-14);
        let r = fyl2xp1(b80(0.25), b80(2.0), RN).unwrap();
        assert!((to_f64(r.bytes) - 2.0 * 1.25f64.log2()).abs() < 1e-15);
        for (y, x) in [
            (1.0f64, 1.0f64),
            (1.0, 2.0),
            (3.0, 1.0),
            (-1.0, -2.0),
            (2.0, -3.0),
            (0.001, 5.0),
        ] {
            let r = fpatan(b80(y), b80(x), RN).unwrap();
            assert!(
                (to_f64(r.bytes) - y.atan2(x)).abs() < 1e-15,
                "atan2 {y} {x}"
            );
        }
    }

    #[test]
    fn exact_cases_keep_the_hardware_exception_reporting() {
        // The microcode reports PE for these exactly representable results (oracle, Core i5-3210M).
        let r = f2xm1(b80(1.0), RN).unwrap();
        assert_eq!(
            (to_f64(r.bytes), r.inexact, r.rounded_up),
            (1.0, true, false)
        );
        let r = fyl2x(b80(8.0), b80(2.0), RN).unwrap();
        assert_eq!((to_f64(r.bytes), r.inexact), (6.0, true));
        // log2(1) = +0 carries the sign of y and no exception; 2^0 - 1 = 0.
        let r = fyl2x(b80(1.0), b80(-2.0), RN).unwrap();
        assert_eq!(
            (r.bytes, r.inexact),
            ([0, 0, 0, 0, 0, 0, 0, 0, 0, 0x80], false)
        );
        let r = f2xm1([0; 10], RN).unwrap();
        assert_eq!((to_f64(r.bytes), r.inexact), (0.0, false));
        let (sig, e) = fxtract(b80(12.0)).unwrap();
        assert_eq!((to_f64(sig), to_f64(e)), (1.5, 3.0));
        assert_eq!(to_f64(fscale(b80(1.5), b80(3.9)).unwrap()), 12.0);
        assert_eq!(to_f64(fscale(b80(1.5), b80(-3.9)).unwrap()), 0.1875);
    }
}
