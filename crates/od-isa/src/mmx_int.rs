//! S2-39 (MMX): the integer MMX instructions of the 64-bit MMX registers MM0..MM7 (the low quadwords of the
//! physical x87 data registers, see [crate::state::X87State::mmx_register]) as pure lane arithmetic.
//!
//! Intel SDM combined 325462-093US, Vol. 2 instruction pages (the "Operation" sections of the 64-bit MMX
//! versions): PADDB/PADDW/PADDD, PADDSB/PADDSW, PADDUSB/PADDUSW, PSUBB/PSUBW/PSUBD, PSUBSB/PSUBSW,
//! PSUBUSB/PSUBUSW, PAND, PANDN, POR, PXOR, PCMPEQB/W/D, PCMPGTB/W/D, PMULLW, PMULHW, PMADDWD,
//! PUNPCKLBW/LWD/LDQ, PUNPCKHBW/HWD/HDQ, PACKSSWB, PACKUSWB, PACKSSDW, PSLLW/D/Q, PSRLW/D/Q, PSRAW/D and
//! PSHUFW. Vol. 1 §9.4 describes the data types and the wraparound, signed and unsigned saturation modes.
//! The MMX forms of the SSE2 additions (PADDQ, PSUBQ, PMULUDQ, PSADBW, PAVG, PMAX, PMIN, PMULHUW, PEXTRW,
//! PINSRW, PMOVMSKB, MASKMOVQ, MOVNTQ, MOVDQ2Q, MOVQ2DQ) are not in this set.

/// The MMX instructions of two operands (destination MMd, source MMs or an 8-byte memory operand) and the
/// shifts, whose count is the source operand or an 8-bit immediate. The 128-bit versions of the same opcodes
/// (with the 66 prefix) are not described by this type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MmxIntKind {
    Paddb,
    Paddw,
    Paddd,
    Paddsb,
    Paddsw,
    Paddusb,
    Paddusw,
    Psubb,
    Psubw,
    Psubd,
    Psubsb,
    Psubsw,
    Psubusb,
    Psubusw,
    Pand,
    Pandn,
    Por,
    Pxor,
    Pcmpeqb,
    Pcmpeqw,
    Pcmpeqd,
    Pcmpgtb,
    Pcmpgtw,
    Pcmpgtd,
    Pmullw,
    Pmulhw,
    Pmaddwd,
    Punpcklbw,
    Punpcklwd,
    Punpckldq,
    Punpckhbw,
    Punpckhwd,
    Punpckhdq,
    Packsswb,
    Packuswb,
    Packssdw,
    Psllw,
    Pslld,
    Psllq,
    Psrlw,
    Psrld,
    Psrlq,
    Psraw,
    Psrad,
    /// `PSHUFW mm, mm/m64, imm8`: word i of the result is the source word selected by imm8[2i+1:2i].
    Pshufw,
}

impl MmxIntKind {
    /// The kind of the no-prefix `0F xx /r` opcode byte (PSHUFW included, whose imm8 follows ModR/M); the
    /// shift groups `0F 71/72/73`, MOVD/MOVQ and EMMS are decoded separately.
    pub const fn from_opcode(byte: u8) -> Option<Self> {
        Some(match byte {
            0x60 => Self::Punpcklbw,
            0x61 => Self::Punpcklwd,
            0x62 => Self::Punpckldq,
            0x63 => Self::Packsswb,
            0x64 => Self::Pcmpgtb,
            0x65 => Self::Pcmpgtw,
            0x66 => Self::Pcmpgtd,
            0x67 => Self::Packuswb,
            0x68 => Self::Punpckhbw,
            0x69 => Self::Punpckhwd,
            0x6a => Self::Punpckhdq,
            0x6b => Self::Packssdw,
            0x70 => Self::Pshufw,
            0x74 => Self::Pcmpeqb,
            0x75 => Self::Pcmpeqw,
            0x76 => Self::Pcmpeqd,
            0xd1 => Self::Psrlw,
            0xd2 => Self::Psrld,
            0xd3 => Self::Psrlq,
            0xd5 => Self::Pmullw,
            0xd8 => Self::Psubusb,
            0xd9 => Self::Psubusw,
            0xdb => Self::Pand,
            0xdc => Self::Paddusb,
            0xdd => Self::Paddusw,
            0xdf => Self::Pandn,
            0xe1 => Self::Psraw,
            0xe2 => Self::Psrad,
            0xe5 => Self::Pmulhw,
            0xe8 => Self::Psubsb,
            0xe9 => Self::Psubsw,
            0xeb => Self::Por,
            0xec => Self::Paddsb,
            0xed => Self::Paddsw,
            0xef => Self::Pxor,
            0xf1 => Self::Psllw,
            0xf2 => Self::Pslld,
            0xf3 => Self::Psllq,
            0xf5 => Self::Pmaddwd,
            0xf8 => Self::Psubb,
            0xf9 => Self::Psubw,
            0xfa => Self::Psubd,
            0xfc => Self::Paddb,
            0xfd => Self::Paddw,
            0xfe => Self::Paddd,
            _ => return None,
        })
    }

    /// The shift kind of `0F 71/72/73 /r ib` (the ModR/M reg field: 2 logical right, 4 arithmetic right,
    /// 6 left); `None` for an encoding the MMX forms do not have (/3 and /7 of 73 are the 128-bit byte
    /// shifts, which need the 66 prefix).
    pub const fn shift_group(opcode: u8, reg: u8) -> Option<Self> {
        Some(match (opcode, reg) {
            (0x71, 2) => Self::Psrlw,
            (0x71, 4) => Self::Psraw,
            (0x71, 6) => Self::Psllw,
            (0x72, 2) => Self::Psrld,
            (0x72, 4) => Self::Psrad,
            (0x72, 6) => Self::Pslld,
            (0x73, 2) => Self::Psrlq,
            (0x73, 6) => Self::Psllq,
            _ => return None,
        })
    }

    /// Whether an imm8 follows the ModR/M operand.
    pub const fn has_immediate(self) -> bool {
        matches!(self, Self::Pshufw)
    }

    /// Whether the count of the shift is the second operand.
    pub const fn is_shift(self) -> bool {
        matches!(
            self,
            Self::Psllw
                | Self::Pslld
                | Self::Psllq
                | Self::Psrlw
                | Self::Psrld
                | Self::Psrlq
                | Self::Psraw
                | Self::Psrad
        )
    }
}

fn bytes(dest: [u8; 8], src: [u8; 8], f: impl Fn(u8, u8) -> u8) -> [u8; 8] {
    core::array::from_fn(|lane| f(dest[lane], src[lane]))
}

fn word(value: [u8; 8], lane: usize) -> u16 {
    u16::from_le_bytes([value[2 * lane], value[2 * lane + 1]])
}

fn dword(value: [u8; 8], lane: usize) -> u32 {
    u32::from_le_bytes(value[4 * lane..4 * lane + 4].try_into().unwrap())
}

fn words(dest: [u8; 8], src: [u8; 8], f: impl Fn(u16, u16) -> u16) -> [u8; 8] {
    let mut result = [0; 8];
    for lane in 0..4 {
        result[2 * lane..2 * lane + 2]
            .copy_from_slice(&f(word(dest, lane), word(src, lane)).to_le_bytes());
    }
    result
}

fn dwords(dest: [u8; 8], src: [u8; 8], f: impl Fn(u32, u32) -> u32) -> [u8; 8] {
    let mut result = [0; 8];
    for lane in 0..2 {
        result[4 * lane..4 * lane + 4]
            .copy_from_slice(&f(dword(dest, lane), dword(src, lane)).to_le_bytes());
    }
    result
}

fn from_words(lanes: [u16; 4]) -> [u8; 8] {
    let mut result = [0; 8];
    for (lane, value) in lanes.into_iter().enumerate() {
        result[2 * lane..2 * lane + 2].copy_from_slice(&value.to_le_bytes());
    }
    result
}

fn mask(condition: bool) -> u8 {
    if condition { 0xff } else { 0 }
}

/// The shifts of PSLLW/D/Q, PSRLW/D/Q and PSRAW/D (Intel SDM Vol. 2): the count is the whole 64-bit source
/// operand or the zero-extended imm8; a logical shift by more than the lane width less one gives zero, an
/// arithmetic shift by more fills every lane with its sign bit.
pub fn mmx_shift_result(kind: MmxIntKind, dest: [u8; 8], count: u64) -> [u8; 8] {
    let word_count = u32::try_from(count).ok().filter(|count| *count <= 15);
    let dword_count = u32::try_from(count).ok().filter(|count| *count <= 31);
    let qword = u64::from_le_bytes(dest);
    match kind {
        MmxIntKind::Psllw => words(dest, dest, |lane, _| {
            word_count.map_or(0, |count| lane << count)
        }),
        MmxIntKind::Psrlw => words(dest, dest, |lane, _| {
            word_count.map_or(0, |count| lane >> count)
        }),
        MmxIntKind::Psraw => words(dest, dest, |lane, _| {
            (lane as i16 >> word_count.unwrap_or(15)) as u16
        }),
        MmxIntKind::Pslld => dwords(dest, dest, |lane, _| {
            dword_count.map_or(0, |count| lane << count)
        }),
        MmxIntKind::Psrld => dwords(dest, dest, |lane, _| {
            dword_count.map_or(0, |count| lane >> count)
        }),
        MmxIntKind::Psrad => dwords(dest, dest, |lane, _| {
            (lane as i32 >> dword_count.unwrap_or(31)) as u32
        }),
        MmxIntKind::Psllq => (if count > 63 { 0 } else { qword << count }).to_le_bytes(),
        MmxIntKind::Psrlq => (if count > 63 { 0 } else { qword >> count }).to_le_bytes(),
        _ => unreachable!("not a shift kind"),
    }
}

/// The result of an MMX instruction for the destination register value `dest` and the source operand
/// `src` (a register value or the 8 bytes of memory, little endian); `immediate` is the PSHUFW control
/// and ignored by the others. The shift kinds take the count from `src`.
pub fn mmx_int_result(kind: MmxIntKind, dest: [u8; 8], src: [u8; 8], immediate: u8) -> [u8; 8] {
    use MmxIntKind::*;
    match kind {
        Paddb => bytes(dest, src, u8::wrapping_add),
        Paddw => words(dest, src, u16::wrapping_add),
        Paddd => dwords(dest, src, u32::wrapping_add),
        Paddsb => bytes(dest, src, |a, b| (a as i8).saturating_add(b as i8) as u8),
        Paddsw => words(dest, src, |a, b| (a as i16).saturating_add(b as i16) as u16),
        Paddusb => bytes(dest, src, u8::saturating_add),
        Paddusw => words(dest, src, u16::saturating_add),
        Psubb => bytes(dest, src, u8::wrapping_sub),
        Psubw => words(dest, src, u16::wrapping_sub),
        Psubd => dwords(dest, src, u32::wrapping_sub),
        Psubsb => bytes(dest, src, |a, b| (a as i8).saturating_sub(b as i8) as u8),
        Psubsw => words(dest, src, |a, b| (a as i16).saturating_sub(b as i16) as u16),
        Psubusb => bytes(dest, src, u8::saturating_sub),
        Psubusw => words(dest, src, u16::saturating_sub),
        Pand => bytes(dest, src, |a, b| a & b),
        Pandn => bytes(dest, src, |a, b| !a & b),
        Por => bytes(dest, src, |a, b| a | b),
        Pxor => bytes(dest, src, |a, b| a ^ b),
        Pcmpeqb => bytes(dest, src, |a, b| mask(a == b)),
        Pcmpeqw => words(dest, src, |a, b| if a == b { 0xffff } else { 0 }),
        Pcmpeqd => dwords(dest, src, |a, b| if a == b { u32::MAX } else { 0 }),
        Pcmpgtb => bytes(dest, src, |a, b| mask(a as i8 > b as i8)),
        Pcmpgtw => words(
            dest,
            src,
            |a, b| if a as i16 > b as i16 { 0xffff } else { 0 },
        ),
        Pcmpgtd => dwords(
            dest,
            src,
            |a, b| if a as i32 > b as i32 { u32::MAX } else { 0 },
        ),
        Pmullw => words(dest, src, |a, b| {
            (i32::from(a as i16) * i32::from(b as i16)) as u16
        }),
        Pmulhw => words(dest, src, |a, b| {
            ((i32::from(a as i16) * i32::from(b as i16)) >> 16) as u16
        }),
        Pmaddwd => {
            let mut result = [0; 8];
            for lane in 0..2 {
                let low =
                    i32::from(word(dest, 2 * lane) as i16) * i32::from(word(src, 2 * lane) as i16);
                let high = i32::from(word(dest, 2 * lane + 1) as i16)
                    * i32::from(word(src, 2 * lane + 1) as i16);
                result[4 * lane..4 * lane + 4]
                    .copy_from_slice(&low.wrapping_add(high).to_le_bytes());
            }
            result
        }
        Punpcklbw => core::array::from_fn(|index| {
            let lane = index / 2;
            if index % 2 == 0 {
                dest[lane]
            } else {
                src[lane]
            }
        }),
        Punpckhbw => core::array::from_fn(|index| {
            let lane = 4 + index / 2;
            if index % 2 == 0 {
                dest[lane]
            } else {
                src[lane]
            }
        }),
        Punpcklwd => from_words([word(dest, 0), word(src, 0), word(dest, 1), word(src, 1)]),
        Punpckhwd => from_words([word(dest, 2), word(src, 2), word(dest, 3), word(src, 3)]),
        Punpckldq => {
            let mut result = [0; 8];
            result[..4].copy_from_slice(&dest[..4]);
            result[4..].copy_from_slice(&src[..4]);
            result
        }
        Punpckhdq => {
            let mut result = [0; 8];
            result[..4].copy_from_slice(&dest[4..]);
            result[4..].copy_from_slice(&src[4..]);
            result
        }
        Packsswb => core::array::from_fn(|index| {
            let value = if index < 4 {
                word(dest, index)
            } else {
                word(src, index - 4)
            };
            (value as i16).clamp(-128, 127) as i8 as u8
        }),
        Packuswb => core::array::from_fn(|index| {
            let value = if index < 4 {
                word(dest, index)
            } else {
                word(src, index - 4)
            };
            (value as i16).clamp(0, 255) as u8
        }),
        Packssdw => from_words(core::array::from_fn(|index| {
            let value = if index < 2 {
                dword(dest, index)
            } else {
                dword(src, index - 2)
            };
            (value as i32).clamp(-32768, 32767) as i16 as u16
        })),
        Psllw | Pslld | Psllq | Psrlw | Psrld | Psrlq | Psraw | Psrad => {
            mmx_shift_result(kind, dest, u64::from_le_bytes(src))
        }
        Pshufw => from_words(core::array::from_fn(|index| {
            word(src, usize::from((immediate >> (2 * index)) & 3))
        })),
    }
}
