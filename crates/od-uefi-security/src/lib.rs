//! Secure Boot wire formats shared by firmware and the host NVRAM boundary.

#![cfg_attr(not(test), no_std)]

#[cfg(feature = "verify")]
extern crate alloc;

pub mod authvar;
pub mod pkcs7;
pub mod siglist;
#[cfg(feature = "verify")]
pub mod verify;

/// The mixed-width layout of EFI_GUID from UEFI 2.11 §2.3.1.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(C)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub const MAX_NAME_UNITS: usize = 127;

/// A nonempty, bounded UCS-2 name represented as little-endian bytes without NUL.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Name<'a>(&'a [u8]);

impl<'a> Name<'a> {
    pub fn new(bytes: &'a [u8]) -> Option<Self> {
        if bytes.is_empty() || bytes.len() % 2 != 0 || bytes.len() > MAX_NAME_UNITS * 2 {
            return None;
        }
        let name = Self(bytes);
        name.units().all(valid_name_unit).then_some(name)
    }

    pub fn units(self) -> impl Iterator<Item = u16> + 'a {
        self.0
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }
}

pub const fn valid_name_unit(unit: u16) -> bool {
    unit != 0 && !(unit >= 0xd800 && unit <= 0xdfff)
}
