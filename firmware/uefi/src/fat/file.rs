//! Volume-relative file lookup, allocation validation and bounded byte reads.

use crate::uefi::status::Status;

use super::{Directory, Entry, ReadAt, Volume, bpb::Root, table::Chain};

const SEPARATOR: u16 = b'\\' as u16;
const DOT: u16 = b'.' as u16;

/// Where a previous read left the cluster chain of a file: its `index`-th cluster (counted from
/// 0) is `cluster` (S7-32). A later read at or past that cluster continues from it instead of
/// walking the chain from the first cluster again: sequential reads of a large file through
/// Simple File System were quadratic in the number of clusters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ChainCursor {
    index: u64,
    cluster: u32,
}

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

    /// Read up to `out.len()` bytes of `entry` from `offset`. `cursor` is the chain position a
    /// previous read of the same file left (or `None`); it is used when it does not lie past the
    /// target cluster and is updated to the cluster holding the last byte read. Physically
    /// consecutive clusters are read with one `read_at`.
    pub(crate) fn read_file(
        &self,
        source: &mut impl ReadAt,
        entry: &Entry,
        offset: u64,
        out: &mut [u8],
        cursor: &mut Option<ChainCursor>,
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
        let target = offset / cluster_bytes;
        let (mut chain, mut index) = match *cursor {
            Some(at) if at.index <= target => (Chain::new(geometry, at.cluster)?, at.index),
            _ => (Chain::new(geometry, entry.cluster)?, 0),
        };
        while index < target {
            if !chain.advance(geometry, source)? {
                return Err(Status::VOLUME_CORRUPTED);
            }
            index += 1;
        }
        let mut within = offset % cluster_bytes;
        let mut written = 0;
        loop {
            let first = chain.current();
            let base = geometry
                .cluster_offset(first)
                .ok_or(Status::VOLUME_CORRUPTED)?;
            // Extend the run over the clusters that follow `first` on the disk while more bytes
            // are needed; `pending` is a chain cluster that breaks the run and is not read yet.
            let mut last = first;
            let mut span = cluster_bytes - within;
            let mut pending = false;
            while written as u64 + span < count as u64 {
                if !chain.advance(geometry, source)? {
                    return Err(Status::VOLUME_CORRUPTED);
                }
                index += 1;
                if chain.current() == last.wrapping_add(1) {
                    last = chain.current();
                    span += cluster_bytes;
                } else {
                    pending = true;
                    break;
                }
            }
            let chunk = span.min((count - written) as u64) as usize;
            source.read_at(base + within, &mut out[written..written + chunk])?;
            written += chunk;
            if written == count {
                // The run ended inside `last`, which is the chain's current cluster.
                *cursor = Some(ChainCursor {
                    index,
                    cluster: last,
                });
                return Ok(written);
            }
            debug_assert!(
                pending,
                "a run shorter than the request ends at a chain break"
            );
            within = 0;
        }
    }
}
