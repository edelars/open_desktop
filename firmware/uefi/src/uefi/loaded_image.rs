//! UEFI 2.11 §9.1 `EFI_LOADED_IMAGE_PROTOCOL`.

use super::{guid::Guid, status::Status, system_table::SystemTable, types::Handle};
use crate::{device_path::DevicePathNode, mem::map::MemoryType};
use core::ffi::c_void;

pub const LOADED_IMAGE_PROTOCOL_GUID: Guid = Guid {
    data1: 0x5b1b_31a1,
    data2: 0x9562,
    data3: 0x11d2,
    data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};
pub const LOADED_IMAGE_PROTOCOL_REVISION: u32 = 0x1000;
pub const LOADED_IMAGE_DEVICE_PATH_PROTOCOL_GUID: Guid = Guid {
    data1: 0xbc62_157e,
    data2: 0x3e33,
    data3: 0x4fec,
    data4: [0x99, 0x20, 0x2d, 0x3b, 0x36, 0xd7, 0x50, 0xdf],
};

#[repr(C)]
pub struct LoadedImageProtocol {
    pub revision: u32,
    pub parent_handle: Handle,
    pub system_table: *mut SystemTable,
    pub device_handle: Handle,
    pub file_path: *mut DevicePathNode,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: MemoryType,
    pub image_data_type: MemoryType,
    pub unload: Option<unsafe extern "efiapi" fn(Handle) -> Status>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::{offset_of, size_of};

    #[test]
    fn loaded_image_matches_uefi_x64_layout() {
        assert_eq!(size_of::<LoadedImageProtocol>(), 96);
        assert_eq!(offset_of!(LoadedImageProtocol, revision), 0);
        assert_eq!(offset_of!(LoadedImageProtocol, parent_handle), 8);
        assert_eq!(offset_of!(LoadedImageProtocol, system_table), 16);
        assert_eq!(offset_of!(LoadedImageProtocol, device_handle), 24);
        assert_eq!(offset_of!(LoadedImageProtocol, file_path), 32);
        assert_eq!(offset_of!(LoadedImageProtocol, reserved), 40);
        assert_eq!(offset_of!(LoadedImageProtocol, load_options_size), 48);
        assert_eq!(offset_of!(LoadedImageProtocol, load_options), 56);
        assert_eq!(offset_of!(LoadedImageProtocol, image_base), 64);
        assert_eq!(offset_of!(LoadedImageProtocol, image_size), 72);
        assert_eq!(offset_of!(LoadedImageProtocol, image_code_type), 80);
        assert_eq!(offset_of!(LoadedImageProtocol, image_data_type), 84);
        assert_eq!(offset_of!(LoadedImageProtocol, unload), 88);
    }
}
