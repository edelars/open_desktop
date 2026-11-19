//! EFI_DISK_IO_PROTOCOL byte offsets over the same NVMe media as Block I/O.

use crate::uefi::{guid::Guid, status::Status};

use super::block_io::Endpoint;

pub const DISK_IO_PROTOCOL_GUID: Guid = Guid {
    data1: 0xce34_5171,
    data2: 0xba0b,
    data3: 0x11d2,
    data4: [0x8e, 0x4f, 0, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};
pub const REVISION: u64 = 0x0001_0000;

#[repr(C)]
pub struct DiskIo {
    pub revision: u64,
    pub read_disk: unsafe extern "efiapi" fn(*mut Self, u32, u64, usize, *mut u8) -> Status,
    pub write_disk: unsafe extern "efiapi" fn(*mut Self, u32, u64, usize, *const u8) -> Status,
}

pub(super) fn disk_io() -> DiskIo {
    DiskIo {
        revision: REVISION,
        read_disk,
        write_disk,
    }
}

unsafe extern "efiapi" fn read_disk(
    this: *mut DiskIo,
    media_id: u32,
    offset: u64,
    bytes: usize,
    buffer: *mut u8,
) -> Status {
    let result = unsafe {
        Endpoint::from_disk(this)
            .and_then(|endpoint| endpoint.access(media_id, false, offset, bytes, buffer))
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn write_disk(
    this: *mut DiskIo,
    media_id: u32,
    offset: u64,
    bytes: usize,
    buffer: *const u8,
) -> Status {
    let result = unsafe {
        Endpoint::from_disk(this)
            .and_then(|endpoint| endpoint.access(media_id, true, offset, bytes, buffer.cast_mut()))
    };
    result.err().unwrap_or(Status::SUCCESS)
}
