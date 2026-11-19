#![no_std]
#![no_main]

use core::{
    ffi::c_void,
    mem::{offset_of, size_of},
    ptr, slice,
};

use od_firmware::{
    block::block_io::BLOCK_IO_PROTOCOL_GUID,
    bs::event::{EVT_NOTIFY_SIGNAL, EVT_TIMER, Event, TIMER_RELATIVE, TPL_CALLBACK},
    fs::simple_fs::{
        FILE_INFO_GUID, FILE_SYSTEM_INFO_GUID, FileInfo, FileProtocol, FileSystemInfo,
        SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, SimpleFileSystem,
    },
    uefi::{
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};
use od_firmware_test_apps::{ascii_to_char16, output};

const README: &[u8] = b"OD-UEFI FAT32 ESP README\r\n";
const FILE: [u16; b"\\efi\\BoOt\\readme.txt\0".len()] =
    ascii_to_char16(b"\\efi\\BoOt\\readme.txt\0");
const ABSENT: [u16; b"\\EFI\\BOOT\\ABSENT.TXT\0".len()] =
    ascii_to_char16(b"\\EFI\\BOOT\\ABSENT.TXT\0");

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    if unsafe { verify(system) }.is_some() {
        Status::SUCCESS
    } else {
        Status::LOAD_ERROR
    }
}

fn ok(status: Status) -> Option<()> {
    (status == Status::SUCCESS).then_some(())
}

unsafe fn verify(system: *mut SystemTable) -> Option<()> {
    let boot = unsafe { (*system).boot_services.as_ref() }?;
    let mut count = 0;
    let mut handles = ptr::null_mut::<Handle>();
    ok(unsafe {
        (boot.locate_handle_buffer)(
            2,
            &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut count,
            &mut handles,
        )
    })?;
    let handle = if count == 1 && !handles.is_null() {
        unsafe { handles.read() }
    } else {
        ptr::null_mut()
    };
    ok(unsafe { (boot.free_pool)(handles.cast()) })?;
    if handle.is_null() {
        return None;
    }
    let mut interface = ptr::null_mut();
    ok(unsafe { (boot.handle_protocol)(handle, &BLOCK_IO_PROTOCOL_GUID, &mut interface) })?;
    if interface.is_null() {
        return None;
    }
    ok(unsafe {
        (boot.handle_protocol)(handle, &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, &mut interface)
    })?;
    let fs = interface.cast::<SimpleFileSystem>();
    let proto = unsafe { fs.as_ref() }?;
    if proto.revision != 0x10000 {
        return None;
    }
    let mut root = ptr::null_mut();
    ok(unsafe { (proto.open_volume)(fs, &mut root) })?;
    let directory = unsafe { root.as_ref() }?;
    unsafe { check_directory(root) }?;

    let mut file = ptr::null_mut();
    if unsafe { (directory.open)(root, &mut file, ABSENT.as_ptr(), 1, 0) } != Status::NOT_FOUND
        || unsafe { (directory.open)(root, &mut file, FILE.as_ptr(), 3, 0) }
            != Status::WRITE_PROTECTED
        || !file.is_null()
    {
        return None;
    }
    ok(unsafe { (directory.open)(root, &mut file, FILE.as_ptr(), 1, 0) })?;
    unsafe { check_position_timer(boot, file) }?;
    unsafe { check_info(file) }?;
    unsafe { read_readme(system, file) }?;
    if unsafe { ((*file).delete)(file) } != Status::WARN_DELETE_FAILURE {
        return None;
    }

    unsafe { check_reentrant_close(boot, root) }?;
    ok(unsafe { (directory.close)(root) })?;
    const CHECKED: &[u8] = b"OD-UEFI fs protocol and read-only status verified\r\n\0";
    let line = ascii_to_char16::<{ CHECKED.len() }>(CHECKED);
    ok(unsafe { output(system, line.as_ptr()) })
}

unsafe fn check_directory(root: *mut FileProtocol) -> Option<()> {
    let directory = unsafe { &*root };
    let mut size = 0;
    if unsafe { (directory.read)(root, &mut size, ptr::null_mut()) } != Status::BUFFER_TOO_SMALL
        || size != size_of::<FileInfo>() + 8
    {
        return None;
    }
    let mut buffer = [0; 96];
    ok(unsafe { (directory.read)(root, &mut size, buffer.as_mut_ptr().cast()) })?;
    let info = unsafe { buffer.as_ptr().cast::<FileInfo>().read_unaligned() };
    let name = &buffer[size_of::<FileInfo>()..size];
    let mut pos = u64::MAX;
    if info.attribute & 0x10 == 0
        || name != b"E\0F\0I\0\0\0"
        || unsafe { (directory.get_position)(root, &mut pos) } != Status::UNSUPPORTED
        || unsafe { (directory.set_position)(root, 1) } != Status::UNSUPPORTED
    {
        return None;
    }
    ok(unsafe { (directory.set_position)(root, 0) })
}

unsafe fn check_info(file: *mut FileProtocol) -> Option<()> {
    let protocol = unsafe { file.as_ref() }?;
    let mut size = 0;
    if protocol.revision != 0x10000
        || unsafe { (protocol.get_info)(file, &FILE_INFO_GUID, &mut size, ptr::null_mut()) }
            != Status::BUFFER_TOO_SMALL
        || size != size_of::<FileInfo>() + b"README.TXT\0".len() * 2
    {
        return None;
    }
    let mut buffer = [0; 128];
    size = buffer.len();
    ok(unsafe {
        (protocol.get_info)(file, &FILE_INFO_GUID, &mut size, buffer.as_mut_ptr().cast())
    })?;
    let info = unsafe { buffer.as_ptr().cast::<FileInfo>().read_unaligned() };
    if info.file_size != README.len() as u64
        || info.physical_size != 512
        || info.attribute != 0x20
        || &buffer[size_of::<FileInfo>()..size] != b"R\0E\0A\0D\0M\0E\0.\0T\0X\0T\0\0\0"
        || unsafe { (protocol.set_info)(file, &FILE_INFO_GUID, size, buffer.as_ptr().cast()) }
            != Status::WRITE_PROTECTED
        || unsafe { (protocol.write)(file, &mut size, buffer.as_ptr().cast()) }
            != Status::WRITE_PROTECTED
        || unsafe { (protocol.flush)(file) } != Status::WRITE_PROTECTED
    {
        return None;
    }
    size = buffer.len();
    ok(unsafe {
        (protocol.get_info)(
            file,
            &FILE_SYSTEM_INFO_GUID,
            &mut size,
            buffer.as_mut_ptr().cast(),
        )
    })?;
    let fs_info = unsafe { buffer.as_ptr().cast::<FileSystemInfo>().read_unaligned() };
    if fs_info.read_only != 1
        || fs_info.block_size != 512
        || fs_info.free_space == 0
        || fs_info.free_space >= fs_info.volume_size
        || &buffer[offset_of!(FileSystemInfo, volume_label)..size] != b"O\0D\0 \0E\0S\0P\0\0\0"
    {
        return None;
    }
    Some(())
}

unsafe fn read_readme(system: *mut SystemTable, file: *mut FileProtocol) -> Option<()> {
    let protocol = unsafe { &*file };
    let mut buffer = [0u8; 128];
    let mut size = buffer.len();
    ok(unsafe { (protocol.read)(file, &mut size, buffer.as_mut_ptr().cast()) })?;
    if &buffer[..size] != README {
        return None;
    }
    let mut output_line = [0u16; 129];
    for (destination, &source) in output_line.iter_mut().zip(&buffer[..size]) {
        *destination = u16::from(source);
    }
    ok(unsafe { output(system, output_line.as_ptr()) })?;
    ok(unsafe { (protocol.set_position)(file, u64::MAX) })?;
    let mut position = 0;
    ok(unsafe { (protocol.get_position)(file, &mut position) })?;
    if position != README.len() as u64 {
        return None;
    }
    size = buffer.len();
    ok(unsafe { (protocol.read)(file, &mut size, buffer.as_mut_ptr().cast()) })?;
    if size != 0 {
        return None;
    }
    ok(unsafe { (protocol.set_position)(file, position + 1) })?;
    size = 1;
    (unsafe { (protocol.read)(file, &mut size, buffer.as_mut_ptr().cast()) }
        == Status::DEVICE_ERROR)
        .then_some(())
}

unsafe fn check_position_timer(boot: &BootServices, file: *mut FileProtocol) -> Option<()> {
    let mut fired = false;
    let mut event = ptr::null_mut();
    ok(unsafe {
        (boot.create_event)(
            EVT_TIMER | EVT_NOTIFY_SIGNAL,
            TPL_CALLBACK,
            Some(mark_timer),
            ptr::addr_of_mut!(fired).cast(),
            &mut event,
        )
    })?;
    let result = (|| {
        ok(unsafe { (boot.set_timer)(event, TIMER_RELATIVE, 200) })?;
        for _ in 0..1024 {
            let mut position = u64::MAX;
            ok(unsafe { ((*file).get_position)(file, &mut position) })?;
            if position != 0 {
                return None;
            }
            if unsafe { ptr::addr_of!(fired).read_volatile() } {
                return Some(());
            }
        }
        None
    })();
    ok(unsafe { (boot.close_event)(event) })?;
    result
}

unsafe extern "efiapi" fn mark_timer(_: Event, context: *mut c_void) {
    unsafe { context.cast::<bool>().write_volatile(true) };
}

struct CloseProbe {
    file: *mut FileProtocol,
    status: Status,
}

unsafe fn check_reentrant_close(boot: &BootServices, root: *mut FileProtocol) -> Option<()> {
    // The timer can expire at File.Read entry, during disk I/O, or after the call.
    // Require one successful read in which the callback closed the active handle.
    for delay in [1, 2, 3, 4, 6, 8, 12, 16, 24, 32, 48, 64] {
        let mut file = ptr::null_mut();
        ok(unsafe { ((*root).open)(root, &mut file, FILE.as_ptr(), 1, 0) })?;
        if unsafe { try_reentrant_close(boot, file, delay) }? {
            return Some(());
        }
    }
    None
}

unsafe fn try_reentrant_close(
    boot: &BootServices,
    file: *mut FileProtocol,
    delay: u64,
) -> Option<bool> {
    let mut probe = CloseProbe {
        file,
        status: Status::NOT_READY,
    };
    let mut event = ptr::null_mut();
    let context = ptr::addr_of_mut!(probe).cast::<c_void>();
    ok(unsafe {
        (boot.create_event)(
            EVT_TIMER | EVT_NOTIFY_SIGNAL,
            TPL_CALLBACK,
            Some(close_during_read),
            context,
            &mut event,
        )
    })?;
    let mut buffer = [0u8; 32];
    let mut size = buffer.len();
    let read = unsafe { (*file).read };
    let armed = unsafe { (boot.set_timer)(event, TIMER_RELATIVE, delay) };
    let before_read = unsafe { ptr::addr_of!(probe.status).read_volatile() };
    let read_status = if armed == Status::SUCCESS && before_read == Status::NOT_READY {
        unsafe { read(file, &mut size, buffer.as_mut_ptr().cast()) }
    } else {
        Status::NOT_READY
    };
    ok(unsafe { (boot.close_event)(event) })?;
    let close_status = unsafe { ptr::addr_of!(probe.status).read_volatile() };
    if close_status == Status::NOT_READY {
        ok(unsafe { ((*file).close)(file) })?;
    }
    ok(armed)?;
    match (read_status, close_status) {
        (Status::SUCCESS, Status::SUCCESS | Status::NOT_READY)
            if buffer.get(..size) == Some(README) =>
        {
            Some(close_status == Status::SUCCESS)
        }
        (Status::NOT_READY, Status::SUCCESS) => Some(false),
        _ => None,
    }
}

unsafe extern "efiapi" fn close_during_read(_: Event, context: *mut c_void) {
    let context = context.cast::<CloseProbe>();
    let file = unsafe { (*context).file };
    unsafe { ptr::addr_of_mut!((*context).status).write_volatile(((*file).close)(file)) };
}
