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

impl RejectedImageAction {
    pub const fn from_raw(value: u32) -> Option<Self> {
        Some(match value {
            0 => Self::Untested,
            1 => Self::SignatureFailed,
            3 => Self::SignatureNotFound,
            4 => Self::SignatureFound,
            5 => Self::PolicyFailed,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RejectedImage {
    pub action: RejectedImageAction,
    /// The SHA-256 Authenticode digest of the checked PE, when available.
    pub digest: Option<[u8; 32]>,
}

impl RejectedImage {
    pub const POLICY_FAILURE: Self = Self {
        action: RejectedImageAction::PolicyFailed,
        digest: None,
    };
}
