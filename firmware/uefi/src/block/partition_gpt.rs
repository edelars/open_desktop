//! GPT and protective MBR decoding for the board's 512-byte logical blocks (UEFI 2.11 §5).

use crate::{crc32, nvme::driver::BLOCK_BYTES, uefi::guid::Guid};
const BASE_ENTRY_BYTES: usize = 128;
const HEADER_BYTES: usize = 92;
const MIN_TABLE_BLOCKS: u64 = 16_384 / BLOCK_BYTES as u64;
const PRIMARY_LBA: u64 = 1;
const NULL_GUID: Guid = Guid {
    data1: 0,
    data2: 0,
    data3: 0,
    data4: [0; 8],
};

pub(crate) const SYSTEM_PARTITION: Guid = Guid {
    data1: 0xc12a_7328,
    data2: 0xf81f,
    data3: 0x11d2,
    data4: [0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b],
};

const NO_BLOCK_IO: u64 = 1 << 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Header {
    pub(crate) table_lba: u64,
    pub(crate) table_bytes: usize,
    entry_bytes: usize,
    table_crc: u32,
    first_usable: u64,
    last_usable: u64,
}

/// The exact EFI_PARTITION_ENTRY payload embedded in EFI_PARTITION_INFO_PROTOCOL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct Partition {
    pub partition_type: Guid,
    pub unique_id: Guid,
    pub start: u64,
    pub end: u64,
    pub attributes: u64,
    pub name: [u16; 36],
}

impl Partition {
    pub(crate) fn exposes_block_io(&self) -> bool {
        self.attributes & NO_BLOCK_IO == 0
    }
}

pub(crate) fn protective_mbr(block: &[u8; BLOCK_BYTES], last_lba: u64) -> bool {
    let entries = &block[446..510];
    let mut protective = false;
    for entry in entries.chunks_exact(16) {
        if entry.iter().all(|&byte| byte == 0) {
            continue;
        }
        if protective
            || entry[4] != 0xee
            || read_u32(entry, 8) != 1
            || read_u32(entry, 12) != last_lba.min(u32::MAX as u64) as u32
        {
            return false;
        }
        protective = true;
    }
    protective && block[510..] == [0x55, 0xaa]
}

impl Header {
    #[cfg(target_os = "none")]
    pub(crate) fn entry_bytes(&self) -> usize {
        self.entry_bytes
    }
    pub(crate) fn parse(block: &[u8; BLOCK_BYTES], lba: u64, last_lba: u64) -> Option<Self> {
        if last_lba <= PRIMARY_LBA || (lba != PRIMARY_LBA && lba != last_lba) {
            return None;
        }
        let alternate = if lba == PRIMARY_LBA {
            last_lba
        } else {
            PRIMARY_LBA
        };
        let header_bytes = read_u32(block, 12) as usize;
        if &block[..8] != b"EFI PART"
            || read_u32(block, 8) != 0x0001_0000
            || !(HEADER_BYTES..=BLOCK_BYTES).contains(&header_bytes)
            || read_u32(block, 20) != 0
            || read_u64(block, 24) != lba
            || read_u64(block, 32) != alternate
            || crc32::with_zeroed_word(&block[..header_bytes], 16) != read_u32(block, 16)
        {
            return None;
        }
        let first_usable = read_u64(block, 40);
        let last_usable = read_u64(block, 48);
        let table_lba = read_u64(block, 72);
        let count = read_u32(block, 80) as usize;
        let entry_bytes = read_u32(block, 84) as usize;
        if first_usable <= PRIMARY_LBA
            || first_usable > last_usable
            || last_usable >= last_lba
            || count == 0
            || entry_bytes < BASE_ENTRY_BYTES
            || !entry_bytes.is_power_of_two()
        {
            return None;
        }
        let table_bytes = count.checked_mul(entry_bytes)?;
        let reserved_blocks = (table_bytes.div_ceil(BLOCK_BYTES) as u64).max(MIN_TABLE_BLOCKS);
        if first_usable < 2 + reserved_blocks
            || last_usable.checked_add(reserved_blocks)? >= last_lba
        {
            return None;
        }
        let end = table_lba.checked_add(reserved_blocks)?;
        if lba == PRIMARY_LBA {
            if table_lba <= PRIMARY_LBA || end > first_usable {
                return None;
            }
        } else if table_lba <= last_usable || end > last_lba {
            return None;
        }

        Some(Self {
            table_lba,
            table_bytes,
            entry_bytes,
            table_crc: read_u32(block, 88),
            first_usable,
            last_usable,
        })
    }

    pub(crate) fn table(self, bytes: &[u8]) -> Option<Table<'_>> {
        let bytes = bytes.get(..self.table_bytes)?;
        if self.table_crc != crc32::crc32(bytes) {
            return None;
        }
        let table = Table {
            header: self,
            bytes,
        };
        for (slot, entry) in table.entries() {
            if entry.unique_id == NULL_GUID
                || entry.start < self.first_usable
                || entry.end > self.last_usable
                || entry.start > entry.end
            {
                return None;
            }
            if table
                .entries()
                .take_while(|&(previous, _)| previous < slot)
                .any(|(_, previous)| {
                    (entry.start <= previous.end && previous.start <= entry.end)
                        || entry.unique_id == previous.unique_id
                })
            {
                return None;
            }
        }
        Some(table)
    }
}

pub(crate) struct Table<'a> {
    header: Header,
    bytes: &'a [u8],
}

impl Table<'_> {
    /// Preserve GPT slot numbers when unused entries appear between partitions.
    pub(crate) fn entries(&self) -> impl Iterator<Item = (u32, Partition)> + '_ {
        self.bytes
            .chunks_exact(self.header.entry_bytes)
            .enumerate()
            .filter_map(|(index, entry)| {
                let partition_type = read_guid(entry, 0);
                if partition_type == NULL_GUID {
                    return None;
                }
                Some((
                    (index + 1) as u32,
                    Partition {
                        partition_type,
                        unique_id: read_guid(entry, 16),
                        start: read_u64(entry, 32),
                        end: read_u64(entry, 40),
                        attributes: read_u64(entry, 48),
                        name: core::array::from_fn(|index| read_u16(entry, 56 + index * 2)),
                    },
                ))
            })
    }
}

fn read_guid(bytes: &[u8], at: usize) -> Guid {
    Guid {
        data1: read_u32(bytes, at),
        data2: read_u16(bytes, at + 4),
        data3: read_u16(bytes, at + 6),
        data4: bytes[at + 8..at + 16].try_into().unwrap(),
    }
}

fn read_u16(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn read_u64(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAST: u64 = 255;
    const ESP_GUID_BYTES: [u8; 16] = [
        0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
    ];

    fn header(lba: u64, table: &[u8], count: u32) -> [u8; BLOCK_BYTES] {
        let (alternate, table_start) = if lba == 1 {
            (LAST, 2u64)
        } else {
            (1, LAST - 32)
        };
        let mut block = [0; BLOCK_BYTES];
        block[..8].copy_from_slice(b"EFI PART");
        block[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        block[12..16].copy_from_slice(&92u32.to_le_bytes());
        block[24..32].copy_from_slice(&lba.to_le_bytes());
        block[32..40].copy_from_slice(&alternate.to_le_bytes());
        block[40..48].copy_from_slice(&34u64.to_le_bytes());
        block[48..56].copy_from_slice(&(LAST - 33).to_le_bytes());
        block[56] = 1;
        block[72..80].copy_from_slice(&table_start.to_le_bytes());
        block[80..84].copy_from_slice(&count.to_le_bytes());
        block[84..88].copy_from_slice(&128u32.to_le_bytes());
        block[88..92].copy_from_slice(&crc32::crc32(table).to_le_bytes());
        seal(&mut block);
        block
    }

    fn seal(block: &mut [u8; BLOCK_BYTES]) {
        let crc = crc32::with_zeroed_word(&block[..HEADER_BYTES], 16);
        block[16..20].copy_from_slice(&crc.to_le_bytes());
    }

    #[test]
    fn protective_mbr_requires_a_covering_partition_and_signature() {
        let mut block = [0; BLOCK_BYTES];
        block[446] = 0x80; // The UEFI PMBR parser ignores BootIndicator.
        block[450] = 0xee;
        block[454..458].copy_from_slice(&1u32.to_le_bytes());
        block[458..462].copy_from_slice(&u32::MAX.to_le_bytes());
        block[510..].copy_from_slice(&[0x55, 0xaa]);
        assert!(protective_mbr(&block, u64::from(u32::MAX) + 1));
        assert!(!protective_mbr(&block, LAST));
        block[510] = 0;
        assert!(!protective_mbr(&block, u64::from(u32::MAX) + 1));
    }

    #[test]
    fn table_crc_covers_only_declared_bytes_and_keeps_the_slot_number() {
        let mut padded_table = [0u8; BLOCK_BYTES];
        let second = &mut padded_table[128..256];
        second[..16].copy_from_slice(&ESP_GUID_BYTES);
        second[16] = 7;
        second[32..40].copy_from_slice(&64u64.to_le_bytes());
        second[40..48].copy_from_slice(&90u64.to_le_bytes());
        second[56..58].copy_from_slice(&0x03a9u16.to_le_bytes());
        let primary = header(1, &padded_table[..256], 2);
        let decoded = Header::parse(&primary, 1, LAST).unwrap();
        padded_table[256..].fill(0x09);
        let table = decoded.table(&padded_table).unwrap();
        let mut entries = table.entries();
        let (slot, entry) = entries.next().unwrap();
        assert!(entries.next().is_none());
        assert_eq!(slot, 2);
        assert_eq!(entry.partition_type, SYSTEM_PARTITION);
        assert_eq!((entry.unique_id.data1, entry.start, entry.end), (7, 64, 90));
        assert_eq!(entry.name[0], 0x03a9);
        drop(entries);
        padded_table[128] ^= 1;
        assert!(decoded.table(&padded_table).is_none());
    }

    #[test]
    fn backup_has_its_own_crc_and_must_fit_before_the_last_header() {
        let table = [0u8; 16_384];
        let mut primary = header(1, &table, 128);
        let mut backup = header(LAST, &table, 128);
        primary[17] ^= 1;
        assert!(Header::parse(&primary, 1, LAST).is_none());
        assert!(
            Header::parse(&backup, LAST, LAST)
                .unwrap()
                .table(&table)
                .is_some()
        );
        backup[72..80].copy_from_slice(&(LAST - 31).to_le_bytes());
        seal(&mut backup);
        assert!(Header::parse(&backup, LAST, LAST).is_none());
    }

    #[test]
    fn either_header_must_leave_room_for_both_copies() {
        let table = [0u8; 16_384];
        let mut primary = header(1, &table, 128);
        primary[48..56].copy_from_slice(&(LAST - 1).to_le_bytes());
        seal(&mut primary);
        assert!(Header::parse(&primary, 1, LAST).is_none());
        let mut backup = header(LAST, &table, 128);
        backup[40..48].copy_from_slice(&2u64.to_le_bytes());
        seal(&mut backup);
        assert!(Header::parse(&backup, LAST, LAST).is_none());
    }
}
