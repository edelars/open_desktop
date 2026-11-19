//! Fixed-root and cluster-chain directories with long-name assembly.

use crate::uefi::status::Status;

use super::{
    ReadAt, Volume,
    bpb::{FatType, Root},
    lfn::LongName,
    name::{Name, ShortName, equivalent},
    table::Chain,
};

const ENTRY_BYTES: usize = 32;
const SECTOR_BYTES: usize = 512;
pub(crate) const DIRECTORY: u8 = 0x10;
const LABEL: u8 = 8;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Directory {
    Root,
    Cluster(u32),
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub(crate) name: Name,
    pub(crate) attributes: u8,
    pub(crate) len: u32,
    pub(crate) times: Times,
    pub(super) cluster: u32,
    short: ShortName,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct Times {
    pub(crate) created_date: u16,
    pub(crate) created_time: u16,
    pub(crate) created_centiseconds: u8,
    pub(crate) accessed_date: u16,
    pub(crate) modified_date: u16,
    pub(crate) modified_time: u16,
}

impl Entry {
    pub(crate) fn root() -> Self {
        Self {
            name: Name::from_units(&[]),
            short: ShortName::parse(b"           ", 0),
            attributes: DIRECTORY,
            len: 0,
            times: Times::default(),
            cluster: 0,
        }
    }

    pub(crate) fn is_directory(&self) -> bool {
        self.attributes & DIRECTORY != 0
    }

    pub(crate) fn is_label(&self) -> bool {
        self.attributes & LABEL != 0
    }

    fn is_parent(&self) -> bool {
        self.short.as_slice() == [b'.' as u16, b'.' as u16]
    }

    fn matches(&self, name: &[u16]) -> bool {
        equivalent(self.name.as_slice(), name) || equivalent(self.short.as_slice(), name)
    }

    fn decode(volume: &Volume, record: &[u8; ENTRY_BYTES], assembly: &mut LongName) -> Self {
        let bytes = record[..11].try_into().unwrap();
        let short = ShortName::parse(bytes, record[12]);
        let long = assembly.take(bytes);
        let attributes = record[11];
        let name = if attributes & LABEL != 0 {
            Name::label(bytes)
        } else {
            Name::from_units(long.unwrap_or_else(|| short.as_slice()))
        };
        let low = u32::from(word(record, 26));
        let cluster = if volume.geometry.fat_type == FatType::Fat32 {
            low | (u32::from(word(record, 20) & 0x0fff) << 16)
        } else {
            low
        };
        let len = u32::from_le_bytes(record[28..].try_into().unwrap());
        Self {
            name,
            short,
            attributes,
            len,
            cluster,
            times: Times {
                created_centiseconds: record[13],
                created_time: word(record, 14),
                created_date: word(record, 16),
                accessed_date: word(record, 18),
                modified_time: word(record, 22),
                modified_date: word(record, 24),
            },
        }
    }
}

fn word(record: &[u8; ENTRY_BYTES], at: usize) -> u16 {
    u16::from_le_bytes([record[at], record[at + 1]])
}

enum Storage {
    Fixed { start: u64, len: u64 },
    Chain { chain: Chain, within: u32 },
}

pub(crate) struct DirectoryCursor {
    storage: Option<Storage>,
    offset: u64,
    cached_sector: Option<u64>,
    scratch: [u8; SECTOR_BYTES],
    long_name: LongName,
}

impl DirectoryCursor {
    pub(crate) fn new(
        volume: &Volume,
        source: &mut impl ReadAt,
        directory: Directory,
        offset: u64,
    ) -> Result<Self, Status> {
        if !offset.is_multiple_of(ENTRY_BYTES as u64) {
            return Err(Status::INVALID_PARAMETER);
        }
        let storage = match (directory, volume.geometry.root) {
            (
                Directory::Root,
                Root::Fixed {
                    offset: start,
                    entries,
                },
            ) => Some(Storage::Fixed {
                start,
                len: u64::from(entries) * ENTRY_BYTES as u64,
            }),
            (Directory::Root, Root::Cluster(first)) | (Directory::Cluster(first), _) => {
                let geometry = volume.geometry;
                let mut chain = Chain::new(geometry, first)?;
                let cluster_bytes = u64::from(geometry.cluster_bytes);
                let mut active = true;
                for _ in 0..offset / cluster_bytes {
                    if !chain.advance(geometry, source)? {
                        active = false;
                        break;
                    }
                }
                active.then_some(Storage::Chain {
                    chain,
                    within: (offset % cluster_bytes) as u32,
                })
            }
        };
        Ok(Self {
            storage,
            offset,
            cached_sector: None,
            scratch: [0; SECTOR_BYTES],
            long_name: LongName::new(),
        })
    }

    pub(crate) fn position(&self) -> u64 {
        self.offset
    }

    pub(crate) fn next(
        &mut self,
        volume: &Volume,
        source: &mut impl ReadAt,
    ) -> Result<Option<Entry>, Status> {
        loop {
            let Some(record) = self.record(volume, source)? else {
                return Ok(None);
            };
            match record[0] {
                0 => {
                    self.storage = None;
                    return Ok(None);
                }
                0xe5 => self.long_name.reset(),
                _ if record[11] == 0x0f => self.long_name.push(&record),
                _ => return Ok(Some(Entry::decode(volume, &record, &mut self.long_name))),
            }
        }
    }

    fn record(
        &mut self,
        volume: &Volume,
        source: &mut impl ReadAt,
    ) -> Result<Option<[u8; ENTRY_BYTES]>, Status> {
        let Some(storage) = &mut self.storage else {
            return Ok(None);
        };
        let address = match storage {
            Storage::Fixed { start, len } => {
                if self.offset >= *len {
                    return Ok(None);
                }
                *start + self.offset
            }
            Storage::Chain { chain, within } => {
                let geometry = volume.geometry;
                if *within == geometry.cluster_bytes {
                    if !chain.advance(geometry, source)? {
                        return Ok(None);
                    }
                    *within = 0;
                }
                let base = geometry
                    .cluster_offset(chain.current())
                    .ok_or(Status::VOLUME_CORRUPTED)?;
                base + u64::from(*within)
            }
        };
        let sector = address & !(SECTOR_BYTES as u64 - 1);
        if self.cached_sector != Some(sector) {
            source.read_at(sector, &mut self.scratch)?;
            self.cached_sector = Some(sector);
        }
        let at = (address - sector) as usize;
        let record = self.scratch[at..at + ENTRY_BYTES].try_into().unwrap();
        if let Some(Storage::Chain { within, .. }) = &mut self.storage {
            *within += ENTRY_BYTES as u32;
        }
        self.offset += ENTRY_BYTES as u64;
        Ok(Some(record))
    }
}

impl Volume {
    pub(crate) fn directory(&self, entry: &Entry) -> Result<Directory, Status> {
        if !entry.is_directory() {
            return Err(Status::NOT_FOUND);
        }
        if entry.cluster == 0 && (entry.name.as_slice().is_empty() || entry.is_parent()) {
            return Ok(Directory::Root);
        }
        self.geometry
            .cluster_offset(entry.cluster)
            .ok_or(Status::VOLUME_CORRUPTED)?;
        Ok(Directory::Cluster(entry.cluster))
    }

    pub(crate) fn find(
        &self,
        source: &mut impl ReadAt,
        directory: Directory,
        name: &[u16],
    ) -> Result<Entry, Status> {
        let mut cursor = DirectoryCursor::new(self, source, directory, 0)?;
        while let Some(entry) = cursor.next(self, source)? {
            if !entry.is_label() && entry.matches(name) {
                return Ok(entry);
            }
        }
        Err(Status::NOT_FOUND)
    }

    pub(crate) fn volume_label(&self, source: &mut impl ReadAt) -> Result<Name, Status> {
        let mut cursor = DirectoryCursor::new(self, source, Directory::Root, 0)?;
        while let Some(entry) = cursor.next(self, source)? {
            if entry.is_label() {
                return Ok(entry.name);
            }
        }
        Ok(Name::label(&self.geometry.bpb_label))
    }
}
