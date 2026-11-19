//! Read-only FAT12, FAT16 and FAT32 filesystem parsing.

use crate::uefi::status::Status;

mod bpb;
mod dir;
mod file;
mod lfn;
mod name;
mod table;

use bpb::Geometry;
pub(crate) use dir::{Directory, DirectoryCursor, Entry, Times};

pub(crate) trait ReadAt {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status>;
}

pub(crate) struct Volume {
    geometry: Geometry,
}

impl Volume {
    pub(crate) fn mount(source: &mut impl ReadAt, bytes: u64) -> Result<Self, Status> {
        if bytes < 512 {
            return Err(Status::UNSUPPORTED);
        }
        let mut boot = [0; 512];
        source.read_at(0, &mut boot)?;
        let geometry = Geometry::parse(&boot, bytes).ok_or(Status::UNSUPPORTED)?;
        Ok(Self { geometry })
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.geometry.total_bytes
    }

    pub(crate) fn block_size(&self) -> u32 {
        self.geometry.bytes_per_sector
    }
}

#[cfg(test)]
mod tests;
