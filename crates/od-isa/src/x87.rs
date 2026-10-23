//! Lossless binary80 fields, raw x87 encoding classification, and finite
//! integer-only helper representations.
//!
//! This module does not execute x87 instructions or perform arithmetic. The
//! byte array follows memory order, least significant byte first (Intel SDM
//! revision 093, Vol. 1 §§4.2.2 and 8.2.2, Tables 4-3 and 8-3).

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
    use super::{X87Binary80, X87Binary80Class, X87Grs, shift_right_jam};
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
}
