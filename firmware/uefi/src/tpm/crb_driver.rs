//! Polled PTP MMIO CRB transport. Only the handoff-validated locality 0 is used.

use crate::uefi::status::Status;
use core::ptr::{read_volatile, write_volatile};

const LOC_STATE: usize = 0;
const LOC_CTRL: usize = 8;
const LOC_STS: usize = 12;
const INTF_ID: usize = 0x30;
const CTRL_REQ: usize = 0x40;
const CTRL_STS: usize = 0x44;
const CTRL_START: usize = 0x4c;
const CMD_SIZE: usize = 0x58;
const CMD_LADDR: usize = 0x5c;
const CMD_HADDR: usize = 0x60;
const RSP_SIZE: usize = 0x64;
const RSP_ADDR: usize = 0x68;
const BUFFER: usize = 0x80;
const CAPACITY: usize = 0x0f80;
const HEADER_LEN: usize = 10;
const POLL_LIMIT: usize = 10_000;

#[derive(Clone, Copy)]
pub(super) struct Crb(usize);

impl Crb {
    /// The caller owns the BSP and has validated and mapped the fixed handoff CRB aperture.
    pub(super) unsafe fn open(base: u64) -> Result<Self, Status> {
        let crb = Self(base as usize);
        if crb.get(LOC_STATE) & 0x80 == 0 || crb.get(INTF_ID) & 0x400f != 0x4001 {
            return Err(Status::DEVICE_ERROR);
        }
        crb.acquire()?;
        let address = crb.0 + BUFFER;
        let valid = crb.get(CMD_LADDR) as usize == address
            && crb.get(CMD_HADDR) == 0
            && crb.get(RSP_ADDR) as usize == address
            && crb.get(RSP_ADDR + 4) == 0
            && crb.get(CMD_SIZE) as usize == CAPACITY
            && crb.get(RSP_SIZE) as usize == CAPACITY;
        crb.put(LOC_CTRL, 2);
        if !valid {
            return Err(Status::DEVICE_ERROR);
        }
        Ok(crb)
    }

    pub(super) fn submit(
        self,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> Result<usize, Status> {
        if input_len < HEADER_LEN || input_len > CAPACITY || output_len < HEADER_LEN {
            return Err(Status::INVALID_PARAMETER);
        }
        self.acquire()?;
        let result = self.transact(input, input_len, output, output_len);
        self.put(CTRL_REQ, 2);
        self.put(LOC_CTRL, 2);
        result
    }

    fn transact(
        self,
        input: *const u8,
        input_len: usize,
        output: *mut u8,
        output_len: usize,
    ) -> Result<usize, Status> {
        self.put(CTRL_REQ, 1);
        self.poll(CTRL_STS, 2, 0)?;
        let buffer = (self.0 + BUFFER) as *mut u8;
        for index in 0..input_len {
            // Each successive MMIO write makes that byte available for the command.
            unsafe { buffer.add(index).write_volatile(input.add(index).read()) };
        }
        self.put(CTRL_START, 1);
        self.poll(CTRL_START, 1, 0)?;
        if self.get(CTRL_STS) & 1 != 0 {
            return Err(Status::DEVICE_ERROR);
        }
        let mut header = [0; HEADER_LEN];
        for (index, byte) in header.iter_mut().enumerate() {
            *byte = unsafe { buffer.add(index).read_volatile() };
        }
        let len = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
        if !(HEADER_LEN..=CAPACITY).contains(&len) {
            return Err(Status::DEVICE_ERROR);
        }
        if len > output_len {
            return Err(Status::BUFFER_TOO_SMALL);
        }
        unsafe { core::ptr::copy_nonoverlapping(header.as_ptr(), output, HEADER_LEN) };
        for index in HEADER_LEN..len {
            unsafe { output.add(index).write(buffer.add(index).read_volatile()) };
        }
        Ok(len)
    }

    pub(super) const fn capacity(self) -> u16 {
        CAPACITY as u16
    }

    fn acquire(self) -> Result<(), Status> {
        self.put(LOC_CTRL, 1);
        self.poll(LOC_STS, 1, 1)
    }

    fn poll(self, offset: usize, mask: u32, expected: u32) -> Result<(), Status> {
        for _ in 0..POLL_LIMIT {
            if self.get(offset) & mask == expected {
                return Ok(());
            }
        }
        Err(Status::TIMEOUT)
    }

    fn get(self, offset: usize) -> u32 {
        unsafe { read_volatile((self.0 + offset) as *const u32) }
    }

    fn put(self, offset: usize, value: u32) {
        unsafe { write_volatile((self.0 + offset) as *mut u32, value) }
    }
}
