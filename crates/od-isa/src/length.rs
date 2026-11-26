//! S7-32: the length of one 64-bit-mode instruction from its bytes alone.
//!
//! The semantic decoder ([`crate::long_decode::decode`]) admits a growing set of forms and reports
//! no length for the forms it does not admit. Diagnostics that walk code (the decode coverage of a
//! PE image, `od-runner decode-coverage`) need to step over such a form, so this module computes the
//! length from the opcode tables of the Intel SDM (Vol. 2, Appendix A "Opcode Map", and Vol. 2 §2.1
//! "Instruction Format", §2.2.1 REX, §2.3 VEX, §2.7 EVEX) without deciding what the instruction does.
//!
//! It is not an admission rule and not used by execution: an instruction the decoder admits is
//! measured by the decoder. Only 64-bit mode (IA-32e, CS.L = 1) is modelled: `0x40..=0x4F` are REX
//! prefixes, `C4`/`C5` are always VEX and `62` is always EVEX, the default address size is 64 bits and
//! `67` selects 32-bit addressing (same ModRM format). Near branches with a `z` displacement are
//! always `rel32` (the `66` prefix does not shorten them in 64-bit mode on Intel processors).

/// Why no length could be given.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LengthError {
    /// The bytes end before the instruction does.
    Truncated,
    /// More than 15 bytes, or more than 14 prefix bytes.
    TooLong,
}

/// The architectural maximum instruction length.
pub const MAX_INSTRUCTION_BYTES: usize = 15;

/// What follows the opcode byte.
#[derive(Clone, Copy)]
enum Tail {
    /// Nothing.
    None,
    /// A ModRM byte (with SIB and displacement as it encodes) and `imm` immediate bytes.
    ModRm { imm: Imm },
    /// Only an immediate.
    Imm(Imm),
}

#[derive(Clone, Copy)]
enum Imm {
    None,
    Byte,
    Word,
    /// 2 bytes with the `66` prefix, else 4 (`Iz`, `Jz`-free forms).
    Z,
    /// `B8..=BF`: 8 bytes with REX.W, else `Z`.
    V,
    /// `C8` (ENTER): imm16 then imm8.
    Enter,
    /// `A0..=A3`: an address-size offset (8 bytes, 4 with `67`).
    Moffs,
    /// A 32-bit relative displacement (`Jz` in 64-bit mode).
    Rel32,
    /// `F6 /0../1`: imm8; `F7 /0../1`: `Z`. Resolved from ModRM.reg.
    Group3Byte,
    Group3Z,
    /// XOP map 0x0A: imm32.
    Dword,
}

/// The measured layout of one instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Long64Layout {
    /// The whole instruction, prefixes to the last immediate byte.
    pub length: usize,
    /// The offset of the ModRM byte, for forms that have one.
    pub modrm: Option<usize>,
}

/// The length of the 64-bit-mode instruction at the start of `bytes`.
pub fn long64_length(bytes: &[u8]) -> Result<usize, LengthError> {
    long64_layout(bytes).map(|layout| layout.length)
}

/// The length and the ModRM position of the 64-bit-mode instruction at the start of `bytes`.
pub fn long64_layout(bytes: &[u8]) -> Result<Long64Layout, LengthError> {
    let byte = |index: usize| bytes.get(index).copied().ok_or(LengthError::Truncated);
    let mut at = 0;
    let mut operand_override = false;
    let mut address_override = false;
    let mut rex_w = false;
    // Legacy prefixes in any order; a REX byte counts only directly before the opcode.
    loop {
        let b = byte(at)?;
        match b {
            0xf0 | 0xf2 | 0xf3 | 0x2e | 0x36 | 0x3e | 0x26 | 0x64 | 0x65 => {}
            0x66 => operand_override = true,
            0x67 => address_override = true,
            0x40..=0x4f => {
                let next = byte(at + 1)?;
                rex_w = b & 8 != 0;
                if !(0x40..=0x4f).contains(&next) && !is_legacy_prefix(next) {
                    at += 1;
                    break;
                }
                // A REX byte followed by another prefix is ignored (SDM §2.2.1).
                rex_w = false;
            }
            _ => break,
        }
        at += 1;
        if at >= MAX_INSTRUCTION_BYTES {
            return Err(LengthError::TooLong);
        }
    }
    let opcode = byte(at)?;
    at += 1;
    let tail = match opcode {
        // VEX (C5: 2-byte, C4: 3-byte), always in 64-bit mode.
        0xc5 | 0xc4 => {
            let (map, vex_bytes) = if opcode == 0xc5 {
                (1, 1)
            } else {
                (byte(at)? & 0x1f, 2)
            };
            at += vex_bytes;
            let op = byte(at)?;
            at += 1;
            vex_tail(map, op)
        }
        // EVEX.
        0x62 => {
            let map = byte(at)? & 0x07;
            at += 3;
            let op = byte(at)?;
            at += 1;
            vex_tail(map, op)
        }
        // XOP: 8F with map select >= 8; otherwise POP r/m (8F /0).
        0x8f if byte(at)? & 0x1f >= 8 => {
            let map = byte(at)? & 0x1f;
            at += 2;
            at += 1; // opcode
            Tail::ModRm {
                imm: match map {
                    8 => Imm::Byte,
                    0x0a => Imm::Dword,
                    _ => Imm::None,
                },
            }
        }
        0x0f => {
            let second = byte(at)?;
            at += 1;
            match second {
                0x38 => {
                    at += 1;
                    Tail::ModRm { imm: Imm::None }
                }
                0x3a => {
                    at += 1;
                    Tail::ModRm { imm: Imm::Byte }
                }
                // 3DNow!: ModRM, then the opcode suffix byte.
                0x0f => Tail::ModRm { imm: Imm::Byte },
                _ => escape0f_tail(second),
            }
        }
        _ => primary_tail(opcode),
    };
    let (modrm_imm, imm) = match tail {
        Tail::None => (false, Imm::None),
        Tail::Imm(imm) => (false, imm),
        Tail::ModRm { imm } => (true, imm),
    };
    let mut reg = 0;
    let modrm_at = modrm_imm.then_some(at);
    if modrm_imm {
        let modrm = byte(at)?;
        at += 1;
        reg = (modrm >> 3) & 7;
        let mode = modrm >> 6;
        let rm = modrm & 7;
        if mode != 3 {
            if rm == 4 {
                let sib = byte(at)?;
                at += 1;
                if mode == 0 && sib & 7 == 5 {
                    at += 4;
                }
            } else if mode == 0 && rm == 5 {
                at += 4;
            }
            at += match mode {
                1 => 1,
                2 => 4,
                _ => 0,
            };
        }
    }
    // `66` selects a 16-bit `Iz` only without REX.W, which takes precedence (SDM Vol. 2 §2.2.1.2).
    let z = if operand_override && !rex_w { 2 } else { 4 };
    at += match imm {
        Imm::None => 0,
        Imm::Byte => 1,
        Imm::Word => 2,
        Imm::Z => z,
        Imm::V => {
            if rex_w {
                8
            } else {
                z
            }
        }
        Imm::Enter => 3,
        Imm::Moffs => {
            if address_override {
                4
            } else {
                8
            }
        }
        Imm::Rel32 | Imm::Dword => 4,
        Imm::Group3Byte => usize::from(reg < 2),
        Imm::Group3Z => {
            if reg < 2 {
                z
            } else {
                0
            }
        }
    };
    if at > MAX_INSTRUCTION_BYTES {
        return Err(LengthError::TooLong);
    }
    if at > bytes.len() {
        return Err(LengthError::Truncated);
    }
    Ok(Long64Layout {
        length: at,
        modrm: modrm_at,
    })
}

fn is_legacy_prefix(byte: u8) -> bool {
    matches!(
        byte,
        0xf0 | 0xf2 | 0xf3 | 0x2e | 0x36 | 0x3e | 0x26 | 0x64 | 0x65 | 0x66 | 0x67
    )
}

/// The primary opcode map in 64-bit mode (SDM Vol. 2 Table A-2). Opcodes invalid in 64-bit mode
/// (`06`, `07`, `0E`, `16`, `17`, `1E`, `1F`, `27`, `2F`, `37`, `3F`, `60`, `61`, `82`, `9A`, `C4`/`C5`
/// as LES/LDS, `CE`, `D4`, `D5`, `D6`, `EA`) are measured as one opcode byte.
fn primary_tail(opcode: u8) -> Tail {
    match opcode {
        0x00..=0x3f => match opcode & 7 {
            0..=3 => Tail::ModRm { imm: Imm::None },
            4 => Tail::Imm(Imm::Byte),
            5 => Tail::Imm(Imm::Z),
            _ => Tail::None,
        },
        0x63 => Tail::ModRm { imm: Imm::None },
        0x68 => Tail::Imm(Imm::Z),
        0x69 => Tail::ModRm { imm: Imm::Z },
        0x6a => Tail::Imm(Imm::Byte),
        0x6b => Tail::ModRm { imm: Imm::Byte },
        0x70..=0x7f => Tail::Imm(Imm::Byte),
        0x80 | 0x83 => Tail::ModRm { imm: Imm::Byte },
        0x81 => Tail::ModRm { imm: Imm::Z },
        0x84..=0x8f => Tail::ModRm { imm: Imm::None },
        0xa0..=0xa3 => Tail::Imm(Imm::Moffs),
        0xa8 => Tail::Imm(Imm::Byte),
        0xa9 => Tail::Imm(Imm::Z),
        0xb0..=0xb7 => Tail::Imm(Imm::Byte),
        0xb8..=0xbf => Tail::Imm(Imm::V),
        0xc0 | 0xc1 => Tail::ModRm { imm: Imm::Byte },
        0xc2 | 0xca => Tail::Imm(Imm::Word),
        0xc6 => Tail::ModRm { imm: Imm::Byte },
        // C7 /0 MOV r/m, Iz and C7 F8 XBEGIN rel32 (= Iz without 66) share the shape.
        0xc7 => Tail::ModRm { imm: Imm::Z },
        0xc8 => Tail::Imm(Imm::Enter),
        0xcd => Tail::Imm(Imm::Byte),
        0xd0..=0xd3 | 0xd8..=0xdf => Tail::ModRm { imm: Imm::None },
        0xe0..=0xe7 | 0xeb => Tail::Imm(Imm::Byte),
        0xe8 | 0xe9 => Tail::Imm(Imm::Rel32),
        0xf6 => Tail::ModRm {
            imm: Imm::Group3Byte,
        },
        0xf7 => Tail::ModRm { imm: Imm::Group3Z },
        0xfe | 0xff => Tail::ModRm { imm: Imm::None },
        _ => Tail::None,
    }
}

/// The two-byte map `0F xx` (SDM Vol. 2 Table A-3).
fn escape0f_tail(second: u8) -> Tail {
    match second {
        0x00..=0x03 | 0x0d => Tail::ModRm { imm: Imm::None },
        0x10..=0x1f | 0x20..=0x23 | 0x28..=0x2f => Tail::ModRm { imm: Imm::None },
        0x40..=0x6f => Tail::ModRm { imm: Imm::None },
        0x70..=0x73 => Tail::ModRm { imm: Imm::Byte },
        0x74..=0x76 | 0x78..=0x7f => Tail::ModRm { imm: Imm::None },
        0x80..=0x8f => Tail::Imm(Imm::Rel32),
        0x90..=0x9f => Tail::ModRm { imm: Imm::None },
        0xa3 | 0xa5 | 0xab | 0xad..=0xaf => Tail::ModRm { imm: Imm::None },
        0xa4 | 0xac => Tail::ModRm { imm: Imm::Byte },
        0xb0..=0xb9 | 0xbb..=0xbf => Tail::ModRm { imm: Imm::None },
        0xba => Tail::ModRm { imm: Imm::Byte },
        0xc0 | 0xc1 | 0xc3 | 0xc7 => Tail::ModRm { imm: Imm::None },
        0xc2 | 0xc4..=0xc6 => Tail::ModRm { imm: Imm::Byte },
        0xd0..=0xff => Tail::ModRm { imm: Imm::None },
        _ => Tail::None,
    }
}

/// VEX and EVEX: every opcode has a ModRM byte except `VZEROUPPER`/`VZEROALL` (map 1, `77`); map 3
/// and the map-1 forms that take an imm8 in the legacy encoding (`70..=73`, `C2`, `C4..=C6`) have one.
fn vex_tail(map: u8, opcode: u8) -> Tail {
    match (map, opcode) {
        (1, 0x77) => Tail::None,
        (3, _) => Tail::ModRm { imm: Imm::Byte },
        (1, 0x70..=0x73 | 0xc2 | 0xc4..=0xc6) => Tail::ModRm { imm: Imm::Byte },
        _ => Tail::ModRm { imm: Imm::None },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn len(bytes: &[u8]) -> usize {
        long64_length(bytes).unwrap()
    }

    #[test]
    fn plain_forms_and_their_operands() {
        assert_eq!(len(&[0x90]), 1);
        assert_eq!(len(&[0xc3]), 1);
        // mov rax, imm64; mov eax, imm32; mov ax, imm16.
        assert_eq!(len(&[0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8]), 10);
        assert_eq!(len(&[0xb8, 1, 2, 3, 4]), 5);
        assert_eq!(len(&[0x66, 0xb8, 1, 2]), 4);
        // add rax, imm32 (sign-extended): 48 05 id.
        assert_eq!(len(&[0x48, 0x05, 1, 2, 3, 4]), 6);
        // call rel32, jmp rel8, jz rel32.
        assert_eq!(len(&[0xe8, 0, 0, 0, 0]), 5);
        assert_eq!(len(&[0xeb, 0]), 2);
        assert_eq!(len(&[0x0f, 0x84, 0, 0, 0, 0]), 6);
        // enter 0x10, 0; ret 8.
        assert_eq!(len(&[0xc8, 0x10, 0, 0]), 4);
        assert_eq!(len(&[0xc2, 8, 0]), 3);
        // mov al, [moffs64]; with 67: moffs32.
        assert_eq!(len(&[0xa0, 0, 0, 0, 0, 0, 0, 0, 0]), 9);
        assert_eq!(len(&[0x67, 0xa0, 0, 0, 0, 0]), 6);
    }

    #[test]
    fn modrm_sib_and_displacements() {
        // mov rax, [rcx]; mov rax, [rcx+8]; mov rax, [rcx+0x1000].
        assert_eq!(len(&[0x48, 0x8b, 0x01]), 3);
        assert_eq!(len(&[0x48, 0x8b, 0x41, 0x08]), 4);
        assert_eq!(len(&[0x48, 0x8b, 0x81, 0, 0x10, 0, 0]), 7);
        // mov rax, [rip+disp32]; mov rax, [rsp]; mov rax, [disp32] (SIB, no base).
        assert_eq!(len(&[0x48, 0x8b, 0x05, 0, 0, 0, 0]), 7);
        assert_eq!(len(&[0x48, 0x8b, 0x04, 0x24]), 4);
        assert_eq!(len(&[0x48, 0x8b, 0x04, 0x25, 0, 0, 0, 0]), 8);
        // mov qword [rsp+8], imm32: 48 C7 44 24 08 id.
        assert_eq!(len(&[0x48, 0xc7, 0x44, 0x24, 0x08, 1, 2, 3, 4]), 9);
        // test byte [rcx], 1 (F6 /0 ib); not byte [rcx] (F6 /2).
        assert_eq!(len(&[0xf6, 0x01, 0x01]), 3);
        assert_eq!(len(&[0xf6, 0x11]), 2);
        // test eax-form F7 /0 id; with 66: iw; neg (F7 /3) has none.
        assert_eq!(len(&[0xf7, 0xc0, 1, 2, 3, 4]), 6);
        assert_eq!(len(&[0x66, 0xf7, 0xc0, 1, 2]), 5);
        assert_eq!(len(&[0xf7, 0xd8]), 2);
    }

    #[test]
    fn rex_w_takes_precedence_over_66_for_iz() {
        // 66 48 81 C0 id: add rax, imm32 (REX.W wins, SDM Vol. 2 §2.2.1.2); 66 48 05 id; 66 48 B8 io.
        assert_eq!(len(&[0x66, 0x48, 0x81, 0xc0, 1, 0, 0, 0]), 8);
        assert_eq!(len(&[0x66, 0x48, 0x05, 1, 0, 0, 0]), 7);
        assert_eq!(len(&[0x66, 0x48, 0xb8, 1, 2, 3, 4, 5, 6, 7, 8]), 11);
        // Without REX.W the 66 prefix keeps the 16-bit immediate.
        assert_eq!(len(&[0x66, 0x81, 0xc0, 1, 0]), 5);
    }

    #[test]
    fn escape_maps() {
        // sidt [rcx+0xa]: 0F 01 49 0A.
        assert_eq!(len(&[0x0f, 0x01, 0x49, 0x0a]), 4);
        // movups xmm0, [rip+disp32]: 0F 10 05 d32.
        assert_eq!(len(&[0x0f, 0x10, 0x05, 0xf7, 0x20, 0xfd, 0xff]), 7);
        // pshufd xmm0, xmm1, 0x1b: 66 0F 70 C1 1B.
        assert_eq!(len(&[0x66, 0x0f, 0x70, 0xc1, 0x1b]), 5);
        // pshufb xmm0, xmm1: 66 0F 38 00 C1; palignr: 66 0F 3A 0F C1 08.
        assert_eq!(len(&[0x66, 0x0f, 0x38, 0x00, 0xc1]), 5);
        assert_eq!(len(&[0x66, 0x0f, 0x3a, 0x0f, 0xc1, 0x08]), 6);
        // rdmsr, cpuid, bt eax, 3 (0F BA /4 ib).
        assert_eq!(len(&[0x0f, 0x32]), 2);
        assert_eq!(len(&[0x0f, 0xa2]), 2);
        assert_eq!(len(&[0x0f, 0xba, 0xe0, 0x03]), 4);
        // lock cmpxchg16b [rcx]: F0 48 0F C7 09.
        assert_eq!(len(&[0xf0, 0x48, 0x0f, 0xc7, 0x09]), 5);
    }

    #[test]
    fn vex_and_evex() {
        // vzeroupper: C5 F8 77.
        assert_eq!(len(&[0xc5, 0xf8, 0x77]), 3);
        // vmovdqu ymm0, [rcx]: C5 FE 6F 01.
        assert_eq!(len(&[0xc5, 0xfe, 0x6f, 0x01]), 4);
        // vpshufd ymm0, ymm1, 0x1b: C5 FD 70 C1 1B; vpalignr (map 3): C4 E3 7D 0F C1 08.
        assert_eq!(len(&[0xc5, 0xfd, 0x70, 0xc1, 0x1b]), 5);
        assert_eq!(len(&[0xc4, 0xe3, 0x7d, 0x0f, 0xc1, 0x08]), 6);
        // vmovdqu64 zmm0, [rcx+0x40] (disp8*N): 62 F1 FE 48 6F 41 01.
        assert_eq!(len(&[0x62, 0xf1, 0xfe, 0x48, 0x6f, 0x41, 0x01]), 7);
    }

    #[test]
    fn prefixes_and_limits() {
        // A REX byte not directly before the opcode is ignored: 48 66 B8 iw (operand size 16).
        assert_eq!(len(&[0x48, 0x66, 0xb8, 1, 2]), 5);
        // lock add [rax], eax with segment override.
        assert_eq!(len(&[0x65, 0xf0, 0x01, 0x00]), 4);
        assert_eq!(long64_length(&[0x48, 0x8b]), Err(LengthError::Truncated));
        assert_eq!(long64_length(&[0x66; 16]), Err(LengthError::TooLong));
    }

    #[test]
    fn layout_names_the_modrm_byte() {
        let layout = |bytes: &[u8]| long64_layout(bytes).unwrap();
        assert_eq!(layout(&[0x48, 0x8b, 0x41, 0x08]).modrm, Some(2));
        assert_eq!(layout(&[0x0f, 0x01, 0x49, 0x0a]).modrm, Some(2));
        assert_eq!(layout(&[0x66, 0x0f, 0x3a, 0x0f, 0xc1, 0x08]).modrm, Some(4));
        assert_eq!(layout(&[0xe8, 0, 0, 0, 0]).modrm, None);
        assert_eq!(layout(&[0x0f, 0xa2]).modrm, None);
    }
}
