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

fn link(geometry: Geometry, source: &mut impl ReadAt, cluster: u32) -> Result<Link, Status> {
    geometry
        .cluster_offset(cluster)
        .ok_or(Status::VOLUME_CORRUPTED)?;
    let (position, size) = entry_position(geometry.fat_type, cluster);
    if position + size as u64 > geometry.fat_bytes {
        return Err(Status::VOLUME_CORRUPTED);
    }
    let mut bytes = [0; 4];
    source.read_at(geometry.fat_base + position, &mut bytes[..size])?;
    let entry = entry_value(geometry.fat_type, cluster, bytes);
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
    pub(crate) fn free_bytes(&self, source: &mut impl ReadAt) -> Result<u64, Status> {
        const CACHE: usize = 512;
        let geometry = self.geometry;
        let last = geometry.cluster_count + 2;
        let mut free = 0u64;
        let mut scratch = [0; CACHE];
        let mut sector_at = None;
        for cluster in 2..last {
            let (position, size) = entry_position(geometry.fat_type, cluster);
            let within = position as usize % CACHE;
            let mut bytes = [0; 4];
            if within + size > CACHE {
                // A packed FAT12 entry can start at the last byte in a sector.
                source.read_at(geometry.fat_base + position, &mut bytes[..size])?;
            } else {
                let at = position - within as u64;
                if sector_at != Some(at) {
                    source.read_at(geometry.fat_base + at, &mut scratch)?;
                    sector_at = Some(at);
                }
                bytes[..size].copy_from_slice(&scratch[within..within + size]);
            }
            if entry_value(geometry.fat_type, cluster, bytes) == 0 {
                free += 1;
            }
        }
        Ok(free * u64::from(geometry.cluster_bytes))
    }
}

/// Brent cycle detection plus a hard bound from the volume's data-cluster count.
pub(super) struct Chain {
    current: u32,
    anchor: u32,
    window: u32,
    distance: u32,
    remaining: u32,
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
        let Link::Next(next) = link(geometry, source, self.current)? else {
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
