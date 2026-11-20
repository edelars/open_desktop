//! UEFI 2.11 §8 wire types and signatures for Runtime Services.

use super::{guid::Guid, status::Status};
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
    pub(crate) _padding: [u8; 3],
}

#[repr(C)]
pub struct CapsuleHeader {
    pub capsule_guid: Guid,
    pub header_size: u32,
    pub flags: u32,
    pub capsule_image_size: u32,
}

pub const RT_PROPERTIES_TABLE_GUID: Guid = Guid {
    data1: 0xeb66_918a,
    data2: 0x7eef,
    data3: 0x402a,
    data4: [0x84, 0x2e, 0x93, 0x1d, 0x21, 0xc3, 0x8a, 0xe9],
};

#[repr(C)]
pub struct RuntimeProperties {
    pub version: u16,
    pub length: u16,
    pub supported: u32,
}

impl RuntimeProperties {
    pub(crate) const fn new(rtc: bool, monotonic: bool, reset: bool) -> Self {
        const VARIABLES: u32 = 0x0010 | 0x0020 | 0x0040 | 0x2000;
        const VIRTUAL_MEMORY: u32 = 0x0080 | 0x0100;
        const TIME: u32 = 0x0001 | 0x0002;
        const HIGH_MONOTONIC_COUNT: u32 = 0x0200;
        const RESET: u32 = 0x0400;
        Self {
            version: 1,
            length: core::mem::size_of::<Self>() as u16,
            supported: VARIABLES
                | VIRTUAL_MEMORY
                | if rtc { TIME } else { 0 }
                | if monotonic { HIGH_MONOTONIC_COUNT } else { 0 }
                | if reset { RESET } else { 0 },
        }
    }
}

pub type ResetType = u32;
pub const RESET_COLD: ResetType = 0;
pub const RESET_WARM: ResetType = 1;
pub const RESET_SHUTDOWN: ResetType = 2;
pub const RESET_PLATFORM_SPECIFIC: ResetType = 3;
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
    get_next_high_monotonic_count(_high_count: *mut u32);
    update_capsule(_capsules: *const *const CapsuleHeader, _count: usize, _scatter_gather_list: u64);
    query_capsule_capabilities(_capsules: *const *const CapsuleHeader, _count: usize, _maximum_size: *mut u64, _reset_type: *mut ResetType);
}
