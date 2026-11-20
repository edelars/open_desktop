//! Host-side conversion of an installer image into a GPT disk with a FAT32 ESP.
//! The ISO-9660 and UDF input subsets are defined by ADR-0012.

use crate::io::{CheckedStorage, StorageBackend, StorageError};

mod fat32;
mod gpt;
mod iso9660;
mod tree;
mod udf;

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
}

impl InstallMedia {
    pub fn from_iso<B: StorageBackend>(source: &CheckedStorage<B>) -> Result<Self, ImportError> {
        let tree = if udf::recognize(source)? {
            udf::read(source)?
        } else {
            iso9660::read(source)?
        };
        let volume = fat32::Volume::plan(tree, source.size()?)?;
        let disk_sectors =
            gpt::disk_sectors(volume.sectors()).ok_or(ImportError::Unsupported("fat_capacity"))?;
        Ok(Self {
            volume,
            disk_sectors,
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
        gpt::emit(self.disk_sectors, identity, &mut put)?;
        self.volume.emit(source, identity, put)
    }
}

impl From<StorageError> for ImportError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
