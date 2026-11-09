//! RSDP revision 2 and the XSDT (S4-31g).
//!
//! The firmware of this board publishes the XSDT only: `RsdtAddress` is 0 and no RSDT exists
//! (ACPI 5.2.5.3 "Root System Description Pointer", Table 5.5, and 5.2.8 "Extended System
//! Description Table"; the layout below is the one of ACPICA's `acpi_table_rsdp`, which Linux
//! reads; the 6.6 text was not re-read in this slice).
//!
//! | offset | size | RSDP field |
//! |--------|------|------------|
//! | 0 | 8 | Signature `"RSD PTR "` |
//! | 8 | 1 | Checksum: the first 20 bytes add to zero |
//! | 9 | 6 | OEMID |
//! | 15 | 1 | Revision = 2 |
//! | 16 | 4 | RsdtAddress = 0 (no RSDT) |
//! | 20 | 4 | Length = 36 |
//! | 24 | 8 | XsdtAddress |
//! | 32 | 1 | Extended Checksum: all 36 bytes add to zero |
//! | 33 | 3 | Reserved, zero |
//!
//! The XSDT is a System Description Table (signature `XSDT`, revision 1) whose body is a list of
//! 64-bit physical addresses of tables. It lists the FADT, MADT and MCFG; the DSDT and the FACS
//! are not listed (the FADT points to them through `X_DSDT` and `X_FIRMWARE_CTRL`).
//!
//! Both builders are `const fn`; the XSDT body is checked by [`validate_xsdt`] before a length is
//! returned.

use super::bytes::{bytes_eq, copy_at, fill_range, head_mut, sub, tri};
use super::table::{
    OEM_ID, SDT_HEADER_LEN, TableError, byte_sum, finish_checksum, validate_table, write_header,
};

/// RSDP signature, 8 bytes including the trailing space.
pub const RSDP_SIGNATURE: [u8; 8] = *b"RSD PTR ";
/// Revision 2: the RSDP has the extended fields (XSDT address, length, extended checksum).
pub const RSDP_REVISION: u8 = 2;
/// Length of the revision-2 RSDP.
pub const RSDP_LEN: usize = 36;
/// Bytes the first checksum covers (the revision-0 part).
pub const RSDP_BASIC_LEN: usize = 20;
/// The RSDP must sit on a 16-byte boundary (ACPI 5.2.5.1).
pub const RSDP_ALIGN: u64 = 16;

pub const XSDT_SIGNATURE: [u8; 4] = *b"XSDT";
pub const XSDT_REVISION: u8 = 1;
pub const XSDT_OEM_TABLE_ID: [u8; 8] = *b"ODFWXSDT";
/// Size of one XSDT entry.
pub const XSDT_ENTRY_LEN: usize = 8;
/// Entries the builder accepts (FADT, MADT, MCFG and room for a few more).
pub const XSDT_MAX_ENTRIES: usize = 8;

/// Field offsets of the RSDP.
mod at {
    pub const CHECKSUM: usize = 8;
    pub const OEM_ID: usize = 9;
    pub const REVISION: usize = 15;
    pub const RSDT_ADDRESS: usize = 16;
    pub const LENGTH: usize = 20;
    pub const XSDT_ADDRESS: usize = 24;
    pub const EXT_CHECKSUM: usize = 32;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RsdpError {
    Table(TableError),
    /// The output buffer or the slice is shorter than the structure.
    TooShort,
    SignatureMismatch,
    /// The first 20 bytes do not add to zero; the value is their sum.
    BadChecksum(u8),
    /// The 36 bytes do not add to zero; the value is their sum.
    BadExtendedChecksum(u8),
    /// The revision byte is not 2.
    BadRevision(u8),
    /// `RsdtAddress` is not 0: this board publishes no RSDT.
    RsdtAddressNotZero(u32),
    /// `Length` is not 36.
    BadLength(u32),
    /// `XsdtAddress` is 0 or not 8-byte aligned (an XSDT is an SDT with 64-bit entries).
    BadXsdtAddress(u64),
    /// A reserved byte (offsets 33..36) is not zero; the value is the offset.
    ReservedNotZero(usize),
    /// The XSDT has no entry, a body that is not a whole number of 8-byte entries, more than
    /// [`XSDT_MAX_ENTRIES`], or an entry that is 0.
    BadXsdtEntries,
    /// The XSDT revision byte is not 1.
    BadXsdtRevision(u8),
}

impl From<TableError> for RsdpError {
    fn from(error: TableError) -> Self {
        RsdpError::Table(error)
    }
}

const fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

const fn le64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes([
        bytes[at],
        bytes[at + 1],
        bytes[at + 2],
        bytes[at + 3],
        bytes[at + 4],
        bytes[at + 5],
        bytes[at + 6],
        bytes[at + 7],
    ])
}

/// Builds the 36-byte RSDP into `out[..36]` (nothing past it is touched) and returns 36.
/// `RsdtAddress` is 0; `xsdt_address` is the physical address of the XSDT.
pub const fn build_rsdp(xsdt_address: u64, out: &mut [u8]) -> Result<usize, RsdpError> {
    if out.len() < RSDP_LEN {
        return Err(RsdpError::TooShort);
    }
    let rsdp = head_mut(out, RSDP_LEN);
    fill_range(rsdp, 0, RSDP_LEN, 0);
    copy_at(rsdp, 0, &RSDP_SIGNATURE);
    copy_at(rsdp, at::OEM_ID, &OEM_ID);
    rsdp[at::REVISION] = RSDP_REVISION;
    // RsdtAddress (16..20) stays 0.
    copy_at(rsdp, at::LENGTH, &(RSDP_LEN as u32).to_le_bytes());
    copy_at(rsdp, at::XSDT_ADDRESS, &xsdt_address.to_le_bytes());
    // The first checksum covers bytes 0..20; the extended one covers all 36 and therefore the
    // first checksum byte too, so it is computed second.
    rsdp[at::CHECKSUM] = byte_sum(sub(rsdp, 0, RSDP_BASIC_LEN)).wrapping_neg();
    rsdp[at::EXT_CHECKSUM] = byte_sum(rsdp).wrapping_neg();
    tri!(validate_rsdp(rsdp));
    Ok(RSDP_LEN)
}

/// Checks a 36-byte RSDP: signature, both checksums, revision 2, `RsdtAddress` 0, `Length` 36, a
/// non-zero 8-aligned `XsdtAddress` and zero reserved bytes. `rsdp` must be at least 36 bytes;
/// only the first 36 are looked at.
pub const fn validate_rsdp(rsdp: &[u8]) -> Result<(), RsdpError> {
    if rsdp.len() < RSDP_LEN {
        return Err(RsdpError::TooShort);
    }
    if !bytes_eq(sub(rsdp, 0, 8), &RSDP_SIGNATURE) {
        return Err(RsdpError::SignatureMismatch);
    }
    let basic = byte_sum(sub(rsdp, 0, RSDP_BASIC_LEN));
    if basic != 0 {
        return Err(RsdpError::BadChecksum(basic));
    }
    let extended = byte_sum(sub(rsdp, 0, RSDP_LEN));
    if extended != 0 {
        return Err(RsdpError::BadExtendedChecksum(extended));
    }
    if rsdp[at::REVISION] != RSDP_REVISION {
        return Err(RsdpError::BadRevision(rsdp[at::REVISION]));
    }
    let rsdt = le32(rsdp, at::RSDT_ADDRESS);
    if rsdt != 0 {
        return Err(RsdpError::RsdtAddressNotZero(rsdt));
    }
    let length = le32(rsdp, at::LENGTH);
    if length as usize != RSDP_LEN {
        return Err(RsdpError::BadLength(length));
    }
    let xsdt = le64(rsdp, at::XSDT_ADDRESS);
    if xsdt == 0 || xsdt % 8 != 0 {
        return Err(RsdpError::BadXsdtAddress(xsdt));
    }
    let mut i = RSDP_LEN - 3;
    while i < RSDP_LEN {
        if rsdp[i] != 0 {
            return Err(RsdpError::ReservedNotZero(i));
        }
        i += 1;
    }
    Ok(())
}

/// Length of an XSDT with `entries` entries.
pub const fn xsdt_len(entries: usize) -> usize {
    SDT_HEADER_LEN + entries * XSDT_ENTRY_LEN
}

/// Builds the XSDT with the given table addresses (in this order) into `out` and returns its
/// length. Between 1 and [`XSDT_MAX_ENTRIES`] non-zero addresses.
pub const fn build_xsdt(entries: &[u64], out: &mut [u8]) -> Result<usize, RsdpError> {
    if entries.is_empty() || entries.len() > XSDT_MAX_ENTRIES {
        return Err(RsdpError::BadXsdtEntries);
    }
    let len = xsdt_len(entries.len());
    if out.len() < len {
        return Err(RsdpError::Table(TableError::TooShort));
    }
    let table = head_mut(out, len);
    tri!(
        write_header(
            table,
            XSDT_SIGNATURE,
            XSDT_REVISION,
            XSDT_OEM_TABLE_ID,
            len as u32,
        ),
        RsdpError::Table
    );
    let mut i = 0;
    while i < entries.len() {
        copy_at(
            table,
            SDT_HEADER_LEN + i * XSDT_ENTRY_LEN,
            &entries[i].to_le_bytes(),
        );
        i += 1;
    }
    tri!(finish_checksum(table), RsdpError::Table);
    tri!(validate_xsdt(table));
    Ok(len)
}

/// Checks signature, `Length`, checksum, revision 1 and the body: a whole number of 8-byte
/// entries (1 to [`XSDT_MAX_ENTRIES`]), none of them 0.
pub const fn validate_xsdt(table: &[u8]) -> Result<(), RsdpError> {
    tri!(validate_table(table, &XSDT_SIGNATURE), RsdpError::Table);
    if table[8] != XSDT_REVISION {
        return Err(RsdpError::BadXsdtRevision(table[8]));
    }
    let body = table.len() - SDT_HEADER_LEN;
    if body == 0 || body % XSDT_ENTRY_LEN != 0 || body / XSDT_ENTRY_LEN > XSDT_MAX_ENTRIES {
        return Err(RsdpError::BadXsdtEntries);
    }
    let mut i = 0;
    while i < body / XSDT_ENTRY_LEN {
        if le64(table, SDT_HEADER_LEN + i * XSDT_ENTRY_LEN) == 0 {
            return Err(RsdpError::BadXsdtEntries);
        }
        i += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
