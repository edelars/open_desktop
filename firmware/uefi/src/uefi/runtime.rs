//! UEFI 2.11 §8 wire signatures for runtime slots in the private boot table.

use super::{guid::Guid, status::Status, types::Char16};
use crate::mem::map::MemoryDescriptor;
use core::ffi::c_void;

#[repr(C)]
pub struct Time {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub pad1: u8,
    pub nanosecond: u32,
    pub time_zone: i16,
    pub daylight: u8,
    pub pad2: u8,
}

#[repr(C)]
pub struct TimeCapabilities {
    pub resolution: u32,
    pub accuracy: u32,
    pub sets_to_zero: u8,
    _padding: [u8; 3],
}

#[repr(C)]
pub struct CapsuleHeader {
    pub capsule_guid: Guid,
    pub header_size: u32,
    pub flags: u32,
    pub capsule_image_size: u32,
}

pub type ResetType = u32;
pub const RESET_COLD: ResetType = 0;
pub const RESET_WARM: ResetType = 1;
pub type ResetSystem = unsafe extern "efiapi" fn(ResetType, Status, usize, *const c_void);

macro_rules! unsupported_services {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {
        $(
            pub(super) unsafe extern "efiapi" fn $name($($arg: $ty),*) -> Status {
                Status::UNSUPPORTED
            }
        )*
    };
}

unsupported_services! {
    get_time(_time: *mut Time, _capabilities: *mut TimeCapabilities);
    set_time(_time: *const Time);
    get_wakeup_time(_enabled: *mut u8, _pending: *mut u8, _time: *mut Time);
    set_wakeup_time(_enable: u8, _time: *const Time);
    set_virtual_address_map(_size: usize, _descriptor_size: usize, _version: u32, _map: *const MemoryDescriptor);
    convert_pointer(_disposition: usize, _address: *mut *mut c_void);
    get_variable(_name: *const Char16, _guid: *const Guid, _attributes: *mut u32, _size: *mut usize, _data: *mut c_void);
    get_next_variable_name(_size: *mut usize, _name: *mut Char16, _guid: *mut Guid);
    set_variable(_name: *const Char16, _guid: *const Guid, _attributes: u32, _size: usize, _data: *const c_void);
    get_next_high_monotonic_count(_high_count: *mut u32);
    update_capsule(_capsules: *const *const CapsuleHeader, _count: usize, _scatter_gather_list: u64);
    query_capsule_capabilities(_capsules: *const *const CapsuleHeader, _count: usize, _maximum_size: *mut u64, _reset_type: *mut ResetType);
    query_variable_info(_attributes: u32, _maximum_storage: *mut u64, _remaining_storage: *mut u64, _maximum_size: *mut u64);
}
