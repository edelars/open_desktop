//! Caller-selected local MSR probe policies for the reference interpreter.
//!
//! `SysenterProbeV1` models a fault-only safe SYSENTER probe, while the FS/GS
//! profiles model one explicit base read/write each. None enables general MSR
//! support.

/// Immutable MSR access policy explicitly selected by the caller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsrProfile {
    /// Fault RDMSR/WRMSR to IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`).
    SysenterProbeV1,
    /// CPL0-only access to IA32_GS_BASE (`0xc000_0101`).
    GsBaseV1,
    /// CPL0-only access to IA32_FS_BASE (`0xc000_0100`).
    FsBaseV1,
    /// CPL0-only access to IA32_KERNEL_GS_BASE (`0xc000_0102`).
    KernelGsBaseV1,
    /// Read-only CPL0 access to a caller-selected synthetic IA32_EFER value.
    EferReadOnlyV1(u64),
}

impl MsrProfile {
    /// Whether this profile handles `index`.
    pub const fn handles_index(self, index: u32) -> bool {
        match self {
            Self::SysenterProbeV1 => matches!(index, 0x174..=0x176),
            Self::GsBaseV1 => index == 0xc000_0101,
            Self::FsBaseV1 => index == 0xc000_0100,
            Self::KernelGsBaseV1 => index == 0xc000_0102,
            Self::EferReadOnlyV1(_) => index == 0xc000_0080,
        }
    }

    /// Whether this exact local EFER value belongs to the bounded S2-25e domain.
    pub const fn accepts_efer_value(value: u64) -> bool {
        matches!(value, 0x500 | 0x501 | 0xd00 | 0xd01)
    }
}

/// S2-26h: the Windows-boot MSR file of the CPL0 profile set (`Cpl0ProfileSet::with_msr_windows`).
///
/// The list is the set of MSRs a Windows x64 kernel/loader is expected to touch during boot
/// (a hypothesis: ADR-0013, Decision 2, "Consequences of variant B", the bullet on Intel kernel/loader
/// paths says the list and numbers of MSRs are not verified; no Windows binary was observed): IA32_APIC_BASE (`0x1B`),
/// IA32_MISC_ENABLE (`0x1A0`), IA32_PAT (`0x277`), IA32_SYSENTER_CS/ESP/EIP (`0x174..=0x176`),
/// IA32_EFER (`0xC0000080`), IA32_STAR/LSTAR/CSTAR/FMASK (`0xC0000081..=0xC0000084`) and
/// IA32_FS_BASE/GS_BASE/KERNEL_GS_BASE (`0xC0000100..=0xC0000102`) and, since S7-32 slice F,
/// IA32_TIME_STAMP_COUNTER (`0x10`, SDM Vol. 4 Table 2-2 10H; bootmgfw reads it with `RDMSR`):
/// `RDMSR` returns what `RDTSC` returns at that instruction (the set's RDTSC source), `WRMSR` is an
/// explicit stop (`Unsupported`, the model keeps no TSC offset; hypothesis, not verified: the
/// Windows HAL writes the TSC only to synchronise processors, and this board runs one vCPU);
/// IA32_PLATFORM_ID (`0x17`, read-only, [`IA32_PLATFORM_ID_VALUE`]) and IA32_BIOS_SIGN_ID (`0x8B`,
/// a cell set by the board, `Cpl0ProfileSet::with_msr_bios_sign_id`). Since S7-32 slice I (the
/// Windows profile declares MTRR and MCA, CPUID.1:EDX[12] and [14]) the MTRR file
/// ([`MtrrState::handles`]) and the MCA file ([`McaState::handles`]), each with a cell set by the
/// board (`Cpl0ProfileSet::with_msr_mtrr`, `Cpl0ProfileSet::with_msr_mca`).
pub const fn windows_msr_is_known(index: u32) -> bool {
    matches!(
        index,
        0x10 | 0x17
            | 0x1b
            | 0x8b
            | 0x1a0
            | 0x277
            | 0x174..=0x176
            | 0xc000_0080..=0xc000_0084
            | 0xc000_0100..=0xc000_0102
    ) || MtrrState::handles(index)
        || McaState::handles(index)
}

/// Result of a `WRMSR` to one of the S7-32 slice I files (MTRR, MCA).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MsrWrite {
    /// The file takes the value.
    Accept,
    /// Architectural `#GP(0)` (SDM Vol. 2D WRMSR: a reserved bit set, an invalid memory type, a
    /// read-only MSR, or a value the MSR cannot be written with); the file is unchanged.
    GeneralProtection,
}

/// MAXPHYADDR of the profiles (CPUID.80000008H:EAX[7:0] = 36, `0x3024`): the physical-address bits
/// of PHYSBASE/PHYSMASK above it are reserved (SDM Vol. 3A, "Variable Range MTRRs").
const MAXPHYADDR: u32 = 36;
/// Bits MAXPHYADDR-1:12 of a variable-range MTRR (PhysBase, PhysMask).
const MTRR_PHYS_ADDRESS: u64 = ((1u64 << MAXPHYADDR) - 1) & !0xfff;

/// IA32_MTRRCAP (SDM Vol. 4 Table 2-2, FEH): read-only.
pub const IA32_MTRRCAP: u32 = 0xfe;
/// IA32_MTRR_DEF_TYPE (SDM Vol. 4 Table 2-2, 2FFH).
pub const IA32_MTRR_DEF_TYPE: u32 = 0x2ff;
/// IA32_MTRR_PHYSBASE0 (200H); PHYSMASKn is at `0x201 + 2n`, PHYSBASEn at `0x200 + 2n`.
pub const IA32_MTRR_PHYSBASE0: u32 = 0x200;
/// The number of variable-range MTRR pairs of the virtual CPU (S7-32 slice I decision): 8, the
/// IA32_MTRRCAP.VCNT the model reports. A model choice, not the physical N5105's value (not
/// sourced): eight pairs cover the board's MMIO hole below 4 GiB with room left for an OS, and it
/// is the count the SDM uses in its examples (SDM Vol. 3A, "Variable Range MTRRs").
pub const MTRR_VARIABLE_COUNT: usize = 8;
/// The fixed-range MTRRs (SDM Vol. 3A "Fixed Range MTRRs", Vol. 4 Table 2-2): FIX64K_00000 (250H),
/// FIX16K_80000 (258H), FIX16K_A0000 (259H) and FIX4K_C0000..FIX4K_F8000 (268H..26FH).
pub const MTRR_FIXED_INDEXES: [u32; 11] = [
    0x250, 0x258, 0x259, 0x268, 0x269, 0x26a, 0x26b, 0x26c, 0x26d, 0x26e, 0x26f,
];
/// IA32_MTRRCAP.FIX (bit 8): the fixed-range MTRRs exist. WC (bit 10): the write-combining type may
/// be used. SMRR (bit 11) and the rest are 0: no SMM and no SMRR in the model.
const MTRRCAP_FIX: u64 = 1 << 8;
const MTRRCAP_WC: u64 = 1 << 10;
/// The value of IA32_MTRRCAP (S7-32 slice I decision): VCNT = [`MTRR_VARIABLE_COUNT`], FIX and WC
/// (`0x508`).
pub const IA32_MTRRCAP_VALUE: u64 = MTRR_VARIABLE_COUNT as u64 | MTRRCAP_FIX | MTRRCAP_WC;
/// IA32_MTRR_DEF_TYPE: Type (7:0), FE (10, fixed-range enable), E (11, MTRR enable); the rest is
/// reserved (SDM Vol. 3A, figure "IA32_MTRR_DEF_TYPE MSR").
const MTRR_DEF_TYPE_WRITABLE: u64 = 0xff | (1 << 10) | (1 << 11);
/// PHYSMASKn.V (bit 11): the pair is valid.
const MTRR_PHYSMASK_VALID: u64 = 1 << 11;

/// Whether `memory_type` is a type an MTRR field may hold (SDM Vol. 3A, table "Memory Types That
/// Can Be Encoded in MTRRs"): UC 0, WC 1 (WC is reported by IA32_MTRRCAP), WT 4, WP 5, WB 6; 2, 3
/// and 7..=0xFF are reserved (#GP(0)).
const fn mtrr_type_valid(memory_type: u64) -> bool {
    matches!(memory_type, 0 | 1 | 4 | 5 | 6)
}

/// The guest-visible MTRR file of one vCPU (S7-32 slice I; SDM Vol. 3A "Memory Type Range Registers
/// (MTRRs)"). The values are stored and checked as the SDM defines them; the memory types they
/// select have no effect: the model has no caches and runs every access in order, so UC, WC, WT, WP
/// and WB memory behave alike. At power-up and reset the MTRRs are disabled (Vol. 3A "MTRR
/// Initialization": E of IA32_MTRR_DEF_TYPE and V of every PHYSMASKn clear; the other bits are
/// undefined, the model gives 0), which is [`MtrrState::default`]. INIT does not change them
/// (Vol. 3A, table "IA-32 and Intel 64 Processor States Following Power-up, Reset, or INIT").
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MtrrState {
    def_type: u64,
    /// PHYSBASE0, PHYSMASK0, PHYSBASE1, ... in MSR order (`index - 0x200`).
    variable: [u64; 2 * MTRR_VARIABLE_COUNT],
    /// In the order of [`MTRR_FIXED_INDEXES`].
    fixed: [u64; MTRR_FIXED_INDEXES.len()],
}

impl MtrrState {
    /// The indexes of the file: IA32_MTRRCAP, IA32_MTRR_DEF_TYPE, the [`MTRR_VARIABLE_COUNT`] pairs
    /// and the fixed-range MTRRs. Variable pairs above VCNT (`0x210..`) are not in the file: on the
    /// declared CPU they do not exist (an unknown index).
    pub const fn handles(index: u32) -> bool {
        index == IA32_MTRRCAP
            || index == IA32_MTRR_DEF_TYPE
            || Self::variable_slot(index).is_some()
            || Self::fixed_slot(index).is_some()
    }

    const fn variable_slot(index: u32) -> Option<usize> {
        if index >= IA32_MTRR_PHYSBASE0
            && index < IA32_MTRR_PHYSBASE0 + 2 * MTRR_VARIABLE_COUNT as u32
        {
            Some((index - IA32_MTRR_PHYSBASE0) as usize)
        } else {
            None
        }
    }

    const fn fixed_slot(index: u32) -> Option<usize> {
        let mut slot = 0;
        while slot < MTRR_FIXED_INDEXES.len() {
            if MTRR_FIXED_INDEXES[slot] == index {
                return Some(slot);
            }
            slot += 1;
        }
        None
    }

    /// `RDMSR` of `index`: `None` when the index is not in the file.
    pub const fn read(&self, index: u32) -> Option<u64> {
        if index == IA32_MTRRCAP {
            return Some(IA32_MTRRCAP_VALUE);
        }
        if index == IA32_MTRR_DEF_TYPE {
            return Some(self.def_type);
        }
        if let Some(slot) = Self::variable_slot(index) {
            return Some(self.variable[slot]);
        }
        if let Some(slot) = Self::fixed_slot(index) {
            return Some(self.fixed[slot]);
        }
        None
    }

    /// `WRMSR` of `value` to `index` (SDM Vol. 3A "Memory Type Range Registers (MTRRs)", Vol. 2D
    /// WRMSR): `None` when the index is not in the file. `#GP(0)` without a change for IA32_MTRRCAP
    /// (read-only), a reserved bit (DEF_TYPE bits 9:8 and 63:12; PHYSBASE bits 11:8 and 63:36;
    /// PHYSMASK bits 10:0 and 63:36) or a reserved memory type in DEF_TYPE, PHYSBASE or any byte of
    /// a fixed-range MTRR.
    pub fn write(&mut self, index: u32, value: u64) -> Option<MsrWrite> {
        let valid = if index == IA32_MTRRCAP {
            false
        } else if index == IA32_MTRR_DEF_TYPE {
            value & !MTRR_DEF_TYPE_WRITABLE == 0 && mtrr_type_valid(value & 0xff)
        } else if let Some(slot) = Self::variable_slot(index) {
            if slot % 2 == 0 {
                value & !(MTRR_PHYS_ADDRESS | 0xff) == 0 && mtrr_type_valid(value & 0xff)
            } else {
                value & !(MTRR_PHYS_ADDRESS | MTRR_PHYSMASK_VALID) == 0
            }
        } else if Self::fixed_slot(index).is_some() {
            let mut byte = 0;
            let mut ok = true;
            while byte < 8 {
                ok &= mtrr_type_valid((value >> (8 * byte)) & 0xff);
                byte += 1;
            }
            ok
        } else {
            return None;
        };
        if !valid {
            return Some(MsrWrite::GeneralProtection);
        }
        if index == IA32_MTRR_DEF_TYPE {
            self.def_type = value;
        } else if let Some(slot) = Self::variable_slot(index) {
            self.variable[slot] = value;
        } else if let Some(slot) = Self::fixed_slot(index) {
            self.fixed[slot] = value;
        }
        Some(MsrWrite::Accept)
    }

    /// IA32_MTRR_DEF_TYPE.
    pub const fn def_type(&self) -> u64 {
        self.def_type
    }

    /// Variable pair `n` as `(PHYSBASEn, PHYSMASKn)`.
    pub const fn variable_pair(&self, n: usize) -> (u64, u64) {
        (self.variable[2 * n], self.variable[2 * n + 1])
    }
}

/// IA32_MCG_CAP (SDM Vol. 4 Table 2-2, 179H): read-only.
pub const IA32_MCG_CAP: u32 = 0x179;
/// IA32_MCG_STATUS (17AH).
pub const IA32_MCG_STATUS: u32 = 0x17a;
/// IA32_MC0_CTL (400H); bank `i` has CTL, STATUS, ADDR, MISC at `0x400 + 4i ..= 0x403 + 4i`.
pub const IA32_MC0_CTL: u32 = 0x400;
/// The number of MCA error-reporting banks of the virtual CPU (S7-32 slice I decision):
/// IA32_MCG_CAP.Count = 4. A model choice (the physical N5105's count is not sourced): a small
/// non-zero count gives an OS the architectural MCA it initialises (CTL of every bank, STATUS
/// cleared) without banks that would mean anything, since no error is ever logged.
pub const MCA_BANK_COUNT: usize = 4;
/// The value of IA32_MCG_CAP (S7-32 slice I decision): Count = [`MCA_BANK_COUNT`]; MCG_CTL_P (8),
/// MCG_EXT_P (9), MCG_CMCI_P (10), MCG_TES_P (11), MCG_SER_P (24), MCG_EMC_P (25), MCG_ELOG_P (26)
/// and MCG_LMCE_P (27) are 0, so IA32_MCG_CTL, IA32_MCi_CTL2, the extended state MSRs and LMCE do
/// not exist (SDM Vol. 3B, "IA32_MCG_CAP MSR").
pub const IA32_MCG_CAP_VALUE: u64 = MCA_BANK_COUNT as u64;
/// IA32_MCG_STATUS: RIPV (0), EIPV (1) and MCIP (2); LMCE_S (3) exists only with MCG_LMCE_P and the
/// rest is reserved (SDM Vol. 3B, "IA32_MCG_STATUS MSR").
const MCG_STATUS_WRITABLE: u64 = 0b111;

/// The guest-visible machine-check architecture of one vCPU (S7-32 slice I; SDM Vol. 3B
/// "Machine-Check MSRs"). No hardware error is modelled, so no bank ever logs one and the model
/// never raises #MC: every IA32_MCi_STATUS, IA32_MCi_ADDR and IA32_MCi_MISC reads 0, and only 0 may
/// be written to them (SDM Vol. 3B, "IA32_MCi_STATUS MSRs", "IA32_MCi_ADDR MSRs" and "IA32_MCi_MISC
/// MSRs": software clears them by writing 0s; writing 1s is #GP). IA32_MCi_CTL (the error-reporting
/// enables) and IA32_MCG_STATUS are stored as written. Power-up values are 0
/// ([`McaState::default`]); INIT does not change them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct McaState {
    mcg_status: u64,
    bank_ctl: [u64; MCA_BANK_COUNT],
}

impl McaState {
    /// The indexes of the file: IA32_MCG_CAP, IA32_MCG_STATUS and the four MSRs of each of the
    /// [`MCA_BANK_COUNT`] banks. IA32_MCG_CTL (17BH, MCG_CTL_P = 0), banks above the count and
    /// IA32_MCi_CTL2 (MCG_CMCI_P = 0) are not in the file: on the declared CPU they do not exist.
    pub const fn handles(index: u32) -> bool {
        index == IA32_MCG_CAP || index == IA32_MCG_STATUS || Self::bank_register(index).is_some()
    }

    /// `(bank, register)` of a bank MSR: register 0 CTL, 1 STATUS, 2 ADDR, 3 MISC.
    const fn bank_register(index: u32) -> Option<(usize, u32)> {
        if index >= IA32_MC0_CTL && index < IA32_MC0_CTL + 4 * MCA_BANK_COUNT as u32 {
            let offset = index - IA32_MC0_CTL;
            Some(((offset / 4) as usize, offset % 4))
        } else {
            None
        }
    }

    /// `RDMSR` of `index`: `None` when the index is not in the file.
    pub const fn read(&self, index: u32) -> Option<u64> {
        if index == IA32_MCG_CAP {
            return Some(IA32_MCG_CAP_VALUE);
        }
        if index == IA32_MCG_STATUS {
            return Some(self.mcg_status);
        }
        match Self::bank_register(index) {
            Some((bank, 0)) => Some(self.bank_ctl[bank]),
            Some(_) => Some(0),
            None => None,
        }
    }

    /// `WRMSR` of `value` to `index`: `None` when the index is not in the file. `#GP(0)` without a
    /// change for IA32_MCG_CAP (read-only), a reserved bit of IA32_MCG_STATUS (63:3) and a non-zero
    /// value to IA32_MCi_STATUS, IA32_MCi_ADDR or IA32_MCi_MISC; IA32_MCi_CTL takes any value (all
    /// 64 enable bits are writable).
    pub fn write(&mut self, index: u32, value: u64) -> Option<MsrWrite> {
        let accepted = if index == IA32_MCG_CAP {
            false
        } else if index == IA32_MCG_STATUS {
            let ok = value & !MCG_STATUS_WRITABLE == 0;
            if ok {
                self.mcg_status = value;
            }
            ok
        } else {
            match Self::bank_register(index) {
                Some((bank, 0)) => {
                    self.bank_ctl[bank] = value;
                    true
                }
                Some(_) => value == 0,
                None => return None,
            }
        };
        Some(if accepted {
            MsrWrite::Accept
        } else {
            MsrWrite::GeneralProtection
        })
    }

    /// IA32_MCi_CTL of `bank`.
    pub const fn bank_ctl(&self, bank: usize) -> u64 {
        self.bank_ctl[bank]
    }

    /// IA32_MCG_STATUS.
    pub const fn mcg_status(&self) -> u64 {
        self.mcg_status
    }
}

/// IA32_TIME_STAMP_COUNTER (SDM Vol. 4 Table 2-2, 10H).
pub const IA32_TIME_STAMP_COUNTER: u32 = 0x10;

/// IA32_PLATFORM_ID (SDM Vol. 4 Table 2-2, 17H): read-only; bits 52:50 give the platform ID that
/// selects a microcode update (SDM Vol. 3A §10.11.4).
pub const IA32_PLATFORM_ID: u32 = 0x17;

/// The value of IA32_PLATFORM_ID of the virtual CPU (S7-32 slice F decision): 0, platform ID 0 in
/// bits 52:50. A virtual CPU belongs to no physical platform; with ID 0 a microcode update matches
/// only if its processor-flags field has bit 0 set (SDM Vol. 3A §10.11.4), and the board reports no
/// update loaded (IA32_BIOS_SIGN_ID).
pub const IA32_PLATFORM_ID_VALUE: u64 = 0;

/// IA32_BIOS_SIGN_ID (SDM Vol. 4 Table 2-2, 8BH): EDX holds the signature (revision) of the loaded
/// microcode update after `WRMSR 0` and `CPUID(1)` (SDM Vol. 3A §10.11.7.1).
pub const IA32_BIOS_SIGN_ID: u32 = 0x8b;

/// Indexes of the Windows list whose value lives in a cell owned by the Windows member itself or
/// by one of the guest MSR cells of the set (everything except APIC_BASE, EFER, FS_BASE, GS_BASE).
pub const fn windows_msr_is_cell_owned(index: u32) -> bool {
    matches!(
        index,
        0x1a0 | 0x277 | 0x174..=0x176 | 0xc000_0081..=0xc000_0084 | 0xc000_0102
    )
}

/// IA32_PAT at power-up (SDM Vol. 3A 14.12.4): PA0 = WB, PA1 = WT, PA2 = UC-, PA3 = UC, repeated.
pub const IA32_PAT_RESET: u64 = 0x0007_0406_0007_0406;
/// IA32_MISC_ENABLE of the virtual CPU (SDM Vol. 4 Table 2-2, IA32_MISC_ENABLE, 1A0H): bit 0
/// (fast-string enable, R/W, default 1) is the only writable bit; bit 11 (BTS unavailable, R/O) and
/// bit 12 (PEBS unavailable, R/O) read as 1 because the model has neither; the performance
/// monitoring bit 7 (R) reads 0 (no PMU); EIST (16), MONITOR/MWAIT (18), Limit CPUID Maxval (22),
/// xTPR disable (23), thermal control (3) and every reserved bit read as 0.
pub const IA32_MISC_ENABLE_RESET: u64 = 0x1801;
const IA32_MISC_ENABLE_WRITABLE: u64 = 1;
/// The read-only bits of IA32_MISC_ENABLE that always read 1 (BTS and PEBS unavailable).
const IA32_MISC_ENABLE_FIXED: u64 = 0x1800;

/// Result of a Windows-list `WRMSR` against the current cell value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowsMsrWrite {
    /// The cell takes the value.
    Accept,
    /// Architectural `#GP(0)`: reserved bits set or a non-canonical address.
    GeneralProtection,
    /// The value touches a bit the model does not implement: fail closed, no state change.
    Unsupported,
}

const fn canonical48(value: u64) -> bool {
    let high = value >> 47;
    high == 0 || high == 0x1_ffff
}

/// Whether every PAT entry holds an encodable memory type (SDM Vol. 3A Table 14-10: 0, 1, 4, 5, 6,
/// 7; the five high bits of each byte are reserved).
const fn pat_value_valid(value: u64) -> bool {
    let mut entry = 0;
    while entry < 8 {
        let byte = (value >> (entry * 8)) & 0xff;
        if !matches!(byte, 0 | 1 | 4 | 5 | 6 | 7) {
            return false;
        }
        entry += 1;
    }
    true
}

/// Write rules of the cell-owned Windows MSRs (SDM Vol. 2D WRMSR: reserved bits and non-canonical
/// IA32_LSTAR/SYSENTER_ESP/SYSENTER_EIP/KERNEL_GS_BASE raise `#GP(0)`).
///
/// - SYSENTER_CS: every value is accepted; bits 63:32 are "not used, writes ignored, reads return
///   zero" (SDM Vol. 4 Table 2-2, 174H), so the cell takes [`windows_msr_stored_value`].
/// - SYSENTER_ESP/EIP, LSTAR, KERNEL_GS_BASE: canonical 48-bit address.
/// - STAR: bits 31:0 are Reserved in SDM Vol. 3A Figure 6-14 (Vol. 4 gives no bit breakdown for
///   C000_0081H), so a non-zero low half is `#GP(0)`. The hardware behaviour was not checked: the
///   check needs CPL0 and the oracle runs in user space.
/// - CSTAR: any value (not in the WRMSR canonical list; Vol. 4: "not used").
/// - FMASK: bits 63:32 reserved (SDM Vol. 3A Figure 6-14).
/// - PAT: every entry an encodable type (Table 14-10).
/// - MISC_ENABLE: only bit 0 may change; a write that would change any other bit (a reserved
///   bit, or the read-only bits 11/12) is `Unsupported` with no state change. That is a deliberate
///   fail-closed decision of the model, not `#GP(0)`: the SDM does not say what such a write does
///   on every part, and the model does not guess.
pub const fn windows_msr_write_decision(index: u32, current: u64, value: u64) -> WindowsMsrWrite {
    let ok = match index {
        0x174 | 0xc000_0083 => true,
        0xc000_0084 => value >> 32 == 0,
        0xc000_0081 => value as u32 == 0,
        0x175 | 0x176 | 0xc000_0082 | 0xc000_0102 => canonical48(value),
        0x277 => pat_value_valid(value),
        0x1a0 => {
            return if (current ^ value) & !IA32_MISC_ENABLE_WRITABLE == 0 {
                WindowsMsrWrite::Accept
            } else {
                WindowsMsrWrite::Unsupported
            };
        }
        _ => return WindowsMsrWrite::Unsupported,
    };
    if ok {
        WindowsMsrWrite::Accept
    } else {
        WindowsMsrWrite::GeneralProtection
    }
}

/// The value a cell takes after an accepted `WRMSR`: SYSENTER_CS keeps only bits 31:0 (bits 63:32
/// are ignored on write and read as zero), every other cell takes the value as is.
pub const fn windows_msr_stored_value(index: u32, value: u64) -> u64 {
    if index == 0x174 {
        value & 0xffff_ffff
    } else {
        value
    }
}

/// Whether a caller-supplied cell value is a state the Windows member can hold. A SYSENTER_CS cell
/// with bits 63:32 set is refused (the register never holds them), and MISC_ENABLE must be bit 0
/// plus the two fixed read-only bits.
pub const fn windows_msr_cell_value_valid(index: u32, value: u64) -> bool {
    match index {
        0x174 => value >> 32 == 0,
        0x1a0 => {
            value & !(IA32_MISC_ENABLE_WRITABLE | IA32_MISC_ENABLE_FIXED) == 0
                && value & IA32_MISC_ENABLE_FIXED == IA32_MISC_ENABLE_FIXED
        }
        _ => matches!(
            windows_msr_write_decision(index, value, value),
            WindowsMsrWrite::Accept
        ),
    }
}

/// CPU-owned state for the separate, exact IA32_APIC_BASE profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApicBaseCpu {
    pub state: crate::LongModeState,
    apic_base: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ApicBaseEffect {
    pub old: u64,
    pub new: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApicBaseExit {
    Read,
    Write(ApicBaseEffect),
    Stop,
    Budget,
    Fault(crate::ArchitecturalFault),
    Unsupported,
    Memory(crate::MemoryError),
}

impl ApicBaseCpu {
    pub const ENABLED: u64 = 0xfee0_0900;
    pub const DISABLED: u64 = 0xfee0_0100;

    pub fn new(state: crate::LongModeState) -> Self {
        Self {
            state,
            apic_base: Self::ENABLED,
        }
    }

    pub fn apic_base(&self) -> u64 {
        self.apic_base
    }

    /// Execute exactly one bare Long64 RDMSR/WRMSR; callers must apply a
    /// returned write effect before executing another instruction or device operation.
    pub fn step(
        &mut self,
        memory: &mut impl crate::GuestMemory,
        context: crate::AccessContext,
        budget: u64,
        stop: bool,
    ) -> ApicBaseExit {
        use crate::long_decode::{self, DecodeError, Operation, Prefixes};
        if stop {
            return ApicBaseExit::Stop;
        }
        if budget == 0 {
            return ApicBaseExit::Budget;
        }
        if self
            .state
            .activity
            .is_some_and(|activity| activity != crate::CpuActivity::Running)
        {
            return ApicBaseExit::Unsupported;
        }
        if self.state.sti_shadow == Some(crate::StiShadow::Blocking) {
            return ApicBaseExit::Unsupported;
        }
        if context.mode != crate::InstructionMode::Long64 {
            return ApicBaseExit::Unsupported;
        }
        let instruction = match long_decode::decode(memory, self.state.rip, context) {
            Ok(i) => i,
            Err(DecodeError::Memory(crate::MemoryError::Architectural(f))) => {
                return ApicBaseExit::Fault(f);
            }
            Err(DecodeError::Memory(e)) => return ApicBaseExit::Memory(e),
            Err(
                DecodeError::Invalid { .. }
                | DecodeError::InvalidMovCr0Lock { .. }
                | DecodeError::InvalidMovCr3Lock { .. }
                | DecodeError::InvalidLgdtLock { .. }
                | DecodeError::InvalidLidtLock { .. },
            ) => return ApicBaseExit::Unsupported,
            Err(DecodeError::Unsupported { .. }) => return ApicBaseExit::Unsupported,
        };
        if instruction.length != 2
            || instruction.prefixes != Prefixes::default()
            || !matches!(
                instruction.operation,
                Operation::ReadMsr | Operation::WriteMsr
            )
            || self.state.gpr[crate::Register::Rcx as usize] as u32 != 0x1b
        {
            return ApicBaseExit::Unsupported;
        }
        let gp = || {
            ApicBaseExit::Fault(crate::ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None,
            })
        };
        if context.privilege != crate::PrivilegeLevel::Ring0 {
            return gp();
        }
        let result = match instruction.operation {
            Operation::ReadMsr => {
                self.state.gpr[crate::Register::Rax as usize] = u64::from(self.apic_base as u32);
                self.state.gpr[crate::Register::Rdx as usize] = self.apic_base >> 32;
                ApicBaseExit::Read
            }
            Operation::WriteMsr => {
                let value = u64::from(self.state.gpr[crate::Register::Rax as usize] as u32)
                    | (u64::from(self.state.gpr[crate::Register::Rdx as usize] as u32) << 32);
                // Bits 0..7, 9, and physical-address bits above bit 35 are reserved in this profile.
                if value & (0xff | (1 << 9) | (!0u64 << 36)) != 0 {
                    return gp();
                }
                if !matches!(value, Self::ENABLED | Self::DISABLED) {
                    return ApicBaseExit::Unsupported;
                }
                let effect = ApicBaseEffect {
                    old: self.apic_base,
                    new: value,
                };
                self.apic_base = value;
                ApicBaseExit::Write(effect)
            }
            _ => unreachable!(),
        };
        self.state.rip = self.state.rip.wrapping_add(2);
        result
    }
}

#[cfg(test)]
mod apic_base_tests {
    use super::*;
    use crate::{AccessContext, GuestMemory, LongModeState, MemoryError, PrivilegeLevel, Register};

    struct Code(Vec<u8>);
    impl GuestMemory for Code {
        fn atomic_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn atomic_fetch_add_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn atomic_compare_exchange_integer(
            &mut self,
            _: u64,
            _: AccessContext,
            _: crate::OperandSize,
            _: u64,
            _: u64,
        ) -> Result<u64, MemoryError> {
            unreachable!()
        }
        fn mfence(&mut self, _: AccessContext) -> Result<(), MemoryError> {
            unreachable!()
        }
        fn fetch(&mut self, address: u64, _: AccessContext) -> Result<u8, MemoryError> {
            self.0
                .get(address as usize)
                .copied()
                .ok_or_else(|| MemoryError::Backend("fetch".into()))
        }
        fn read(&mut self, _: u64, _: AccessContext, _: &mut [u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
        fn write(&mut self, _: u64, _: AccessContext, _: &[u8]) -> Result<(), MemoryError> {
            unreachable!()
        }
    }
    fn context() -> AccessContext {
        AccessContext {
            privilege: PrivilegeLevel::Ring0,
            ..AccessContext::default()
        }
    }
    fn cpu() -> ApicBaseCpu {
        let mut state = LongModeState::default();
        state.gpr[Register::Rcx as usize] = 0x1b;
        ApicBaseCpu::new(state)
    }
    fn value(cpu: &mut ApicBaseCpu, value: u64) {
        cpu.state.gpr[Register::Rax as usize] = value as u32 as u64;
        cpu.state.gpr[Register::Rdx as usize] = (value >> 32) as u32 as u64;
    }
    #[test]
    fn blocking_sti_shadow_rejects_before_fetch_and_preserves_cpu() {
        for bytes in [vec![], vec![0x0f, 0x32], vec![0x0f, 0x30]] {
            let mut cpu = cpu();
            cpu.state.sti_shadow = Some(crate::StiShadow::Blocking);
            value(&mut cpu, ApicBaseCpu::DISABLED);
            let before = cpu.clone();
            assert_eq!(
                cpu.step(&mut Code(bytes), context(), 1, false),
                ApicBaseExit::Unsupported
            );
            assert_eq!(cpu, before);
        }
        let mut cpu = cpu();
        cpu.state.sti_shadow = Some(crate::StiShadow::Clear);
        assert_eq!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), context(), 1, false),
            ApicBaseExit::Read
        );
    }

    #[test]
    fn unrelated_invalid_decoder_forms_remain_unsupported() {
        for bytes in [
            &[0xf0, 0x0f, 0x20, 0xc0][..],
            &[0xf0, 0x0f, 0x22, 0xc0][..],
            &[0xf0, 0x0f, 0x22, 0xd8][..],
        ] {
            let mut cpu = cpu();
            let before = cpu.clone();
            assert_eq!(
                cpu.step(&mut Code(bytes.to_vec()), context(), 1, false),
                ApicBaseExit::Unsupported
            );
            assert_eq!(cpu, before);
        }
        let mut cpu = cpu();
        let before = cpu.clone();
        let user_context = AccessContext {
            privilege: PrivilegeLevel::Ring3,
            ..context()
        };
        assert!(matches!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), user_context, 1, false),
            ApicBaseExit::Fault(crate::ArchitecturalFault { vector: 13, .. })
        ));
        assert_eq!(cpu, before);
    }
    #[test]
    fn read_write_and_failures_are_atomic() {
        let mut cpu = cpu();
        cpu.state.gpr[Register::Rax as usize] = u64::MAX;
        cpu.state.gpr[Register::Rdx as usize] = u64::MAX;
        assert_eq!(
            cpu.step(&mut Code(vec![0x0f, 0x32]), context(), 1, false),
            ApicBaseExit::Read
        );
        assert_eq!(cpu.state.gpr[Register::Rax as usize], ApicBaseCpu::ENABLED);
        assert_eq!(cpu.state.gpr[Register::Rdx as usize], 0);
        for target in [
            ApicBaseCpu::DISABLED,
            ApicBaseCpu::ENABLED,
            ApicBaseCpu::ENABLED,
        ] {
            cpu.state.rip = 0;
            value(&mut cpu, target);
            let old = cpu.apic_base();
            assert_eq!(
                cpu.step(&mut Code(vec![0x0f, 0x30]), context(), 1, false),
                ApicBaseExit::Write(ApicBaseEffect { old, new: target })
            );
            assert_eq!(cpu.state.rip, 2);
        }
        for (target, fault) in [
            (ApicBaseCpu::ENABLED | 1, true),
            (ApicBaseCpu::ENABLED | (1 << 36), true),
            (ApicBaseCpu::ENABLED ^ 0x1000, false),
            (ApicBaseCpu::ENABLED ^ 0x100, false),
            (ApicBaseCpu::ENABLED | 0x400, false),
        ] {
            cpu.state.rip = 0;
            value(&mut cpu, target);
            let before = cpu.clone();
            let result = cpu.step(&mut Code(vec![0x0f, 0x30]), context(), 1, false);
            assert_eq!(
                matches!(
                    result,
                    ApicBaseExit::Fault(crate::ArchitecturalFault { vector: 13, .. })
                ),
                fault
            );
            if !fault {
                assert_eq!(result, ApicBaseExit::Unsupported);
            }
            assert_eq!(cpu, before);
        }
        let before = cpu.clone();
        assert_eq!(
            cpu.step(&mut Code(vec![]), context(), 0, false),
            ApicBaseExit::Budget
        );
        assert_eq!(
            cpu.step(&mut Code(vec![]), context(), 1, true),
            ApicBaseExit::Stop
        );
        assert!(matches!(
            cpu.step(&mut Code(vec![0x0f]), context(), 1, false),
            ApicBaseExit::Memory(_)
        ));
        assert_eq!(cpu, before);
    }
}
