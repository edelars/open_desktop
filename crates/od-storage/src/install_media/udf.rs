//! Read-only UDF 1.02 on a single physical partition (ECMA TR/112-7 and
//! ECMA-167, second edition, Parts 3 and 4).

use std::collections::BTreeSet;

use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;
use super::tree::{Contents, Entry, Extent, File, FileData, Name};

const BLOCK: usize = 2048;
const MAX_SEQUENCE_BLOCKS: u32 = 64;
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 65_536;
const MAX_EXTENTS: usize = 131_072;
const MAX_DIRECTORY_BYTES: u64 = 1024 * 1024;

const ANCHOR: u16 = 2;
const PARTITION: u16 = 5;
const LOGICAL_VOLUME: u16 = 6;
const TERMINATOR: u16 = 8;
const FILE_SET: u16 = 256;
const FILE_IDENTIFIER: u16 = 257;
const FILE_ENTRY: u16 = 261;
const DIRECTORY_TYPE: u8 = 4;
const REGULAR_TYPE: u8 = 5;
// Only short allocations and untransformed data. Multi-version and reserved
// flags are outside this profile as well (ECMA-167 Part 4 §14.6.8).
const UNSUPPORTED_ICB_FLAGS: u16 = 0xf807;

const OSTA_DOMAIN: &[u8] = b"\0*OSTA UDF Compliant\0\0\0\0";
const OSTA_CHARSET: &[u8] = b"\0OSTA Compressed Unicode";

pub(super) fn recognize<B: StorageBackend>(
    source: &CheckedStorage<B>,
) -> Result<bool, ImportError> {
    let blocks = source.size()? / BLOCK as u64;
    let mut block = [0; BLOCK];
    let mut extended = false;
    let mut nsr = false;
    for lba in 16..blocks.min(80) {
        source.read_exact_at(lba * BLOCK as u64, &mut block)?;
        let identifier = &block[1..6];
        if !extended && identifier == b"CD001" {
            continue;
        }
        if !extended && !matches!(identifier, b"BEA01" | b"NSR02" | b"NSR03" | b"TEA01") {
            return Ok(false);
        }
        if block[0] != 0 || block[6] != 1 {
            return Err(ImportError::Corrupt("udf_recognition"));
        }
        match (identifier, extended, nsr) {
            (b"BEA01", false, false) => extended = true,
            (b"NSR02", true, false) => nsr = true,
            (b"NSR03", true, false) => return Err(ImportError::Unsupported("udf_version")),
            (b"TEA01", true, true) => return Ok(true),
            _ => return Err(ImportError::Corrupt("udf_recognition")),
        }
    }
    if extended {
        Err(ImportError::Corrupt("udf_recognition"))
    } else {
        Ok(false)
    }
}

#[derive(Clone, Copy)]
struct Partition {
    number: u16,
    start: u32,
    blocks: u32,
}

impl Partition {
    fn extent(self, lbn: u32, length: u32) -> Result<Extent, ImportError> {
        if lbn >= self.blocks || length.div_ceil(BLOCK as u32) > self.blocks - lbn {
            return Err(ImportError::Corrupt("udf_partition_bounds"));
        }
        Ok(Extent {
            offset: (u64::from(self.start) + u64::from(lbn)) * BLOCK as u64,
            length: u64::from(length),
        })
    }
}

struct LogicalVolume {
    partition: u16,
    file_set: u32,
    file_set_bytes: u32,
}

struct Reader<'a, B: StorageBackend> {
    source: &'a CheckedStorage<B>,
    partition: Partition,
    directories: BTreeSet<u32>,
    entries_left: usize,
    extents_left: usize,
}

pub(super) fn read<B: StorageBackend>(
    source: &CheckedStorage<B>,
) -> Result<Vec<Entry>, ImportError> {
    let bytes = source.size()?;
    let blocks = u32::try_from(bytes / BLOCK as u64)
        .map_err(|_| ImportError::Unsupported("udf_volume_size"))?;
    if blocks <= 256 || !bytes.is_multiple_of(BLOCK as u64) {
        return Err(ImportError::Corrupt("udf_volume_size"));
    }

    let mut block = [0; BLOCK];
    source.read_exact_at(256 * BLOCK as u64, &mut block)?;
    let anchor = expect_tag(&block, 256, ANCHOR, 512)?;
    let sequence = le32(anchor, 20);
    let sequence_bytes = le32(anchor, 16);
    let sequence_blocks = sequence_bytes / BLOCK as u32;
    if sequence_blocks == 0
        || sequence_blocks > MAX_SEQUENCE_BLOCKS
        || !sequence_bytes.is_multiple_of(BLOCK as u32)
        || sequence
            .checked_add(sequence_blocks)
            .is_none_or(|end| end > blocks)
    {
        return Err(ImportError::Corrupt("udf_volume_sequence"));
    }
    let mut logical = None;
    let mut partition = None;
    let mut terminated = false;
    for lba in sequence..sequence + sequence_blocks {
        source.read_exact_at(u64::from(lba) * BLOCK as u64, &mut block)?;
        let (tag, data) = tagged(&block, lba)?;
        match tag {
            1 | 4 | 7 => {}
            PARTITION if partition.is_none() => {
                partition = Some(partition_descriptor(data, blocks)?)
            }
            LOGICAL_VOLUME if logical.is_none() => logical = Some(logical_descriptor(data)?),
            TERMINATOR => {
                terminated = true;
                break;
            }
            _ => return Err(ImportError::Unsupported("udf_volume_sequence")),
        }
    }
    let (Some(partition), Some(logical), true) = (partition, logical, terminated) else {
        return Err(ImportError::Corrupt("udf_volume_sequence"));
    };
    if partition.number != logical.partition {
        return Err(ImportError::Corrupt("udf_partition_map"));
    }
    let mut reader = Reader {
        source,
        partition,
        directories: BTreeSet::new(),
        entries_left: MAX_ENTRIES,
        extents_left: MAX_EXTENTS,
    };
    partition.extent(logical.file_set, logical.file_set_bytes)?;
    reader.read_block(logical.file_set, &mut block)?;
    let file_set = expect_tag(&block, logical.file_set, FILE_SET, 512)?;
    if !file_set[240..304].starts_with(OSTA_CHARSET)
        || file_set[240 + OSTA_CHARSET.len()..304]
            .iter()
            .any(|&byte| byte != 0)
    {
        return Err(ImportError::Unsupported("udf_character_set"));
    }
    if le32(file_set, 448) != 0 {
        return Err(ImportError::Unsupported("udf_multiple_file_sets"));
    }
    let root = icb(&file_set[400..416])?;
    if logical.file_set_bytes == 2 * BLOCK as u32 {
        let next = logical.file_set + 1;
        reader.read_block(next, &mut block)?;
        expect_tag(&block, next, TERMINATOR, 512)?;
    }
    reader.directory(root, root, 1)
}

fn partition_descriptor(data: &[u8], blocks: u32) -> Result<Partition, ImportError> {
    if data.len() < 512 {
        return Err(ImportError::Corrupt("udf_partition_descriptor"));
    }
    if &data[24..31] != b"\0+NSR02" || le32(data, 184) != 1 {
        return Err(ImportError::Unsupported("udf_partition_type"));
    }
    let start = le32(data, 188);
    let length = le32(data, 192);
    if length == 0 || start.checked_add(length).is_none_or(|end| end > blocks) {
        return Err(ImportError::Corrupt("udf_partition_bounds"));
    }
    Ok(Partition {
        number: le16(data, 22),
        start,
        blocks: length,
    })
}

fn logical_descriptor(data: &[u8]) -> Result<LogicalVolume, ImportError> {
    if data.len() < 446 {
        return Err(ImportError::Corrupt("udf_logical_volume_descriptor"));
    }
    if le32(data, 212) != BLOCK as u32
        || &data[216..240] != OSTA_DOMAIN
        || le16(data, 240) != 0x0102
    {
        return Err(ImportError::Unsupported("udf_logical_volume"));
    }
    if le32(data, 264) != 6 || le32(data, 268) != 1 || &data[440..444] != [1, 6, 1, 0] {
        return Err(ImportError::Unsupported("udf_partition_map"));
    }
    let file_set_bytes = le32(data, 248);
    if !matches!(file_set_bytes, 2048 | 4096) || le16(data, 256) != 0 {
        return Err(ImportError::Unsupported("udf_file_set_extent"));
    }
    Ok(LogicalVolume {
        partition: le16(data, 444),
        file_set: le32(data, 252),
        file_set_bytes,
    })
}

fn icb(data: &[u8]) -> Result<u32, ImportError> {
    if le32(data, 0) != BLOCK as u32 || le16(data, 8) != 0 {
        return Err(ImportError::Unsupported("udf_icb_extent"));
    }
    Ok(le32(data, 4))
}

impl<B: StorageBackend> Reader<'_, B> {
    fn read_block(&self, lbn: u32, block: &mut [u8; BLOCK]) -> Result<(), ImportError> {
        let extent = self.partition.extent(lbn, BLOCK as u32)?;
        Ok(self.source.read_exact_at(extent.offset, block)?)
    }

    fn file(&mut self, lbn: u32, kind: u8) -> Result<File, ImportError> {
        let mut block = [0; BLOCK];
        self.read_block(lbn, &mut block)?;
        let (tag, data) = tagged(&block, lbn)?;
        if tag != FILE_ENTRY || data.len() < 176 {
            return Err(ImportError::Unsupported("udf_file_entry"));
        }
        if le16(data, 20) != 4 || data[27] != kind || data[50] != 0 {
            return Err(ImportError::Unsupported("udf_file_entry_type"));
        }
        if le16(data, 34) & UNSUPPORTED_ICB_FLAGS != 0 {
            return Err(ImportError::Unsupported("udf_icb_flags"));
        }
        let length = le64(data, 56);
        let start = 176usize.checked_add(le32(data, 168) as usize);
        let descriptor_bytes = le32(data, 172) as usize;
        let start = start.ok_or(ImportError::Corrupt("udf_allocation_descriptors"))?;
        if !descriptor_bytes.is_multiple_of(8)
            || data.len().checked_sub(start) != Some(descriptor_bytes)
        {
            return Err(ImportError::Corrupt("udf_allocation_descriptors"));
        }
        let num_extents = descriptor_bytes / 8;
        self.extents_left = self
            .extents_left
            .checked_sub(num_extents)
            .ok_or(ImportError::Unsupported("udf_extent_limit"))?;
        let mut extents: Vec<Extent> = Vec::with_capacity(num_extents);
        let mut descriptors = data[start..].chunks_exact(8);
        while let Some(descriptor) = descriptors.next() {
            let extent_length = le32(descriptor, 0);
            if extent_length >> 30 != 0 {
                return Err(ImportError::Unsupported("udf_allocation_type"));
            }
            if extent_length == 0 {
                if le32(descriptor, 4) != 0
                    || descriptors.any(|rest| rest.iter().any(|&byte| byte != 0))
                {
                    return Err(ImportError::Corrupt("udf_allocation_length"));
                }
                break;
            }
            if extents
                .last()
                .is_some_and(|extent| !extent.length.is_multiple_of(BLOCK as u64))
            {
                return Err(ImportError::Corrupt("udf_allocation_length"));
            }
            extents.push(self.partition.extent(le32(descriptor, 4), extent_length)?);
        }
        File::new(length, extents)
    }

    fn directory(
        &mut self,
        lbn: u32,
        parent: u32,
        depth: usize,
    ) -> Result<Vec<Entry>, ImportError> {
        if depth > MAX_DEPTH {
            return Err(ImportError::Unsupported("udf_depth"));
        }
        if !self.directories.insert(lbn) {
            return Err(ImportError::Corrupt("udf_directory_cycle"));
        }
        let file = self.file(lbn, DIRECTORY_TYPE)?;
        if file.length == 0 || file.length > MAX_DIRECTORY_BYTES {
            return Err(ImportError::Unsupported("udf_directory_size"));
        }
        let mut data = vec![0; file.length as usize];
        file.read_exact_at(self.source, 0, &mut data)?;
        let mut position = 0;
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        while position < data.len() {
            let tail = &data[position..];
            if tail.len() < 38 {
                return Err(ImportError::Corrupt("udf_file_identifier"));
            }
            let name_bytes = usize::from(tail[19]);
            let name_start = 38 + usize::from(le16(tail, 36));
            let name_end = name_start + name_bytes;
            let length = name_end.next_multiple_of(4);
            let record = tail
                .get(..length)
                .ok_or(ImportError::Corrupt("udf_file_identifier"))?;
            let (physical, _) = file
                .locate(position as u64)
                .ok_or(ImportError::Corrupt("udf_dir_extent"))?;
            let tag_location = (physical / BLOCK as u64) as u32 - self.partition.start;
            let record = expect_tag(record, tag_location, FILE_IDENTIFIER, length)?;
            if le16(record, 16) != 1 || record[name_end..].iter().any(|&byte| byte != 0) {
                return Err(ImportError::Unsupported("udf_file_identifier_format"));
            }
            let child = icb(&record[20..36])?;
            let properties = record[18];
            let first = position == 0;
            position += length;
            if first {
                if properties != 0x0a || name_bytes != 0 || child != parent {
                    return Err(ImportError::Corrupt("udf_directory_parent"));
                }
                continue;
            }
            if !matches!(properties, 0 | 2) {
                return Err(ImportError::Unsupported("udf_file_identifier_flags"));
            }
            let name = decode_name(&record[name_start..name_end])?;
            if !names.insert(name.to_ascii_uppercase()) {
                return Err(ImportError::Unsupported("udf_fat_name_collision"));
            }
            self.entries_left = self
                .entries_left
                .checked_sub(1)
                .ok_or(ImportError::Unsupported("udf_entry_limit"))?;
            let contents = if properties == 2 {
                Contents::Directory(self.directory(child, lbn, depth + 1)?)
            } else {
                Contents::File(FileData::Source(self.file(child, REGULAR_TYPE)?))
            };
            entries.push(Entry {
                name: Name::Long(name),
                contents,
            });
        }
        Ok(entries)
    }
}

fn decode_name(bytes: &[u8]) -> Result<String, ImportError> {
    let Some((&compression, value)) = bytes.split_first() else {
        return Err(ImportError::Corrupt("udf_name"));
    };
    let octets = match compression {
        8 => value.to_vec(),
        16 if value.len().is_multiple_of(2) && value.chunks_exact(2).all(|pair| pair[0] == 0) => {
            value.chunks_exact(2).map(|pair| pair[1]).collect()
        }
        _ => return Err(ImportError::Unsupported("udf_name_encoding")),
    };
    if octets.is_empty()
        || matches!(octets.last(), Some(b'.' | b' '))
        || octets
            .iter()
            .any(|&byte| !(0x20..=0x7e).contains(&byte) || b"\"*/:<>?\\|".contains(&byte))
    {
        return Err(ImportError::Unsupported("udf_fat_name"));
    }
    String::from_utf8(octets).map_err(|_| ImportError::Corrupt("udf_name"))
}

fn expect_tag(
    data: &[u8],
    location: u32,
    expected: u16,
    minimum: usize,
) -> Result<&[u8], ImportError> {
    let (tag, data) = tagged(data, location)?;
    if tag != expected || data.len() < minimum {
        return Err(ImportError::Corrupt("udf_descriptor_type"));
    }
    Ok(data)
}

fn tagged(data: &[u8], location: u32) -> Result<(u16, &[u8]), ImportError> {
    if data.len() < 16 || le16(data, 2) != 2 || le32(data, 12) != location {
        return Err(ImportError::Corrupt("udf_descriptor_tag"));
    }
    let checksum = data[..16]
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != 4)
        .fold(0u8, |sum, (_, &byte)| sum.wrapping_add(byte));
    let length = 16 + usize::from(le16(data, 10));
    let checked = data
        .get(..length)
        .ok_or(ImportError::Corrupt("udf_descriptor_length"))?;
    if checksum != data[4] || crc16(&checked[16..]) != le16(data, 8) {
        return Err(ImportError::Corrupt("udf_descriptor_checksum"));
    }
    Ok((le16(data, 0), checked))
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0u16;
    for &byte in bytes {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

fn le16(data: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(data[at..at + 2].try_into().expect("UDF field bounds"))
}

fn le32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().expect("UDF field bounds"))
}

fn le64(data: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(data[at..at + 8].try_into().expect("UDF field bounds"))
}
