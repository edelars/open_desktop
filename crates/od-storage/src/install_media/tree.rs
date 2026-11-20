use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;

pub(super) enum Name {
    Short([u8; 11]),
    Long(String),
}

pub(super) struct Entry {
    pub(super) name: Name,
    pub(super) contents: Contents,
}

pub(super) enum Contents {
    Directory(Vec<Entry>),
    File(File),
}

pub(super) struct Extent {
    pub(super) offset: u64,
    pub(super) length: u64,
}

pub(super) struct File {
    pub(super) length: u64,
    extents: Vec<Extent>,
}

impl File {
    pub(super) fn contiguous(extent: Extent) -> Self {
        Self {
            length: extent.length,
            extents: vec![extent],
        }
    }

    pub(super) fn new(length: u64, extents: Vec<Extent>) -> Result<Self, ImportError> {
        let recorded = extents
            .iter()
            .try_fold(0u64, |length, extent| length.checked_add(extent.length));
        if recorded != Some(length) {
            return Err(ImportError::Corrupt("media_extent_length"));
        }
        Ok(Self { length, extents })
    }

    /// Physical offset and available bytes in the extent containing the logical offset.
    pub(super) fn locate(&self, mut offset: u64) -> Option<(u64, u64)> {
        for extent in &self.extents {
            if offset < extent.length {
                return Some((extent.offset + offset, extent.length - offset));
            }
            offset -= extent.length;
        }
        None
    }

    pub(super) fn read_exact_at<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        mut offset: u64,
        mut bytes: &mut [u8],
    ) -> Result<(), ImportError> {
        if offset > self.length || bytes.len() as u64 > self.length - offset {
            return Err(ImportError::Corrupt("media_read_bounds"));
        }
        while !bytes.is_empty() {
            let (physical, available) = self
                .locate(offset)
                .ok_or(ImportError::Corrupt("media_extent_length"))?;
            let count = available.min(bytes.len() as u64) as usize;
            source.read_exact_at(physical, &mut bytes[..count])?;
            offset += count as u64;
            bytes = &mut bytes[count..];
        }
        Ok(())
    }
}
