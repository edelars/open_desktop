//! Poll-free synchronous transfers to the board NVRAM device.

use super::codec::MAX_STORE_BYTES;
use crate::uefi::status::Status;
use core::ptr;
#[cfg(target_os = "none")]
use od_uefi_security::image_execution::{RejectedImage, RejectedImageAction};

const ADDR: usize = MAX_STORE_BYTES;
const LEN: usize = MAX_STORE_BYTES + 4;
const CMD: usize = MAX_STORE_BYTES + 8;
const STATUS: usize = MAX_STORE_BYTES + 12;
const ACTIVE_LEN: usize = MAX_STORE_BYTES + 16;
#[cfg(target_os = "none")]
const POLICY_EPOCH_LOW: usize = MAX_STORE_BYTES + 20;
#[cfg(target_os = "none")]
const POLICY_EPOCH_HIGH: usize = MAX_STORE_BYTES + 24;
#[cfg(target_os = "none")]
pub(crate) const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;

#[cfg(target_os = "none")]
#[derive(Clone, Copy)]
pub(crate) enum ImageCheck {
    Disabled,
    Authorized(usize),
    Rejected(RejectedImage),
}

#[derive(Clone, Copy)]
pub(crate) struct Nvram {
    base: usize,
}

impl Nvram {
    /// # Safety
    /// `base` identifies the validated and mapped NVRAM aperture of this board.
    pub(crate) unsafe fn at(base: usize) -> Self {
        Self { base }
    }

    pub(crate) fn read(self, out: &mut [u8]) -> Result<usize, Status> {
        let length = self.read_register(ACTIVE_LEN) as usize;
        if length > out.len() || length > MAX_STORE_BYTES {
            return Err(Status::DEVICE_ERROR);
        }
        if length == 0 {
            return Ok(0);
        }
        self.begin_transfer(1, 1, length)?;
        for (index, byte) in out[..length].iter_mut().enumerate() {
            // SAFETY: READ latched this range and the aperture supports byte access.
            *byte =
                unsafe { ptr::with_exposed_provenance::<u8>(self.base + index).read_volatile() };
        }
        Ok(length)
    }

    pub(crate) fn write(self, image: &[u8]) -> Result<(), Status> {
        self.stage(image)?;
        self.write_register(CMD, 3);
        self.expect_status(3)
    }

    pub(crate) fn authenticated_update(self, request: &[u8]) -> Result<(), Status> {
        self.stage(request)?;
        self.write_register(CMD, 4);
        let status = self.read_register(STATUS);
        if status == 3 {
            return Ok(());
        }
        const ERRORS: [Status; 7] = [
            Status::INVALID_PARAMETER,
            Status::DEVICE_ERROR,
            Status::DEVICE_ERROR,
            Status::DEVICE_ERROR,
            Status::SECURITY_VIOLATION,
            Status::NOT_FOUND,
            Status::OUT_OF_RESOURCES,
        ];
        let error_index = status.wrapping_sub(0x8000_0001) as usize;
        Err(ERRORS
            .get(error_index)
            .copied()
            .unwrap_or(Status::DEVICE_ERROR))
    }

    #[cfg(target_os = "none")]
    pub(crate) fn policy_epoch(self) -> Result<u64, Status> {
        let high = self.read_register(POLICY_EPOCH_HIGH);
        let low = self.read_register(POLICY_EPOCH_LOW);
        if high != self.read_register(POLICY_EPOCH_HIGH) {
            return Err(Status::DEVICE_ERROR);
        }
        Ok(u64::from(high) << 32 | u64::from(low))
    }

    /// Submits one complete file. Disabled means SetupMode or the host's explicit bypass.
    /// If provided, `authority` receives the exact enrolled EFI_SIGNATURE_DATA at success.
    #[cfg(target_os = "none")]
    pub(crate) fn authorize_image(
        self,
        file: &[u8],
        authority: Option<&mut [u8]>,
    ) -> Result<ImageCheck, Status> {
        if !(1..=MAX_IMAGE_BYTES).contains(&file.len()) {
            return Err(Status::ACCESS_DENIED);
        }
        match self.image_command(5, 0, file.len()) {
            4 => {}
            0x8000_0007 => return Err(Status::OUT_OF_RESOURCES),
            _ => return Err(Status::DEVICE_ERROR),
        }
        for (index, chunk) in file.chunks(MAX_STORE_BYTES).enumerate() {
            if self.image_command(6, index * MAX_STORE_BYTES, chunk.len()) != 2 {
                return Err(Status::DEVICE_ERROR);
            }
            for (offset, &byte) in chunk.iter().enumerate() {
                // The image chunk command accepted this ordered range.
                unsafe {
                    ptr::with_exposed_provenance_mut::<u8>(self.base + offset).write_volatile(byte)
                };
            }
        }
        match self.image_command(7, 0, 0) {
            5 => {
                let length = self.read_register(LEN) as usize;
                if !(17..=od_uefi_security::protected::PROTECTED_VALUE_LIMIT).contains(&length) {
                    return Err(Status::DEVICE_ERROR);
                }
                if let Some(authority) = authority {
                    let out = authority
                        .get_mut(..length)
                        .ok_or(Status::BUFFER_TOO_SMALL)?;
                    self.read_image_reply(out);
                }
                Ok(ImageCheck::Authorized(length))
            }
            6 => Ok(ImageCheck::Disabled),
            0x8000_0005 => {
                let action = RejectedImageAction::from_raw(self.read_register(LEN))
                    .ok_or(Status::DEVICE_ERROR)?;
                let digest = match self.read_register(ADDR) {
                    0 => None,
                    32 => {
                        let mut digest = [0; 32];
                        self.read_image_reply(&mut digest);
                        Some(digest)
                    }
                    _ => return Err(Status::DEVICE_ERROR),
                };
                Ok(ImageCheck::Rejected(RejectedImage { action, digest }))
            }
            0x8000_0007 => Err(Status::OUT_OF_RESOURCES),
            _ => Err(Status::DEVICE_ERROR),
        }
    }

    #[cfg(target_os = "none")]
    fn read_image_reply(self, out: &mut [u8]) {
        for (offset, byte) in out.iter_mut().enumerate() {
            *byte =
                unsafe { ptr::with_exposed_provenance::<u8>(self.base + offset).read_volatile() };
        }
    }

    #[cfg(target_os = "none")]
    fn image_command(self, command: u32, address: usize, length: usize) -> u32 {
        self.write_register(ADDR, address as u32);
        self.write_register(LEN, length as u32);
        self.write_register(CMD, command);
        self.read_register(STATUS)
    }

    fn stage(self, image: &[u8]) -> Result<(), Status> {
        if !(1..=MAX_STORE_BYTES).contains(&image.len()) {
            return Err(Status::DEVICE_ERROR);
        }
        self.begin_transfer(2, 2, image.len())?;
        for (index, &byte) in image.iter().enumerate() {
            // SAFETY: WRITE accepted this range and bytes are submitted in order.
            unsafe {
                ptr::with_exposed_provenance_mut::<u8>(self.base + index).write_volatile(byte)
            };
        }
        Ok(())
    }

    fn begin_transfer(self, command: u32, expected: u32, length: usize) -> Result<(), Status> {
        self.write_register(ADDR, 0);
        self.write_register(LEN, length as u32);
        self.write_register(CMD, command);
        self.expect_status(expected)
    }

    fn expect_status(self, expected: u32) -> Result<(), Status> {
        if self.read_register(STATUS) == expected {
            Ok(())
        } else {
            Err(Status::DEVICE_ERROR)
        }
    }

    fn read_register(self, offset: usize) -> u32 {
        // SAFETY: callers use aligned read registers in the mapped window.
        unsafe { ptr::with_exposed_provenance::<u32>(self.base + offset).read_volatile() }
    }

    fn write_register(self, offset: usize, value: u32) {
        // SAFETY: callers use aligned writable registers in the mapped window.
        unsafe { ptr::with_exposed_provenance_mut::<u32>(self.base + offset).write_volatile(value) }
    }
}
