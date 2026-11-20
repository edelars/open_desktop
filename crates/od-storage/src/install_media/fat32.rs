use std::collections::{BTreeMap, BTreeSet};

use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;
use super::gpt::PARTITION_START;
use super::tree::{Contents, Entry, File, Name};

const SECTOR: u32 = 512;
const RESERVED: u32 = 32;
const FATS: u32 = 2;
const FAT_COUNT_PER_SECTOR: u32 = SECTOR / 4;
const FAT_EOF: u32 = 0x0fff_ffff;
const MIN_CLUSTERS: u32 = 65_525;
const MAX_CLUSTERS: u32 = 0x0fff_ffed;
const ENTRY_BYTES: usize = 32;
const LFN_CHARS: usize = 13;
const DATA_CHUNK: usize = 128 * 1024;

enum Data {
    Directory(Vec<u8>),
    File(File),
}

struct Allocation {
    first: u32,
    count: u32,
    data: Data,
}

struct Allocator {
    allocations: Vec<Allocation>,
    next: u32,
    cluster_bytes: u32,
    files: u32,
}

pub(super) struct Volume {
    allocations: Vec<Allocation>,
    next: u32,
    cluster_sectors: u32,
    cluster_count: u32,
    fat_sectors: u32,
    partition_sectors: u32,
    pub(super) files: u32,
}

fn full() -> ImportError {
    ImportError::Unsupported("fat_capacity")
}

impl Volume {
    pub(super) fn plan(input: Vec<Entry>, input_bytes: u64) -> Result<Self, ImportError> {
        let cluster_sectors = if input_bytes < 64 * 1024 * 1024 { 1 } else { 8 };
        let mut allocator = Allocator {
            allocations: Vec::new(),
            next: 2,
            cluster_bytes: cluster_sectors * SECTOR,
            files: 0,
        };
        allocator.directory(input, None)?;
        allocator.allocations.sort_by_key(|row| row.first);
        let cluster_count = MIN_CLUSTERS.max(allocator.next - 2 + 16);
        if cluster_count > MAX_CLUSTERS {
            return Err(full());
        }
        let fat_sectors = (cluster_count + 2).div_ceil(FAT_COUNT_PER_SECTOR);
        let partition_sectors = cluster_count
            .checked_mul(cluster_sectors)
            .and_then(|count| count.checked_add(RESERVED + FATS * fat_sectors))
            .ok_or_else(full)?;
        Ok(Self {
            allocations: allocator.allocations,
            next: allocator.next,
            cluster_sectors,
            cluster_count,
            fat_sectors,
            partition_sectors,
            files: allocator.files,
        })
    }

    pub(super) fn sectors(&self) -> u32 {
        self.partition_sectors
    }

    pub(super) fn emit<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        identity: u64,
        mut put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        let base = disk_offset(PARTITION_START);
        let boot = self.boot_sector(identity);
        let mut fsinfo = [0; SECTOR as usize];
        fsinfo[..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
        fsinfo[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
        fsinfo[488..492].copy_from_slice(&(self.cluster_count - (self.next - 2)).to_le_bytes());
        fsinfo[492..496].copy_from_slice(&self.next.to_le_bytes());
        fsinfo[508..].copy_from_slice(&0xaa55_0000u32.to_le_bytes());
        for sector in [0, 6] {
            put(base + disk_offset(sector), &boot)?;
            put(base + disk_offset(sector + 1), &fsinfo)?;
        }
        let mut allocations = self.allocations.iter().peekable();
        let fat_start = PARTITION_START + RESERVED;
        let mut fat = [0; SECTOR as usize];
        for sector in 0..self.fat_sectors {
            for (index, entry) in fat.chunks_exact_mut(4).enumerate() {
                let cluster = sector * FAT_COUNT_PER_SECTOR + index as u32;
                if allocations
                    .peek()
                    .is_some_and(|run| cluster >= run.first + run.count)
                {
                    allocations.next();
                }
                let value = match cluster {
                    0 => 0x0fff_fff8,
                    1 => FAT_EOF,
                    _ => match allocations.peek() {
                        Some(run) if cluster == run.first + run.count - 1 => FAT_EOF,
                        Some(run) if cluster >= run.first => cluster + 1,
                        _ => 0,
                    },
                };
                entry.copy_from_slice(&value.to_le_bytes());
            }
            put(disk_offset(fat_start + sector), &fat)?;
            put(disk_offset(fat_start + self.fat_sectors + sector), &fat)?;
        }

        let data_base = disk_offset(fat_start + FATS * self.fat_sectors);
        let cluster_bytes = disk_offset(self.cluster_sectors);
        let mut buf = vec![0; DATA_CHUNK];
        for run in &self.allocations {
            let out = data_base + u64::from(run.first - 2) * cluster_bytes;
            match &run.data {
                Data::Directory(bytes) => put(out, bytes)?,
                Data::File(file) => {
                    let len = file.length;
                    for at in (0..len).step_by(DATA_CHUNK) {
                        let count = (len - at).min(DATA_CHUNK as u64) as usize;
                        file.read_exact_at(source, at, &mut buf[..count])?;
                        put(out + at, &buf[..count])?;
                    }
                }
            }
        }
        Ok(())
    }

    fn boot_sector(&self, identity: u64) -> [u8; SECTOR as usize] {
        let mut boot = [0; SECTOR as usize];
        boot[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
        boot[3..11].copy_from_slice(b"ODSETUP ");
        boot[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
        boot[13] = self.cluster_sectors as u8;
        boot[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
        boot[16] = FATS as u8;
        boot[21] = 0xf8;
        boot[24..26].copy_from_slice(&63u16.to_le_bytes());
        boot[26..28].copy_from_slice(&255u16.to_le_bytes());
        boot[28..32].copy_from_slice(&PARTITION_START.to_le_bytes());
        boot[32..36].copy_from_slice(&self.partition_sectors.to_le_bytes());
        boot[36..40].copy_from_slice(&self.fat_sectors.to_le_bytes());
        boot[44..48].copy_from_slice(&2u32.to_le_bytes());
        boot[48..50].copy_from_slice(&1u16.to_le_bytes());
        boot[50..52].copy_from_slice(&6u16.to_le_bytes());
        boot[64] = 0x80;
        boot[66] = 0x29;
        boot[67..71].copy_from_slice(&(identity as u32).to_le_bytes());
        boot[71..82].copy_from_slice(b"OD INSTALL ");
        boot[82..90].copy_from_slice(b"FAT32   ");
        boot[510..].copy_from_slice(&[0x55, 0xaa]);
        boot
    }
}

impl Allocator {
    fn reserve(&mut self, bytes: u64) -> Result<(u32, u32), ImportError> {
        let count =
            u32::try_from(bytes.div_ceil(u64::from(self.cluster_bytes))).map_err(|_| full())?;
        let first = self.next;
        self.next = first
            .checked_add(count)
            .filter(|&next| next <= MAX_CLUSTERS + 2)
            .ok_or_else(full)?;
        Ok((first, count))
    }

    fn directory(&mut self, entries: Vec<Entry>, parent: Option<u32>) -> Result<u32, ImportError> {
        let mut occupied = BTreeSet::new();
        for entry in &entries {
            let short = match &entry.name {
                Name::Short(short) => Some(*short),
                Name::Long(long) => canonical_short(long),
            };
            if short.is_some_and(|short| !occupied.insert(short)) {
                return Err(ImportError::Unsupported("fat_name_collision"));
            }
        }
        let mut alias_cursors = BTreeMap::new();
        let entries = entries
            .into_iter()
            .map(|entry| {
                let (short, long) = match entry.name {
                    Name::Short(short) => (short, None),
                    Name::Long(long) => {
                        let canonical = canonical_short(&long);
                        let short = match canonical {
                            Some(short) => short,
                            None => allocate_alias(&long, &mut occupied, &mut alias_cursors)?,
                        };
                        let needs_lfn =
                            canonical.is_none() || long.bytes().any(|b| b.is_ascii_lowercase());
                        (short, needs_lfn.then_some(long))
                    }
                };
                Ok((short, long, entry.contents))
            })
            .collect::<Result<Vec<_>, ImportError>>()?;
        let slots: usize = entries
            .iter()
            .map(|(_, long, _)| {
                1 + long
                    .as_ref()
                    .map_or(0, |name| name.len().div_ceil(LFN_CHARS))
            })
            .sum();
        let prefix_entries = if parent.is_some() { 2 } else { 1 };
        let bytes = (prefix_entries + slots + 1) * ENTRY_BYTES;
        let (first, count) = self.reserve(bytes as u64)?;
        let mut directory = Vec::with_capacity(bytes);
        if let Some(parent) = parent {
            directory.extend_from_slice(&dir_entry(b".          ", 0x10, first, 0));
            directory.extend_from_slice(&dir_entry(b"..         ", 0x10, parent, 0));
        } else {
            directory.extend_from_slice(&dir_entry(b"OD INSTALL ", 0x08, 0, 0));
        }
        for (short, long, contents) in entries {
            let (attributes, cluster, len) = match contents {
                Contents::Directory(children) => {
                    let parent_cluster = if parent.is_none() { 0 } else { first };
                    (0x10, self.directory(children, Some(parent_cluster))?, 0)
                }
                Contents::File(file) => {
                    self.files += 1;
                    let len = u32::try_from(file.length)
                        .map_err(|_| ImportError::Unsupported("fat_file_size"))?;
                    let cluster = if len == 0 {
                        0
                    } else {
                        let (first, count) = self.reserve(u64::from(len))?;
                        self.allocations.push(Allocation {
                            first,
                            count,
                            data: Data::File(file),
                        });
                        first
                    };
                    (0x20, cluster, len)
                }
            };
            if let Some(long) = long {
                write_long_name(&mut directory, long.as_bytes(), &short);
            }
            directory.extend_from_slice(&dir_entry(&short, attributes, cluster, len));
        }
        directory.resize(bytes, 0);
        self.allocations.push(Allocation {
            first,
            count,
            data: Data::Directory(directory),
        });
        Ok(first)
    }
}

fn short_character(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"$%'-_@~`!(){}^#&".contains(&byte)
}

fn split_name(name: &str) -> (&str, &str) {
    name.rsplit_once('.').unwrap_or((name, ""))
}

fn canonical_short(name: &str) -> Option<[u8; 11]> {
    let (base, extension) = split_name(name);
    if base.is_empty()
        || base.len() > 8
        || extension.len() > 3
        || !base.bytes().chain(extension.bytes()).all(short_character)
    {
        return None;
    }
    let mut out = [b' '; 11];
    for (dest, src) in out[..8].iter_mut().zip(base.bytes()) {
        *dest = src.to_ascii_uppercase();
    }
    for (dest, src) in out[8..].iter_mut().zip(extension.bytes()) {
        *dest = src.to_ascii_uppercase();
    }
    Some(out)
}

fn allocate_alias(
    name: &str,
    occupied: &mut BTreeSet<[u8; 11]>,
    cursors: &mut BTreeMap<[u8; 11], u32>,
) -> Result<[u8; 11], ImportError> {
    let (base, extension) = split_name(name);
    let mut stem: Vec<u8> = base
        .bytes()
        .filter(|&byte| short_character(byte))
        .map(|byte| byte.to_ascii_uppercase())
        .take(6)
        .collect();
    if stem.is_empty() {
        stem.extend_from_slice(b"FILE");
    }
    let mut out = [b' '; 11];
    for (dest, src) in out[8..]
        .iter_mut()
        .zip(extension.bytes().filter(|&byte| short_character(byte)))
    {
        *dest = src.to_ascii_uppercase();
    }
    out[..stem.len()].copy_from_slice(&stem);
    let cursor = cursors.entry(out).or_insert(1);
    for index in *cursor..=999_999 {
        let suffix = index.to_string();
        let prefix = stem.len().min(7 - suffix.len());
        out[..8].fill(b' ');
        out[..prefix].copy_from_slice(&stem[..prefix]);
        out[prefix] = b'~';
        out[prefix + 1..prefix + 1 + suffix.len()].copy_from_slice(suffix.as_bytes());
        if occupied.insert(out) {
            *cursor = index + 1;
            return Ok(out);
        }
    }
    Err(ImportError::Unsupported("fat_alias_limit"))
}

fn write_long_name(directory: &mut Vec<u8>, name: &[u8], short: &[u8; 11]) {
    const CHAR_OFFSETS: [usize; LFN_CHARS] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let checksum = short
        .iter()
        .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
    let slots = name.len().div_ceil(LFN_CHARS);
    for (index, characters) in name.chunks(LFN_CHARS).enumerate().rev() {
        let mut entry = [0; ENTRY_BYTES];
        entry[0] = (index + 1) as u8;
        if index + 1 == slots {
            entry[0] |= 0x40;
        }
        entry[11] = 0x0f;
        entry[13] = checksum;
        for (index, offset) in CHAR_OFFSETS.into_iter().enumerate() {
            let code = match characters.get(index) {
                Some(&character) => u16::from(character),
                None if index == characters.len() => 0,
                None => 0xffff,
            };
            entry[offset..offset + 2].copy_from_slice(&code.to_le_bytes());
        }
        directory.extend_from_slice(&entry);
    }
}

fn dir_entry(name: &[u8; 11], attributes: u8, cluster: u32, len: u32) -> [u8; ENTRY_BYTES] {
    let mut row = [0; ENTRY_BYTES];
    row[..11].copy_from_slice(name);
    row[11] = attributes;
    // The ISO record does not supply a FAT creation time.
    row[24..26].copy_from_slice(&0x0021u16.to_le_bytes());
    row[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
    row[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
    row[28..32].copy_from_slice(&len.to_le_bytes());
    row
}

fn disk_offset(sectors: u32) -> u64 {
    u64::from(sectors) * u64::from(SECTOR)
}
