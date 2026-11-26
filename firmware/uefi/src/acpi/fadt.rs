//! FADT (Fixed ACPI Description Table, signature `FACP`) revision 6 without `HW_REDUCED_ACPI`,
//! and the FACS it points to (S4-31e).
//!
//! Both are `const fn` serializers (method A of S4-31): the bytes depend only on the validated
//! PM entry of the board manifest ([`AcpiPmEntry`], handoff code 9) and on the two table
//! addresses the caller passes; nothing is read from the machine at run time.
//!
//! Layout and flags follow ACPI 6.6 §5.2.9 and §5.2.10:
//! <https://uefi.org/specs/ACPI/6.6/05_ACPI_Software_Programming_Model.html>.
//! The FADT length is 276 bytes, including the eight-byte hypervisor identity
//! field. FACS revision 2 occupies 64 bytes.
//!
//! Fields (see MACHINE_CONTRACT.md, "ACPI fixed hardware (S7-14a)" and "S4-31 hand-over"):
//! `SMI_CMD`, `ACPI_ENABLE/DISABLE`, `PM1b_*`, `PM2_CNT_BLK`, `GPE0/1` and their lengths are 0;
//! `PM1a_EVT_BLK`/`PM1a_CNT_BLK`/`PM_TMR_BLK` and `RESET_REG`/`RESET_VALUE` come from the PM
//! entry; the 64-bit `X_*` copies equal the 32-bit fields (ACPICA converts a legacy field to the
//! `X_` one the same way, `tbfadt.c`, and a mismatch would be reported by it).
//!
//! Access sizes of the `X_` GAS (S7-32 slice L, ISSUE-698): §5.2.3.2 Table 5.1 (GAS) gives the
//! access size of a register (1 byte, 2 word, 3 dword; 0 "undefined" for legacy reasons). The PM1
//! event block holds two registers of one size, `PM1_STS` and `PM1_EN` (§4.8.1.1; length 4, so 16
//! bits each), `PM1_CNT` is 16 bits (§4.8.1.2), the PM timer "is accessed as 32 bits" (§4.8.4.3);
//! so: word, word and dword. These numbers and words were checked in ACPI 6.4 (Context7); the 6.6
//! pages refused automated reading (HTTP 403). The board's PM block takes the same widths for a
//! whole register (`od-machine` `acpi_pm.rs`). The Windows HAL (FADT revision >= 5) reads these
//! GAS and takes access size 0 for one byte: with 0 it stops with bugcheck 0x5C.

use super::bytes::{bytes_eq, copy_at, fill_range, head_mut, sub, tri};
use super::manifest::{AcpiPmEntry, PM_TMR_LEN, PM1_CNT_LEN, PM1_EVT_LEN, RESET_REG_LEN};
use super::table::{SDT_HEADER_LEN, TableError, finish_checksum, validate_table, write_header};

// --- FADT ------------------------------------------------------------------------------------

/// FADT signature (ACPI Table 5.9).
pub const FADT_SIGNATURE: [u8; 4] = *b"FACP";
/// FADT major version (the table revision byte).
pub const FADT_REVISION: u8 = 6;
/// Offset 131 follows the literal ACPI 6.6 Table 5.9 entry: minor 5, errata 0.
pub const FADT_MINOR_VERSION: u8 = 5;
/// Length of a revision-6 FADT.
pub const FADT_LEN: usize = 276;
/// OEM Table ID of the FADT: an own string, 8 bytes.
pub const FADT_OEM_TABLE_ID: [u8; 8] = *b"ODFWFACP";

/// `Preferred_PM_Profile`: 1 = Desktop (Table 5.11 value; 0 = Unspecified). The board is the
/// machine of a desktop operating system; Linux reads the value only as a hint (for example the
/// default of a CPU frequency driver), so 0 would be equally valid. Pinned by a golden test.
pub const PM_PROFILE_DESKTOP: u8 = 1;
/// `P_LVL2_LAT`/`P_LVL3_LAT`: a value above 100 / 1000 means "C2 / C3 not supported" (ACPI 5.2.9);
/// the board has no processor power states and no `P_BLK`.
pub const P_LVL_NOT_SUPPORTED: u16 = 0x0FFF;

/// WBINVD flushes and invalidates the board's (empty) virtual processor caches.
pub const FLAG_WBINVD: u32 = 1;
pub const FLAG_PWR_BUTTON: u32 = 1 << 4;
pub const FLAG_SLP_BUTTON: u32 = 1 << 5;
pub const FLAG_FIX_RTC: u32 = 1 << 6;
pub const FLAG_TMR_VAL_EXT: u32 = 1 << 8;
pub const FLAG_RESET_REG_SUP: u32 = 1 << 10;
pub const FLAG_HW_REDUCED_ACPI: u32 = 1 << 20;
pub const FADT_FLAGS: u32 =
    FLAG_WBINVD | FLAG_PWR_BUTTON | FLAG_SLP_BUTTON | FLAG_FIX_RTC | FLAG_RESET_REG_SUP;

/// `IAPC_BOOT_ARCH` (Table 5.11, bit offsets). `VGA_NOT_PRESENT` is always set. The builder sets
/// `CMOS_RTC_NOT_PRESENT` only when the handoff has no RTC/CMOS entry (S4-29/S7-14). Zero:
/// `LEGACY_DEVICES` (no user-visible ISA/LPC devices; COM1 is described in the DSDT), `8042`
/// (S4-29 removed the controller), `MSI_NOT_SUPPORTED`, `PCIE_ASPM_CONTROLS`.
pub const IAPC_VGA_NOT_PRESENT: u16 = 1 << 2;
pub const IAPC_CMOS_RTC_NOT_PRESENT: u16 = 1 << 5;
pub const IAPC_BOOT_ARCH: u16 = IAPC_VGA_NOT_PRESENT | IAPC_CMOS_RTC_NOT_PRESENT;
/// CMOS century register advertised through FADT when the RTC is present.
pub const CMOS_CENTURY_REGISTER: u8 = 0x32;

/// Generic Address Structure (Table 5.1): address space ids and access sizes used here.
const GAS_SYSTEM_IO: u8 = 1;
const GAS_ACCESS_BYTE: u8 = 1;
const GAS_ACCESS_WORD: u8 = 2;
const GAS_ACCESS_DWORD: u8 = 3;
/// Access size of `X_PM1a_EVT_BLK`: each of `PM1_STS` and `PM1_EN` is one 16-bit access.
const PM1_EVT_ACCESS: u8 = GAS_ACCESS_WORD;
/// Access size of `X_PM1a_CNT_BLK` (`PM1_CNT`, 16 bits).
const PM1_CNT_ACCESS: u8 = GAS_ACCESS_WORD;
/// Access size of `X_PM_TMR_BLK` (the PM timer, read as 32 bits).
const PM_TMR_ACCESS: u8 = GAS_ACCESS_DWORD;

/// Field offsets of the revision-6 FADT.
mod at {
    pub const FIRMWARE_CTRL: usize = 36;
    pub const DSDT: usize = 40;
    pub const INT_MODEL: usize = 44;
    pub const PM_PROFILE: usize = 45;
    pub const SCI_INT: usize = 46;
    pub const PM1A_EVT_BLK: usize = 56;
    pub const PM1A_CNT_BLK: usize = 64;
    pub const PM_TMR_BLK: usize = 76;
    pub const PM1_EVT_LEN: usize = 88;
    pub const PM1_CNT_LEN: usize = 89;
    pub const PM2_CNT_LEN: usize = 90;
    pub const PM_TMR_LEN: usize = 91;
    pub const P_LVL2_LAT: usize = 96;
    pub const P_LVL3_LAT: usize = 98;
    pub const CENTURY: usize = 108;
    pub const IAPC_BOOT_ARCH: usize = 109;
    pub const FLAGS: usize = 112;
    pub const RESET_REG: usize = 116;
    pub const RESET_VALUE: usize = 128;
    pub const MINOR_VERSION: usize = 131;
    pub const X_FIRMWARE_CTRL: usize = 132;
    pub const X_DSDT: usize = 140;
    pub const X_PM1A_EVT_BLK: usize = 148;
    pub const X_PM1A_CNT_BLK: usize = 172;
    pub const X_PM_TMR_BLK: usize = 208;
}

/// Physical addresses of the tables the FADT points to. The caller (the blob layout, S4-31g)
/// chooses them; the FADT only checks that they lie where the contract puts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FadtAddresses {
    /// Address of the FACS (`FIRMWARE_CTRL` / `X_FIRMWARE_CTRL`).
    pub facs: u64,
    /// Address of the DSDT (`DSDT` / `X_DSDT`).
    pub dsdt: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FadtError {
    Table(TableError),
    /// The SCI GSI does not fit the 8259-mode vector of `SCI_INT` (0..=15; the board has a PIC).
    SciOutOfRange(u32),
    /// The DSDT address is not inside the table part `[0xE_0000, 0xE_F000)` of the ACPI region.
    DsdtOutsideRegion(u64),
    /// The FACS address is not 64-byte aligned inside the NVS part `[0xE_F000, 0xF_0000)` with
    /// room for 64 bytes.
    FacsOutsideRegion(u64),
    /// A finished table: the field named in the text breaks a rule of this board's FADT.
    BadField(&'static str),
}

impl From<TableError> for FadtError {
    fn from(error: TableError) -> Self {
        FadtError::Table(error)
    }
}

/// Builds the FADT into `out[..276]` and returns the length.
///
/// A `const fn`: evaluated at compile time it gives static bytes, called at run time the same
/// bytes. The result is checked by [`validate_fadt`] before it is returned; on an error before
/// any byte is written `out` is untouched.
pub const fn build_fadt(
    pm: &AcpiPmEntry,
    addresses: &FadtAddresses,
    out: &mut [u8],
) -> Result<usize, FadtError> {
    build_fadt_with_rtc(pm, addresses, false, out)
}

/// Builds the FADT with RTC presence derived from handoff device code 10.
pub const fn build_fadt_with_rtc(
    pm: &AcpiPmEntry,
    addresses: &FadtAddresses,
    rtc_present: bool,
    out: &mut [u8],
) -> Result<usize, FadtError> {
    if pm.sci_gsi > 15 {
        return Err(FadtError::SciOutOfRange(pm.sci_gsi));
    }
    if addresses.dsdt < super::blob::ACPI_REGION_BASE
        || addresses.dsdt >= super::blob::ACPI_RECLAIM_END
    {
        return Err(FadtError::DsdtOutsideRegion(addresses.dsdt));
    }
    if addresses.facs < super::blob::ACPI_RECLAIM_END
        || addresses.facs > super::blob::ACPI_REGION_END - FACS_LEN as u64
        || !addresses.facs.is_multiple_of(64)
    {
        return Err(FadtError::FacsOutsideRegion(addresses.facs));
    }
    if out.len() < FADT_LEN {
        return Err(FadtError::Table(TableError::TooShort));
    }
    let table = head_mut(out, FADT_LEN);
    tri!(
        write_header(
            table,
            FADT_SIGNATURE,
            FADT_REVISION,
            FADT_OEM_TABLE_ID,
            FADT_LEN as u32,
        ),
        FadtError::Table
    );
    fill_range(table, SDT_HEADER_LEN, FADT_LEN, 0);

    // 32-bit addresses (both tables sit below 1 MiB, so they fit) and their 64-bit twins.
    copy_at(
        table,
        at::FIRMWARE_CTRL,
        &(addresses.facs as u32).to_le_bytes(),
    );
    copy_at(table, at::DSDT, &(addresses.dsdt as u32).to_le_bytes());
    copy_at(table, at::X_FIRMWARE_CTRL, &addresses.facs.to_le_bytes());
    copy_at(table, at::X_DSDT, &addresses.dsdt.to_le_bytes());

    table[at::INT_MODEL] = 0;
    table[at::PM_PROFILE] = PM_PROFILE_DESKTOP;
    copy_at(table, at::SCI_INT, &(pm.sci_gsi as u16).to_le_bytes());
    // SMI_CMD, ACPI_ENABLE, ACPI_DISABLE, S4BIOS_REQ, PSTATE_CNT: zero (no SMI handler).

    let evt = pm.pm1a_evt_blk as u32;
    let cnt = pm.pm1a_cnt_blk as u32;
    let tmr = pm.pm_tmr_blk as u32;
    copy_at(table, at::PM1A_EVT_BLK, &evt.to_le_bytes());
    copy_at(table, at::PM1A_CNT_BLK, &cnt.to_le_bytes());
    copy_at(table, at::PM_TMR_BLK, &tmr.to_le_bytes());
    // PM1b_EVT_BLK, PM1b_CNT_BLK, PM2_CNT_BLK, GPE0_BLK, GPE1_BLK: zero.
    table[at::PM1_EVT_LEN] = PM1_EVT_LEN as u8;
    table[at::PM1_CNT_LEN] = PM1_CNT_LEN as u8;
    table[at::PM2_CNT_LEN] = 0;
    table[at::PM_TMR_LEN] = PM_TMR_LEN as u8;
    // GPE0_BLK_LEN, GPE1_BLK_LEN, GPE1_BASE, CST_CNT: zero.
    copy_at(table, at::P_LVL2_LAT, &P_LVL_NOT_SUPPORTED.to_le_bytes());
    copy_at(table, at::P_LVL3_LAT, &P_LVL_NOT_SUPPORTED.to_le_bytes());
    // FLUSH_SIZE/STRIDE, DUTY_*, DAY_ALRM, MON_ALRM: zero (no processor ports or RTC alarms).
    table[at::CENTURY] = if rtc_present {
        CMOS_CENTURY_REGISTER
    } else {
        0
    };
    let iapc_boot_arch = if rtc_present {
        IAPC_VGA_NOT_PRESENT
    } else {
        IAPC_BOOT_ARCH
    };
    copy_at(table, at::IAPC_BOOT_ARCH, &iapc_boot_arch.to_le_bytes());
    let mut flags = FADT_FLAGS;
    if pm.reset_reg == 0 {
        flags &= !FLAG_RESET_REG_SUP;
    } else {
        write_gas(
            table,
            at::RESET_REG,
            8,
            GAS_ACCESS_BYTE,
            pm.reset_reg as u64,
        );
        table[at::RESET_VALUE] = pm.reset_value;
    }
    copy_at(table, at::FLAGS, &flags.to_le_bytes());
    // ARM_BOOT_ARCH: zero. Minor version, then the extended copies of the PM registers.
    table[at::MINOR_VERSION] = FADT_MINOR_VERSION;
    write_gas(
        table,
        at::X_PM1A_EVT_BLK,
        (PM1_EVT_LEN * 8) as u8,
        PM1_EVT_ACCESS,
        evt as u64,
    );
    write_gas(
        table,
        at::X_PM1A_CNT_BLK,
        (PM1_CNT_LEN * 8) as u8,
        PM1_CNT_ACCESS,
        cnt as u64,
    );
    write_gas(
        table,
        at::X_PM_TMR_BLK,
        (PM_TMR_LEN * 8) as u8,
        PM_TMR_ACCESS,
        tmr as u64,
    );
    // X_PM1b_*, X_PM2_CNT_BLK, X_GPE0/1_BLK, SLEEP_CONTROL/STATUS_REG and the hypervisor vendor
    // identity: zero.
    tri!(finish_checksum(table), FadtError::Table);
    tri!(validate_fadt(table));
    Ok(FADT_LEN)
}

/// A SystemIO Generic Address Structure (Table 5.1): space id, bit width, bit offset 0, access
/// size, 64-bit address.
const fn write_gas(table: &mut [u8], at: usize, bit_width: u8, access: u8, address: u64) {
    table[at] = GAS_SYSTEM_IO;
    table[at + 1] = bit_width;
    table[at + 2] = 0;
    table[at + 3] = access;
    copy_at(table, at + 4, &address.to_le_bytes());
}

const fn le32(table: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([table[at], table[at + 1], table[at + 2], table[at + 3]])
}

const fn le64(table: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        table[at],
        table[at + 1],
        table[at + 2],
        table[at + 3],
        table[at + 4],
        table[at + 5],
        table[at + 6],
        table[at + 7],
    ])
}

const fn le16(table: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([table[at], table[at + 1]])
}

/// Checks signature, `Length` (exactly 276), checksum, revision 6 and the internal rules of
/// this board's FADT: `HW_REDUCED_ACPI` clear, non-zero table addresses whose 32-bit and
/// 64-bit forms agree, the PM registers equal to their `X_` twins (with the access size of each
/// register) and consistent reset fields.
pub const fn validate_fadt(table: &[u8]) -> Result<(), FadtError> {
    tri!(validate_table(table, &FADT_SIGNATURE), FadtError::Table);
    if table.len() != FADT_LEN {
        // `validate_table` has accepted the header, so its `Length` equals `table.len()`; report
        // the value read from the table, not the constant the rule demands.
        return Err(FadtError::Table(TableError::LengthMismatch {
            declared: le32(table, 4),
            actual: table.len(),
        }));
    }
    if table[8] != FADT_REVISION {
        return Err(FadtError::BadField("revision"));
    }
    let flags = le32(table, at::FLAGS);
    if flags & FLAG_HW_REDUCED_ACPI != 0 {
        return Err(FadtError::BadField("flags: HW_REDUCED_ACPI is set"));
    }
    if le64(table, at::X_FIRMWARE_CTRL) == 0
        || le32(table, at::FIRMWARE_CTRL) as u64 != le64(table, at::X_FIRMWARE_CTRL)
    {
        return Err(FadtError::BadField("FIRMWARE_CTRL / X_FIRMWARE_CTRL"));
    }
    if le64(table, at::X_DSDT) == 0 || le32(table, at::DSDT) as u64 != le64(table, at::X_DSDT) {
        return Err(FadtError::BadField("DSDT / X_DSDT"));
    }
    if !pm_twin(
        table,
        at::PM1A_EVT_BLK,
        at::PM1_EVT_LEN,
        at::X_PM1A_EVT_BLK,
        PM1_EVT_LEN as u8,
        PM1_EVT_ACCESS,
    ) {
        return Err(FadtError::BadField("PM1a_EVT_BLK / X_PM1a_EVT_BLK"));
    }
    if !pm_twin(
        table,
        at::PM1A_CNT_BLK,
        at::PM1_CNT_LEN,
        at::X_PM1A_CNT_BLK,
        PM1_CNT_LEN as u8,
        PM1_CNT_ACCESS,
    ) {
        return Err(FadtError::BadField("PM1a_CNT_BLK / X_PM1a_CNT_BLK"));
    }
    if !pm_twin(
        table,
        at::PM_TMR_BLK,
        at::PM_TMR_LEN,
        at::X_PM_TMR_BLK,
        PM_TMR_LEN as u8,
        PM_TMR_ACCESS,
    ) {
        return Err(FadtError::BadField("PM_TMR_BLK / X_PM_TMR_BLK"));
    }
    if flags & FLAG_RESET_REG_SUP == 0 {
        if le32(table, at::RESET_REG) != 0
            || le64(table, at::RESET_REG + 4) != 0
            || table[at::RESET_VALUE] != 0
        {
            return Err(FadtError::BadField("flags: RESET_REG_SUP is clear"));
        }
    } else if table[at::RESET_REG] != GAS_SYSTEM_IO
        || table[at::RESET_REG + 1] != (RESET_REG_LEN * 8) as u8
        || table[at::RESET_REG + 2] != 0
        || table[at::RESET_REG + 3] != GAS_ACCESS_BYTE
        || le64(table, at::RESET_REG + 4) == 0
    {
        return Err(FadtError::BadField("RESET_REG"));
    }
    let iapc_boot_arch = le16(table, at::IAPC_BOOT_ARCH);
    let rtc_not_present = iapc_boot_arch & IAPC_CMOS_RTC_NOT_PRESENT != 0;
    if (table[at::CENTURY] == CMOS_CENTURY_REGISTER) == rtc_not_present
        || (table[at::CENTURY] != 0 && table[at::CENTURY] != CMOS_CENTURY_REGISTER)
    {
        return Err(FadtError::BadField("CENTURY / CMOS_RTC_NOT_PRESENT"));
    }
    Ok(())
}

/// A legacy PM block address, its length byte (at `length_at`) and the `X_` GAS must describe
/// the same SystemIO register (a non-zero address, `bit_width = length * 8`, bit offset 0) with
/// the access size of that register.
const fn pm_twin(
    table: &[u8],
    legacy: usize,
    length_at: usize,
    extended: usize,
    length: u8,
    access: u8,
) -> bool {
    let address = le32(table, legacy);
    address != 0
        && table[length_at] == length
        && table[extended] == GAS_SYSTEM_IO
        && table[extended + 1] == length * 8
        && table[extended + 2] == 0
        && table[extended + 3] == access
        && le64(table, extended + 4) == address as u64
}

// --- FACS ------------------------------------------------------------------------------------

/// FACS signature (ACPI Table 5.13).
pub const FACS_SIGNATURE: [u8; 4] = *b"FACS";
/// Length of the FACS (a multiple of 64 bytes; 64 for version 2).
pub const FACS_LEN: usize = 64;
/// FACS version (offset 32): 2 for ACPI 5.0 and later.
pub const FACS_VERSION: u8 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacsError {
    /// The output buffer or the slice is shorter than 64 bytes.
    TooShort,
    SignatureMismatch,
    /// The `Length` field is not 64, or differs from the number of bytes given.
    LengthMismatch {
        declared: u32,
        actual: usize,
    },
    BadVersion(u8),
    /// A reserved byte (offsets 33..36 and 40..64) is not zero; the value is the offset.
    ReservedNotZero(usize),
    /// `Flags` has a bit above bit 1 set.
    ReservedFlags(u32),
}

/// Builds the 64-byte FACS: signature, length, version 2; `HARDWARE_SIGNATURE`,
/// `FIRMWARE_WAKING_VECTOR`, `GLOBAL_LOCK`, `FLAGS` (no `S4BIOS_F`, no `64BIT_WAKE_SUPPORTED_F`),
/// `X_FIRMWARE_WAKING_VECTOR` and `OSPM FLAGS` are zero. The FACS has no checksum. A `const fn`.
pub const fn build_facs(out: &mut [u8]) -> Result<usize, FacsError> {
    if out.len() < FACS_LEN {
        return Err(FacsError::TooShort);
    }
    let table = head_mut(out, FACS_LEN);
    fill_range(table, 0, FACS_LEN, 0);
    copy_at(table, 0, &FACS_SIGNATURE);
    copy_at(table, 4, &(FACS_LEN as u32).to_le_bytes());
    table[32] = FACS_VERSION;
    tri!(validate_facs(table));
    Ok(FACS_LEN)
}

/// Checks signature, `Length` (64, equal to the slice), version 2, reserved bytes and reserved
/// flag bits. The rule that `OSPM Flags` (offset 36) are zero describes the table as the
/// firmware builds it: an OS that already uses the FACS may legitimately have written them, so
/// this function is not a check of a live table.
pub const fn validate_facs(table: &[u8]) -> Result<(), FacsError> {
    if table.len() < FACS_LEN {
        return Err(FacsError::TooShort);
    }
    if !bytes_eq(sub(table, 0, 4), &FACS_SIGNATURE) {
        return Err(FacsError::SignatureMismatch);
    }
    let declared = le32(table, 4);
    if declared as usize != FACS_LEN || table.len() != FACS_LEN {
        return Err(FacsError::LengthMismatch {
            declared,
            actual: table.len(),
        });
    }
    if table[32] != FACS_VERSION {
        return Err(FacsError::BadVersion(table[32]));
    }
    let mut i = 33;
    while i < FACS_LEN {
        // 36..40 is `OSPM Flags`, which the firmware initializes to zero as well.
        if table[i] != 0 {
            return Err(FacsError::ReservedNotZero(i));
        }
        i += 1;
    }
    let flags = le32(table, 20);
    if flags >> 2 != 0 {
        return Err(FacsError::ReservedFlags(flags));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
