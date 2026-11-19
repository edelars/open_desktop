//! Volume-relative file lookup, allocation validation and bounded byte reads.

use crate::uefi::status::Status;

use super::{Directory, Entry, ReadAt, Volume, bpb::Root, table::Chain};

const SEPARATOR: u16 = b'\\' as u16;
const DOT: u16 = b'.' as u16;

impl Volume {
    pub(crate) fn open_path(
        &self,
        source: &mut impl ReadAt,
        base: &Entry,
        path: &[u16],
    ) -> Result<Entry, Status> {
        if path.is_empty() {
            return Err(Status::INVALID_PARAMETER);
        }
        let mut entry = if path[0] == SEPARATOR {
            Entry::root()
        } else {
            base.clone()
        };
        for component in path
            .split(|&unit| unit == SEPARATOR)
            .filter(|component| !component.is_empty())
        {
            let directory = self.directory(&entry)?;
            if component == [DOT] {
                continue;
            }
            entry = if component == [DOT, DOT] && directory == Directory::Root {
                Entry::root()
            } else {
                self.find(source, directory, component)?
            };
        }
        if path.last() == Some(&SEPARATOR) && !entry.is_directory() {
            return Err(Status::NOT_FOUND);
        }
        Ok(entry)
    }

    pub(crate) fn allocation_bytes(
        &self,
        source: &mut impl ReadAt,
        entry: &Entry,
    ) -> Result<u64, Status> {
        let geometry = self.geometry;
        let first = if entry.is_directory() {
            match (self.directory(entry)?, geometry.root) {
                (Directory::Root, Root::Fixed { entries, .. }) => {
                    let sector = u64::from(geometry.bytes_per_sector);
                    return Ok((u64::from(entries) * 32).div_ceil(sector) * sector);
                }
                (Directory::Root, Root::Cluster(first)) | (Directory::Cluster(first), _) => first,
            }
        } else if entry.cluster == 0 && entry.len == 0 {
            return Ok(0);
        } else {
            entry.cluster
        };
        let mut chain = Chain::new(geometry, first)?;
        let mut allocated = u64::from(geometry.cluster_bytes);
        while chain.advance(geometry, source)? {
            allocated += u64::from(geometry.cluster_bytes);
        }
        if allocated < u64::from(entry.len) {
            return Err(Status::VOLUME_CORRUPTED);
        }
        Ok(allocated)
    }

    pub(crate) fn read_file(
        &self,
        source: &mut impl ReadAt,
        entry: &Entry,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, Status> {
        if entry.is_directory() {
            return Err(Status::UNSUPPORTED);
        }
        let length = u64::from(entry.len);
        let count = (length.saturating_sub(offset) as usize).min(out.len());
        if count == 0 {
            return Ok(0);
        }
        let geometry = self.geometry;
        let cluster_bytes = u64::from(geometry.cluster_bytes);
        let mut chain = Chain::new(geometry, entry.cluster)?;
        for _ in 0..offset / cluster_bytes {
            if !chain.advance(geometry, source)? {
                return Err(Status::VOLUME_CORRUPTED);
            }
        }
        let mut within = offset % cluster_bytes;
        let mut written = 0;
        loop {
            let base = geometry
                .cluster_offset(chain.current())
                .ok_or(Status::VOLUME_CORRUPTED)?;
            let chunk = ((cluster_bytes - within) as usize).min(count - written);
            source.read_at(base + within, &mut out[written..written + chunk])?;
            written += chunk;
            if written == count {
                return Ok(written);
            }
            if !chain.advance(geometry, source)? {
                return Err(Status::VOLUME_CORRUPTED);
            }
            within = 0;
        }
    }
}
