//! EFI_BLOCK_IO_PROTOCOL revision 2 and the shared media access for Disk I/O.

use core::{mem::offset_of, ptr};

use crate::{
    bs::boot_services_active,
    nvme::driver::{BLOCK_BYTES, Controller},
    paging::PAGE_BYTES,
    uefi::{guid::Guid, status::Status},
};

use super::disk_io::{DiskIo, disk_io};

pub const BLOCK_IO_PROTOCOL_GUID: Guid = Guid {
    data1: 0x964e_5b21,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};
pub const REVISION: u64 = 0x0002_0001;

#[repr(C)]
pub struct BlockMedia {
    pub media_id: u32,
    pub removable_media: u8,
    pub media_present: u8,
    pub logical_partition: u8,
    pub read_only: u8,
    pub write_caching: u8,
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
    pub lowest_aligned_lba: u64,
    pub logical_blocks_per_physical_block: u32,
}

#[repr(C)]
pub struct BlockIo {
    pub revision: u64,
    pub media: *const BlockMedia,
    pub reset: unsafe extern "efiapi" fn(*mut Self, u8) -> Status,
    pub read_blocks: unsafe extern "efiapi" fn(*mut Self, u32, u64, usize, *mut u8) -> Status,
    pub write_blocks: unsafe extern "efiapi" fn(*mut Self, u32, u64, usize, *const u8) -> Status,
    pub flush_blocks: unsafe extern "efiapi" fn(*mut Self) -> Status,
}

#[repr(C)]
pub(super) struct Endpoint {
    pub(super) block: BlockIo,
    pub(super) disk: DiskIo,
    pub(super) media: BlockMedia,
    controller: *const Controller,
    start: u64,
    length: u64,
}

impl Endpoint {
    /// # Safety
    /// Both allocations remain pinned in BootServicesData until all interfaces are withdrawn.
    pub(super) unsafe fn init(
        at: *mut Self,
        controller: *const Controller,
        start: u64,
        sectors: u64,
        partition: bool,
    ) {
        let media = BlockMedia {
            media_id: 1,
            removable_media: 0,
            media_present: 1,
            logical_partition: u8::from(partition),
            read_only: 0,
            write_caching: 0,
            block_size: BLOCK_BYTES as u32,
            io_align: 1,
            last_block: sectors - 1,
            lowest_aligned_lba: 0,
            logical_blocks_per_physical_block: 1,
        };
        let block = BlockIo {
            revision: REVISION,
            media: unsafe { ptr::addr_of!((*at).media) },
            reset,
            read_blocks,
            write_blocks,
            flush_blocks,
        };
        unsafe {
            at.write(Self {
                block,
                disk: disk_io(),
                media,
                controller,
                start,
                length: sectors * BLOCK_BYTES as u64,
            })
        };
    }

    unsafe fn from_block<'a>(this: *mut BlockIo) -> Result<&'a Self, Status> {
        unsafe { Self::from_interface(this.cast()) }
    }

    pub(super) unsafe fn from_disk<'a>(this: *mut DiskIo) -> Result<&'a Self, Status> {
        if this.is_null() {
            return Err(Status::INVALID_PARAMETER);
        }
        let endpoint = unsafe { this.cast::<u8>().sub(offset_of!(Self, disk)).cast() };
        unsafe { Self::from_interface(endpoint) }
    }

    unsafe fn from_interface<'a>(this: *const Self) -> Result<&'a Self, Status> {
        if this.is_null() {
            return Err(Status::INVALID_PARAMETER);
        }
        // Timer notifications may call this protocol again before the outer operation starts.
        #[cfg(target_os = "none")]
        unsafe {
            crate::bs::event::poll_due_events()
        };
        // SAFETY: a non-null This belongs to a live protocol instance through ExitBootServices.
        if !unsafe { boot_services_active() } {
            return Err(Status::NOT_READY);
        }
        Ok(unsafe { &*this })
    }

    fn controller(&self) -> &Controller {
        // SAFETY: initialization pins the controller for at least the endpoint's lifetime.
        unsafe { &*self.controller }
    }

    pub(super) unsafe fn access(
        &self,
        media_id: u32,
        write: bool,
        offset: u64,
        bytes: usize,
        buffer: *mut u8,
    ) -> Result<(), Status> {
        if self.media.media_present == 0 {
            return Err(Status::NO_MEDIA);
        }
        if media_id != self.media.media_id {
            return Err(Status::MEDIA_CHANGED);
        }
        unsafe { self.access_bytes(write, offset, bytes, buffer) }
    }

    /// The buffer covers the requested bytes and is disjoint from firmware-owned state.
    pub(super) unsafe fn access_bytes(
        &self,
        write: bool,
        mut offset: u64,
        mut bytes: usize,
        mut buffer: *mut u8,
    ) -> Result<(), Status> {
        if bytes == 0 {
            return Ok(());
        }
        if buffer.is_null()
            || buffer.addr().checked_add(bytes).is_none()
            || offset
                .checked_add(bytes as u64)
                .is_none_or(|end| end > self.length)
        {
            return Err(Status::INVALID_PARAMETER);
        }
        self.controller().exclusive(|controller| {
            while bytes != 0 {
                let within = offset as usize % BLOCK_BYTES;
                let available = (PAGE_BYTES as usize - within).min(bytes);
                let count = if within == 0 && available >= BLOCK_BYTES {
                    available / BLOCK_BYTES * BLOCK_BYTES
                } else {
                    available.min(BLOCK_BYTES - within)
                };
                let sectors = (within + count).div_ceil(BLOCK_BYTES) as u32;
                let lba = self.start + offset / BLOCK_BYTES as u64;
                let bounce = controller.buffer();
                if !write || within != 0 || count % BLOCK_BYTES != 0 {
                    controller.transfer(false, lba, sectors)?;
                }
                let data = unsafe { bounce.add(within) };
                if write {
                    unsafe { copy_bytes(buffer, data, count) };
                    controller.transfer(true, lba, sectors)?;
                } else {
                    unsafe { copy_bytes(data, buffer, count) };
                }
                offset += count as u64;
                bytes -= count;
                buffer = unsafe { buffer.add(count) };
            }
            Ok(())
        })
    }

    unsafe fn access_blocks(
        &self,
        media_id: u32,
        write: bool,
        lba: u64,
        bytes: usize,
        buffer: *mut u8,
    ) -> Result<(), Status> {
        if self.media.media_present == 0 {
            return Err(Status::NO_MEDIA);
        }
        if media_id != self.media.media_id {
            return Err(Status::MEDIA_CHANGED);
        }
        if bytes == 0 {
            return Ok(());
        }
        if !bytes.is_multiple_of(BLOCK_BYTES) {
            return Err(Status::BAD_BUFFER_SIZE);
        }
        let offset = lba
            .checked_mul(BLOCK_BYTES as u64)
            .ok_or(Status::INVALID_PARAMETER)?;
        unsafe { self.access_bytes(write, offset, bytes, buffer) }
    }
}

unsafe fn copy_bytes(source: *const u8, destination: *mut u8, bytes: usize) {
    // Keep compiler-rt string operations within the probe's per-instruction REP bound.
    for at in (0..bytes).step_by(1024) {
        let count = (bytes - at).min(1024);
        unsafe { ptr::copy_nonoverlapping(source.add(at), destination.add(at), count) };
    }
}

unsafe extern "efiapi" fn reset(this: *mut BlockIo, extended: u8) -> Status {
    let result = (|| {
        let endpoint = unsafe { Endpoint::from_block(this) }?;
        endpoint.controller().exclusive(|controller| {
            controller.reset()?;
            if extended != 0 {
                controller.transfer(false, endpoint.start, 1)?;
            }
            Ok(())
        })
    })();
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn read_blocks(
    this: *mut BlockIo,
    media_id: u32,
    lba: u64,
    bytes: usize,
    buffer: *mut u8,
) -> Status {
    let result = unsafe {
        Endpoint::from_block(this)
            .and_then(|endpoint| endpoint.access_blocks(media_id, false, lba, bytes, buffer))
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn write_blocks(
    this: *mut BlockIo,
    media_id: u32,
    lba: u64,
    bytes: usize,
    buffer: *const u8,
) -> Status {
    let result = unsafe {
        Endpoint::from_block(this).and_then(|endpoint| {
            endpoint.access_blocks(media_id, true, lba, bytes, buffer.cast_mut())
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn flush_blocks(this: *mut BlockIo) -> Status {
    let result = unsafe { Endpoint::from_block(this) }
        .and_then(|endpoint| endpoint.controller().exclusive(Controller::flush));
    result.err().unwrap_or(Status::SUCCESS)
}
