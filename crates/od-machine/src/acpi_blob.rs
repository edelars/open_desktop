//! Machine-side validator of the ACPI table blob (S4-31d).
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
pub const ACPI_BLOB_VERSION: u32 = 1;
pub const ACPI_BLOB_HEADER_BYTES: usize = 0x60;
pub const ACPI_BLOB_DIR_ENTRY_BYTES: usize = 16;
pub const ACPI_BLOB_MAX_TABLES: usize = 8;
/// Payload bytes at offset 0 reserved for the RSDP (S4-31g); no table may start there.
pub const ACPI_BLOB_RSDP_SLOT_BYTES: u32 = 0x40;
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

/// Signatures the blob may carry in this slice. FADT/FACS/MADT join with S4-31e/f.
const ALLOWED_SIGNATURES: [[u8; 4]; 2] = [*b"DSDT", *b"MCFG"];

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
    /// A reserved header field or directory word is not zero.
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
    /// The RSDP address is not zero; the RSDP is built by S4-31g and checked there.
    RsdpNotSupported(u64),
    BadTableCount(u32),
    /// `payload_offset`/`payload_len` do not describe the tail of the blob after the directory.
    BadPayload {
        offset: u32,
        len: u32,
    },
    MisalignedLoadBase(u64),
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
    /// A payload byte that no table covers (and the RSDP slot) is not zero.
    NonZeroGap {
        payload_offset: usize,
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
    pub manifest_hash: AcpiManifestHash,
    /// GPA of payload offset 0.
    pub load_base: u64,
    /// Offset of the payload in the blob (the bytes the loader copies).
    pub payload_offset: usize,
    pub payload_len: usize,
    pub tables: Vec<AcpiBlobTable>,
}

impl ValidatedAcpiBlob {
    /// GPA range `[start, end)` the payload occupies.
    pub fn load_range(&self) -> (u64, u64) {
        (self.load_base, self.load_base + self.payload_len as u64)
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
/// Rejected: wrong magic/version/lengths, a foreign manifest hash, a non-zero RSDP address,
/// a payload outside `[0xE_0000, 0xF_0000)` or reaching into the NVS part, unknown or repeated
/// signatures, tables that overlap, are unaligned, run outside the payload or start in the RSDP
/// slot, a table whose `Length` field differs from its directory entry, a table whose bytes do
/// not add to zero, and non-zero bytes between tables. On success nothing is changed and
/// nothing is placed anywhere; the returned description names the load range and the tables.
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
    if blob[0x50..0x60].iter().any(|&byte| byte != 0) {
        return Err(AcpiBlobError::ReservedNotZero);
    }
    let rsdp = le64(blob, 0x40);
    if rsdp != 0 {
        return Err(AcpiBlobError::RsdpNotSupported(rsdp));
    }

    let load_base = le64(blob, 0x30);
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
    let payload_end = u64::from(payload_offset) + u64::from(payload_len);
    if (payload_offset as usize) < directory_end
        || !payload_offset.is_multiple_of(ACPI_BLOB_ALIGN)
        || payload_len < ACPI_BLOB_RSDP_SLOT_BYTES
        || payload_end != blob.len() as u64
    {
        return Err(AcpiBlobError::BadPayload {
            offset: payload_offset,
            len: payload_len,
        });
    }

    if !load_base.is_multiple_of(u64::from(ACPI_BLOB_ALIGN)) {
        return Err(AcpiBlobError::MisalignedLoadBase(load_base));
    }
    let load_end = load_base.checked_add(u64::from(payload_len));
    match load_end {
        Some(end) if load_base >= ACPI_REGION_BASE && end <= ACPI_REGION_END => {
            if end > ACPI_RECLAIM_END {
                return Err(AcpiBlobError::PayloadIntoNvs { end });
            }
        }
        _ => {
            return Err(AcpiBlobError::OutOfRegion {
                start: load_base,
                end: load_end,
            });
        }
    }

    let payload = &blob[payload_offset as usize..];
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

    let mut covered = vec![false; payload_len as usize];
    for slot in covered.iter_mut().take(ACPI_BLOB_RSDP_SLOT_BYTES as usize) {
        *slot = true;
    }
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
    // The RSDP slot is "covered" but must still be empty in this slice (rsdp_address is 0).
    if let Some(at) = payload.iter().enumerate().position(|(at, &byte)| {
        byte != 0 && (!covered[at] || at < ACPI_BLOB_RSDP_SLOT_BYTES as usize)
    }) {
        return Err(AcpiBlobError::NonZeroGap { payload_offset: at });
    }

    let mut manifest_hash = [0u8; 32];
    manifest_hash.copy_from_slice(&blob[0x10..0x30]);
    Ok(ValidatedAcpiBlob {
        version,
        manifest_hash,
        load_base,
        payload_offset: payload_offset as usize,
        payload_len: payload_len as usize,
        tables,
    })
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
