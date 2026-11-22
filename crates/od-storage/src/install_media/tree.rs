use crate::io::{CheckedStorage, StorageBackend};

use super::ImportError;
use super::wim;

pub(super) enum Name {
    Short([u8; 11]),
    Long(String),
}

impl Name {
    pub(super) fn eq_ignore_ascii_case(&self, name: &str) -> bool {
        match self {
            Name::Long(long) => long.eq_ignore_ascii_case(name),
            Name::Short(short) => {
                let (base, extension) = name.rsplit_once('.').unwrap_or((name, ""));
                short[..8]
                    .trim_ascii_end()
                    .eq_ignore_ascii_case(base.as_bytes())
                    && short[8..]
                        .trim_ascii_end()
                        .eq_ignore_ascii_case(extension.as_bytes())
            }
        }
    }
}

impl Name {
    pub(super) fn display(&self) -> String {
        match self {
            Name::Long(long) => long.clone(),
            Name::Short(short) => {
                let base = String::from_utf8_lossy(short[..8].trim_ascii_end());
                let extension = String::from_utf8_lossy(short[8..].trim_ascii_end());
                if extension.is_empty() {
                    base.into_owned()
                } else {
                    format!("{base}.{extension}")
                }
            }
        }
    }
}

/// Adds the file at `path` (components separated by `/`). Missing parent
/// directories are created only with `create_dirs`; an existing entry of the
/// same name (compared case-insensitively) is refused.
pub(super) fn insert_file(
    entries: &mut Vec<Entry>,
    path: &str,
    file: File,
    create_dirs: bool,
) -> Result<(), ImportError> {
    let components: Vec<&str> = path.split('/').collect();
    if !components.iter().all(|part| valid_component(part)) {
        return Err(ImportError::Unsupported("media_path"));
    }
    let Some((name, directories)) = components.split_last() else {
        return Err(ImportError::Unsupported("media_path"));
    };
    let mut level = entries;
    for directory in directories {
        let index = match level
            .iter()
            .position(|entry| entry.name.eq_ignore_ascii_case(directory))
        {
            Some(index) => index,
            None if create_dirs => {
                level.push(Entry {
                    name: Name::Long((*directory).to_owned()),
                    contents: Contents::Directory(Vec::new()),
                });
                level.len() - 1
            }
            None => return Err(ImportError::Unsupported("media_directory_missing")),
        };
        level = match &mut level[index].contents {
            Contents::Directory(children) => children,
            Contents::File(_) => return Err(ImportError::Unsupported("media_path_conflict")),
        };
    }
    if level
        .iter()
        .any(|entry| entry.name.eq_ignore_ascii_case(name))
    {
        return Err(ImportError::Unsupported("media_file_exists"));
    }
    level.push(Entry {
        name: Name::Long((*name).to_owned()),
        contents: Contents::File(FileData::Source(file)),
    });
    Ok(())
}

fn valid_component(part: &str) -> bool {
    !part.is_empty()
        && part.len() <= 255
        && part != "."
        && part != ".."
        && !part.ends_with([' ', '.'])
        && part
            .bytes()
            .all(|byte| (0x20..0x7f).contains(&byte) && !b"\"*/:<>?\\|".contains(&byte))
}

pub(super) struct Entry {
    pub(super) name: Name,
    pub(super) contents: Contents,
}

pub(super) enum Contents {
    Directory(Vec<Entry>),
    File(FileData),
}

pub(super) enum FileData {
    Source(File),
    SplitWim(wim::Part),
}

impl FileData {
    pub(super) fn length(&self) -> u64 {
        match self {
            Self::Source(file) => file.length,
            Self::SplitWim(part) => part.length(),
        }
    }

    pub(super) fn emit<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        identity: u64,
        buffer: &mut [u8],
        put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        match self {
            Self::Source(file) => file.emit(source, buffer, put),
            Self::SplitWim(part) => part.emit(source, identity, buffer, put),
        }
    }
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

    /// Map a logical subrange into ordered source extents. Neighbouring ranges
    /// may share one extent when they are adjacent on the source.
    pub(super) fn append_ranges(
        &self,
        mut offset: u64,
        mut length: u64,
        ranges: &mut Vec<Extent>,
    ) -> Result<(), ImportError> {
        if offset > self.length || length > self.length - offset {
            return Err(ImportError::Corrupt("media_read_bounds"));
        }
        while length != 0 {
            let (physical, available) = self
                .locate(offset)
                .ok_or(ImportError::Corrupt("media_extent_length"))?;
            let count = available.min(length);
            if let Some(previous) = ranges.last_mut()
                && previous.offset + previous.length == physical
            {
                previous.length += count;
            } else {
                ranges.push(Extent {
                    offset: physical,
                    length: count,
                });
            }
            offset += count;
            length -= count;
        }
        Ok(())
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

    pub(super) fn emit<B: StorageBackend>(
        &self,
        source: &CheckedStorage<B>,
        buffer: &mut [u8],
        mut put: impl FnMut(u64, &[u8]) -> Result<(), ImportError>,
    ) -> Result<(), ImportError> {
        let mut output = 0;
        for extent in &self.extents {
            for at in (0..extent.length).step_by(buffer.len()) {
                let count = (extent.length - at).min(buffer.len() as u64) as usize;
                source.read_exact_at(extent.offset + at, &mut buffer[..count])?;
                put(output + at, &buffer[..count])?;
            }
            output += extent.length;
        }
        Ok(())
    }
}
