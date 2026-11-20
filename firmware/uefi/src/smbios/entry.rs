//! SMBIOS 3.x 64-bit entry point (DSP0134 3.9, section 5.2.2).

use crate::{acpi::table::byte_sum, uefi::guid::Guid};

pub const SMBIOS3_TABLE_GUID: Guid = Guid {
    data1: 0xf2fd_1544,
    data2: 0x9794,
    data3: 0x4a2c,
    data4: [0x99, 0x2e, 0xe5, 0xbb, 0xcf, 0x20, 0xe3, 0x94],
};

pub const ENTRY_LEN: usize = 0x18;
const ANCHOR: &[u8; 5] = b"_SM3_";

pub(super) fn build(table_address: u64, table_len: u32) -> [u8; ENTRY_LEN] {
    let mut entry = [0; ENTRY_LEN];
    entry[..5].copy_from_slice(ANCHOR);
    entry[6] = ENTRY_LEN as u8;
    entry[7..11].copy_from_slice(&[3, 9, 0, 1]);
    entry[12..16].copy_from_slice(&table_len.to_le_bytes());
    entry[16..24].copy_from_slice(&table_address.to_le_bytes());
    entry[5] = 0u8.wrapping_sub(byte_sum(&entry));
    entry
}

/// Check the complete entry point and return the physical table address and size bound.
pub fn validate(entry: &[u8; ENTRY_LEN]) -> Option<(u64, usize)> {
    if &entry[..5] != ANCHOR
        || entry[6] as usize != ENTRY_LEN
        || entry[7..12] != [3, 9, 0, 1, 0]
        || byte_sum(entry) != 0
    {
        return None;
    }
    let len = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
    let address = u64::from_le_bytes(entry[16..24].try_into().ok()?);
    if len == 0 || address == 0 || address.checked_add(len as u64).is_none() {
        return None;
    }
    Some((address, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_point_has_an_independent_checksum_and_64_bit_address() {
        let entry = build(0x12_3456_7890, 0x200);
        assert_eq!(
            &entry,
            b"_SM3_\xa4\x18\x03\x09\x00\x01\x00\x00\x02\x00\x00\x90\x78\x56\x34\x12\x00\x00\x00"
        );
        assert_eq!(validate(&entry), Some((0x12_3456_7890, 0x200)));
        let mut bad = entry;
        bad[17] ^= 1;
        assert_eq!(validate(&bad), None);
    }
}
