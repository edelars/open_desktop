use core::ffi::c_void;

pub type Handle = *mut c_void;
pub type Char16 = u16;

/// UEFI 2.11 §4.2 measures the minor revision in decimal tenths.
pub const REVISION_2_11: u32 = (2 << 16) | 110;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct TableHeader {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

impl TableHeader {
    pub(super) const fn new(signature: u64, size: usize) -> Self {
        Self {
            signature,
            revision: REVISION_2_11,
            header_size: size as u32,
            crc32: 0,
            reserved: 0,
        }
    }
}
