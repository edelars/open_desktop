//! S2-39 (integer SSE2 remainder): the integer SSE2 instructions on XMM registers that the narrow register
//! profile of `long_decode` does not carry, as pure lane arithmetic on 128-bit values.
//!
//! Intel SDM combined 325462-093US, Vol. 2 instruction pages ("Operation" of the 128-bit versions with the 66
//! prefix): PADDB/W/D/Q, PSUBB/W/D/Q, PADDSB/W, PADDUSB/W, PSUBSB/W, PSUBUSB/W, PAND, PANDN, POR, PXOR,
//! PCMPEQB/W/D, PCMPGTB/W/D, PMULLW, PMULHW, PMULHUW, PMADDWD, PMULUDQ, PSADBW, PAVGB/W, PMAXUB, PMINUB,
//! PMAXSW, PMINSW, PUNPCK{L,H}{BW,WD,DQ,QDQ}, PACKSSWB, PACKUSWB, PACKSSDW, PSLL/PSRL{W,D,Q} and PSRA{W,D}
//! by an XMM or 128-bit memory count (the count is the low quadword), PSHUFD, PSHUFHW, PSHUFLW, MOVQ,
//! MOVD, MOVLPS/MOVLPD, MOVHPS/MOVHPD, MOVHLPS, MOVLHPS, PEXTRW, PINSRW and the register forms of MOVDQU
//! and MOVDQA; S2-39 (rest) adds the non-temporal stores MOVNTDQ/MOVNTPS/MOVNTPD, MASKMOVDQU and MOVDQ2Q/MOVQ2DQ
//! (the last three execute through their own operations). Vol. 1 section 10.4.2 describes the 128-bit packed integer data types; Vol. 2A section 2.5
//! (Table 2-21/2-22, exception types 4 and 5) gives the alignment and exception rules the executor applies.

/// The operation of an [`crate::long_decode::Operation::Sse2Int`] instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sse2IntKind {
    Paddb,
    Paddw,
    Paddd,
    Paddq,
    Psubb,
    Psubw,
    Psubd,
    Psubq,
    Paddsb,
    Paddsw,
    Paddusb,
    Paddusw,
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
    Pmulhuw,
    Pmaddwd,
    Pmuludq,
    Psadbw,
    Pavgb,
    Pavgw,
    Pmaxub,
    Pminub,
    Pmaxsw,
    Pminsw,
    Punpcklbw,
    Punpcklwd,
    Punpckldq,
    Punpcklqdq,
    Punpckhbw,
    Punpckhwd,
    Punpckhdq,
    Punpckhqdq,
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
    /// `PSHUFD xmm, xmm/m128, imm8`: dword i of the result is the source dword selected by imm8[2i+1:2i].
    Pshufd,
    /// `PSHUFHW`: the low quadword of the source is copied, word 4+i is the source word 4+imm8[2i+1:2i].
    Pshufhw,
    /// `PSHUFLW`: word i is the source word imm8[2i+1:2i], the high quadword of the source is copied.
    Pshuflw,
    /// `F3 0F 7E` MOVQ xmm, xmm/m64: the low quadword of the source, the high quadword cleared.
    MovqLoad,
    /// `66 0F D6` MOVQ xmm/m64, xmm: the low quadword of the source; an XMM destination has its high quadword
    /// cleared.
    MovqStore,
    /// `66 0F 6E` MOVD xmm, r/m32 (zero-extended into the low quadword, the high quadword cleared).
    MovdLoad,
    /// `66 REX.W 0F 6E` MOVQ xmm, r/m64.
    MovqFromGeneral,
    /// `66 0F 7E` MOVD r/m32, xmm (a general register destination is zero-extended to 64 bits).
    MovdStore,
    /// `66 REX.W 0F 7E` MOVQ r/m64, xmm.
    MovqToGeneral,
    /// `66 0F C4` PINSRW xmm, r32/m16, imm8: word imm8[2:0] of the destination is replaced, the rest kept.
    Pinsrw,
    /// `66 0F C5` PEXTRW r32, xmm, imm8: word imm8[2:0] of the source, zero-extended (register source only).
    Pextrw,
    /// `0F 12` MOVLPS and `66 0F 12` MOVLPD xmm, m64: the low quadword of the destination is loaded.
    MovlLoad,
    /// `0F 16` MOVHPS and `66 0F 16` MOVHPD xmm, m64: the high quadword of the destination is loaded.
    MovhLoad,
    /// `0F 13` MOVLPS and `66 0F 13` MOVLPD m64, xmm: the low quadword of the source is stored.
    MovlStore,
    /// `0F 17` MOVHPS and `66 0F 17` MOVHPD m64, xmm: the high quadword of the source is stored.
    MovhStore,
    /// `0F 12` with a register source (MOVHLPS xmm1, xmm2): the low quadword of the destination is the high
    /// quadword of the source.
    Movhlps,
    /// `0F 16` with a register source (MOVLHPS xmm1, xmm2): the high quadword of the destination is the low
    /// quadword of the source.
    Movlhps,
    /// `F3 0F 6F` (MOVDQU) and `66 0F 6F` (MOVDQA) xmm, xmm; the memory forms are the S2-34 raw moves.
    MovdqRegisterLoad,
    /// `F3 0F 7F` (MOVDQU) and `66 0F 7F` (MOVDQA) xmm, xmm with the destination in ModRM.r/m.
    MovdqRegisterStore,
    /// `0F 2B` MOVNTPS, `66 0F 2B` MOVNTPD and `66 0F E7` MOVNTDQ m128, xmm: the whole source register is stored to a
    /// 16-byte aligned memory operand (the non-temporal hint has no effect in this model). Memory form only.
    Movnt128,
    /// `66 0F F7` MASKMOVDQU xmm1, xmm2 (register form only; decoded as [`crate::long_decode::Operation::Sse2Maskmov`]):
    /// the bytes of xmm1 whose mask byte in xmm2 has its most significant bit set are stored at DS:DI/EDI/RDI.
    Maskmovdqu,
    /// `F2 0F D6` MOVDQ2Q mm, xmm (register form only; decoded as [`crate::long_decode::Operation::Sse2MmxMove`]).
    Movdq2q,
    /// `F3 0F D6` MOVQ2DQ xmm, mm (register form only; decoded as [`crate::long_decode::Operation::Sse2MmxMove`]).
    Movq2dq,
}

/// The mandatory prefix of an encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Sse2Prefix {
    None,
    P66,
    F2,
    F3,
}

impl Sse2IntKind {
    /// The kind of `[prefix] 0F opcode /r` (`register_form`: ModRM.mod = 3; `rex_w`: REX.W) or `None` for an
    /// encoding this module does not carry (including the memory form of an instruction that has only a register
    /// form here and the other way round).
    pub const fn decode(
        prefix: Sse2Prefix,
        opcode: u8,
        register_form: bool,
        rex_w: bool,
    ) -> Option<Self> {
        use Sse2Prefix::{F2, F3, None as NoPrefix, P66};
        Some(match (prefix, opcode) {
            (P66, 0x60) => Self::Punpcklbw,
            (P66, 0x61) => Self::Punpcklwd,
            (P66, 0x62) => Self::Punpckldq,
            (P66, 0x63) => Self::Packsswb,
            (P66, 0x64) => Self::Pcmpgtb,
            (P66, 0x65) => Self::Pcmpgtw,
            (P66, 0x66) => Self::Pcmpgtd,
            (P66, 0x67) => Self::Packuswb,
            (P66, 0x68) => Self::Punpckhbw,
            (P66, 0x69) => Self::Punpckhwd,
            (P66, 0x6a) => Self::Punpckhdq,
            (P66, 0x6b) => Self::Packssdw,
            (P66, 0x6c) => Self::Punpcklqdq,
            (P66, 0x6d) => Self::Punpckhqdq,
            (P66, 0x6e) => {
                if rex_w {
                    Self::MovqFromGeneral
                } else {
                    Self::MovdLoad
                }
            }
            (P66 | F3, 0x6f) if register_form => Self::MovdqRegisterLoad,
            (P66, 0x70) => Self::Pshufd,
            (F3, 0x70) => Self::Pshufhw,
            (F2, 0x70) => Self::Pshuflw,
            (P66, 0x74) => Self::Pcmpeqb,
            (P66, 0x75) => Self::Pcmpeqw,
            (P66, 0x76) => Self::Pcmpeqd,
            (P66, 0x7e) => {
                if rex_w {
                    Self::MovqToGeneral
                } else {
                    Self::MovdStore
                }
            }
            (F3, 0x7e) => Self::MovqLoad,
            (P66 | F3, 0x7f) if register_form => Self::MovdqRegisterStore,
            (P66, 0xc4) => Self::Pinsrw,
            (P66, 0xc5) if register_form => Self::Pextrw,
            (P66, 0xd1) => Self::Psrlw,
            (P66, 0xd2) => Self::Psrld,
            (P66, 0xd3) => Self::Psrlq,
            (P66, 0xd4) => Self::Paddq,
            (P66, 0xd5) => Self::Pmullw,
            (P66, 0xd6) => Self::MovqStore,
            (F2, 0xd6) if register_form => Self::Movdq2q,
            (F3, 0xd6) if register_form => Self::Movq2dq,
            (P66, 0xf7) if register_form => Self::Maskmovdqu,
            (NoPrefix | P66, 0x2b) | (P66, 0xe7) if !register_form => Self::Movnt128,
            (P66, 0xd8) => Self::Psubusb,
            (P66, 0xd9) => Self::Psubusw,
            (P66, 0xda) => Self::Pminub,
            (P66, 0xdb) => Self::Pand,
            (P66, 0xdc) => Self::Paddusb,
            (P66, 0xdd) => Self::Paddusw,
            (P66, 0xde) => Self::Pmaxub,
            (P66, 0xdf) => Self::Pandn,
            (P66, 0xe0) => Self::Pavgb,
            (P66, 0xe1) => Self::Psraw,
            (P66, 0xe2) => Self::Psrad,
            (P66, 0xe3) => Self::Pavgw,
            (P66, 0xe4) => Self::Pmulhuw,
            (P66, 0xe5) => Self::Pmulhw,
            (P66, 0xe8) => Self::Psubsb,
            (P66, 0xe9) => Self::Psubsw,
            (P66, 0xea) => Self::Pminsw,
            (P66, 0xeb) => Self::Por,
            (P66, 0xec) => Self::Paddsb,
            (P66, 0xed) => Self::Paddsw,
            (P66, 0xee) => Self::Pmaxsw,
            (P66, 0xef) => Self::Pxor,
            (P66, 0xf1) => Self::Psllw,
            (P66, 0xf2) => Self::Pslld,
            (P66, 0xf3) => Self::Psllq,
            (P66, 0xf4) => Self::Pmuludq,
            (P66, 0xf5) => Self::Pmaddwd,
            (P66, 0xf6) => Self::Psadbw,
            (P66, 0xf8) => Self::Psubb,
            (P66, 0xf9) => Self::Psubw,
            (P66, 0xfa) => Self::Psubd,
            (P66, 0xfb) => Self::Psubq,
            (P66, 0xfc) => Self::Paddb,
            (P66, 0xfd) => Self::Paddw,
            (P66, 0xfe) => Self::Paddd,
            // MOVLPS/MOVLPD/MOVHPS/MOVHPD: the memory forms carry the load (0F 12, 0F 16) or the store (0F 13,
            // 0F 17); the register forms of 0F 12 and 0F 16 without a prefix are MOVHLPS and MOVLHPS.
            (NoPrefix | P66, 0x12) if !register_form => Self::MovlLoad,
            (NoPrefix | P66, 0x16) if !register_form => Self::MovhLoad,
            (NoPrefix | P66, 0x13) if !register_form => Self::MovlStore,
            (NoPrefix | P66, 0x17) if !register_form => Self::MovhStore,
            (NoPrefix, 0x12) => Self::Movhlps,
            (NoPrefix, 0x16) => Self::Movlhps,
            _ => return None,
        })
    }

    /// Whether this module owns `[prefix] 0F opcode` in any of its forms, so that a decoder reads the ModRM byte only for
    /// an opcode that it will decode.
    pub const fn owns(prefix: Sse2Prefix, opcode: u8) -> bool {
        Self::decode(prefix, opcode, true, false).is_some()
            || Self::decode(prefix, opcode, false, false).is_some()
            || Self::undefined_form(prefix, opcode, true)
            || Self::undefined_form(prefix, opcode, false)
    }

    /// Whether the encoding is one of this module's opcodes in a form the SDM does not define, which is `#UD` on
    /// the physical oracle: the memory form of PEXTRW (`66 0F C5`), the register form of the MOVLPS/MOVLPD/MOVHPS/MOVHPD
    /// stores (`0F 13`, `0F 17`, with or without `66`) and of the `66` loads (`66 0F 12`, `66 0F 16`), the register
    /// form of the non-temporal stores (`0F 2B`, `66 0F 2B`, `66 0F E7`) and the memory form of MASKMOVDQU (`66 0F F7`),
    /// MOVDQ2Q and MOVQ2DQ (`F2/F3 0F D6`).
    pub const fn undefined_form(prefix: Sse2Prefix, opcode: u8, register_form: bool) -> bool {
        use Sse2Prefix::{F2, F3, None as NoPrefix, P66};
        match (prefix, opcode) {
            (P66, 0xc5) => !register_form,
            (NoPrefix | P66, 0x13 | 0x17 | 0x2b) => register_form,
            (P66, 0x12 | 0x16 | 0xe7) => register_form,
            (P66, 0xf7) | (F2 | F3, 0xd6) => !register_form,
            _ => false,
        }
    }

    /// Whether an imm8 follows the ModRM operand.
    pub const fn has_immediate(self) -> bool {
        matches!(
            self,
            Self::Pshufd | Self::Pshufhw | Self::Pshuflw | Self::Pinsrw | Self::Pextrw
        )
    }

    /// Whether a memory operand has to be 16-byte aligned (`#GP(0)` otherwise): exception type 4 for the packed forms,
    /// type 1 for MOVNTDQ/MOVNTPS/MOVNTPD.
    pub const fn requires_alignment(self) -> bool {
        self.is_packed() || matches!(self, Self::Movnt128)
    }

    /// The two-operand forms of 128-bit data: a source of an XMM register or 16 bytes of memory.
    pub const fn is_packed(self) -> bool {
        !matches!(
            self,
            Self::MovqLoad
                | Self::MovqStore
                | Self::MovdLoad
                | Self::MovqFromGeneral
                | Self::MovdStore
                | Self::MovqToGeneral
                | Self::Pinsrw
                | Self::Pextrw
                | Self::MovlLoad
                | Self::MovhLoad
                | Self::MovlStore
                | Self::MovhStore
                | Self::Movhlps
                | Self::Movlhps
                | Self::MovdqRegisterLoad
                | Self::MovdqRegisterStore
                | Self::Movnt128
                | Self::Maskmovdqu
                | Self::Movdq2q
                | Self::Movq2dq
        )
    }

    /// The bytes a memory operand of this form moves.
    pub const fn memory_bytes(self) -> usize {
        match self {
            Self::MovdLoad | Self::MovdStore => 4,
            Self::Pinsrw => 2,
            Self::MovqLoad
            | Self::MovqStore
            | Self::MovqFromGeneral
            | Self::MovqToGeneral
            | Self::MovlLoad
            | Self::MovhLoad
            | Self::MovlStore
            | Self::MovhStore => 8,
            _ => 16,
        }
    }

    /// Whether the ModRM.r/m operand is written and the ModRM.reg XMM register is the source.
    pub const fn stores_to_rm(self) -> bool {
        matches!(
            self,
            Self::MovqStore
                | Self::MovdStore
                | Self::MovqToGeneral
                | Self::MovlStore
                | Self::MovhStore
                | Self::MovdqRegisterStore
                | Self::Movnt128
        )
    }

    /// Whether the ModRM.r/m operand is a general-purpose register in the register form.
    pub const fn rm_is_general(self) -> bool {
        matches!(
            self,
            Self::MovdLoad
                | Self::MovqFromGeneral
                | Self::MovdStore
                | Self::MovqToGeneral
                | Self::Pinsrw
        )
    }
}

type Xmm = [u8; 16];

fn word(value: &Xmm, lane: usize) -> u16 {
    u16::from_le_bytes([value[2 * lane], value[2 * lane + 1]])
}

fn dword(value: &Xmm, lane: usize) -> u32 {
    u32::from_le_bytes(value[4 * lane..4 * lane + 4].try_into().unwrap())
}

fn qword(value: &Xmm, lane: usize) -> u64 {
    u64::from_le_bytes(value[8 * lane..8 * lane + 8].try_into().unwrap())
}

fn from_words(f: impl Fn(usize) -> u16) -> Xmm {
    let mut out = [0; 16];
    for lane in 0..8 {
        out[2 * lane..2 * lane + 2].copy_from_slice(&f(lane).to_le_bytes());
    }
    out
}

fn from_dwords(f: impl Fn(usize) -> u32) -> Xmm {
    let mut out = [0; 16];
    for lane in 0..4 {
        out[4 * lane..4 * lane + 4].copy_from_slice(&f(lane).to_le_bytes());
    }
    out
}

fn from_qwords(f: impl Fn(usize) -> u64) -> Xmm {
    let mut out = [0; 16];
    for lane in 0..2 {
        out[8 * lane..8 * lane + 8].copy_from_slice(&f(lane).to_le_bytes());
    }
    out
}

fn bytes(dst: &Xmm, src: &Xmm, f: impl Fn(u8, u8) -> u8) -> Xmm {
    core::array::from_fn(|lane| f(dst[lane], src[lane]))
}

fn words(dst: &Xmm, src: &Xmm, f: impl Fn(u16, u16) -> u16) -> Xmm {
    from_words(|lane| f(word(dst, lane), word(src, lane)))
}

fn dwords(dst: &Xmm, src: &Xmm, f: impl Fn(u32, u32) -> u32) -> Xmm {
    from_dwords(|lane| f(dword(dst, lane), dword(src, lane)))
}

fn qwords(dst: &Xmm, src: &Xmm, f: impl Fn(u64, u64) -> u64) -> Xmm {
    from_qwords(|lane| f(qword(dst, lane), qword(src, lane)))
}

fn mask(condition: bool) -> u64 {
    if condition { u64::MAX } else { 0 }
}

/// The result of a form of `is_packed` on the destination `dst` (first operand) and the 128-bit source `src`
/// (second operand, an XMM register or memory); `immediate` is the imm8 of the shuffles. The shifts take the
/// count from the low quadword of `src`.
pub fn apply(kind: Sse2IntKind, dst: Xmm, src: Xmm, immediate: u8) -> Xmm {
    use Sse2IntKind::*;
    let (d, s) = (&dst, &src);
    let count = qword(s, 0);
    match kind {
        Paddb => bytes(d, s, u8::wrapping_add),
        Paddw => words(d, s, u16::wrapping_add),
        Paddd => dwords(d, s, u32::wrapping_add),
        Paddq => qwords(d, s, u64::wrapping_add),
        Psubb => bytes(d, s, u8::wrapping_sub),
        Psubw => words(d, s, u16::wrapping_sub),
        Psubd => dwords(d, s, u32::wrapping_sub),
        Psubq => qwords(d, s, u64::wrapping_sub),
        Paddsb => bytes(d, s, |a, b| (a as i8).saturating_add(b as i8) as u8),
        Paddsw => words(d, s, |a, b| (a as i16).saturating_add(b as i16) as u16),
        Paddusb => bytes(d, s, u8::saturating_add),
        Paddusw => words(d, s, u16::saturating_add),
        Psubsb => bytes(d, s, |a, b| (a as i8).saturating_sub(b as i8) as u8),
        Psubsw => words(d, s, |a, b| (a as i16).saturating_sub(b as i16) as u16),
        Psubusb => bytes(d, s, u8::saturating_sub),
        Psubusw => words(d, s, u16::saturating_sub),
        Pand => bytes(d, s, |a, b| a & b),
        Pandn => bytes(d, s, |a, b| !a & b),
        Por => bytes(d, s, |a, b| a | b),
        Pxor => bytes(d, s, |a, b| a ^ b),
        Pcmpeqb => bytes(d, s, |a, b| mask(a == b) as u8),
        Pcmpeqw => words(d, s, |a, b| mask(a == b) as u16),
        Pcmpeqd => dwords(d, s, |a, b| mask(a == b) as u32),
        Pcmpgtb => bytes(d, s, |a, b| mask(a as i8 > b as i8) as u8),
        Pcmpgtw => words(d, s, |a, b| mask(a as i16 > b as i16) as u16),
        Pcmpgtd => dwords(d, s, |a, b| mask(a as i32 > b as i32) as u32),
        Pmullw => words(d, s, |a, b| {
            (i32::from(a as i16) * i32::from(b as i16)) as u16
        }),
        Pmulhw => words(d, s, |a, b| {
            ((i32::from(a as i16) * i32::from(b as i16)) >> 16) as u16
        }),
        Pmulhuw => words(d, s, |a, b| ((u32::from(a) * u32::from(b)) >> 16) as u16),
        Pmaddwd => from_dwords(|lane| {
            let product =
                |index: usize| i32::from(word(d, index) as i16) * i32::from(word(s, index) as i16);
            product(2 * lane).wrapping_add(product(2 * lane + 1)) as u32
        }),
        Pmuludq => {
            from_qwords(|lane| u64::from(dword(d, 2 * lane)) * u64::from(dword(s, 2 * lane)))
        }
        Psadbw => from_qwords(|lane| {
            (0..8)
                .map(|byte| u64::from(d[8 * lane + byte].abs_diff(s[8 * lane + byte])))
                .sum()
        }),
        Pavgb => bytes(d, s, |a, b| ((u16::from(a) + u16::from(b) + 1) >> 1) as u8),
        Pavgw => words(d, s, |a, b| ((u32::from(a) + u32::from(b) + 1) >> 1) as u16),
        Pmaxub => bytes(d, s, u8::max),
        Pminub => bytes(d, s, u8::min),
        Pmaxsw => words(d, s, |a, b| (a as i16).max(b as i16) as u16),
        Pminsw => words(d, s, |a, b| (a as i16).min(b as i16) as u16),
        Punpcklbw | Punpckhbw => {
            let base = if kind == Punpcklbw { 0 } else { 8 };
            core::array::from_fn(|lane| {
                let source = if lane & 1 == 0 { d } else { s };
                source[base + lane / 2]
            })
        }
        Punpcklwd | Punpckhwd => {
            let base = if kind == Punpcklwd { 0 } else { 4 };
            from_words(|lane| word(if lane & 1 == 0 { d } else { s }, base + lane / 2))
        }
        Punpckldq | Punpckhdq => {
            let base = if kind == Punpckldq { 0 } else { 2 };
            from_dwords(|lane| dword(if lane & 1 == 0 { d } else { s }, base + lane / 2))
        }
        Punpcklqdq => from_qwords(|lane| qword(if lane == 0 { d } else { s }, 0)),
        Punpckhqdq => from_qwords(|lane| qword(if lane == 0 { d } else { s }, 1)),
        Packsswb => core::array::from_fn(|lane| {
            let value = (if lane < 8 {
                word(d, lane)
            } else {
                word(s, lane - 8)
            }) as i16;
            value.clamp(i16::from(i8::MIN), i16::from(i8::MAX)) as u8
        }),
        Packuswb => core::array::from_fn(|lane| {
            let value = (if lane < 8 {
                word(d, lane)
            } else {
                word(s, lane - 8)
            }) as i16;
            value.clamp(0, 255) as u8
        }),
        Packssdw => from_words(|lane| {
            let value = (if lane < 4 {
                dword(d, lane)
            } else {
                dword(s, lane - 4)
            }) as i32;
            value.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as u16
        }),
        Psllw => from_words(|lane| {
            if count > 15 {
                0
            } else {
                word(d, lane) << count
            }
        }),
        Pslld => from_dwords(|lane| {
            if count > 31 {
                0
            } else {
                dword(d, lane) << count
            }
        }),
        Psllq => from_qwords(|lane| {
            if count > 63 {
                0
            } else {
                qword(d, lane) << count
            }
        }),
        Psrlw => from_words(|lane| {
            if count > 15 {
                0
            } else {
                word(d, lane) >> count
            }
        }),
        Psrld => from_dwords(|lane| {
            if count > 31 {
                0
            } else {
                dword(d, lane) >> count
            }
        }),
        Psrlq => from_qwords(|lane| {
            if count > 63 {
                0
            } else {
                qword(d, lane) >> count
            }
        }),
        Psraw => from_words(|lane| (word(d, lane) as i16 >> count.min(15)) as u16),
        Psrad => from_dwords(|lane| (dword(d, lane) as i32 >> count.min(31)) as u32),
        Pshufd => from_dwords(|lane| dword(s, usize::from(immediate >> (2 * lane)) & 3)),
        Pshuflw => from_words(|lane| {
            if lane < 4 {
                word(s, usize::from(immediate >> (2 * lane)) & 3)
            } else {
                word(s, lane)
            }
        }),
        Pshufhw => from_words(|lane| {
            if lane >= 4 {
                word(s, 4 + (usize::from(immediate >> (2 * (lane - 4))) & 3))
            } else {
                word(s, lane)
            }
        }),
        _ => unreachable!("only the packed forms have a lane function"),
    }
}
