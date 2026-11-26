//! Synchronous revision 1 EFI file handles backed by a read-only FAT volume.

use core::{
    cell::Cell,
    ffi::c_void,
    mem::size_of,
    ptr::{self, NonNull},
    slice,
};

use crate::{
    bs::{
        boot_services_active,
        memory::{allocate_pool, free_pool},
    },
    fat::{DirectoryCursor, Entry},
    mem::map::MemoryType,
    uefi::{guid::Guid, status::Status},
};

use super::{
    info::{
        FILE_INFO_GUID, FILE_SYSTEM_INFO_GUID, check_buffer, file_info, file_info_size,
        file_system_info,
    },
    simple_fs::{Mounted, check},
};

const READ_ONLY: u64 = 1;
const READ_WRITE: u64 = 3;
const CREATE: u64 = 0x8000_0000_0000_0003;
const MAX_PATH_UNITS: usize = 32 * 1024;

#[repr(C)]
pub struct FileProtocol {
    pub revision: u64,
    pub open: unsafe extern "efiapi" fn(*mut Self, *mut *mut Self, *const u16, u64, u64) -> Status,
    pub close: unsafe extern "efiapi" fn(*mut Self) -> Status,
    pub delete: unsafe extern "efiapi" fn(*mut Self) -> Status,
    pub read: unsafe extern "efiapi" fn(*mut Self, *mut usize, *mut c_void) -> Status,
    pub write: unsafe extern "efiapi" fn(*mut Self, *mut usize, *const c_void) -> Status,
    pub get_position: unsafe extern "efiapi" fn(*mut Self, *mut u64) -> Status,
    pub set_position: unsafe extern "efiapi" fn(*mut Self, u64) -> Status,
    pub get_info:
        unsafe extern "efiapi" fn(*mut Self, *const Guid, *mut usize, *mut c_void) -> Status,
    pub set_info: unsafe extern "efiapi" fn(*mut Self, *const Guid, usize, *const c_void) -> Status,
    pub flush: unsafe extern "efiapi" fn(*mut Self) -> Status,
}

#[repr(C)]
struct File {
    protocol: FileProtocol,
    mounted: *const Mounted,
    entry: Entry,
    allocation: u64,
    position: Cell<u64>,
    /// S7-32: the chain position the last read left (see [`crate::fat::ChainCursor`]).
    cursor: Cell<Option<crate::fat::ChainCursor>>,
    active: Cell<bool>,
    closing: Cell<bool>,
}

impl File {
    fn mounted(&self) -> &Mounted {
        // SAFETY: the ESP mount record remains pinned for the whole Boot Services lifetime.
        unsafe { &*self.mounted }
    }
}

pub(super) fn new_handle(mounted: &Mounted, entry: Entry) -> Result<*mut FileProtocol, Status> {
    let allocation = mounted
        .volume
        .allocation_bytes(&mut { mounted.disk }, &entry)?;
    let mut pointer = ptr::null_mut();
    check(unsafe {
        allocate_pool(
            MemoryType::BOOT_SERVICES_DATA,
            size_of::<File>(),
            &mut pointer,
        )
    })?;
    let file = pointer.cast::<File>();
    unsafe {
        file.write(File {
            protocol: FileProtocol {
                revision: 0x0001_0000,
                open,
                close,
                delete,
                read,
                write,
                get_position,
                set_position,
                get_info,
                set_info,
                flush,
            },
            mounted,
            entry,
            allocation,
            position: Cell::new(0),
            cursor: Cell::new(None),
            active: Cell::new(false),
            closing: Cell::new(false),
        });
    }
    Ok(file.cast())
}

struct Operation(NonNull<File>);

impl Drop for Operation {
    fn drop(&mut self) {
        let file = self.0.as_ptr();
        let close = unsafe {
            (*file).active.set(false);
            (*file).closing.get()
        };
        if close {
            // The operation held the last live borrow. Close may have run during Disk I/O.
            unsafe { free_pool(file.cast()) };
        }
    }
}

unsafe fn file_pointer(this: *mut FileProtocol) -> Result<NonNull<File>, Status> {
    let pointer = NonNull::new(this.cast()).ok_or(Status::INVALID_PARAMETER)?;
    if !unsafe { boot_services_active() } {
        return Err(Status::NOT_READY);
    }
    Ok(pointer)
}

unsafe fn with_file<T>(
    this: *mut FileProtocol,
    action: impl FnOnce(&File) -> Result<T, Status>,
) -> Result<T, Status> {
    let pointer = unsafe { file_pointer(this) }?;
    let file = unsafe { pointer.as_ref() };
    if file.closing.get() || file.active.replace(true) {
        return Err(Status::NOT_READY);
    }
    let _operation = Operation(pointer);
    // A notification can close this handle, so keep it pinned across the poll.
    #[cfg(target_os = "none")]
    unsafe {
        crate::bs::event::poll_due_events()
    };
    if file.closing.get() || !unsafe { boot_services_active() } {
        return Err(Status::NOT_READY);
    }
    action(file)
}

unsafe fn path<'a>(name: *const u16) -> Result<&'a [u16], Status> {
    if name.is_null() || !name.is_aligned() || name.addr().checked_add(MAX_PATH_UNITS * 2).is_none()
    {
        return Err(Status::INVALID_PARAMETER);
    }
    for index in 0..MAX_PATH_UNITS {
        let unit = unsafe { name.add(index).read() };
        if unit == 0 {
            return Ok(unsafe { slice::from_raw_parts(name, index) });
        }
        if char::from_u32(u32::from(unit)).is_none() {
            return Err(Status::INVALID_PARAMETER);
        }
    }
    Err(Status::INVALID_PARAMETER)
}

unsafe extern "efiapi" fn open(
    this: *mut FileProtocol,
    out: *mut *mut FileProtocol,
    filename: *const u16,
    mode: u64,
    _attributes: u64,
) -> Status {
    let result = unsafe {
        with_file(this, |file| {
            if out.is_null() || filename.is_null() {
                return Err(Status::INVALID_PARAMETER);
            }
            match mode {
                READ_ONLY => (),
                READ_WRITE | CREATE => return Err(Status::WRITE_PROTECTED),
                _ => return Err(Status::INVALID_PARAMETER),
            }
            let path = path(filename)?;
            let mounted = file.mounted();
            let entry = mounted
                .volume
                .open_path(&mut { mounted.disk }, &file.entry, path)?;
            out.write(new_handle(mounted, entry)?);
            Ok(())
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn close(this: *mut FileProtocol) -> Status {
    let pointer = match unsafe { file_pointer(this) } {
        Ok(pointer) => pointer,
        Err(error) => return error,
    };
    let now = unsafe {
        let file = pointer.as_ref();
        if file.closing.replace(true) {
            return Status::NOT_READY;
        }
        !file.active.get()
    };
    if now {
        unsafe { free_pool(pointer.as_ptr().cast()) }
    } else {
        Status::SUCCESS
    }
}

unsafe extern "efiapi" fn delete(this: *mut FileProtocol) -> Status {
    let status = unsafe { close(this) };
    if status == Status::SUCCESS {
        Status::WARN_DELETE_FAILURE
    } else {
        status
    }
}

unsafe extern "efiapi" fn read(
    this: *mut FileProtocol,
    size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    let result = unsafe {
        with_file(this, |file| {
            if size.is_null() {
                return Err(Status::INVALID_PARAMETER);
            }
            let mounted = file.mounted();
            let mut disk = mounted.disk;
            let volume = &mounted.volume;
            let position = file.position.get();
            if file.entry.is_directory() {
                let directory = volume.directory(&file.entry)?;
                let mut entries = DirectoryCursor::new(volume, &mut disk, directory, position)?;
                while let Some(entry) = entries.next(volume, &mut disk)? {
                    if entry.is_label() {
                        continue;
                    }
                    let needed = file_info_size(&entry);
                    check_buffer(size, buffer, needed)?;
                    file_info(&entry, volume.allocation_bytes(&mut disk, &entry)?, buffer);
                    size.write(needed);
                    file.position.set(entries.position());
                    return Ok(());
                }
                size.write(0);
                return Ok(());
            }
            let len = u64::from(file.entry.len);
            if position > len {
                return Err(Status::DEVICE_ERROR);
            }
            let amount = size.read().min((len - position) as usize);
            if amount == 0 {
                size.write(0);
                return Ok(());
            }
            if buffer.is_null() || buffer.addr().checked_add(amount).is_none() {
                return Err(Status::INVALID_PARAMETER);
            }
            let out = slice::from_raw_parts_mut(buffer.cast::<u8>(), amount);
            let mut cursor = file.cursor.get();
            let copied = volume.read_file(&mut disk, &file.entry, position, out, &mut cursor)?;
            file.cursor.set(cursor);
            file.position.set(position + copied as u64);
            size.write(copied);
            Ok(())
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn write(
    this: *mut FileProtocol,
    _size: *mut usize,
    _buffer: *const c_void,
) -> Status {
    unsafe { flush(this) }
}

unsafe extern "efiapi" fn get_position(this: *mut FileProtocol, out: *mut u64) -> Status {
    let result = unsafe {
        with_file(this, |file| {
            if out.is_null() {
                return Err(Status::INVALID_PARAMETER);
            }
            if file.entry.is_directory() {
                return Err(Status::UNSUPPORTED);
            }
            out.write(file.position.get());
            Ok(())
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn set_position(this: *mut FileProtocol, position: u64) -> Status {
    let result = unsafe {
        with_file(this, |file| {
            if file.entry.is_directory() && position != 0 {
                return Err(Status::UNSUPPORTED);
            }
            let position = if position == u64::MAX {
                u64::from(file.entry.len)
            } else {
                position
            };
            file.position.set(position);
            Ok(())
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn get_info(
    this: *mut FileProtocol,
    kind: *const Guid,
    size: *mut usize,
    buffer: *mut c_void,
) -> Status {
    let result = unsafe {
        with_file(this, |file| {
            if kind.is_null() || size.is_null() {
                return Err(Status::INVALID_PARAMETER);
            }
            match kind.read_unaligned() {
                FILE_INFO_GUID => {
                    let needed = file_info_size(&file.entry);
                    check_buffer(size, buffer, needed)?;
                    file_info(&file.entry, file.allocation, buffer);
                    size.write(needed);
                    Ok(())
                }
                FILE_SYSTEM_INFO_GUID => file_system_info(file.mounted(), size, buffer),
                _ => Err(Status::UNSUPPORTED),
            }
        })
    };
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn set_info(
    this: *mut FileProtocol,
    kind: *const Guid,
    _size: usize,
    _buffer: *const c_void,
) -> Status {
    unsafe {
        with_file(this, |_| {
            if kind.is_null() {
                return Err::<(), _>(Status::INVALID_PARAMETER);
            }
            match kind.read_unaligned() {
                FILE_INFO_GUID | FILE_SYSTEM_INFO_GUID => Err(Status::WRITE_PROTECTED),
                _ => Err(Status::UNSUPPORTED),
            }
        })
    }
    .err()
    .unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn flush(this: *mut FileProtocol) -> Status {
    unsafe { with_file(this, |_| Err::<(), _>(Status::WRITE_PROTECTED)) }
        .err()
        .unwrap_or(Status::SUCCESS)
}
