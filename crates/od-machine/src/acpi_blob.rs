//! Machine-side validator of the ACPI table blob (S4-31d, completed by S4-31g).
//!
//! The blob is built by `firmware/uefi` (`src/acpi/blob.rs`, evaluated by the compiler into the
//! `.acpi_blob` section of `od-acpi-blob`) and extracted by `tools/build/build_stage4_acpi_blob.py`.
//! This module does not build or fix anything: it decides whether a given byte string is a blob
//! that belongs to this board's resolved manifest and may be placed in the ACPI region. The
//! loader and the placement in RAM are S4-37; E820 is S4-37 too.
//!
//! Layout and fields are documented next to the builder (`blob.rs` there); offsets below are
//! deliberately written out again so that a drift of the builder fails the fixture test.

use std::fmt;

use crate::firmware_board::{
    FIRMWARE_ACPI_FACS_BASE, FIRMWARE_ACPI_FACS_BYTES, FIRMWARE_ACPI_TABLES_BASE,
    FIRMWARE_ACPI_TABLES_BYTES,
};

pub const ACPI_BLOB_MAGIC: [u8; 8] = *b"ODACPIB1";
pub const ACPI_BLOB_VERSION: u32 = 2;
pub const ACPI_BLOB_HEADER_BYTES: usize = 0x60;
pub const ACPI_BLOB_DIR_ENTRY_BYTES: usize = 16;
pub const ACPI_BLOB_MAX_TABLES: usize = 8;
/// Payload bytes at offset 0 reserved for the RSDP; no table may start there. The RSDP itself is
/// 36 bytes, the rest of the slot is zero.
pub const ACPI_BLOB_RSDP_SLOT_BYTES: u32 = 0x40;
/// Length of the revision-2 RSDP.
pub const ACPI_RSDP_BYTES: usize = 36;
/// Length of the FACS segment (ACPI Table 5.13, version 2).
pub const ACPI_FACS_BYTES: u32 = 64;
/// Offset and value of the FACS `Version` byte (2 for ACPI 5.0 and later; the firmware writes 2).
const FACS_VERSION_OFFSET: usize = 32;
const ACPI_FACS_VERSION: u8 = 2;
/// Tables and the payload start are 16-aligned.
pub const ACPI_BLOB_ALIGN: u32 = 16;
/// Length of a System Description Table header (ACPI Table 5.4).
const SDT_HEADER_BYTES: u32 = 36;

/// The ACPI region of the board: `[0xE_0000, 0xF_0000)` (decision 6 of S4-31), including FACS.
pub const ACPI_REGION_BASE: u64 = FIRMWARE_ACPI_TABLES_BASE;
pub const ACPI_REGION_END: u64 = FIRMWARE_ACPI_FACS_BASE + FIRMWARE_ACPI_FACS_BYTES;
/// End of the part for tables (E820 ACPI reclaim); the NVS part for FACS starts here.
pub const ACPI_RECLAIM_END: u64 = FIRMWARE_ACPI_TABLES_BASE + FIRMWARE_ACPI_TABLES_BYTES;

/// Domain tag of the manifest hash; the trailing digit is the hash scheme version.
pub const ACPI_MANIFEST_HASH_TAG: [u8; 16] = *b"ODACPI-MANIFEST1";
/// Highest device type code a manifest hash accepts (FIRMWARE_BOOT.md 6.3 reserves 1..=15).
const MAX_DEVICE_TYPE: u32 = 15;
const DEVICE_ENTRY_BYTES: usize = 48;

/// Signatures the blob may carry in its directory. The FACS is not an SDT and has its own
/// segment; the RSDP is in the payload slot.
const ALLOWED_SIGNATURES: [[u8; 4]; 5] = [*b"DSDT", *b"FACP", *b"APIC", *b"MCFG", *b"XSDT"];
/// Tables the XSDT must list (every table of the directory except DSDT and XSDT).
const XSDT_LISTED: [[u8; 4]; 3] = [*b"FACP", *b"APIC", *b"MCFG"];
/// Tables every blob has: the board always builds DSDT, MADT, FADT and XSDT (MCFG needs ECAM).
const REQUIRED_SIGNATURES: [[u8; 4]; 4] = [*b"DSDT", *b"FACP", *b"APIC", *b"XSDT"];
/// FADT field offsets the validator reads (ACPI Table 5.9).
const FADT_FIRMWARE_CTRL: usize = 36;
const FADT_DSDT: usize = 40;
const FADT_X_FIRMWARE_CTRL: usize = 132;
const FADT_X_DSDT: usize = 140;
/// Shortest FADT that has `X_DSDT` (offset 140, 8 bytes).
const FADT_MIN_LEN: usize = FADT_X_DSDT + 8;

pub type AcpiManifestHash = [u8; 32];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AcpiBlobError {
    /// A device type outside `1..=15` in a table given to [`acpi_manifest_hash`].
    BadDeviceType(u32),
    DuplicateDeviceType(u32),
    /// Fewer bytes than the header or the directory needs.
    Truncated {
        len: usize,
    },
    BadMagic,
    UnsupportedVersion(u32),
    BadHeaderLength(u32),
    /// A reserved directory word is not zero.
    ReservedNotZero,
    /// `total_len` of the header differs from the number of bytes given.
    LengthMismatch {
        declared: u32,
        actual: usize,
    },
    /// The blob was built for another manifest: the board changed (or the blob is foreign) and
    /// the blob has to be rebuilt.
    ManifestMismatch {
        expected: AcpiManifestHash,
        found: AcpiManifestHash,
    },
    /// The load base is not `0xE_0000` (the start of the ACPI region): the RSDP is the first
    /// thing in the payload and the RSDP search area of an OS starts there.
    UnexpectedLoadBase(u64),
    /// `rsdp_address` is not `load_base` (the RSDP lives at payload offset 0).
    RsdpAddressMismatch {
        found: u64,
    },
    /// `facs_address` is not `0xE_F000` (the NVS part of the region).
    FacsAddressMismatch {
        found: u64,
    },
    /// `facs_len` is not 64, or the FACS bytes differ from what the firmware writes (signature,
    /// `Length`, version 2, all other bytes zero).
    BadFacs,
    /// A byte between the end of the directory and `payload_offset` is not zero.
    NonZeroHeaderGap {
        blob_offset: usize,
    },
    BadTableCount(u32),
    /// `payload_offset`/`payload_len` do not describe the tail of the blob after the directory.
    BadPayload {
        offset: u32,
        len: u32,
    },
    /// The payload does not fit in `[0xE_0000, 0xF_0000)` (or overflows).
    OutOfRegion {
        start: u64,
        end: Option<u64>,
    },
    /// The payload reaches into the NVS part (`>= 0xE_F000`), which is for FACS only.
    PayloadIntoNvs {
        end: u64,
    },
    /// A table signature this slice does not accept.
    UnexpectedTable {
        signature: [u8; 4],
    },
    DuplicateTable {
        signature: [u8; 4],
    },
    /// A table (`index` in the directory) starts at an unaligned payload offset.
    MisalignedTable {
        index: usize,
        offset: u32,
    },
    /// A table starts inside the RSDP slot.
    TableInRsdpSlot {
        index: usize,
        offset: u32,
    },
    /// A table is shorter than an SDT header.
    TableTooShort {
        index: usize,
        len: u32,
    },
    TableOutsidePayload {
        index: usize,
    },
    TableOverlap {
        first: usize,
        second: usize,
    },
    /// The signature in the table differs from the directory entry.
    SignatureMismatch {
        index: usize,
    },
    /// The `Length` field in the table header differs from the directory length.
    TableLengthMismatch {
        index: usize,
        directory: u32,
        header: u32,
    },
    BadChecksum {
        index: usize,
        sum: u8,
    },
    /// A payload byte that no table covers (and the rest of the RSDP slot) is not zero.
    NonZeroGap {
        payload_offset: usize,
    },
    /// A table every blob needs (DSDT, FADT, MADT, XSDT) is not in the directory.
    MissingTable {
        signature: [u8; 4],
    },
    /// The RSDP signature is not `"RSD PTR "`.
    BadRsdpSignature,
    /// The first 20 bytes of the RSDP do not add to zero.
    BadRsdpChecksum {
        sum: u8,
    },
    /// The 36 bytes of the RSDP do not add to zero.
    BadRsdpExtendedChecksum {
        sum: u8,
    },
    /// Another RSDP rule: revision 2, `RsdtAddress` 0, `Length` 36, zero reserved bytes.
    BadRsdp(&'static str),
    /// `XsdtAddress` of the RSDP is not the address of the XSDT of this blob.
    RsdpXsdtAddress {
        found: u64,
        expected: u64,
    },
    /// The XSDT revision is not 1, or its body is not a whole number of 8-byte entries.
    BadXsdt(&'static str),
    /// An XSDT entry is not the address of a table of this blob.
    XsdtEntryOutsideBlob {
        index: usize,
        address: u64,
    },
    /// An XSDT entry points at a table that must not be listed (DSDT, XSDT) or at one already
    /// listed.
    XsdtEntryWrongTable {
        index: usize,
        signature: [u8; 4],
    },
    /// The XSDT does not list a table of the blob that it must.
    XsdtMissingTable {
        signature: [u8; 4],
    },
    /// The FADT is shorter than `X_DSDT` needs.
    FadtTooShort {
        len: u32,
    },
    /// A FADT table address (32-bit or 64-bit form) is not where the blob puts the table.
    FadtAddressMismatch {
        field: &'static str,
        found: u64,
        expected: u64,
    },
}

impl fmt::Display for AcpiBlobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ACPI blob rejected: {self:?}")
    }
}

impl std::error::Error for AcpiBlobError {}

/// One table of a validated blob.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AcpiBlobTable {
    pub signature: [u8; 4],
    /// Offset in the payload; the table lives at `load_base + payload_offset`.
    pub payload_offset: u32,
    pub length: u32,
}

/// What a blob that passed [`validate_acpi_blob`] says about itself.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedAcpiBlob {
    pub version: u32,
    /// SHA-256 of the whole blob this description was validated from (header included).
    pub blob_sha256: [u8; 32],
    pub manifest_hash: AcpiManifestHash,
    /// GPA of payload offset 0.
    pub load_base: u64,
    /// Offset of the payload in the blob (the bytes the loader copies).
    pub payload_offset: usize,
    pub payload_len: usize,
    pub tables: Vec<AcpiBlobTable>,
    /// GPA of the RSDP (payload offset 0).
    pub rsdp_address: u64,
    /// Offset of the FACS bytes in the blob, and their GPA (0xE_F000).
    pub facs_offset: usize,
    pub facs_address: u64,
}

impl ValidatedAcpiBlob {
    /// GPA range `[start, end)` the payload occupies.
    pub fn load_range(&self) -> (u64, u64) {
        (self.load_base, self.load_base + self.payload_len as u64)
    }

    /// GPA range `[start, end)` of the FACS segment (the NVS part).
    pub fn facs_range(&self) -> (u64, u64) {
        (
            self.facs_address,
            self.facs_address + u64::from(ACPI_FACS_BYTES),
        )
    }

    /// The 64 FACS bytes inside `blob`.
    pub fn facs_bytes<'a>(&self, blob: &'a [u8]) -> &'a [u8] {
        &blob[self.facs_offset..self.facs_offset + ACPI_FACS_BYTES as usize]
    }

    /// Address of the table with `signature`, if the directory has it.
    pub fn table_address(&self, signature: &[u8; 4]) -> Option<u64> {
        self.tables
            .iter()
            .find(|table| &table.signature == signature)
            .map(|table| self.load_base + u64::from(table.payload_offset))
    }

    /// The payload bytes inside `blob` (the same slice that was validated).
    pub fn payload<'a>(&self, blob: &'a [u8]) -> &'a [u8] {
        &blob[self.payload_offset..self.payload_offset + self.payload_len]
    }

    /// The bytes of one table inside `blob`.
    pub fn table_bytes<'a>(&self, blob: &'a [u8], table: &AcpiBlobTable) -> &'a [u8] {
        let start = self.payload_offset + table.payload_offset as usize;
        &blob[start..start + table.length as usize]
    }
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

/// Manifest hash of a resolved device table: SHA-256 over [`ACPI_MANIFEST_HASH_TAG`] and the
/// 48-byte handoff encodings of the entries ordered by type code (so the order of the table does
/// not matter). Types must be in `1..=15` and unique. The firmware computes the same hash with
/// its own `const fn` when it builds the blob.
pub fn acpi_manifest_hash(
    entries: &[[u8; DEVICE_ENTRY_BYTES]],
) -> Result<AcpiManifestHash, AcpiBlobError> {
    let mut sorted: Vec<&[u8; DEVICE_ENTRY_BYTES]> = entries.iter().collect();
    sorted.sort_by_key(|entry| le32(&entry[..], 0));
    let mut previous = 0u32;
    for entry in &sorted {
        let device_type = le32(&entry[..], 0);
        if device_type == 0 || device_type > MAX_DEVICE_TYPE {
            return Err(AcpiBlobError::BadDeviceType(device_type));
        }
        if device_type == previous {
            return Err(AcpiBlobError::DuplicateDeviceType(device_type));
        }
        previous = device_type;
    }
    let mut message = Vec::with_capacity(16 + sorted.len() * DEVICE_ENTRY_BYTES);
    message.extend_from_slice(&ACPI_MANIFEST_HASH_TAG);
    for entry in sorted {
        message.extend_from_slice(&entry[..]);
    }
    Ok(sha256(&message))
}

/// Checks a blob against the manifest hash of the machine's resolved board.
///
/// Rejected: wrong magic/version/lengths, a foreign manifest hash, a load base other than
/// `0xE_0000`, an `rsdp_address` other than the load base, a FACS address other than `0xE_F000`,
/// a payload outside `[0xE_0000, 0xF_0000)` or reaching into the NVS part, unknown or repeated
/// signatures, missing DSDT/FADT/MADT/XSDT, tables that overlap, are unaligned, run outside the
/// payload or start in the RSDP slot, a table whose `Length` field differs from its directory
/// entry, a table whose bytes do not add to zero, non-zero bytes between the directory and the
/// payload or between tables, a bad RSDP (signature, either checksum, revision, `RsdtAddress`,
/// `Length`, an `XsdtAddress` that is not the XSDT of the blob), an XSDT whose entries are not
/// exactly the FADT, MADT and MCFG of the blob, a FADT whose `DSDT`/`X_DSDT` or
/// `FIRMWARE_CTRL`/`X_FIRMWARE_CTRL` is not the DSDT of the blob or the FACS at `0xE_F000`, and a
/// FACS with a wrong signature or length. On success nothing is changed and nothing is placed
/// anywhere; the returned description names the load ranges and the tables.
pub fn validate_acpi_blob(
    blob: &[u8],
    expected_manifest_hash: &AcpiManifestHash,
) -> Result<ValidatedAcpiBlob, AcpiBlobError> {
    if blob.len() < ACPI_BLOB_HEADER_BYTES {
        return Err(AcpiBlobError::Truncated { len: blob.len() });
    }
    if blob[..8] != ACPI_BLOB_MAGIC {
        return Err(AcpiBlobError::BadMagic);
    }
    let version = le32(blob, 0x08);
    if version != ACPI_BLOB_VERSION {
        return Err(AcpiBlobError::UnsupportedVersion(version));
    }
    let header_len = le32(blob, 0x0C);
    if header_len as usize != ACPI_BLOB_HEADER_BYTES {
        return Err(AcpiBlobError::BadHeaderLength(header_len));
    }
    let total_len = le32(blob, 0x4C);
    if total_len as usize != blob.len() {
        return Err(AcpiBlobError::LengthMismatch {
            declared: total_len,
            actual: blob.len(),
        });
    }
    let mut found = [0u8; 32];
    found.copy_from_slice(&blob[0x10..0x30]);
    if found != *expected_manifest_hash {
        return Err(AcpiBlobError::ManifestMismatch {
            expected: *expected_manifest_hash,
            found,
        });
    }

    let load_base = le64(blob, 0x30);
    if load_base != ACPI_REGION_BASE {
        return Err(AcpiBlobError::UnexpectedLoadBase(load_base));
    }
    let rsdp_address = le64(blob, 0x40);
    if rsdp_address != load_base {
        return Err(AcpiBlobError::RsdpAddressMismatch {
            found: rsdp_address,
        });
    }
    let facs_offset = le32(blob, 0x50);
    let facs_len = le32(blob, 0x54);
    let facs_address = le64(blob, 0x58);
    if facs_address != FIRMWARE_ACPI_FACS_BASE {
        return Err(AcpiBlobError::FacsAddressMismatch {
            found: facs_address,
        });
    }
    if facs_len != ACPI_FACS_BYTES {
        return Err(AcpiBlobError::BadFacs);
    }

    let payload_offset = le32(blob, 0x38);
    let payload_len = le32(blob, 0x3C);
    let table_count = le32(blob, 0x48);
    if table_count == 0 || table_count as usize > ACPI_BLOB_MAX_TABLES {
        return Err(AcpiBlobError::BadTableCount(table_count));
    }
    let directory_end = ACPI_BLOB_HEADER_BYTES + table_count as usize * ACPI_BLOB_DIR_ENTRY_BYTES;
    if directory_end > blob.len() {
        return Err(AcpiBlobError::Truncated { len: blob.len() });
    }
    // The payload follows the directory, is 16-aligned in offset and length, holds at least the
    // RSDP slot, and the FACS segment follows it and ends the blob.
    let payload_end = u64::from(payload_offset) + u64::from(payload_len);
    if (payload_offset as usize) < directory_end
        || !payload_offset.is_multiple_of(ACPI_BLOB_ALIGN)
        || payload_len < ACPI_BLOB_RSDP_SLOT_BYTES
        || !payload_len.is_multiple_of(ACPI_BLOB_ALIGN)
        || payload_end != u64::from(facs_offset)
        || payload_end + u64::from(facs_len) != blob.len() as u64
    {
        return Err(AcpiBlobError::BadPayload {
            offset: payload_offset,
            len: payload_len,
        });
    }
    if let Some(at) = blob[directory_end..payload_offset as usize]
        .iter()
        .position(|&byte| byte != 0)
    {
        return Err(AcpiBlobError::NonZeroHeaderGap {
            blob_offset: directory_end + at,
        });
    }

    // `load_base` is fixed, so the payload range is `[0xE_0000, 0xE_0000 + payload_len)`.
    let load_end = load_base + u64::from(payload_len);
    if load_end > ACPI_REGION_END {
        return Err(AcpiBlobError::OutOfRegion {
            start: load_base,
            end: Some(load_end),
        });
    }
    if load_end > ACPI_RECLAIM_END {
        return Err(AcpiBlobError::PayloadIntoNvs { end: load_end });
    }

    let payload = &blob[payload_offset as usize..payload_end as usize];
    let mut tables = Vec::with_capacity(table_count as usize);
    for index in 0..table_count as usize {
        let at = ACPI_BLOB_HEADER_BYTES + index * ACPI_BLOB_DIR_ENTRY_BYTES;
        let signature = [blob[at], blob[at + 1], blob[at + 2], blob[at + 3]];
        let offset = le32(blob, at + 4);
        let length = le32(blob, at + 8);
        if le32(blob, at + 12) != 0 {
            return Err(AcpiBlobError::ReservedNotZero);
        }
        if !ALLOWED_SIGNATURES.contains(&signature) {
            return Err(AcpiBlobError::UnexpectedTable { signature });
        }
        if tables
            .iter()
            .any(|table: &AcpiBlobTable| table.signature == signature)
        {
            return Err(AcpiBlobError::DuplicateTable { signature });
        }
        if !offset.is_multiple_of(ACPI_BLOB_ALIGN) {
            return Err(AcpiBlobError::MisalignedTable { index, offset });
        }
        if offset < ACPI_BLOB_RSDP_SLOT_BYTES {
            return Err(AcpiBlobError::TableInRsdpSlot { index, offset });
        }
        if length < SDT_HEADER_BYTES {
            return Err(AcpiBlobError::TableTooShort { index, len: length });
        }
        if u64::from(offset) + u64::from(length) > u64::from(payload_len) {
            return Err(AcpiBlobError::TableOutsidePayload { index });
        }
        tables.push(AcpiBlobTable {
            signature,
            payload_offset: offset,
            length,
        });
    }

    // Overlap: after sorting by offset every table must end before the next one starts.
    let mut order: Vec<usize> = (0..tables.len()).collect();
    order.sort_by_key(|&index| tables[index].payload_offset);
    for pair in order.windows(2) {
        let (first, second) = (&tables[pair[0]], &tables[pair[1]]);
        if first.payload_offset + first.length > second.payload_offset {
            return Err(AcpiBlobError::TableOverlap {
                first: pair[0],
                second: pair[1],
            });
        }
    }

    for signature in REQUIRED_SIGNATURES {
        if !tables.iter().any(|table| table.signature == signature) {
            return Err(AcpiBlobError::MissingTable { signature });
        }
    }

    // The RSDP (first 36 bytes of the slot) is "covered"; the rest of the slot and every byte
    // no table owns must be zero.
    let mut covered = vec![false; payload_len as usize];
    covered[..ACPI_RSDP_BYTES].fill(true);
    for (index, table) in tables.iter().enumerate() {
        let start = table.payload_offset as usize;
        let bytes = &payload[start..start + table.length as usize];
        if bytes[..4] != table.signature {
            return Err(AcpiBlobError::SignatureMismatch { index });
        }
        let header = le32(bytes, 4);
        if header != table.length {
            return Err(AcpiBlobError::TableLengthMismatch {
                index,
                directory: table.length,
                header,
            });
        }
        let sum = bytes.iter().fold(0u8, |sum, &byte| sum.wrapping_add(byte));
        if sum != 0 {
            return Err(AcpiBlobError::BadChecksum { index, sum });
        }
        covered[start..start + bytes.len()].fill(true);
    }
    if let Some(at) = payload
        .iter()
        .enumerate()
        .position(|(at, &byte)| byte != 0 && !covered[at])
    {
        return Err(AcpiBlobError::NonZeroGap { payload_offset: at });
    }

    let info = ValidatedAcpiBlob {
        version,
        blob_sha256: sha256(blob),
        manifest_hash: found,
        load_base,
        payload_offset: payload_offset as usize,
        payload_len: payload_len as usize,
        tables,
        rsdp_address,
        facs_offset: facs_offset as usize,
        facs_address,
    };
    check_rsdp_and_references(blob, &info)?;
    Ok(info)
}

/// RSDP, XSDT, FADT and FACS of a blob whose directory, tables and gaps are already checked.
fn check_rsdp_and_references(blob: &[u8], info: &ValidatedAcpiBlob) -> Result<(), AcpiBlobError> {
    let payload = info.payload(blob);
    let address_of =
        |signature: &[u8; 4]| info.table_address(signature).expect("table is required");
    let bytes_of = |signature: &[u8; 4]| {
        let table = info
            .tables
            .iter()
            .find(|table| &table.signature == signature)
            .expect("table is required");
        info.table_bytes(blob, table)
    };

    // RSDP revision 2: signature, both sums, revision, no RSDT, length 36, XSDT address.
    let rsdp = &payload[..ACPI_RSDP_BYTES];
    if rsdp[..8] != *b"RSD PTR " {
        return Err(AcpiBlobError::BadRsdpSignature);
    }
    let basic = rsdp[..20].iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    if basic != 0 {
        return Err(AcpiBlobError::BadRsdpChecksum { sum: basic });
    }
    let extended = rsdp.iter().fold(0u8, |sum, &b| sum.wrapping_add(b));
    if extended != 0 {
        return Err(AcpiBlobError::BadRsdpExtendedChecksum { sum: extended });
    }
    if rsdp[15] != 2 {
        return Err(AcpiBlobError::BadRsdp("revision is not 2"));
    }
    if le32(rsdp, 16) != 0 {
        return Err(AcpiBlobError::BadRsdp("RsdtAddress is not 0"));
    }
    if le32(rsdp, 20) as usize != ACPI_RSDP_BYTES {
        return Err(AcpiBlobError::BadRsdp("Length is not 36"));
    }
    if rsdp[33..36].iter().any(|&byte| byte != 0) {
        return Err(AcpiBlobError::BadRsdp("reserved bytes are not zero"));
    }
    let xsdt_address = address_of(b"XSDT");
    if le64(rsdp, 24) != xsdt_address {
        return Err(AcpiBlobError::RsdpXsdtAddress {
            found: le64(rsdp, 24),
            expected: xsdt_address,
        });
    }

    // XSDT: revision 1, whole 8-byte entries, each the address of a listable table of this blob,
    // none twice, and every listable table present.
    let xsdt = bytes_of(b"XSDT");
    if xsdt[8] != 1 {
        return Err(AcpiBlobError::BadXsdt("revision is not 1"));
    }
    let body = xsdt.len() - SDT_HEADER_BYTES as usize;
    if body == 0 || !body.is_multiple_of(8) {
        return Err(AcpiBlobError::BadXsdt(
            "body is not a whole number of entries",
        ));
    }
    let mut listed: Vec<[u8; 4]> = Vec::new();
    for index in 0..body / 8 {
        let address = le64(xsdt, SDT_HEADER_BYTES as usize + index * 8);
        let target = info
            .tables
            .iter()
            .find(|table| info.load_base + u64::from(table.payload_offset) == address);
        let Some(target) = target else {
            return Err(AcpiBlobError::XsdtEntryOutsideBlob { index, address });
        };
        if !XSDT_LISTED.contains(&target.signature) || listed.contains(&target.signature) {
            return Err(AcpiBlobError::XsdtEntryWrongTable {
                index,
                signature: target.signature,
            });
        }
        listed.push(target.signature);
    }
    for signature in info.tables.iter().map(|table| table.signature) {
        if XSDT_LISTED.contains(&signature) && !listed.contains(&signature) {
            return Err(AcpiBlobError::XsdtMissingTable { signature });
        }
    }

    // FADT: both forms of the DSDT and FACS addresses agree with the blob.
    let fadt = bytes_of(b"FACP");
    if fadt.len() < FADT_MIN_LEN {
        return Err(AcpiBlobError::FadtTooShort {
            len: fadt.len() as u32,
        });
    }
    let dsdt_address = address_of(b"DSDT");
    for (field, found, expected) in [
        ("DSDT", u64::from(le32(fadt, FADT_DSDT)), dsdt_address),
        ("X_DSDT", le64(fadt, FADT_X_DSDT), dsdt_address),
        (
            "FIRMWARE_CTRL",
            u64::from(le32(fadt, FADT_FIRMWARE_CTRL)),
            info.facs_address,
        ),
        (
            "X_FIRMWARE_CTRL",
            le64(fadt, FADT_X_FIRMWARE_CTRL),
            info.facs_address,
        ),
    ] {
        if found != expected {
            return Err(AcpiBlobError::FadtAddressMismatch {
                field,
                found,
                expected,
            });
        }
    }

    // FACS (no checksum): exactly the bytes the firmware serializer writes - signature, Length,
    // version 2 at offset 32, every other byte (hardware signature, vectors, Global Lock, flags,
    // reserved, OSPM flags) zero.
    let facs = info.facs_bytes(blob);
    if facs[..4] != *b"FACS"
        || le32(facs, 4) != ACPI_FACS_BYTES
        || facs[FACS_VERSION_OFFSET] != ACPI_FACS_VERSION
        || facs[8..FACS_VERSION_OFFSET].iter().any(|&b| b != 0)
        || facs[FACS_VERSION_OFFSET + 1..].iter().any(|&b| b != 0)
    {
        return Err(AcpiBlobError::BadFacs);
    }
    Ok(())
}

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// SHA-256 (FIPS 180-4). The firmware has its own `const fn` copy; the same vectors pin both.
pub(crate) fn sha256(data: &[u8]) -> [u8; 32] {
    let mut state: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        let mut w = [0u32; 64];
        for (t, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[t] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for t in 16..64 {
            let s0 = w[t - 15].rotate_right(7) ^ w[t - 15].rotate_right(18) ^ (w[t - 15] >> 3);
            let s1 = w[t - 2].rotate_right(17) ^ w[t - 2].rotate_right(19) ^ (w[t - 2] >> 10);
            w[t] = w[t - 16]
                .wrapping_add(s0)
                .wrapping_add(w[t - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut h] = state;
        for t in 0..64 {
            let big_s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let temp1 = h
                .wrapping_add(big_s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[t])
                .wrapping_add(w[t]);
            let big_s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = big_s0.wrapping_add(maj);
            h = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        for (slot, value) in state.iter_mut().zip([a, b, c, d, e, f, g, h]) {
            *slot = slot.wrapping_add(value);
        }
    }
    let mut out = [0u8; 32];
    for (chunk, word) in out.as_chunks_mut::<4>().0.iter_mut().zip(state) {
        chunk.copy_from_slice(&word.to_be_bytes());
    }
    out
}

#[cfg(test)]
mod tests;
