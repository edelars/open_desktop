//! Attach Simple File System to storage's existing ESP handles.

use core::{ffi::c_void, mem::size_of, ptr, slice};

use crate::{
    block::{
        block_io::{BLOCK_IO_PROTOCOL_GUID, BlockIo},
        disk_io::{DISK_IO_PROTOCOL_GUID, DiskIo},
        partition_gpt::SYSTEM_PARTITION,
        storage::{PARTITION_INFO_PROTOCOL_GUID, PartitionInfo},
    },
    bs::{
        boot_services_active,
        memory::{allocate_pool, free_pool},
    },
    fat::{Entry, ReadAt, Volume},
    mem::map::MemoryType,
    uefi::{
        guid::Guid,
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};

use super::file::new_handle;
pub use super::{
    file::FileProtocol,
    info::{FILE_INFO_GUID, FILE_SYSTEM_INFO_GUID, FileInfo, FileSystemInfo},
};

pub const SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: Guid = Guid {
    data1: 0x964e_5b22,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

#[repr(C)]
pub struct SimpleFileSystem {
    pub revision: u64,
    pub open_volume: unsafe extern "efiapi" fn(*mut Self, *mut *mut FileProtocol) -> Status,
}

#[repr(C)]
pub(super) struct Mounted {
    protocol: SimpleFileSystem,
    pub(super) volume: Volume,
    pub(super) disk: Disk,
}

#[derive(Clone, Copy)]
pub(super) struct Disk {
    interface: *mut DiskIo,
    media_id: u32,
}

impl ReadAt for Disk {
    fn read_at(&mut self, offset: u64, out: &mut [u8]) -> Result<(), Status> {
        // SAFETY: the published ESP handle owns the live Disk I/O protocol.
        let status = unsafe {
            ((*self.interface).read_disk)(
                self.interface,
                self.media_id,
                offset,
                out.len(),
                out.as_mut_ptr(),
            )
        };
        check(status)
    }
}

pub(super) fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

unsafe extern "efiapi" fn open_volume(
    this: *mut SimpleFileSystem,
    out: *mut *mut FileProtocol,
) -> Status {
    if this.is_null() || out.is_null() {
        return Status::INVALID_PARAMETER;
    }
    #[cfg(target_os = "none")]
    unsafe {
        crate::bs::event::poll_due_events()
    };
    if !unsafe { boot_services_active() } {
        return Status::NOT_READY;
    }
    // SAFETY: the protocol is at offset zero of its pinned mount record.
    let mounted = unsafe { &*this.cast::<Mounted>() };
    match new_handle(mounted, Entry::root()) {
        Ok(file) => {
            unsafe { out.write(file) };
            Status::SUCCESS
        }
        Err(status) => status,
    }
}

unsafe fn interface<T>(boot: &BootServices, handle: Handle, guid: &Guid) -> Result<*mut T, Status> {
    let mut value = ptr::null_mut();
    check(unsafe { (boot.handle_protocol)(handle, guid, &mut value) })?;
    if value.is_null() {
        Err(Status::DEVICE_ERROR)
    } else {
        Ok(value.cast())
    }
}

unsafe fn mount(boot: &BootServices, mut handle: Handle) -> Result<(), Status> {
    let info =
        unsafe { &*interface::<PartitionInfo>(boot, handle, &PARTITION_INFO_PROTOCOL_GUID)? };
    if info.system == 0 || info.partition.partition_type != SYSTEM_PARTITION {
        return Ok(());
    }
    let block = unsafe { &*interface::<BlockIo>(boot, handle, &BLOCK_IO_PROTOCOL_GUID)? };
    let media = unsafe { block.media.as_ref() }.ok_or(Status::DEVICE_ERROR)?;
    if media.logical_partition == 0 || media.media_present == 0 {
        return Ok(());
    }
    let byte_count = media
        .last_block
        .checked_add(1)
        .and_then(|blocks| blocks.checked_mul(u64::from(media.block_size)))
        .ok_or(Status::DEVICE_ERROR)?;
    let mut disk = Disk {
        interface: unsafe { interface::<DiskIo>(boot, handle, &DISK_IO_PROTOCOL_GUID) }?,
        media_id: media.media_id,
    };
    let volume = match Volume::mount(&mut disk, byte_count) {
        Ok(volume) => volume,
        Err(Status::UNSUPPORTED | Status::VOLUME_CORRUPTED | Status::NO_MEDIA) => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut pointer = ptr::null_mut();
    check(unsafe {
        allocate_pool(
            MemoryType::BOOT_SERVICES_DATA,
            size_of::<Mounted>(),
            &mut pointer,
        )
    })?;
    let mounted = pointer.cast::<Mounted>();
    unsafe {
        mounted.write(Mounted {
            protocol: SimpleFileSystem {
                revision: 0x0001_0000,
                open_volume,
            },
            volume,
            disk,
        })
    };
    let status = unsafe {
        (boot.install_protocol_interface)(
            &mut handle,
            &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
            0,
            pointer,
        )
    };
    if status != Status::SUCCESS {
        // InstallProtocolInterface leaves the existing device handle undisturbed on failure.
        unsafe { free_pool(pointer) };
    }
    check(status)
}

/// # Safety
/// The BSP calls this with a live System Table before starting any EFI applications.
pub unsafe fn install(system: *mut SystemTable) -> Status {
    let boot = unsafe { &*(*system).boot_services };
    let mut count = 0;
    let mut handles = ptr::null_mut();
    let status = unsafe {
        (boot.locate_handle_buffer)(
            2,
            &SYSTEM_PARTITION,
            ptr::null_mut(),
            &mut count,
            &mut handles,
        )
    };
    if status == Status::NOT_FOUND {
        return Status::SUCCESS;
    }
    if status != Status::SUCCESS || count == 0 || handles.is_null() {
        return if status == Status::SUCCESS {
            Status::DEVICE_ERROR
        } else {
            status
        };
    }
    let result = unsafe { slice::from_raw_parts(handles, count) }
        .iter()
        .try_for_each(|&handle| unsafe { mount(boot, handle) });
    let freed = unsafe { (boot.free_pool)(handles.cast()) };
    result.err().unwrap_or(freed)
}
