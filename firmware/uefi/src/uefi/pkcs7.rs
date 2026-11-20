//! EFI_PKCS7_VERIFY_PROTOCOL, UEFI 2.11 §37.4.

use core::ffi::c_void;

use super::{guid::Guid, status::Status};

pub const PKCS7_VERIFY_PROTOCOL_GUID: Guid = Guid {
    data1: 0x4788_9fb2,
    data2: 0xd671,
    data3: 0x4fab,
    data4: [0xa0, 0xca, 0xdf, 0x0e, 0x44, 0xdf, 0x70, 0xd6],
};

pub type SignatureListArray = *const *const c_void;

#[repr(C)]
pub struct Protocol {
    pub verify_buffer: unsafe extern "efiapi" fn(
        *const Self,
        *const c_void,
        usize,
        *const c_void,
        usize,
        SignatureListArray,
        SignatureListArray,
        SignatureListArray,
        *mut c_void,
        *mut usize,
    ) -> Status,
    pub verify_signature: unsafe extern "efiapi" fn(
        *const Self,
        *const c_void,
        usize,
        *const c_void,
        usize,
        SignatureListArray,
        SignatureListArray,
        SignatureListArray,
    ) -> Status,
}
