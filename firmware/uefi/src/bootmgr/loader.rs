//! Start a single boot option through the existing image service.

use super::{
    char16,
    policy::{DEFAULT_NODE, boot_name, set_variable},
};
use crate::bs::{image::load_boot_option, memory::BootPool};
use crate::device_path::{END_PATH_BYTES, path_bytes};
use crate::mem::map::MemoryType;
use crate::tpm::measure;
use crate::uefi::{
    loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
    status::Status,
    system_table::{BootServices, RuntimeServices},
    types::Handle,
};
use crate::var::services::BOOT_RUNTIME;
use core::ptr;

const BOOT_CURRENT: [u16; 12] = char16(b"BootCurrent\0");
const WATCHDOG_SECONDS: usize = 300;
const WATCHDOG_CODE: u64 = 0x1_0000;

pub(super) enum Started {
    Default(Handle),
    Other,
}

pub(super) unsafe fn launch(
    boot: &BootServices,
    runtime: &RuntimeServices,
    number: u16,
    path: &[u8],
    options: &[u8],
) -> Option<Started> {
    let write =
        unsafe { set_variable(runtime, &BOOT_CURRENT, BOOT_RUNTIME, &number.to_le_bytes()) };
    if write != Status::SUCCESS {
        return None;
    }
    let entered = unsafe { load_and_start(boot, number, path, options) };
    unsafe { set_variable(runtime, &BOOT_CURRENT, 0, &[]) };
    entered
}

unsafe fn load_and_start(
    boot: &BootServices,
    number: u16,
    path: &[u8],
    options: &[u8],
) -> Option<Started> {
    if measure::action(4, b"Calling EFI Application from Boot Option").is_err()
        || measure::booting_to(&boot_name(number)).is_err()
        || unsafe { measure::ready_to_boot() }.is_err()
    {
        return None;
    }
    let mut image = ptr::null_mut();
    let status = unsafe { load_boot_option(path.as_ptr().cast(), &mut image) };
    if status != Status::SUCCESS {
        return None;
    }
    let entered = unsafe { start_application(boot, image, options) };
    // StartImage normally releases a returning application. This also handles
    // a failure before entry, where the image can still be registered.
    unsafe { (boot.unload_image)(image) };
    entered
}

unsafe fn start_application(boot: &BootServices, image: Handle, options: &[u8]) -> Option<Started> {
    let mut pointer = ptr::null_mut();
    let status =
        unsafe { (boot.handle_protocol)(image, &LOADED_IMAGE_PROTOCOL_GUID, &mut pointer) };
    if status != Status::SUCCESS {
        return None;
    }
    let loaded = pointer.cast::<LoadedImageProtocol>();
    if loaded.is_null() || unsafe { (*loaded).image_code_type } != MemoryType::LOADER_CODE {
        return None;
    }
    let options_buffer = if options.is_empty() {
        None
    } else {
        let buffer = match unsafe { BootPool::allocate(options.len()) } {
            Ok(buffer) => buffer,
            Err(_) => return None,
        };
        unsafe { ptr::copy_nonoverlapping(options.as_ptr(), buffer.as_ptr(), options.len()) };
        Some(buffer)
    };
    let watchdog =
        unsafe { (boot.set_watchdog_timer)(WATCHDOG_SECONDS, WATCHDOG_CODE, 0, ptr::null()) };
    if !matches!(watchdog, Status::SUCCESS | Status::UNSUPPORTED) {
        return None;
    }
    if let Some(buffer) = &options_buffer {
        unsafe {
            (*loaded).load_options_size = options.len() as u32;
            (*loaded).load_options = buffer.as_ptr().cast();
        }
    }
    let started = if options.is_empty()
        && unsafe { path_bytes((*loaded).file_path) }
            .and_then(|path| path.strip_suffix(&END_PATH_BYTES))
            == Some(&DEFAULT_NODE)
    {
        Started::Default(unsafe { (*loaded).device_handle })
    } else {
        Started::Other
    };
    unsafe { (boot.start_image)(image, ptr::null_mut(), ptr::null_mut()) };
    let _ = measure::action(4, b"Returning from EFI Application from Boot Option");
    if options_buffer.is_some() {
        let mut live = ptr::null_mut();
        let lookup =
            unsafe { (boot.handle_protocol)(image, &LOADED_IMAGE_PROTOCOL_GUID, &mut live) };
        if lookup == Status::SUCCESS && live == loaded.cast() {
            unsafe {
                (*loaded).load_options_size = 0;
                (*loaded).load_options = ptr::null_mut();
            }
        }
    }
    if watchdog == Status::SUCCESS {
        unsafe { (boot.set_watchdog_timer)(0, WATCHDOG_CODE, 0, ptr::null()) };
    }
    Some(started)
}
