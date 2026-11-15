//! Parser for the machine-owned firmware handoff block (FIRMWARE_BOOT.md §6).
//!
//! This module is allocation-free so it can validate the header, CRC and table extents before
//! firmware services exist. Region/device record structure is checked; device-specific parameter
//! values and consistency with the complete board memory map are validated by later steps.

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
    MisalignedOffset {
        table: &'static str,
        offset: u32,
    },
    OverlappingTables,
    InvalidTableCount {
        table: &'static str,
        count: u32,
    },
    InvalidDeviceCountForVersion {
        version_minor: u16,
        count: u32,
    },
    InvalidRegionType {
        index: usize,
        kind: u32,
    },
    InvalidRegionAttributes {
        index: usize,
        attrs: u32,
    },
    InvalidRegionRange {
        index: usize,
    },
    InvalidRegionOwner {
        index: usize,
        owner: u32,
    },
    NonZeroRegionReserved {
        index: usize,
    },
    RegionsUnsortedOrOverlapping {
        index: usize,
    },
    InvalidDeviceType {
        index: usize,
        kind: u32,
    },
    DuplicateDeviceType {
        index: usize,
        kind: u32,
    },
    InvalidDeviceFlags {
        index: usize,
        flags: u32,
    },
    InvalidDeviceIrqFlags {
        index: usize,
        flags: u32,
    },
    InvalidDeviceRange {
        index: usize,
    },
    InvalidDeviceVersion {
        index: usize,
        kind: u32,
        version_minor: u16,
    },
    RequiredDeviceNotPresent {
        index: usize,
        kind: u32,
    },
    MissingDeviceType {
        kind: u32,
    },
    InvalidHeaderFlags(u32),
    InvalidBoardProfile(u32),
    InvalidCpuCount(u32),
    InvalidTscFrequency(u64),
    InvalidSerialLength(u16),
    InvalidSerial,
    NonZeroHeaderReserved {
        offset: usize,
    },
    InvalidTableEntrySize {
        table: &'static str,
        actual: u32,
    },
    TableOutOfBounds {
        table: &'static str,
    },
    BadCrc {
        stored: u32,
        computed: u32,
    },
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

fn validate_regions(
    bytes: &[u8],
    offset: usize,
    count: usize,
    device_count: usize,
    version_minor: u16,
) -> Result<(), HandoffError> {
    let mut previous_end = 0u64;
    for index in 0..count {
        let at = offset + index * 0x20;
        let kind = u32_at(bytes, at);
        let attrs = u32_at(bytes, at + 4);
        let base = u64_at(bytes, at + 8);
        let length = u64_at(bytes, at + 16);
        let owner = u32_at(bytes, at + 24);
        let reserved = u32_at(bytes, at + 28);

        if !(1..=12).contains(&kind) || (kind >= 11 && version_minor < 2) {
            return Err(HandoffError::InvalidRegionType { index, kind });
        }
        if (kind == 6 && attrs > 0xFF) || (kind != 6 && attrs != 0) {
            return Err(HandoffError::InvalidRegionAttributes { index, attrs });
        }
        if base % 0x1000 != 0
            || length == 0
            || length % 0x1000 != 0
            || base
                .checked_add(length)
                .is_none_or(|end| end > (1u64 << 36))
        {
            return Err(HandoffError::InvalidRegionRange { index });
        }
        if owner != u32::MAX && owner as usize >= device_count {
            return Err(HandoffError::InvalidRegionOwner { index, owner });
        }
        if reserved != 0 {
            return Err(HandoffError::NonZeroRegionReserved { index });
        }
        if index != 0 && base < previous_end {
            return Err(HandoffError::RegionsUnsortedOrOverlapping { index });
        }
        previous_end = base + length;
    }
    Ok(())
}

fn validate_devices(
    bytes: &[u8],
    offset: usize,
    count: usize,
    version_minor: u16,
) -> Result<(), HandoffError> {
    let mut seen = [false; 10];
    for index in 0..count {
        let at = offset + index * 0x30;
        let kind = u32_at(bytes, at);
        let flags = u32_at(bytes, at + 4);
        let base = u64_at(bytes, at + 8);
        let length = u64_at(bytes, at + 16);
        let irq_flags = u32_at(bytes, at + 0x1C);
        let present = flags & 1 != 0;
        let pio = flags & 2 != 0;

        if !(1..=9).contains(&kind) {
            return Err(HandoffError::InvalidDeviceType { index, kind });
        }
        let required_minor = match kind {
            8 => 1,
            9 => 2,
            _ => 0,
        };
        if version_minor < required_minor {
            return Err(HandoffError::InvalidDeviceVersion {
                index,
                kind,
                version_minor,
            });
        }
        if seen[kind as usize] {
            return Err(HandoffError::DuplicateDeviceType { index, kind });
        }
        seen[kind as usize] = true;
        if flags & !0x3 != 0 {
            return Err(HandoffError::InvalidDeviceFlags { index, flags });
        }
        if irq_flags & !0x3 != 0 {
            return Err(HandoffError::InvalidDeviceIrqFlags {
                index,
                flags: irq_flags,
            });
        }
        if pio && !matches!(kind, 1 | 9) || present && matches!(kind, 1 | 9) && !pio {
            return Err(HandoffError::InvalidDeviceFlags { index, flags });
        }
        if kind >= 8 && !present {
            return Err(HandoffError::RequiredDeviceNotPresent { index, kind });
        }
        if (!present && (base != 0 || length != 0))
            || (present
                && (length == 0
                    || if pio {
                        base > 0xFFFF || base.checked_add(length).is_none_or(|end| end > 0x1_0000)
                    } else {
                        base.checked_add(length)
                            .is_none_or(|end| end > (1u64 << 36))
                    }))
        {
            return Err(HandoffError::InvalidDeviceRange { index });
        }
    }
    for kind in 1..=7 {
        if !seen[kind] {
            return Err(HandoffError::MissingDeviceType { kind: kind as u32 });
        }
    }
    if version_minor == 1 && !seen[8] {
        return Err(HandoffError::MissingDeviceType { kind: 8 });
    }
    if version_minor == 2 && !seen[9] {
        return Err(HandoffError::MissingDeviceType { kind: 9 });
    }
    Ok(())
}

fn validate_header_fields(bytes: &[u8]) -> Result<(), HandoffError> {
    let cpu_count = u32_at(bytes, 0x1C);
    if cpu_count != 1 {
        return Err(HandoffError::InvalidCpuCount(cpu_count));
    }
    let tsc_hz = u64_at(bytes, 0x20);
    if tsc_hz == 0 {
        return Err(HandoffError::InvalidTscFrequency(tsc_hz));
    }
    let serial_len = u16_at(bytes, 0x58);
    if serial_len > 64 {
        return Err(HandoffError::InvalidSerialLength(serial_len));
    }
    let serial_end = 0x60 + serial_len as usize;
    if bytes[0x60..serial_end]
        .iter()
        .any(|byte| !(0x20..=0x7E).contains(byte))
        || bytes[serial_end..0xA0].iter().any(|&byte| byte != 0)
    {
        return Err(HandoffError::InvalidSerial);
    }
    let flags = u32_at(bytes, 0x18);
    if flags != 0 {
        return Err(HandoffError::InvalidHeaderFlags(flags));
    }
    let board_profile = u32_at(bytes, 0x28);
    if board_profile != 1 {
        return Err(HandoffError::InvalidBoardProfile(board_profile));
    }
    for (offset, range) in [(0x2C, 0x2C..0x30), (0x5A, 0x5A..0x60)] {
        if bytes[range].iter().any(|&byte| byte != 0) {
            return Err(HandoffError::NonZeroHeaderReserved { offset });
        }
    }
    Ok(())
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
        if minor > supported_minor_max || minor > 2 {
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
        if !(1..=128).contains(&region_count) {
            return Err(HandoffError::InvalidTableCount {
                table: "region",
                count: region_count,
            });
        }
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
        let valid_device_count = match minor {
            0 => device_count == 7,
            1 => device_count == 8,
            2 => (8..=9).contains(&device_count),
            _ => false,
        };
        if !valid_device_count {
            return Err(HandoffError::InvalidDeviceCountForVersion {
                version_minor: minor,
                count: device_count,
            });
        }
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
        validate_regions(
            bytes,
            region_offset as usize,
            region_count as usize,
            device_count as usize,
            minor,
        )?;
        validate_devices(bytes, device_offset as usize, device_count as usize, minor)?;
        validate_header_fields(bytes)?;
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

    fn sample() -> Vec<u8> {
        let region_offset = HEADER_SIZE;
        let device_offset = region_offset + 0x20;
        let total_size = device_offset + 7 * 0x30;
        let mut b = vec![0u8; total_size];
        b[..8].copy_from_slice(MAGIC);
        b[0x08..0x0A].copy_from_slice(&1u16.to_le_bytes());
        b[0x0A..0x0C].copy_from_slice(&0u16.to_le_bytes());
        b[0x0C..0x10].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        b[0x10..0x14].copy_from_slice(&(total_size as u32).to_le_bytes());
        b[0x1C..0x20].copy_from_slice(&1u32.to_le_bytes());
        b[0x20..0x28].copy_from_slice(&1_000_000_000u64.to_le_bytes());
        b[0x28..0x2C].copy_from_slice(&1u32.to_le_bytes());
        b[0x30..0x34].copy_from_slice(&(region_offset as u32).to_le_bytes());
        b[0x34..0x38].copy_from_slice(&1u32.to_le_bytes());
        b[0x38..0x3C].copy_from_slice(&0x20u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&(device_offset as u32).to_le_bytes());
        b[0x40..0x44].copy_from_slice(&7u32.to_le_bytes());
        b[0x44..0x48].copy_from_slice(&0x30u32.to_le_bytes());
        b[region_offset..region_offset + 4].copy_from_slice(&1u32.to_le_bytes());
        b[region_offset + 16..region_offset + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        b[region_offset + 24..region_offset + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        for index in 0..7 {
            let at = device_offset + index * 0x30;
            b[at..at + 4].copy_from_slice(&((index + 1) as u32).to_le_bytes());
        }
        let crc = crc32_with_zeroed_field(&b);
        b[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
        b
    }

    fn seal(bytes: &mut [u8]) {
        let crc = crc32_with_zeroed_field(bytes);
        bytes[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&crc.to_le_bytes());
    }

    fn sample_with_two_regions() -> Vec<u8> {
        let mut b = sample();
        let old_device_offset = HEADER_SIZE + 0x20;
        let new_device_offset = old_device_offset + 0x20;
        b.resize(b.len() + 0x20, 0);
        b.copy_within(
            old_device_offset..old_device_offset + 7 * 0x30,
            new_device_offset,
        );
        let total_size = b.len() as u32;
        b[0x10..0x14].copy_from_slice(&total_size.to_le_bytes());
        b[0x34..0x38].copy_from_slice(&2u32.to_le_bytes());
        b[0x3C..0x40].copy_from_slice(&(new_device_offset as u32).to_le_bytes());
        let second = old_device_offset;
        b[second..second + 4].copy_from_slice(&1u32.to_le_bytes());
        b[second + 8..second + 16].copy_from_slice(&0u64.to_le_bytes());
        b[second + 16..second + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        b[second + 24..second + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        seal(&mut b);
        b
    }

    fn sample_minor1() -> Vec<u8> {
        let mut b = sample();
        let device_offset = u32_at(&b, 0x3C) as usize;
        b.resize(b.len() + 0x30, 0);
        let total_size = b.len() as u32;
        b[0x10..0x14].copy_from_slice(&total_size.to_le_bytes());
        b[0x0A..0x0C].copy_from_slice(&1u16.to_le_bytes());
        b[0x40..0x44].copy_from_slice(&8u32.to_le_bytes());
        let extension = device_offset + 7 * 0x30;
        b[extension..extension + 4].copy_from_slice(&8u32.to_le_bytes());
        b[extension + 4..extension + 8].copy_from_slice(&1u32.to_le_bytes());
        b[extension + 8..extension + 16].copy_from_slice(&0xFC00_1000u64.to_le_bytes());
        b[extension + 16..extension + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        seal(&mut b);
        b
    }

    #[test]
    fn parses_canonical_v1_tables_and_checks_crc() {
        let b = sample();
        let parsed = Handoff::parse(&b, 2).unwrap();
        assert_eq!(parsed.region_count, 1);
        assert_eq!(parsed.device_count, 7);
        assert_eq!(parsed.device_entry_size, 0x30);
        assert_eq!(parsed.bytes().len(), b.len());
    }

    #[test]
    fn accepts_canonical_v1_block_with_48_byte_device_records() {
        let b = sample();
        let parsed = Handoff::parse(&b, 2).unwrap();
        assert_eq!(parsed.region_count, 1);
        assert_eq!(parsed.device_count, 7);
        assert_eq!(parsed.device_entry_size, 0x30);
        assert_eq!(parsed.bytes().len(), b.len());
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
        seal(&mut b);
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
        let mut b = sample();
        b[0x3C..0x40].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
        seal(&mut b);
        assert_eq!(Handoff::parse(&b, 2), Err(HandoffError::OverlappingTables));

        b[0x3C..0x40].copy_from_slice(&((HEADER_SIZE + 1) as u32).to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::MisalignedOffset {
                table: "device",
                ..
            })
        ));
    }

    #[test]
    fn validates_region_kind_attributes_range_owner_reserved_and_order() {
        let mut b = sample();
        let at = HEADER_SIZE;
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());

        b[at..at + 4].copy_from_slice(&99u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidRegionType { .. })
        ));
        b[at..at + 4].copy_from_slice(&1u32.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&256u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidRegionAttributes { .. })
        ));

        b[at + 4..at + 8].copy_from_slice(&0u32.to_le_bytes());
        b[at + 16..at + 24].copy_from_slice(&0u64.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidRegionRange { .. })
        ));

        b[at + 16..at + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        b[at + 24..at + 28].copy_from_slice(&7u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidRegionOwner { .. })
        ));

        b[at + 24..at + 28].copy_from_slice(&u32::MAX.to_le_bytes());
        b[at + 28..at + 32].copy_from_slice(&1u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::NonZeroRegionReserved { .. })
        ));

        b[at + 28..at + 32].copy_from_slice(&0u32.to_le_bytes());
        b[at + 8..at + 16].copy_from_slice(&((1u64 << 36) - 0x1000).to_le_bytes());
        b[at + 16..at + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidRegionRange { .. })
        ));

        b[at..at + 4].copy_from_slice(&6u32.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&0xFFu32.to_le_bytes());
        b[at + 8..at + 16].copy_from_slice(&0u64.to_le_bytes());
        b[at + 16..at + 24].copy_from_slice(&0x1000u64.to_le_bytes());
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());

        b[at..at + 4].copy_from_slice(&11u32.to_le_bytes());
        b[at + 4..at + 8].copy_from_slice(&0u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 0),
            Err(HandoffError::InvalidRegionType { .. })
        ));
        b.extend_from_slice(&[0; 0x30]);
        let total_size = b.len() as u32;
        b[0x10..0x14].copy_from_slice(&total_size.to_le_bytes());
        b[0x40..0x44].copy_from_slice(&8u32.to_le_bytes());
        let device_offset = u32_at(&b, 0x3C) as usize;
        let extension = device_offset + 7 * 0x30;
        b[extension..extension + 4].copy_from_slice(&9u32.to_le_bytes());
        b[extension + 4..extension + 8].copy_from_slice(&3u32.to_le_bytes());
        b[extension + 8..extension + 16].copy_from_slice(&0x600u64.to_le_bytes());
        b[extension + 16..extension + 24].copy_from_slice(&0x10u64.to_le_bytes());
        b[extension + 0x18..extension + 0x1C].copy_from_slice(&9u32.to_le_bytes());
        b[extension + 0x1C..extension + 0x20].copy_from_slice(&3u32.to_le_bytes());
        b[0x0A..0x0C].copy_from_slice(&2u16.to_le_bytes());
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());
        b[extension + 4..extension + 8].copy_from_slice(&0u32.to_le_bytes());
        b[extension + 8..extension + 24].fill(0);
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::RequiredDeviceNotPresent { kind: 9, .. })
        ));
    }

    #[test]
    fn rejects_unsorted_or_overlapping_region_ranges_and_invalid_counts() {
        let mut b = sample_with_two_regions();
        let second = HEADER_SIZE + 0x20;
        // This page overlaps the first RAM page and is also out of order.
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::RegionsUnsortedOrOverlapping { index: 1 })
        );

        b[second + 8..second + 16].copy_from_slice(&0x2000u64.to_le_bytes());
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());

        b[0x34..0x38].copy_from_slice(&0u32.to_le_bytes());
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidTableCount {
                table: "region",
                count: 0
            })
        );

        let mut b = sample();
        b[0x40..0x44].copy_from_slice(&8u32.to_le_bytes());
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceCountForVersion {
                version_minor: 0,
                count: 8
            })
        );
    }

    #[test]
    fn validates_device_types_flags_presence_and_address_ranges() {
        let mut b = sample();
        let offset = u32_at(&b, 0x3C) as usize;
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());

        b[offset..offset + 4].copy_from_slice(&99u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceType { .. })
        ));

        b[offset..offset + 4].copy_from_slice(&1u32.to_le_bytes());
        let second = offset + 0x30;
        b[second..second + 4].copy_from_slice(&1u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::DuplicateDeviceType { .. })
        ));

        b[second..second + 4].copy_from_slice(&2u32.to_le_bytes());
        b[offset + 4..offset + 8].copy_from_slice(&4u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceFlags { .. })
        ));

        b[offset + 4..offset + 8].copy_from_slice(&0u32.to_le_bytes());
        b[offset + 0x1C..offset + 0x20].copy_from_slice(&4u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceIrqFlags { .. })
        ));

        b[offset + 0x1C..offset + 0x20].copy_from_slice(&0u32.to_le_bytes());
        b[offset + 8..offset + 16].copy_from_slice(&1u64.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceRange { .. })
        ));

        b[offset + 8..offset + 16].copy_from_slice(&0xFFFFu64.to_le_bytes());
        b[offset + 16..offset + 24].copy_from_slice(&2u64.to_le_bytes());
        b[offset + 4..offset + 8].copy_from_slice(&3u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceRange { .. })
        ));

        b[offset + 8..offset + 16].copy_from_slice(&0x3F8u64.to_le_bytes());
        b[offset + 16..offset + 24].copy_from_slice(&8u64.to_le_bytes());
        b[offset + 4..offset + 8].copy_from_slice(&3u32.to_le_bytes());
        seal(&mut b);
        assert!(Handoff::parse(&b, 2).is_ok());

        let third = offset + 0x30;
        b[third + 4..third + 8].copy_from_slice(&1u32.to_le_bytes());
        b[third + 8..third + 16].copy_from_slice(&((1u64 << 36) - 0x1000).to_le_bytes());
        b[third + 16..third + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceRange { .. })
        ));

        let mut b = sample();
        let offset = u32_at(&b, 0x3C) as usize;
        b[offset..offset + 4].copy_from_slice(&8u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceVersion { .. })
        ));

        let mut b = sample_minor1();
        assert!(Handoff::parse(&b, 2).is_ok());
        let extension = u32_at(&b, 0x3C) as usize + 7 * 0x30;
        b[extension + 4..extension + 8].copy_from_slice(&0u32.to_le_bytes());
        b[extension + 8..extension + 24].fill(0);
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::RequiredDeviceNotPresent { kind: 8, .. })
        ));
        b[extension + 4..extension + 8].copy_from_slice(&1u32.to_le_bytes());
        b[extension + 8..extension + 16].copy_from_slice(&0xFC00_1000u64.to_le_bytes());
        b[extension + 16..extension + 24].copy_from_slice(&0x2000u64.to_le_bytes());
        b[extension..extension + 4].copy_from_slice(&9u32.to_le_bytes());
        seal(&mut b);
        assert!(matches!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidDeviceVersion { .. })
        ));
    }

    #[test]
    fn validates_cpu_profile_clock_board_reserved_and_serial_fields() {
        let cases: &[(usize, &[u8], HandoffError)] = &[
            (0x18, &[1, 0, 0, 0], HandoffError::InvalidHeaderFlags(1)),
            (0x1C, &[2, 0, 0, 0], HandoffError::InvalidCpuCount(2)),
            (0x20, &[0; 8], HandoffError::InvalidTscFrequency(0)),
            (0x28, &[2, 0, 0, 0], HandoffError::InvalidBoardProfile(2)),
        ];
        for (offset, value, expected) in cases {
            let mut b = sample();
            b[*offset..*offset + value.len()].copy_from_slice(value);
            seal(&mut b);
            assert_eq!(Handoff::parse(&b, 2), Err(*expected));
        }

        let mut b = sample();
        b[0x2C] = 1;
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::NonZeroHeaderReserved { offset: 0x2C })
        );
        let mut b = sample();
        b[0x5A] = 1;
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::NonZeroHeaderReserved { offset: 0x5A })
        );
        let mut b = sample();
        b[0x58..0x5A].copy_from_slice(&65u16.to_le_bytes());
        seal(&mut b);
        assert_eq!(
            Handoff::parse(&b, 2),
            Err(HandoffError::InvalidSerialLength(65))
        );
        let mut b = sample();
        b[0x58..0x5A].copy_from_slice(&1u16.to_le_bytes());
        b[0x60] = 0x7F;
        seal(&mut b);
        assert_eq!(Handoff::parse(&b, 2), Err(HandoffError::InvalidSerial));
        let mut b = sample();
        b[0x61] = b'X';
        seal(&mut b);
        assert_eq!(Handoff::parse(&b, 2), Err(HandoffError::InvalidSerial));
    }
}
