//! NVMe submission and completion rings in identity-mapped guest RAM.

use core::{
    cell::Cell,
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};

use crate::{bs::event::stall, uefi::status::Status};

use super::driver::{POLL_US, Registers};

pub(super) const DEPTH: u32 = 4;
const SQE_BYTES: usize = 64;
const CQE_BYTES: usize = 16;
const DOORBELLS: usize = 0x1000;
const COMMAND_WAIT_US: usize = 10_000;

#[derive(Clone, Copy, Default)]
pub(super) struct Command {
    pub(super) opcode: u8,
    pub(super) namespace: u32,
    pub(super) data: u64,
    pub(super) cdw10: u32,
    pub(super) cdw11: u32,
    pub(super) cdw12: u32,
}

pub(super) struct Queue {
    pub(super) submission: u64,
    pub(super) completion: u64,
    id: u16,
    cursor: Cell<Cursor>,
}

#[derive(Clone, Copy)]
struct Cursor {
    tail: u8,
    head: u8,
    phase: bool,
    cid: u16,
}

impl Cursor {
    const INITIAL: Self = Self {
        tail: 0,
        head: 0,
        phase: true,
        cid: 0,
    };
}

impl Queue {
    /// The queue owns its two DMA pages for the lifetime of the driver.
    pub(super) fn new(id: u16, submission: u64, completion: u64) -> Self {
        Self {
            submission,
            completion,
            id,
            cursor: Cell::new(Cursor::INITIAL),
        }
    }

    /// Call with the controller disabled and the queue pages still allocated.
    pub(super) unsafe fn reset(&self) {
        let completion = ptr::with_exposed_provenance_mut::<u8>(self.completion as usize);
        unsafe { completion.write_bytes(0, DEPTH as usize * CQE_BYTES) };
        self.cursor.set(Cursor::INITIAL);
    }

    /// The BSP owns the controller while any stall notification is dispatched.
    pub(super) unsafe fn submit(
        &self,
        registers: Registers,
        command: Command,
    ) -> Result<u16, Status> {
        let mut cursor = self.cursor.get();
        let at = self.submission as usize + usize::from(cursor.tail) * SQE_BYTES;
        let submission = ptr::with_exposed_provenance_mut::<u32>(at);
        let words = [
            u32::from(command.opcode) | (u32::from(cursor.cid) << 16),
            command.namespace,
            0,
            0,
            0,
            0,
            command.data as u32,
            (command.data >> 32) as u32,
            0,
            0,
            command.cdw10,
            command.cdw11,
            command.cdw12,
            0,
            0,
            0,
        ];
        for (index, word) in words.into_iter().enumerate() {
            unsafe { submission.add(index).write_volatile(word) };
        }
        let doorbell = DOORBELLS + usize::from(self.id) * 8;
        cursor.tail = (cursor.tail + 1) % DEPTH as u8;
        compiler_fence(Ordering::Release);
        registers.write(doorbell, u32::from(cursor.tail));

        let at = self.completion as usize + usize::from(cursor.head) * CQE_BYTES;
        let completion = ptr::with_exposed_provenance::<u32>(at);
        let mut waits = COMMAND_WAIT_US / POLL_US;
        loop {
            let status_cid = unsafe { completion.add(3).read_volatile() };
            if (status_cid & (1 << 16) != 0) == cursor.phase {
                compiler_fence(Ordering::Acquire);
                let sqid_head = unsafe { completion.add(2).read_volatile() };
                if status_cid as u16 != cursor.cid || sqid_head >> 16 != u32::from(self.id) {
                    return Err(Status::DEVICE_ERROR);
                }
                cursor.cid = cursor.cid.wrapping_add(1);
                cursor.head = (cursor.head + 1) % DEPTH as u8;
                if cursor.head == 0 {
                    cursor.phase = !cursor.phase;
                }
                self.cursor.set(cursor);
                registers.write(doorbell + 4, u32::from(cursor.head));
                return Ok(((status_cid >> 17) & 0x7ff) as u16);
            }
            if registers.failed() {
                return Err(Status::DEVICE_ERROR);
            }
            if waits == 0 {
                return Err(Status::TIMEOUT);
            }
            let status = unsafe { stall(POLL_US) };
            if status != Status::SUCCESS {
                return Err(status);
            }
            waits -= 1;
        }
    }
}
