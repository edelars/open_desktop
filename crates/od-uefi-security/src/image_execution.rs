//! Authentication actions for rejected images in UEFI 2.11 §32.4.

/// The authentication part of EFI_IMAGE_EXECUTION_ACTION for a rejected image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum RejectedImageAction {
    Untested = 0,
    SignatureFailed = 1,
    SignatureNotFound = 3,
    SignatureFound = 4,
    PolicyFailed = 5,
}
