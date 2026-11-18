use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ENCODER_ID: AtomicU64 = AtomicU64::new(1);

/// Register numbers 0..=30. Encoded register 31 must be selected explicitly
/// as `Zero` or `Sp` according to the instruction's operand role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct XReg(u8);

impl XReg {
    pub fn new(number: u8) -> Result<Self, EncodeError> {
        if number < 31 {
            Ok(Self(number))
        } else {
            Err(EncodeError::InvalidRegister(number))
        }
    }

    pub const fn number(self) -> u8 {
        self.0
    }
}

/// 32-bit register number 0..=30; register 31 is not a `WReg`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WReg(u8);

impl WReg {
    pub fn new(number: u8) -> Result<Self, EncodeError> {
        if number < 31 {
            Ok(Self(number))
        } else {
            Err(EncodeError::InvalidRegister(number))
        }
    }

    pub const fn number(self) -> u8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XRegOrZero {
    Reg(XReg),
    Zero,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum XRegOrSp {
    Reg(XReg),
    Sp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AddressBase {
    Reg(XReg),
    Sp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PairIndexMode {
    Offset,
    PreIndex,
    PostIndex,
}

/// Access width of the S6-41 memory-order forms: B, H, W, X suffixes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessWidth {
    Byte,
    Half,
    Word,
    Dword,
}

impl AccessWidth {
    const fn size_bits(self) -> u32 {
        match self {
            Self::Byte => 0,
            Self::Half => 1,
            Self::Word => 2,
            Self::Dword => 3,
        }
    }

    pub const fn bytes(self) -> u32 {
        1 << self.size_bits()
    }
}

/// Acquire/release bits of an LSE read-modify-write. Only the two orders the
/// lowering needs: no ordering (interleaved mode) and `AL` (parallel mode).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtomicOrder {
    Relaxed,
    AcquireRelease,
}

impl AtomicOrder {
    const fn bits(self) -> (u32, u32) {
        match self {
            Self::Relaxed => (0, 0),
            Self::AcquireRelease => (1, 1),
        }
    }
}

/// `DMB` options in the fence norm: `ISH` (full) and `ISHLD` (load barrier).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierOption {
    Ish,
    IshLd,
}

impl BarrierOption {
    const fn crm(self) -> u32 {
        match self {
            Self::Ish => 0b1011,
            Self::IshLd => 0b1001,
        }
    }
}

/// Shift of the shifted-register logical forms of S6-41b (`LSL`, `LSR`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shift {
    Lsl,
    Lsr,
}

impl Shift {
    const fn bits(self) -> u32 {
        match self {
            Self::Lsl => 0,
            Self::Lsr => 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Condition {
    Equal,
    Lower,
}

impl Condition {
    const fn bits(self) -> u32 {
        match self {
            Self::Equal => 0,
            Self::Lower => 3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EncodeError {
    InvalidRegister(u8),
    InvalidHalfwordIndex(u8),
    ImmediateOutOfRange {
        value: u32,
        maximum: u32,
    },
    OffsetOutOfRange {
        value: i32,
        minimum: i32,
        maximum: i32,
    },
    UnalignedOffset {
        value: i64,
        alignment: u8,
    },
    PairWritebackOverlapsBase {
        base: u8,
    },
    DuplicateLoadPairRegister {
        register: u8,
    },
    InvalidLabel(Label),
    ForeignLabel(Label),
    LabelAlreadyBound(Label),
    UnboundLabel(Label),
    BranchOutOfRange {
        words: i64,
        minimum: i64,
        maximum: i64,
    },
    BufferTooLarge,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ARM64 encoding error: {self:?}")
    }
}

impl std::error::Error for EncodeError {}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Label {
    owner: u64,
    index: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedCode {
    words: Vec<u32>,
}

impl EncodedCode {
    pub fn words(&self) -> &[u32] {
        &self.words
    }

    pub fn into_words(self) -> Vec<u32> {
        self.words
    }

    pub fn to_le_bytes(&self) -> Result<Vec<u8>, EncodeError> {
        let capacity = self
            .words
            .len()
            .checked_mul(4)
            .ok_or(EncodeError::BufferTooLarge)?;
        let mut bytes = Vec::with_capacity(capacity);
        for word in &self.words {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        Ok(bytes)
    }
}

#[derive(Clone, Copy)]
enum Relocation {
    Branch26,
    Conditional19,
}

#[derive(Clone, Copy)]
struct Fixup {
    at: usize,
    target: Label,
    relocation: Relocation,
}

/// Small checked buffer for the instruction forms currently emitted by od-jit.
/// Labels are word offsets; branch displacement is relative to the branch
/// instruction address and encoded in 4-byte words as required by A64.
pub struct Encoder {
    owner: u64,
    words: Vec<u32>,
    labels: Vec<Option<usize>>,
    fixups: Vec<Fixup>,
}

impl Encoder {
    pub fn new() -> Self {
        Self {
            owner: NEXT_ENCODER_ID.fetch_add(1, Ordering::Relaxed),
            words: Vec::new(),
            labels: Vec::new(),
            fixups: Vec::new(),
        }
    }

    pub fn new_label(&mut self) -> Result<Label, EncodeError> {
        let index = u32::try_from(self.labels.len()).map_err(|_| EncodeError::BufferTooLarge)?;
        let label = Label {
            owner: self.owner,
            index,
        };
        self.labels.push(None);
        Ok(label)
    }

    pub fn position(&self) -> usize {
        self.words.len()
    }

    pub fn bind(&mut self, label: Label) -> Result<(), EncodeError> {
        self.validate_label(label)?;
        let position = self.words.len();
        let slot = self
            .labels
            .get_mut(label.index as usize)
            .ok_or(EncodeError::InvalidLabel(label))?;
        if slot.is_some() {
            return Err(EncodeError::LabelAlreadyBound(label));
        }
        *slot = Some(position);
        Ok(())
    }

    pub fn movz_x(&mut self, dst: XReg, immediate: u16, halfword: u8) -> Result<(), EncodeError> {
        self.validate_halfword(halfword)?;
        self.emit(
            0xD280_0000
                | (u32::from(halfword) << 21)
                | (u32::from(immediate) << 5)
                | u32::from(dst.0),
        );
        Ok(())
    }

    pub fn movk_x(&mut self, dst: XReg, immediate: u16, halfword: u8) -> Result<(), EncodeError> {
        self.validate_halfword(halfword)?;
        self.emit(
            0xF280_0000
                | (u32::from(halfword) << 21)
                | (u32::from(immediate) << 5)
                | u32::from(dst.0),
        );
        Ok(())
    }

    pub fn mov_x(&mut self, dst: XReg, src: XRegOrZero) {
        let rm = match src {
            XRegOrZero::Reg(reg) => reg.0,
            XRegOrZero::Zero => 31,
        };
        self.emit(0xAA00_03E0 | (u32::from(rm) << 16) | u32::from(dst.0));
    }

    pub fn add_x(&mut self, dst: XReg, lhs: XReg, rhs: XRegOrZero) {
        self.emit(
            0x8B00_0000
                | (u32::from(rhs.number()) << 16)
                | (u32::from(lhs.0) << 5)
                | u32::from(dst.0),
        );
    }

    pub fn sub_x(&mut self, dst: XReg, lhs: XReg, rhs: XRegOrZero) {
        self.emit(
            0xCB00_0000
                | (u32::from(rhs.number()) << 16)
                | (u32::from(lhs.0) << 5)
                | u32::from(dst.0),
        );
    }

    pub fn and_x(&mut self, dst: XReg, lhs: XReg, rhs: XRegOrZero) {
        self.emit(
            0x8A00_0000
                | (u32::from(rhs.number()) << 16)
                | (u32::from(lhs.0) << 5)
                | u32::from(dst.0),
        );
    }

    pub fn subs_x(&mut self, dst: XRegOrZero, lhs: XReg, rhs: XRegOrZero) {
        self.emit(
            0xEB00_0000
                | (u32::from(rhs.number()) << 16)
                | (u32::from(lhs.0) << 5)
                | u32::from(dst.number()),
        );
    }

    /// `ORR Xd, Xn, Xm, <shift> #amount` (S6-41b). `amount` is 0..=63.
    pub fn orr_x(
        &mut self,
        dst: XReg,
        lhs: XRegOrZero,
        rhs: XRegOrZero,
        shift: Shift,
        amount: u8,
    ) -> Result<(), EncodeError> {
        self.emit_logical_shifted(0xAA00_0000, dst, lhs, rhs, shift, amount)
    }

    /// `EOR Xd, Xn, Xm, <shift> #amount` (S6-41b).
    pub fn eor_x(
        &mut self,
        dst: XReg,
        lhs: XRegOrZero,
        rhs: XRegOrZero,
        shift: Shift,
        amount: u8,
    ) -> Result<(), EncodeError> {
        self.emit_logical_shifted(0xCA00_0000, dst, lhs, rhs, shift, amount)
    }

    /// `ORN Xd, Xn, Xm, <shift> #amount` (S6-41b); with `lhs = Zero` it is
    /// `MVN Xd, Xm`.
    pub fn orn_x(
        &mut self,
        dst: XReg,
        lhs: XRegOrZero,
        rhs: XRegOrZero,
        shift: Shift,
        amount: u8,
    ) -> Result<(), EncodeError> {
        self.emit_logical_shifted(0xAA20_0000, dst, lhs, rhs, shift, amount)
    }

    /// `UBFM Xd, Xn, #immr, #imms` (S6-41b), the base of `LSL`/`LSR`/`UBFX`.
    pub fn ubfm_x(&mut self, dst: XReg, src: XReg, immr: u8, imms: u8) -> Result<(), EncodeError> {
        if immr > 63 || imms > 63 {
            return Err(EncodeError::ImmediateOutOfRange {
                value: u32::from(immr.max(imms)),
                maximum: 63,
            });
        }
        self.emit(
            0xD340_0000
                | (u32::from(immr) << 16)
                | (u32::from(imms) << 10)
                | (u32::from(src.0) << 5)
                | u32::from(dst.0),
        );
        Ok(())
    }

    /// `LSL Xd, Xn, #amount` (alias of `UBFM`), `amount` 1..=63.
    pub fn lsl_imm_x(&mut self, dst: XReg, src: XReg, amount: u8) -> Result<(), EncodeError> {
        if !(1..=63).contains(&amount) {
            return Err(EncodeError::ImmediateOutOfRange {
                value: u32::from(amount),
                maximum: 63,
            });
        }
        self.ubfm_x(dst, src, 64 - amount, 63 - amount)
    }

    /// `LSR Xd, Xn, #amount` (alias of `UBFM`), `amount` 1..=63.
    pub fn lsr_imm_x(&mut self, dst: XReg, src: XReg, amount: u8) -> Result<(), EncodeError> {
        if !(1..=63).contains(&amount) {
            return Err(EncodeError::ImmediateOutOfRange {
                value: u32::from(amount),
                maximum: 63,
            });
        }
        self.ubfm_x(dst, src, amount, 63)
    }

    /// `UBFX Xd, Xn, #lsb, #width` (alias of `UBFM`): the `width` bits of
    /// `src` starting at `lsb`, zero-extended.
    pub fn ubfx_x(&mut self, dst: XReg, src: XReg, lsb: u8, width: u8) -> Result<(), EncodeError> {
        if width == 0 || u32::from(lsb) + u32::from(width) > 64 {
            return Err(EncodeError::ImmediateOutOfRange {
                value: u32::from(lsb) + u32::from(width),
                maximum: 64,
            });
        }
        self.ubfm_x(dst, src, lsb, lsb + width - 1)
    }

    /// `CSET Xd, <cond>` (alias of `CSINC Xd, XZR, XZR, !cond`, S6-41b).
    pub fn cset_x(&mut self, dst: XReg, condition: Condition) {
        self.emit(0x9A9F_07E0 | ((condition.bits() ^ 1) << 12) | u32::from(dst.0));
    }

    fn emit_logical_shifted(
        &mut self,
        opcode: u32,
        dst: XReg,
        lhs: XRegOrZero,
        rhs: XRegOrZero,
        shift: Shift,
        amount: u8,
    ) -> Result<(), EncodeError> {
        if amount > 63 {
            return Err(EncodeError::ImmediateOutOfRange {
                value: u32::from(amount),
                maximum: 63,
            });
        }
        self.emit(
            opcode
                | (shift.bits() << 22)
                | (u32::from(rhs.number()) << 16)
                | (u32::from(amount) << 10)
                | (u32::from(lhs.number()) << 5)
                | u32::from(dst.0),
        );
        Ok(())
    }

    pub fn adds_imm_x(
        &mut self,
        dst: XRegOrZero,
        lhs: XRegOrSp,
        immediate: u16,
    ) -> Result<(), EncodeError> {
        self.validate_imm12(u32::from(immediate))?;
        self.emit(
            0xB100_0000
                | (u32::from(immediate) << 10)
                | (u32::from(lhs.number()) << 5)
                | u32::from(dst.number()),
        );
        Ok(())
    }

    pub fn subs_imm_x(
        &mut self,
        dst: XRegOrZero,
        lhs: XRegOrSp,
        immediate: u16,
    ) -> Result<(), EncodeError> {
        self.validate_imm12(u32::from(immediate))?;
        self.emit(
            0xF100_0000
                | (u32::from(immediate) << 10)
                | (u32::from(lhs.number()) << 5)
                | u32::from(dst.number()),
        );
        Ok(())
    }

    pub fn ldr_x(
        &mut self,
        rt: XRegOrZero,
        base: AddressBase,
        byte_offset: u32,
    ) -> Result<(), EncodeError> {
        self.validate_scaled_offset(byte_offset, 8, 4095)?;
        self.emit(
            0xF940_0000
                | ((byte_offset / 8) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
        Ok(())
    }

    pub fn str_x(
        &mut self,
        rt: XRegOrZero,
        base: AddressBase,
        byte_offset: u32,
    ) -> Result<(), EncodeError> {
        self.validate_scaled_offset(byte_offset, 8, 4095)?;
        self.emit(
            0xF900_0000
                | ((byte_offset / 8) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
        Ok(())
    }

    pub fn ldarb_w(&mut self, rt: WReg, base: AddressBase) {
        self.emit(0x08DF_FC00 | (u32::from(base.number()) << 5) | u32::from(rt.0));
    }

    /// `LDR{B,H,W,X}` with an unsigned scaled immediate offset (S6-41). The
    /// register is `Wt` for `Byte`/`Half`/`Word` and `Xt` for `Dword`; `Zero`
    /// selects WZR/XZR. Used by the interleaved-mode lowering of an ordinary
    /// load, which carries no ordering.
    pub fn ldr_sized(
        &mut self,
        width: AccessWidth,
        rt: XRegOrZero,
        base: AddressBase,
        byte_offset: u32,
    ) -> Result<(), EncodeError> {
        self.validate_scaled_offset(byte_offset, width.bytes(), 4095)?;
        self.emit(
            0x3940_0000
                | (width.size_bits() << 30)
                | ((byte_offset / width.bytes()) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
        Ok(())
    }

    /// `STR{B,H,W,X}` with an unsigned scaled immediate offset (S6-41).
    pub fn str_sized(
        &mut self,
        width: AccessWidth,
        rt: XRegOrZero,
        base: AddressBase,
        byte_offset: u32,
    ) -> Result<(), EncodeError> {
        self.validate_scaled_offset(byte_offset, width.bytes(), 4095)?;
        self.emit(
            0x3900_0000
                | (width.size_bits() << 30)
                | ((byte_offset / width.bytes()) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
        Ok(())
    }

    /// `LDAR{B,H,W,X}` (load-acquire, S6-41). Register class as in
    /// [`Encoder::ldr_sized`]. `LDAR`/`STLR` are RCsc with respect to each
    /// other, so a pair of them gives the sequentially consistent ordering the
    /// parallel mode requires of an ordinary access (MEMORY_MODEL, S6-32).
    pub fn ldar(&mut self, width: AccessWidth, rt: XRegOrZero, base: AddressBase) {
        self.emit(
            0x08DF_FC00
                | (width.size_bits() << 30)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
    }

    /// `STLR{B,H,W,X}` (store-release, S6-41).
    pub fn stlr(&mut self, width: AccessWidth, rt: XRegOrZero, base: AddressBase) {
        self.emit(
            0x089F_FC00
                | (width.size_bits() << 30)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
    }

    /// `LDADD{,A}{,L}{B,H,}` (FEAT_LSE, S6-41): `rt = [base]; [base] += rs`
    /// as one atomic operation. `rt` is never the zero register: with
    /// `Relaxed` or release-only forms that encoding is the `STADD` alias,
    /// whose ordering differs, so it is rejected for every order.
    pub fn ldadd(
        &mut self,
        width: AccessWidth,
        order: AtomicOrder,
        rs: XRegOrZero,
        rt: XReg,
        base: AddressBase,
    ) {
        self.emit_lse_rmw(0x3820_0000, width, order, rs.number(), rt.0, base);
    }

    /// `SWP{,A}{,L}{B,H,}` (FEAT_LSE, S6-41): `rt = [base]; [base] = rs`.
    pub fn swp(
        &mut self,
        width: AccessWidth,
        order: AtomicOrder,
        rs: XRegOrZero,
        rt: XReg,
        base: AddressBase,
    ) {
        self.emit_lse_rmw(0x3820_8000, width, order, rs.number(), rt.0, base);
    }

    /// `CAS{,A}{,L}{B,H,}` (FEAT_LSE, S6-41): compare `[base]` with `rs`; if
    /// equal store `rt`; `rs` always receives the old memory value (zero
    /// extended for the sub-doubleword widths).
    pub fn cas(
        &mut self,
        width: AccessWidth,
        order: AtomicOrder,
        rs: XReg,
        rt: XRegOrZero,
        base: AddressBase,
    ) {
        let (acquire, release) = order.bits();
        self.emit(
            0x08A0_7C00
                | (width.size_bits() << 30)
                | (acquire << 22)
                | (release << 15)
                | (u32::from(rs.0) << 16)
                | (u32::from(base.number()) << 5)
                | u32::from(rt.number()),
        );
    }

    /// `LDSET{,A}{,L}{B,H,}` (FEAT_LSE, S6-41b): `rt = [base]; [base] |= rs`
    /// as one atomic operation (`LOCK OR`, `LOCK BTS`). `rt` is never the zero
    /// register, for the same reason as in [`Encoder::ldadd`] (`STSET` alias).
    pub fn ldset(
        &mut self,
        width: AccessWidth,
        order: AtomicOrder,
        rs: XRegOrZero,
        rt: XReg,
        base: AddressBase,
    ) {
        self.emit_lse_rmw(0x3820_3000, width, order, rs.number(), rt.0, base);
    }

    /// `LDCLR{,A}{,L}{B,H,}` (FEAT_LSE, S6-41b): `rt = [base]; [base] &= !rs`
    /// as one atomic operation (`LOCK AND` with an inverted mask, `LOCK BTR`).
    pub fn ldclr(
        &mut self,
        width: AccessWidth,
        order: AtomicOrder,
        rs: XRegOrZero,
        rt: XReg,
        base: AddressBase,
    ) {
        self.emit_lse_rmw(0x3820_1000, width, order, rs.number(), rt.0, base);
    }

    /// `DMB <option>` (S6-41), the inner-shareable barriers of the fence norm.
    pub fn dmb(&mut self, option: BarrierOption) {
        self.emit(0xD503_30BF | (option.crm() << 8));
    }

    fn emit_lse_rmw(
        &mut self,
        opcode: u32,
        width: AccessWidth,
        order: AtomicOrder,
        rs: u8,
        rt: u8,
        base: AddressBase,
    ) {
        let (acquire, release) = order.bits();
        self.emit(
            opcode
                | (width.size_bits() << 30)
                | (acquire << 23)
                | (release << 22)
                | (u32::from(rs) << 16)
                | (u32::from(base.number()) << 5)
                | u32::from(rt),
        );
    }

    pub fn stp_x(
        &mut self,
        first: XReg,
        second: XReg,
        base: AddressBase,
        byte_offset: i32,
        mode: PairIndexMode,
    ) -> Result<(), EncodeError> {
        self.validate_pair_registers(first, second, base, mode, false)?;
        let imm7 = self.validate_pair_offset(byte_offset)?;
        let mode_bits = match mode {
            PairIndexMode::Offset => 0xA900_0000,
            PairIndexMode::PreIndex => 0xA980_0000,
            PairIndexMode::PostIndex => 0xA880_0000,
        };
        self.emit(
            mode_bits
                | (imm7 << 15)
                | (u32::from(second.0) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(first.0),
        );
        Ok(())
    }

    pub fn ldp_x(
        &mut self,
        first: XReg,
        second: XReg,
        base: AddressBase,
        byte_offset: i32,
        mode: PairIndexMode,
    ) -> Result<(), EncodeError> {
        self.validate_pair_registers(first, second, base, mode, true)?;
        let imm7 = self.validate_pair_offset(byte_offset)?;
        let mode_bits = match mode {
            PairIndexMode::Offset => 0xA940_0000,
            PairIndexMode::PreIndex => 0xA9C0_0000,
            PairIndexMode::PostIndex => 0xA8C0_0000,
        };
        self.emit(
            mode_bits
                | (imm7 << 15)
                | (u32::from(second.0) << 10)
                | (u32::from(base.number()) << 5)
                | u32::from(first.0),
        );
        Ok(())
    }

    pub fn b(&mut self, target: Label) -> Result<(), EncodeError> {
        self.branch26(0x1400_0000, target)
    }
    pub fn bl(&mut self, target: Label) -> Result<(), EncodeError> {
        self.branch26(0x9400_0000, target)
    }

    pub fn b_cond(&mut self, condition: Condition, target: Label) -> Result<(), EncodeError> {
        self.branch19(0x5400_0000 | condition.bits(), target)
    }

    pub fn cbz_x(&mut self, reg: XRegOrZero, target: Label) -> Result<(), EncodeError> {
        self.branch19(0xB400_0000 | u32::from(reg.number()), target)
    }

    pub fn cbnz_x(&mut self, reg: XRegOrZero, target: Label) -> Result<(), EncodeError> {
        self.branch19(0xB500_0000 | u32::from(reg.number()), target)
    }

    pub fn cbz_w(&mut self, reg: WReg, target: Label) -> Result<(), EncodeError> {
        self.branch19(0x3400_0000 | u32::from(reg.0), target)
    }

    pub fn cbnz_w(&mut self, reg: WReg, target: Label) -> Result<(), EncodeError> {
        self.branch19(0x3500_0000 | u32::from(reg.0), target)
    }

    pub fn blr(&mut self, reg: XReg) {
        self.emit(0xD63F_0000 | (u32::from(reg.0) << 5));
    }

    pub fn ret(&mut self, reg: XReg) {
        self.emit(0xD65F_0000 | (u32::from(reg.0) << 5));
    }

    /// Resolve every relocation into a temporary copy. No encoded output is
    /// returned unless all labels and displacement ranges are valid.
    pub fn finish(self) -> Result<EncodedCode, EncodeError> {
        let mut words = self.words;
        for fixup in self.fixups {
            let target = self
                .labels
                .get(fixup.target.index as usize)
                .ok_or(EncodeError::InvalidLabel(fixup.target))?
                .ok_or(EncodeError::UnboundLabel(fixup.target))?;
            let delta = i64::try_from(target).map_err(|_| EncodeError::BufferTooLarge)?
                - i64::try_from(fixup.at).map_err(|_| EncodeError::BufferTooLarge)?;
            match fixup.relocation {
                Relocation::Branch26 => {
                    check_branch_range(delta, 26)?;
                    words[fixup.at] =
                        (words[fixup.at] & 0xFC00_0000) | ((delta as u32) & 0x03FF_FFFF);
                }
                Relocation::Conditional19 => {
                    check_branch_range(delta, 19)?;
                    words[fixup.at] =
                        (words[fixup.at] & !0x00FF_FFE0) | (((delta as u32) & 0x7_FFFF) << 5);
                }
            }
        }
        Ok(EncodedCode { words })
    }

    fn emit(&mut self, word: u32) {
        self.words.push(word);
    }

    fn branch26(&mut self, opcode: u32, target: Label) -> Result<(), EncodeError> {
        let at = self.words.len();
        self.validate_label(target)?;
        self.emit(opcode);
        self.fixups.push(Fixup {
            at,
            target,
            relocation: Relocation::Branch26,
        });
        Ok(())
    }

    fn branch19(&mut self, opcode: u32, target: Label) -> Result<(), EncodeError> {
        let at = self.words.len();
        self.validate_label(target)?;
        self.emit(opcode);
        self.fixups.push(Fixup {
            at,
            target,
            relocation: Relocation::Conditional19,
        });
        Ok(())
    }

    fn validate_label(&self, label: Label) -> Result<(), EncodeError> {
        if label.owner != self.owner {
            return Err(EncodeError::ForeignLabel(label));
        }
        self.labels
            .get(label.index as usize)
            .map(|_| ())
            .ok_or(EncodeError::InvalidLabel(label))
    }

    fn validate_halfword(&self, halfword: u8) -> Result<(), EncodeError> {
        if halfword < 4 {
            Ok(())
        } else {
            Err(EncodeError::InvalidHalfwordIndex(halfword))
        }
    }

    fn validate_imm12(&self, value: u32) -> Result<(), EncodeError> {
        if value <= 4095 {
            Ok(())
        } else {
            Err(EncodeError::ImmediateOutOfRange {
                value,
                maximum: 4095,
            })
        }
    }

    fn validate_scaled_offset(
        &self,
        offset: u32,
        scale: u32,
        max_scaled: u32,
    ) -> Result<(), EncodeError> {
        if offset % scale != 0 {
            return Err(EncodeError::UnalignedOffset {
                value: i64::from(offset),
                alignment: scale as u8,
            });
        }
        let scaled = offset / scale;
        if scaled > max_scaled {
            return Err(EncodeError::ImmediateOutOfRange {
                value: scaled,
                maximum: max_scaled,
            });
        }
        Ok(())
    }

    fn validate_pair_offset(&self, offset: i32) -> Result<u32, EncodeError> {
        if !(-512..=504).contains(&offset) {
            return Err(EncodeError::OffsetOutOfRange {
                value: offset,
                minimum: -512,
                maximum: 504,
            });
        }
        if offset % 8 != 0 {
            return Err(EncodeError::UnalignedOffset {
                value: i64::from(offset),
                alignment: 8,
            });
        }
        Ok(((offset / 8) as u32) & 0x7f)
    }

    fn validate_pair_registers(
        &self,
        first: XReg,
        second: XReg,
        base: AddressBase,
        mode: PairIndexMode,
        is_load: bool,
    ) -> Result<(), EncodeError> {
        if is_load && first == second {
            return Err(EncodeError::DuplicateLoadPairRegister { register: first.0 });
        }
        if mode != PairIndexMode::Offset {
            if let AddressBase::Reg(base) = base {
                if base == first || base == second {
                    return Err(EncodeError::PairWritebackOverlapsBase { base: base.0 });
                }
            }
        }
        Ok(())
    }
}

impl XRegOrZero {
    const fn number(self) -> u8 {
        match self {
            Self::Reg(reg) => reg.0,
            Self::Zero => 31,
        }
    }
}

impl XRegOrSp {
    const fn number(self) -> u8 {
        match self {
            Self::Reg(reg) => reg.0,
            Self::Sp => 31,
        }
    }
}

impl AddressBase {
    const fn number(self) -> u8 {
        match self {
            Self::Reg(reg) => reg.0,
            Self::Sp => 31,
        }
    }
}

fn check_branch_range(words: i64, bits: u32) -> Result<(), EncodeError> {
    let minimum = -(1_i64 << (bits - 1));
    let maximum = 1_i64 << (bits - 1);
    if (minimum..maximum).contains(&words) {
        Ok(())
    } else {
        Err(EncodeError::BranchOutOfRange {
            words,
            minimum,
            maximum: maximum - 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(number: u8) -> XReg {
        XReg::new(number).unwrap()
    }

    #[test]
    fn matches_local_apple_clang_a64_assembler_fixture() {
        // Fixture: tests/fixtures/arm64_encoder.s, clang 21.0.0,
        // -target arm64-apple-macosx15.0. Words independently read with otool.
        let mut encoder = Encoder::new();
        let target = encoder.new_label().unwrap();
        encoder.movz_x(x(10), 0x1234, 0).unwrap();
        encoder.movk_x(x(10), 0xabcd, 1).unwrap();
        encoder.mov_x(x(19), XRegOrZero::Reg(x(0)));
        encoder.mov_x(x(14), XRegOrZero::Zero);
        encoder.add_x(x(11), x(9), XRegOrZero::Reg(x(10)));
        encoder.sub_x(x(12), x(10), XRegOrZero::Reg(x(9)));
        encoder.and_x(x(13), x(9), XRegOrZero::Reg(x(10)));
        encoder.subs_x(XRegOrZero::Zero, x(9), XRegOrZero::Reg(x(10)));
        encoder
            .adds_imm_x(XRegOrZero::Zero, XRegOrSp::Sp, 4095)
            .unwrap();
        encoder
            .adds_imm_x(XRegOrZero::Reg(x(14)), XRegOrSp::Reg(x(14)), 1)
            .unwrap();
        encoder
            .subs_imm_x(XRegOrZero::Zero, XRegOrSp::Reg(x(9)), 1)
            .unwrap();
        encoder
            .ldr_x(XRegOrZero::Reg(x(9)), AddressBase::Reg(x(19)), 32)
            .unwrap();
        encoder
            .str_x(XRegOrZero::Reg(x(9)), AddressBase::Reg(x(21)), 0)
            .unwrap();
        encoder.ldarb_w(WReg::new(9).unwrap(), AddressBase::Reg(x(22)));
        encoder
            .stp_x(x(19), x(20), AddressBase::Sp, -16, PairIndexMode::PreIndex)
            .unwrap();
        encoder
            .ldp_x(x(22), x(30), AddressBase::Sp, 16, PairIndexMode::PostIndex)
            .unwrap();
        encoder.b(target).unwrap();
        encoder.bl(target).unwrap();
        encoder.b_cond(Condition::Equal, target).unwrap();
        encoder.b_cond(Condition::Lower, target).unwrap();
        encoder.cbz_x(XRegOrZero::Reg(x(9)), target).unwrap();
        encoder.cbz_w(WReg::new(9).unwrap(), target).unwrap();
        encoder.cbnz_x(XRegOrZero::Reg(x(9)), target).unwrap();
        encoder.cbnz_w(WReg::new(9).unwrap(), target).unwrap();
        encoder.blr(x(16));
        encoder.ret(x(30));
        encoder.bind(target).unwrap();
        encoder.ret(x(30));

        let code = encoder.finish().unwrap();
        assert_eq!(
            code.words(),
            &[
                0xd282_468a,
                0xf2b5_79aa,
                0xaa00_03f3,
                0xaa1f_03ee,
                0x8b0a_012b,
                0xcb09_014c,
                0x8a0a_012d,
                0xeb0a_013f,
                0xb13f_ffff,
                0xb100_05ce,
                0xf100_053f,
                0xf940_1269,
                0xf900_02a9,
                0x08df_fec9,
                0xa9bf_53f3,
                0xa8c1_7bf6,
                0x1400_000a,
                0x9400_0009,
                0x5400_0100,
                0x5400_00e3,
                0xb400_00c9,
                0x3400_00a9,
                0xb500_0089,
                0x3500_0069,
                0xd63f_0200,
                0xd65f_03c0,
                0xd65f_03c0,
            ]
        );
        let bytes = code.to_le_bytes().unwrap();
        assert_eq!(&bytes[..4], &[0x8a, 0x46, 0x82, 0xd2]);
        assert_eq!(bytes.len(), code.words().len() * 4);
    }

    #[test]
    fn validates_register_immediate_and_memory_offset_domains_before_emit() {
        assert_eq!(XReg::new(31), Err(EncodeError::InvalidRegister(31)));
        assert_eq!(WReg::new(31), Err(EncodeError::InvalidRegister(31)));
        let mut encoder = Encoder::new();
        assert_eq!(
            encoder.movz_x(x(0), 1, 4),
            Err(EncodeError::InvalidHalfwordIndex(4))
        );
        assert!(matches!(
            encoder.adds_imm_x(XRegOrZero::Reg(x(0)), XRegOrSp::Reg(x(1)), 4096),
            Err(EncodeError::ImmediateOutOfRange { .. })
        ));
        assert!(matches!(
            encoder.ldr_x(XRegOrZero::Reg(x(0)), AddressBase::Sp, 7),
            Err(EncodeError::UnalignedOffset {
                value: 7,
                alignment: 8
            })
        ));
        assert!(matches!(
            encoder.str_x(XRegOrZero::Reg(x(0)), AddressBase::Sp, 32768),
            Err(EncodeError::ImmediateOutOfRange { .. })
        ));
        assert!(matches!(
            encoder.stp_x(x(0), x(1), AddressBase::Sp, -520, PairIndexMode::Offset),
            Err(EncodeError::OffsetOutOfRange { .. })
        ));
        assert!(matches!(
            encoder.ldp_x(x(0), x(1), AddressBase::Sp, 4, PairIndexMode::PostIndex),
            Err(EncodeError::UnalignedOffset { .. })
        ));
        assert!(matches!(
            encoder.ldp_x(
                x(0),
                x(1),
                AddressBase::Reg(x(0)),
                16,
                PairIndexMode::PostIndex
            ),
            Err(EncodeError::PairWritebackOverlapsBase { base: 0 })
        ));
        assert!(matches!(
            encoder.ldp_x(x(0), x(0), AddressBase::Sp, 0, PairIndexMode::Offset),
            Err(EncodeError::DuplicateLoadPairRegister { register: 0 })
        ));
        assert_eq!(
            encoder.position(),
            0,
            "failed operands must not append words"
        );
    }

    #[test]
    fn accepts_highest_valid_immediate_and_scaled_offset_boundaries() {
        let mut encoder = Encoder::new();
        let top_register = XReg::new(30).unwrap();
        encoder.movz_x(top_register, u16::MAX, 3).unwrap();
        encoder
            .adds_imm_x(XRegOrZero::Zero, XRegOrSp::Sp, 4095)
            .unwrap();
        encoder
            .subs_imm_x(XRegOrZero::Zero, XRegOrSp::Sp, 4095)
            .unwrap();
        encoder
            .ldr_x(XRegOrZero::Zero, AddressBase::Sp, 32760)
            .unwrap();
        encoder
            .str_x(XRegOrZero::Zero, AddressBase::Sp, 32760)
            .unwrap();
        encoder
            .stp_x(x(29), x(30), AddressBase::Sp, -512, PairIndexMode::Offset)
            .unwrap();
        encoder
            .ldp_x(x(29), x(30), AddressBase::Sp, 504, PairIndexMode::Offset)
            .unwrap();
        assert_eq!(encoder.position(), 7);
        assert!(encoder.finish().is_ok());
    }

    #[test]
    fn finish_rejects_unbound_labels_and_branch_range_overflow() {
        let mut encoder = Encoder::new();
        let target = encoder.new_label().unwrap();
        encoder.b(target).unwrap();
        assert_eq!(encoder.finish(), Err(EncodeError::UnboundLabel(target)));

        for (bits, edge) in [(19, 1_i64 << 18), (26, 1_i64 << 25)] {
            assert_eq!(check_branch_range(-edge, bits), Ok(()));
            assert_eq!(check_branch_range(edge - 1, bits), Ok(()));
            assert!(matches!(
                check_branch_range(edge, bits),
                Err(EncodeError::BranchOutOfRange { .. })
            ));
            assert!(matches!(
                check_branch_range(-edge - 1, bits),
                Err(EncodeError::BranchOutOfRange { .. })
            ));
        }

        let mut encoder = Encoder::new();
        let target = encoder.new_label().unwrap();
        encoder.b_cond(Condition::Equal, target).unwrap();
        for _ in 0..(1 << 18) {
            encoder.movz_x(x(0), 0, 0).unwrap();
        }
        encoder.bind(target).unwrap();
        assert!(matches!(
            encoder.finish(),
            Err(EncodeError::BranchOutOfRange { .. })
        ));
    }

    #[test]
    fn label_binding_and_branch_to_self_use_instruction_relative_base() {
        let mut encoder = Encoder::new();
        let start = encoder.new_label().unwrap();
        encoder.bind(start).unwrap();
        encoder.b(start).unwrap();
        encoder.bl(start).unwrap();
        let words = encoder.finish().unwrap().into_words();
        assert_eq!(words, vec![0x1400_0000, 0x97ff_ffff]);
    }

    #[test]
    fn rejects_unknown_and_duplicate_labels_without_adding_branch_words() {
        let mut encoder = Encoder::new();
        assert_eq!(
            encoder.b(Label {
                owner: encoder.owner,
                index: u32::MAX,
            }),
            Err(EncodeError::InvalidLabel(Label {
                owner: encoder.owner,
                index: u32::MAX,
            }))
        );
        assert_eq!(encoder.position(), 0);
        let label = encoder.new_label().unwrap();
        encoder.bind(label).unwrap();
        assert_eq!(
            encoder.bind(label),
            Err(EncodeError::LabelAlreadyBound(label))
        );

        let mut owner = Encoder::new();
        let foreign = owner.new_label().unwrap();
        let mut other = Encoder::new();
        assert_eq!(other.bind(foreign), Err(EncodeError::ForeignLabel(foreign)));
        assert_eq!(other.b(foreign), Err(EncodeError::ForeignLabel(foreign)));
        assert_eq!(other.position(), 0);
    }

    #[test]
    fn s6_41_memory_order_forms_match_local_apple_clang_fixture() {
        // Fixture: tests/fixtures/arm64_memory_order.s, Apple clang 21.0.0,
        // -target arm64-apple-macosx15.0 -march=armv8.4-a, words read with
        // `otool -tV` / `otool -t`. Order below equals the fixture order.
        use AccessWidth::{Byte, Dword, Half, Word};
        use AtomicOrder::{AcquireRelease as Al, Relaxed};
        let mut e = Encoder::new();
        let r = |n: u8| XRegOrZero::Reg(x(n));
        let base = AddressBase::Reg(x(22));
        let b12 = AddressBase::Reg(x(12));
        e.ldr_sized(Byte, r(9), base, 0).unwrap();
        e.ldr_sized(Half, r(9), base, 2).unwrap();
        e.ldr_sized(Word, r(9), base, 4).unwrap();
        e.ldr_sized(Dword, r(9), base, 8).unwrap();
        e.ldr_sized(Dword, XRegOrZero::Zero, base, 0).unwrap();
        e.str_sized(Byte, r(9), base, 1).unwrap();
        e.str_sized(Half, r(9), base, 2).unwrap();
        e.str_sized(Word, r(9), base, 4).unwrap();
        e.str_sized(Dword, r(9), base, 8).unwrap();
        e.str_sized(Word, XRegOrZero::Zero, base, 0).unwrap();
        e.ldar(Byte, r(9), base);
        e.ldar(Half, r(9), base);
        e.ldar(Word, r(9), base);
        e.ldar(Dword, r(9), base);
        e.stlr(Byte, r(9), base);
        e.stlr(Half, r(9), base);
        e.stlr(Word, r(9), base);
        e.stlr(Dword, r(9), base);
        e.stlr(Dword, XRegOrZero::Zero, AddressBase::Sp);
        for width in [Byte, Half, Word, Dword] {
            e.ldadd(width, Relaxed, r(10), x(11), b12);
        }
        for width in [Byte, Half, Word, Dword] {
            e.ldadd(width, Al, r(10), x(11), b12);
        }
        e.ldadd(Dword, Al, XRegOrZero::Zero, x(11), AddressBase::Sp);
        for order in [Relaxed, Al] {
            for width in [Byte, Half, Word, Dword] {
                e.swp(width, order, r(10), x(11), b12);
            }
        }
        for order in [Relaxed, Al] {
            for width in [Byte, Half, Word, Dword] {
                e.cas(width, order, x(10), r(11), b12);
            }
        }
        e.cas(Dword, Al, x(10), XRegOrZero::Zero, b12);
        e.dmb(BarrierOption::Ish);
        e.dmb(BarrierOption::IshLd);
        assert_eq!(
            e.finish().unwrap().words(),
            &[
                0x3940_02c9,
                0x7940_06c9,
                0xb940_06c9,
                0xf940_06c9,
                0xf940_02df, // LDR*
                0x3900_06c9,
                0x7900_06c9,
                0xb900_06c9,
                0xf900_06c9,
                0xb900_02df, // STR*
                0x08df_fec9,
                0x48df_fec9,
                0x88df_fec9,
                0xc8df_fec9, // LDAR*
                0x089f_fec9,
                0x489f_fec9,
                0x889f_fec9,
                0xc89f_fec9,
                0xc89f_ffff, // STLR*
                0x382a_018b,
                0x782a_018b,
                0xb82a_018b,
                0xf82a_018b, // LDADD*
                0x38ea_018b,
                0x78ea_018b,
                0xb8ea_018b,
                0xf8ea_018b,
                0xf8ff_03eb, // LDADDAL*
                0x382a_818b,
                0x782a_818b,
                0xb82a_818b,
                0xf82a_818b, // SWP*
                0x38ea_818b,
                0x78ea_818b,
                0xb8ea_818b,
                0xf8ea_818b, // SWPAL*
                0x08aa_7d8b,
                0x48aa_7d8b,
                0x88aa_7d8b,
                0xc8aa_7d8b, // CAS*
                0x08ea_fd8b,
                0x48ea_fd8b,
                0x88ea_fd8b,
                0xc8ea_fd8b,
                0xc8ea_fd9f, // CASAL*
                0xd503_3bbf,
                0xd503_39bf, // DMB ISH, DMB ISHLD
            ]
        );
    }

    #[test]
    fn s6_41b_atomic_logic_and_flag_forms_match_local_apple_clang_fixture() {
        // Fixture: tests/fixtures/arm64_atomic_logic.s, Apple clang 21.0.0,
        // -target arm64-apple-macosx15.0 -march=armv8.4-a, words read with
        // `otool -tV` / `otool -t`. Order below equals the fixture order.
        use AccessWidth::{Byte, Dword, Half, Word};
        use AtomicOrder::{AcquireRelease as Al, Relaxed};
        use Shift::{Lsl, Lsr};
        let mut e = Encoder::new();
        let r = |n: u8| XRegOrZero::Reg(x(n));
        let z = XRegOrZero::Zero;
        let b12 = AddressBase::Reg(x(12));
        for order in [Relaxed, Al] {
            for width in [Byte, Half, Word, Dword] {
                e.ldset(width, order, r(10), x(11), b12);
            }
        }
        e.ldset(Dword, Al, z, x(11), AddressBase::Sp);
        for order in [Relaxed, Al] {
            for width in [Byte, Half, Word, Dword] {
                e.ldclr(width, order, r(10), x(11), b12);
            }
        }
        e.orr_x(x(9), r(10), r(11), Lsl, 0).unwrap();
        e.orr_x(x(9), r(10), r(11), Lsl, 7).unwrap();
        e.orr_x(x(9), z, r(11), Lsr, 3).unwrap();
        e.eor_x(x(9), r(10), r(11), Lsl, 0).unwrap();
        e.eor_x(x(9), r(9), r(9), Lsr, 4).unwrap();
        e.eor_x(x(9), r(10), z, Lsl, 63).unwrap();
        e.orn_x(x(9), z, r(11), Lsl, 0).unwrap();
        e.orn_x(x(9), r(10), r(11), Lsr, 1).unwrap();
        e.lsl_imm_x(x(9), x(10), 1).unwrap();
        e.lsl_imm_x(x(9), x(10), 56).unwrap();
        e.lsl_imm_x(x(9), x(10), 63).unwrap();
        e.lsr_imm_x(x(9), x(10), 1).unwrap();
        e.lsr_imm_x(x(9), x(10), 32).unwrap();
        e.lsr_imm_x(x(9), x(10), 63).unwrap();
        e.ubfx_x(x(9), x(10), 0, 1).unwrap();
        e.ubfx_x(x(9), x(10), 4, 1).unwrap();
        e.ubfx_x(x(9), x(10), 7, 1).unwrap();
        e.ubfx_x(x(9), x(10), 0, 8).unwrap();
        e.ubfx_x(x(9), x(10), 31, 1).unwrap();
        e.ubfx_x(x(9), x(10), 63, 1).unwrap();
        e.cset_x(x(9), Condition::Equal);
        e.cset_x(x(9), Condition::Lower);
        e.cset_x(x(15), Condition::Equal);
        assert_eq!(
            e.finish().unwrap().words(),
            &[
                0x382a_318b,
                0x782a_318b,
                0xb82a_318b,
                0xf82a_318b, // LDSET*
                0x38ea_318b,
                0x78ea_318b,
                0xb8ea_318b,
                0xf8ea_318b,
                0xf8ff_33eb, // LDSETAL*
                0x382a_118b,
                0x782a_118b,
                0xb82a_118b,
                0xf82a_118b, // LDCLR*
                0x38ea_118b,
                0x78ea_118b,
                0xb8ea_118b,
                0xf8ea_118b, // LDCLRAL*
                0xaa0b_0149,
                0xaa0b_1d49,
                0xaa4b_0fe9, // ORR
                0xca0b_0149,
                0xca49_1129,
                0xca1f_fd49, // EOR
                0xaa2b_03e9,
                0xaa6b_0549, // ORN / MVN
                0xd37f_f949,
                0xd348_1d49,
                0xd341_0149, // LSL
                0xd341_fd49,
                0xd360_fd49,
                0xd37f_fd49, // LSR
                0xd340_0149,
                0xd344_1149,
                0xd347_1d49,
                0xd340_1d49,
                0xd35f_7d49,
                0xd37f_fd49, // UBFX
                0x9a9f_17e9,
                0x9a9f_27e9,
                0x9a9f_17ef, // CSET
            ]
        );
    }

    #[test]
    fn s6_41b_shift_and_bitfield_forms_reject_out_of_range_operands() {
        let mut e = Encoder::new();
        let r = XRegOrZero::Reg(x(1));
        assert!(e.orr_x(x(0), r, r, Shift::Lsl, 64).is_err());
        assert!(e.lsl_imm_x(x(0), x(1), 0).is_err());
        assert!(e.lsr_imm_x(x(0), x(1), 64).is_err());
        assert!(e.ubfx_x(x(0), x(1), 63, 2).is_err());
        assert!(e.ubfx_x(x(0), x(1), 0, 0).is_err());
        assert_eq!(e.position(), 0);
    }

    #[test]
    fn s6_41_sized_loads_and_stores_scale_and_reject_bad_offsets() {
        let mut e = Encoder::new();
        let base = AddressBase::Reg(x(1));
        let t = XRegOrZero::Reg(x(2));
        // Dword forms coincide with the pre-existing X-register encoders.
        e.ldr_sized(AccessWidth::Dword, t, base, 32).unwrap();
        e.str_sized(AccessWidth::Dword, t, base, 32).unwrap();
        let mut reference = Encoder::new();
        reference.ldr_x(t, base, 32).unwrap();
        reference.str_x(t, base, 32).unwrap();
        assert_eq!(e.finish().unwrap(), reference.finish().unwrap());

        let mut e = Encoder::new();
        assert_eq!(
            e.ldr_sized(AccessWidth::Word, t, base, 2),
            Err(EncodeError::UnalignedOffset {
                value: 2,
                alignment: 4
            })
        );
        assert_eq!(
            e.str_sized(AccessWidth::Half, t, base, 4096 * 2),
            Err(EncodeError::ImmediateOutOfRange {
                value: 4096,
                maximum: 4095
            })
        );
        assert_eq!(e.position(), 0);
        assert_eq!(AccessWidth::Byte.bytes(), 1);
        assert_eq!(AccessWidth::Dword.bytes(), 8);
    }
}
