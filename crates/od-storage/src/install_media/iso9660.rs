use std::collections::BTreeSet;

use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;

const SECTOR: u64 = 2048;
const FIRST_DESCRIPTOR: u64 = 16;
const MAX_DESCRIPTORS: u64 = 64;
const MAX_DEPTH: usize = 8;
const MAX_ENTRIES: usize = 65_536;
const MIN_RECORD: usize = 34;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Extent {
    lba: u32,
    pub(super) length: u32,
}

impl Extent {
    pub(super) fn offset(self) -> u64 {
        u64::from(self.lba) * SECTOR
    }
}

pub(super) enum Contents {
    Directory(Vec<Entry>),
    File(Extent),
}

pub(super) struct Entry {
    pub(super) name: [u8; 11],
    pub(super) contents: Contents,
}

struct Record<'a> {
    extent: Extent,
    directory: bool,
    name: &'a [u8],
}

struct Reader<'a, B: StorageBackend> {
    source: &'a CheckedStorage<B>,
    volume_blocks: u32,
    directories: BTreeSet<u32>,
    entries_left: usize,
}

pub(super) fn read<B: StorageBackend>(
    source: &CheckedStorage<B>,
) -> Result<Vec<Entry>, ImportError> {
    let source_blocks = source.size()? / SECTOR;
    let mut sector = [0; SECTOR as usize];
    let mut primary = None;
    let mut terminated = false;
    for lba in FIRST_DESCRIPTOR..source_blocks.min(FIRST_DESCRIPTOR + MAX_DESCRIPTORS) {
        source.read_exact_at(lba * SECTOR, &mut sector)?;
        let identifier = &sector[1..6];
        if matches!(identifier, b"BEA01" | b"NSR02" | b"NSR03") {
            return Err(ImportError::Unsupported("udf"));
        }
        if terminated {
            break;
        }
        if identifier != b"CD001" || sector[6] != 1 {
            return Err(ImportError::Unsupported("iso_descriptor_format"));
        }
        match sector[0] {
            0 => {} // Boot record. Preserve the files in the primary directory tree.
            1 if primary.is_none() => {
                if sector[881] != 1 {
                    return Err(ImportError::Unsupported("iso_file_structure_version"));
                }
                let blocks = both_u32(&sector[80..88])?;
                if u64::from(blocks) > source_blocks || u64::from(blocks) <= lba {
                    return Err(ImportError::Corrupt("iso_volume_bounds"));
                }
                if both_u16(&sector[120..124])? != 1
                    || both_u16(&sector[124..128])? != 1
                    || u64::from(both_u16(&sector[128..132])?) != SECTOR
                {
                    return Err(ImportError::Unsupported("iso_volume_geometry"));
                }
                let root = record(&sector[156..190], blocks)?;
                if !root.directory || root.name != b"\0" {
                    return Err(ImportError::Corrupt("iso_root"));
                }
                primary = Some((blocks, root.extent));
            }
            255 => terminated = true,
            _ => return Err(ImportError::Unsupported("iso_descriptor_type")),
        }
    }
    let Some((volume_blocks, root)) = primary.filter(|_| terminated) else {
        return Err(ImportError::Corrupt("iso_descriptor_sequence"));
    };
    let mut reader = Reader {
        source,
        volume_blocks,
        directories: BTreeSet::new(),
        entries_left: MAX_ENTRIES,
    };
    reader.directory(root, root, 1)
}

impl<B: StorageBackend> Reader<'_, B> {
    fn directory(
        &mut self,
        extent: Extent,
        parent: Extent,
        depth: usize,
    ) -> Result<Vec<Entry>, ImportError> {
        if depth > MAX_DEPTH {
            return Err(ImportError::Unsupported("iso_depth"));
        }
        if !self.directories.insert(extent.lba)
            || extent.length == 0
            || !u64::from(extent.length).is_multiple_of(SECTOR)
        {
            return Err(ImportError::Corrupt("iso_directory_extent"));
        }
        let mut data = [0; SECTOR as usize];
        let mut entries = Vec::new();
        let mut names = BTreeSet::new();
        let mut dot_entries = 0;
        for block in 0..u64::from(extent.length) / SECTOR {
            self.source
                .read_exact_at(extent.offset() + block * SECTOR, &mut data)?;
            let mut rest = data.as_slice();
            while let Some((&length, tail)) = rest.split_first() {
                if length == 0 {
                    if tail.iter().any(|&byte| byte != 0) {
                        return Err(ImportError::Corrupt("iso_directory_padding"));
                    }
                    break;
                }
                let at = usize::from(length);
                let raw = rest
                    .get(..at)
                    .ok_or(ImportError::Corrupt("iso_record_length"))?;
                let child = record(raw, self.volume_blocks)?;
                rest = &rest[at..];
                if dot_entries < 2 {
                    let dot_extent = if dot_entries == 0 { extent } else { parent };
                    if !child.directory || child.name != [dot_entries] || child.extent != dot_extent
                    {
                        return Err(ImportError::Corrupt("iso_directory_links"));
                    }
                    dot_entries += 1;
                    continue;
                }
                let name = short_name(child.name, child.directory)?;
                if !names.insert(name) {
                    return Err(ImportError::Unsupported("iso_fat_name_collision"));
                }
                self.entries_left = self
                    .entries_left
                    .checked_sub(1)
                    .ok_or(ImportError::Unsupported("iso_entry_limit"))?;
                let contents = if child.directory {
                    Contents::Directory(self.directory(child.extent, extent, depth + 1)?)
                } else {
                    Contents::File(child.extent)
                };
                entries.push(Entry { name, contents });
            }
        }
        if dot_entries != 2 {
            return Err(ImportError::Corrupt("iso_directory_links"));
        }
        Ok(entries)
    }
}

fn record(data: &[u8], volume_blocks: u32) -> Result<Record<'_>, ImportError> {
    if data.len() < MIN_RECORD || usize::from(data[0]) != data.len() || data.len() % 2 != 0 {
        return Err(ImportError::Corrupt("iso_record_length"));
    }
    let name_len = usize::from(data[32]);
    let name_end = 33 + name_len;
    let padded_end = name_end + usize::from(name_len.is_multiple_of(2));
    if name_len == 0 || padded_end > data.len() {
        return Err(ImportError::Corrupt("iso_identifier_length"));
    }
    if data[1] != 0
        || !matches!(data[25], 0 | 2)
        || data[26..28] != [0, 0]
        || both_u16(&data[28..32])? != 1
        || data[name_end..].iter().any(|&byte| byte != 0)
    {
        return Err(ImportError::Unsupported("iso_record_features"));
    }
    let extent = Extent {
        lba: both_u32(&data[2..10])?,
        length: both_u32(&data[10..18])?,
    };
    let end = extent.offset() + u64::from(extent.length);
    if end > u64::from(volume_blocks) * SECTOR || extent.length != 0 && extent.lba < 16 {
        return Err(ImportError::Corrupt("iso_extent_bounds"));
    }
    Ok(Record {
        extent,
        directory: data[25] == 2,
        name: &data[33..name_end],
    })
}

fn short_name(name: &[u8], directory: bool) -> Result<[u8; 11], ImportError> {
    let bad = || ImportError::Unsupported("iso_fat_short_name");
    let identifier = std::str::from_utf8(name).map_err(|_| bad())?;
    let (stem, extension) = if directory {
        (identifier, "")
    } else {
        let (file, version) = identifier.rsplit_once(';').ok_or_else(bad)?;
        if !version.bytes().all(|b| b.is_ascii_digit())
            || !version
                .parse::<u16>()
                .is_ok_and(|n| (1..=32767).contains(&n))
        {
            return Err(bad());
        }
        file.split_once('.').unwrap_or((file, ""))
    };
    if !(1..=8).contains(&stem.len())
        || extension.len() > 3
        || !stem
            .bytes()
            .chain(extension.bytes())
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(bad());
    }
    let mut result = [b' '; 11];
    result[..stem.len()].copy_from_slice(stem.as_bytes());
    result[8..8 + extension.len()].copy_from_slice(extension.as_bytes());
    Ok(result)
}

fn both_u16(data: &[u8]) -> Result<u16, ImportError> {
    let value = u16::from_le_bytes([data[0], data[1]]);
    if value != u16::from_be_bytes([data[2], data[3]]) {
        return Err(ImportError::Corrupt("iso_byte_order_fields"));
    }
    Ok(value)
}

fn both_u32(data: &[u8]) -> Result<u32, ImportError> {
    let value = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if value != u32::from_be_bytes([data[4], data[5], data[6], data[7]]) {
        return Err(ImportError::Corrupt("iso_byte_order_fields"));
    }
    Ok(value)
}
