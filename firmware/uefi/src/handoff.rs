//! Parser for the machine-owned firmware handoff block (FIRMWARE_BOOT.md §6).
//!
//! This module is allocation-free so it can validate the header, CRC and table extents before
//! firmware services exist. It does not validate region/device contents or reserved fields.

pub const HANDOFF_WINDOW_SIZE: usize = 0x1_0000;
const MAGIC: &[u8; 8] = b"ODHANDOF";
const HEADER_SIZE: usize = 0xA0;
const CRC_OFFSET: usize = 0x14;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Handoff<'a> {
    bytes: &'a [u8],
    pub version_minor: u16,
    pub tsc_hz: u64,
    pub board_profile: u32,
    pub region_table_offset: usize,
    pub region_count: usize,
    pub region_entry_size: usize,
    pub device_table_offset: usize,
    pub device_count: usize,
    pub device_entry_size: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HandoffError {
    TruncatedHeader,
    BadMagic,
    UnsupportedMajor(u16),
    UnsupportedMinor(u16),
    InvalidHeaderSize(u32),
    InvalidTotalSize(u32),
    MisalignedOffset { table: &'static str, offset: u32 },
    OverlappingTables,
    InvalidTableEntrySize { table: &'static str, actual: u32 },
    TableOutOfBounds { table: &'static str },
    BadCrc { stored: u32, computed: u32 },
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(
        bytes[offset..offset + 4]
            .try_into()
            .expect("validated fixed field"),
    )
}

fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(
        bytes[offset..offset + 8]
            .try_into()
            .expect("validated fixed field"),
    )
}

fn crc32_with_zeroed_field(bytes: &[u8]) -> u32 {
    let mut state = 0xFFFF_FFFFu32;
    for (index, &byte) in bytes.iter().enumerate() {
        state ^= u32::from(if (CRC_OFFSET..CRC_OFFSET + 4).contains(&index) {
            0
        } else {
            byte
        });
        for _ in 0..8 {
            state = if state & 1 != 0 {
                (state >> 1) ^ 0xEDB8_8320
            } else {
                state >> 1
            };
        }
    }
    state ^ 0xFFFF_FFFF
}

fn table_fits(offset: u32, count: u32, entry_size: u32, total: usize) -> bool {
    let Some(size) = (count as usize).checked_mul(entry_size as usize) else {
        return false;
    };
    let Some(end) = (offset as usize).checked_add(size) else {
        return false;
    };
    offset as usize >= HEADER_SIZE && end <= total
}

fn table_end(offset: u32, count: u32, entry_size: u32) -> usize {
    offset as usize + count as usize * entry_size as usize
}

impl<'a> Handoff<'a> {
    /// Validates the fixed header, table extents and CRC before exposing any offsets.
    pub fn parse(bytes: &'a [u8], supported_minor_max: u16) -> Result<Self, HandoffError> {
        if bytes.len() < HEADER_SIZE {
            return Err(HandoffError::TruncatedHeader);
        }
        if &bytes[..8] != MAGIC {
            return Err(HandoffError::BadMagic);
        }
        let major = u16_at(bytes, 0x08);
        if major != 1 {
            return Err(HandoffError::UnsupportedMajor(major));
        }
        let minor = u16_at(bytes, 0x0A);
        if minor > supported_minor_max {
            return Err(HandoffError::UnsupportedMinor(minor));
        }
        let header_size = u32_at(bytes, 0x0C);
        if header_size as usize != HEADER_SIZE {
            return Err(HandoffError::InvalidHeaderSize(header_size));
        }
        let total_size = u32_at(bytes, 0x10);
        let total = total_size as usize;
        if !(HEADER_SIZE..=HANDOFF_WINDOW_SIZE).contains(&total)
            || total > bytes.len()
            || total % 8 != 0
        {
            return Err(HandoffError::InvalidTotalSize(total_size));
        }
        // CRC is the first content check after validating the range it covers. Report corruption
        // uniformly before interpreting table metadata from the damaged block.
        let stored = u32_at(bytes, CRC_OFFSET);
        let computed = crc32_with_zeroed_field(&bytes[..total]);
        if stored != computed {
            return Err(HandoffError::BadCrc { stored, computed });
        }
        let region_offset = u32_at(bytes, 0x30);
        let region_count = u32_at(bytes, 0x34);
        let region_entry_size = u32_at(bytes, 0x38);
        if region_entry_size != 0x20 {
            return Err(HandoffError::InvalidTableEntrySize {
                table: "region",
                actual: region_entry_size,
            });
        }
        if region_offset % 8 != 0 {
            return Err(HandoffError::MisalignedOffset {
                table: "region",
                offset: region_offset,
            });
        }
        if !table_fits(region_offset, region_count, region_entry_size, total) {
            return Err(HandoffError::TableOutOfBounds { table: "region" });
        }
        let device_offset = u32_at(bytes, 0x3C);
        let device_count = u32_at(bytes, 0x40);
        let device_entry_size = u32_at(bytes, 0x44);
        if device_entry_size != 0x30 {
            return Err(HandoffError::InvalidTableEntrySize {
                table: "device",
                actual: device_entry_size,
            });
        }
        if device_offset % 8 != 0 {
            return Err(HandoffError::MisalignedOffset {
                table: "device",
                offset: device_offset,
            });
        }
        if !table_fits(device_offset, device_count, device_entry_size, total) {
            return Err(HandoffError::TableOutOfBounds { table: "device" });
        }
        let region_end = table_end(region_offset, region_count, region_entry_size);
        let device_end = table_end(device_offset, device_count, device_entry_size);
        if region_count != 0
            && device_count != 0
            && (region_offset as usize) < device_end
            && (device_offset as usize) < region_end
        {
            return Err(HandoffError::OverlappingTables);
        }
        Ok(Self {
            bytes: &bytes[..total],
            version_minor: minor,
            tsc_hz: u64_at(bytes, 0x20),
            board_profile: u32_at(bytes, 0x28),
            region_table_offset: region_offset as usize,
            region_count: region_count as usize,
            region_entry_size: region_entry_size as usize,
            device_table_offset: device_offset as usize,
            device_count: device_count as usize,
            device_entry_size: device_entry_size as usize,
        })
    }

    pub fn bytes(&self) -> &'a [u8] {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> [u8; HEADER_SIZE] {
        let mut b = [0u8; HEADER_SIZE];
        b[..8].copy_from_slice(MAGIC);
        b[0x08..0x0A].copy_from_slice(&1u16.to_le_bytes());
        b[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes());
        b[0x0C..0x10].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x10..0x14].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x30..0x34].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x38..0x3C].copy_from_slice(&0x20u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x44..0x48].copy_from_slice(&0x30u32.to_le_bytes());
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    #[test]
    fn parses_valid_empty_tables_and_checks_crc() {
        let b = sample();
        let parsed = Handoff::parse(&b, 2).unwrap();
        assert_eq!(parsed.region_count, 0);
        assert_eq!(parsed.device_count, 0);
        assert_eq!(parsed.bytes().len(), HEADER_SIZE);
    }

    #[test]
    fn accepts_canonical_v1_block_with_48_byte_device_records() {
        let region_offset = HEADER_SIZE;
        let device_offset = region_offset + 0x20;
        let total_size = device_offset + 7 * 0x30;
        let mut b = vec![0u8; total_size];
        b[..HEADER_SIZE].copy_from_slice(&sample());
        b[0x10..0x14].copy_from_slice(&(total_size as u32).to_le_bytes());
        b[0x34..0x38].copy_from_slice(&1u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&(device_offset as u32).to_le_bytes());
        b[0x40..0x44].copy_from_slice(&7u32.to_le_bytes());
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());

        let parsed = Handoff::parse(&b, 2).unwrap();
        assert_eq!(parsed.region_count, 1);
        assert_eq!(parsed.device_count, 7);
        assert_eq!(parsed.device_entry_size, 0x30);
        assert_eq!(parsed.bytes().len(), total_size);
    }

    #[test]
    fn rejects_invalid_signature_version_and_sizes() {
        let mut b = sample();
        b[0] ^= 1;
        assert_eq!(Handoff::parse(&b, 2), Err(HandoffError::BadMagic));
        let mut b = sample();
        b[8] = 2;
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::UnsupportedMajor(2))
        );
        let mut b = sample();
        b[10] = 3;
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::UnsupportedMinor(3))
        );
        let mut b = sample();
        b[12] = 0;
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidHeaderSize(_))
        ));
        assert_eq!(
            Handoff::parse(&b[..16], 2),
            Err(HandoffError::TruncatedHeader)
        );
    }

    #[test]
    fn rejects_tables_outside_declared_block_and_crc_mismatch() {
        let mut b = sample();
        b[0x30..0x34].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::BadCrc { .. })
        ));
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::TableOutOfBounds { table: "region" })
        );
        let mut b = sample();
        b[0x20] = 1;
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::BadCrc { .. })
        ));
    }

    #[test]
    fn rejects_overlapping_and_misaligned_tables() {
        let mut b = [0u8; HEADER_SIZE + 0x30];
        b[..HEADER_SIZE].copy_from_slice(&sample());
        let total_size = b.len() as u32;
        b[0x10..0x14].copy_from_slice(&total_size.to_le_bytes());
        b[0x34..0x38].copy_from_slice(&1u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x40..0x44].copy_from_slice(&1u32.to_le_bytes());
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert_eq!(Handoff::parse(&b, 2), Err(HandoffError::OverlappingTables));

        b[0x3C..0x40].copy_from_slice(&((HEADER_SIZE + 1) as u32).to_le_bytes());
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::MisalignedOffset {
                table: "device",
                ..
            })
        ));
    }
}
