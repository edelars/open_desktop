#![no_std]
#![no_main]

use core::{ffi::c_void, ptr};
use od_firmware::mem::map::MemoryType;
use od_firmware::uefi::loaded_image::{
    LOADED_IMAGE_PROTOCOL_GUID, LOADED_IMAGE_PROTOCOL_REVISION, LoadedImageProtocol,
};
use od_firmware::uefi::{
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware_test_apps::{ascii_to_char16, output};

const TEXT: &str = concat!("OD-UEFI hello ", env!("CARGO_PKG_VERSION"), "\r\n\0");
static MESSAGE: [Char16; TEXT.len()] = ascii_to_char16(TEXT.as_bytes());
// Keep one absolute pointer in the PE so the same app can exercise DIR64.
static MESSAGE_ADDRESS: &[Char16; TEXT.len()] = &MESSAGE;

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let boot_services = unsafe { (*system).boot_services };
    if boot_services.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let message = unsafe { ptr::read_volatile(&MESSAGE_ADDRESS) };
    let status = unsafe { output(system, message.as_ptr()) };
    if status.is_error() {
        return status;
    }

    let mut interface: *mut c_void = ptr::null_mut();
    let status = unsafe {
        ((*boot_services).handle_protocol)(handle, &LOADED_IMAGE_PROTOCOL_GUID, &mut interface)
    };
    if status.is_error() {
        return status;
    }
    let Some(image) = (unsafe { interface.cast::<LoadedImageProtocol>().as_ref() }) else {
        return Status::LOAD_ERROR;
    };
    if image.revision != LOADED_IMAGE_PROTOCOL_REVISION
        || image.system_table != system
        || image.image_base.is_null()
        || image.image_size == 0
        || image.image_code_type != MemoryType::LOADER_CODE
        || image.image_data_type != MemoryType::LOADER_DATA
    {
        return Status::LOAD_ERROR;
    }
    Status::SUCCESS
}
