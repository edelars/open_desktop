//! Host-side conversion of an installer image into a GPT disk with a FAT32 ESP.
//! The ISO-9660 and UDF input subsets are defined by ADR-0012.
//!
//! The same GPT and FAT32 writer also builds a payload disk (a Microsoft Basic
//! Data partition, so that Windows assigns a drive letter) from host files, and
//! extends a converted installation disk by files into a new disk.

use crate::io::{CheckedStorage, StorageBackend, StorageError};

mod fat32;
mod gpt;
mod iso9660;
mod reader;
mod tree;
mod udf;
mod wim;

/// Fixed reasons are safe to put in tool output without exposing image data.
#[derive(Debug)]
pub enum ImportError {
    Corrupt(&'static str),
    Unsupported(&'static str),
    Storage(StorageError),
    Output(&'static str),
}

/// Validated source tree and output layout. Planning opens no output.
pub struct InstallMedia {
    volume: fat32::Volume,
    disk_sectors: u32,
    partition: gpt::Partition,
}

/// A file to add to a planned disk. Its bytes are the range
/// `source_offset..source_offset + length` of the source object that is later
/// passed to [`InstallMedia::write_and_verify`].
#[derive(Clone, Copy, Debug)]
pub struct AddedFile<'a> {
    /// Path inside the volume, components separated by `/`.
    pub path: &'a str,
    pub source_offset: u64,
    pub length: u64,
}

const INSTALL_LABEL: [u8; 11] = *b"OD INSTALL ";

impl InstallMedia {
    pub fn from_iso<B: StorageBackend>(source: &CheckedStorage<B>) -> Result<Self, ImportError> {
        let mut tree = if udf::recognize(source)? {
            udf::read(source)?
        } else {
            iso9660::read(source)?
        };
        wim::split_install_wim(source, &mut tree)?;
        Self::plan(tree, source.size()?, INSTALL_LABEL, 0, gpt::ESP)
    }

    /// Plans a copy of a disk made by this module with extra files. The first
    /// `disk_len` bytes of `source` are that disk; the added files' bytes are
    /// elsewhere in `source`. The parent directory of each file must already
    /// exist and no file may exist at its path.
    pub fn from_converted_disk<B: StorageBackend>(
        source: &CheckedStorage<B>,
        disk_len: u64,
        additions: &[AddedFile<'_>],
    ) -> Result<Self, ImportError> {
        let disk = reader::read(source, disk_len)?;
        if disk.partition_type != gpt::ESP.type_guid() {
            return Err(ImportError::Unsupported("media_partition_type"));
        }
        let mut entries = disk.entries;
        for added in additions {
            tree::insert_file(&mut entries, added.path, added_file(source, added)?, false)?;
        }
        Self::plan(entries, source.size()?, disk.label, 0, gpt::ESP)
    }

    /// Plans a payload disk: one Microsoft Basic Data partition with a FAT32
    /// volume labelled `label` (1 to 11 characters of A-Z, 0-9, `_`, `-`)
    /// holding `files`, with at least `spare_bytes` of free space after them.
    pub fn payload_disk<B: StorageBackend>(
        source: &CheckedStorage<B>,
        label: &str,
        spare_bytes: u64,
        files: &[AddedFile<'_>],
    ) -> Result<Self, ImportError> {
        let label = volume_label(label)?;
        let mut entries = Vec::new();
        for added in files {
            tree::insert_file(&mut entries, added.path, added_file(source, added)?, true)?;
        }
        let size_hint = source
            .size()?
            .checked_add(spare_bytes)
            .ok_or(ImportError::Unsupported("fat_capacity"))?;
        Self::plan(entries, size_hint, label, spare_bytes, gpt::BASIC_DATA)
    }

    fn plan(
        entries: Vec<tree::Entry>,
        size_hint: u64,
        label: [u8; 11],
        spare_bytes: u64,
        partition: gpt::Partition,
    ) -> Result<Self, ImportError> {
        let volume = fat32::Volume::plan(entries, size_hint, label, spare_bytes)?;
        let disk_sectors =
            gpt::disk_sectors(volume.sectors()).ok_or(ImportError::Unsupported("fat_capacity"))?;
        Ok(Self {
            volume,
            disk_sectors,
            partition,
        })
    }

    pub fn disk_bytes(&self) -> u64 {
        u64::from(self.disk_sectors) * 512
    }

    pub fn file_count(&self) -> u32 {
        self.volume.files
    }

    /// The source must be the same unchanged object used for planning. Writes to
    /// an empty destination, flushes file contents, and then verifies the bytes
    /// used by the GPT, FAT32 and copied files before the caller publishes it.
    pub fn write_and_verify<S: StorageBackend, D: StorageBackend>(
        &self,
        source: &CheckedStorage<S>,
        destination: &mut CheckedStorage<D>,
        identity: u64,
    ) -> Result<(), ImportError> {
        if destination.size()? != 0 {
            return Err(ImportError::Output("target_not_empty"));
        }
        if !destination.capabilities().file_durability {
            return Err(ImportError::Output("durability_unavailable"));
        }
        destination.set_len(self.disk_bytes())?;
        self.emit(source, identity, |offset, bytes| {
            Ok(destination.write_all_at(offset, bytes)?)
        })?;
        destination.flush_file_durable()?;

        let mut check = Vec::new();
        self.emit(source, identity, |offset, bytes| {
            check.resize(bytes.len(), 0);
            destination.read_exact_at(offset, &mut check)?;
            if check != bytes {
                return Err(ImportError::Output("verification_failed"));
            }
            Ok(())
        })
    }

    fn emit<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        identity: u64,
        mut put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        gpt::emit(self.disk_sectors, identity, self.partition, &mut put)?;
        self.volume.emit(source, identity, put)
    }
}

fn added_file<B: StorageBackend>(
    source: &CheckedStorage<B>,
    added: &AddedFile<'_>,
) -> Result<tree::File, ImportError> {
    let end = added
        .source_offset
        .checked_add(added.length)
        .ok_or(ImportError::Corrupt("media_read_bounds"))?;
    if end > source.size()? {
        return Err(ImportError::Corrupt("media_read_bounds"));
    }
    let extents = if added.length == 0 {
        Vec::new()
    } else {
        vec![tree::Extent {
            offset: added.source_offset,
            length: added.length,
        }]
    };
    tree::File::new(added.length, extents)
}

fn volume_label(label: &str) -> Result<[u8; 11], ImportError> {
    if label.is_empty()
        || label.len() > 11
        || !label
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
    {
        return Err(ImportError::Unsupported("volume_label"));
    }
    let mut out = [b' '; 11];
    out[..label.len()].copy_from_slice(label.as_bytes());
    Ok(out)
}

/// One file of a disk read back with [`list_disk`].
pub struct ListedFile {
    /// Path inside the volume, components separated by `/`.
    pub path: String,
    pub length: u64,
    file: tree::File,
}

impl ListedFile {
    /// The whole file; for files up to 256 MiB.
    pub fn read_to_vec<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
    ) -> Result<Vec<u8>, ImportError> {
        if self.length > 256 * 1024 * 1024 {
            return Err(ImportError::Unsupported("media_read_size"));
        }
        let mut bytes = vec![0; self.length as usize];
        let mut done = 0;
        while done < bytes.len() {
            let (physical, available) = self
                .file
                .locate(done as u64)
                .ok_or(ImportError::Corrupt("media_extent_length"))?;
            let count = (bytes.len() - done).min(available.min(1 << 20) as usize);
            source.read_exact_at(physical, &mut bytes[done..done + count])?;
            done += count;
        }
        Ok(bytes)
    }
}

/// Contents of a disk made by this module.
pub struct DiskListing {
    /// FAT volume label without trailing spaces.
    pub label: String,
    /// GPT partition type of the only partition, in on-disk byte order.
    pub partition_type: [u8; 16],
    pub directories: Vec<String>,
    pub files: Vec<ListedFile>,
}

/// Reads back a disk made by this module (the first `disk_len` bytes of
/// `source`) through the GPT and FAT32 structures only.
pub fn list_disk<B: StorageBackend>(
    source: &CheckedStorage<B>,
    disk_len: u64,
) -> Result<DiskListing, ImportError> {
    let disk = reader::read(source, disk_len)?;
    let mut listing = DiskListing {
        label: String::from_utf8_lossy(&disk.label)
            .trim_end_matches(' ')
            .to_owned(),
        partition_type: disk.partition_type,
        directories: Vec::new(),
        files: Vec::new(),
    };
    flatten(disk.entries, "", &mut listing);
    Ok(listing)
}

fn flatten(entries: Vec<tree::Entry>, prefix: &str, listing: &mut DiskListing) {
    for entry in entries {
        let path = format!("{prefix}{}", entry.name.display());
        match entry.contents {
            tree::Contents::Directory(children) => {
                listing.directories.push(path.clone());
                flatten(children, &format!("{path}/"), listing);
            }
            tree::Contents::File(tree::FileData::Source(file)) => listing.files.push(ListedFile {
                path,
                length: file.length,
                file,
            }),
            tree::Contents::File(tree::FileData::SplitWim(_)) => {}
        }
    }
}

impl From<StorageError> for ImportError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
