#![no_std]
#![no_main]

use core::{ptr, slice};
use od_firmware::{
    device_path::DevicePathNode,
    fs::simple_fs::SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
    uefi::{
        loaded_image::{
            LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID, LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol,
        },
        status::Status,
        system_table::SystemTable,
        types::Handle,
    },
    var::services::{BOOT_RUNTIME, GLOBAL_VARIABLE, NON_VOLATILE_RUNTIME},
};
use od_firmware_test_apps::{ascii_to_char16, output};

const BOOT_CURRENT: [u16; 12] = ascii_to_char16(b"BootCurrent\0");
const BOOT_NEXT: [u16; 9] = ascii_to_char16(b"BootNext\0");
const BOOT_ORDER: [u16; 10] = ascii_to_char16(b"BootOrder\0");
const FILE: &[u8] = b"\\EFI\\BOOT\\CHECK.EFI\0";
const NEXT: &[u8] = b"OD-UEFI ESP BootNext, BootCurrent and LoadOptions verified\r\n\0";
const ORDER: &[u8] = b"OD-UEFI ESP BootOrder, BootCurrent and LoadOptions verified\r\n\0";
static NEXT_MESSAGE: [u16; NEXT.len()] = ascii_to_char16(NEXT);
static ORDER_MESSAGE: [u16; ORDER.len()] = ascii_to_char16(ORDER);

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(image: Handle, system: *mut SystemTable) -> Status {
    const ERROR: Status = Status::LOAD_ERROR;
    let Some((next, number)) = (unsafe { verify(image, system) }) else {
        return ERROR;
    };
    if next && number == 10 {
        let next_boot_order = [12u16, 13, 11, 14];
        let runtime = unsafe { &*(*system).runtime_services };
        let saved = unsafe {
            (runtime.set_variable)(
                BOOT_ORDER.as_ptr(),
                &GLOBAL_VARIABLE,
                NON_VOLATILE_RUNTIME,
                size_of_val(&next_boot_order),
                next_boot_order.as_ptr().cast(),
            )
        };
        if saved != Status::SUCCESS {
            return ERROR;
        }
        unsafe { output(system, NEXT_MESSAGE.as_ptr()) };
        // The manager must continue to BootOrder after an application error.
        ERROR
    } else if !next && number == 11 {
        // Success with a return to the headless manager also leaves boot pending.
        unsafe { output(system, ORDER_MESSAGE.as_ptr()) }
    } else {
        ERROR
    }
}

unsafe fn verify(image: Handle, system: *mut SystemTable) -> Option<(bool, u16)> {
    let table = unsafe { system.as_ref() }?;
    let boot = unsafe { table.boot_services.as_ref() }?;
    let runtime = unsafe { table.runtime_services.as_ref() }?;
    let mut info = ptr::null_mut();
    if unsafe { (boot.handle_protocol)(image, &LOADED_IMAGE_PROTOCOL_GUID, &mut info) }
        != Status::SUCCESS
    {
        return None;
    }
    let loaded = unsafe { info.cast::<LoadedImageProtocol>().as_ref() }?;
    if !loaded.parent_handle.is_null()
        || loaded.system_table != system
        || loaded.device_handle.is_null()
        || loaded.load_options.is_null()
        || !matches!(loaded.load_options_size, 4 | 5)
        || !unsafe { is_file_path(loaded.file_path) }
    {
        return None;
    }
    let options = unsafe {
        slice::from_raw_parts(
            loaded.load_options.cast::<u8>(),
            loaded.load_options_size as usize,
        )
    };
    let next = match options {
        b"next" => true,
        b"order" => false,
        _ => return None,
    };
    let mut path = ptr::null_mut();
    let mut device = ptr::null_mut();
    if unsafe { (boot.handle_protocol)(image, &LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID, &mut path) }
        != Status::SUCCESS
    {
        return None;
    }
    let mut path = path.cast::<DevicePathNode>();
    if unsafe {
        (boot.locate_device_path)(&SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, &mut path, &mut device)
    } != Status::SUCCESS
        || device != loaded.device_handle
        || !unsafe { is_file_path(path) }
    {
        return None;
    }
    let mut number = 0u16;
    let mut size = size_of::<u16>();
    let mut attributes = 0;
    let current = unsafe {
        (runtime.get_variable)(
            BOOT_CURRENT.as_ptr(),
            &GLOBAL_VARIABLE,
            &mut attributes,
            &mut size,
            ptr::addr_of_mut!(number).cast(),
        )
    };
    if current != Status::SUCCESS || size != 2 || attributes != BOOT_RUNTIME {
        return None;
    }
    size = 0;
    let pending = unsafe {
        (runtime.get_variable)(
            BOOT_NEXT.as_ptr(),
            &GLOBAL_VARIABLE,
            ptr::null_mut(),
            &mut size,
            ptr::null_mut(),
        )
    };
    (pending == Status::NOT_FOUND).then_some((next, number))
}

unsafe fn is_file_path(path: *const DevicePathNode) -> bool {
    let Some(path) = (unsafe { path.as_ref() }) else {
        return false;
    };
    let length = 4 + 2 * FILE.len();
    if path.node_type != 4
        || path.subtype != 4
        || usize::from(u16::from_le_bytes(path.length)) != length
    {
        return false;
    }
    let bytes = unsafe { slice::from_raw_parts(ptr::from_ref(path).cast::<u8>(), length + 4) };
    bytes[length..] == [0x7f, 0xff, 4, 0]
        && FILE
            .iter()
            .zip(bytes[4..length].chunks_exact(2))
            .all(|(&c, unit)| unit == [c, 0])
}
