//! Polled PTP MMIO CRB transport. Only the handoff-validated locality 0 is used.

use crate::uefi::status::Status;
use core::ptr::{read_volatile, write_volatile};

const LOC_STATE: usize = 0;
const LOC_CTRL: usize = 8;
const LOC_STS: usize = 12;
const INTF_ID: usize = 0x30;
const CAP_CRB_CHUNK: u32 = 1 << 10;
const CTRL_REQ: usize = 0x40;
const CTRL_STS: usize = 0x44;
const CTRL_START: usize = 0x4c;
const START: u32 = 1;
const NEXT_CHUNK: u32 = 1 << 2;
const CMD_SIZE: usize = 0x58;
const CMD_LADDR: usize = 0x5c;
const CMD_HADDR: usize = 0x60;
const RSP_SIZE: usize = 0x64;
const RSP_ADDR: usize = 0x68;
const BUFFER: usize = 0x80;
const CAPACITY: usize = 0x0f80;
// Logical command/response limit of the board's chunking-capable TPM.
const MAX_FRAME_SIZE: u16 = 8192;
const HEADER_LEN: usize = 10;
const POLL_LIMIT: usize = 10_000;

#[derive(Clone, Copy)]
pub(super) struct Crb {
    base: usize,
    capacity: u16,
}

impl Crb {
    /// The caller owns the BSP and has validated and mapped the fixed handoff CRB aperture.
    pub(super) unsafe fn open(base: u64) -> Result<Self, Status> {
        let mut crb = Self {
            base: base as usize,
            capacity: CAPACITY as u16,
        };
        let interface = crb.get(INTF_ID);
        if crb.get(LOC_STATE) & 0x80 == 0 || interface & 0x400f != 0x4001 {
            return Err(Status::DEVICE_ERROR);
        }
        crb.acquire()?;
        let address = crb.base + BUFFER;
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
        if interface & CAP_CRB_CHUNK != 0 {
            crb.capacity = MAX_FRAME_SIZE;
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
        if input_len < HEADER_LEN
            || input_len > usize::from(self.capacity)
            || output_len < HEADER_LEN
        {
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
        let buffer = (self.base + BUFFER) as *mut u8;
        let mut sent = 0;
        while sent < input_len {
            let chunk = (input_len - sent).min(CAPACITY);
            for index in 0..chunk {
                unsafe {
                    buffer
                        .add(index)
                        .write_volatile(input.add(sent + index).read())
                };
            }
            sent += chunk;
            // The final chunk starts execution, even if it fills the MMIO window.
            let field = if sent == input_len { START } else { NEXT_CHUNK };
            self.put(CTRL_START, field);
            self.poll(CTRL_START, field, 0)?;
        }
        if self.get(CTRL_STS) & 1 != 0 {
            return Err(Status::DEVICE_ERROR);
        }
        let mut header = [0; HEADER_LEN];
        for (index, byte) in header.iter_mut().enumerate() {
            *byte = unsafe { buffer.add(index).read_volatile() };
        }
        let len = u32::from_be_bytes(header[2..6].try_into().unwrap()) as usize;
        if !(HEADER_LEN..=usize::from(self.capacity)).contains(&len) {
            return Err(Status::DEVICE_ERROR);
        }
        if len > output_len {
            return Err(Status::BUFFER_TOO_SMALL);
        }
        unsafe { core::ptr::copy_nonoverlapping(header.as_ptr(), output, HEADER_LEN) };
        let mut copied = HEADER_LEN;
        while copied < len {
            let start = copied % CAPACITY;
            let chunk = (len - copied).min(CAPACITY - start);
            for index in 0..chunk {
                unsafe {
                    output
                        .add(copied + index)
                        .write(buffer.add(start + index).read_volatile());
                }
            }
            copied += chunk;
            if copied < len {
                self.put(CTRL_START, NEXT_CHUNK);
                self.poll(CTRL_START, NEXT_CHUNK, 0)?;
            }
        }
        Ok(len)
    }

    pub(super) const fn capacity(self) -> u16 {
        self.capacity
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
        unsafe { read_volatile((self.base + offset) as *const u32) }
    }

    fn put(self, offset: usize, value: u32) {
        unsafe { write_volatile((self.base + offset) as *mut u32, value) }
    }
}
