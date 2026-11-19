//! Controller ownership and synchronous NVMe commands for Boot Services.

use core::{cell::Cell, ptr};

use crate::{
    acpi::manifest::device_type,
    bs::{
        event::stall,
        memory::{allocate_pages, free_pages},
    },
    handoff::{Handoff, region_kind},
    mem::map::MemoryType,
    paging::PAGE_BYTES,
    uefi::status::Status,
};

use super::queue::{Command, DEPTH, Queue};

pub(crate) const REGISTERS_BASE: u64 = 0xfc00_1000;
pub(crate) const REGISTERS_BYTES: u64 = 0x2000;
pub(crate) const NAMESPACE_ID: u32 = 1;
pub(crate) const BLOCK_BYTES: usize = 512;
const PAGE: usize = PAGE_BYTES as usize;
const DMA_PAGES: usize = 5;
pub(super) const POLL_US: usize = 10;
const CSTS: usize = 0x1c;
const CC: usize = 0x14;
const RDY: u32 = 1;
const CFS: u32 = 1 << 1;
const ENABLE: u32 = 1 | (6 << 16) | (4 << 20);
const AQA: usize = 0x24;
const ASQ: usize = 0x28;
const ACQ: usize = 0x30;

#[derive(Clone, Copy)]
pub(super) struct Registers(usize);

impl Registers {
    pub(super) fn read(self, offset: usize) -> u32 {
        // SAFETY: discovery validated this fixed MMIO window. Callers use aligned dwords.
        unsafe { ptr::with_exposed_provenance::<u32>(self.0 + offset).read_volatile() }
    }

    pub(super) fn write(self, offset: usize, word: u32) {
        // SAFETY: discovery validated this fixed MMIO window. Callers use aligned dwords.
        unsafe { ptr::with_exposed_provenance_mut::<u32>(self.0 + offset).write_volatile(word) };
    }

    fn write_address(self, offset: usize, address: u64) {
        self.write(offset, address as u32);
        self.write(offset + 4, (address >> 32) as u32);
    }

    pub(super) fn failed(self) -> bool {
        self.read(CSTS) & (RDY | CFS) != RDY
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct Namespace {
    pub(crate) sectors: u64,
    pub(crate) eui: u64,
}

pub(crate) struct Controller {
    registers: Registers,
    admin: Queue,
    io: Queue,
    data: u64,
    ready_waits: usize,
    namespace: Namespace,
    busy: Cell<bool>,
    online: Cell<bool>,
}

struct Busy<'a>(&'a Cell<bool>);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Controller {
    /// # Safety
    /// Call on the BSP with the live board's identity map and Boot Services allocator.
    pub(crate) unsafe fn discover(handoff: &Handoff<'_>) -> Result<Option<Self>, Status> {
        let (devices, count) = handoff.device_entries();
        let Some((index, entry)) = devices[..count]
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.device_type == device_type::NVME && entry.is_present())
        else {
            return Ok(None);
        };
        if entry.base != REGISTERS_BASE
            || entry.length != REGISTERS_BYTES
            || !handoff.regions().any(|region| {
                region.kind == region_kind::MMIO
                    && region.owner == index as u32
                    && region.base == REGISTERS_BASE
                    && region.length == REGISTERS_BYTES
            })
        {
            return Err(Status::UNSUPPORTED);
        }
        let registers = Registers(REGISTERS_BASE as usize);
        let cap_low = registers.read(0);
        let cap_high = registers.read(4);
        // Fixed queue depth, doorbell stride 0, NVM command set, 4 KiB host pages.
        if cap_low & 0xffff < DEPTH - 1
            || cap_high & (0x0f | 0x000f_0000) != 0
            || cap_high & (1 << 5) == 0
        {
            return Err(Status::UNSUPPORTED);
        }
        let mut base = 0;
        let status =
            unsafe { allocate_pages(0, MemoryType::BOOT_SERVICES_DATA, DMA_PAGES, &mut base) };
        if status != Status::SUCCESS {
            return Err(status);
        }
        let mut controller = Self {
            registers,
            admin: Queue::new(0, base, base + PAGE_BYTES),
            io: Queue::new(1, base + 2 * PAGE_BYTES, base + 3 * PAGE_BYTES),
            data: base + 4 * PAGE_BYTES,
            ready_waits: (((cap_low >> 24) as usize).max(1) * 500_000) / POLL_US,
            namespace: Namespace { sectors: 0, eui: 0 },
            busy: Cell::new(false),
            online: Cell::new(false),
        };
        match controller.start() {
            Ok(namespace) => {
                controller.namespace = namespace;
                Ok(Some(controller))
            }
            Err(error) => {
                unsafe { controller.release() };
                Err(error)
            }
        }
    }

    /// # Safety
    /// Called before publication, or after all handles and events have been removed.
    pub(crate) unsafe fn release(self) {
        self.quiesce();
        // Keep the queues owned until the controller has stopped referencing guest RAM.
        if self.wait_ready(false).is_ok() {
            unsafe { free_pages(self.admin.submission, DMA_PAGES) };
        }
    }

    pub(crate) fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub(crate) fn quiesce(&self) {
        self.online.set(false);
        self.registers.write(CC, 0);
    }

    fn wait_ready(&self, ready: bool) -> Result<(), Status> {
        let mut waits = self.ready_waits;
        loop {
            let status = self.registers.read(CSTS);
            if ready && status & CFS != 0 {
                return Err(Status::DEVICE_ERROR);
            }
            if (status & RDY != 0) == ready {
                return Ok(());
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

    fn start(&self) -> Result<Namespace, Status> {
        self.quiesce();
        self.wait_ready(false)?;
        // SAFETY: both queues are private DMA pages and the disabled controller is idle.
        unsafe {
            self.admin.reset();
            self.io.reset();
        }
        self.registers.write(AQA, (DEPTH - 1) | ((DEPTH - 1) << 16));
        self.registers.write_address(ASQ, self.admin.submission);
        self.registers.write_address(ACQ, self.admin.completion);
        self.registers.write(CC, ENABLE);
        self.wait_ready(true)?;

        self.send(
            &self.admin,
            Command {
                opcode: 0x06,
                data: self.data,
                cdw10: 1,
                ..Command::default()
            },
        )?;
        let data = self.buffer();
        // SAFETY: Identify has completed; the bounce page is exclusively owned by the BSP.
        let namespaces = unsafe { data.add(516).cast::<u32>().read_volatile() };
        if namespaces < NAMESPACE_ID {
            return Err(Status::NO_MEDIA);
        }
        self.send(
            &self.admin,
            Command {
                opcode: 0x06,
                namespace: NAMESPACE_ID,
                data: self.data,
                ..Command::default()
            },
        )?;
        let (sectors, format, lbaf, eui) = unsafe {
            (
                data.cast::<u64>().read_volatile(),
                data.add(26).read_volatile(),
                data.add(128).cast::<u32>().read_volatile(),
                data.add(120).cast::<u64>().read_volatile(),
            )
        };
        if sectors == 0 {
            return Err(Status::NO_MEDIA);
        }
        if format != 0
            || lbaf & 0x00ff_ffff != 9 << 16
            || sectors.checked_mul(BLOCK_BYTES as u64).is_none()
        {
            return Err(Status::UNSUPPORTED);
        }

        let queue_size = 1 | ((DEPTH - 1) << 16);
        self.send(
            &self.admin,
            Command {
                opcode: 0x05,
                data: self.io.completion,
                cdw10: queue_size,
                cdw11: 1,
                ..Command::default()
            },
        )?;
        self.send(
            &self.admin,
            Command {
                opcode: 0x01,
                data: self.io.submission,
                cdw10: queue_size,
                cdw11: 1 | (1 << 16),
                ..Command::default()
            },
        )?;
        self.online.set(true);
        Ok(Namespace { sectors, eui })
    }

    fn send(&self, queue: &Queue, command: Command) -> Result<(), Status> {
        // SAFETY: only initialization or a caller holding Busy can own the queue.
        match unsafe { queue.submit(self.registers, command) } {
            Ok(0) => Ok(()),
            Ok(0x82) => Err(Status::NO_MEDIA),
            Ok(_) => Err(Status::DEVICE_ERROR),
            Err(error) => {
                self.quiesce();
                Err(error)
            }
        }
    }

    /// Only one complete user operation may use the DMA pages and rings, including during Stall.
    pub(crate) fn exclusive<T>(
        &self,
        operation: impl FnOnce(&Self) -> Result<T, Status>,
    ) -> Result<T, Status> {
        if self.busy.replace(true) {
            return Err(Status::NOT_READY);
        }
        let _busy = Busy(&self.busy);
        operation(self)
    }

    pub(crate) fn reset(&self) -> Result<(), Status> {
        let status = match self.start() {
            Ok(namespace) if namespace == self.namespace => return Ok(()),
            Ok(_) => Status::MEDIA_CHANGED,
            Err(status) => status,
        };
        self.quiesce();
        Err(status)
    }

    pub(crate) fn buffer(&self) -> *mut u8 {
        ptr::with_exposed_provenance_mut(self.data as usize)
    }

    pub(crate) fn transfer(&self, write: bool, lba: u64, sectors: u32) -> Result<(), Status> {
        if sectors == 0
            || sectors as usize > PAGE / BLOCK_BYTES
            || lba
                .checked_add(u64::from(sectors))
                .is_none_or(|end| end > self.namespace.sectors)
        {
            return Err(Status::INVALID_PARAMETER);
        }
        self.send_io(Command {
            opcode: if write { 0x01 } else { 0x02 },
            namespace: NAMESPACE_ID,
            data: self.data,
            cdw10: lba as u32,
            cdw11: (lba >> 32) as u32,
            cdw12: sectors - 1,
        })
    }

    pub(crate) fn flush(&self) -> Result<(), Status> {
        self.send_io(Command {
            namespace: NAMESPACE_ID,
            ..Command::default()
        })
    }

    fn send_io(&self, command: Command) -> Result<(), Status> {
        if !self.online.get() {
            return Err(Status::NOT_READY);
        }
        self.send(&self.io, command)
    }
}
