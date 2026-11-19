//! Poll-free synchronous transfers to the board NVRAM device.

use super::codec::MAX_STORE_BYTES;
use crate::uefi::status::Status;
use core::ptr;

const ADDR: usize = MAX_STORE_BYTES;
const LEN: usize = MAX_STORE_BYTES + 4;
const CMD: usize = MAX_STORE_BYTES + 8;
const STATUS: usize = MAX_STORE_BYTES + 12;
const ACTIVE_LEN: usize = MAX_STORE_BYTES + 16;

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
        if length == 0 || length > out.len() || length > MAX_STORE_BYTES {
            return Err(Status::DEVICE_ERROR);
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
        self.write_register(CMD, 3);
        self.expect_status(3)
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
