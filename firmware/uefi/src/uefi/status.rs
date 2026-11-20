/// Native-width EFI_STATUS. Warnings have the high bit clear.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct Status(usize);

impl Status {
    pub const SUCCESS: Self = Self(0);
    pub const WARN_UNKNOWN_GLYPH: Self = Self(1);
    pub const WARN_DELETE_FAILURE: Self = Self(2);
    pub const LOAD_ERROR: Self = Self(Self::ERROR_BIT | 1);
    pub const INVALID_PARAMETER: Self = Self(Self::ERROR_BIT | 2);
    pub const UNSUPPORTED: Self = Self(Self::ERROR_BIT | 3);
    pub const BAD_BUFFER_SIZE: Self = Self(Self::ERROR_BIT | 4);
    pub const BUFFER_TOO_SMALL: Self = Self(Self::ERROR_BIT | 5);
    pub const NOT_READY: Self = Self(Self::ERROR_BIT | 6);
    pub const DEVICE_ERROR: Self = Self(Self::ERROR_BIT | 7);
    pub const WRITE_PROTECTED: Self = Self(Self::ERROR_BIT | 8);
    pub const OUT_OF_RESOURCES: Self = Self(Self::ERROR_BIT | 9);
    pub const VOLUME_CORRUPTED: Self = Self(Self::ERROR_BIT | 10);
    pub const VOLUME_FULL: Self = Self(Self::ERROR_BIT | 11);
    pub const NO_MEDIA: Self = Self(Self::ERROR_BIT | 12);
    pub const MEDIA_CHANGED: Self = Self(Self::ERROR_BIT | 13);
    pub const NOT_FOUND: Self = Self(Self::ERROR_BIT | 14);
    pub const ACCESS_DENIED: Self = Self(Self::ERROR_BIT | 15);
    pub const NO_MAPPING: Self = Self(Self::ERROR_BIT | 17);
    pub const TIMEOUT: Self = Self(Self::ERROR_BIT | 18);
    pub const ALREADY_STARTED: Self = Self(Self::ERROR_BIT | 20);
    pub const ABORTED: Self = Self(Self::ERROR_BIT | 21);
    pub const SECURITY_VIOLATION: Self = Self(Self::ERROR_BIT | 26);
    pub const COMPROMISED_DATA: Self = Self(Self::ERROR_BIT | 33);

    const ERROR_BIT: usize = 1 << (usize::BITS - 1);

    pub const fn raw(self) -> usize {
        self.0
    }

    pub const fn is_error(self) -> bool {
        self.0 & Self::ERROR_BIT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_distinguishes_errors_from_success_and_warnings() {
        assert_eq!(
            core::mem::size_of::<Status>(),
            core::mem::size_of::<usize>()
        );
        assert!(!Status::SUCCESS.is_error());
        assert!(!Status(1).is_error());
        assert!(Status::INVALID_PARAMETER.is_error());
        assert_eq!(Status::INVALID_PARAMETER.0, Status::ERROR_BIT | 2);
    }
}
