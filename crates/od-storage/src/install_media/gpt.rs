use crate::crc32::crc32;

use super::ImportError;

pub(super) const PARTITION_START: u32 = 2048;
const TABLE_SECTORS: u32 = 32;
const TABLE_ENTRIES: u32 = 128;
const ENTRY_BYTES: u32 = 128;
const HEADER_BYTES: u32 = 92;
const SECTOR: u64 = 512;

/// Partition type and name written to the single GPT entry.
#[derive(Clone, Copy)]
pub(super) struct Partition {
    type_guid: [u8; 16],
    name: &'static str,
}

impl Partition {
    pub(super) fn type_guid(self) -> [u8; 16] {
        self.type_guid
    }
}

/// EFI System Partition, C12A7328-F81F-11D2-BA4B-00A0C93EC93B.
pub(super) const ESP: Partition = Partition {
    type_guid: [
        0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
    ],
    name: "OD Install Media",
};

/// Microsoft Basic Data, EBD0A0A2-B9E5-4433-87C0-68B6B72699C7; Windows assigns
/// a drive letter to such a volume, unlike to an ESP.
pub(super) const BASIC_DATA: Partition = Partition {
    type_guid: [
        0xa2, 0xa0, 0xd0, 0xeb, 0xe5, 0xb9, 0x33, 0x44, 0x87, 0xc0, 0x68, 0xb6, 0xb7, 0x26, 0x99,
        0xc7,
    ],
    name: "OD8 Payload",
};

pub(super) fn disk_sectors(partition_sectors: u32) -> Option<u32> {
    PARTITION_START
        .checked_add(partition_sectors)?
        .checked_add(TABLE_SECTORS + 1)
}

pub(super) fn emit(
    disk_sectors: u32,
    identity: u64,
    partition: Partition,
    mut put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
) -> Result<(), ImportError> {
    let last = disk_sectors - 1;
    let backup_table = last - TABLE_SECTORS;
    let mut mbr = [0; 512];
    let record = &mut mbr[446..462];
    record[1..4].copy_from_slice(&[0x00, 0x02, 0x00]);
    record[4..8].copy_from_slice(&[0xee, 0xff, 0xff, 0xff]);
    record[8..12].copy_from_slice(&1u32.to_le_bytes());
    record[12..16].copy_from_slice(&last.to_le_bytes());
    mbr[510..].copy_from_slice(&[0x55, 0xaa]);
    let mut entries = [0; (TABLE_ENTRIES * ENTRY_BYTES) as usize];
    entries[..16].copy_from_slice(&partition.type_guid);
    entries[16..32].copy_from_slice(&guid(identity, 2));
    entries[32..40].copy_from_slice(&u64::from(PARTITION_START).to_le_bytes());
    entries[40..48].copy_from_slice(&u64::from(backup_table - 1).to_le_bytes());
    for (field, unit) in entries[56..128]
        .chunks_exact_mut(2)
        .zip(partition.name.encode_utf16())
    {
        field.copy_from_slice(&unit.to_le_bytes());
    }
    let table_crc = crc32(&entries);
    put(0, &mbr)?;
    for (lba, alternate, table) in [(1, last, 2), (last, 1, backup_table)] {
        let mut header = [0; 512];
        header[..8].copy_from_slice(b"EFI PART");
        header[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        header[12..16].copy_from_slice(&HEADER_BYTES.to_le_bytes());
        header[24..32].copy_from_slice(&u64::from(lba).to_le_bytes());
        header[32..40].copy_from_slice(&u64::from(alternate).to_le_bytes());
        header[40..48].copy_from_slice(&u64::from(TABLE_SECTORS + 2).to_le_bytes());
        header[48..56].copy_from_slice(&u64::from(backup_table - 1).to_le_bytes());
        header[56..72].copy_from_slice(&guid(identity, 1));
        header[72..80].copy_from_slice(&u64::from(table).to_le_bytes());
        header[80..84].copy_from_slice(&TABLE_ENTRIES.to_le_bytes());
        header[84..88].copy_from_slice(&ENTRY_BYTES.to_le_bytes());
        header[88..92].copy_from_slice(&table_crc.to_le_bytes());
        let header_crc = crc32(&header[..HEADER_BYTES as usize]);
        header[16..20].copy_from_slice(&header_crc.to_le_bytes());
        put(u64::from(table) * SECTOR, &entries)?;
        put(u64::from(lba) * SECTOR, &header)?;
    }
    Ok(())
}

fn guid(identity: u64, kind: u8) -> [u8; 16] {
    let mut result = [0; 16];
    result[0] = kind;
    result[7] = 0x40;
    result[8..].copy_from_slice(&identity.to_be_bytes());
    result[8] = (result[8] & 0x3f) | 0x80;
    result
}
