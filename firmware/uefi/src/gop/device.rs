//! Fixed-register display transport from DISPLAY_DEVICE.md §§4–6, 14.

use core::{
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};

use crate::{
    acpi::manifest::device_type,
    handoff::{Handoff, region_kind},
    uefi::status::Status,
};

pub(super) const REGISTERS_BASE: u64 = 0xfc00_0000;
pub(super) const REGISTERS_BYTES: u64 = 0x1000;
const FRAMEBUFFER_BASE: u64 = 0xf800_0000;
const FRAMEBUFFER_BYTES: u64 = 0x0240_0000;
const SIGNATURE: u32 = 0x4246_444f;
const BGRX8888: u32 = 1;
pub(super) const PIXEL_BYTES: usize = 4;

const ID: usize = 0x000;
const VERSION: usize = 0x004;
const CAPS: usize = 0x008;
const MAX_WIDTH: usize = 0x00c;
const MAX_HEIGHT: usize = 0x010;
const FORMATS: usize = 0x014;
const FB_BASE_LOW: usize = 0x018;
const FB_BASE_HIGH: usize = 0x01c;
const FB_SIZE: usize = 0x020;
const WIDTH: usize = 0x040;
const HEIGHT: usize = 0x044;
const FORMAT: usize = 0x048;
const MODE_COMMIT: usize = 0x04c;
const ACTIVE_WIDTH: usize = 0x060;
const ACTIVE_HEIGHT: usize = 0x064;
const ACTIVE_FORMAT: usize = 0x068;
const STRIDE: usize = 0x06c;
const STATUS: usize = 0x070;
const FRAME_GENERATION: usize = 0x080;
const MODE_COUNT: usize = 0x110;
const MODE_TABLE: usize = 0x400;
const MODE_ENTRY_BYTES: usize = 16;
const MODE_TABLE_ENTRIES: u32 = 64;
const CAPS_MODE_LIST: u32 = 1;
const STATUS_ACTIVE: u32 = 1;
const STATUS_FRAME_READY: u32 = 1 << 1;
const COMMIT_ERROR_MASK: u32 = 0xff << 8;
const FRAME_ERROR_MASK: u32 = 0xff << 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Geometry {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) stride: u32,
    pub(super) bytes: usize,
}

impl Geometry {
    fn new(width: u32, height: u32, capacity: usize) -> Option<Self> {
        if width == 0 || height == 0 {
            return None;
        }
        let stride = width.checked_mul(PIXEL_BYTES as u32)?;
        let bytes = (stride as usize).checked_mul(height as usize)?;
        (bytes <= capacity).then_some(Self {
            width,
            height,
            stride,
            bytes,
        })
    }
}

pub(super) struct Display {
    pub(super) mode_count: u32,
    pub(super) framebuffer: u64,
    capacity: usize,
    max_width: u32,
    max_height: u32,
}

impl Display {
    /// # Safety
    /// The validated boot handoff describes the live board's identity map.
    pub(super) unsafe fn discover(handoff: &Handoff<'_>) -> Result<Option<Self>, Status> {
        let (devices, count) = handoff.device_entries();
        let Some((index, entry)) = devices[..count]
            .iter()
            .enumerate()
            .find(|(_, entry)| entry.device_type == device_type::DISPLAY && entry.is_present())
        else {
            return Ok(None);
        };
        let owner = index as u32;
        if entry.base != REGISTERS_BASE
            || entry.length != REGISTERS_BYTES
            || !handoff.regions().any(|region| {
                region.kind == region_kind::MMIO
                    && region.owner == owner
                    && region.base == REGISTERS_BASE
                    && region.length == REGISTERS_BYTES
            })
            || !handoff.regions().any(|region| {
                region.kind == region_kind::FRAMEBUFFER
                    && region.owner == owner
                    && region.attrs == 0
                    && region.base == FRAMEBUFFER_BASE
                    && region.length == FRAMEBUFFER_BYTES
            })
        {
            return Err(Status::DEVICE_ERROR);
        }
        // SAFETY: the exact register window and its framebuffer are mapped by the board.
        unsafe {
            if Self::read(ID) != SIGNATURE
                || Self::read(VERSION) != 2
                || Self::read(CAPS) & CAPS_MODE_LIST == 0
                || Self::read(FORMATS) & (1 << BGRX8888) == 0
            {
                return Err(Status::UNSUPPORTED);
            }
            let framebuffer =
                u64::from(Self::read(FB_BASE_LOW)) | (u64::from(Self::read(FB_BASE_HIGH)) << 32);
            let capacity = Self::read(FB_SIZE) as usize;
            let mode_count = Self::read(MODE_COUNT);
            if framebuffer != FRAMEBUFFER_BASE
                || capacity as u64 != FRAMEBUFFER_BYTES
                || !(1..=MODE_TABLE_ENTRIES).contains(&mode_count)
            {
                return Err(Status::DEVICE_ERROR);
            }
            Ok(Some(Self {
                framebuffer,
                capacity,
                mode_count,
                max_width: Self::read(MAX_WIDTH),
                max_height: Self::read(MAX_HEIGHT),
            }))
        }
    }

    pub(super) fn mode(&self, number: u32) -> Result<Geometry, Status> {
        if number >= self.mode_count {
            return Err(Status::UNSUPPORTED);
        }
        let at = MODE_TABLE + number as usize * MODE_ENTRY_BYTES;
        // SAFETY: discovery validated the register window and table count.
        let (width, height, format) =
            unsafe { (Self::read(at), Self::read(at + 4), Self::read(at + 8)) };
        if width > self.max_width || height > self.max_height || format != BGRX8888 {
            return Err(Status::DEVICE_ERROR);
        }
        Geometry::new(width, height, self.capacity).ok_or(Status::DEVICE_ERROR)
    }

    pub(super) fn commit_mode(&self, mode: Geometry) -> Result<(), Status> {
        // SAFETY: discovery validated the window and mode() checked this geometry.
        unsafe {
            Self::write(WIDTH, mode.width);
            Self::write(HEIGHT, mode.height);
            Self::write(FORMAT, BGRX8888);
            Self::write(MODE_COMMIT, 1);
            if Self::read(STATUS) & (STATUS_ACTIVE | COMMIT_ERROR_MASK) != STATUS_ACTIVE
                || (
                    Self::read(ACTIVE_WIDTH),
                    Self::read(ACTIVE_HEIGHT),
                    Self::read(ACTIVE_FORMAT),
                    Self::read(STRIDE),
                ) != (mode.width, mode.height, BGRX8888, mode.stride)
            {
                return Err(Status::DEVICE_ERROR);
            }
        }
        Ok(())
    }

    pub(super) fn publish_frame(&self) -> Result<(), Status> {
        // Keep ordinary framebuffer stores before the volatile device doorbell.
        compiler_fence(Ordering::Release);
        // SAFETY: discovery validated the register window.
        unsafe {
            Self::write(FRAME_GENERATION, 0);
            if Self::read(STATUS) & (STATUS_FRAME_READY | FRAME_ERROR_MASK) != STATUS_FRAME_READY {
                return Err(Status::DEVICE_ERROR);
            }
        }
        Ok(())
    }

    unsafe fn read(offset: usize) -> u32 {
        let address = (REGISTERS_BASE as usize) + offset;
        // SAFETY: every caller validated this fixed MMIO window and uses aligned dword registers.
        unsafe { ptr::read_volatile(ptr::with_exposed_provenance(address)) }
    }

    unsafe fn write(offset: usize, value: u32) {
        let address = (REGISTERS_BASE as usize) + offset;
        // SAFETY: every caller validated this fixed MMIO window and uses aligned dword registers.
        unsafe { ptr::write_volatile(ptr::with_exposed_provenance_mut(address), value) };
    }
}
