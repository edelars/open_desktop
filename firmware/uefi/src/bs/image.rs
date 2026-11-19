//! Boot Services ABI while no EFI images have been loaded (UEFI 2.11 §7.4).

use crate::device_path::DevicePathNode;
use crate::uefi::{
    status::Status,
    types::{Char16, Handle},
};
use core::ffi::c_void;

/// # Safety
/// Call on the BSP. At this stage the pointers are checked for presence.
pub unsafe extern "efiapi" fn load_image(
    _boot_policy: u8,
    parent: Handle,
    device_path: *const DevicePathNode,
    source: *const c_void,
    _source_size: usize,
    image: *mut Handle,
) -> Status {
    if parent.is_null() || image.is_null() {
        Status::INVALID_PARAMETER
    } else if device_path.is_null() && source.is_null() {
        Status::NOT_FOUND
    } else {
        Status::UNSUPPORTED
    }
}

/// # Safety
/// Call on the BSP. The loader must have registered the image before start.
pub unsafe extern "efiapi" fn start_image(
    _image: Handle,
    _exit_data_size: *mut usize,
    _exit_data: *mut *mut Char16,
) -> Status {
    Status::INVALID_PARAMETER
}

/// # Safety
/// Call on the BSP. Only the currently executing loaded image can exit.
pub unsafe extern "efiapi" fn exit(
    _image: Handle,
    _exit_status: Status,
    _exit_data_size: usize,
    _exit_data: *const Char16,
) -> Status {
    Status::INVALID_PARAMETER
}

/// # Safety
/// Call on the BSP with a loaded image handle.
pub unsafe extern "efiapi" fn unload_image(_image: Handle) -> Status {
    Status::INVALID_PARAMETER
}
