//! FAT entry decoding and a bounded cursor for data-cluster chains.

use crate::uefi::status::Status;

use super::{
    ReadAt, Volume,
    bpb::{FatType, Geometry},
};

enum Link {
    Next(u32),
    End,
}

/// Bytes of the FAT one [`FatWindow`] read covers.
const WINDOW: usize = 512;

/// One 512-byte window of the first FAT, read on demand (S7-32). Consecutive entries of a chain
/// walk or of a scan share one disk read instead of one read per entry: reading a 612 MB file
/// through Simple File System walked its chain with one Disk I/O read per cluster link.
pub(super) struct FatWindow {
    bytes: [u8; WINDOW],
    at: Option<u64>,
}

impl FatWindow {
    pub(super) const fn new() -> Self {
        Self {
            bytes: [0; WINDOW],
            at: None,
        }
    }

    /// The raw FAT entry of `cluster`, bounds-checked against the FAT.
    fn entry(
        &mut self,
        geometry: Geometry,
        source: &mut impl ReadAt,
        cluster: u32,
    ) -> Result<u32, Status> {
        let (position, size) = entry_position(geometry.fat_type, cluster);
        if position + size as u64 > geometry.fat_bytes {
            return Err(Status::VOLUME_CORRUPTED);
        }
        let within = (position % WINDOW as u64) as usize;
        let mut bytes = [0; 4];
        if within + size > WINDOW {
            // A packed FAT12 entry can start at the last byte of a window.
            source.read_at(geometry.fat_base + position, &mut bytes[..size])?;
        } else {
            let at = position - within as u64;
            if self.at != Some(at) {
                // The FAT ends on a sector boundary; a short last window stays inside it.
                let length = WINDOW.min((geometry.fat_bytes - at) as usize);
                self.at = None;
                source.read_at(geometry.fat_base + at, &mut self.bytes[..length])?;
                self.at = Some(at);
            }
            bytes[..size].copy_from_slice(&self.bytes[within..within + size]);
        }
        Ok(entry_value(geometry.fat_type, cluster, bytes))
    }
}

fn link(
    geometry: Geometry,
    window: &mut FatWindow,
    source: &mut impl ReadAt,
    cluster: u32,
) -> Result<Link, Status> {
    geometry
        .cluster_offset(cluster)
        .ok_or(Status::VOLUME_CORRUPTED)?;
    let entry = window.entry(geometry, source, cluster)?;
    if entry >= geometry.fat_type.reserved_cluster() + 8 {
        return Ok(Link::End);
    }
    if entry >= geometry.fat_type.reserved_cluster() || geometry.cluster_offset(entry).is_none() {
        // Includes free, reserved, bad, and out-of-volume cluster references.
        return Err(Status::VOLUME_CORRUPTED);
    }
    Ok(Link::Next(entry))
}

fn entry_position(format: FatType, cluster: u32) -> (u64, usize) {
    let cluster = u64::from(cluster);
    match format {
        FatType::Fat12 => (cluster + cluster / 2, 2),
        FatType::Fat16 => (cluster * 2, 2),
        FatType::Fat32 => (cluster * 4, 4),
    }
}

fn entry_value(format: FatType, cluster: u32, bytes: [u8; 4]) -> u32 {
    let word = u32::from_le_bytes(bytes);
    match format {
        FatType::Fat12 if cluster & 1 == 0 => word & 0x0fff,
        FatType::Fat12 => word >> 4,
        FatType::Fat16 => word,
        FatType::Fat32 => word & 0x0fff_ffff,
    }
}

impl Volume {
    /// Bytes of the data clusters whose FAT entry is free (zero).
    pub(crate) fn free_bytes(&self, source: &mut impl ReadAt) -> Result<u64, Status> {
        let geometry = self.geometry;
        let free = match geometry.fat_type {
            // Packed 12-bit entries can cross a window; a FAT12 volume has at most 4 084 data
            // clusters, so the entry-by-entry path costs little.
            FatType::Fat12 => {
                let mut free = 0u64;
                let mut window = FatWindow::new();
                for cluster in 2..geometry.cluster_count + 2 {
                    if window.entry(geometry, source, cluster)? == 0 {
                        free += 1;
                    }
                }
                free
            }
            FatType::Fat16 => free_entries::<2>(geometry, source, 0xffff)?,
            FatType::Fat32 => free_entries::<4>(geometry, source, 0x0fff_ffff)?,
        };
        Ok(free * u64::from(geometry.cluster_bytes))
    }
}

/// Counts the free entries of the data clusters `2..cluster_count + 2` of a FAT16 (`SIZE` 2) or
/// FAT32 (`SIZE` 4) table a whole window at a time (ISSUE-697). `mask` keeps the bits
/// [`entry_value`] keeps for that type (FAT32: the low 28 bits). The table is read in aligned
/// 512-byte windows (the last one ends with the last entry); an entry of 2 or 4 bytes never
/// crosses a window, so the entries of a window are compared in place, without a bounds check or
/// a call per entry. The two reserved entries 0 and 1 and the bytes past the last data cluster
/// (the rest of the last FAT sector) are not counted.
fn free_entries<const SIZE: usize>(
    geometry: Geometry,
    source: &mut impl ReadAt,
    mask: u32,
) -> Result<u64, Status> {
    let end = (u64::from(geometry.cluster_count) + 2) * SIZE as u64;
    if end > geometry.fat_bytes {
        return Err(Status::VOLUME_CORRUPTED);
    }
    let mut bytes = [0; WINDOW];
    let mut free = 0u64;
    let mut at = 0u64;
    // Entries 0 and 1 are the first `2 * SIZE` bytes of the first window.
    let mut first = 2 * SIZE;
    while at < end {
        let length = WINDOW.min((end - at) as usize);
        source.read_at(geometry.fat_base + at, &mut bytes[..length])?;
        for entry in bytes[first..length].chunks_exact(SIZE) {
            let mut word = [0; 4];
            word[..SIZE].copy_from_slice(entry);
            if u32::from_le_bytes(word) & mask == 0 {
                free += 1;
            }
        }
        first = 0;
        at += WINDOW as u64;
    }
    Ok(free)
}

/// Brent cycle detection plus a hard bound from the volume's data-cluster count.
pub(super) struct Chain {
    current: u32,
    anchor: u32,
    window: u32,
    distance: u32,
    remaining: u32,
    fat: FatWindow,
}

impl Chain {
    pub(super) fn new(geometry: Geometry, first: u32) -> Result<Self, Status> {
        geometry
            .cluster_offset(first)
            .ok_or(Status::VOLUME_CORRUPTED)?;
        Ok(Self {
            current: first,
            anchor: first,
            window: 1,
            distance: 0,
            remaining: geometry.cluster_count,
            fat: FatWindow::new(),
        })
    }

    pub(super) fn current(&self) -> u32 {
        self.current
    }

    /// Advance one link. False means end of chain.
    pub(super) fn advance(
        &mut self,
        geometry: Geometry,
        source: &mut impl ReadAt,
    ) -> Result<bool, Status> {
        let Link::Next(next) = link(geometry, &mut self.fat, source, self.current)? else {
            return Ok(false);
        };
        if next == self.anchor || self.remaining <= 1 {
            return Err(Status::VOLUME_CORRUPTED);
        }
        self.remaining -= 1;
        self.distance += 1;
        if self.distance == self.window {
            self.anchor = next;
            self.window *= 2;
            self.distance = 0;
        }
        self.current = next;
        Ok(true)
    }
}
