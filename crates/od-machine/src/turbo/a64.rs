//! Experiment (turbo loop): a minimal AArch64 encoder for the turbo JIT. Raw instruction words
//! only; labels and fixups live in [`Asm`].

#![allow(dead_code)]

pub const XZR: u32 = 31;
pub const SP: u32 = 31;

/// AArch64 condition codes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Cond {
    Eq = 0,
    Ne = 1,
    Hs = 2,
    Lo = 3,
    Mi = 4,
    Pl = 5,
    Vs = 6,
    Vc = 7,
    Hi = 8,
    Ls = 9,
    Ge = 10,
    Lt = 11,
    Gt = 12,
    Le = 13,
    Al = 14,
}

impl Cond {
    pub fn invert(self) -> Cond {
        // SAFETY-free: flip the low bit (AL has no inverse and is never inverted here).
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Hs => Cond::Lo,
            Cond::Lo => Cond::Hs,
            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,
            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,
            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,
            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,
            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,
            Cond::Al => Cond::Al,
        }
    }
}

#[inline]
fn sf(x64: bool) -> u32 {
    if x64 { 1 << 31 } else { 0 }
}

pub fn movz(x64: bool, rd: u32, imm16: u32, hw: u32) -> u32 {
    0x5280_0000 | sf(x64) | (hw << 21) | ((imm16 & 0xffff) << 5) | rd
}
pub fn movk(x64: bool, rd: u32, imm16: u32, hw: u32) -> u32 {
    0x7280_0000 | sf(x64) | (hw << 21) | ((imm16 & 0xffff) << 5) | rd
}
pub fn movn(x64: bool, rd: u32, imm16: u32, hw: u32) -> u32 {
    0x1280_0000 | sf(x64) | (hw << 21) | ((imm16 & 0xffff) << 5) | rd
}

/// ADD/ADDS/SUB/SUBS (immediate, 12 bits, optional LSL 12).
pub fn add_imm(x64: bool, rd: u32, rn: u32, imm12: u32, shift12: bool) -> u32 {
    0x1100_0000 | sf(x64) | ((shift12 as u32) << 22) | ((imm12 & 0xfff) << 10) | (rn << 5) | rd
}
pub fn adds_imm(x64: bool, rd: u32, rn: u32, imm12: u32, shift12: bool) -> u32 {
    0x3100_0000 | sf(x64) | ((shift12 as u32) << 22) | ((imm12 & 0xfff) << 10) | (rn << 5) | rd
}
pub fn sub_imm(x64: bool, rd: u32, rn: u32, imm12: u32, shift12: bool) -> u32 {
    0x5100_0000 | sf(x64) | ((shift12 as u32) << 22) | ((imm12 & 0xfff) << 10) | (rn << 5) | rd
}
pub fn subs_imm(x64: bool, rd: u32, rn: u32, imm12: u32, shift12: bool) -> u32 {
    0x7100_0000 | sf(x64) | ((shift12 as u32) << 22) | ((imm12 & 0xfff) << 10) | (rn << 5) | rd
}

/// Shift kinds of the shifted-register forms.
pub const LSL: u32 = 0;
pub const LSR: u32 = 1;
pub const ASR: u32 = 2;

pub fn add_reg(x64: bool, rd: u32, rn: u32, rm: u32, shift: u32, amount: u32) -> u32 {
    0x0B00_0000 | sf(x64) | (shift << 22) | (rm << 16) | (amount << 10) | (rn << 5) | rd
}
pub fn adds_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x2B00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn sub_reg(x64: bool, rd: u32, rn: u32, rm: u32, shift: u32, amount: u32) -> u32 {
    0x4B00_0000 | sf(x64) | (shift << 22) | (rm << 16) | (amount << 10) | (rn << 5) | rd
}
pub fn subs_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x6B00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn adcs(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x3A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn sbcs(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x7A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn adc(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn sbc(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x5A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}

pub fn and_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x0A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn ands_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x6A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn orr_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x2A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn orr_reg_shift(x64: bool, rd: u32, rn: u32, rm: u32, shift: u32, amount: u32) -> u32 {
    0x2A00_0000 | sf(x64) | (shift << 22) | (rm << 16) | (amount << 10) | (rn << 5) | rd
}
pub fn eor_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x4A00_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn eor_reg_shift(x64: bool, rd: u32, rn: u32, rm: u32, shift: u32, amount: u32) -> u32 {
    0x4A00_0000 | sf(x64) | (shift << 22) | (rm << 16) | (amount << 10) | (rn << 5) | rd
}
pub fn bic_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x0A20_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn orn_reg(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x2A20_0000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn mov_reg(x64: bool, rd: u32, rm: u32) -> u32 {
    orr_reg(x64, rd, XZR, rm)
}
pub fn mvn(x64: bool, rd: u32, rm: u32) -> u32 {
    orn_reg(x64, rd, XZR, rm)
}
pub fn tst_reg(x64: bool, rn: u32, rm: u32) -> u32 {
    ands_reg(x64, XZR, rn, rm)
}
/// MOV to/from SP is ADD #0.
pub fn mov_sp(rd: u32, rn: u32) -> u32 {
    add_imm(true, rd, rn, 0, false)
}

/// Logical immediates (AND/ORR/EOR/ANDS): `None` when `value` is not a bitmask immediate.
pub fn encode_bitmask(value: u64, x64: bool) -> Option<(u32, u32, u32)> {
    let value = if x64 {
        value
    } else {
        let low = value & 0xffff_ffff;
        low | (low << 32)
    };
    if value == 0 || value == u64::MAX {
        return None;
    }
    // Find the smallest element size that repeats.
    let mut size = 64u32;
    while size > 2 {
        let half = size / 2;
        let mask = (1u64 << half) - 1;
        if (value & mask) != ((value >> half) & mask) {
            break;
        }
        size = half;
    }
    let mask = if size == 64 { u64::MAX } else { (1u64 << size) - 1 };
    let element = value & mask;
    // Rotate so that the element is a run of ones starting at bit 0.
    let ones = element.count_ones();
    if ones == 0 || ones == size {
        return None;
    }
    // Find rotation r such that rotr(element, r) == (1<<ones)-1 within `size` bits.
    let target = (1u64 << ones) - 1;
    let mut rotation = None;
    for r in 0..size {
        let rotated = if r == 0 {
            element
        } else {
            ((element >> r) | (element << (size - r))) & mask
        };
        if rotated == target {
            rotation = Some(r);
            break;
        }
    }
    let r = rotation?;
    // immr is the right-rotate amount that maps the run back: element = rotr(target, immr).
    let immr = (size - r) % size;
    let n = (size == 64) as u32;
    let imms = match size {
        64 => ones - 1,
        32 => ones - 1,
        16 => 0b10_0000 | (ones - 1),
        8 => 0b11_0000 | (ones - 1),
        4 => 0b11_1000 | (ones - 1),
        2 => 0b11_1100 | (ones - 1),
        _ => return None,
    };
    if !x64 && n == 1 {
        return None;
    }
    Some((n, immr, imms))
}

fn logical_imm(base: u32, x64: bool, rd: u32, rn: u32, value: u64) -> Option<u32> {
    let (n, immr, imms) = encode_bitmask(value, x64)?;
    Some(base | sf(x64) | (n << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd)
}
pub fn and_imm(x64: bool, rd: u32, rn: u32, value: u64) -> Option<u32> {
    logical_imm(0x1200_0000, x64, rd, rn, value)
}
pub fn orr_imm(x64: bool, rd: u32, rn: u32, value: u64) -> Option<u32> {
    logical_imm(0x3200_0000, x64, rd, rn, value)
}
pub fn eor_imm(x64: bool, rd: u32, rn: u32, value: u64) -> Option<u32> {
    logical_imm(0x5200_0000, x64, rd, rn, value)
}
pub fn ands_imm(x64: bool, rd: u32, rn: u32, value: u64) -> Option<u32> {
    logical_imm(0x7200_0000, x64, rd, rn, value)
}

pub fn ubfm(x64: bool, rd: u32, rn: u32, immr: u32, imms: u32) -> u32 {
    0x5300_0000 | sf(x64) | ((x64 as u32) << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
pub fn sbfm(x64: bool, rd: u32, rn: u32, immr: u32, imms: u32) -> u32 {
    0x1300_0000 | sf(x64) | ((x64 as u32) << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
pub fn bfm(x64: bool, rd: u32, rn: u32, immr: u32, imms: u32) -> u32 {
    0x3300_0000 | sf(x64) | ((x64 as u32) << 22) | (immr << 16) | (imms << 10) | (rn << 5) | rd
}
pub fn lsl_imm(x64: bool, rd: u32, rn: u32, shift: u32) -> u32 {
    let width = if x64 { 64 } else { 32 };
    ubfm(x64, rd, rn, (width - shift) % width, width - 1 - shift)
}
pub fn lsr_imm(x64: bool, rd: u32, rn: u32, shift: u32) -> u32 {
    let width = if x64 { 64 } else { 32 };
    ubfm(x64, rd, rn, shift, width - 1)
}
pub fn asr_imm(x64: bool, rd: u32, rn: u32, shift: u32) -> u32 {
    let width = if x64 { 64 } else { 32 };
    sbfm(x64, rd, rn, shift, width - 1)
}
pub fn ror_imm(x64: bool, rd: u32, rn: u32, shift: u32) -> u32 {
    // EXTR Rd, Rn, Rn, #shift
    0x1380_0000 | sf(x64) | ((x64 as u32) << 22) | (rn << 16) | (shift << 10) | (rn << 5) | rd
}
pub fn ubfx(x64: bool, rd: u32, rn: u32, lsb: u32, width: u32) -> u32 {
    ubfm(x64, rd, rn, lsb, lsb + width - 1)
}
pub fn sbfx(x64: bool, rd: u32, rn: u32, lsb: u32, width: u32) -> u32 {
    sbfm(x64, rd, rn, lsb, lsb + width - 1)
}
pub fn bfi(x64: bool, rd: u32, rn: u32, lsb: u32, width: u32) -> u32 {
    let size = if x64 { 64 } else { 32 };
    bfm(x64, rd, rn, (size - lsb) % size, width - 1)
}
pub fn bfxil(x64: bool, rd: u32, rn: u32, lsb: u32, width: u32) -> u32 {
    bfm(x64, rd, rn, lsb, lsb + width - 1)
}
pub fn sxtb(rd: u32, rn: u32) -> u32 {
    sbfm(true, rd, rn, 0, 7)
}
pub fn sxth(rd: u32, rn: u32) -> u32 {
    sbfm(true, rd, rn, 0, 15)
}
pub fn sxtw(rd: u32, rn: u32) -> u32 {
    sbfm(true, rd, rn, 0, 31)
}
pub fn uxtw(rd: u32, rn: u32) -> u32 {
    mov_reg(false, rd, rn)
}

pub fn lslv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_2000 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn lsrv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_2400 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn asrv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_2800 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn rorv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_2C00 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn madd(x64: bool, rd: u32, rn: u32, rm: u32, ra: u32) -> u32 {
    0x1B00_0000 | sf(x64) | (rm << 16) | (ra << 10) | (rn << 5) | rd
}
pub fn mul(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    madd(x64, rd, rn, rm, XZR)
}
pub fn smulh(rd: u32, rn: u32, rm: u32) -> u32 {
    0x9B40_7C00 | (rm << 16) | (rn << 5) | rd
}
pub fn umulh(rd: u32, rn: u32, rm: u32) -> u32 {
    0x9BC0_7C00 | (rm << 16) | (rn << 5) | rd
}
pub fn smull(rd: u32, rn: u32, rm: u32) -> u32 {
    // SMADDL Xd, Wn, Wm, XZR
    0x9B20_0000 | (rm << 16) | (XZR << 10) | (rn << 5) | rd
}
pub fn udiv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_0800 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn sdiv(x64: bool, rd: u32, rn: u32, rm: u32) -> u32 {
    0x1AC0_0C00 | sf(x64) | (rm << 16) | (rn << 5) | rd
}
pub fn clz(x64: bool, rd: u32, rn: u32) -> u32 {
    0x5AC0_1000 | sf(x64) | (rn << 5) | rd
}
pub fn rbit(x64: bool, rd: u32, rn: u32) -> u32 {
    0x5AC0_0000 | sf(x64) | (rn << 5) | rd
}
pub fn rev(x64: bool, rd: u32, rn: u32) -> u32 {
    if x64 {
        0xDAC0_0C00 | (rn << 5) | rd
    } else {
        0x5AC0_0800 | (rn << 5) | rd
    }
}
pub fn rev16(rd: u32, rn: u32) -> u32 {
    0x5AC0_0400 | (rn << 5) | rd
}

/// Load/store with an unsigned immediate offset scaled by the access size (`size` in bytes).
pub fn ldr_imm(size: u32, rt: u32, rn: u32, offset: u32) -> u32 {
    debug_assert!(offset % size == 0 && offset / size < 4096, "ldr offset {offset}");
    let (base, scale) = match size {
        1 => (0x3940_0000, 0),
        2 => (0x7940_0000, 1),
        4 => (0xB940_0000, 2),
        _ => (0xF940_0000, 3),
    };
    base | ((offset >> scale) << 10) | (rn << 5) | rt
}
pub fn str_imm(size: u32, rt: u32, rn: u32, offset: u32) -> u32 {
    debug_assert!(offset % size == 0 && offset / size < 4096, "str offset {offset}");
    let (base, scale) = match size {
        1 => (0x3900_0000, 0),
        2 => (0x7900_0000, 1),
        4 => (0xB900_0000, 2),
        _ => (0xF900_0000, 3),
    };
    base | ((offset >> scale) << 10) | (rn << 5) | rt
}
/// Sign-extending loads into an X register.
pub fn ldrs_imm(size: u32, rt: u32, rn: u32, offset: u32) -> u32 {
    let (base, scale) = match size {
        1 => (0x3980_0000, 0),
        2 => (0x7980_0000, 1),
        _ => (0xB980_0000, 2),
    };
    base | ((offset >> scale) << 10) | (rn << 5) | rt
}
/// Load/store with a register offset (no extension, no scale).
pub fn ldr_reg(size: u32, rt: u32, rn: u32, rm: u32) -> u32 {
    let base = match size {
        1 => 0x3860_6800,
        2 => 0x7860_6800,
        4 => 0xB860_6800,
        _ => 0xF860_6800,
    };
    base | (rm << 16) | (rn << 5) | rt
}
pub fn str_reg(size: u32, rt: u32, rn: u32, rm: u32) -> u32 {
    let base = match size {
        1 => 0x3820_6800,
        2 => 0x7820_6800,
        4 => 0xB820_6800,
        _ => 0xF820_6800,
    };
    base | (rm << 16) | (rn << 5) | rt
}
pub fn ldp(rt: u32, rt2: u32, rn: u32, offset: i32) -> u32 {
    0xA940_0000 | ((((offset / 8) as u32) & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
pub fn stp(rt: u32, rt2: u32, rn: u32, offset: i32) -> u32 {
    0xA900_0000 | ((((offset / 8) as u32) & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
pub fn stp_pre(rt: u32, rt2: u32, rn: u32, offset: i32) -> u32 {
    0xA980_0000 | ((((offset / 8) as u32) & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}
pub fn ldp_post(rt: u32, rt2: u32, rn: u32, offset: i32) -> u32 {
    0xA8C0_0000 | ((((offset / 8) as u32) & 0x7f) << 15) | (rt2 << 10) | (rn << 5) | rt
}

pub fn b(offset_words: i32) -> u32 {
    0x1400_0000 | ((offset_words as u32) & 0x03ff_ffff)
}
pub fn bl(offset_words: i32) -> u32 {
    0x9400_0000 | ((offset_words as u32) & 0x03ff_ffff)
}
pub fn b_cond(cond: Cond, offset_words: i32) -> u32 {
    0x5400_0000 | (((offset_words as u32) & 0x7ffff) << 5) | cond as u32
}
pub fn cbz(x64: bool, rt: u32, offset_words: i32) -> u32 {
    0x3400_0000 | sf(x64) | (((offset_words as u32) & 0x7ffff) << 5) | rt
}
pub fn cbnz(x64: bool, rt: u32, offset_words: i32) -> u32 {
    0x3500_0000 | sf(x64) | (((offset_words as u32) & 0x7ffff) << 5) | rt
}
pub fn tbz(rt: u32, bit: u32, offset_words: i32) -> u32 {
    0x3600_0000 | ((bit >> 5) << 31) | ((bit & 31) << 19) | (((offset_words as u32) & 0x3fff) << 5) | rt
}
pub fn tbnz(rt: u32, bit: u32, offset_words: i32) -> u32 {
    0x3700_0000 | ((bit >> 5) << 31) | ((bit & 31) << 19) | (((offset_words as u32) & 0x3fff) << 5) | rt
}
pub fn br(rn: u32) -> u32 {
    0xD61F_0000 | (rn << 5)
}
pub fn blr(rn: u32) -> u32 {
    0xD63F_0000 | (rn << 5)
}
pub fn ret() -> u32 {
    0xD65F_03C0
}
pub fn nop() -> u32 {
    0xD503_201F
}

pub fn csel(x64: bool, rd: u32, rn: u32, rm: u32, cond: Cond) -> u32 {
    0x1A80_0000 | sf(x64) | (rm << 16) | ((cond as u32) << 12) | (rn << 5) | rd
}
pub fn csinc(x64: bool, rd: u32, rn: u32, rm: u32, cond: Cond) -> u32 {
    0x1A80_0400 | sf(x64) | (rm << 16) | ((cond as u32) << 12) | (rn << 5) | rd
}
pub fn cset(x64: bool, rd: u32, cond: Cond) -> u32 {
    csinc(x64, rd, XZR, XZR, cond.invert())
}

pub fn mrs_nzcv(rt: u32) -> u32 {
    0xD53B_4200 | rt
}
pub fn msr_nzcv(rt: u32) -> u32 {
    0xD51B_4200 | rt
}
pub fn cfinv() -> u32 {
    0xD500_401F
}
/// RMIF Xn, #shift, #mask (FEAT_FlagM): rotate Xn right by `shift`, insert the low four bits
/// selected by `mask` (N=8, Z=4, C=2, V=1) into the flags.
pub fn rmif(rn: u32, shift: u32, mask: u32) -> u32 {
    0xBA00_0400 | (shift << 15) | (rn << 5) | mask
}
pub fn setf8(rn: u32) -> u32 {
    0x3A00_080D | (rn << 5)
}
pub fn setf16(rn: u32) -> u32 {
    0x3A00_480D | (rn << 5)
}
pub fn ccmp_imm(x64: bool, rn: u32, imm5: u32, nzcv: u32, cond: Cond) -> u32 {
    0x7A40_0800 | sf(x64) | (imm5 << 16) | ((cond as u32) << 12) | (rn << 5) | nzcv
}

/// A label inside one assembly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Label(pub usize);

#[derive(Clone, Copy, Debug)]
enum FixKind {
    B26,
    B19,
    B14,
}

/// An assembly buffer with local labels and branches to absolute addresses.
pub struct Asm {
    pub words: Vec<u32>,
    labels: Vec<Option<usize>>,
    fixups: Vec<(usize, Label, FixKind)>,
    /// (word index, absolute target address): resolved by [`Asm::finish`] (B/BL imm26).
    pub externals: Vec<(usize, usize)>,
}

impl Asm {
    pub fn new() -> Self {
        Self {
            words: Vec::with_capacity(256),
            labels: Vec::new(),
            fixups: Vec::new(),
            externals: Vec::new(),
        }
    }
    #[inline]
    pub fn emit(&mut self, word: u32) {
        self.words.push(word);
    }
    pub fn pos(&self) -> usize {
        self.words.len()
    }
    pub fn label(&mut self) -> Label {
        self.labels.push(None);
        Label(self.labels.len() - 1)
    }
    pub fn bind(&mut self, label: Label) {
        self.labels[label.0] = Some(self.words.len());
    }
    pub fn b_to(&mut self, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B26));
        self.emit(b(0));
    }
    pub fn b_cond_to(&mut self, cond: Cond, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B19));
        self.emit(b_cond(cond, 0));
    }
    pub fn cbz_to(&mut self, x64: bool, rt: u32, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B19));
        self.emit(cbz(x64, rt, 0));
    }
    pub fn cbnz_to(&mut self, x64: bool, rt: u32, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B19));
        self.emit(cbnz(x64, rt, 0));
    }
    pub fn tbz_to(&mut self, rt: u32, bit: u32, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B14));
        self.emit(tbz(rt, bit, 0));
    }
    pub fn tbnz_to(&mut self, rt: u32, bit: u32, label: Label) {
        self.fixups.push((self.words.len(), label, FixKind::B14));
        self.emit(tbnz(rt, bit, 0));
    }
    /// B to an absolute address (resolved at [`Asm::finish`]).
    pub fn b_abs(&mut self, target: usize) {
        self.externals.push((self.words.len(), target));
        self.emit(b(0));
    }

    /// Materialize a 64-bit constant into `rd` (X register).
    pub fn mov_imm64(&mut self, rd: u32, value: u64) {
        if value == 0 {
            self.emit(movz(true, rd, 0, 0));
            return;
        }
        let halves = [
            (value & 0xffff) as u32,
            ((value >> 16) & 0xffff) as u32,
            ((value >> 32) & 0xffff) as u32,
            ((value >> 48) & 0xffff) as u32,
        ];
        let zeros = halves.iter().filter(|&&h| h == 0).count();
        let ones = halves.iter().filter(|&&h| h == 0xffff).count();
        if ones > zeros {
            // MOVN base, then MOVK the halves that are not 0xffff.
            let mut first = true;
            for (i, &h) in halves.iter().enumerate() {
                if h != 0xffff {
                    if first {
                        self.emit(movn(true, rd, !h & 0xffff, i as u32));
                        first = false;
                    } else {
                        self.emit(movk(true, rd, h, i as u32));
                    }
                }
            }
            if first {
                self.emit(movn(true, rd, 0, 0));
            }
        } else {
            let mut first = true;
            for (i, &h) in halves.iter().enumerate() {
                if h != 0 {
                    if first {
                        self.emit(movz(true, rd, h, i as u32));
                        first = false;
                    } else {
                        self.emit(movk(true, rd, h, i as u32));
                    }
                }
            }
        }
    }

    /// Materialize a 32-bit constant into `rd` (W register, upper half zero).
    pub fn mov_imm32(&mut self, rd: u32, value: u32) {
        let low = value & 0xffff;
        let high = value >> 16;
        if high == 0 {
            self.emit(movz(false, rd, low, 0));
        } else if low == 0 {
            self.emit(movz(false, rd, high, 1));
        } else if high == 0xffff {
            self.emit(movn(false, rd, !low & 0xffff, 0));
        } else {
            self.emit(movz(false, rd, low, 0));
            self.emit(movk(false, rd, high, 1));
        }
    }

    /// rd = rn + imm (64-bit), any immediate; `scratch` is clobbered only for large values.
    pub fn add_any(&mut self, rd: u32, rn: u32, imm: i64, scratch: u32) {
        if imm == 0 {
            if rd != rn {
                self.emit(mov_reg(true, rd, rn));
            }
        } else if (0..4096).contains(&imm) {
            self.emit(add_imm(true, rd, rn, imm as u32, false));
        } else if (-4095..0).contains(&imm) {
            self.emit(sub_imm(true, rd, rn, (-imm) as u32, false));
        } else if imm > 0 && imm & 0xfff == 0 && imm < (1 << 24) {
            self.emit(add_imm(true, rd, rn, (imm >> 12) as u32, true));
        } else if imm < 0 && (-imm) & 0xfff == 0 && (-imm) < (1 << 24) {
            self.emit(sub_imm(true, rd, rn, ((-imm) >> 12) as u32, true));
        } else {
            self.mov_imm64(scratch, imm as u64);
            self.emit(add_reg(true, rd, rn, scratch, LSL, 0));
        }
    }

    /// Resolve local fixups and external branches for code placed at `base`.
    pub fn finish(&mut self, base: usize) -> Result<(), &'static str> {
        for &(at, label, kind) in &self.fixups {
            let target = self.labels[label.0].ok_or("unbound label")?;
            let delta = target as i64 - at as i64;
            let word = &mut self.words[at];
            match kind {
                FixKind::B26 => *word |= (delta as u32) & 0x03ff_ffff,
                FixKind::B19 => {
                    if !(-(1 << 18)..(1 << 18)).contains(&delta) {
                        return Err("b19 out of range");
                    }
                    *word |= ((delta as u32) & 0x7ffff) << 5;
                }
                FixKind::B14 => {
                    if !(-(1 << 13)..(1 << 13)).contains(&delta) {
                        return Err("b14 out of range");
                    }
                    *word |= ((delta as u32) & 0x3fff) << 5;
                }
            }
        }
        for &(at, target) in &self.externals {
            let here = base + at * 4;
            let delta = (target as i64 - here as i64) / 4;
            if !(-(1 << 25)..(1 << 25)).contains(&delta) {
                return Err("b26 out of range");
            }
            self.words[at] |= (delta as u32) & 0x03ff_ffff;
        }
        Ok(())
    }
}
