//! System Description Table header and checksum.
//!
//! Spec: ACPI 5.2.6 "System Description Table Header" (Table 5.4) and 5.2.11.1 "DSDT"
//! (Table 5.17), read from the ACPI 6.4 edition through Context7 (the 6.6 pages returned
//! HTTP 403). The rule "the entire table, including the checksum field, must add to zero" is
//! Table 5.4 / 5.17; the 6.6 wording is NOT VERIFIED.

/// Length of the header shared by all System Description Tables (Table 5.4).
pub const SDT_HEADER_LEN: usize = 36;
const OFFSET_LENGTH: usize = 4;
const OFFSET_CHECKSUM: usize = 9;

/// Own identification strings of tables the firmware builds. The values are chosen by this
/// project (S7-13 takes them over for the other tables); they do not imitate a vendor.
pub const OEM_ID: [u8; 6] = *b"OPNDSK";
pub const OEM_REVISION: u32 = 1;
pub const CREATOR_ID: [u8; 4] = *b"ODFW";
pub const CREATOR_REVISION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableError {
    /// Fewer bytes than a header, or the output buffer is smaller than the table.
    TooShort,
    SignatureMismatch,
    /// The `Length` field differs from the number of bytes given.
    LengthMismatch {
        declared: u32,
        actual: usize,
    },
    /// The bytes of the table do not add to zero modulo 256.
    BadChecksum {
        sum: u8,
    },
}

/// Sum of all bytes modulo 256.
pub fn byte_sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |acc, &b| acc.wrapping_add(b))
}

/// Writes the 36-byte header into `out[..36]` with a zero checksum; `total_len` is the length
/// of the whole table including the header. Call [`finish_checksum`] once the body is in place.
pub fn write_header(
    out: &mut [u8],
    signature: [u8; 4],
    revision: u8,
    oem_table_id: [u8; 8],
    total_len: u32,
) -> Result<(), TableError> {
    let header = out.get_mut(..SDT_HEADER_LEN).ok_or(TableError::TooShort)?;
    header[0..4].copy_from_slice(&signature);
    header[4..8].copy_from_slice(&total_len.to_le_bytes());
    header[8] = revision;
    header[9] = 0;
    header[10..16].copy_from_slice(&OEM_ID);
    header[16..24].copy_from_slice(&oem_table_id);
    header[24..28].copy_from_slice(&OEM_REVISION.to_le_bytes());
    header[28..32].copy_from_slice(&CREATOR_ID);
    header[32..36].copy_from_slice(&CREATOR_REVISION.to_le_bytes());
    Ok(())
}

/// Sets the checksum byte so that the whole table adds to zero.
pub fn finish_checksum(table: &mut [u8]) -> Result<(), TableError> {
    if table.len() < SDT_HEADER_LEN {
        return Err(TableError::TooShort);
    }
    table[OFFSET_CHECKSUM] = 0;
    table[OFFSET_CHECKSUM] = byte_sum(table).wrapping_neg();
    Ok(())
}

/// Checks signature, `Length` field against the slice, and the checksum.
pub fn validate_table(table: &[u8], signature: &[u8; 4]) -> Result<(), TableError> {
    if table.len() < SDT_HEADER_LEN {
        return Err(TableError::TooShort);
    }
    if table[0..4] != signature[..] {
        return Err(TableError::SignatureMismatch);
    }
    let declared = u32::from_le_bytes([
        table[OFFSET_LENGTH],
        table[OFFSET_LENGTH + 1],
        table[OFFSET_LENGTH + 2],
        table[OFFSET_LENGTH + 3],
    ]);
    if usize::try_from(declared) != Ok(table.len()) {
        return Err(TableError::LengthMismatch {
            declared,
            actual: table.len(),
        });
    }
    match byte_sum(table) {
        0 => Ok(()),
        sum => Err(TableError::BadChecksum { sum }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> [u8; 40] {
        let mut table = [0u8; 40];
        write_header(&mut table, *b"TEST", 1, *b"TESTTBL1", 40).unwrap();
        table[36..].copy_from_slice(&[1, 2, 3, 4]);
        finish_checksum(&mut table).unwrap();
        table
    }

    #[test]
    fn header_fields_sit_at_the_documented_offsets() {
        // ACPI Table 5.4: Signature 0, Length 4, Revision 8, Checksum 9, OEMID 10, OEM Table ID
        // 16, OEM Revision 24, Creator ID 28, Creator Revision 32.
        let t = sample();
        assert_eq!(&t[0..4], b"TEST");
        assert_eq!(&t[4..8], &40u32.to_le_bytes());
        assert_eq!(t[8], 1);
        assert_eq!(&t[10..16], b"OPNDSK");
        assert_eq!(&t[16..24], b"TESTTBL1");
        assert_eq!(&t[24..28], &1u32.to_le_bytes());
        assert_eq!(&t[28..32], b"ODFW");
        assert_eq!(&t[32..36], &1u32.to_le_bytes());
    }

    #[test]
    fn checksum_makes_the_table_sum_to_zero() {
        // Header bytes sum + body 1+2+3+4 = 10; the checksum byte is whatever completes 0 mod 256.
        let t = sample();
        assert_eq!(byte_sum(&t), 0);
        assert_eq!(validate_table(&t, b"TEST"), Ok(()));
    }

    #[test]
    fn validation_reports_each_defect() {
        let mut t = sample();
        assert_eq!(
            validate_table(&t, b"OTHR"),
            Err(TableError::SignatureMismatch)
        );
        assert_eq!(
            validate_table(&t[..39], b"TEST"),
            Err(TableError::LengthMismatch {
                declared: 40,
                actual: 39
            })
        );
        t[37] ^= 0x10;
        assert_eq!(
            validate_table(&t, b"TEST"),
            Err(TableError::BadChecksum { sum: 0x10 })
        );
        assert_eq!(validate_table(&t[..8], b"TEST"), Err(TableError::TooShort));
    }
}
