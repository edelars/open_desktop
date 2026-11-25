//! Raw AArch64 instruction words for the turbo JIT of the unified loop (`od-machine` `turbo`,
//! ADR-0022): free functions return one encoded word; [`Asm`] collects words, binds labels and
//! resolves local and absolute branches once the final address of the code is known.

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
    let mask = if size == 64 {
        u64::MAX
    } else {
        (1u64 << size) - 1
    };
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
/// EXTR Rd, Rn, Rm, #lsb: the low bits of (Rn:Rm) >> lsb.
pub fn extr(x64: bool, rd: u32, rn: u32, rm: u32, lsb: u32) -> u32 {
    0x1380_0000 | sf(x64) | ((x64 as u32) << 22) | (rm << 16) | (lsb << 10) | (rn << 5) | rd
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
    debug_assert!(
        offset % size == 0 && offset / size < 4096,
        "ldr offset {offset}"
    );
    let (base, scale) = match size {
        1 => (0x3940_0000, 0),
        2 => (0x7940_0000, 1),
        4 => (0xB940_0000, 2),
        _ => (0xF940_0000, 3),
    };
    base | ((offset >> scale) << 10) | (rn << 5) | rt
}
pub fn str_imm(size: u32, rt: u32, rn: u32, offset: u32) -> u32 {
    debug_assert!(
        offset % size == 0 && offset / size < 4096,
        "str offset {offset}"
    );
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
    0x3600_0000
        | ((bit >> 5) << 31)
        | ((bit & 31) << 19)
        | (((offset_words as u32) & 0x3fff) << 5)
        | rt
}
pub fn tbnz(rt: u32, bit: u32, offset_words: i32) -> u32 {
    0x3700_0000
        | ((bit >> 5) << 31)
        | ((bit & 31) << 19)
        | (((offset_words as u32) & 0x3fff) << 5)
        | rt
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
        } else if imm < 0 && imm.unsigned_abs() & 0xfff == 0 && imm.unsigned_abs() < (1 << 24) {
            self.emit(sub_imm(
                true,
                rd,
                rn,
                (imm.unsigned_abs() >> 12) as u32,
                true,
            ));
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
#[cfg(test)]
mod tests {
    //! Every expected word below was produced by the host assembler (`clang -c -target
    //! arm64-apple-macos -march=armv8.4-a+flagm`, read back with `objdump -d`), never written by
    //! hand. The comment/string next to each word is the assembly that produced it.

    use super::*;

    /// Compares each encoder result with the assembler word: `(encoder, assembler, asm text)`.
    fn check(cases: &[(u32, u32, &str)]) {
        for &(got, want, asm) in cases {
            assert_eq!(
                got, want,
                "`{asm}`: encoder {got:#010x}, assembler {want:#010x}"
            );
        }
    }

    /// Compares an assembly buffer with the assembler words: `(assembler word, asm text)`.
    fn check_words(a: &Asm, expected: &[(u32, &str)]) {
        let want: Vec<u32> = expected.iter().map(|e| e.0).collect();
        assert_eq!(
            a.words.len(),
            want.len(),
            "word count: got {:08x?}, want {:08x?} ({:?})",
            a.words,
            want,
            expected.iter().map(|e| e.1).collect::<Vec<_>>()
        );
        for (i, (&got, &(want, asm))) in a.words.iter().zip(expected).enumerate() {
            assert_eq!(
                got, want,
                "word {i} `{asm}`: encoder {got:#010x}, assembler {want:#010x}"
            );
        }
    }

    #[test]
    fn enc_mov_wide() {
        check(&[
            (movz(true, 3, 0x1234, 0), 0xd2824683, "movz x3, #0x1234"),
            (
                movz(true, 17, 0xffff, 1),
                0xd2bffff1,
                "movz x17, #0xffff, lsl #16",
            ),
            (movk(true, 3, 0xabcd, 0), 0xf29579a3, "movk x3, #0xabcd"),
            (
                movk(true, 20, 0x0001, 1),
                0xf2a00034,
                "movk x20, #0x1, lsl #16",
            ),
            (movn(true, 5, 0x00ff, 0), 0x92801fe5, "movn x5, #0xff"),
            (
                movn(true, 29, 0x8000, 1),
                0x92b0001d,
                "movn x29, #0x8000, lsl #16",
            ),
            (movz(false, 3, 0x1234, 0), 0x52824683, "movz w3, #0x1234"),
            (
                movz(false, 17, 0xffff, 1),
                0x52bffff1,
                "movz w17, #0xffff, lsl #16",
            ),
            (movk(false, 3, 0xabcd, 0), 0x729579a3, "movk w3, #0xabcd"),
            (
                movk(false, 20, 0x0001, 1),
                0x72a00034,
                "movk w20, #0x1, lsl #16",
            ),
            (movn(false, 5, 0x00ff, 0), 0x12801fe5, "movn w5, #0xff"),
            (
                movn(false, 29, 0x8000, 1),
                0x12b0001d,
                "movn w29, #0x8000, lsl #16",
            ),
            (
                movz(true, 7, 0x4321, 2),
                0xd2c86427,
                "movz x7, #0x4321, lsl #32",
            ),
            (
                movk(true, 7, 0x8765, 3),
                0xf2f0eca7,
                "movk x7, #0x8765, lsl #48",
            ),
            (
                movn(true, 7, 0x0f0f, 3),
                0x92e1e1e7,
                "movn x7, #0xf0f, lsl #48",
            ),
        ]);
    }

    #[test]
    fn enc_addsub_imm() {
        check(&[
            (
                add_imm(true, 3, 17, 0x123, false),
                0x91048e23,
                "add x3, x17, #0x123",
            ),
            (
                add_imm(true, 4, 18, 0xfff, true),
                0x917ffe44,
                "add x4, x18, #0xfff, lsl #12",
            ),
            (
                adds_imm(true, 3, 17, 0x123, false),
                0xb1048e23,
                "adds x3, x17, #0x123",
            ),
            (
                adds_imm(true, 4, 18, 0xfff, true),
                0xb17ffe44,
                "adds x4, x18, #0xfff, lsl #12",
            ),
            (
                sub_imm(true, 3, 17, 0x123, false),
                0xd1048e23,
                "sub x3, x17, #0x123",
            ),
            (
                sub_imm(true, 4, 18, 0xfff, true),
                0xd17ffe44,
                "sub x4, x18, #0xfff, lsl #12",
            ),
            (
                subs_imm(true, 3, 17, 0x123, false),
                0xf1048e23,
                "subs x3, x17, #0x123",
            ),
            (
                subs_imm(true, 4, 18, 0xfff, true),
                0xf17ffe44,
                "subs x4, x18, #0xfff, lsl #12",
            ),
            (
                add_imm(false, 3, 17, 0x123, false),
                0x11048e23,
                "add w3, w17, #0x123",
            ),
            (
                add_imm(false, 4, 18, 0xfff, true),
                0x117ffe44,
                "add w4, w18, #0xfff, lsl #12",
            ),
            (
                adds_imm(false, 3, 17, 0x123, false),
                0x31048e23,
                "adds w3, w17, #0x123",
            ),
            (
                adds_imm(false, 4, 18, 0xfff, true),
                0x317ffe44,
                "adds w4, w18, #0xfff, lsl #12",
            ),
            (
                sub_imm(false, 3, 17, 0x123, false),
                0x51048e23,
                "sub w3, w17, #0x123",
            ),
            (
                sub_imm(false, 4, 18, 0xfff, true),
                0x517ffe44,
                "sub w4, w18, #0xfff, lsl #12",
            ),
            (
                subs_imm(false, 3, 17, 0x123, false),
                0x71048e23,
                "subs w3, w17, #0x123",
            ),
            (
                subs_imm(false, 4, 18, 0xfff, true),
                0x717ffe44,
                "subs w4, w18, #0xfff, lsl #12",
            ),
            (
                add_imm(true, 3, SP, 16, false),
                0x910043e3,
                "add x3, sp, #16",
            ),
            (
                sub_imm(true, SP, SP, 32, false),
                0xd10083ff,
                "sub sp, sp, #32",
            ),
            (mov_sp(SP, 3), 0x9100007f, "mov sp, x3"),
            (mov_sp(4, SP), 0x910003e4, "mov x4, sp"),
        ]);
    }

    #[test]
    fn enc_addsub_reg() {
        check(&[
            (
                add_reg(true, 3, 17, 25, LSL, 0),
                0x8b190223,
                "add x3, x17, x25",
            ),
            (
                add_reg(true, 3, 17, 25, LSL, 5),
                0x8b191623,
                "add x3, x17, x25, lsl #5",
            ),
            (
                add_reg(true, 3, 17, 25, LSR, 7),
                0x8b591e23,
                "add x3, x17, x25, lsr #7",
            ),
            (
                add_reg(true, 3, 17, 25, ASR, 9),
                0x8b992623,
                "add x3, x17, x25, asr #9",
            ),
            (
                sub_reg(true, 4, 18, 26, LSL, 0),
                0xcb1a0244,
                "sub x4, x18, x26",
            ),
            (
                sub_reg(true, 4, 18, 26, LSL, 3),
                0xcb1a0e44,
                "sub x4, x18, x26, lsl #3",
            ),
            (
                sub_reg(true, 4, 18, 26, LSR, 11),
                0xcb5a2e44,
                "sub x4, x18, x26, lsr #11",
            ),
            (
                sub_reg(true, 4, 18, 26, ASR, 13),
                0xcb9a3644,
                "sub x4, x18, x26, asr #13",
            ),
            (adds_reg(true, 5, 19, 27), 0xab1b0265, "adds x5, x19, x27"),
            (subs_reg(true, 6, 20, 28), 0xeb1c0286, "subs x6, x20, x28"),
            (subs_reg(true, XZR, 20, 28), 0xeb1c029f, "cmp x20, x28"),
            (
                add_reg(false, 3, 17, 25, LSL, 0),
                0x0b190223,
                "add w3, w17, w25",
            ),
            (
                add_reg(false, 3, 17, 25, LSL, 5),
                0x0b191623,
                "add w3, w17, w25, lsl #5",
            ),
            (
                add_reg(false, 3, 17, 25, LSR, 7),
                0x0b591e23,
                "add w3, w17, w25, lsr #7",
            ),
            (
                add_reg(false, 3, 17, 25, ASR, 9),
                0x0b992623,
                "add w3, w17, w25, asr #9",
            ),
            (
                sub_reg(false, 4, 18, 26, LSL, 0),
                0x4b1a0244,
                "sub w4, w18, w26",
            ),
            (
                sub_reg(false, 4, 18, 26, LSL, 3),
                0x4b1a0e44,
                "sub w4, w18, w26, lsl #3",
            ),
            (
                sub_reg(false, 4, 18, 26, LSR, 11),
                0x4b5a2e44,
                "sub w4, w18, w26, lsr #11",
            ),
            (
                sub_reg(false, 4, 18, 26, ASR, 13),
                0x4b9a3644,
                "sub w4, w18, w26, asr #13",
            ),
            (adds_reg(false, 5, 19, 27), 0x2b1b0265, "adds w5, w19, w27"),
            (subs_reg(false, 6, 20, 28), 0x6b1c0286, "subs w6, w20, w28"),
            (subs_reg(false, XZR, 20, 28), 0x6b1c029f, "cmp w20, w28"),
        ]);
    }

    #[test]
    fn enc_carry_ops() {
        check(&[
            (adc(true, 3, 17, 25), 0x9a190223, "adc x3, x17, x25"),
            (adcs(true, 4, 18, 26), 0xba1a0244, "adcs x4, x18, x26"),
            (sbc(true, 5, 19, 27), 0xda1b0265, "sbc x5, x19, x27"),
            (sbcs(true, 6, 20, 28), 0xfa1c0286, "sbcs x6, x20, x28"),
            (adc(false, 3, 17, 25), 0x1a190223, "adc w3, w17, w25"),
            (adcs(false, 4, 18, 26), 0x3a1a0244, "adcs w4, w18, w26"),
            (sbc(false, 5, 19, 27), 0x5a1b0265, "sbc w5, w19, w27"),
            (sbcs(false, 6, 20, 28), 0x7a1c0286, "sbcs w6, w20, w28"),
        ]);
    }

    #[test]
    fn enc_logical_reg() {
        check(&[
            (and_reg(true, 3, 17, 25), 0x8a190223, "and x3, x17, x25"),
            (ands_reg(true, 3, 17, 25), 0xea190223, "ands x3, x17, x25"),
            (orr_reg(true, 3, 17, 25), 0xaa190223, "orr x3, x17, x25"),
            (eor_reg(true, 3, 17, 25), 0xca190223, "eor x3, x17, x25"),
            (bic_reg(true, 3, 17, 25), 0x8a390223, "bic x3, x17, x25"),
            (orn_reg(true, 3, 17, 25), 0xaa390223, "orn x3, x17, x25"),
            (and_reg(false, 3, 17, 25), 0x0a190223, "and w3, w17, w25"),
            (ands_reg(false, 3, 17, 25), 0x6a190223, "ands w3, w17, w25"),
            (orr_reg(false, 3, 17, 25), 0x2a190223, "orr w3, w17, w25"),
            (eor_reg(false, 3, 17, 25), 0x4a190223, "eor w3, w17, w25"),
            (bic_reg(false, 3, 17, 25), 0x0a390223, "bic w3, w17, w25"),
            (orn_reg(false, 3, 17, 25), 0x2a390223, "orn w3, w17, w25"),
        ]);
    }

    #[test]
    fn enc_logical_reg_shift() {
        check(&[
            (
                orr_reg_shift(true, 3, 17, 25, LSL, 4),
                0xaa191223,
                "orr x3, x17, x25, lsl #4",
            ),
            (
                orr_reg_shift(true, 3, 17, 25, LSR, 6),
                0xaa591a23,
                "orr x3, x17, x25, lsr #6",
            ),
            (
                orr_reg_shift(true, 3, 17, 25, ASR, 8),
                0xaa992223,
                "orr x3, x17, x25, asr #8",
            ),
            (
                eor_reg_shift(true, 3, 17, 25, LSL, 4),
                0xca191223,
                "eor x3, x17, x25, lsl #4",
            ),
            (
                eor_reg_shift(true, 3, 17, 25, LSR, 6),
                0xca591a23,
                "eor x3, x17, x25, lsr #6",
            ),
            (
                eor_reg_shift(true, 3, 17, 25, ASR, 8),
                0xca992223,
                "eor x3, x17, x25, asr #8",
            ),
            (
                orr_reg_shift(false, 3, 17, 25, LSL, 4),
                0x2a191223,
                "orr w3, w17, w25, lsl #4",
            ),
            (
                orr_reg_shift(false, 3, 17, 25, LSR, 6),
                0x2a591a23,
                "orr w3, w17, w25, lsr #6",
            ),
            (
                orr_reg_shift(false, 3, 17, 25, ASR, 8),
                0x2a992223,
                "orr w3, w17, w25, asr #8",
            ),
            (
                eor_reg_shift(false, 3, 17, 25, LSL, 4),
                0x4a191223,
                "eor w3, w17, w25, lsl #4",
            ),
            (
                eor_reg_shift(false, 3, 17, 25, LSR, 6),
                0x4a591a23,
                "eor w3, w17, w25, lsr #6",
            ),
            (
                eor_reg_shift(false, 3, 17, 25, ASR, 8),
                0x4a992223,
                "eor w3, w17, w25, asr #8",
            ),
        ]);
    }

    #[test]
    fn enc_logical_alias() {
        check(&[
            (mov_reg(true, 3, 25), 0xaa1903e3, "mov x3, x25"),
            (mvn(true, 3, 25), 0xaa3903e3, "mvn x3, x25"),
            (tst_reg(true, 17, 25), 0xea19023f, "tst x17, x25"),
            (mov_reg(false, 3, 25), 0x2a1903e3, "mov w3, w25"),
            (mvn(false, 3, 25), 0x2a3903e3, "mvn w3, w25"),
            (tst_reg(false, 17, 25), 0x6a19023f, "tst w17, w25"),
        ]);
    }

    #[test]
    fn enc_logical_imm() {
        check(&[
            (
                and_imm(true, 3, 17, 0xff).unwrap(),
                0x92401e23,
                "and x3, x17, #0xff",
            ),
            (
                orr_imm(true, 3, 17, 0xff).unwrap(),
                0xb2401e23,
                "orr x3, x17, #0xff",
            ),
            (
                eor_imm(true, 3, 17, 0xff).unwrap(),
                0xd2401e23,
                "eor x3, x17, #0xff",
            ),
            (
                ands_imm(true, 3, 17, 0xff).unwrap(),
                0xf2401e23,
                "ands x3, x17, #0xff",
            ),
            (
                and_imm(true, 3, 17, 0x7fffffffffffffff).unwrap(),
                0x9240fa23,
                "and x3, x17, #0x7fffffffffffffff",
            ),
            (
                orr_imm(true, 3, 17, 0x7fffffffffffffff).unwrap(),
                0xb240fa23,
                "orr x3, x17, #0x7fffffffffffffff",
            ),
            (
                eor_imm(true, 3, 17, 0x7fffffffffffffff).unwrap(),
                0xd240fa23,
                "eor x3, x17, #0x7fffffffffffffff",
            ),
            (
                ands_imm(true, 3, 17, 0x7fffffffffffffff).unwrap(),
                0xf240fa23,
                "ands x3, x17, #0x7fffffffffffffff",
            ),
            (
                and_imm(true, 3, 17, 0x5555555555555555).unwrap(),
                0x9200f223,
                "and x3, x17, #0x5555555555555555",
            ),
            (
                orr_imm(true, 3, 17, 0x5555555555555555).unwrap(),
                0xb200f223,
                "orr x3, x17, #0x5555555555555555",
            ),
            (
                eor_imm(true, 3, 17, 0x5555555555555555).unwrap(),
                0xd200f223,
                "eor x3, x17, #0x5555555555555555",
            ),
            (
                ands_imm(true, 3, 17, 0x5555555555555555).unwrap(),
                0xf200f223,
                "ands x3, x17, #0x5555555555555555",
            ),
            (
                and_imm(true, 3, 17, 0x3000300030003).unwrap(),
                0x92008623,
                "and x3, x17, #0x3000300030003",
            ),
            (
                orr_imm(true, 3, 17, 0x3000300030003).unwrap(),
                0xb2008623,
                "orr x3, x17, #0x3000300030003",
            ),
            (
                eor_imm(true, 3, 17, 0x3000300030003).unwrap(),
                0xd2008623,
                "eor x3, x17, #0x3000300030003",
            ),
            (
                ands_imm(true, 3, 17, 0x3000300030003).unwrap(),
                0xf2008623,
                "ands x3, x17, #0x3000300030003",
            ),
            (
                and_imm(true, 3, 17, 0xffff0000).unwrap(),
                0x92703e23,
                "and x3, x17, #0xffff0000",
            ),
            (
                orr_imm(true, 3, 17, 0xffff0000).unwrap(),
                0xb2703e23,
                "orr x3, x17, #0xffff0000",
            ),
            (
                eor_imm(true, 3, 17, 0xffff0000).unwrap(),
                0xd2703e23,
                "eor x3, x17, #0xffff0000",
            ),
            (
                ands_imm(true, 3, 17, 0xffff0000).unwrap(),
                0xf2703e23,
                "ands x3, x17, #0xffff0000",
            ),
            (
                and_imm(true, 3, 17, 0x6).unwrap(),
                0x927f0623,
                "and x3, x17, #0x6",
            ),
            (
                orr_imm(true, 3, 17, 0x6).unwrap(),
                0xb27f0623,
                "orr x3, x17, #0x6",
            ),
            (
                eor_imm(true, 3, 17, 0x6).unwrap(),
                0xd27f0623,
                "eor x3, x17, #0x6",
            ),
            (
                ands_imm(true, 3, 17, 0x6).unwrap(),
                0xf27f0623,
                "ands x3, x17, #0x6",
            ),
            (
                and_imm(true, 3, 17, 0x101010101010101).unwrap(),
                0x9200c223,
                "and x3, x17, #0x101010101010101",
            ),
            (
                orr_imm(true, 3, 17, 0x101010101010101).unwrap(),
                0xb200c223,
                "orr x3, x17, #0x101010101010101",
            ),
            (
                eor_imm(true, 3, 17, 0x101010101010101).unwrap(),
                0xd200c223,
                "eor x3, x17, #0x101010101010101",
            ),
            (
                ands_imm(true, 3, 17, 0x101010101010101).unwrap(),
                0xf200c223,
                "ands x3, x17, #0x101010101010101",
            ),
            (
                and_imm(true, 3, 17, 0xfff00000fff).unwrap(),
                0x92002e23,
                "and x3, x17, #0xfff00000fff",
            ),
            (
                orr_imm(true, 3, 17, 0xfff00000fff).unwrap(),
                0xb2002e23,
                "orr x3, x17, #0xfff00000fff",
            ),
            (
                eor_imm(true, 3, 17, 0xfff00000fff).unwrap(),
                0xd2002e23,
                "eor x3, x17, #0xfff00000fff",
            ),
            (
                ands_imm(true, 3, 17, 0xfff00000fff).unwrap(),
                0xf2002e23,
                "ands x3, x17, #0xfff00000fff",
            ),
            (
                and_imm(true, 3, 17, 0xe0e0e0e0e0e0e0e0).unwrap(),
                0x9203ca23,
                "and x3, x17, #0xe0e0e0e0e0e0e0e0",
            ),
            (
                orr_imm(true, 3, 17, 0xe0e0e0e0e0e0e0e0).unwrap(),
                0xb203ca23,
                "orr x3, x17, #0xe0e0e0e0e0e0e0e0",
            ),
            (
                eor_imm(true, 3, 17, 0xe0e0e0e0e0e0e0e0).unwrap(),
                0xd203ca23,
                "eor x3, x17, #0xe0e0e0e0e0e0e0e0",
            ),
            (
                ands_imm(true, 3, 17, 0xe0e0e0e0e0e0e0e0).unwrap(),
                0xf203ca23,
                "ands x3, x17, #0xe0e0e0e0e0e0e0e0",
            ),
            (
                and_imm(false, 3, 17, 0x1).unwrap(),
                0x12000223,
                "and w3, w17, #0x1",
            ),
            (
                orr_imm(false, 3, 17, 0x1).unwrap(),
                0x32000223,
                "orr w3, w17, #0x1",
            ),
            (
                eor_imm(false, 3, 17, 0x1).unwrap(),
                0x52000223,
                "eor w3, w17, #0x1",
            ),
            (
                ands_imm(false, 3, 17, 0x1).unwrap(),
                0x72000223,
                "ands w3, w17, #0x1",
            ),
            (
                and_imm(false, 3, 17, 0xfffffffe).unwrap(),
                0x121f7a23,
                "and w3, w17, #0xfffffffe",
            ),
            (
                orr_imm(false, 3, 17, 0xfffffffe).unwrap(),
                0x321f7a23,
                "orr w3, w17, #0xfffffffe",
            ),
            (
                eor_imm(false, 3, 17, 0xfffffffe).unwrap(),
                0x521f7a23,
                "eor w3, w17, #0xfffffffe",
            ),
            (
                ands_imm(false, 3, 17, 0xfffffffe).unwrap(),
                0x721f7a23,
                "ands w3, w17, #0xfffffffe",
            ),
            (
                and_imm(false, 3, 17, 0xaaaaaaaa).unwrap(),
                0x1201f223,
                "and w3, w17, #0xaaaaaaaa",
            ),
            (
                orr_imm(false, 3, 17, 0xaaaaaaaa).unwrap(),
                0x3201f223,
                "orr w3, w17, #0xaaaaaaaa",
            ),
            (
                eor_imm(false, 3, 17, 0xaaaaaaaa).unwrap(),
                0x5201f223,
                "eor w3, w17, #0xaaaaaaaa",
            ),
            (
                ands_imm(false, 3, 17, 0xaaaaaaaa).unwrap(),
                0x7201f223,
                "ands w3, w17, #0xaaaaaaaa",
            ),
            (
                and_imm(false, 3, 17, 0x1ffff).unwrap(),
                0x12004223,
                "and w3, w17, #0x1ffff",
            ),
            (
                orr_imm(false, 3, 17, 0x1ffff).unwrap(),
                0x32004223,
                "orr w3, w17, #0x1ffff",
            ),
            (
                eor_imm(false, 3, 17, 0x1ffff).unwrap(),
                0x52004223,
                "eor w3, w17, #0x1ffff",
            ),
            (
                ands_imm(false, 3, 17, 0x1ffff).unwrap(),
                0x72004223,
                "ands w3, w17, #0x1ffff",
            ),
            (
                and_imm(false, 3, 17, 0x80000001).unwrap(),
                0x12010623,
                "and w3, w17, #0x80000001",
            ),
            (
                orr_imm(false, 3, 17, 0x80000001).unwrap(),
                0x32010623,
                "orr w3, w17, #0x80000001",
            ),
            (
                eor_imm(false, 3, 17, 0x80000001).unwrap(),
                0x52010623,
                "eor w3, w17, #0x80000001",
            ),
            (
                ands_imm(false, 3, 17, 0x80000001).unwrap(),
                0x72010623,
                "ands w3, w17, #0x80000001",
            ),
            (
                and_imm(false, 3, 17, 0x1010101).unwrap(),
                0x1200c223,
                "and w3, w17, #0x1010101",
            ),
            (
                orr_imm(false, 3, 17, 0x1010101).unwrap(),
                0x3200c223,
                "orr w3, w17, #0x1010101",
            ),
            (
                eor_imm(false, 3, 17, 0x1010101).unwrap(),
                0x5200c223,
                "eor w3, w17, #0x1010101",
            ),
            (
                ands_imm(false, 3, 17, 0x1010101).unwrap(),
                0x7200c223,
                "ands w3, w17, #0x1010101",
            ),
            (
                and_imm(false, 3, 17, 0xfffffff0).unwrap(),
                0x121c6e23,
                "and w3, w17, #0xfffffff0",
            ),
            (
                orr_imm(false, 3, 17, 0xfffffff0).unwrap(),
                0x321c6e23,
                "orr w3, w17, #0xfffffff0",
            ),
            (
                eor_imm(false, 3, 17, 0xfffffff0).unwrap(),
                0x521c6e23,
                "eor w3, w17, #0xfffffff0",
            ),
            (
                ands_imm(false, 3, 17, 0xfffffff0).unwrap(),
                0x721c6e23,
                "ands w3, w17, #0xfffffff0",
            ),
            (
                and_imm(false, 3, 17, 0xff00ff0).unwrap(),
                0x120c9e23,
                "and w3, w17, #0xff00ff0",
            ),
            (
                orr_imm(false, 3, 17, 0xff00ff0).unwrap(),
                0x320c9e23,
                "orr w3, w17, #0xff00ff0",
            ),
            (
                eor_imm(false, 3, 17, 0xff00ff0).unwrap(),
                0x520c9e23,
                "eor w3, w17, #0xff00ff0",
            ),
            (
                ands_imm(false, 3, 17, 0xff00ff0).unwrap(),
                0x720c9e23,
                "ands w3, w17, #0xff00ff0",
            ),
            (
                and_imm(true, SP, 3, 0xff).unwrap(),
                0x92401c7f,
                "and sp, x3, #0xff",
            ),
            (
                orr_imm(true, SP, 17, 0xff0).unwrap(),
                0xb27c1e3f,
                "orr sp, x17, #0xff0",
            ),
            (
                ands_imm(true, XZR, 17, 0x8000).unwrap(),
                0xf271023f,
                "tst x17, #0x8000",
            ),
        ]);
    }

    #[test]
    fn enc_bitfield() {
        check(&[
            (ubfm(true, 3, 17, 3, 9), 0xd3432623, "ubfm x3, x17, #3, #9"),
            (
                ubfm(true, 3, 17, 20, 7),
                0xd3541e23,
                "ubfm x3, x17, #20, #7",
            ),
            (
                sbfm(true, 4, 18, 5, 11),
                0x93452e44,
                "sbfm x4, x18, #5, #11",
            ),
            (
                sbfm(true, 4, 18, 25, 3),
                0x93590e44,
                "sbfm x4, x18, #25, #3",
            ),
            (bfm(true, 5, 19, 6, 12), 0xb3463265, "bfm x5, x19, #6, #12"),
            (bfm(true, 5, 19, 28, 4), 0xb35c1265, "bfm x5, x19, #28, #4"),
            (ubfm(false, 3, 17, 3, 9), 0x53032623, "ubfm w3, w17, #3, #9"),
            (
                ubfm(false, 3, 17, 20, 7),
                0x53141e23,
                "ubfm w3, w17, #20, #7",
            ),
            (
                sbfm(false, 4, 18, 5, 11),
                0x13052e44,
                "sbfm w4, w18, #5, #11",
            ),
            (
                sbfm(false, 4, 18, 25, 3),
                0x13190e44,
                "sbfm w4, w18, #25, #3",
            ),
            (bfm(false, 5, 19, 6, 12), 0x33063265, "bfm w5, w19, #6, #12"),
            (bfm(false, 5, 19, 28, 4), 0x331c1265, "bfm w5, w19, #28, #4"),
        ]);
    }

    #[test]
    fn enc_shift_imm() {
        check(&[
            (lsl_imm(true, 3, 17, 1), 0xd37ffa23, "lsl x3, x17, #1"),
            (lsr_imm(true, 3, 17, 1), 0xd341fe23, "lsr x3, x17, #1"),
            (asr_imm(true, 3, 17, 1), 0x9341fe23, "asr x3, x17, #1"),
            (ror_imm(true, 3, 17, 1), 0x93d10623, "ror x3, x17, #1"),
            (lsl_imm(true, 3, 17, 7), 0xd379e223, "lsl x3, x17, #7"),
            (lsr_imm(true, 3, 17, 7), 0xd347fe23, "lsr x3, x17, #7"),
            (asr_imm(true, 3, 17, 7), 0x9347fe23, "asr x3, x17, #7"),
            (ror_imm(true, 3, 17, 7), 0x93d11e23, "ror x3, x17, #7"),
            (lsl_imm(true, 3, 17, 63), 0xd3410223, "lsl x3, x17, #63"),
            (lsr_imm(true, 3, 17, 63), 0xd37ffe23, "lsr x3, x17, #63"),
            (asr_imm(true, 3, 17, 63), 0x937ffe23, "asr x3, x17, #63"),
            (ror_imm(true, 3, 17, 63), 0x93d1fe23, "ror x3, x17, #63"),
            (lsr_imm(true, 3, 17, 0), 0xd340fe23, "lsr x3, x17, #0"),
            (lsl_imm(true, 3, 17, 0), 0xd340fe23, "lsl x3, x17, #0"),
            (lsl_imm(false, 3, 17, 1), 0x531f7a23, "lsl w3, w17, #1"),
            (lsr_imm(false, 3, 17, 1), 0x53017e23, "lsr w3, w17, #1"),
            (asr_imm(false, 3, 17, 1), 0x13017e23, "asr w3, w17, #1"),
            (ror_imm(false, 3, 17, 1), 0x13910623, "ror w3, w17, #1"),
            (lsl_imm(false, 3, 17, 7), 0x53196223, "lsl w3, w17, #7"),
            (lsr_imm(false, 3, 17, 7), 0x53077e23, "lsr w3, w17, #7"),
            (asr_imm(false, 3, 17, 7), 0x13077e23, "asr w3, w17, #7"),
            (ror_imm(false, 3, 17, 7), 0x13911e23, "ror w3, w17, #7"),
            (lsl_imm(false, 3, 17, 31), 0x53010223, "lsl w3, w17, #31"),
            (lsr_imm(false, 3, 17, 31), 0x531f7e23, "lsr w3, w17, #31"),
            (asr_imm(false, 3, 17, 31), 0x131f7e23, "asr w3, w17, #31"),
            (ror_imm(false, 3, 17, 31), 0x13917e23, "ror w3, w17, #31"),
            (lsr_imm(false, 3, 17, 0), 0x53007e23, "lsr w3, w17, #0"),
            (lsl_imm(false, 3, 17, 0), 0x53007e23, "lsl w3, w17, #0"),
        ]);
    }

    #[test]
    fn enc_extr() {
        check(&[
            (
                extr(true, 3, 17, 25, 9),
                0x93d92623,
                "extr x3, x17, x25, #9",
            ),
            (
                extr(true, 3, 17, 25, 0),
                0x93d90223,
                "extr x3, x17, x25, #0",
            ),
            (
                extr(true, 3, 17, 25, 63),
                0x93d9fe23,
                "extr x3, x17, x25, #63",
            ),
            (
                extr(false, 3, 17, 25, 9),
                0x13992623,
                "extr w3, w17, w25, #9",
            ),
            (
                extr(false, 3, 17, 25, 0),
                0x13990223,
                "extr w3, w17, w25, #0",
            ),
            (
                extr(false, 3, 17, 25, 31),
                0x13997e23,
                "extr w3, w17, w25, #31",
            ),
        ]);
    }

    #[test]
    fn enc_bitfield_alias() {
        check(&[
            (ubfx(true, 3, 17, 4, 8), 0xd3442e23, "ubfx x3, x17, #4, #8"),
            (ubfx(true, 3, 17, 0, 1), 0xd3400223, "ubfx x3, x17, #0, #1"),
            (sbfx(true, 4, 18, 5, 9), 0x93453644, "sbfx x4, x18, #5, #9"),
            (
                sbfx(true, 4, 18, 56, 8),
                0x9378fe44,
                "sbfx x4, x18, #56, #8",
            ),
            (bfi(true, 5, 19, 4, 8), 0xb37c1e65, "bfi x5, x19, #4, #8"),
            (bfi(true, 5, 19, 0, 16), 0xb3403e65, "bfi x5, x19, #0, #16"),
            (
                bfxil(true, 6, 20, 8, 12),
                0xb3484e86,
                "bfxil x6, x20, #8, #12",
            ),
            (
                bfxil(true, 6, 20, 0, 4),
                0xb3400e86,
                "bfxil x6, x20, #0, #4",
            ),
            (ubfx(false, 3, 17, 4, 8), 0x53042e23, "ubfx w3, w17, #4, #8"),
            (ubfx(false, 3, 17, 0, 1), 0x53000223, "ubfx w3, w17, #0, #1"),
            (sbfx(false, 4, 18, 5, 9), 0x13053644, "sbfx w4, w18, #5, #9"),
            (
                sbfx(false, 4, 18, 24, 8),
                0x13187e44,
                "sbfx w4, w18, #24, #8",
            ),
            (bfi(false, 5, 19, 4, 8), 0x331c1e65, "bfi w5, w19, #4, #8"),
            (bfi(false, 5, 19, 0, 16), 0x33003e65, "bfi w5, w19, #0, #16"),
            (
                bfxil(false, 6, 20, 8, 12),
                0x33084e86,
                "bfxil w6, w20, #8, #12",
            ),
            (
                bfxil(false, 6, 20, 0, 4),
                0x33000e86,
                "bfxil w6, w20, #0, #4",
            ),
        ]);
    }

    #[test]
    fn enc_extend() {
        check(&[
            (sxtb(3, 17), 0x93401e23, "sxtb x3, w17"),
            (sxth(3, 17), 0x93403e23, "sxth x3, w17"),
            (sxtw(3, 17), 0x93407e23, "sxtw x3, w17"),
            (uxtw(3, 17), 0x2a1103e3, "mov w3, w17"),
        ]);
    }

    #[test]
    fn enc_variable_shift_div_mul() {
        check(&[
            (lslv(true, 3, 17, 25), 0x9ad92223, "lsl x3, x17, x25"),
            (lsrv(true, 3, 17, 25), 0x9ad92623, "lsr x3, x17, x25"),
            (asrv(true, 3, 17, 25), 0x9ad92a23, "asr x3, x17, x25"),
            (rorv(true, 3, 17, 25), 0x9ad92e23, "ror x3, x17, x25"),
            (udiv(true, 3, 17, 25), 0x9ad90a23, "udiv x3, x17, x25"),
            (sdiv(true, 3, 17, 25), 0x9ad90e23, "sdiv x3, x17, x25"),
            (mul(true, 3, 17, 25), 0x9b197e23, "mul x3, x17, x25"),
            (
                madd(true, 3, 17, 25, 9),
                0x9b192623,
                "madd x3, x17, x25, x9",
            ),
            (madd(true, 3, 17, 25, XZR), 0x9b197e23, "mul x3, x17, x25"),
            (lslv(false, 3, 17, 25), 0x1ad92223, "lsl w3, w17, w25"),
            (lsrv(false, 3, 17, 25), 0x1ad92623, "lsr w3, w17, w25"),
            (asrv(false, 3, 17, 25), 0x1ad92a23, "asr w3, w17, w25"),
            (rorv(false, 3, 17, 25), 0x1ad92e23, "ror w3, w17, w25"),
            (udiv(false, 3, 17, 25), 0x1ad90a23, "udiv w3, w17, w25"),
            (sdiv(false, 3, 17, 25), 0x1ad90e23, "sdiv w3, w17, w25"),
            (mul(false, 3, 17, 25), 0x1b197e23, "mul w3, w17, w25"),
            (
                madd(false, 3, 17, 25, 9),
                0x1b192623,
                "madd w3, w17, w25, w9",
            ),
            (madd(false, 3, 17, 25, XZR), 0x1b197e23, "mul w3, w17, w25"),
        ]);
    }

    #[test]
    fn enc_bit_ops() {
        check(&[
            (clz(true, 3, 17), 0xdac01223, "clz x3, x17"),
            (rbit(true, 3, 17), 0xdac00223, "rbit x3, x17"),
            (rev(true, 3, 17), 0xdac00e23, "rev x3, x17"),
            (clz(false, 3, 17), 0x5ac01223, "clz w3, w17"),
            (rbit(false, 3, 17), 0x5ac00223, "rbit w3, w17"),
            (rev(false, 3, 17), 0x5ac00a23, "rev w3, w17"),
            (rev16(3, 17), 0x5ac00623, "rev16 w3, w17"),
        ]);
    }

    #[test]
    fn enc_wide_mul() {
        check(&[
            (smulh(3, 17, 25), 0x9b597e23, "smulh x3, x17, x25"),
            (umulh(3, 17, 25), 0x9bd97e23, "umulh x3, x17, x25"),
            (smull(3, 17, 25), 0x9b397e23, "smull x3, w17, w25"),
        ]);
    }

    #[test]
    fn enc_ldr_str_imm() {
        check(&[
            (ldr_imm(1, 3, 17, 0), 0x39400223, "ldrb  w3, [x17, #0]"),
            (str_imm(1, 3, 17, 0), 0x39000223, "strb  w3, [x17, #0]"),
            (ldr_imm(1, 3, 17, 5), 0x39401623, "ldrb  w3, [x17, #5]"),
            (str_imm(1, 3, 17, 5), 0x39001623, "strb  w3, [x17, #5]"),
            (
                ldr_imm(1, 3, 17, 4095),
                0x397ffe23,
                "ldrb  w3, [x17, #4095]",
            ),
            (
                str_imm(1, 3, 17, 4095),
                0x393ffe23,
                "strb  w3, [x17, #4095]",
            ),
            (ldr_imm(1, 3, SP, 2), 0x39400be3, "ldrb  w3, [sp, #2]"),
            (ldr_imm(2, 3, 17, 0), 0x79400223, "ldrh  w3, [x17, #0]"),
            (str_imm(2, 3, 17, 0), 0x79000223, "strh  w3, [x17, #0]"),
            (ldr_imm(2, 3, 17, 10), 0x79401623, "ldrh  w3, [x17, #10]"),
            (str_imm(2, 3, 17, 10), 0x79001623, "strh  w3, [x17, #10]"),
            (
                ldr_imm(2, 3, 17, 8190),
                0x797ffe23,
                "ldrh  w3, [x17, #8190]",
            ),
            (
                str_imm(2, 3, 17, 8190),
                0x793ffe23,
                "strh  w3, [x17, #8190]",
            ),
            (ldr_imm(2, 3, SP, 4), 0x79400be3, "ldrh  w3, [sp, #4]"),
            (ldr_imm(4, 3, 17, 0), 0xb9400223, "ldr  w3, [x17, #0]"),
            (str_imm(4, 3, 17, 0), 0xb9000223, "str  w3, [x17, #0]"),
            (ldr_imm(4, 3, 17, 20), 0xb9401623, "ldr  w3, [x17, #20]"),
            (str_imm(4, 3, 17, 20), 0xb9001623, "str  w3, [x17, #20]"),
            (
                ldr_imm(4, 3, 17, 16380),
                0xb97ffe23,
                "ldr  w3, [x17, #16380]",
            ),
            (
                str_imm(4, 3, 17, 16380),
                0xb93ffe23,
                "str  w3, [x17, #16380]",
            ),
            (ldr_imm(4, 3, SP, 8), 0xb9400be3, "ldr  w3, [sp, #8]"),
            (ldr_imm(8, 3, 17, 0), 0xf9400223, "ldr  x3, [x17, #0]"),
            (str_imm(8, 3, 17, 0), 0xf9000223, "str  x3, [x17, #0]"),
            (ldr_imm(8, 3, 17, 40), 0xf9401623, "ldr  x3, [x17, #40]"),
            (str_imm(8, 3, 17, 40), 0xf9001623, "str  x3, [x17, #40]"),
            (
                ldr_imm(8, 3, 17, 32760),
                0xf97ffe23,
                "ldr  x3, [x17, #32760]",
            ),
            (
                str_imm(8, 3, 17, 32760),
                0xf93ffe23,
                "str  x3, [x17, #32760]",
            ),
            (ldr_imm(8, 3, SP, 16), 0xf9400be3, "ldr  x3, [sp, #16]"),
        ]);
    }

    #[test]
    fn enc_ldr_str_reg() {
        check(&[
            (ldr_reg(1, 3, 17, 25), 0x38796a23, "ldrb  w3, [x17, x25]"),
            (str_reg(1, 3, 17, 25), 0x38396a23, "strb  w3, [x17, x25]"),
            (ldr_reg(2, 3, 17, 25), 0x78796a23, "ldrh  w3, [x17, x25]"),
            (str_reg(2, 3, 17, 25), 0x78396a23, "strh  w3, [x17, x25]"),
            (ldr_reg(4, 3, 17, 25), 0xb8796a23, "ldr  w3, [x17, x25]"),
            (str_reg(4, 3, 17, 25), 0xb8396a23, "str  w3, [x17, x25]"),
            (ldr_reg(8, 3, 17, 25), 0xf8796a23, "ldr  x3, [x17, x25]"),
            (str_reg(8, 3, 17, 25), 0xf8396a23, "str  x3, [x17, x25]"),
        ]);
    }

    #[test]
    fn enc_ldrs_imm() {
        check(&[
            (ldrs_imm(1, 3, 17, 0), 0x39800223, "ldrsb x3, [x17, #0]"),
            (ldrs_imm(1, 3, 17, 3), 0x39800e23, "ldrsb x3, [x17, #3]"),
            (
                ldrs_imm(1, 3, 17, 4095),
                0x39bffe23,
                "ldrsb x3, [x17, #4095]",
            ),
            (ldrs_imm(2, 3, 17, 0), 0x79800223, "ldrsh x3, [x17, #0]"),
            (ldrs_imm(2, 3, 17, 6), 0x79800e23, "ldrsh x3, [x17, #6]"),
            (
                ldrs_imm(2, 3, 17, 8190),
                0x79bffe23,
                "ldrsh x3, [x17, #8190]",
            ),
            (ldrs_imm(4, 3, 17, 0), 0xb9800223, "ldrsw x3, [x17, #0]"),
            (ldrs_imm(4, 3, 17, 12), 0xb9800e23, "ldrsw x3, [x17, #12]"),
            (
                ldrs_imm(4, 3, 17, 16380),
                0xb9bffe23,
                "ldrsw x3, [x17, #16380]",
            ),
        ]);
    }

    #[test]
    fn enc_pairs() {
        check(&[
            (ldp(3, 4, 17, 0), 0xa9401223, "ldp x3, x4, [x17, #0]"),
            (stp(3, 4, 17, 0), 0xa9001223, "stp x3, x4, [x17, #0]"),
            (ldp(3, 4, 17, 8), 0xa9409223, "ldp x3, x4, [x17, #8]"),
            (stp(3, 4, 17, 8), 0xa9009223, "stp x3, x4, [x17, #8]"),
            (ldp(3, 4, 17, 504), 0xa95f9223, "ldp x3, x4, [x17, #504]"),
            (stp(3, 4, 17, 504), 0xa91f9223, "stp x3, x4, [x17, #504]"),
            (ldp(3, 4, 17, -8), 0xa97f9223, "ldp x3, x4, [x17, #-8]"),
            (stp(3, 4, 17, -8), 0xa93f9223, "stp x3, x4, [x17, #-8]"),
            (ldp(3, 4, 17, -512), 0xa9601223, "ldp x3, x4, [x17, #-512]"),
            (stp(3, 4, 17, -512), 0xa9201223, "stp x3, x4, [x17, #-512]"),
            (
                stp_pre(29, 30, SP, -16),
                0xa9bf7bfd,
                "stp x29, x30, [sp, #-16]!",
            ),
            (stp_pre(3, 4, 17, 64), 0xa9841223, "stp x3, x4, [x17, #64]!"),
            (
                stp_pre(3, 4, 17, -512),
                0xa9a01223,
                "stp x3, x4, [x17, #-512]!",
            ),
            (
                ldp_post(29, 30, SP, 16),
                0xa8c17bfd,
                "ldp x29, x30, [sp], #16",
            ),
            (
                ldp_post(3, 4, 17, -64),
                0xa8fc1223,
                "ldp x3, x4, [x17], #-64",
            ),
            (
                ldp_post(3, 4, 17, 504),
                0xa8df9223,
                "ldp x3, x4, [x17], #504",
            ),
        ]);
    }

    #[test]
    fn enc_branch_imm26() {
        check(&[
            (b(0), 0x14000000, "b #0"),
            (bl(0), 0x94000000, "bl #0"),
            (b(1), 0x14000001, "b #4"),
            (bl(1), 0x94000001, "bl #4"),
            (b(2), 0x14000002, "b #8"),
            (bl(2), 0x94000002, "bl #8"),
            (b(1000), 0x140003e8, "b #4000"),
            (bl(1000), 0x940003e8, "bl #4000"),
            (b(-1), 0x17ffffff, "b #-4"),
            (bl(-1), 0x97ffffff, "bl #-4"),
            (b(-2), 0x17fffffe, "b #-8"),
            (bl(-2), 0x97fffffe, "bl #-8"),
            (b(-1000), 0x17fffc18, "b #-4000"),
            (bl(-1000), 0x97fffc18, "bl #-4000"),
            (b(33554431), 0x15ffffff, "b #134217724"),
            (bl(33554431), 0x95ffffff, "bl #134217724"),
            (b(-33554432), 0x16000000, "b #-134217728"),
            (bl(-33554432), 0x96000000, "bl #-134217728"),
        ]);
    }

    #[test]
    fn enc_b_cond() {
        check(&[
            (b_cond(Cond::Eq, 0), 0x54000000, "b.eq #0"),
            (b_cond(Cond::Al, 0), 0x5400000e, "b.al #0"),
            (b_cond(Cond::Eq, 1), 0x54000020, "b.eq #4"),
            (b_cond(Cond::Al, 1), 0x5400002e, "b.al #4"),
            (b_cond(Cond::Eq, 3), 0x54000060, "b.eq #12"),
            (b_cond(Cond::Ne, 3), 0x54000061, "b.ne #12"),
            (b_cond(Cond::Hs, 3), 0x54000062, "b.hs #12"),
            (b_cond(Cond::Lo, 3), 0x54000063, "b.lo #12"),
            (b_cond(Cond::Mi, 3), 0x54000064, "b.mi #12"),
            (b_cond(Cond::Pl, 3), 0x54000065, "b.pl #12"),
            (b_cond(Cond::Vs, 3), 0x54000066, "b.vs #12"),
            (b_cond(Cond::Vc, 3), 0x54000067, "b.vc #12"),
            (b_cond(Cond::Hi, 3), 0x54000068, "b.hi #12"),
            (b_cond(Cond::Ls, 3), 0x54000069, "b.ls #12"),
            (b_cond(Cond::Ge, 3), 0x5400006a, "b.ge #12"),
            (b_cond(Cond::Lt, 3), 0x5400006b, "b.lt #12"),
            (b_cond(Cond::Gt, 3), 0x5400006c, "b.gt #12"),
            (b_cond(Cond::Le, 3), 0x5400006d, "b.le #12"),
            (b_cond(Cond::Al, 3), 0x5400006e, "b.al #12"),
            (b_cond(Cond::Eq, 1000), 0x54007d00, "b.eq #4000"),
            (b_cond(Cond::Al, 1000), 0x54007d0e, "b.al #4000"),
            (b_cond(Cond::Eq, -1), 0x54ffffe0, "b.eq #-4"),
            (b_cond(Cond::Al, -1), 0x54ffffee, "b.al #-4"),
            (b_cond(Cond::Eq, -3), 0x54ffffa0, "b.eq #-12"),
            (b_cond(Cond::Ne, -3), 0x54ffffa1, "b.ne #-12"),
            (b_cond(Cond::Hs, -3), 0x54ffffa2, "b.hs #-12"),
            (b_cond(Cond::Lo, -3), 0x54ffffa3, "b.lo #-12"),
            (b_cond(Cond::Mi, -3), 0x54ffffa4, "b.mi #-12"),
            (b_cond(Cond::Pl, -3), 0x54ffffa5, "b.pl #-12"),
            (b_cond(Cond::Vs, -3), 0x54ffffa6, "b.vs #-12"),
            (b_cond(Cond::Vc, -3), 0x54ffffa7, "b.vc #-12"),
            (b_cond(Cond::Hi, -3), 0x54ffffa8, "b.hi #-12"),
            (b_cond(Cond::Ls, -3), 0x54ffffa9, "b.ls #-12"),
            (b_cond(Cond::Ge, -3), 0x54ffffaa, "b.ge #-12"),
            (b_cond(Cond::Lt, -3), 0x54ffffab, "b.lt #-12"),
            (b_cond(Cond::Gt, -3), 0x54ffffac, "b.gt #-12"),
            (b_cond(Cond::Le, -3), 0x54ffffad, "b.le #-12"),
            (b_cond(Cond::Al, -3), 0x54ffffae, "b.al #-12"),
            (b_cond(Cond::Eq, 262143), 0x547fffe0, "b.eq #1048572"),
            (b_cond(Cond::Al, 262143), 0x547fffee, "b.al #1048572"),
            (b_cond(Cond::Eq, -262144), 0x54800000, "b.eq #-1048576"),
            (b_cond(Cond::Al, -262144), 0x5480000e, "b.al #-1048576"),
        ]);
    }

    #[test]
    fn enc_cbz_cbnz() {
        check(&[
            (cbz(true, 17, 0), 0xb4000011, "cbz x17, #0"),
            (cbnz(true, 17, 0), 0xb5000011, "cbnz x17, #0"),
            (cbz(false, 17, 0), 0x34000011, "cbz w17, #0"),
            (cbnz(false, 17, 0), 0x35000011, "cbnz w17, #0"),
            (cbz(true, 17, 1), 0xb4000031, "cbz x17, #4"),
            (cbnz(true, 17, 1), 0xb5000031, "cbnz x17, #4"),
            (cbz(false, 17, 1), 0x34000031, "cbz w17, #4"),
            (cbnz(false, 17, 1), 0x35000031, "cbnz w17, #4"),
            (cbz(true, 17, 3), 0xb4000071, "cbz x17, #12"),
            (cbnz(true, 17, 3), 0xb5000071, "cbnz x17, #12"),
            (cbz(false, 17, 3), 0x34000071, "cbz w17, #12"),
            (cbnz(false, 17, 3), 0x35000071, "cbnz w17, #12"),
            (cbz(true, 17, 1000), 0xb4007d11, "cbz x17, #4000"),
            (cbnz(true, 17, 1000), 0xb5007d11, "cbnz x17, #4000"),
            (cbz(false, 17, 1000), 0x34007d11, "cbz w17, #4000"),
            (cbnz(false, 17, 1000), 0x35007d11, "cbnz w17, #4000"),
            (cbz(true, 17, -1), 0xb4fffff1, "cbz x17, #-4"),
            (cbnz(true, 17, -1), 0xb5fffff1, "cbnz x17, #-4"),
            (cbz(false, 17, -1), 0x34fffff1, "cbz w17, #-4"),
            (cbnz(false, 17, -1), 0x35fffff1, "cbnz w17, #-4"),
            (cbz(true, 17, -3), 0xb4ffffb1, "cbz x17, #-12"),
            (cbnz(true, 17, -3), 0xb5ffffb1, "cbnz x17, #-12"),
            (cbz(false, 17, -3), 0x34ffffb1, "cbz w17, #-12"),
            (cbnz(false, 17, -3), 0x35ffffb1, "cbnz w17, #-12"),
            (cbz(true, 17, 262143), 0xb47ffff1, "cbz x17, #1048572"),
            (cbnz(true, 17, 262143), 0xb57ffff1, "cbnz x17, #1048572"),
            (cbz(false, 17, 262143), 0x347ffff1, "cbz w17, #1048572"),
            (cbnz(false, 17, 262143), 0x357ffff1, "cbnz w17, #1048572"),
            (cbz(true, 17, -262144), 0xb4800011, "cbz x17, #-1048576"),
            (cbnz(true, 17, -262144), 0xb5800011, "cbnz x17, #-1048576"),
            (cbz(false, 17, -262144), 0x34800011, "cbz w17, #-1048576"),
            (cbnz(false, 17, -262144), 0x35800011, "cbnz w17, #-1048576"),
        ]);
    }

    #[test]
    fn enc_tbz_tbnz() {
        check(&[
            (tbz(17, 0, 0), 0x36000011, "tbz w17, #0, #0"),
            (tbnz(17, 0, 0), 0x37000011, "tbnz w17, #0, #0"),
            (tbz(17, 5, 0), 0x36280011, "tbz w17, #5, #0"),
            (tbnz(17, 5, 0), 0x37280011, "tbnz w17, #5, #0"),
            (tbz(17, 31, 0), 0x36f80011, "tbz w17, #31, #0"),
            (tbnz(17, 31, 0), 0x37f80011, "tbnz w17, #31, #0"),
            (tbz(17, 32, 0), 0xb6000011, "tbz x17, #32, #0"),
            (tbnz(17, 32, 0), 0xb7000011, "tbnz x17, #32, #0"),
            (tbz(17, 40, 0), 0xb6400011, "tbz x17, #40, #0"),
            (tbnz(17, 40, 0), 0xb7400011, "tbnz x17, #40, #0"),
            (tbz(17, 63, 0), 0xb6f80011, "tbz x17, #63, #0"),
            (tbnz(17, 63, 0), 0xb7f80011, "tbnz x17, #63, #0"),
            (tbz(17, 0, 2), 0x36000051, "tbz w17, #0, #8"),
            (tbnz(17, 0, 2), 0x37000051, "tbnz w17, #0, #8"),
            (tbz(17, 5, 2), 0x36280051, "tbz w17, #5, #8"),
            (tbnz(17, 5, 2), 0x37280051, "tbnz w17, #5, #8"),
            (tbz(17, 31, 2), 0x36f80051, "tbz w17, #31, #8"),
            (tbnz(17, 31, 2), 0x37f80051, "tbnz w17, #31, #8"),
            (tbz(17, 32, 2), 0xb6000051, "tbz x17, #32, #8"),
            (tbnz(17, 32, 2), 0xb7000051, "tbnz x17, #32, #8"),
            (tbz(17, 40, 2), 0xb6400051, "tbz x17, #40, #8"),
            (tbnz(17, 40, 2), 0xb7400051, "tbnz x17, #40, #8"),
            (tbz(17, 63, 2), 0xb6f80051, "tbz x17, #63, #8"),
            (tbnz(17, 63, 2), 0xb7f80051, "tbnz x17, #63, #8"),
            (tbz(17, 0, -2), 0x3607ffd1, "tbz w17, #0, #-8"),
            (tbnz(17, 0, -2), 0x3707ffd1, "tbnz w17, #0, #-8"),
            (tbz(17, 5, -2), 0x362fffd1, "tbz w17, #5, #-8"),
            (tbnz(17, 5, -2), 0x372fffd1, "tbnz w17, #5, #-8"),
            (tbz(17, 31, -2), 0x36ffffd1, "tbz w17, #31, #-8"),
            (tbnz(17, 31, -2), 0x37ffffd1, "tbnz w17, #31, #-8"),
            (tbz(17, 32, -2), 0xb607ffd1, "tbz x17, #32, #-8"),
            (tbnz(17, 32, -2), 0xb707ffd1, "tbnz x17, #32, #-8"),
            (tbz(17, 40, -2), 0xb647ffd1, "tbz x17, #40, #-8"),
            (tbnz(17, 40, -2), 0xb747ffd1, "tbnz x17, #40, #-8"),
            (tbz(17, 63, -2), 0xb6ffffd1, "tbz x17, #63, #-8"),
            (tbnz(17, 63, -2), 0xb7ffffd1, "tbnz x17, #63, #-8"),
            (tbz(17, 0, 8191), 0x3603fff1, "tbz w17, #0, #32764"),
            (tbnz(17, 0, 8191), 0x3703fff1, "tbnz w17, #0, #32764"),
            (tbz(17, 5, 8191), 0x362bfff1, "tbz w17, #5, #32764"),
            (tbnz(17, 5, 8191), 0x372bfff1, "tbnz w17, #5, #32764"),
            (tbz(17, 31, 8191), 0x36fbfff1, "tbz w17, #31, #32764"),
            (tbnz(17, 31, 8191), 0x37fbfff1, "tbnz w17, #31, #32764"),
            (tbz(17, 32, 8191), 0xb603fff1, "tbz x17, #32, #32764"),
            (tbnz(17, 32, 8191), 0xb703fff1, "tbnz x17, #32, #32764"),
            (tbz(17, 40, 8191), 0xb643fff1, "tbz x17, #40, #32764"),
            (tbnz(17, 40, 8191), 0xb743fff1, "tbnz x17, #40, #32764"),
            (tbz(17, 63, 8191), 0xb6fbfff1, "tbz x17, #63, #32764"),
            (tbnz(17, 63, 8191), 0xb7fbfff1, "tbnz x17, #63, #32764"),
            (tbz(17, 0, -8192), 0x36040011, "tbz w17, #0, #-32768"),
            (tbnz(17, 0, -8192), 0x37040011, "tbnz w17, #0, #-32768"),
            (tbz(17, 5, -8192), 0x362c0011, "tbz w17, #5, #-32768"),
            (tbnz(17, 5, -8192), 0x372c0011, "tbnz w17, #5, #-32768"),
            (tbz(17, 31, -8192), 0x36fc0011, "tbz w17, #31, #-32768"),
            (tbnz(17, 31, -8192), 0x37fc0011, "tbnz w17, #31, #-32768"),
            (tbz(17, 32, -8192), 0xb6040011, "tbz x17, #32, #-32768"),
            (tbnz(17, 32, -8192), 0xb7040011, "tbnz x17, #32, #-32768"),
            (tbz(17, 40, -8192), 0xb6440011, "tbz x17, #40, #-32768"),
            (tbnz(17, 40, -8192), 0xb7440011, "tbnz x17, #40, #-32768"),
            (tbz(17, 63, -8192), 0xb6fc0011, "tbz x17, #63, #-32768"),
            (tbnz(17, 63, -8192), 0xb7fc0011, "tbnz x17, #63, #-32768"),
        ]);
    }

    #[test]
    fn enc_branch_reg() {
        check(&[
            (br(17), 0xd61f0220, "br x17"),
            (br(30), 0xd61f03c0, "br x30"),
            (blr(17), 0xd63f0220, "blr x17"),
            (ret(), 0xd65f03c0, "ret"),
            (nop(), 0xd503201f, "nop"),
        ]);
    }

    #[test]
    fn enc_csel_csinc() {
        check(&[
            (
                csel(true, 3, 17, 25, Cond::Eq),
                0x9a990223,
                "csel x3, x17, x25, eq",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Eq),
                0x9a990623,
                "csinc x3, x17, x25, eq",
            ),
            (
                csel(true, 3, 17, 25, Cond::Ne),
                0x9a991223,
                "csel x3, x17, x25, ne",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Ne),
                0x9a991623,
                "csinc x3, x17, x25, ne",
            ),
            (
                csel(true, 3, 17, 25, Cond::Hs),
                0x9a992223,
                "csel x3, x17, x25, hs",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Hs),
                0x9a992623,
                "csinc x3, x17, x25, hs",
            ),
            (
                csel(true, 3, 17, 25, Cond::Lo),
                0x9a993223,
                "csel x3, x17, x25, lo",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Lo),
                0x9a993623,
                "csinc x3, x17, x25, lo",
            ),
            (
                csel(true, 3, 17, 25, Cond::Mi),
                0x9a994223,
                "csel x3, x17, x25, mi",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Mi),
                0x9a994623,
                "csinc x3, x17, x25, mi",
            ),
            (
                csel(true, 3, 17, 25, Cond::Pl),
                0x9a995223,
                "csel x3, x17, x25, pl",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Pl),
                0x9a995623,
                "csinc x3, x17, x25, pl",
            ),
            (
                csel(true, 3, 17, 25, Cond::Vs),
                0x9a996223,
                "csel x3, x17, x25, vs",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Vs),
                0x9a996623,
                "csinc x3, x17, x25, vs",
            ),
            (
                csel(true, 3, 17, 25, Cond::Vc),
                0x9a997223,
                "csel x3, x17, x25, vc",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Vc),
                0x9a997623,
                "csinc x3, x17, x25, vc",
            ),
            (
                csel(true, 3, 17, 25, Cond::Hi),
                0x9a998223,
                "csel x3, x17, x25, hi",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Hi),
                0x9a998623,
                "csinc x3, x17, x25, hi",
            ),
            (
                csel(true, 3, 17, 25, Cond::Ls),
                0x9a999223,
                "csel x3, x17, x25, ls",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Ls),
                0x9a999623,
                "csinc x3, x17, x25, ls",
            ),
            (
                csel(true, 3, 17, 25, Cond::Ge),
                0x9a99a223,
                "csel x3, x17, x25, ge",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Ge),
                0x9a99a623,
                "csinc x3, x17, x25, ge",
            ),
            (
                csel(true, 3, 17, 25, Cond::Lt),
                0x9a99b223,
                "csel x3, x17, x25, lt",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Lt),
                0x9a99b623,
                "csinc x3, x17, x25, lt",
            ),
            (
                csel(true, 3, 17, 25, Cond::Gt),
                0x9a99c223,
                "csel x3, x17, x25, gt",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Gt),
                0x9a99c623,
                "csinc x3, x17, x25, gt",
            ),
            (
                csel(true, 3, 17, 25, Cond::Le),
                0x9a99d223,
                "csel x3, x17, x25, le",
            ),
            (
                csinc(true, 3, 17, 25, Cond::Le),
                0x9a99d623,
                "csinc x3, x17, x25, le",
            ),
            (
                csel(true, 3, 17, 25, Cond::Al),
                0x9a99e223,
                "csel x3, x17, x25, al",
            ),
            (
                csel(false, 3, 17, 25, Cond::Eq),
                0x1a990223,
                "csel w3, w17, w25, eq",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Eq),
                0x1a990623,
                "csinc w3, w17, w25, eq",
            ),
            (
                csel(false, 3, 17, 25, Cond::Ne),
                0x1a991223,
                "csel w3, w17, w25, ne",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Ne),
                0x1a991623,
                "csinc w3, w17, w25, ne",
            ),
            (
                csel(false, 3, 17, 25, Cond::Hs),
                0x1a992223,
                "csel w3, w17, w25, hs",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Hs),
                0x1a992623,
                "csinc w3, w17, w25, hs",
            ),
            (
                csel(false, 3, 17, 25, Cond::Lo),
                0x1a993223,
                "csel w3, w17, w25, lo",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Lo),
                0x1a993623,
                "csinc w3, w17, w25, lo",
            ),
            (
                csel(false, 3, 17, 25, Cond::Mi),
                0x1a994223,
                "csel w3, w17, w25, mi",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Mi),
                0x1a994623,
                "csinc w3, w17, w25, mi",
            ),
            (
                csel(false, 3, 17, 25, Cond::Pl),
                0x1a995223,
                "csel w3, w17, w25, pl",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Pl),
                0x1a995623,
                "csinc w3, w17, w25, pl",
            ),
            (
                csel(false, 3, 17, 25, Cond::Vs),
                0x1a996223,
                "csel w3, w17, w25, vs",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Vs),
                0x1a996623,
                "csinc w3, w17, w25, vs",
            ),
            (
                csel(false, 3, 17, 25, Cond::Vc),
                0x1a997223,
                "csel w3, w17, w25, vc",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Vc),
                0x1a997623,
                "csinc w3, w17, w25, vc",
            ),
            (
                csel(false, 3, 17, 25, Cond::Hi),
                0x1a998223,
                "csel w3, w17, w25, hi",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Hi),
                0x1a998623,
                "csinc w3, w17, w25, hi",
            ),
            (
                csel(false, 3, 17, 25, Cond::Ls),
                0x1a999223,
                "csel w3, w17, w25, ls",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Ls),
                0x1a999623,
                "csinc w3, w17, w25, ls",
            ),
            (
                csel(false, 3, 17, 25, Cond::Ge),
                0x1a99a223,
                "csel w3, w17, w25, ge",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Ge),
                0x1a99a623,
                "csinc w3, w17, w25, ge",
            ),
            (
                csel(false, 3, 17, 25, Cond::Lt),
                0x1a99b223,
                "csel w3, w17, w25, lt",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Lt),
                0x1a99b623,
                "csinc w3, w17, w25, lt",
            ),
            (
                csel(false, 3, 17, 25, Cond::Gt),
                0x1a99c223,
                "csel w3, w17, w25, gt",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Gt),
                0x1a99c623,
                "csinc w3, w17, w25, gt",
            ),
            (
                csel(false, 3, 17, 25, Cond::Le),
                0x1a99d223,
                "csel w3, w17, w25, le",
            ),
            (
                csinc(false, 3, 17, 25, Cond::Le),
                0x1a99d623,
                "csinc w3, w17, w25, le",
            ),
            (
                csel(false, 3, 17, 25, Cond::Al),
                0x1a99e223,
                "csel w3, w17, w25, al",
            ),
        ]);
    }

    #[test]
    fn enc_cset() {
        check(&[
            (cset(true, 3, Cond::Eq), 0x9a9f17e3, "cset x3, eq"),
            (cset(true, 3, Cond::Ne), 0x9a9f07e3, "cset x3, ne"),
            (cset(true, 3, Cond::Hs), 0x9a9f37e3, "cset x3, hs"),
            (cset(true, 3, Cond::Lo), 0x9a9f27e3, "cset x3, lo"),
            (cset(true, 3, Cond::Mi), 0x9a9f57e3, "cset x3, mi"),
            (cset(true, 3, Cond::Pl), 0x9a9f47e3, "cset x3, pl"),
            (cset(true, 3, Cond::Vs), 0x9a9f77e3, "cset x3, vs"),
            (cset(true, 3, Cond::Vc), 0x9a9f67e3, "cset x3, vc"),
            (cset(true, 3, Cond::Hi), 0x9a9f97e3, "cset x3, hi"),
            (cset(true, 3, Cond::Ls), 0x9a9f87e3, "cset x3, ls"),
            (cset(true, 3, Cond::Ge), 0x9a9fb7e3, "cset x3, ge"),
            (cset(true, 3, Cond::Lt), 0x9a9fa7e3, "cset x3, lt"),
            (cset(true, 3, Cond::Gt), 0x9a9fd7e3, "cset x3, gt"),
            (cset(true, 3, Cond::Le), 0x9a9fc7e3, "cset x3, le"),
            (cset(false, 3, Cond::Eq), 0x1a9f17e3, "cset w3, eq"),
            (cset(false, 3, Cond::Ne), 0x1a9f07e3, "cset w3, ne"),
            (cset(false, 3, Cond::Hs), 0x1a9f37e3, "cset w3, hs"),
            (cset(false, 3, Cond::Lo), 0x1a9f27e3, "cset w3, lo"),
            (cset(false, 3, Cond::Mi), 0x1a9f57e3, "cset w3, mi"),
            (cset(false, 3, Cond::Pl), 0x1a9f47e3, "cset w3, pl"),
            (cset(false, 3, Cond::Vs), 0x1a9f77e3, "cset w3, vs"),
            (cset(false, 3, Cond::Vc), 0x1a9f67e3, "cset w3, vc"),
            (cset(false, 3, Cond::Hi), 0x1a9f97e3, "cset w3, hi"),
            (cset(false, 3, Cond::Ls), 0x1a9f87e3, "cset w3, ls"),
            (cset(false, 3, Cond::Ge), 0x1a9fb7e3, "cset w3, ge"),
            (cset(false, 3, Cond::Lt), 0x1a9fa7e3, "cset w3, lt"),
            (cset(false, 3, Cond::Gt), 0x1a9fd7e3, "cset w3, gt"),
            (cset(false, 3, Cond::Le), 0x1a9fc7e3, "cset w3, le"),
        ]);
    }

    #[test]
    fn enc_flags() {
        check(&[
            (mrs_nzcv(3), 0xd53b4203, "mrs x3, nzcv"),
            (mrs_nzcv(30), 0xd53b421e, "mrs x30, nzcv"),
            (msr_nzcv(3), 0xd51b4203, "msr nzcv, x3"),
            (msr_nzcv(30), 0xd51b421e, "msr nzcv, x30"),
            (cfinv(), 0xd500401f, "cfinv"),
            (rmif(17, 0, 0), 0xba000620, "rmif x17, #0, #0"),
            (rmif(17, 0, 1), 0xba000621, "rmif x17, #0, #1"),
            (rmif(17, 0, 5), 0xba000625, "rmif x17, #0, #5"),
            (rmif(17, 0, 8), 0xba000628, "rmif x17, #0, #8"),
            (rmif(17, 0, 15), 0xba00062f, "rmif x17, #0, #15"),
            (rmif(17, 1, 0), 0xba008620, "rmif x17, #1, #0"),
            (rmif(17, 1, 1), 0xba008621, "rmif x17, #1, #1"),
            (rmif(17, 1, 5), 0xba008625, "rmif x17, #1, #5"),
            (rmif(17, 1, 8), 0xba008628, "rmif x17, #1, #8"),
            (rmif(17, 1, 15), 0xba00862f, "rmif x17, #1, #15"),
            (rmif(17, 7, 0), 0xba038620, "rmif x17, #7, #0"),
            (rmif(17, 7, 1), 0xba038621, "rmif x17, #7, #1"),
            (rmif(17, 7, 5), 0xba038625, "rmif x17, #7, #5"),
            (rmif(17, 7, 8), 0xba038628, "rmif x17, #7, #8"),
            (rmif(17, 7, 15), 0xba03862f, "rmif x17, #7, #15"),
            (rmif(17, 63, 0), 0xba1f8620, "rmif x17, #63, #0"),
            (rmif(17, 63, 1), 0xba1f8621, "rmif x17, #63, #1"),
            (rmif(17, 63, 5), 0xba1f8625, "rmif x17, #63, #5"),
            (rmif(17, 63, 8), 0xba1f8628, "rmif x17, #63, #8"),
            (rmif(17, 63, 15), 0xba1f862f, "rmif x17, #63, #15"),
            (rmif(0, 3, 15), 0xba01840f, "rmif x0, #3, #15"),
            (setf8(3), 0x3a00086d, "setf8 w3"),
            (setf8(30), 0x3a000bcd, "setf8 w30"),
            (setf16(3), 0x3a00486d, "setf16 w3"),
            (setf16(30), 0x3a004bcd, "setf16 w30"),
        ]);
    }

    #[test]
    fn enc_ccmp() {
        check(&[
            (
                ccmp_imm(true, 17, 5, 4, Cond::Eq),
                0xfa450a24,
                "ccmp x17, #5, #4, eq",
            ),
            (
                ccmp_imm(true, 17, 5, 4, Cond::Hs),
                0xfa452a24,
                "ccmp x17, #5, #4, hs",
            ),
            (
                ccmp_imm(true, 17, 5, 4, Cond::Lt),
                0xfa45ba24,
                "ccmp x17, #5, #4, lt",
            ),
            (
                ccmp_imm(true, 17, 5, 4, Cond::Gt),
                0xfa45ca24,
                "ccmp x17, #5, #4, gt",
            ),
            (
                ccmp_imm(true, 3, 31, 15, Cond::Ne),
                0xfa5f186f,
                "ccmp x3, #31, #15, ne",
            ),
            (
                ccmp_imm(true, 3, 0, 0, Cond::Hs),
                0xfa402860,
                "ccmp x3, #0, #0, hs",
            ),
            (
                ccmp_imm(false, 17, 5, 4, Cond::Eq),
                0x7a450a24,
                "ccmp w17, #5, #4, eq",
            ),
            (
                ccmp_imm(false, 17, 5, 4, Cond::Hs),
                0x7a452a24,
                "ccmp w17, #5, #4, hs",
            ),
            (
                ccmp_imm(false, 17, 5, 4, Cond::Lt),
                0x7a45ba24,
                "ccmp w17, #5, #4, lt",
            ),
            (
                ccmp_imm(false, 17, 5, 4, Cond::Gt),
                0x7a45ca24,
                "ccmp w17, #5, #4, gt",
            ),
            (
                ccmp_imm(false, 3, 31, 15, Cond::Ne),
                0x7a5f186f,
                "ccmp w3, #31, #15, ne",
            ),
            (
                ccmp_imm(false, 3, 0, 0, Cond::Hs),
                0x7a402860,
                "ccmp w3, #0, #0, hs",
            ),
        ]);
    }

    #[test]
    fn encode_bitmask_matches_assembler() {
        // (value, x64, assembler word of `and Rd3, Rn17, #value`): decode (N, immr, imms) back.
        let cases: &[(u64, bool, u32)] = &[
            (0xff, true, 0x92401e23),               // and x3, x17, #0xff
            (0x1, true, 0x92400223),                // and x3, x17, #0x1
            (0x8000000000000000, true, 0x92410223), // and x3, x17, #0x8000000000000000
            (0x7fffffffffffffff, true, 0x9240fa23), // and x3, x17, #0x7fffffffffffffff
            (0xfffffffffffffffe, true, 0x927ffa23), // and x3, x17, #0xfffffffffffffffe
            (0xf0f0f0f0f0f0f0f0, true, 0x9204ce23), // and x3, x17, #0xf0f0f0f0f0f0f0f0
            (0x5555555555555555, true, 0x9200f223), // and x3, x17, #0x5555555555555555
            (0xaaaaaaaaaaaaaaaa, true, 0x9201f223), // and x3, x17, #0xaaaaaaaaaaaaaaaa
            (0xff00ff00ff00ff, true, 0x92009e23),   // and x3, x17, #0xff00ff00ff00ff
            (0x3000300030003, true, 0x92008623),    // and x3, x17, #0x3000300030003
            (0x1ffff, true, 0x92404223),            // and x3, x17, #0x1ffff
            (0xffff0000ffff0000, true, 0x92103e23), // and x3, x17, #0xffff0000ffff0000
            (0xffff0000, true, 0x92703e23),         // and x3, x17, #0xffff0000
            (0xffff00000000, true, 0x92603e23),     // and x3, x17, #0xffff00000000
            (0x7ffffffe, true, 0x927f7623),         // and x3, x17, #0x7ffffffe
            (0x6, true, 0x927f0623),                // and x3, x17, #0x6
            (0xc000000000000003, true, 0x92420e23), // and x3, x17, #0xc000000000000003
            (0x3333333333333333, true, 0x9200e623), // and x3, x17, #0x3333333333333333
            (0x101010101010101, true, 0x9200c223),  // and x3, x17, #0x101010101010101
            (0xfefefefefefefefe, true, 0x9207da23), // and x3, x17, #0xfefefefefefefefe
            (0x100000001, true, 0x92000223),        // and x3, x17, #0x100000001
            (0xfff00000fff, true, 0x92002e23),      // and x3, x17, #0xfff00000fff
            (0xfffffffffffffff0, true, 0x927cee23), // and x3, x17, #0xfffffffffffffff0
            (0xfffffffe, true, 0x927f7a23),         // and x3, x17, #0xfffffffe
            (0xe0e0e0e0e0e0e0e0, true, 0x9203ca23), // and x3, x17, #0xe0e0e0e0e0e0e0e0
            (0x6666666666666666, true, 0x9203e623), // and x3, x17, #0x6666666666666666
            (0xff, false, 0x12001e23),              // and w3, w17, #0xff
            (0x1, false, 0x12000223),               // and w3, w17, #0x1
            (0x80000000, false, 0x12010223),        // and w3, w17, #0x80000000
            (0x7fffffff, false, 0x12007a23),        // and w3, w17, #0x7fffffff
            (0xfffffffe, false, 0x121f7a23),        // and w3, w17, #0xfffffffe
            (0xf0f0f0f0, false, 0x1204ce23),        // and w3, w17, #0xf0f0f0f0
            (0x55555555, false, 0x1200f223),        // and w3, w17, #0x55555555
            (0xaaaaaaaa, false, 0x1201f223),        // and w3, w17, #0xaaaaaaaa
            (0xff00ff, false, 0x12009e23),          // and w3, w17, #0xff00ff
            (0x30003, false, 0x12008623),           // and w3, w17, #0x30003
            (0x1ffff, false, 0x12004223),           // and w3, w17, #0x1ffff
            (0xffff0000, false, 0x12103e23),        // and w3, w17, #0xffff0000
            (0x7ffffffe, false, 0x121f7623),        // and w3, w17, #0x7ffffffe
            (0x80000001, false, 0x12010623),        // and w3, w17, #0x80000001
            (0xc0000003, false, 0x12020e23),        // and w3, w17, #0xc0000003
            (0x33333333, false, 0x1200e623),        // and w3, w17, #0x33333333
            (0x1010101, false, 0x1200c223),         // and w3, w17, #0x1010101
            (0xfefefefe, false, 0x1207da23),        // and w3, w17, #0xfefefefe
            (0xffff, false, 0x12003e23),            // and w3, w17, #0xffff
            (0xfffffff0, false, 0x121c6e23),        // and w3, w17, #0xfffffff0
            (0xe0e0e0e0, false, 0x1203ca23),        // and w3, w17, #0xe0e0e0e0
            (0x66666666, false, 0x1203e623),        // and w3, w17, #0x66666666
            (0xff00ff0, false, 0x120c9e23),         // and w3, w17, #0xff00ff0
            (0x3fffffff, false, 0x12007623),        // and w3, w17, #0x3fffffff
        ];
        for &(value, x64, word) in cases {
            let (n, immr, imms) = encode_bitmask(value, x64)
                .unwrap_or_else(|| panic!("{value:#x} (x64={x64}) must be encodable"));
            let rebuilt =
                0x1200_0000 | sf(x64) | (n << 22) | (immr << 16) | (imms << 10) | (17 << 5) | 3;
            assert_eq!(
                rebuilt, word,
                "{value:#x} (x64={x64}): n={n} immr={immr} imms={imms}"
            );
        }
    }

    #[test]
    fn encode_bitmask_known_fields() {
        // Spot checks of the raw (N, immr, imms) triple taken from the assembler words above.
        assert_eq!(encode_bitmask(0xff, true), Some((1, 0, 7)));
        assert_eq!(encode_bitmask(0xff, false), Some((0, 0, 7)));
        assert_eq!(
            encode_bitmask(0x5555_5555_5555_5555, true),
            Some((0, 0, 0b11_1100))
        );
        assert_eq!(encode_bitmask(0x8000_0000_0000_0000, true), Some((1, 1, 0)));
    }

    #[test]
    fn encode_bitmask_rejects_unencodable() {
        // The assembler rejects every one of these as a logical immediate ("expected compatible
        // register or logical immediate").
        for &(value, x64) in &[
            (0u64, true),
            (u64::MAX, true),
            (0x5, true),
            (0x123_4567, true),
            (0x8000_0001, true),
            (0xfc3f_c3fc_3fc3_fc3f, true),
            (0, false),
            (0xffff_ffff, false),
            (0x5, false),
            (0x1234_5678, false),
            (0xfc3f_c3fc, false),
        ] {
            assert_eq!(encode_bitmask(value, x64), None, "{value:#x} (x64={x64})");
        }
        // For the 32-bit form only the low word counts: the high word is ignored, so a value
        // whose low word is 0 is not encodable and one whose low word is a mask equals the plain mask.
        assert_eq!(encode_bitmask(0x1_0000_0000, false), None);
        assert_eq!(
            encode_bitmask(0xdead_beef_0000_00ff, false),
            encode_bitmask(0xff, false)
        );
    }

    #[test]
    fn logical_imm_rejects_unencodable() {
        assert_eq!(and_imm(true, 3, 17, 0), None);
        assert_eq!(orr_imm(true, 3, 17, u64::MAX), None);
        assert_eq!(eor_imm(false, 3, 17, 0x1234_5678), None);
        assert_eq!(ands_imm(true, 3, 17, 0x123_4567), None);
    }

    #[test]
    fn cond_invert_roundtrip() {
        // `cset` is `csinc ..., invert(cond)`, which the assembler words in `cset` verify; here the
        // algebra: inverting twice is the identity and flips only the low bit (except AL).
        for code in 0..14u32 {
            let c = [
                Cond::Eq,
                Cond::Ne,
                Cond::Hs,
                Cond::Lo,
                Cond::Mi,
                Cond::Pl,
                Cond::Vs,
                Cond::Vc,
                Cond::Hi,
                Cond::Ls,
                Cond::Ge,
                Cond::Lt,
                Cond::Gt,
                Cond::Le,
            ][code as usize];
            assert_eq!(c as u32, code);
            assert_eq!(c.invert() as u32, code ^ 1, "{c:?}");
            assert_eq!(c.invert().invert(), c);
        }
        assert_eq!(Cond::Al.invert(), Cond::Al);
    }

    #[test]
    fn asm_mov64_0() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0x0);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd2800000, "movz x0, #0")]);
    }

    #[test]
    fn asm_mov64_1() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0x1234);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd2824680, "movz x0, #0x1234")]);
    }

    #[test]
    fn asm_mov64_2() {
        let mut a = Asm::new();
        a.mov_imm64(20, 0x12340000);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd2a24694, "movz x20, #0x1234, lsl #16")]);
    }

    #[test]
    fn asm_mov64_3() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0x100000000);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd2c00020, "movz x0, #1, lsl #32")]);
    }

    #[test]
    fn asm_mov64_4() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0x8000000000000000);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd2f00000, "movz x0, #0x8000, lsl #48")]);
    }

    #[test]
    fn asm_mov64_5() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xffffffffffffffff);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x92800000, "movn x0, #0")]);
    }

    #[test]
    fn asm_mov64_6() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xffffffffffff1234);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x929db960, "movn x0, #0xedcb")]);
    }

    #[test]
    fn asm_mov64_7() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xffffffff1234ffff);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x92bdb960, "movn x0, #0xedcb, lsl #16")]);
    }

    #[test]
    fn asm_mov64_8() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xffff0000ffff1234);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x929db960, "movn x0, #0xedcb"),
                (0xf2c00000, "movk x0, #0, lsl #32"),
            ],
        );
    }

    #[test]
    fn asm_mov64_9() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xffff00001234);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd2824680, "movz x0, #0x1234"),
                (0xf2dfffe0, "movk x0, #0xffff, lsl #32"),
            ],
        );
    }

    #[test]
    fn asm_mov64_10() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0xdeadbeefffffffff);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x92c82200, "movn x0, #0x4110, lsl #32"),
                (0xf2fbd5a0, "movk x0, #0xdead, lsl #48"),
            ],
        );
    }

    #[test]
    fn asm_mov64_11() {
        let mut a = Asm::new();
        a.mov_imm64(9, 0x123456789abcdef0);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd29bde09, "movz x9, #0xdef0"),
                (0xf2b35789, "movk x9, #0x9abc, lsl #16"),
                (0xf2cacf09, "movk x9, #0x5678, lsl #32"),
                (0xf2e24689, "movk x9, #0x1234, lsl #48"),
            ],
        );
    }

    #[test]
    fn asm_mov64_12() {
        let mut a = Asm::new();
        a.mov_imm64(0, 0x100000001);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd2800020, "movz x0, #1"),
                (0xf2c00020, "movk x0, #1, lsl #32"),
            ],
        );
    }

    #[test]
    fn asm_mov32_0() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0x0);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x52800000, "movz w0, #0")]);
    }

    #[test]
    fn asm_mov32_1() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0x1234);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x52824680, "movz w0, #0x1234")]);
    }

    #[test]
    fn asm_mov32_2() {
        let mut a = Asm::new();
        a.mov_imm32(5, 0x12340000);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x52a24685, "movz w5, #0x1234, lsl #16")]);
    }

    #[test]
    fn asm_mov32_3() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0xffff1234);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x129db960, "movn w0, #0xedcb")]);
    }

    #[test]
    fn asm_mov32_4() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0x12345678);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x528acf00, "movz w0, #0x5678"),
                (0x72a24680, "movk w0, #0x1234, lsl #16"),
            ],
        );
    }

    #[test]
    fn asm_mov32_5() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0xffffffff);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x12800000, "movn w0, #0")]);
    }

    #[test]
    fn asm_mov32_6() {
        let mut a = Asm::new();
        a.mov_imm32(0, 0xffff0000);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x52bfffe0, "movz w0, #0xffff, lsl #16")]);
    }

    #[test]
    fn asm_add_any_0() {
        let mut a = Asm::new();
        a.add_any(1, 1, 0, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[]);
    }

    #[test]
    fn asm_add_any_1() {
        let mut a = Asm::new();
        a.add_any(1, 2, 0, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xaa0203e1, "mov x1, x2")]);
    }

    #[test]
    fn asm_add_any_2() {
        let mut a = Asm::new();
        a.add_any(1, 2, 5, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x91001441, "add x1, x2, #5")]);
    }

    #[test]
    fn asm_add_any_3() {
        let mut a = Asm::new();
        a.add_any(1, 2, 4095, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x913ffc41, "add x1, x2, #4095")]);
    }

    #[test]
    fn asm_add_any_4() {
        let mut a = Asm::new();
        a.add_any(1, 2, -5, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd1001441, "sub x1, x2, #5")]);
    }

    #[test]
    fn asm_add_any_5() {
        let mut a = Asm::new();
        a.add_any(1, 2, -4095, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd13ffc41, "sub x1, x2, #4095")]);
    }

    #[test]
    fn asm_add_any_6() {
        let mut a = Asm::new();
        a.add_any(1, 2, 12288, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x91400c41, "add x1, x2, #3, lsl #12")]);
    }

    #[test]
    fn asm_add_any_7() {
        let mut a = Asm::new();
        a.add_any(1, 2, 4096, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0x91400441, "add x1, x2, #1, lsl #12")]);
    }

    #[test]
    fn asm_add_any_8() {
        let mut a = Asm::new();
        a.add_any(1, 2, -12288, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd1400c41, "sub x1, x2, #3, lsl #12")]);
    }

    #[test]
    fn asm_add_any_9() {
        let mut a = Asm::new();
        a.add_any(1, 2, -4096, 9);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd1400441, "sub x1, x2, #1, lsl #12")]);
    }

    #[test]
    fn asm_add_any_10() {
        let mut a = Asm::new();
        a.add_any(1, 2, 1193046, 9);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd2868ac9, "movz x9, #0x3456"),
                (0xf2a00249, "movk x9, #0x12, lsl #16"),
                (0x8b090041, "add x1, x2, x9"),
            ],
        );
    }

    #[test]
    fn asm_add_any_11() {
        let mut a = Asm::new();
        a.add_any(1, 2, 16777216, 9);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd2a02009, "movz x9, #0x100, lsl #16"),
                (0x8b090041, "add x1, x2, x9"),
            ],
        );
    }

    #[test]
    fn asm_add_any_12() {
        let mut a = Asm::new();
        a.add_any(1, 2, -1193046, 9);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x92868aa9, "movn x9, #0x3455"),
                (0xf2bffda9, "movk x9, #0xffed, lsl #16"),
                (0x8b090041, "add x1, x2, x9"),
            ],
        );
    }

    #[test]
    fn asm_add_any_13() {
        let mut a = Asm::new();
        a.add_any(1, 2, 4294967296, 9);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd2c00029, "movz x9, #1, lsl #32"),
                (0x8b090041, "add x1, x2, x9"),
            ],
        );
    }

    #[test]
    fn asm_labels_forward_backward() {
        let mut a = Asm::new();
        let start = a.label();
        let fwd = a.label();
        a.bind(start);
        a.emit(nop());
        a.b_to(fwd);
        a.emit(nop());
        a.b_cond_to(Cond::Eq, fwd);
        a.cbz_to(true, 1, fwd);
        a.cbnz_to(false, 2, fwd);
        a.tbz_to(3, 4, fwd);
        a.tbnz_to(5, 40, fwd);
        a.bind(fwd);
        a.emit(nop());
        a.b_to(start);
        a.b_cond_to(Cond::Ne, start);
        a.cbz_to(false, 6, start);
        a.cbnz_to(true, 7, start);
        a.tbz_to(8, 5, start);
        a.tbnz_to(9, 63, start);
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd503201f, "nop"),
                (0x14000007, "b fwd"),
                (0xd503201f, "nop"),
                (0x540000a0, "b.eq fwd"),
                (0xb4000081, "cbz x1, fwd"),
                (0x35000062, "cbnz w2, fwd"),
                (0x36200043, "tbz w3, #4, fwd"),
                (0xb7400025, "tbnz x5, #40, fwd"),
                (0xd503201f, "nop"),
                (0x17fffff7, "b start"),
                (0x54fffec1, "b.ne start"),
                (0x34fffea6, "cbz w6, start"),
                (0xb5fffe87, "cbnz x7, start"),
                (0x362ffe68, "tbz w8, #5, start"),
                (0xb7fffe49, "tbnz x9, #63, start"),
            ],
        );
    }

    #[test]
    fn asm_labels_self_and_adjacent() {
        let mut a = Asm::new();
        let here = a.label();
        let next = a.label();
        a.bind(here);
        a.b_to(here);
        a.b_cond_to(Cond::Lt, here);
        a.cbz_to(true, 4, next);
        a.bind(next);
        a.emit(ret());
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x14000000, "b here"),
                (0x54ffffeb, "b.lt here"),
                (0xb4000024, "cbz x4, next"),
                (0xd65f03c0, "ret"),
            ],
        );
    }

    #[test]
    fn asm_labels_shared_target_two_labels() {
        let mut a = Asm::new();
        let l1 = a.label();
        let l2 = a.label();
        a.b_to(l2);
        a.bind(l1);
        a.emit(nop());
        a.b_cond_to(Cond::Hi, l1);
        a.bind(l2);
        a.emit(nop());
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0x14000003, "b l2"),
                (0xd503201f, "nop"),
                (0x54ffffe8, "b.hi l1"),
                (0xd503201f, "nop"),
            ],
        );
    }

    #[test]
    fn asm_b_abs_forward() {
        let mut a = Asm::new();
        a.emit(nop());
        a.emit(nop());
        a.b_abs(0x20_0000);
        a.emit(nop());
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd503201f, "nop"),
                (0xd503201f, "nop"),
                (0x1403fffe, "b #0xffff8"),
                (0xd503201f, "nop"),
            ],
        );
    }

    #[test]
    fn asm_b_abs_backward() {
        let mut a = Asm::new();
        a.emit(nop());
        a.emit(nop());
        a.b_abs(0x8_0000);
        a.emit(nop());
        a.finish(0x10_0000).unwrap();
        check_words(
            &a,
            &[
                (0xd503201f, "nop"),
                (0xd503201f, "nop"),
                (0x17fdfffe, "b #-0x80008"),
                (0xd503201f, "nop"),
            ],
        );
    }

    #[test]
    fn asm_b_abs_self() {
        let mut a = Asm::new();
        a.emit(nop());
        a.b_abs(0x10_0004);
        a.finish(0x10_0000).unwrap();
        check_words(&a, &[(0xd503201f, "nop"), (0x14000000, "b #0")]);
    }

    #[test]
    fn asm_b_abs_limits() {
        let mut a = Asm::new();
        a.b_abs(0x1000_0000 + 0x7ff_fffc);
        a.b_abs(0x1000_0004 - 0x800_0000);
        a.finish(0x1000_0000).unwrap();
        check_words(
            &a,
            &[(0x15ffffff, "b #0x7fffffc"), (0x16000000, "b #-0x8000000")],
        );
    }

    #[test]
    fn asm_finish_errors() {
        // Unbound label.
        let mut a = Asm::new();
        let never = a.label();
        a.b_to(never);
        assert_eq!(a.finish(0), Err("unbound label"));

        // b.cond/cbz reach +-2^18 words: the last reachable forward target is 2^18 - 1 words.
        let mut a = Asm::new();
        let l = a.label();
        a.b_cond_to(Cond::Eq, l);
        for _ in 0..(1 << 18) - 2 {
            a.emit(nop());
        }
        a.bind(l);
        a.emit(nop());
        a.finish(0).unwrap();
        assert_eq!(a.words[0], b_cond(Cond::Eq, (1 << 18) - 1));

        let mut a = Asm::new();
        let l = a.label();
        a.cbz_to(true, 1, l);
        for _ in 0..(1 << 18) - 1 {
            a.emit(nop());
        }
        a.bind(l);
        a.emit(nop());
        assert_eq!(a.finish(0), Err("b19 out of range"));

        // Backward limit: exactly -2^18 words is reachable, one more is not.
        let mut a = Asm::new();
        let l = a.label();
        a.bind(l);
        for _ in 0..(1 << 18) {
            a.emit(nop());
        }
        a.cbnz_to(false, 1, l);
        a.finish(0).unwrap();
        assert_eq!(*a.words.last().unwrap(), cbnz(false, 1, -(1 << 18)));

        let mut a = Asm::new();
        let l = a.label();
        a.bind(l);
        for _ in 0..(1 << 18) + 1 {
            a.emit(nop());
        }
        a.cbnz_to(false, 1, l);
        assert_eq!(a.finish(0), Err("b19 out of range"));

        let mut a = Asm::new();
        let l = a.label();
        a.bind(l);
        for _ in 0..(1 << 18) - 1 {
            a.emit(nop());
        }
        a.b_cond_to(Cond::Ne, l);
        a.finish(0).unwrap();
        assert_eq!(*a.words.last().unwrap(), b_cond(Cond::Ne, -((1 << 18) - 1)));

        // tbz/tbnz reach +-2^13 words.
        let mut a = Asm::new();
        let l = a.label();
        a.tbz_to(1, 3, l);
        for _ in 0..(1 << 13) - 1 {
            a.emit(nop());
        }
        a.bind(l);
        a.emit(nop());
        assert_eq!(a.finish(0), Err("b14 out of range"));

        let mut a = Asm::new();
        let l = a.label();
        a.tbnz_to(1, 3, l);
        for _ in 0..(1 << 13) - 2 {
            a.emit(nop());
        }
        a.bind(l);
        a.emit(nop());
        a.finish(0).unwrap();
        assert_eq!(a.words[0], tbnz(1, 3, (1 << 13) - 1));

        // External B reaches +-2^25 words (128 MiB).
        let mut a = Asm::new();
        a.b_abs(0x8000_0000 + 0x800_0000);
        assert_eq!(a.finish(0x8000_0000), Err("b26 out of range"));
        let mut a = Asm::new();
        a.b_abs(0x8000_0000 - 0x800_0004);
        assert_eq!(a.finish(0x8000_0000), Err("b26 out of range"));
    }

    #[test]
    fn asm_pos_and_emit() {
        let mut a = Asm::new();
        assert_eq!(a.pos(), 0);
        a.emit(nop());
        a.emit(ret());
        assert_eq!(a.pos(), 2);
        assert_eq!(a.words, vec![nop(), ret()]);
        assert_ne!(a.label(), a.label());
    }

    #[test]
    fn asm_add_any_i64_min() {
        let mut a = Asm::new();
        a.add_any(1, 2, i64::MIN, 9);
        // Assembler: `movz x9, #0x8000, lsl #48` (0xd2f00009), `add x1, x2, x9` (0x8b090041).
        check_words(
            &a,
            &[
                (0xd2f0_0009, "movz x9, #0x8000, lsl #48"),
                (0x8b09_0041, "add x1, x2, x9"),
            ],
        );
    }
}
