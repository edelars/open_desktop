//! Own display device, contract versions 1 and 2 (`docs/specs/DISPLAY_DEVICE.md`,
//! `protocols/guest/DISPLAY_DRIVER_V1.md`).
//!
//! Version 2 is a strict superset of version 1: a read-only mode table, a
//! bounded host-to-guest event queue and the `MODE_REQUEST` request/response
//! exchange, all in previously reserved register offsets. The optional cursor
//! block (`CAPS` bit 2, section 15, S10-14) occupies `0x200-0x2FC`; its image
//! lives in two 16 KiB buffers inside the device, not in guest memory.
//!
//! The device owns only registers and counters. Pixel bytes live in a
//! `MemoryRegionKind::Framebuffer` region of `PhysicalMemory`; the guest writes
//! them with ordinary stores and signals a finished frame through the
//! `FRAME_GENERATION` doorbell. All register accesses are synchronous 32-bit
//! MMIO accesses.

use crate::{
    AccessWidth, BusDevice, DeviceAccess, DeviceError, FramebufferBinding, GuestPhysicalAddress,
    IoSpace, MachineBus, MemoryAccessError,
};
use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};

/// GPA of the framebuffer memory region.
pub const DISPLAY_FRAMEBUFFER_BASE: u64 = 0xF800_0000;
/// Length of the framebuffer memory region: 36 MiB.
pub const DISPLAY_FRAMEBUFFER_BYTES: u64 = 0x0240_0000;
/// GPA of the 4 KiB register window.
pub const DISPLAY_REGISTERS_BASE: u64 = 0xFC00_0000;
/// Length of the register window.
pub const DISPLAY_REGISTERS_BYTES: u64 = 0x1000;
pub const DISPLAY_MAX_WIDTH: u32 = 4096;
pub const DISPLAY_MAX_HEIGHT: u32 = 2304;
/// `BGRX8888`: byte 0 = B, 1 = G, 2 = R, 3 = X.
pub const DISPLAY_FORMAT_BGRX8888: u32 = 1;

const ID: u32 = 0x4246_444F;
const VERSION: u32 = 2;
/// `CAPS` bit 0: `MODE_COUNT` and the mode table exist.
const CAPS_MODE_LIST: u32 = 1 << 0;
/// `CAPS` bit 1: driver protocol, event queue and `MODE_REQUEST` exist.
const CAPS_EVENTS: u32 = 1 << 1;
/// `CAPS` bit 2: the cursor block `0x200-0x2FC` exists (section 15).
const CAPS_CURSOR: u32 = 1 << 2;
const CAPS: u32 = CAPS_MODE_LIST | CAPS_EVENTS | CAPS_CURSOR;
/// Driver protocol versions the device supports: bit `n` = version `n`.
const DRIVER_PROTOCOLS: u32 = 1 << 1;
/// Event queue capacity in records.
pub const DISPLAY_EVENT_CAPACITY: usize = 16;
/// Event type `MODE_REQUEST` (DISPLAY_DRIVER_V1.md, 5.3).
const EVENT_MODE_REQUEST: u32 = 1;
const MODE_TABLE_ENTRIES: u64 = 64;
/// Mode list of revision 2 (DISPLAY_DEVICE.md, 14.6); every entry is `BGRX8888`.
const MODE_LIST: [(u32, u32); 14] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 720),
    (1280, 800),
    (1280, 1024),
    (1440, 900),
    (1600, 900),
    (1680, 1050),
    (1920, 1080),
    (1920, 1200),
    (2560, 1440),
    (2560, 1600),
    (3840, 2160),
];
const FORMATS: u32 = 1 << DISPLAY_FORMAT_BGRX8888;
const BYTES_PER_PIXEL: u32 = 4;

/// Largest cursor image width in pixels (`CURSOR_MAX_WIDTH`).
pub const DISPLAY_CURSOR_MAX_WIDTH: u32 = 64;
/// Largest cursor image height in pixels (`CURSOR_MAX_HEIGHT`).
pub const DISPLAY_CURSOR_MAX_HEIGHT: u32 = 64;
/// `BGRA8888_PREMUL`: byte 0 = B, 1 = G, 2 = R, 3 = A; colour premultiplied.
pub const DISPLAY_CURSOR_FORMAT_BGRA8888_PREMUL: u32 = 2;
/// Words in each cursor image buffer: 64 x 64 pixels, 16 KiB.
pub const DISPLAY_CURSOR_IMAGE_WORDS: usize = 4096;
const CURSOR_FORMATS: u32 = 1 << DISPLAY_CURSOR_FORMAT_BGRA8888_PREMUL;

mod reg {
    pub const ID: u64 = 0x000;
    pub const VERSION: u64 = 0x004;
    pub const CAPS: u64 = 0x008;
    pub const MAX_WIDTH: u64 = 0x00C;
    pub const MAX_HEIGHT: u64 = 0x010;
    pub const FORMATS: u64 = 0x014;
    pub const FB_BASE_LO: u64 = 0x018;
    pub const FB_BASE_HI: u64 = 0x01C;
    pub const FB_SIZE: u64 = 0x020;
    pub const WIDTH: u64 = 0x040;
    pub const HEIGHT: u64 = 0x044;
    pub const FORMAT: u64 = 0x048;
    pub const MODE_COMMIT: u64 = 0x04C;
    pub const ACTIVE_WIDTH: u64 = 0x060;
    pub const ACTIVE_HEIGHT: u64 = 0x064;
    pub const ACTIVE_FORMAT: u64 = 0x068;
    pub const STRIDE: u64 = 0x06C;
    pub const STATUS: u64 = 0x070;
    pub const FRAME_GENERATION: u64 = 0x080;
    pub const FRAME_GENERATION_HI: u64 = 0x084;
    pub const DRIVER_PROTOCOLS: u64 = 0x100;
    pub const DRIVER_PROTOCOL: u64 = 0x104;
    pub const EVENT_STATUS: u64 = 0x108;
    pub const MODE_COUNT: u64 = 0x110;
    pub const EVENT_PENDING: u64 = 0x140;
    pub const EVENT_CAPACITY: u64 = 0x144;
    pub const EVENT_POP: u64 = 0x148;
    pub const EVENT_SEQ_LO: u64 = 0x150;
    pub const EVENT_SEQ_HI: u64 = 0x154;
    pub const EVENT_TYPE: u64 = 0x158;
    pub const EVENT_ARG0: u64 = 0x160;
    pub const EVENT_ARG1: u64 = 0x164;
    pub const EVENT_ARG2: u64 = 0x168;
    pub const EVENT_ARG3: u64 = 0x16C;
    pub const RESPONSE_SEQ_LO: u64 = 0x180;
    pub const RESPONSE_SEQ_HI: u64 = 0x184;
    pub const REQUEST_RESPONSE: u64 = 0x188;
    pub const CURSOR_BLOCK_START: u64 = 0x200;
    pub const CURSOR_BLOCK_END: u64 = 0x300;
    pub const CURSOR_MAX_WIDTH: u64 = 0x200;
    pub const CURSOR_MAX_HEIGHT: u64 = 0x204;
    pub const CURSOR_FORMATS: u64 = 0x208;
    pub const CURSOR_STATUS: u64 = 0x20C;
    pub const CURSOR_WIDTH: u64 = 0x210;
    pub const CURSOR_HEIGHT: u64 = 0x214;
    pub const CURSOR_FORMAT: u64 = 0x218;
    pub const CURSOR_HOTSPOT: u64 = 0x21C;
    pub const CURSOR_IMAGE_INDEX: u64 = 0x220;
    pub const CURSOR_IMAGE_DATA: u64 = 0x224;
    pub const CURSOR_IMAGE_COMMIT: u64 = 0x228;
    pub const CURSOR_POSITION: u64 = 0x230;
    pub const CURSOR_ENABLE: u64 = 0x234;
    pub const MODE_TABLE: u64 = 0x400;
    pub const MODE_TABLE_END: u64 = 0x800;
}

const STATUS_ACTIVE: u32 = 1 << 0;
const STATUS_FRAME_SINCE_COMMIT: u32 = 1 << 1;
const STATUS_COMMIT_ERROR_SHIFT: u32 = 8;
const STATUS_FRAME_ERROR_SHIFT: u32 = 16;

const ERR_NONE: u8 = 0x00;
const ERR_SIZE_NOT_SET: u8 = 0x01;
const ERR_UNSUPPORTED_SIZE: u8 = 0x02;
const ERR_UNSUPPORTED_FORMAT: u8 = 0x03;
const ERR_INVALID_COMMAND: u8 = 0x04;
const ERR_NO_ACTIVE_MODE: u8 = 0x05;
const ERR_COUNTER_EXHAUSTED: u8 = 0x06;
const ERR_NOT_ENABLED: u8 = 0x07;
const ERR_QUEUE_EMPTY: u8 = 0x08;
const ERR_STALE_REQUEST: u8 = 0x09;
const ERR_MODE_MISMATCH: u8 = 0x0A;
const ERR_UNSUPPORTED_PROTOCOL: u8 = 0x0B;
const ERR_INDEX_OUT_OF_RANGE: u8 = 0x0C;
const ERR_HOTSPOT_OUT_OF_IMAGE: u8 = 0x0D;
const ERR_POSITION_OUT_OF_FRAME: u8 = 0x0E;
const ERR_NO_CURSOR_IMAGE: u8 = 0x0F;

const EVENT_STATUS_RESPONSE_SHIFT: u32 = 8;
const EVENT_STATUS_PROTOCOL_SHIFT: u32 = 16;

const CURSOR_STATUS_IMAGE_SHIFT: u32 = 8;
const CURSOR_STATUS_POSITION_SHIFT: u32 = 16;
const CURSOR_STATUS_ENABLE_SHIFT: u32 = 24;

const VERDICT_ACCEPT: u32 = 1;
const VERDICT_REJECT: u32 = 2;

/// Active display mode. All fields are zero when there is no mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    /// Bytes per scanline (`width * 4`, no padding).
    pub stride: u32,
    pub format: u32,
}

impl DisplayMode {
    /// The explicit "no mode" value.
    pub const NONE: Self = Self {
        width: 0,
        height: 0,
        stride: 0,
        format: 0,
    };

    pub const fn is_active(self) -> bool {
        self.width != 0
    }

    /// `stride * height`: the exact size of one frame in bytes.
    pub const fn frame_bytes(self) -> u64 {
        self.stride as u64 * self.height as u64
    }
}

/// Description of the frame copied by [`DisplayHandle::read_frame_into`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameInfo {
    pub mode: DisplayMode,
    pub generation: u64,
    pub mode_change_count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisplayReadError {
    /// No active mode.
    NoActiveMode,
    /// The mode is active but the guest has not rung the doorbell since the
    /// last successful `MODE_COMMIT`.
    NoFrame,
    /// The destination length differs from `stride * height`.
    BufferLength { expected: usize, actual: usize },
    /// The mode-change counter differed before and after the copy.
    ModeChanged,
    /// The framebuffer region could not be read from the supplied bus.
    Memory(MemoryAccessError),
}

impl fmt::Display for DisplayReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "display frame read error: {self:?}")
    }
}

impl std::error::Error for DisplayReadError {}

/// Why the guest driver rejected a `MODE_REQUEST` (DISPLAY_DRIVER_V1.md, 6.1).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayRejectReason {
    /// The guest OS or driver cannot use the mode (`0x01`).
    Unsupported,
    /// The mode is usable but guest policy keeps the current one (`0x02`).
    Declined,
    /// The change was started and failed (`0x03`).
    Failed,
}

impl DisplayRejectReason {
    fn from_code(code: u32) -> Option<Self> {
        match code {
            0x01 => Some(Self::Unsupported),
            0x02 => Some(Self::Declined),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

/// How a `MODE_REQUEST` ended (DISPLAY_DEVICE.md, 14.8).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModeRequestOutcome {
    /// Open: no answer yet.
    Pending,
    /// The guest answered `ACCEPT` with the requested mode already active.
    Accepted,
    Rejected(DisplayRejectReason),
    /// Replaced by a newer request. Only the replaced request gets this
    /// outcome, and the host API no longer shows it.
    Superseded,
    /// Closed by `DRIVER_PROTOCOL <- 0` or a device reset.
    Cancelled,
}

/// The latest `MODE_REQUEST` of screen 0, as seen by the host.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeRequestState {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub outcome: ModeRequestOutcome,
}

/// Host-side refusal to post a `MODE_REQUEST`. The guest never sees these
/// (DISPLAY_DEVICE.md, 14.10); a refusal leaves the device state unchanged.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DisplayRequestError {
    /// `DRIVER_PROTOCOL` is `0`: no driver has enabled events.
    NoDriver,
    /// The size or format fails `MODE_COMMIT` checks 2-4.
    InvalidMode,
    /// `next_event_seq` is `2^64 - 1`.
    SequenceExhausted,
    /// The queue has no free slot after merging.
    QueueFull,
}

impl fmt::Display for DisplayRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "display mode request refused: {self:?}")
    }
}

impl std::error::Error for DisplayRequestError {}

/// Parameters of the published cursor image (`active` buffer), section 15.5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorShapeInfo {
    /// Value of `cursor_seq` at the commit that published this shape. One
    /// value always describes the same bytes within one device instance.
    pub shape_seq: u64,
    pub width: u32,
    pub height: u32,
    /// `CURSOR_FORMAT` at commit; only `BGRA8888_PREMUL` (`2`) is accepted.
    pub format: u32,
    /// Hotspot inside the image, in image pixels.
    pub hot_x: u32,
    pub hot_y: u32,
}

/// Host view of the cursor (DISPLAY_DEVICE.md, 15.10).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorState {
    /// Changes exactly when the shape, the position or the visibility changes.
    pub cursor_seq: u64,
    pub enabled: bool,
    /// Hotspot position in frame pixels.
    pub x: u32,
    pub y: u32,
    /// The published shape, or `None` if the guest never committed one (or the
    /// device was reset since).
    pub shape: Option<CursorShapeInfo>,
}

/// Refusal of [`DisplayHandle::read_cursor_image_into`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorReadError {
    /// No shape is published.
    NoShape,
    /// The destination length differs from `width * height * 4`.
    BufferLength { expected: usize, actual: usize },
}

impl fmt::Display for CursorReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "display cursor image read error: {self:?}")
    }
}

impl std::error::Error for CursorReadError {}

/// One queue or latch record: 28 bytes of guest-visible state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct QueuedEvent {
    seq: u64,
    kind: u32,
    arg: [u32; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DisplayState {
    requested_width: u32,
    requested_height: u32,
    requested_format: u32,
    mode: DisplayMode,
    frame_since_commit: bool,
    commit_error: u8,
    frame_error: u8,
    generation: u64,
    mode_change_count: u64,
    frame_mode_epoch: u64,
    driver_protocol: u32,
    queue: VecDeque<QueuedEvent>,
    latch: QueuedEvent,
    next_event_seq: u64,
    mode_request: Option<ModeRequestState>,
    response_seq_lo: u32,
    response_seq_hi: u32,
    pop_error: u8,
    response_error: u8,
    protocol_error: u8,
    // Cursor block (section 15). `staging` and `active` are
    // `DISPLAY_CURSOR_IMAGE_WORDS` words each, allocated once, never in guest
    // memory.
    cursor_width: u32,
    cursor_height: u32,
    cursor_format: u32,
    cursor_hotspot: u32,
    cursor_index: u32,
    cursor_position: u32,
    cursor_enabled: bool,
    cursor_data_error: u8,
    cursor_image_error: u8,
    cursor_position_error: u8,
    cursor_enable_error: u8,
    staging: Box<[u32]>,
    active: Box<[u32]>,
    /// Parameters of `active`; meaningful only while `shape_seq != 0`.
    active_shape: CursorShapeInfo,
    cursor_seq: u64,
    shape_seq: u64,
}

impl DisplayState {
    fn new() -> Self {
        Self::with_buffers(
            vec![0; DISPLAY_CURSOR_IMAGE_WORDS].into_boxed_slice(),
            vec![0; DISPLAY_CURSOR_IMAGE_WORDS].into_boxed_slice(),
        )
    }

    /// Power-on state around zero-filled cursor buffers.
    fn with_buffers(staging: Box<[u32]>, active: Box<[u32]>) -> Self {
        debug_assert!(staging.len() == DISPLAY_CURSOR_IMAGE_WORDS);
        debug_assert!(active.len() == DISPLAY_CURSOR_IMAGE_WORDS);
        Self {
            requested_width: 0,
            requested_height: 0,
            requested_format: 0,
            mode: DisplayMode::NONE,
            frame_since_commit: false,
            commit_error: ERR_NONE,
            frame_error: ERR_NONE,
            generation: 0,
            mode_change_count: 0,
            frame_mode_epoch: 0,
            driver_protocol: 0,
            queue: VecDeque::new(),
            latch: QueuedEvent {
                seq: 0,
                kind: 0,
                arg: [0; 4],
            },
            next_event_seq: 1,
            mode_request: None,
            response_seq_lo: 0,
            response_seq_hi: 0,
            pop_error: ERR_NONE,
            response_error: ERR_NONE,
            protocol_error: ERR_NONE,
            cursor_width: 0,
            cursor_height: 0,
            cursor_format: 0,
            cursor_hotspot: 0,
            cursor_index: 0,
            cursor_position: 0,
            cursor_enabled: false,
            cursor_data_error: ERR_NONE,
            cursor_image_error: ERR_NONE,
            cursor_position_error: ERR_NONE,
            cursor_enable_error: ERR_NONE,
            staging,
            active,
            active_shape: CursorShapeInfo {
                shape_seq: 0,
                width: 0,
                height: 0,
                format: 0,
                hot_x: 0,
                hot_y: 0,
            },
            cursor_seq: 0,
            shape_seq: 0,
        }
    }

    /// Host reset: registers and mode return to power-on values; `generation`
    /// and `frame_mode_epoch` are kept so the generation stays monotonic.
    /// Version 2: the driver protocol is switched off like
    /// `DRIVER_PROTOCOL <- 0` (an open request becomes `Cancelled`),
    /// `next_event_seq` is kept so event numbers are never reused, and the
    /// last request stays visible to the host.
    fn reset(&mut self) {
        let was_active = self.mode.is_active();
        self.cancel_open_request();
        // Cursor (section 15.9): the shown state changes if a shape existed,
        // the cursor was enabled or the position was not (0, 0). At the
        // counter limit the reset still hides the cursor but the counter stays.
        let cursor_changed =
            self.shape_seq != 0 || self.cursor_enabled || self.cursor_position != 0;
        let cursor_seq = if cursor_changed {
            self.cursor_seq.saturating_add(1)
        } else {
            self.cursor_seq
        };
        // Reuse the two 16 KiB buffers: zero them instead of reallocating.
        let mut staging = std::mem::take(&mut self.staging);
        let mut active = std::mem::take(&mut self.active);
        staging.fill(0);
        active.fill(0);
        *self = Self {
            generation: self.generation,
            mode_change_count: self.mode_change_count,
            frame_mode_epoch: self.frame_mode_epoch,
            next_event_seq: self.next_event_seq,
            mode_request: self.mode_request,
            cursor_seq,
            ..Self::with_buffers(staging, active)
        };
        if was_active {
            self.mode_change_count = self.mode_change_count.saturating_add(1);
        }
    }

    /// A frame of the current mode exists (DISPLAY_DEVICE.md, section 6).
    fn has_current_frame(&self) -> bool {
        self.mode.is_active()
            && self.generation > 0
            && self.frame_mode_epoch == self.mode_change_count
    }

    fn status(&self) -> u32 {
        let mut status = 0;
        if self.mode.is_active() {
            status |= STATUS_ACTIVE;
        }
        if self.frame_since_commit {
            status |= STATUS_FRAME_SINCE_COMMIT;
        }
        status |= u32::from(self.commit_error) << STATUS_COMMIT_ERROR_SHIFT;
        status |= u32::from(self.frame_error) << STATUS_FRAME_ERROR_SHIFT;
        status
    }

    fn validate_access(access: DeviceAccess) -> Result<u64, DeviceError> {
        if access.space != IoSpace::Mmio {
            return Err(DeviceError::UnsupportedSpace);
        }
        if access.width != AccessWidth::Dword {
            return Err(DeviceError::UnsupportedWidth);
        }
        if access.offset % 4 != 0 {
            return Err(DeviceError::Misaligned);
        }
        if access.offset > DISPLAY_REGISTERS_BYTES - 4 {
            return Err(DeviceError::UnknownOffset);
        }
        Ok(access.offset)
    }

    fn read(&self, access: DeviceAccess) -> Result<u64, DeviceError> {
        let offset = Self::validate_access(access)?;
        let value = match offset {
            reg::ID => ID,
            reg::VERSION => VERSION,
            reg::CAPS => CAPS,
            reg::MAX_WIDTH => DISPLAY_MAX_WIDTH,
            reg::MAX_HEIGHT => DISPLAY_MAX_HEIGHT,
            reg::FORMATS => FORMATS,
            reg::FB_BASE_LO => DISPLAY_FRAMEBUFFER_BASE as u32,
            reg::FB_BASE_HI => (DISPLAY_FRAMEBUFFER_BASE >> 32) as u32,
            reg::FB_SIZE => DISPLAY_FRAMEBUFFER_BYTES as u32,
            reg::WIDTH => self.requested_width,
            reg::HEIGHT => self.requested_height,
            reg::FORMAT => self.requested_format,
            reg::ACTIVE_WIDTH => self.mode.width,
            reg::ACTIVE_HEIGHT => self.mode.height,
            reg::ACTIVE_FORMAT => self.mode.format,
            reg::STRIDE => self.mode.stride,
            reg::STATUS => self.status(),
            reg::FRAME_GENERATION => self.generation as u32,
            reg::FRAME_GENERATION_HI => (self.generation >> 32) as u32,
            reg::DRIVER_PROTOCOLS => DRIVER_PROTOCOLS,
            reg::DRIVER_PROTOCOL => self.driver_protocol,
            reg::EVENT_STATUS => self.event_status(),
            reg::MODE_COUNT => MODE_LIST.len() as u32,
            // Invariant: the queue is empty while `driver_protocol == 0`.
            reg::EVENT_PENDING => self.queue.len() as u32,
            reg::EVENT_CAPACITY => DISPLAY_EVENT_CAPACITY as u32,
            reg::EVENT_SEQ_LO => self.latch.seq as u32,
            reg::EVENT_SEQ_HI => (self.latch.seq >> 32) as u32,
            reg::EVENT_TYPE => self.latch.kind,
            reg::EVENT_ARG0 => self.latch.arg[0],
            reg::EVENT_ARG1 => self.latch.arg[1],
            reg::EVENT_ARG2 => self.latch.arg[2],
            reg::EVENT_ARG3 => self.latch.arg[3],
            reg::RESPONSE_SEQ_LO => self.response_seq_lo,
            reg::RESPONSE_SEQ_HI => self.response_seq_hi,
            // The block exists only while `CAPS` bit 2 is set (section 15.2).
            reg::CURSOR_BLOCK_START..reg::CURSOR_BLOCK_END if CAPS & CAPS_CURSOR != 0 => {
                self.cursor_read(offset)
            }
            reg::MODE_TABLE..reg::MODE_TABLE_END => Self::mode_table_word(offset),
            // MODE_COMMIT, EVENT_POP, REQUEST_RESPONSE (write-only) and every
            // reserved offset read as zero.
            _ => 0,
        };
        Ok(u64::from(value))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        let offset = Self::validate_access(access)?;
        let value = value as u32;
        match offset {
            reg::WIDTH => self.requested_width = value,
            reg::HEIGHT => self.requested_height = value,
            reg::FORMAT => self.requested_format = value,
            reg::MODE_COMMIT => self.mode_commit(value),
            reg::FRAME_GENERATION => self.doorbell(),
            reg::DRIVER_PROTOCOL => self.write_driver_protocol(value),
            reg::EVENT_POP => self.event_pop(value),
            reg::RESPONSE_SEQ_LO => self.response_seq_lo = value,
            reg::RESPONSE_SEQ_HI => self.response_seq_hi = value,
            reg::REQUEST_RESPONSE => self.request_response(value),
            reg::CURSOR_BLOCK_START..reg::CURSOR_BLOCK_END if CAPS & CAPS_CURSOR != 0 => {
                self.cursor_write(offset, value);
            }
            // Read-only and reserved offsets: the write is ignored.
            _ => {}
        }
        Ok(())
    }

    /// Cursor block read (section 15.4). Write-only registers and reserved
    /// offsets read as zero.
    fn cursor_read(&self, offset: u64) -> u32 {
        match offset {
            reg::CURSOR_MAX_WIDTH => DISPLAY_CURSOR_MAX_WIDTH,
            reg::CURSOR_MAX_HEIGHT => DISPLAY_CURSOR_MAX_HEIGHT,
            reg::CURSOR_FORMATS => CURSOR_FORMATS,
            reg::CURSOR_STATUS => self.cursor_status(),
            reg::CURSOR_WIDTH => self.cursor_width,
            reg::CURSOR_HEIGHT => self.cursor_height,
            reg::CURSOR_FORMAT => self.cursor_format,
            reg::CURSOR_HOTSPOT => self.cursor_hotspot,
            reg::CURSOR_IMAGE_INDEX => self.cursor_index,
            reg::CURSOR_POSITION => self.cursor_position,
            reg::CURSOR_ENABLE => u32::from(self.cursor_enabled),
            _ => 0,
        }
    }

    /// Cursor block write (section 15.4). Read-only registers and reserved
    /// offsets ignore the write.
    fn cursor_write(&mut self, offset: u64, value: u32) {
        match offset {
            reg::CURSOR_WIDTH => self.cursor_width = value,
            reg::CURSOR_HEIGHT => self.cursor_height = value,
            reg::CURSOR_FORMAT => self.cursor_format = value,
            reg::CURSOR_HOTSPOT => self.cursor_hotspot = value,
            reg::CURSOR_IMAGE_INDEX => self.cursor_index = value,
            reg::CURSOR_IMAGE_DATA => self.cursor_image_data(value),
            reg::CURSOR_IMAGE_COMMIT => self.cursor_image_commit(value),
            reg::CURSOR_POSITION => self.cursor_set_position(value),
            reg::CURSOR_ENABLE => self.cursor_set_enable(value),
            _ => {}
        }
    }

    fn cursor_status(&self) -> u32 {
        u32::from(self.cursor_data_error)
            | u32::from(self.cursor_image_error) << CURSOR_STATUS_IMAGE_SHIFT
            | u32::from(self.cursor_position_error) << CURSOR_STATUS_POSITION_SHIFT
            | u32::from(self.cursor_enable_error) << CURSOR_STATUS_ENABLE_SHIFT
    }

    /// `CURSOR_IMAGE_DATA <- v` (15.6.1): the index is compared as a 32-bit
    /// value; only `staging` and the index change.
    fn cursor_image_data(&mut self, value: u32) {
        let index = self.cursor_index as usize;
        if index >= DISPLAY_CURSOR_IMAGE_WORDS {
            self.cursor_data_error = ERR_INDEX_OUT_OF_RANGE;
            return;
        }
        self.staging[index] = value;
        self.cursor_index += 1;
        self.cursor_data_error = ERR_NONE;
    }

    /// `CURSOR_IMAGE_COMMIT <- v` (15.6.2): checks in order, a failure changes
    /// `IMAGE_ERROR` only.
    fn cursor_image_commit(&mut self, command: u32) {
        match self.validate_cursor_commit(command) {
            Err(code) => self.cursor_image_error = code,
            Ok(mut shape) => {
                let words = shape.width as usize * shape.height as usize;
                self.active[..words].copy_from_slice(&self.staging[..words]);
                // Words past `W x H` are not part of the shape: keep `active`
                // a function of the published bytes only.
                self.active[words..].fill(0);
                self.cursor_seq += 1;
                self.shape_seq = self.cursor_seq;
                shape.shape_seq = self.shape_seq;
                self.active_shape = shape;
                self.cursor_image_error = ERR_NONE;
            }
        }
    }

    fn validate_cursor_commit(&self, command: u32) -> Result<CursorShapeInfo, u8> {
        if command != 1 {
            return Err(ERR_INVALID_COMMAND);
        }
        if self.cursor_width == 0 || self.cursor_height == 0 {
            return Err(ERR_SIZE_NOT_SET);
        }
        if self.cursor_width > DISPLAY_CURSOR_MAX_WIDTH
            || self.cursor_height > DISPLAY_CURSOR_MAX_HEIGHT
        {
            return Err(ERR_UNSUPPORTED_SIZE);
        }
        if self.cursor_format >= 32 || CURSOR_FORMATS & (1 << self.cursor_format) == 0 {
            return Err(ERR_UNSUPPORTED_FORMAT);
        }
        let hot_x = self.cursor_hotspot & 0xFFFF;
        let hot_y = self.cursor_hotspot >> 16;
        if hot_x >= self.cursor_width || hot_y >= self.cursor_height {
            return Err(ERR_HOTSPOT_OUT_OF_IMAGE);
        }
        if self.cursor_seq == u64::MAX {
            return Err(ERR_COUNTER_EXHAUSTED);
        }
        Ok(CursorShapeInfo {
            shape_seq: 0,
            width: self.cursor_width,
            height: self.cursor_height,
            format: self.cursor_format,
            hot_x,
            hot_y,
        })
    }

    /// `CURSOR_POSITION <- v` (15.6.3). The pair is checked against the
    /// active frame; a failure keeps the previous value.
    fn cursor_set_position(&mut self, value: u32) {
        if !self.mode.is_active() {
            self.cursor_position_error = ERR_NO_ACTIVE_MODE;
            return;
        }
        if (value & 0xFFFF) >= self.mode.width || (value >> 16) >= self.mode.height {
            self.cursor_position_error = ERR_POSITION_OUT_OF_FRAME;
            return;
        }
        let changed = value != self.cursor_position;
        if changed && self.cursor_seq == u64::MAX {
            self.cursor_position_error = ERR_COUNTER_EXHAUSTED;
            return;
        }
        self.cursor_position = value;
        if changed {
            self.cursor_seq += 1;
        }
        self.cursor_position_error = ERR_NONE;
    }

    /// `CURSOR_ENABLE <- v` (15.6.4).
    fn cursor_set_enable(&mut self, value: u32) {
        if value > 1 {
            self.cursor_enable_error = ERR_INVALID_COMMAND;
            return;
        }
        if value == 1 && self.shape_seq == 0 {
            self.cursor_enable_error = ERR_NO_CURSOR_IMAGE;
            return;
        }
        let enabled = value == 1;
        let changed = enabled != self.cursor_enabled;
        if changed && self.cursor_seq == u64::MAX {
            self.cursor_enable_error = ERR_COUNTER_EXHAUSTED;
            return;
        }
        self.cursor_enabled = enabled;
        if changed {
            self.cursor_seq += 1;
        }
        self.cursor_enable_error = ERR_NONE;
    }

    fn cursor_state(&self) -> CursorState {
        CursorState {
            cursor_seq: self.cursor_seq,
            enabled: self.cursor_enabled,
            x: self.cursor_position & 0xFFFF,
            y: self.cursor_position >> 16,
            shape: (self.shape_seq != 0).then_some(self.active_shape),
        }
    }

    /// Copies `active` as little-endian words (bytes B, G, R, A) into `output`.
    fn read_cursor_image_into(
        &self,
        output: &mut [u8],
    ) -> Result<CursorShapeInfo, CursorReadError> {
        if self.shape_seq == 0 {
            return Err(CursorReadError::NoShape);
        }
        let shape = self.active_shape;
        let words = shape.width as usize * shape.height as usize;
        let expected = words * 4;
        if output.len() != expected {
            return Err(CursorReadError::BufferLength {
                expected,
                actual: output.len(),
            });
        }
        for (chunk, word) in output.chunks_exact_mut(4).zip(&self.active[..words]) {
            chunk.copy_from_slice(&word.to_le_bytes());
        }
        Ok(shape)
    }

    /// Checks in the exact order of DISPLAY_DEVICE.md section 5. A failure
    /// changes `commit_error` only.
    fn mode_commit(&mut self, command: u32) {
        match self.validate_commit(command) {
            Err(code) => self.commit_error = code,
            Ok(mode) => {
                self.mode = mode;
                self.frame_since_commit = false;
                self.commit_error = ERR_NONE;
                self.mode_change_count += 1;
            }
        }
    }

    fn validate_commit(&self, command: u32) -> Result<DisplayMode, u8> {
        if command != 1 {
            return Err(ERR_INVALID_COMMAND);
        }
        let mode = Self::validate_mode(
            self.requested_width,
            self.requested_height,
            self.requested_format,
        )?;
        if self.mode_change_count == u64::MAX {
            return Err(ERR_COUNTER_EXHAUSTED);
        }
        Ok(mode)
    }

    /// `MODE_COMMIT` checks 2-4 (DISPLAY_DEVICE.md, section 5); also the
    /// validity rule of `MODE_REQUEST` arguments.
    fn validate_mode(width: u32, height: u32, format: u32) -> Result<DisplayMode, u8> {
        if width == 0 || height == 0 {
            return Err(ERR_SIZE_NOT_SET);
        }
        // Limits are compared before any multiplication (section 2.1).
        if width > DISPLAY_MAX_WIDTH || height > DISPLAY_MAX_HEIGHT {
            return Err(ERR_UNSUPPORTED_SIZE);
        }
        let stride = width * BYTES_PER_PIXEL;
        if u64::from(stride) * u64::from(height) > DISPLAY_FRAMEBUFFER_BYTES {
            return Err(ERR_UNSUPPORTED_SIZE);
        }
        if format >= 32 || FORMATS & (1 << format) == 0 {
            return Err(ERR_UNSUPPORTED_FORMAT);
        }
        Ok(DisplayMode {
            width,
            height,
            stride,
            format,
        })
    }

    /// `FRAME_GENERATION` write: the written value is discarded.
    fn doorbell(&mut self) {
        if !self.mode.is_active() {
            self.frame_error = ERR_NO_ACTIVE_MODE;
        } else if self.generation == u64::MAX {
            self.frame_error = ERR_COUNTER_EXHAUSTED;
        } else {
            self.generation += 1;
            self.frame_mode_epoch = self.mode_change_count;
            self.frame_since_commit = true;
            self.frame_error = ERR_NONE;
        }
    }

    fn event_status(&self) -> u32 {
        u32::from(self.pop_error)
            | u32::from(self.response_error) << EVENT_STATUS_RESPONSE_SHIFT
            | u32::from(self.protocol_error) << EVENT_STATUS_PROTOCOL_SHIFT
    }

    /// One dword of the read-only mode table at `0x400 + 16 * i + 4 * field`.
    /// Entries at or beyond `MODE_COUNT` and the reserved field read as zero.
    fn mode_table_word(offset: u64) -> u32 {
        let relative = offset - reg::MODE_TABLE;
        let index = relative / 16;
        debug_assert!(index < MODE_TABLE_ENTRIES);
        let Some(&(width, height)) = usize::try_from(index)
            .ok()
            .and_then(|index| MODE_LIST.get(index))
        else {
            return 0;
        };
        match relative % 16 {
            0x0 => width,
            0x4 => height,
            0x8 => DISPLAY_FORMAT_BGRX8888,
            _ => 0,
        }
    }

    /// `DRIVER_PROTOCOL <- v` (DISPLAY_DEVICE.md, 14.9): "RW with check".
    fn write_driver_protocol(&mut self, value: u32) {
        if value == 0 {
            self.disable_driver_protocol();
            self.protocol_error = ERR_NONE;
        } else if value < 32 && DRIVER_PROTOCOLS & (1 << value) != 0 {
            if self.driver_protocol == 0 {
                // The queue is already empty while the protocol is off.
                self.driver_protocol = value;
                self.protocol_error = ERR_NONE;
            } else if self.driver_protocol == value {
                self.protocol_error = ERR_NONE;
            } else {
                // Version changes go through `0`.
                self.protocol_error = ERR_UNSUPPORTED_PROTOCOL;
            }
        } else {
            self.protocol_error = ERR_UNSUPPORTED_PROTOCOL;
        }
    }

    /// Protocol off: empty queue, zero latch, open request `Cancelled`. Mode,
    /// framebuffer, `RESPONSE_SEQ_*` and `next_event_seq` stay.
    fn disable_driver_protocol(&mut self) {
        self.driver_protocol = 0;
        self.queue.clear();
        self.latch = QueuedEvent::default();
        self.cancel_open_request();
    }

    fn cancel_open_request(&mut self) {
        if let Some(request) = &mut self.mode_request
            && request.outcome == ModeRequestOutcome::Pending
        {
            request.outcome = ModeRequestOutcome::Cancelled;
        }
    }

    /// `EVENT_POP <- v` (14.7): the first failing check wins and changes
    /// `pop_error` only.
    fn event_pop(&mut self, command: u32) {
        if command != 1 {
            self.pop_error = ERR_INVALID_COMMAND;
        } else if self.driver_protocol == 0 {
            self.pop_error = ERR_NOT_ENABLED;
        } else if let Some(head) = self.queue.pop_front() {
            self.latch = head;
            self.pop_error = ERR_NONE;
        } else {
            self.pop_error = ERR_QUEUE_EMPTY;
        }
    }

    /// `REQUEST_RESPONSE <- v` (14.8): the first failing check wins and
    /// changes `response_error` only; the request stays open.
    fn request_response(&mut self, value: u32) {
        let Some(outcome) = Self::decode_response(value) else {
            self.response_error = ERR_INVALID_COMMAND;
            return;
        };
        if self.driver_protocol == 0 {
            self.response_error = ERR_NOT_ENABLED;
            return;
        }
        let seq = u64::from(self.response_seq_hi) << 32 | u64::from(self.response_seq_lo);
        let mode = self.mode;
        let Some(request) = self
            .mode_request
            .as_mut()
            .filter(|request| request.outcome == ModeRequestOutcome::Pending && request.seq == seq)
        else {
            self.response_error = ERR_STALE_REQUEST;
            return;
        };
        if outcome == ModeRequestOutcome::Accepted
            && (!mode.is_active()
                || (mode.width, mode.height, mode.format)
                    != (request.width, request.height, request.format))
        {
            self.response_error = ERR_MODE_MISMATCH;
            return;
        }
        request.outcome = outcome;
        self.response_error = ERR_NONE;
    }

    /// Decodes `REQUEST_RESPONSE` (DISPLAY_DRIVER_V1.md, 6.1): bits 7:0
    /// verdict, 15:8 reason, 31:16 zero. `ACCEPT` takes reason `0`; `REJECT`
    /// takes `1..=3`.
    fn decode_response(value: u32) -> Option<ModeRequestOutcome> {
        let verdict = value & 0xFF;
        let reason = (value >> 8) & 0xFF;
        if value >> 16 != 0 {
            return None;
        }
        match verdict {
            VERDICT_ACCEPT if reason == 0 => Some(ModeRequestOutcome::Accepted),
            VERDICT_REJECT => {
                DisplayRejectReason::from_code(reason).map(ModeRequestOutcome::Rejected)
            }
            _ => None,
        }
    }

    /// Host-side `MODE_REQUEST` posting (14.7, 14.8). Checks run in order and
    /// all of them before any change.
    fn post_mode_request(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<u64, DisplayRequestError> {
        // The entry removed by merging: an untaken `MODE_REQUEST` of screen 0.
        let merged = self
            .queue
            .iter()
            .position(|event| event.kind == EVENT_MODE_REQUEST && event.arg[3] == 0);
        if self.driver_protocol == 0 {
            return Err(DisplayRequestError::NoDriver);
        }
        if Self::validate_mode(width, height, format).is_err() {
            return Err(DisplayRequestError::InvalidMode);
        }
        if self.next_event_seq == u64::MAX {
            return Err(DisplayRequestError::SequenceExhausted);
        }
        if self.queue.len() - usize::from(merged.is_some()) >= DISPLAY_EVENT_CAPACITY {
            return Err(DisplayRequestError::QueueFull);
        }
        if let Some(index) = merged {
            self.queue.remove(index);
        }
        if let Some(previous) = &mut self.mode_request
            && previous.outcome == ModeRequestOutcome::Pending
        {
            previous.outcome = ModeRequestOutcome::Superseded;
        }
        let seq = self.next_event_seq;
        self.next_event_seq += 1;
        self.queue.push_back(QueuedEvent {
            seq,
            kind: EVENT_MODE_REQUEST,
            arg: [width, height, format, 0],
        });
        self.mode_request = Some(ModeRequestState {
            seq,
            width,
            height,
            format,
            outcome: ModeRequestOutcome::Pending,
        });
        Ok(seq)
    }
}

/// The display register block, bound to an MMIO region of
/// [`DISPLAY_REGISTERS_BYTES`] bytes. Host access goes through the
/// [`DisplayHandle`] obtained at construction.
#[derive(Clone, Debug)]
pub struct DisplayDevice {
    state: Arc<Mutex<DisplayState>>,
}

/// Typed host-side handle aliasing the state of one [`DisplayDevice`].
///
/// Call it only between guest quanta, from the thread that owns the
/// `MachineBus` and executes quanta. The handle never changes registers except
/// through [`DisplayHandle::reset`].
#[derive(Clone, Debug)]
pub struct DisplayHandle {
    state: Arc<Mutex<DisplayState>>,
}

impl DisplayDevice {
    /// Creates a powered-on device and the handle that aliases its state.
    pub fn new_with_handle() -> (Self, DisplayHandle) {
        let state = Arc::new(Mutex::new(DisplayState::new()));
        (
            Self {
                state: Arc::clone(&state),
            },
            DisplayHandle { state },
        )
    }
}

fn lock(state: &Mutex<DisplayState>) -> std::sync::MutexGuard<'_, DisplayState> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl BusDevice for DisplayDevice {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        lock(&self.state).read(access)
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        lock(&self.state).write(access, value)
    }

    fn framebuffer_binding(&self) -> Option<FramebufferBinding> {
        Some(FramebufferBinding {
            base: GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE),
            size: DISPLAY_FRAMEBUFFER_BYTES,
        })
    }
}

impl DisplayHandle {
    /// The active mode, or [`DisplayMode::NONE`].
    pub fn current_mode(&self) -> DisplayMode {
        lock(&self.state).mode
    }

    /// Number of successful doorbells since the device was created.
    pub fn frame_generation(&self) -> u64 {
        lock(&self.state).generation
    }

    /// Number of mode changes: successful commits plus host resets that
    /// removed an active mode.
    pub fn mode_change_count(&self) -> u64 {
        lock(&self.state).mode_change_count
    }

    /// `ACTIVE && generation > 0 && frame_mode_epoch == mode_change_count`.
    pub fn has_current_frame(&self) -> bool {
        lock(&self.state).has_current_frame()
    }

    /// The enabled driver protocol version; `0` means no driver
    /// (DISPLAY_DEVICE.md, 14.13).
    pub fn driver_protocol(&self) -> u32 {
        lock(&self.state).driver_protocol
    }

    /// Posts a `MODE_REQUEST` for screen 0 and returns its event number.
    ///
    /// Merges with an untaken `MODE_REQUEST` and supersedes an open one. A
    /// refusal changes nothing (DISPLAY_DEVICE.md, 14.7-14.8). Call it only
    /// between guest quanta, like the rest of this handle.
    pub fn post_mode_request(
        &self,
        width: u32,
        height: u32,
        format: u32,
    ) -> Result<u64, DisplayRequestError> {
        lock(&self.state).post_mode_request(width, height, format)
    }

    /// The latest posted request, or `None` if none was ever posted.
    pub fn mode_request_state(&self) -> Option<ModeRequestState> {
        lock(&self.state).mode_request
    }

    /// The cursor as the host should show it: sequence number, visibility,
    /// hotspot position and the published shape parameters (15.10). Read-only.
    pub fn cursor_state(&self) -> CursorState {
        lock(&self.state).cursor_state()
    }

    /// Copies exactly `width * height * 4` bytes of the published cursor image
    /// (`BGRA8888_PREMUL`, rows packed) into `output` and returns the
    /// parameters of the copied shape. The copy and the parameters are taken
    /// under one lock, so `shape_seq` always matches the bytes. Read-only; no
    /// bus is needed because the image lives in the device (15.10).
    pub fn read_cursor_image_into(
        &self,
        output: &mut [u8],
    ) -> Result<CursorShapeInfo, CursorReadError> {
        lock(&self.state).read_cursor_image_into(output)
    }

    /// Host reset (DISPLAY_DEVICE.md, sections 10 and 14.12). Framebuffer
    /// bytes are not cleared and `generation` is preserved.
    pub fn reset(&self) {
        lock(&self.state).reset();
    }

    /// Copies exactly `stride * height` bytes from the start of the
    /// framebuffer region of `bus` into `output`.
    ///
    /// `bus` must be the bus that owns this device. The call only reads: it
    /// does not ring the doorbell, clear errors or touch registers.
    pub fn read_frame_into(
        &self,
        bus: &MachineBus,
        output: &mut [u8],
    ) -> Result<FrameInfo, DisplayReadError> {
        self.read_frame_with(output, |destination| {
            bus.read_framebuffer_bytes(
                GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE),
                destination,
            )
            .map_err(DisplayReadError::Memory)
        })
    }

    /// The mode-change counter is sampled before and after `copy`; the lock is
    /// not held across the copy.
    fn read_frame_with(
        &self,
        output: &mut [u8],
        copy: impl FnOnce(&mut [u8]) -> Result<(), DisplayReadError>,
    ) -> Result<FrameInfo, DisplayReadError> {
        let before = {
            let state = lock(&self.state);
            if !state.mode.is_active() {
                return Err(DisplayReadError::NoActiveMode);
            }
            if !state.has_current_frame() {
                return Err(DisplayReadError::NoFrame);
            }
            FrameInfo {
                mode: state.mode,
                generation: state.generation,
                mode_change_count: state.mode_change_count,
            }
        };
        let expected = usize::try_from(before.mode.frame_bytes()).map_err(|_| {
            DisplayReadError::BufferLength {
                expected: usize::MAX,
                actual: output.len(),
            }
        })?;
        if output.len() != expected {
            return Err(DisplayReadError::BufferLength {
                expected,
                actual: output.len(),
            });
        }
        copy(output)?;
        if lock(&self.state).mode_change_count != before.mode_change_count {
            return Err(DisplayReadError::ModeChanged);
        }
        Ok(before)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BusAccessError, BusBuildError, DisplayBindingError, GuestPhysicalRange, MachineMemoryMap,
        MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
    };
    use std::cell::Cell;

    const RAM_BYTES: u64 = 0x10_000;

    fn range(start: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
    }

    fn display_regions() -> Vec<MemoryRegionSpec> {
        vec![
            MemoryRegionSpec::new(range(0, RAM_BYTES), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                MemoryRegionKind::Framebuffer,
            ),
            MemoryRegionSpec::new(
                range(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
                MemoryRegionKind::Mmio,
            ),
        ]
    }

    fn try_bus(
        regions: Vec<MemoryRegionSpec>,
        device: DisplayDevice,
    ) -> Result<MachineBus, BusBuildError> {
        MachineBus::new(
            MachineMemoryMap::new(36, regions).unwrap(),
            RamLimits::new(RAM_BYTES),
            vec![Box::new(device)],
            vec![MmioBinding {
                region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
                device_index: 0,
            }],
            vec![],
        )
    }

    fn setup() -> (MachineBus, DisplayHandle) {
        let (device, handle) = DisplayDevice::new_with_handle();
        (try_bus(display_regions(), device).unwrap(), handle)
    }

    fn reg_addr(offset: u64) -> GuestPhysicalAddress {
        GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE + offset)
    }

    fn read_reg(bus: &mut MachineBus, offset: u64) -> u32 {
        bus.read_physical(reg_addr(offset), AccessWidth::Dword)
            .unwrap() as u32
    }

    fn write_reg(bus: &mut MachineBus, offset: u64, value: u32) {
        bus.write_physical(
            reg_addr(offset),
            AccessWidth::Dword,
            u64::from(value),
            |_| panic!("register writes must not reach the committed-write callback"),
        )
        .unwrap();
    }

    fn set_mode(bus: &mut MachineBus, width: u32, height: u32, format: u32) {
        write_reg(bus, reg::WIDTH, width);
        write_reg(bus, reg::HEIGHT, height);
        write_reg(bus, reg::FORMAT, format);
    }

    fn commit(bus: &mut MachineBus) {
        write_reg(bus, reg::MODE_COMMIT, 1);
    }

    fn commit_error(bus: &mut MachineBus) -> u8 {
        (read_reg(bus, reg::STATUS) >> 8) as u8
    }

    fn frame_error(bus: &mut MachineBus) -> u8 {
        (read_reg(bus, reg::STATUS) >> 16) as u8
    }

    #[test]
    fn reset_values_match_the_contract() {
        let (mut bus, handle) = setup();
        for (offset, expected) in [
            (reg::ID, 0x4246_444F),
            (reg::VERSION, 2),
            (reg::CAPS, 7),
            (reg::MAX_WIDTH, 4096),
            (reg::MAX_HEIGHT, 2304),
            (reg::FORMATS, 2),
            (reg::FB_BASE_LO, 0xF800_0000),
            (reg::FB_BASE_HI, 0),
            (reg::FB_SIZE, 0x0240_0000),
            (reg::WIDTH, 0),
            (reg::HEIGHT, 0),
            (reg::FORMAT, 0),
            (reg::MODE_COMMIT, 0),
            (reg::ACTIVE_WIDTH, 0),
            (reg::ACTIVE_HEIGHT, 0),
            (reg::ACTIVE_FORMAT, 0),
            (reg::STRIDE, 0),
            (reg::STATUS, 0),
            (reg::FRAME_GENERATION, 0),
            (reg::FRAME_GENERATION_HI, 0),
            (0x024, 0),
            (0x0C0, 0),
            (0x0C4, 0),
            (0x0C8, 0),
            (0xFFC, 0),
        ] {
            assert_eq!(read_reg(&mut bus, offset), expected, "offset {offset:#x}");
        }
        assert_eq!(handle.current_mode(), DisplayMode::NONE);
        assert_eq!(handle.frame_generation(), 0);
        assert_eq!(handle.mode_change_count(), 0);
        assert!(!handle.has_current_frame());
    }

    #[test]
    fn successful_commit_changes_mode_atomically() {
        let (mut bus, handle) = setup();
        set_mode(&mut bus, 640, 480, DISPLAY_FORMAT_BGRX8888);
        // Staged values do not change the active mode.
        assert_eq!(read_reg(&mut bus, reg::ACTIVE_WIDTH), 0);
        commit(&mut bus);
        assert_eq!(read_reg(&mut bus, reg::ACTIVE_WIDTH), 640);
        assert_eq!(read_reg(&mut bus, reg::ACTIVE_HEIGHT), 480);
        assert_eq!(read_reg(&mut bus, reg::ACTIVE_FORMAT), 1);
        assert_eq!(read_reg(&mut bus, reg::STRIDE), 2560);
        assert_eq!(read_reg(&mut bus, reg::STATUS), 0x0000_0001);
        assert_eq!(
            handle.current_mode(),
            DisplayMode {
                width: 640,
                height: 480,
                stride: 2560,
                format: 1
            }
        );
        assert_eq!(handle.mode_change_count(), 1);
        // The same mode again still counts as a mode change.
        commit(&mut bus);
        assert_eq!(handle.mode_change_count(), 2);
        // Maximum mode is accepted: 4096 x 2304 x 4 == FB_SIZE.
        set_mode(&mut bus, 4096, 2304, 1);
        commit(&mut bus);
        assert_eq!(handle.current_mode().frame_bytes(), 0x0240_0000);
        assert_eq!(handle.mode_change_count(), 3);
        // Minimum mode.
        set_mode(&mut bus, 1, 1, 1);
        commit(&mut bus);
        assert_eq!(handle.current_mode().stride, 4);
    }

    #[test]
    fn every_commit_failure_sets_only_commit_error_and_keeps_the_old_mode() {
        let (mut bus, handle) = setup();
        // From reset: no sizes yet.
        commit(&mut bus);
        assert_eq!(commit_error(&mut bus), ERR_SIZE_NOT_SET);
        assert_eq!(handle.mode_change_count(), 0);
        assert_eq!(handle.current_mode(), DisplayMode::NONE);

        set_mode(&mut bus, 800, 600, 1);
        commit(&mut bus);
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        let mode = handle.current_mode();
        let status_before = read_reg(&mut bus, reg::STATUS);
        assert_eq!(status_before & 0b11, 0b11);

        let cases: [(&str, u32, u32, u32, u32, u8); 14] = [
            ("zero width", 0, 600, 1, 1, ERR_SIZE_NOT_SET),
            ("zero height", 800, 0, 1, 1, ERR_SIZE_NOT_SET),
            ("wide", 4097, 600, 1, 1, ERR_UNSUPPORTED_SIZE),
            ("tall", 800, 2305, 1, 1, ERR_UNSUPPORTED_SIZE),
            (
                "huge no overflow",
                u32::MAX,
                u32::MAX,
                1,
                1,
                ERR_UNSUPPORTED_SIZE,
            ),
            // Size checks take priority over a bad format.
            ("zero width, bad format", 0, 600, 0, 1, ERR_SIZE_NOT_SET),
            ("zero height, bad format", 800, 0, 32, 1, ERR_SIZE_NOT_SET),
            ("wide, bad format", 4097, 600, 0, 1, ERR_UNSUPPORTED_SIZE),
            ("tall, bad format", 800, 2305, 32, 1, ERR_UNSUPPORTED_SIZE),
            ("format zero", 800, 600, 0, 1, ERR_UNSUPPORTED_FORMAT),
            ("format 2", 800, 600, 2, 1, ERR_UNSUPPORTED_FORMAT),
            ("format 32", 800, 600, 32, 1, ERR_UNSUPPORTED_FORMAT),
            ("command 0", 800, 600, 1, 0, ERR_INVALID_COMMAND),
            ("command 2", 800, 600, 1, 2, ERR_INVALID_COMMAND),
        ];
        for (name, width, height, format, command, code) in cases {
            set_mode(&mut bus, width, height, format);
            write_reg(&mut bus, reg::MODE_COMMIT, command);
            assert_eq!(commit_error(&mut bus), code, "{name}");
            assert_eq!(handle.current_mode(), mode, "{name}");
            assert_eq!(handle.mode_change_count(), 1, "{name}");
            assert!(handle.has_current_frame(), "{name}");
            assert_eq!(
                read_reg(&mut bus, reg::STATUS) & 0xFFFF_00FF,
                status_before,
                "{name}"
            );
            // Staged registers keep the failed values.
            assert_eq!(read_reg(&mut bus, reg::WIDTH), width, "{name}");
        }
        // A command error is reported even when sizes are also bad (check order).
        set_mode(&mut bus, 0, 0, 0);
        write_reg(&mut bus, reg::MODE_COMMIT, 7);
        assert_eq!(commit_error(&mut bus), ERR_INVALID_COMMAND);

        // A later success clears COMMIT_ERROR.
        set_mode(&mut bus, 800, 600, 1);
        commit(&mut bus);
        assert_eq!(commit_error(&mut bus), ERR_NONE);
    }

    #[test]
    fn counter_exhaustion_rejects_commit_and_doorbell() {
        let mut state = DisplayState::new();
        state.requested_width = 2;
        state.requested_height = 2;
        state.requested_format = 1;
        state.mode_change_count = u64::MAX;
        state.mode_commit(1);
        assert_eq!(state.commit_error, ERR_COUNTER_EXHAUSTED);
        assert_eq!(state.mode_change_count, u64::MAX);
        assert_eq!(state.mode, DisplayMode::NONE);

        let mut state = DisplayState::new();
        state.mode = DisplayMode {
            width: 2,
            height: 2,
            stride: 8,
            format: 1,
        };
        state.generation = u64::MAX;
        state.doorbell();
        assert_eq!(state.frame_error, ERR_COUNTER_EXHAUSTED);
        assert_eq!(state.generation, u64::MAX);
        assert!(!state.frame_since_commit);
    }

    #[test]
    fn doorbell_requires_an_active_mode_and_discards_the_value() {
        let (mut bus, handle) = setup();
        write_reg(&mut bus, reg::FRAME_GENERATION, 0xDEAD_BEEF);
        assert_eq!(frame_error(&mut bus), ERR_NO_ACTIVE_MODE);
        assert_eq!(read_reg(&mut bus, reg::FRAME_GENERATION), 0);
        assert_eq!(handle.frame_generation(), 0);

        set_mode(&mut bus, 4, 4, 1);
        commit(&mut bus);
        assert_eq!(read_reg(&mut bus, reg::STATUS) & 0b11, 0b01);
        write_reg(&mut bus, reg::FRAME_GENERATION, 0xDEAD_BEEF);
        assert_eq!(read_reg(&mut bus, reg::FRAME_GENERATION), 1);
        assert_eq!(read_reg(&mut bus, reg::FRAME_GENERATION_HI), 0);
        assert_eq!(frame_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::STATUS) & 0b11, 0b11);
        assert!(handle.has_current_frame());
        // Each field is overwritten only by its own command.
        write_reg(&mut bus, reg::MODE_COMMIT, 9);
        assert_eq!(commit_error(&mut bus), ERR_INVALID_COMMAND);
        assert_eq!(frame_error(&mut bus), ERR_NONE);
        // A new commit drops the frame until the next doorbell.
        commit(&mut bus);
        assert!(!handle.has_current_frame());
        assert_eq!(read_reg(&mut bus, reg::STATUS) & 0b11, 0b01);
        assert_eq!(handle.frame_generation(), 1);
    }

    #[test]
    fn read_only_register_writes_are_ignored_without_touching_status() {
        let (mut bus, _handle) = setup();
        set_mode(&mut bus, 8, 8, 1);
        commit(&mut bus);
        write_reg(&mut bus, reg::MODE_COMMIT, 5);
        let status = read_reg(&mut bus, reg::STATUS);
        assert_ne!(status >> 8 & 0xff, 0);
        for offset in [
            reg::ID,
            reg::VERSION,
            reg::CAPS,
            reg::MAX_WIDTH,
            reg::MAX_HEIGHT,
            reg::FORMATS,
            reg::FB_BASE_LO,
            reg::FB_BASE_HI,
            reg::FB_SIZE,
            reg::ACTIVE_WIDTH,
            reg::ACTIVE_HEIGHT,
            reg::ACTIVE_FORMAT,
            reg::STRIDE,
            reg::STATUS,
            reg::FRAME_GENERATION_HI,
            0x024,
            0x0C0,
            0x0C4,
            0x0C8,
            0xFFC,
        ] {
            let before = read_reg(&mut bus, offset);
            write_reg(&mut bus, offset, 0xFFFF_FFFF);
            assert_eq!(read_reg(&mut bus, offset), before, "offset {offset:#x}");
            assert_eq!(
                read_reg(&mut bus, reg::STATUS),
                status,
                "offset {offset:#x}"
            );
        }
        assert_eq!(read_reg(&mut bus, reg::ACTIVE_WIDTH), 8);
        assert_eq!(read_reg(&mut bus, reg::FRAME_GENERATION), 0);
    }

    #[test]
    fn unsupported_widths_spaces_and_offsets_fail_before_any_state_change() {
        let (device, handle) = DisplayDevice::new_with_handle();
        let mut device = device;
        let access = |space, offset, width| DeviceAccess {
            space,
            offset,
            width,
        };
        for width in [AccessWidth::Byte, AccessWidth::Word, AccessWidth::Qword] {
            for offset in [
                reg::WIDTH,
                reg::MODE_COMMIT,
                reg::FRAME_GENERATION,
                reg::STATUS,
            ] {
                let a = access(IoSpace::Mmio, offset, width);
                assert_eq!(device.read(a), Err(DeviceError::UnsupportedWidth));
                assert_eq!(device.write(a, 1), Err(DeviceError::UnsupportedWidth));
            }
        }
        let dword = |offset| access(IoSpace::Mmio, offset, AccessWidth::Dword);
        assert_eq!(device.write(dword(0x41), 1), Err(DeviceError::Misaligned));
        assert_eq!(device.read(dword(0x42)), Err(DeviceError::Misaligned));
        assert_eq!(device.read(dword(0x1000)), Err(DeviceError::UnknownOffset));
        for offset in [u64::MAX - 3, u64::MAX - 7, 0xFFFF_FFFF_FFFF_F000] {
            assert_eq!(device.read(dword(offset)), Err(DeviceError::UnknownOffset));
            assert_eq!(
                device.write(dword(offset), 1),
                Err(DeviceError::UnknownOffset)
            );
        }
        assert_eq!(device.read(dword(0xFFC)), Ok(0));
        assert_eq!(
            device.write(dword(0x1000), 1),
            Err(DeviceError::UnknownOffset)
        );
        let pio = access(IoSpace::Pio, reg::WIDTH, AccessWidth::Dword);
        assert_eq!(device.read(pio), Err(DeviceError::UnsupportedSpace));
        assert_eq!(device.write(pio, 1), Err(DeviceError::UnsupportedSpace));
        // The device is still in its reset state.
        assert_eq!(device.read(dword(reg::WIDTH)), Ok(0));
        assert_eq!(device.read(dword(reg::STATUS)), Ok(0));
        assert_eq!(handle.mode_change_count(), 0);
        assert_eq!(handle.frame_generation(), 0);

        // The same rejections reach the guest as backend errors through the bus.
        let (mut bus, _handle) = setup();
        let width_before = read_reg(&mut bus, reg::WIDTH);
        assert!(matches!(
            bus.write_physical(reg_addr(reg::WIDTH), AccessWidth::Qword, 7, |_| {}),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        assert!(matches!(
            bus.write_physical(reg_addr(reg::MODE_COMMIT), AccessWidth::Byte, 1, |_| {}),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedWidth,
                ..
            })
        ));
        assert_eq!(read_reg(&mut bus, reg::WIDTH), width_before);
        assert_eq!(read_reg(&mut bus, reg::STATUS), 0);
        // An access leaving the window is rejected by the bus before the device.
        assert_eq!(
            bus.read_physical(reg_addr(0xFFE), AccessWidth::Dword),
            Err(BusAccessError::CrossesRegion)
        );
    }

    #[test]
    fn framebuffer_is_rejected_by_code_generation_and_tracked_snapshot_apis() {
        use crate::{Long64ProbeConfig, TrackedAccessError, TrackedCodeSnapshotError};
        use od_isa::memory::AccessContext;

        let (mut bus, _handle) = setup();
        let fb_base = GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE);
        // code_page_generation: Framebuffer is a non-RAM region, RAM still works.
        assert_eq!(
            bus.code_page_generation(fb_base),
            Err(MemoryAccessError::UnreadableRegion(
                MemoryRegionKind::Framebuffer
            ))
        );
        assert_eq!(
            bus.code_page_generation(GuestPhysicalAddress::new(0x1000)),
            Ok(1)
        );

        // Identity-style tables in RAM: linear 0x0 maps the first framebuffer page.
        for (address, value) in [
            (0x1000u64, 0x2007u64),
            (0x2000, 0x3007),
            (0x3000, 0x4007),
            (0x4000, DISPLAY_FRAMEBUFFER_BASE | 7),
        ] {
            bus.write_physical(
                GuestPhysicalAddress::new(address),
                AccessWidth::Qword,
                value,
                |_| {},
            )
            .unwrap();
        }
        let paging =
            Long64ProbeConfig::new(0x8001_0033, 0x1000, 1 << 5, 1 << 8 | 1 << 10 | 1 << 11)
                .unwrap();
        let ram_generation = bus
            .code_page_generation(GuestPhysicalAddress::new(0x4000))
            .unwrap();
        // The instruction fetch from framebuffer pixels is rejected, not snapshotted.
        assert!(matches!(
            bus.tracked_code_snapshot(&paging, 0, 4, AccessContext::default()),
            Err(TrackedCodeSnapshotError::Access(
                TrackedAccessError::OperandPhysical {
                    source: MemoryAccessError::UnreadableRegion(MemoryRegionKind::Framebuffer),
                    ..
                }
            ))
        ));
        // A page-walk A-bit commit in RAM may bump RAM generations, never framebuffer ones.
        assert!(
            bus.code_page_generation(GuestPhysicalAddress::new(0x4000))
                .unwrap()
                >= ram_generation
        );
        assert_eq!(
            bus.code_page_generation(fb_base),
            Err(MemoryAccessError::UnreadableRegion(
                MemoryRegionKind::Framebuffer
            ))
        );
    }

    #[test]
    fn frame_read_returns_pixels_written_through_the_bus_without_callbacks() {
        let (mut bus, handle) = setup();
        set_mode(&mut bus, 3, 2, 1);
        commit(&mut bus);
        let mut frame = vec![0u8; 24];
        assert_eq!(
            handle.read_frame_into(&bus, &mut frame),
            Err(DisplayReadError::NoFrame)
        );

        let callbacks = Cell::new(0u32);
        let fb = |offset: u64| GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE + offset);
        // Dword pixels, a qword store, and a loader-style byte write.
        bus.write_physical(fb(0), AccessWidth::Dword, 0x00FF_0000, |_| {
            callbacks.set(callbacks.get() + 1)
        })
        .unwrap();
        bus.write_physical(fb(4), AccessWidth::Qword, 0x1122_3344_5566_7788, |_| {
            callbacks.set(callbacks.get() + 1)
        })
        .unwrap();
        bus.write_flat_bytes(fb(12), &[1, 2, 3, 4, 5, 6, 7, 8], |_| {
            callbacks.set(callbacks.get() + 1)
        })
        .unwrap();
        bus.write_physical(fb(20), AccessWidth::Dword, 0xAABB_CCDD, |_| {
            callbacks.set(callbacks.get() + 1)
        })
        .unwrap();
        assert_eq!(callbacks.get(), 0);
        // Pixels read back as data through the bus.
        assert_eq!(
            bus.read_physical(fb(0), AccessWidth::Dword),
            Ok(0x00FF_0000)
        );
        // No doorbell yet: the host still has no frame.
        assert_eq!(
            handle.read_frame_into(&bus, &mut frame),
            Err(DisplayReadError::NoFrame)
        );

        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        let info = handle.read_frame_into(&bus, &mut frame).unwrap();
        assert_eq!(
            info,
            FrameInfo {
                mode: DisplayMode {
                    width: 3,
                    height: 2,
                    stride: 12,
                    format: 1
                },
                generation: 1,
                mode_change_count: 1,
            }
        );
        assert_eq!(
            frame,
            [
                0x00, 0x00, 0xFF, 0x00, 0x88, 0x77, 0x66, 0x55, 0x44, 0x33, 0x22, 0x11, 1, 2, 3, 4,
                5, 6, 7, 8, 0xDD, 0xCC, 0xBB, 0xAA
            ]
        );
        // Reading is side-effect free.
        assert_eq!(handle.frame_generation(), 1);
        assert_eq!(handle.read_frame_into(&bus, &mut frame).unwrap(), info);
        assert_eq!(frame_error(&mut bus), ERR_NONE);
    }

    #[test]
    fn frame_read_rejects_missing_mode_and_wrong_buffer_length() {
        let (mut bus, handle) = setup();
        let mut frame = vec![0u8; 16];
        assert_eq!(
            handle.read_frame_into(&bus, &mut frame),
            Err(DisplayReadError::NoActiveMode)
        );
        set_mode(&mut bus, 2, 2, 1);
        commit(&mut bus);
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        for len in [0usize, 15, 17] {
            let mut wrong = vec![0xEE; len];
            assert_eq!(
                handle.read_frame_into(&bus, &mut wrong),
                Err(DisplayReadError::BufferLength {
                    expected: 16,
                    actual: len
                })
            );
            assert!(wrong.iter().all(|byte| *byte == 0xEE));
        }
        assert!(handle.read_frame_into(&bus, &mut frame).is_ok());
    }

    #[test]
    fn mode_change_between_reads_is_visible_and_one_during_the_copy_is_detected() {
        let (mut bus, handle) = setup();
        set_mode(&mut bus, 2, 2, 1);
        commit(&mut bus);
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        let mut small = vec![0u8; 16];
        let first = handle.read_frame_into(&bus, &mut small).unwrap();
        assert_eq!(first.mode_change_count, 1);

        // Between two reads: the new mode has no frame until the doorbell.
        set_mode(&mut bus, 4, 1, 1);
        commit(&mut bus);
        assert_eq!(
            handle.read_frame_into(&bus, &mut small),
            Err(DisplayReadError::NoFrame)
        );
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        let second = handle.read_frame_into(&bus, &mut small).unwrap();
        assert_eq!(second.mode_change_count, 2);
        assert_eq!(second.generation, 2);
        assert_eq!(second.mode.width, 4);

        // During the copy: a standalone device stands in for a guest commit
        // that races the copy.
        let (mut device, handle) = DisplayDevice::new_with_handle();
        let dword = |offset| DeviceAccess {
            space: IoSpace::Mmio,
            offset,
            width: AccessWidth::Dword,
        };
        for (offset, value) in [(reg::WIDTH, 2), (reg::HEIGHT, 2), (reg::FORMAT, 1)] {
            device.write(dword(offset), value).unwrap();
        }
        device.write(dword(reg::MODE_COMMIT), 1).unwrap();
        device.write(dword(reg::FRAME_GENERATION), 0).unwrap();
        let mut buffer = vec![0u8; 16];
        let result = handle.read_frame_with(&mut buffer, |_| {
            device.write(dword(reg::MODE_COMMIT), 1).unwrap();
            Ok(())
        });
        assert_eq!(result, Err(DisplayReadError::ModeChanged));
        // A host reset during the copy is detected as well.
        let (mut device, handle) = DisplayDevice::new_with_handle();
        for (offset, value) in [(reg::WIDTH, 2), (reg::HEIGHT, 2), (reg::FORMAT, 1)] {
            device.write(dword(offset), value).unwrap();
        }
        device.write(dword(reg::MODE_COMMIT), 1).unwrap();
        device.write(dword(reg::FRAME_GENERATION), 0).unwrap();
        let result = handle.read_frame_with(&mut buffer, |_| {
            handle.reset();
            Ok(())
        });
        assert_eq!(result, Err(DisplayReadError::ModeChanged));
    }

    #[test]
    fn host_reset_clears_the_mode_but_keeps_generation_and_pixels() {
        let (mut bus, handle) = setup();
        set_mode(&mut bus, 2, 1, 1);
        commit(&mut bus);
        bus.write_physical(
            GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE),
            AccessWidth::Dword,
            0x1234_5678,
            |_| {},
        )
        .unwrap();
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        write_reg(&mut bus, reg::MODE_COMMIT, 3);
        handle.reset();
        assert_eq!(handle.current_mode(), DisplayMode::NONE);
        assert_eq!(handle.frame_generation(), 1);
        assert_eq!(handle.mode_change_count(), 2);
        assert!(!handle.has_current_frame());
        for offset in [
            reg::WIDTH,
            reg::HEIGHT,
            reg::FORMAT,
            reg::STATUS,
            reg::STRIDE,
        ] {
            assert_eq!(read_reg(&mut bus, offset), 0, "offset {offset:#x}");
        }
        assert_eq!(read_reg(&mut bus, reg::FRAME_GENERATION), 1);
        assert_eq!(
            bus.read_physical(
                GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE),
                AccessWidth::Dword
            ),
            Ok(0x1234_5678)
        );
        let mut frame = vec![0u8; 8];
        assert_eq!(
            handle.read_frame_into(&bus, &mut frame),
            Err(DisplayReadError::NoActiveMode)
        );
        // Reset without a mode leaves the counter alone.
        handle.reset();
        assert_eq!(handle.mode_change_count(), 2);
    }

    // ---- Version 2 (DISPLAY_DEVICE.md, section 14) ----

    fn event_status(bus: &mut MachineBus) -> u32 {
        read_reg(bus, reg::EVENT_STATUS)
    }

    fn pop_error(bus: &mut MachineBus) -> u8 {
        event_status(bus) as u8
    }

    fn response_error(bus: &mut MachineBus) -> u8 {
        (event_status(bus) >> 8) as u8
    }

    fn protocol_error(bus: &mut MachineBus) -> u8 {
        (event_status(bus) >> 16) as u8
    }

    fn enable_events(bus: &mut MachineBus) {
        write_reg(bus, reg::DRIVER_PROTOCOL, 1);
        assert_eq!(protocol_error(bus), ERR_NONE);
    }

    fn pop(bus: &mut MachineBus) {
        write_reg(bus, reg::EVENT_POP, 1);
    }

    fn latch(bus: &mut MachineBus) -> (u64, u32, [u32; 4]) {
        let seq = u64::from(read_reg(bus, reg::EVENT_SEQ_HI)) << 32
            | u64::from(read_reg(bus, reg::EVENT_SEQ_LO));
        (
            seq,
            read_reg(bus, reg::EVENT_TYPE),
            [
                read_reg(bus, reg::EVENT_ARG0),
                read_reg(bus, reg::EVENT_ARG1),
                read_reg(bus, reg::EVENT_ARG2),
                read_reg(bus, reg::EVENT_ARG3),
            ],
        )
    }

    fn respond(bus: &mut MachineBus, seq: u64, value: u32) {
        write_reg(bus, reg::RESPONSE_SEQ_LO, seq as u32);
        write_reg(bus, reg::RESPONSE_SEQ_HI, (seq >> 32) as u32);
        write_reg(bus, reg::REQUEST_RESPONSE, value);
    }

    fn pending(bus: &mut MachineBus) -> u32 {
        read_reg(bus, reg::EVENT_PENDING)
    }

    fn commit_mode(bus: &mut MachineBus, width: u32, height: u32) {
        set_mode(bus, width, height, 1);
        commit(bus);
        assert_eq!(commit_error(bus), ERR_NONE);
    }

    const ACCEPT: u32 = 0x0000_0001;
    const REJECT_UNSUPPORTED: u32 = 0x0000_0102;
    const REJECT_DECLINED: u32 = 0x0000_0202;
    const REJECT_FAILED: u32 = 0x0000_0302;

    #[test]
    fn version_2_register_values_after_reset() {
        let (mut bus, handle) = setup();
        for (offset, expected) in [
            (reg::DRIVER_PROTOCOLS, 2),
            (reg::DRIVER_PROTOCOL, 0),
            (reg::EVENT_STATUS, 0),
            (0x10C, 0),
            (reg::MODE_COUNT, 14),
            (0x114, 0),
            (0x13C, 0),
            (reg::EVENT_PENDING, 0),
            (reg::EVENT_CAPACITY, 16),
            (reg::EVENT_POP, 0),
            (0x14C, 0),
            (reg::EVENT_SEQ_LO, 0),
            (reg::EVENT_SEQ_HI, 0),
            (reg::EVENT_TYPE, 0),
            (0x15C, 0),
            (reg::EVENT_ARG0, 0),
            (reg::EVENT_ARG1, 0),
            (reg::EVENT_ARG2, 0),
            (reg::EVENT_ARG3, 0),
            (0x170, 0),
            (0x17C, 0),
            (reg::RESPONSE_SEQ_LO, 0),
            (reg::RESPONSE_SEQ_HI, 0),
            (reg::REQUEST_RESPONSE, 0),
            (0x18C, 0),
            (0x1FC, 0),
            // The cursor block is covered by the cursor tests; the second
            // screen block stays reserved.
            (0x300, 0),
            (0x3FC, 0),
            (0x800, 0),
            (0xFFC, 0),
        ] {
            assert_eq!(read_reg(&mut bus, offset), expected, "offset {offset:#x}");
        }
        assert_eq!(handle.driver_protocol(), 0);
        assert_eq!(handle.mode_request_state(), None);
        assert_eq!(lock(&handle.state).next_event_seq, 1);
    }

    #[test]
    fn version_1_probe_sequence_is_unchanged_except_version_and_caps() {
        let (mut bus, handle) = setup();
        // The S10-08 probe: size, format, commit, pixels, doorbell, status.
        set_mode(&mut bus, 64, 48, 1);
        commit(&mut bus);
        bus.write_physical(
            GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE),
            AccessWidth::Dword,
            0x00AA_BBCC,
            |_| {},
        )
        .unwrap();
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        assert_eq!(read_reg(&mut bus, reg::STATUS), 0x0000_0003);

        // Every version 1 offset reads as version 1 would, including the
        // reserved ones; only VERSION and CAPS differ.
        let version_1: [(u64, u32); 24] = [
            (reg::ID, 0x4246_444F),
            (reg::VERSION, 2),
            (reg::CAPS, 7),
            (reg::MAX_WIDTH, 4096),
            (reg::MAX_HEIGHT, 2304),
            (reg::FORMATS, 2),
            (reg::FB_BASE_LO, 0xF800_0000),
            (reg::FB_BASE_HI, 0),
            (reg::FB_SIZE, 0x0240_0000),
            (reg::WIDTH, 64),
            (reg::HEIGHT, 48),
            (reg::FORMAT, 1),
            (reg::MODE_COMMIT, 0),
            (reg::ACTIVE_WIDTH, 64),
            (reg::ACTIVE_HEIGHT, 48),
            (reg::ACTIVE_FORMAT, 1),
            (reg::STRIDE, 256),
            (reg::STATUS, 3),
            (reg::FRAME_GENERATION, 1),
            (reg::FRAME_GENERATION_HI, 0),
            (0x0C0, 0),
            (0x0C4, 0),
            (0x0C8, 0),
            (0x024, 0),
        ];
        for (offset, expected) in version_1 {
            assert_eq!(read_reg(&mut bus, offset), expected, "offset {offset:#x}");
        }
        for offset in (0x088..0x0C0).step_by(4).chain((0x0CC..0x100).step_by(4)) {
            assert_eq!(read_reg(&mut bus, offset), 0, "offset {offset:#x}");
        }
        // The guest never touched DRIVER_PROTOCOL: events stay inert.
        assert_eq!(pending(&mut bus), 0);
        assert_eq!(read_reg(&mut bus, reg::EVENT_STATUS), 0);
        assert_eq!(
            handle.post_mode_request(1280, 720, 1),
            Err(DisplayRequestError::NoDriver)
        );
        assert_eq!(pending(&mut bus), 0);
        assert_eq!(handle.mode_request_state(), None);
        assert_eq!(lock(&handle.state).next_event_seq, 1);
        // The commit of a size outside the list keeps working.
        assert_eq!(handle.current_mode().width, 64);
    }

    #[test]
    fn read_only_and_reserved_writes_change_nothing_in_version_2() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 800, 600);
        enable_events(&mut bus);
        handle.post_mode_request(1024, 768, 1).unwrap();
        pop(&mut bus);
        handle.post_mode_request(1280, 720, 1).unwrap();
        write_reg(&mut bus, reg::EVENT_POP, 9);
        respond(&mut bus, 7, ACCEPT);
        // Leave non-zero fields in every status register.
        assert_ne!(event_status(&mut bus), 0);
        write_reg(&mut bus, reg::MODE_COMMIT, 5);
        let writable = [
            reg::WIDTH,
            reg::HEIGHT,
            reg::FORMAT,
            reg::MODE_COMMIT,
            reg::FRAME_GENERATION,
            reg::DRIVER_PROTOCOL,
            reg::EVENT_POP,
            reg::RESPONSE_SEQ_LO,
            reg::RESPONSE_SEQ_HI,
            reg::REQUEST_RESPONSE,
            // Cursor block: the RW and command registers (writes change the
            // cursor by design; the cursor tests cover them).
            reg::CURSOR_WIDTH,
            reg::CURSOR_HEIGHT,
            reg::CURSOR_FORMAT,
            reg::CURSOR_HOTSPOT,
            reg::CURSOR_IMAGE_INDEX,
            reg::CURSOR_IMAGE_DATA,
            reg::CURSOR_IMAGE_COMMIT,
            reg::CURSOR_POSITION,
            reg::CURSOR_ENABLE,
        ];
        let snapshot = |bus: &mut MachineBus| -> Vec<u32> {
            (0..0x1000u64)
                .step_by(4)
                .map(|offset| read_reg(bus, offset))
                .collect()
        };
        let before = snapshot(&mut bus);
        let state_before = lock(&handle.state).clone();
        for offset in (0..0x1000u64).step_by(4) {
            if !writable.contains(&offset) {
                write_reg(&mut bus, offset, 0xFFFF_FFFF);
            }
        }
        assert_eq!(snapshot(&mut bus), before);
        assert_eq!(*lock(&handle.state), state_before);
    }

    #[test]
    fn mode_table_is_read_only_matches_the_contract_and_reads_byte_exact() {
        // (width, height, stride, bytes per frame) of DISPLAY_DEVICE.md, 14.6.
        let spec: [(u32, u32, u32, u32); 14] = [
            (640, 480, 2_560, 1_228_800),
            (800, 600, 3_200, 1_920_000),
            (1024, 768, 4_096, 3_145_728),
            (1280, 720, 5_120, 3_686_400),
            (1280, 800, 5_120, 4_096_000),
            (1280, 1024, 5_120, 5_242_880),
            (1440, 900, 5_760, 5_184_000),
            (1600, 900, 6_400, 5_760_000),
            (1680, 1050, 6_720, 7_056_000),
            (1920, 1080, 7_680, 8_294_400),
            (1920, 1200, 7_680, 9_216_000),
            (2560, 1440, 10_240, 14_745_600),
            (2560, 1600, 10_240, 16_384_000),
            (3840, 2160, 15_360, 33_177_600),
        ];
        let (mut bus, handle) = setup();
        assert_eq!(read_reg(&mut bus, reg::MODE_COUNT), 14);
        let mut expected = vec![0u8; 0x400];
        for (index, &(width, height, _, _)) in spec.iter().enumerate() {
            for (field, value) in [width, height, 1, 0].into_iter().enumerate() {
                let at = index * 16 + field * 4;
                expected[at..at + 4].copy_from_slice(&value.to_le_bytes());
            }
        }
        // The whole table, 256 dwords, reassembled into bytes.
        let mut actual = Vec::new();
        for offset in (reg::MODE_TABLE..reg::MODE_TABLE_END).step_by(4) {
            actual.extend_from_slice(&read_reg(&mut bus, offset).to_le_bytes());
        }
        assert_eq!(actual, expected);

        // Strictly ascending by (width, height); every entry commits.
        for pair in spec.windows(2) {
            assert!((pair[0].0, pair[0].1) < (pair[1].0, pair[1].1));
        }
        for (index, &(width, height, stride, bytes)) in spec.iter().enumerate() {
            let entry = reg::MODE_TABLE + 16 * index as u64;
            assert_eq!(read_reg(&mut bus, entry), width);
            assert_eq!(read_reg(&mut bus, entry + 4), height);
            assert_eq!(read_reg(&mut bus, entry + 8), 1);
            assert_eq!(read_reg(&mut bus, entry + 12), 0);
            assert!(u64::from(bytes) < DISPLAY_FRAMEBUFFER_BYTES);
            let (w, h, f) = (
                read_reg(&mut bus, entry),
                read_reg(&mut bus, entry + 4),
                read_reg(&mut bus, entry + 8),
            );
            set_mode(&mut bus, w, h, f);
            commit(&mut bus);
            assert_eq!(commit_error(&mut bus), ERR_NONE, "entry {index}");
            assert_eq!(read_reg(&mut bus, reg::STRIDE), stride);
            assert_eq!(handle.current_mode().frame_bytes(), u64::from(bytes));
        }
        // Entries past MODE_COUNT read as zero; the list does not restrict commit.
        for index in 14..64u64 {
            for field in 0..4u64 {
                assert_eq!(
                    read_reg(&mut bus, reg::MODE_TABLE + 16 * index + 4 * field),
                    0
                );
            }
        }
        commit_mode(&mut bus, 64, 48);
        // Reading is free of side effects, and reset does not touch the list.
        handle.reset();
        assert_eq!(read_reg(&mut bus, reg::MODE_COUNT), 14);
        let mut again = Vec::new();
        for offset in (reg::MODE_TABLE..reg::MODE_TABLE_END).step_by(4) {
            again.extend_from_slice(&read_reg(&mut bus, offset).to_le_bytes());
        }
        assert_eq!(again, expected);
    }

    #[test]
    fn driver_protocol_follows_the_contract_table() {
        let (mut bus, handle) = setup();
        // Unsupported values while off: register and state unchanged.
        for value in [2u32, 3, 31, 32, 0xFFFF_FFFF] {
            write_reg(&mut bus, reg::DRIVER_PROTOCOL, value);
            assert_eq!(protocol_error(&mut bus), ERR_UNSUPPORTED_PROTOCOL);
            assert_eq!(read_reg(&mut bus, reg::DRIVER_PROTOCOL), 0, "value {value}");
        }
        // Another command does not touch PROTOCOL_ERROR.
        write_reg(&mut bus, reg::EVENT_POP, 1);
        assert_eq!(protocol_error(&mut bus), ERR_UNSUPPORTED_PROTOCOL);
        assert_eq!(pop_error(&mut bus), ERR_NOT_ENABLED);
        // 0 -> 1.
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 1);
        assert_eq!(read_reg(&mut bus, reg::DRIVER_PROTOCOL), 1);
        assert_eq!(protocol_error(&mut bus), ERR_NONE);
        assert_eq!(handle.driver_protocol(), 1);
        handle.post_mode_request(800, 600, 1).unwrap();
        // 1 -> 1 changes nothing: the queued request survives.
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 1);
        assert_eq!(protocol_error(&mut bus), ERR_NONE);
        assert_eq!(pending(&mut bus), 1);
        // Unsupported values while on: state unchanged.
        for value in [2u32, 32, 0xFFFF_FFFF] {
            write_reg(&mut bus, reg::DRIVER_PROTOCOL, value);
            assert_eq!(protocol_error(&mut bus), ERR_UNSUPPORTED_PROTOCOL);
            assert_eq!(read_reg(&mut bus, reg::DRIVER_PROTOCOL), 1);
            assert_eq!(pending(&mut bus), 1);
        }
        // 1 -> 0 clears queue, latch and the open request, but not the rest.
        commit_mode(&mut bus, 640, 480);
        pop(&mut bus);
        handle.post_mode_request(1024, 768, 1).unwrap();
        respond(&mut bus, 99, ACCEPT);
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 0);
        assert_eq!(protocol_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::DRIVER_PROTOCOL), 0);
        assert_eq!(pending(&mut bus), 0);
        assert_eq!(latch(&mut bus), (0, 0, [0; 4]));
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Cancelled
        );
        assert_eq!(handle.current_mode().width, 640);
        assert_eq!(read_reg(&mut bus, reg::RESPONSE_SEQ_LO), 99);
        assert_eq!(lock(&handle.state).next_event_seq, 3);
        // The other error fields keep their values.
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
        // 0 -> 0 is harmless; 0 -> 1 works again.
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 0);
        assert_eq!(protocol_error(&mut bus), ERR_NONE);
        enable_events(&mut bus);
        assert_eq!(handle.post_mode_request(800, 600, 1), Ok(3));
    }

    #[test]
    fn event_queue_delivers_in_order_through_the_latch() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        // Empty pop: QUEUE_EMPTY, latch untouched.
        pop(&mut bus);
        assert_eq!(pop_error(&mut bus), ERR_QUEUE_EMPTY);
        assert_eq!(latch(&mut bus), (0, 0, [0; 4]));

        let seq = handle.post_mode_request(1280, 720, 1).unwrap();
        assert_eq!(seq, 1);
        assert_eq!(pending(&mut bus), 1);
        assert_eq!(latch(&mut bus), (0, 0, [0; 4]), "no pop yet");
        pop(&mut bus);
        assert_eq!(pop_error(&mut bus), ERR_NONE);
        assert_eq!(latch(&mut bus), (1, 1, [1280, 720, 1, 0]));
        assert_eq!(pending(&mut bus), 0);
        // A failing pop keeps the latch and changes only POP_ERROR.
        pop(&mut bus);
        assert_eq!(pop_error(&mut bus), ERR_QUEUE_EMPTY);
        assert_eq!(latch(&mut bus), (1, 1, [1280, 720, 1, 0]));
        // Bad commands.
        for value in [0u32, 2, 0xFFFF_FFFF] {
            write_reg(&mut bus, reg::EVENT_POP, value);
            assert_eq!(pop_error(&mut bus), ERR_INVALID_COMMAND, "value {value}");
            assert_eq!(latch(&mut bus), (1, 1, [1280, 720, 1, 0]));
        }
        // A command error wins over NOT_ENABLED.
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 0);
        write_reg(&mut bus, reg::EVENT_POP, 2);
        assert_eq!(pop_error(&mut bus), ERR_INVALID_COMMAND);
        write_reg(&mut bus, reg::EVENT_POP, 1);
        assert_eq!(pop_error(&mut bus), ERR_NOT_ENABLED);
        // Successful pop clears the error again, and numbering continues.
        enable_events(&mut bus);
        let second = handle.post_mode_request(1920, 1080, 1).unwrap();
        assert_eq!(second, 2);
        pop(&mut bus);
        assert_eq!(pop_error(&mut bus), ERR_NONE);
        assert_eq!(latch(&mut bus), (2, 1, [1920, 1080, 1, 0]));
    }

    #[test]
    fn latch_is_stable_while_the_host_posts_between_guest_reads() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        handle.post_mode_request(1280, 720, 1).unwrap();
        pop(&mut bus);
        // Guest reads the first half of the latch ...
        let lo = read_reg(&mut bus, reg::EVENT_SEQ_LO);
        let arg0 = read_reg(&mut bus, reg::EVENT_ARG0);
        // ... a quantum ends and the host posts more events ...
        handle.post_mode_request(1920, 1080, 1).unwrap();
        handle.post_mode_request(2560, 1440, 1).unwrap();
        assert_eq!(pending(&mut bus), 1);
        // ... the rest of the latch still belongs to the first event.
        assert_eq!(read_reg(&mut bus, reg::EVENT_SEQ_HI), 0);
        assert_eq!(read_reg(&mut bus, reg::EVENT_TYPE), 1);
        assert_eq!(read_reg(&mut bus, reg::EVENT_ARG1), 720);
        assert_eq!((lo, arg0), (1, 1280));
        // Only the guest's own pop changes the latch.
        pop(&mut bus);
        assert_eq!(latch(&mut bus), (3, 1, [2560, 1440, 1, 0]));
    }

    #[test]
    fn latch_carries_a_full_64_bit_sequence_number() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        lock(&handle.state).next_event_seq = 0x1_0000_0002;
        let seq = handle.post_mode_request(800, 600, 1).unwrap();
        assert_eq!(seq, 0x1_0000_0002);
        pop(&mut bus);
        assert_eq!(read_reg(&mut bus, reg::EVENT_SEQ_LO), 2);
        assert_eq!(read_reg(&mut bus, reg::EVENT_SEQ_HI), 1);
        // The answer needs both halves.
        respond(&mut bus, 2, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
        commit_mode(&mut bus, 800, 600);
        respond(&mut bus, seq, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_NONE);
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Accepted
        );
    }

    #[test]
    fn untaken_requests_merge_and_a_taken_one_is_superseded() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        let first = handle.post_mode_request(1280, 720, 1).unwrap();
        let second = handle.post_mode_request(1920, 1080, 1).unwrap();
        assert!(second > first);
        assert_eq!(pending(&mut bus), 1);
        pop(&mut bus);
        assert_eq!(latch(&mut bus), (second, 1, [1920, 1080, 1, 0]));
        assert_eq!(
            handle.mode_request_state(),
            Some(ModeRequestState {
                seq: second,
                width: 1920,
                height: 1080,
                format: 1,
                outcome: ModeRequestOutcome::Pending,
            })
        );
        // The answer to the merged-away request is stale.
        respond(&mut bus, first, REJECT_DECLINED);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);

        // A request already in the latch is superseded by a new one.
        let third = handle.post_mode_request(2560, 1440, 1).unwrap();
        assert_eq!(pending(&mut bus), 1);
        commit_mode(&mut bus, 1920, 1080);
        respond(&mut bus, second, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
        // The latch still shows the old event; the new one waits in the queue.
        assert_eq!(latch(&mut bus), (second, 1, [1920, 1080, 1, 0]));
        pop(&mut bus);
        assert_eq!(latch(&mut bus), (third, 1, [2560, 1440, 1, 0]));
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Pending
        );
    }

    #[test]
    fn response_accept_requires_the_requested_mode_to_be_active() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        let seq = handle.post_mode_request(1280, 720, 1).unwrap();
        pop(&mut bus);
        // No mode at all, then a different mode: MODE_MISMATCH, still open.
        respond(&mut bus, seq, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_MODE_MISMATCH);
        commit_mode(&mut bus, 1280, 800);
        respond(&mut bus, seq, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_MODE_MISMATCH);
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Pending
        );
        // ACCEPT never applies the mode.
        assert_eq!(handle.current_mode().height, 800);
        commit_mode(&mut bus, 1280, 720);
        respond(&mut bus, seq, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_NONE);
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Accepted
        );
        // A second answer to a closed request is stale.
        respond(&mut bus, seq, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
        respond(&mut bus, seq, REJECT_FAILED);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Accepted
        );

        // The requested mode is already active: ACCEPT without a commit.
        let again = handle.post_mode_request(1280, 720, 1).unwrap();
        respond(&mut bus, again, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_NONE);
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Accepted
        );
    }

    #[test]
    fn response_reject_accepts_every_reason_without_a_mode_check() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        for (value, reason) in [
            (REJECT_UNSUPPORTED, DisplayRejectReason::Unsupported),
            (REJECT_DECLINED, DisplayRejectReason::Declined),
            (REJECT_FAILED, DisplayRejectReason::Failed),
        ] {
            let seq = handle.post_mode_request(1024, 768, 1).unwrap();
            respond(&mut bus, seq, value);
            assert_eq!(response_error(&mut bus), ERR_NONE);
            assert_eq!(
                handle.mode_request_state().unwrap().outcome,
                ModeRequestOutcome::Rejected(reason)
            );
        }
        // The mode never changed.
        assert_eq!(handle.current_mode(), DisplayMode::NONE);
    }

    #[test]
    fn invalid_response_encodings_are_rejected_before_any_other_check() {
        let (mut bus, handle) = setup();
        let bad = [
            0x0000_0000u32,
            0x0000_0003,
            0x0000_00FF,
            // ACCEPT carries reason 0; REJECT needs reason 1..=3.
            0x0000_0101,
            0x0000_0002,
            0x0000_0402,
            0x0000_FF02,
            // Bits 31:16 must be zero.
            0x0001_0001,
            0x8000_0001,
            0x0001_0102,
            0xFFFF_FFFF,
        ];
        // Protocol off: the encoding check still comes first.
        for value in bad {
            respond(&mut bus, 1, value);
            assert_eq!(response_error(&mut bus), ERR_INVALID_COMMAND, "{value:#x}");
        }
        respond(&mut bus, 1, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_NOT_ENABLED);
        respond(&mut bus, 1, REJECT_FAILED);
        assert_eq!(response_error(&mut bus), ERR_NOT_ENABLED);
        // With a protocol and a request open, bad encodings still fail and
        // leave the request open.
        enable_events(&mut bus);
        let seq = handle.post_mode_request(800, 600, 1).unwrap();
        for value in bad {
            respond(&mut bus, seq, value);
            assert_eq!(response_error(&mut bus), ERR_INVALID_COMMAND, "{value:#x}");
            assert_eq!(
                handle.mode_request_state().unwrap().outcome,
                ModeRequestOutcome::Pending
            );
        }
        // Without an open request: stale.
        let (mut bus, _handle) = setup();
        enable_events(&mut bus);
        respond(&mut bus, 1, ACCEPT);
        assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST);
    }

    #[test]
    fn response_with_a_foreign_sequence_is_stale_even_if_only_the_high_half_differs() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        let seq = handle.post_mode_request(800, 600, 1).unwrap();
        for wrong in [
            seq + 1,
            seq - 1,
            0,
            seq | 1 << 32,
            seq | 0x8000_0000_0000_0000,
        ] {
            respond(&mut bus, wrong, REJECT_DECLINED);
            assert_eq!(response_error(&mut bus), ERR_STALE_REQUEST, "{wrong:#x}");
        }
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Pending
        );
        // RESPONSE_SEQ_* are plain RW registers; the answer follows them.
        respond(&mut bus, seq, REJECT_DECLINED);
        assert_eq!(response_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::RESPONSE_SEQ_LO), seq as u32);
        assert_eq!(read_reg(&mut bus, reg::RESPONSE_SEQ_HI), 0);
    }

    #[test]
    fn host_refusals_follow_the_check_order_and_change_nothing() {
        let (mut bus, handle) = setup();
        // 1. NoDriver wins over a bad mode.
        assert_eq!(
            handle.post_mode_request(0, 0, 0),
            Err(DisplayRequestError::NoDriver)
        );
        enable_events(&mut bus);
        // 2. InvalidMode: each way a MODE_COMMIT 2-4 check fails.
        for (w, h, f) in [
            (0, 600, 1),
            (800, 0, 1),
            (4097, 600, 1),
            (800, 2305, 1),
            (u32::MAX, u32::MAX, 1),
            (800, 600, 0),
            (800, 600, 2),
            (800, 600, 32),
        ] {
            assert_eq!(
                handle.post_mode_request(w, h, f),
                Err(DisplayRequestError::InvalidMode),
                "{w}x{h} format {f}"
            );
        }
        // Valid requests that are not in the list are fine.
        let first = handle.post_mode_request(64, 48, 1).unwrap();
        let snapshot = lock(&handle.state).clone();

        // 3. SequenceExhausted with an otherwise valid request and a queue
        // that holds a mergeable request (no QueueFull involved here; the
        // priority over QueueFull is checked below with a full queue).
        lock(&handle.state).next_event_seq = u64::MAX;
        let exhausted = lock(&handle.state).clone();
        assert_eq!(
            handle.post_mode_request(800, 600, 1),
            Err(DisplayRequestError::SequenceExhausted)
        );
        assert_eq!(*lock(&handle.state), exhausted);
        lock(&handle.state).next_event_seq = snapshot.next_event_seq;

        // 4. QueueFull: sixteen events that cannot be merged.
        lock(&handle.state).queue.clear();
        for n in 0..DISPLAY_EVENT_CAPACITY as u64 {
            lock(&handle.state).queue.push_back(QueuedEvent {
                seq: 100 + n,
                kind: 2,
                arg: [0; 4],
            });
        }
        assert_eq!(pending(&mut bus), 16);
        let full = lock(&handle.state).clone();
        assert_eq!(
            handle.post_mode_request(800, 600, 1),
            Err(DisplayRequestError::QueueFull)
        );
        assert_eq!(*lock(&handle.state), full);
        // Order, SequenceExhausted before QueueFull: the queue is full of
        // unmergeable events (QueueFull alone is shown above) and the
        // sequence counter is exhausted. If the QueueFull check ran first,
        // this would return QueueFull.
        lock(&handle.state).next_event_seq = u64::MAX;
        let full_exhausted = lock(&handle.state).clone();
        assert_eq!(
            handle.post_mode_request(800, 600, 1),
            Err(DisplayRequestError::SequenceExhausted)
        );
        assert_eq!(*lock(&handle.state), full_exhausted);
        // Order, InvalidMode before SequenceExhausted: same exhausted state
        // and a full queue, but the mode is invalid. If the sequence check
        // ran first, this would return SequenceExhausted.
        for (w, h, f) in [(0, 600, 1), (800, 600, 2), (4097, 600, 1)] {
            assert_eq!(
                handle.post_mode_request(w, h, f),
                Err(DisplayRequestError::InvalidMode),
                "{w}x{h} format {f}"
            );
        }
        assert_eq!(*lock(&handle.state), full_exhausted);
        lock(&handle.state).next_event_seq = full.next_event_seq;
        assert_eq!(*lock(&handle.state), full);
        // The older events survive the refusal in order.
        pop(&mut bus);
        assert_eq!(latch(&mut bus).0, 100);
        assert_eq!(handle.mode_request_state().unwrap().seq, first);

        // A full queue holding one MODE_REQUEST still merges.
        let mut state = lock(&handle.state);
        state.queue.clear();
        for n in 0..15u64 {
            state.queue.push_back(QueuedEvent {
                seq: 200 + n,
                kind: 2,
                arg: [0; 4],
            });
        }
        state.queue.push_back(QueuedEvent {
            seq: 300,
            kind: EVENT_MODE_REQUEST,
            arg: [1, 1, 1, 0],
        });
        drop(state);
        let seq = handle.post_mode_request(800, 600, 1).unwrap();
        let state = lock(&handle.state);
        assert_eq!(state.queue.len(), 16);
        assert!(state.queue.iter().all(|event| event.seq != 300));
        assert_eq!(state.queue.back().unwrap().seq, seq);
        // A MODE_REQUEST for another screen is not merged away.
        drop(state);
        let mut state = lock(&handle.state);
        state.queue.clear();
        state.queue.push_back(QueuedEvent {
            seq: 400,
            kind: EVENT_MODE_REQUEST,
            arg: [1, 1, 1, 1],
        });
        drop(state);
        handle.post_mode_request(800, 600, 1).unwrap();
        assert_eq!(pending(&mut bus), 2);
    }

    #[test]
    fn reset_cancels_the_request_and_keeps_numbering_monotonic() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 800, 600);
        enable_events(&mut bus);
        let before = handle.post_mode_request(1024, 768, 1).unwrap();
        pop(&mut bus);
        handle.post_mode_request(1280, 720, 1).unwrap();
        respond(&mut bus, 5, REJECT_FAILED);
        write_reg(&mut bus, reg::EVENT_POP, 7);
        handle.reset();
        assert_eq!(handle.driver_protocol(), 0);
        assert_eq!(read_reg(&mut bus, reg::DRIVER_PROTOCOL), 0);
        assert_eq!(pending(&mut bus), 0);
        assert_eq!(latch(&mut bus), (0, 0, [0; 4]));
        assert_eq!(event_status(&mut bus), 0);
        assert_eq!(read_reg(&mut bus, reg::RESPONSE_SEQ_LO), 0);
        assert_eq!(read_reg(&mut bus, reg::RESPONSE_SEQ_HI), 0);
        let cancelled = handle.mode_request_state().unwrap();
        assert_eq!(cancelled.outcome, ModeRequestOutcome::Cancelled);
        assert_eq!(cancelled.seq, before + 1);
        assert_eq!(read_reg(&mut bus, reg::MODE_COUNT), 14);
        // Events are inert again until the guest re-enables them.
        assert_eq!(
            handle.post_mode_request(800, 600, 1),
            Err(DisplayRequestError::NoDriver)
        );
        pop(&mut bus);
        assert_eq!(pop_error(&mut bus), ERR_NOT_ENABLED);
        enable_events(&mut bus);
        let after = handle.post_mode_request(800, 600, 1).unwrap();
        assert!(after > cancelled.seq);
        // A request closed by the guest keeps its outcome across reset.
        respond(&mut bus, after, REJECT_DECLINED);
        handle.reset();
        assert_eq!(
            handle.mode_request_state().unwrap().outcome,
            ModeRequestOutcome::Rejected(DisplayRejectReason::Declined)
        );
    }

    #[test]
    fn version_2_registers_fail_with_backend_errors_before_any_state_change() {
        let (mut bus, handle) = setup();
        enable_events(&mut bus);
        handle.post_mode_request(800, 600, 1).unwrap();
        let state_before = lock(&handle.state).clone();
        let offsets = [
            reg::DRIVER_PROTOCOLS,
            reg::DRIVER_PROTOCOL,
            reg::EVENT_STATUS,
            reg::MODE_COUNT,
            reg::EVENT_PENDING,
            reg::EVENT_POP,
            reg::EVENT_SEQ_LO,
            reg::EVENT_ARG3,
            reg::RESPONSE_SEQ_LO,
            reg::REQUEST_RESPONSE,
            reg::MODE_TABLE,
            reg::MODE_TABLE_END - 4,
        ];
        let (mut device, _h) = DisplayDevice::new_with_handle();
        for offset in offsets {
            for width in [AccessWidth::Byte, AccessWidth::Word, AccessWidth::Qword] {
                let access = DeviceAccess {
                    space: IoSpace::Mmio,
                    offset,
                    width,
                };
                assert_eq!(device.read(access), Err(DeviceError::UnsupportedWidth));
                assert_eq!(device.write(access, 1), Err(DeviceError::UnsupportedWidth));
                let on_bus_read = bus.read_physical(reg_addr(offset), width);
                assert!(matches!(
                    on_bus_read,
                    Err(BusAccessError::Device {
                        source: DeviceError::UnsupportedWidth,
                        ..
                    })
                ));
                assert!(matches!(
                    bus.write_physical(reg_addr(offset), width, 1, |_| {}),
                    Err(BusAccessError::Device {
                        source: DeviceError::UnsupportedWidth,
                        ..
                    })
                ));
            }
            let misaligned = DeviceAccess {
                space: IoSpace::Mmio,
                offset: offset + 1,
                width: AccessWidth::Dword,
            };
            assert_eq!(device.read(misaligned), Err(DeviceError::Misaligned));
            assert_eq!(device.write(misaligned, 1), Err(DeviceError::Misaligned));
            let pio = DeviceAccess {
                space: IoSpace::Pio,
                offset,
                width: AccessWidth::Dword,
            };
            assert_eq!(device.read(pio), Err(DeviceError::UnsupportedSpace));
            assert_eq!(device.write(pio, 1), Err(DeviceError::UnsupportedSpace));
            assert!(matches!(
                bus.write_physical(reg_addr(offset + 2), AccessWidth::Dword, 1, |_| {}),
                Err(BusAccessError::Device {
                    source: DeviceError::Misaligned,
                    ..
                })
            ));
        }
        assert_eq!(*lock(&handle.state), state_before);
    }

    #[test]
    fn guest_errors_stay_in_status_registers_and_host_errors_stay_with_the_host() {
        // Guest-correctable errors: every access succeeds, the code is in the
        // matching field and never in STATUS.
        let (mut bus, handle) = setup();
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 9);
        write_reg(&mut bus, reg::EVENT_POP, 1);
        respond(&mut bus, 1, ACCEPT);
        assert_eq!(
            event_status(&mut bus),
            u32::from(ERR_UNSUPPORTED_PROTOCOL) << 16
                | u32::from(ERR_NOT_ENABLED) << 8
                | u32::from(ERR_NOT_ENABLED)
        );
        assert_eq!(read_reg(&mut bus, reg::STATUS), 0);
        // Each field is overwritten by its own command only.
        enable_events(&mut bus);
        assert_eq!(protocol_error(&mut bus), ERR_NONE);
        assert_eq!(pop_error(&mut bus), ERR_NOT_ENABLED);
        assert_eq!(response_error(&mut bus), ERR_NOT_ENABLED);
        write_reg(&mut bus, reg::MODE_COMMIT, 1);
        assert_eq!(commit_error(&mut bus), ERR_SIZE_NOT_SET);
        assert_eq!(event_status(&mut bus) >> 24, 0);
        // Host errors are results of host calls and leave guest state alone.
        let status = (event_status(&mut bus), read_reg(&mut bus, reg::STATUS));
        assert_eq!(
            handle.post_mode_request(0, 0, 1),
            Err(DisplayRequestError::InvalidMode)
        );
        assert_eq!(
            (event_status(&mut bus), read_reg(&mut bus, reg::STATUS)),
            status
        );
    }

    #[test]
    fn instruction_fetch_from_framebuffer_is_rejected_but_data_reads_work() {
        use crate::IdentityMemoryAdapter;
        use od_isa::memory::{
            AccessContext, AccessOrigin, GuestMemory, InstructionMode, MemoryError, PrivilegeLevel,
        };
        let (mut bus, _handle) = setup();
        let mut commits = 0u32;
        let mut callback = |_: GuestPhysicalRange| commits += 1;
        let context = AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        };
        let mut memory = IdentityMemoryAdapter::new(&mut bus, &mut callback);
        memory
            .write(DISPLAY_FRAMEBUFFER_BASE, context, &[0x90, 0xC3])
            .unwrap();
        let mut data = [0u8; 2];
        memory
            .read(DISPLAY_FRAMEBUFFER_BASE, context, &mut data)
            .unwrap();
        assert_eq!(data, [0x90, 0xC3]);
        assert!(matches!(
            memory.fetch(DISPLAY_FRAMEBUFFER_BASE, context),
            Err(MemoryError::Backend(_))
        ));
        // Unmapped reserve between the framebuffer and the registers.
        assert!(matches!(
            memory.read(
                DISPLAY_FRAMEBUFFER_BASE + DISPLAY_FRAMEBUFFER_BYTES,
                context,
                &mut data
            ),
            Err(MemoryError::Backend(_))
        ));
        assert_eq!(commits, 0);
    }

    #[test]
    fn register_window_and_framebuffer_do_not_overlap_each_other_or_ram_or_mmio() {
        let mut regions = display_regions();
        regions.push(MemoryRegionSpec::new(
            range(0x1000_0000, 0x1000_0000),
            MemoryRegionKind::Ram,
        ));
        regions.push(MemoryRegionSpec::new(
            range(0xFEC0_0000, 0x1000),
            MemoryRegionKind::Mmio,
        ));
        assert!(MachineMemoryMap::new(36, regions).is_ok());
        assert_eq!(
            DISPLAY_FRAMEBUFFER_BASE + DISPLAY_FRAMEBUFFER_BYTES,
            0xFA40_0000
        );
        assert!(DISPLAY_FRAMEBUFFER_BASE + DISPLAY_FRAMEBUFFER_BYTES <= DISPLAY_REGISTERS_BASE);
        assert!(DISPLAY_REGISTERS_BASE + DISPLAY_REGISTERS_BYTES <= 1 << 32);
        let colliding = vec![
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
                MemoryRegionKind::Framebuffer,
            ),
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE + 0x1000, 0x1000),
                MemoryRegionKind::Mmio,
            ),
        ];
        assert!(matches!(
            MachineMemoryMap::new(36, colliding),
            Err(MemoryAccessError::OverlappingRegions { .. })
        ));
    }

    #[test]
    fn framebuffer_region_validation_in_map_and_memory() {
        // Backing is outside RamLimits: RAM budget covers RAM only.
        let (device, _handle) = DisplayDevice::new_with_handle();
        assert!(try_bus(display_regions(), device).is_ok());

        let fb =
            |start, len| MemoryRegionSpec::new(range(start, len), MemoryRegionKind::Framebuffer);
        assert_eq!(
            MachineMemoryMap::new(36, vec![fb(0x1000_0000, 0x1000), fb(0x2000_0000, 0x1000)]).err(),
            Some(MemoryAccessError::MultipleFramebufferRegions)
        );
        for (start, len) in [
            (0x1000_0800, 0x1000),
            (0x1000_0000, 0x1800),
            (0x1000_0000, 0x0240_1000),
        ] {
            assert_eq!(
                MachineMemoryMap::new(36, vec![fb(start, len)]).err(),
                Some(MemoryAccessError::InvalidFramebufferRegion(range(
                    start, len
                )))
            );
        }
    }

    #[test]
    fn build_rejects_display_device_and_region_disagreement() {
        let fb_spec =
            |start, len| MemoryRegionSpec::new(range(start, len), MemoryRegionKind::Framebuffer);
        let mmio_spec =
            |start, len| MemoryRegionSpec::new(range(start, len), MemoryRegionKind::Mmio);
        let ram_spec = MemoryRegionSpec::new(range(0, RAM_BYTES), MemoryRegionKind::Ram);

        // Region without a display device.
        let regions = vec![
            ram_spec,
            fb_spec(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
        ];
        let bus = MachineBus::new(
            MachineMemoryMap::new(36, regions).unwrap(),
            RamLimits::new(RAM_BYTES),
            vec![],
            vec![],
            vec![],
        );
        assert_eq!(
            bus.err(),
            Some(BusBuildError::Display(DisplayBindingError::MissingDevice))
        );

        // Device without a region.
        let (device, _h) = DisplayDevice::new_with_handle();
        let regions = vec![
            ram_spec,
            mmio_spec(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
        ];
        assert_eq!(
            try_bus(regions, device).err(),
            Some(BusBuildError::Display(
                DisplayBindingError::MissingFramebufferRegion
            ))
        );

        // Wrong start, wrong length.
        for (start, len) in [
            (
                DISPLAY_FRAMEBUFFER_BASE + 0x1000,
                DISPLAY_FRAMEBUFFER_BYTES - 0x1000,
            ),
            (DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES - 0x1000),
            (0x2000_0000, DISPLAY_FRAMEBUFFER_BYTES),
        ] {
            let (device, _h) = DisplayDevice::new_with_handle();
            let regions = vec![
                ram_spec,
                fb_spec(start, len),
                mmio_spec(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES),
            ];
            assert!(matches!(
                try_bus(regions, device).err(),
                Some(BusBuildError::Display(
                    DisplayBindingError::FramebufferMismatch { .. }
                ))
            ));
        }

        // Register window of the wrong length.
        let (device, _h) = DisplayDevice::new_with_handle();
        let regions = vec![
            ram_spec,
            fb_spec(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
            mmio_spec(DISPLAY_REGISTERS_BASE, 0x2000),
        ];
        assert_eq!(
            try_bus(regions, device).err(),
            Some(BusBuildError::Display(
                DisplayBindingError::RegisterWindowLength { actual: 0x2000 }
            ))
        );

        // No MMIO binding for the device.
        let (device, _h) = DisplayDevice::new_with_handle();
        let regions = vec![
            ram_spec,
            fb_spec(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES),
        ];
        assert_eq!(
            MachineBus::new(
                MachineMemoryMap::new(36, regions).unwrap(),
                RamLimits::new(RAM_BYTES),
                vec![Box::new(device)],
                vec![],
                vec![]
            )
            .err(),
            Some(BusBuildError::Display(
                DisplayBindingError::RegisterWindowUnbound
            ))
        );

        // A second binding of the display device, to a region of another length.
        let (device, _h) = DisplayDevice::new_with_handle();
        let mut regions = display_regions();
        regions.push(mmio_spec(0x2000_0000, 0x100));
        assert_eq!(
            MachineBus::new(
                MachineMemoryMap::new(36, regions).unwrap(),
                RamLimits::new(RAM_BYTES),
                vec![Box::new(device)],
                vec![
                    MmioBinding {
                        region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
                        device_index: 0,
                    },
                    MmioBinding {
                        region_start: GuestPhysicalAddress::new(0x2000_0000),
                        device_index: 0,
                    },
                ],
                vec![]
            )
            .err(),
            Some(BusBuildError::Display(
                DisplayBindingError::MultipleRegisterWindows
            ))
        );

        // Two displays.
        let (first, _h1) = DisplayDevice::new_with_handle();
        let (second, _h2) = DisplayDevice::new_with_handle();
        let regions = display_regions();
        assert_eq!(
            MachineBus::new(
                MachineMemoryMap::new(36, regions).unwrap(),
                RamLimits::new(RAM_BYTES),
                vec![Box::new(first), Box::new(second)],
                vec![MmioBinding {
                    region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
                    device_index: 0,
                }],
                vec![]
            )
            .err(),
            Some(BusBuildError::Display(DisplayBindingError::MultipleDevices))
        );
    }

    // ---- Cursor block (DISPLAY_DEVICE.md, section 15) ----

    fn cursor_status(bus: &mut MachineBus) -> u32 {
        read_reg(bus, reg::CURSOR_STATUS)
    }

    fn data_error(bus: &mut MachineBus) -> u8 {
        cursor_status(bus) as u8
    }

    fn image_error(bus: &mut MachineBus) -> u8 {
        (cursor_status(bus) >> 8) as u8
    }

    fn position_error(bus: &mut MachineBus) -> u8 {
        (cursor_status(bus) >> 16) as u8
    }

    fn enable_error(bus: &mut MachineBus) -> u8 {
        (cursor_status(bus) >> 24) as u8
    }

    fn set_shape(bus: &mut MachineBus, width: u32, height: u32, format: u32, hot: (u32, u32)) {
        write_reg(bus, reg::CURSOR_WIDTH, width);
        write_reg(bus, reg::CURSOR_HEIGHT, height);
        write_reg(bus, reg::CURSOR_FORMAT, format);
        write_reg(bus, reg::CURSOR_HOTSPOT, hot.1 << 16 | hot.0);
    }

    fn image_commit(bus: &mut MachineBus) {
        write_reg(bus, reg::CURSOR_IMAGE_COMMIT, 1);
    }

    fn load_words(bus: &mut MachineBus, words: &[u32]) {
        write_reg(bus, reg::CURSOR_IMAGE_INDEX, 0);
        for word in words {
            write_reg(bus, reg::CURSOR_IMAGE_DATA, *word);
        }
    }

    /// Publishes a `w x h` shape whose word `n` is `seed + n`.
    fn publish(bus: &mut MachineBus, width: u32, height: u32, seed: u32) {
        set_shape(bus, width, height, 2, (0, 0));
        let words: Vec<u32> = (0..width * height).map(|n| seed.wrapping_add(n)).collect();
        load_words(bus, &words);
        image_commit(bus);
        assert_eq!(image_error(bus), ERR_NONE);
    }

    fn position(x: u32, y: u32) -> u32 {
        y << 16 | x
    }

    fn cursor_image(handle: &DisplayHandle) -> (CursorShapeInfo, Vec<u8>) {
        let state = handle.cursor_state();
        let shape = state.shape.expect("shape published");
        let mut bytes = vec![0u8; (shape.width * shape.height * 4) as usize];
        let info = handle.read_cursor_image_into(&mut bytes).unwrap();
        assert_eq!(info, shape);
        (info, bytes)
    }

    #[test]
    fn cursor_register_values_after_reset() {
        let (mut bus, handle) = setup();
        for (offset, expected) in [
            (reg::CURSOR_MAX_WIDTH, 64),
            (reg::CURSOR_MAX_HEIGHT, 64),
            (reg::CURSOR_FORMATS, 4),
            (reg::CURSOR_STATUS, 0),
            (reg::CURSOR_WIDTH, 0),
            (reg::CURSOR_HEIGHT, 0),
            (reg::CURSOR_FORMAT, 0),
            (reg::CURSOR_HOTSPOT, 0),
            (reg::CURSOR_IMAGE_INDEX, 0),
            (reg::CURSOR_IMAGE_DATA, 0),
            (reg::CURSOR_IMAGE_COMMIT, 0),
            (0x22C, 0),
            (reg::CURSOR_POSITION, 0),
            (reg::CURSOR_ENABLE, 0),
            (0x238, 0),
            (0x2FC, 0),
            (0x300, 0),
        ] {
            assert_eq!(read_reg(&mut bus, offset), expected, "offset {offset:#x}");
        }
        assert_eq!(read_reg(&mut bus, reg::VERSION), 2);
        assert_eq!(read_reg(&mut bus, reg::CAPS), 0x7);
        assert_eq!(
            handle.cursor_state(),
            CursorState {
                cursor_seq: 0,
                enabled: false,
                x: 0,
                y: 0,
                shape: None
            }
        );
        let mut out = [0u8; 4];
        assert_eq!(
            handle.read_cursor_image_into(&mut out),
            Err(CursorReadError::NoShape)
        );
        let state = lock(&handle.state);
        assert!(state.staging.iter().all(|w| *w == 0));
        assert!(state.active.iter().all(|w| *w == 0));
        assert_eq!((state.cursor_seq, state.shape_seq), (0, 0));
    }

    #[test]
    fn cursor_read_only_and_reserved_writes_change_nothing() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 64, 48);
        write_reg(&mut bus, reg::CURSOR_IMAGE_COMMIT, 5);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(64, 0));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        write_reg(&mut bus, reg::EVENT_POP, 5);
        let cursor_status_before = cursor_status(&mut bus);
        assert_ne!(data_error(&mut bus) | image_error(&mut bus), 0);
        assert_ne!(position_error(&mut bus), 0);
        assert_ne!(enable_error(&mut bus), 0);
        let status = read_reg(&mut bus, reg::STATUS);
        let event_status = read_reg(&mut bus, reg::EVENT_STATUS);
        let state_before = lock(&handle.state).clone();
        for offset in [
            reg::CURSOR_MAX_WIDTH,
            reg::CURSOR_MAX_HEIGHT,
            reg::CURSOR_FORMATS,
            reg::CURSOR_STATUS,
            0x22C,
            0x238,
            0x23C,
            0x240,
            0x2F8,
            0x2FC,
        ] {
            let before = read_reg(&mut bus, offset);
            write_reg(&mut bus, offset, 0xFFFF_FFFF);
            assert_eq!(read_reg(&mut bus, offset), before, "offset {offset:#x}");
            assert_eq!(cursor_status(&mut bus), cursor_status_before);
            assert_eq!(read_reg(&mut bus, reg::STATUS), status);
            assert_eq!(read_reg(&mut bus, reg::EVENT_STATUS), event_status);
        }
        assert_eq!(*lock(&handle.state), state_before);
    }

    #[test]
    fn cursor_rw_registers_store_all_32_bits_and_command_registers_read_zero() {
        let (mut bus, handle) = setup();
        for offset in [
            reg::CURSOR_WIDTH,
            reg::CURSOR_HEIGHT,
            reg::CURSOR_FORMAT,
            reg::CURSOR_HOTSPOT,
            reg::CURSOR_IMAGE_INDEX,
        ] {
            for value in [0xFFFF_FFFFu32, 0x8000_0001, 0x1234_5678, 0] {
                write_reg(&mut bus, offset, value);
                assert_eq!(read_reg(&mut bus, offset), value, "offset {offset:#x}");
            }
        }
        // Writes to the staged-shape registers and the index alone never touch
        // the cursor counters or the status.
        assert_eq!(handle.cursor_state().cursor_seq, 0);
        assert_eq!(cursor_status(&mut bus), 0);
        // WO registers read zero after a write.
        write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 0xDEAD_BEEF);
        write_reg(&mut bus, reg::CURSOR_IMAGE_COMMIT, 1);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_DATA), 0);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_COMMIT), 0);
    }

    #[test]
    fn image_index_auto_increments_and_is_range_checked() {
        let (mut bus, handle) = setup();
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 0);
        for n in 0..3u32 {
            write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 0x100 + n);
            assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), n + 1);
            assert_eq!(data_error(&mut bus), ERR_NONE);
        }
        assert_eq!(&lock(&handle.state).staging[..4], &[0x100, 0x101, 0x102, 0]);
        // Writes anywhere in staging never change cursor_seq or active.
        assert_eq!(handle.cursor_state().cursor_seq, 0);
        assert!(lock(&handle.state).active.iter().all(|w| *w == 0));

        // The last word succeeds and leaves the index at 4096.
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 4095);
        write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 0xAAAA_0001);
        assert_eq!(data_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), 4096);
        assert_eq!(lock(&handle.state).staging[4095], 0xAAAA_0001);
        // The next write fails without touching staging or the index.
        let staging_before = lock(&handle.state).staging.clone();
        write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 0xBBBB_0002);
        assert_eq!(data_error(&mut bus), ERR_INDEX_OUT_OF_RANGE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), 4096);
        assert_eq!(lock(&handle.state).staging, staging_before);
        // The same for the largest index.
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 0xFFFF_FFFF);
        write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 0xCCCC_0003);
        assert_eq!(data_error(&mut bus), ERR_INDEX_OUT_OF_RANGE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), 0xFFFF_FFFF);
        assert_eq!(lock(&handle.state).staging, staging_before);
        // The error is only DATA_ERROR and the next good write clears it.
        assert_eq!(cursor_status(&mut bus), u32::from(ERR_INDEX_OUT_OF_RANGE));
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 0);
        write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, 7);
        assert_eq!(cursor_status(&mut bus), 0);
        assert_eq!(read_reg(&mut bus, reg::STATUS), 0);
        assert_eq!(read_reg(&mut bus, reg::EVENT_STATUS), 0);
    }

    #[test]
    fn image_commit_checks_run_in_order_and_a_failure_changes_only_image_error() {
        let (mut bus, handle) = setup();
        publish(&mut bus, 4, 4, 0x10);
        let before = lock(&handle.state).clone();
        let seq_before = handle.cursor_state();

        let cases: [(&str, u32, u32, u32, u32, u32, u8); 14] = [
            // (name, command, width, height, format, hotspot, expected)
            (
                "command first",
                2,
                0,
                0,
                0,
                0x0001_0001,
                ERR_INVALID_COMMAND,
            ),
            ("command 0", 0, 4, 4, 2, 0, ERR_INVALID_COMMAND),
            (
                "size unset (width)",
                1,
                0,
                4,
                9,
                0xFFFF_FFFF,
                ERR_SIZE_NOT_SET,
            ),
            (
                "size unset (height)",
                1,
                4,
                0,
                9,
                0xFFFF_FFFF,
                ERR_SIZE_NOT_SET,
            ),
            (
                "size before format",
                1,
                65,
                4,
                9,
                0xFFFF_FFFF,
                ERR_UNSUPPORTED_SIZE,
            ),
            ("tall", 1, 4, 65, 2, 0, ERR_UNSUPPORTED_SIZE),
            (
                "huge, no overflow",
                1,
                u32::MAX,
                u32::MAX,
                2,
                0,
                ERR_UNSUPPORTED_SIZE,
            ),
            ("format 0", 1, 4, 4, 0, 0xFFFF_FFFF, ERR_UNSUPPORTED_FORMAT),
            ("format 1 (no alpha)", 1, 4, 4, 1, 0, ERR_UNSUPPORTED_FORMAT),
            ("format 3", 1, 4, 4, 3, 0, ERR_UNSUPPORTED_FORMAT),
            ("format 32", 1, 4, 4, 32, 0, ERR_UNSUPPORTED_FORMAT),
            ("format max", 1, 4, 4, u32::MAX, 0, ERR_UNSUPPORTED_FORMAT),
            ("hotspot x", 1, 4, 4, 2, 4, ERR_HOTSPOT_OUT_OF_IMAGE),
            ("hotspot y", 1, 4, 4, 2, 4 << 16, ERR_HOTSPOT_OUT_OF_IMAGE),
        ];
        for (name, command, width, height, format, hotspot, expected) in cases {
            write_reg(&mut bus, reg::CURSOR_WIDTH, width);
            write_reg(&mut bus, reg::CURSOR_HEIGHT, height);
            write_reg(&mut bus, reg::CURSOR_FORMAT, format);
            write_reg(&mut bus, reg::CURSOR_HOTSPOT, hotspot);
            write_reg(&mut bus, reg::CURSOR_IMAGE_COMMIT, command);
            assert_eq!(image_error(&mut bus), expected, "{name}");
            assert_eq!(handle.cursor_state(), seq_before, "{name}");
            let state = lock(&handle.state);
            assert_eq!(state.active, before.active, "{name}");
            assert_eq!(state.staging, before.staging, "{name}");
            assert_eq!(state.shape_seq, before.shape_seq, "{name}");
            drop(state);
            // Only IMAGE_ERROR moved; the other three fields stay clear.
            assert_eq!(cursor_status(&mut bus), u32::from(expected) << 8, "{name}");
            assert_eq!(read_reg(&mut bus, reg::STATUS), 0, "{name}");
        }
    }

    #[test]
    fn image_commit_boundaries() {
        let (mut bus, handle) = setup();
        for (w, h, hot, ok) in [
            (1, 1, (0, 0), true),
            (64, 64, (63, 63), true),
            (64, 64, (64, 0), false),
            (7, 3, (6, 2), true),
            (7, 3, (7, 0), false),
            (7, 3, (0, 3), false),
            (65, 1, (0, 0), false),
            (1, 65, (0, 0), false),
        ] {
            set_shape(&mut bus, w, h, 2, hot);
            image_commit(&mut bus);
            let code = image_error(&mut bus);
            if ok {
                assert_eq!(code, ERR_NONE, "{w}x{h} {hot:?}");
                let shape = handle.cursor_state().shape.unwrap();
                assert_eq!(
                    (
                        shape.width,
                        shape.height,
                        shape.format,
                        shape.hot_x,
                        shape.hot_y
                    ),
                    (w, h, 2, hot.0, hot.1)
                );
            } else {
                assert_ne!(code, ERR_NONE, "{w}x{h} {hot:?}");
            }
        }
        // (W, 0) is rejected with the hotspot code, not with a size code.
        set_shape(&mut bus, 64, 64, 2, (64, 0));
        image_commit(&mut bus);
        assert_eq!(image_error(&mut bus), ERR_HOTSPOT_OUT_OF_IMAGE);
        set_shape(&mut bus, 65, 1, 2, (0, 0));
        image_commit(&mut bus);
        assert_eq!(image_error(&mut bus), ERR_UNSUPPORTED_SIZE);
    }

    #[test]
    fn image_commit_is_atomic_and_staging_survives_it() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 64, 48);
        publish(&mut bus, 3, 2, 0x1000);
        let (first, bytes) = cursor_image(&handle);
        assert_eq!(first.shape_seq, 1);
        assert_eq!(handle.cursor_state().cursor_seq, 1);
        assert_eq!(&bytes[..8], &[0x00, 0x10, 0, 0, 0x01, 0x10, 0, 0]);

        // New pixels, shape registers and index do not reach `active` until
        // the commit.
        let mut next: Vec<u32> = (0..6).map(|n| 0x2000 + n).collect();
        write_reg(&mut bus, reg::CURSOR_WIDTH, 2);
        write_reg(&mut bus, reg::CURSOR_HEIGHT, 3);
        write_reg(&mut bus, reg::CURSOR_HOTSPOT, position(1, 2));
        load_words(&mut bus, &next);
        assert_eq!(cursor_image(&handle), (first, bytes.clone()));
        assert_eq!(handle.cursor_state().cursor_seq, 1);

        image_commit(&mut bus);
        let (second, second_bytes) = cursor_image(&handle);
        assert_eq!(
            (
                second.shape_seq,
                second.width,
                second.height,
                second.hot_x,
                second.hot_y
            ),
            (2, 2, 3, 1, 2)
        );
        assert_eq!(handle.cursor_state().cursor_seq, 2);
        assert_eq!(&second_bytes[..4], &[0x00, 0x20, 0, 0]);

        // After a commit a staging write does not change what the host reads.
        next[0] = 0xFFFF_FFFF;
        load_words(&mut bus, &next);
        assert_eq!(cursor_image(&handle), (second, second_bytes.clone()));

        // A repeated commit is a new shape with a new `shape_seq`; changing
        // only the hotspot needs no pixel upload (staging survived).
        write_reg(&mut bus, reg::CURSOR_HOTSPOT, position(0, 0));
        image_commit(&mut bus);
        let (third, third_bytes) = cursor_image(&handle);
        assert_eq!((third.shape_seq, third.hot_x, third.hot_y), (3, 0, 0));
        assert_eq!(&third_bytes[..4], &[0xFF, 0xFF, 0xFF, 0xFF]);
        image_commit(&mut bus);
        assert_eq!(handle.cursor_state().shape.unwrap().shape_seq, 4);
        assert_eq!(handle.cursor_state().cursor_seq, 4);

        // Commit changes neither enable nor position, and needs no mode.
        assert_eq!(handle.cursor_state().x, 0);
        assert!(!handle.cursor_state().enabled);
        assert_eq!(lock(&handle.state).generation, 0);
        let (mut bus2, handle2) = setup();
        publish(&mut bus2, 1, 1, 5);
        assert_eq!(handle2.cursor_state().shape.unwrap().shape_seq, 1);
    }

    #[test]
    fn cursor_image_words_follow_the_shape_row_major_without_padding() {
        let (mut bus, handle) = setup();
        set_shape(&mut bus, 3, 2, 2, (1, 1));
        // Pixel (i, j) is word j * W + i; the word is 0xAARRGGBB.
        load_words(
            &mut bus,
            &[
                0x80_10_20_30,
                0xFF_00_00_00,
                0x00_00_00_00,
                0x11_22_33_44,
                0x55_66_77_88,
                0x01_02_03_04,
            ],
        );
        image_commit(&mut bus);
        let (info, bytes) = cursor_image(&handle);
        assert_eq!((info.width, info.height), (3, 2));
        assert_eq!(
            bytes,
            [
                0x30, 0x20, 0x10, 0x80, // B, G, R, A of word 0
                0x00, 0x00, 0x00, 0xFF, //
                0, 0, 0, 0, //
                0x44, 0x33, 0x22, 0x11, //
                0x88, 0x77, 0x66, 0x55, //
                0x04, 0x03, 0x02, 0x01,
            ]
        );
        // Wrong buffer lengths are refused and nothing is copied.
        for len in [0usize, 23, 25, 16384] {
            let mut out = vec![0xEEu8; len];
            assert_eq!(
                handle.read_cursor_image_into(&mut out),
                Err(CursorReadError::BufferLength {
                    expected: 24,
                    actual: len
                })
            );
            assert!(out.iter().all(|b| *b == 0xEE));
        }
    }

    #[test]
    fn cursor_position_is_checked_against_the_active_frame() {
        let (mut bus, handle) = setup();
        // No mode: NO_ACTIVE_MODE, register and counter unchanged.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(0, 0));
        assert_eq!(position_error(&mut bus), ERR_NO_ACTIVE_MODE);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(1, 1));
        assert_eq!(position_error(&mut bus), ERR_NO_ACTIVE_MODE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_POSITION), 0);
        assert_eq!(handle.cursor_state().cursor_seq, 0);

        commit_mode(&mut bus, 64, 48);
        // The edge pixel is inside, one past it is outside.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(63, 47));
        assert_eq!(position_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_POSITION), position(63, 47));
        assert_eq!(handle.cursor_state().cursor_seq, 1);
        for bad in [
            position(64, 0),
            position(0, 48),
            position(64, 48),
            position(0xFFFF, 0),
            position(0, 0xFFFF),
            0xFFFF_FFFF,
        ] {
            write_reg(&mut bus, reg::CURSOR_POSITION, bad);
            assert_eq!(position_error(&mut bus), ERR_POSITION_OUT_OF_FRAME);
            assert_eq!(read_reg(&mut bus, reg::CURSOR_POSITION), position(63, 47));
            assert_eq!(handle.cursor_state().cursor_seq, 1);
            // Only POSITION_ERROR is set.
            assert_eq!(
                cursor_status(&mut bus),
                u32::from(ERR_POSITION_OUT_OF_FRAME) << 16
            );
        }
        // The same value succeeds, clears the error and keeps the counter.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(63, 47));
        assert_eq!(cursor_status(&mut bus), 0);
        assert_eq!(handle.cursor_state().cursor_seq, 1);
        // X and Y travel in one register: X low, Y high.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(5, 9));
        let state = handle.cursor_state();
        assert_eq!((state.x, state.y, state.cursor_seq), (5, 9, 2));
        assert_eq!(read_reg(&mut bus, reg::STATUS) >> 8 & 0xFF, 0);
    }

    #[test]
    fn cursor_enable_follows_the_contract() {
        let (mut bus, handle) = setup();
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        assert_eq!(enable_error(&mut bus), ERR_NO_CURSOR_IMAGE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_ENABLE), 0);
        for bad in [2u32, 3, 0x8000_0000, 0xFFFF_FFFF] {
            write_reg(&mut bus, reg::CURSOR_ENABLE, bad);
            assert_eq!(enable_error(&mut bus), ERR_INVALID_COMMAND, "{bad:#x}");
            assert_eq!(read_reg(&mut bus, reg::CURSOR_ENABLE), 0);
        }
        // Hiding a hidden cursor is allowed and changes nothing.
        write_reg(&mut bus, reg::CURSOR_ENABLE, 0);
        assert_eq!(enable_error(&mut bus), ERR_NONE);
        assert_eq!(handle.cursor_state().cursor_seq, 0);

        publish(&mut bus, 2, 2, 1);
        let mut seqs = vec![handle.cursor_state().cursor_seq];
        for value in [1u32, 1, 0, 0, 1] {
            write_reg(&mut bus, reg::CURSOR_ENABLE, value);
            assert_eq!(enable_error(&mut bus), ERR_NONE);
            assert_eq!(read_reg(&mut bus, reg::CURSOR_ENABLE), value);
            seqs.push(handle.cursor_state().cursor_seq);
        }
        // Grows only on 0 -> 1 and 1 -> 0.
        assert_eq!(seqs, [1, 2, 2, 3, 3, 4]);
        // Hiding keeps shape and position; showing returns the same cursor.
        commit_mode(&mut bus, 16, 16);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(3, 4));
        let shown = handle.cursor_state();
        write_reg(&mut bus, reg::CURSOR_ENABLE, 0);
        let hidden = handle.cursor_state();
        assert_eq!((hidden.shape, hidden.x, hidden.y), (shown.shape, 3, 4));
        assert!(!hidden.enabled);
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        let again = handle.cursor_state();
        assert!(again.enabled);
        assert_eq!((again.shape, again.x, again.y), (shown.shape, 3, 4));
    }

    #[test]
    fn cursor_never_touches_the_frame_and_the_frame_never_touches_the_cursor() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 64, 48);
        for n in 0..(64 * 48u64) {
            bus.write_physical(
                GuestPhysicalAddress::new(DISPLAY_FRAMEBUFFER_BASE + n * 4),
                AccessWidth::Dword,
                0xFF00_0000 | n,
                |_| panic!("pixel stores must not reach the callback"),
            )
            .unwrap();
        }
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        let mut frame_before = vec![0u8; 64 * 48 * 4];
        let info_before = handle.read_frame_into(&bus, &mut frame_before).unwrap();
        let status = read_reg(&mut bus, reg::STATUS);
        let event_status = read_reg(&mut bus, reg::EVENT_STATUS);
        let (generation, changes) = (handle.frame_generation(), handle.mode_change_count());
        let seq_before = handle.cursor_state().cursor_seq;

        let words: Vec<u32> = (0..4096u32).collect();
        set_shape(&mut bus, 64, 64, 2, (2, 2));
        load_words(&mut bus, &words);
        image_commit(&mut bus);
        for n in 0..1000u32 {
            write_reg(&mut bus, reg::CURSOR_POSITION, position(n % 64, n % 48));
        }
        for value in [1, 0, 1, 0] {
            write_reg(&mut bus, reg::CURSOR_ENABLE, value);
        }

        assert_eq!(handle.frame_generation(), generation);
        assert_eq!(handle.mode_change_count(), changes);
        assert!(handle.has_current_frame());
        assert_eq!(read_reg(&mut bus, reg::STATUS), status);
        assert_eq!(read_reg(&mut bus, reg::EVENT_STATUS), event_status);
        let mut frame_after = vec![0u8; 64 * 48 * 4];
        let info_after = handle.read_frame_into(&bus, &mut frame_after).unwrap();
        assert_eq!(info_after, info_before);
        assert_eq!(frame_after, frame_before);
        assert!(handle.cursor_state().cursor_seq > seq_before);

        // The reverse: MODE_COMMIT and the doorbell leave the cursor alone,
        // even when the old position is outside the new frame.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(40, 30));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        let cursor = handle.cursor_state();
        commit_mode(&mut bus, 16, 16);
        write_reg(&mut bus, reg::FRAME_GENERATION, 0);
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 1);
        write_reg(&mut bus, reg::DRIVER_PROTOCOL, 0);
        assert_eq!(handle.cursor_state(), cursor);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_POSITION), position(40, 30));
        // Writes elsewhere in versions 1 and 2 do not change the cursor.
        write_reg(&mut bus, reg::WIDTH, 5);
        write_reg(&mut bus, reg::EVENT_POP, 1);
        write_reg(&mut bus, reg::REQUEST_RESPONSE, 1);
        assert_eq!(handle.cursor_state(), cursor);
    }

    #[test]
    fn host_cursor_api_only_reads() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 16, 16);
        publish(&mut bus, 4, 4, 9);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(2, 2));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        write_reg(&mut bus, reg::CURSOR_ENABLE, 7);
        let before = lock(&handle.state).clone();
        let _ = handle.cursor_state();
        let _ = cursor_image(&handle);
        let mut small = [0u8; 3];
        let _ = handle.read_cursor_image_into(&mut small);
        assert_eq!(*lock(&handle.state), before);
    }

    #[test]
    fn reset_removes_the_cursor_and_bumps_the_counter_only_when_something_was_shown() {
        // A clean device: nothing shown, nothing changes.
        let (mut bus, handle) = setup();
        handle.reset();
        assert_eq!(handle.cursor_state().cursor_seq, 0);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_STATUS), 0);

        // A shown cursor is hidden: the counter grows by exactly one.
        commit_mode(&mut bus, 64, 48);
        publish(&mut bus, 4, 4, 0x77);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(10, 10));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 99);
        write_reg(&mut bus, reg::CURSOR_ENABLE, 9);
        let seq = handle.cursor_state().cursor_seq;
        handle.reset();
        let after = handle.cursor_state();
        assert_eq!(
            after,
            CursorState {
                cursor_seq: seq + 1,
                enabled: false,
                x: 0,
                y: 0,
                shape: None
            }
        );
        for offset in [
            reg::CURSOR_STATUS,
            reg::CURSOR_WIDTH,
            reg::CURSOR_HEIGHT,
            reg::CURSOR_FORMAT,
            reg::CURSOR_HOTSPOT,
            reg::CURSOR_IMAGE_INDEX,
            reg::CURSOR_POSITION,
            reg::CURSOR_ENABLE,
        ] {
            assert_eq!(read_reg(&mut bus, offset), 0, "offset {offset:#x}");
        }
        {
            let state = lock(&handle.state);
            assert!(state.staging.iter().all(|w| *w == 0));
            assert!(state.active.iter().all(|w| *w == 0));
            assert_eq!(state.shape_seq, 0);
        }
        let mut out = [0u8; 64];
        assert_eq!(
            handle.read_cursor_image_into(&mut out),
            Err(CursorReadError::NoShape)
        );
        // A second reset shows nothing new.
        handle.reset();
        assert_eq!(handle.cursor_state().cursor_seq, seq + 1);

        // A shape alone, or a non-zero position alone, counts as shown state.
        commit_mode(&mut bus, 64, 48);
        publish(&mut bus, 1, 1, 1);
        let seq = handle.cursor_state().cursor_seq;
        handle.reset();
        assert_eq!(handle.cursor_state().cursor_seq, seq + 1);
        commit_mode(&mut bus, 64, 48);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(1, 0));
        let seq = handle.cursor_state().cursor_seq;
        handle.reset();
        assert_eq!(handle.cursor_state().cursor_seq, seq + 1);

        // Numbers are not reused: the shape after a reset is newer than any
        // shape before it, and an empty staging buffer publishes zeros.
        commit_mode(&mut bus, 64, 48);
        let before = handle.cursor_state().cursor_seq;
        set_shape(&mut bus, 2, 2, 2, (0, 0));
        image_commit(&mut bus);
        let (info, bytes) = cursor_image(&handle);
        assert!(info.shape_seq > before);
        assert!(bytes.iter().all(|b| *b == 0));
        // Four commits and three resets of an active mode: the frame counters
        // follow their own rules and the cursor never moved them.
        assert_eq!(handle.mode_change_count(), 7);
        assert_eq!(handle.frame_generation(), 0);
    }

    #[test]
    fn cursor_counter_saturation_rejects_the_three_commands_and_reset_keeps_the_counter() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 64, 48);
        publish(&mut bus, 2, 2, 3);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(4, 4));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        lock(&handle.state).cursor_seq = u64::MAX;
        let before = lock(&handle.state).clone();

        image_commit(&mut bus);
        assert_eq!(image_error(&mut bus), ERR_COUNTER_EXHAUSTED);
        write_reg(&mut bus, reg::CURSOR_POSITION, position(5, 5));
        assert_eq!(position_error(&mut bus), ERR_COUNTER_EXHAUSTED);
        write_reg(&mut bus, reg::CURSOR_ENABLE, 0);
        assert_eq!(enable_error(&mut bus), ERR_COUNTER_EXHAUSTED);
        let mut expected = before.clone();
        expected.cursor_image_error = ERR_COUNTER_EXHAUSTED;
        expected.cursor_position_error = ERR_COUNTER_EXHAUSTED;
        expected.cursor_enable_error = ERR_COUNTER_EXHAUSTED;
        assert_eq!(*lock(&handle.state), expected);
        // Commands that change nothing still succeed.
        write_reg(&mut bus, reg::CURSOR_POSITION, position(4, 4));
        write_reg(&mut bus, reg::CURSOR_ENABLE, 1);
        assert_eq!(position_error(&mut bus), ERR_NONE);
        assert_eq!(enable_error(&mut bus), ERR_NONE);
        // The assumption of 15.9: reset hides the cursor, the counter stays.
        handle.reset();
        let state = handle.cursor_state();
        assert_eq!(state.cursor_seq, u64::MAX);
        assert!(!state.enabled && state.shape.is_none());
        assert_eq!((state.x, state.y), (0, 0));
    }

    #[test]
    fn cursor_registers_fail_with_backend_errors_before_any_state_change() {
        let (mut bus, handle) = setup();
        commit_mode(&mut bus, 64, 48);
        publish(&mut bus, 2, 2, 3);
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 5);
        let before = lock(&handle.state).clone();
        let (mut device, _h) = DisplayDevice::new_with_handle();
        for offset in [
            reg::CURSOR_MAX_WIDTH,
            reg::CURSOR_STATUS,
            reg::CURSOR_WIDTH,
            reg::CURSOR_HOTSPOT,
            reg::CURSOR_IMAGE_INDEX,
            reg::CURSOR_IMAGE_DATA,
            reg::CURSOR_IMAGE_COMMIT,
            0x22C,
            reg::CURSOR_POSITION,
            reg::CURSOR_ENABLE,
            0x2FC,
        ] {
            for width in [AccessWidth::Byte, AccessWidth::Word, AccessWidth::Qword] {
                let access = DeviceAccess {
                    space: IoSpace::Mmio,
                    offset,
                    width,
                };
                assert_eq!(device.read(access), Err(DeviceError::UnsupportedWidth));
                assert_eq!(device.write(access, 1), Err(DeviceError::UnsupportedWidth));
                assert!(matches!(
                    bus.read_physical(reg_addr(offset), width),
                    Err(BusAccessError::Device {
                        source: DeviceError::UnsupportedWidth,
                        ..
                    })
                ));
                assert!(matches!(
                    bus.write_physical(reg_addr(offset), width, 1, |_| {}),
                    Err(BusAccessError::Device {
                        source: DeviceError::UnsupportedWidth,
                        ..
                    })
                ));
            }
            let misaligned = DeviceAccess {
                space: IoSpace::Mmio,
                offset: offset + 1,
                width: AccessWidth::Dword,
            };
            assert_eq!(device.read(misaligned), Err(DeviceError::Misaligned));
            assert_eq!(device.write(misaligned, 1), Err(DeviceError::Misaligned));
            let pio = DeviceAccess {
                space: IoSpace::Pio,
                offset,
                width: AccessWidth::Dword,
            };
            assert_eq!(device.read(pio), Err(DeviceError::UnsupportedSpace));
            assert_eq!(device.write(pio, 1), Err(DeviceError::UnsupportedSpace));
            assert!(matches!(
                bus.write_physical(reg_addr(offset + 2), AccessWidth::Dword, 1, |_| {}),
                Err(BusAccessError::Device {
                    source: DeviceError::Misaligned,
                    ..
                })
            ));
        }
        // Including the auto-increment of IMAGE_INDEX and the staging write.
        assert_eq!(*lock(&handle.state), before);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), 5);
    }

    #[test]
    fn cursor_block_leaves_guest_memory_alone_and_loading_never_calls_back() {
        // `write_reg` panics if a register write reaches the committed-write
        // callback; a full 64 x 64 upload must therefore stay silent.
        let (mut bus, handle) = setup();
        let words: Vec<u32> = (0..4096u32).map(|n| n.wrapping_mul(0x0101_0101)).collect();
        set_shape(&mut bus, 64, 64, 2, (63, 63));
        load_words(&mut bus, &words);
        image_commit(&mut bus);
        assert_eq!(image_error(&mut bus), ERR_NONE);
        assert_eq!(read_reg(&mut bus, reg::CURSOR_IMAGE_INDEX), 4096);
        let (info, bytes) = cursor_image(&handle);
        assert_eq!(bytes.len(), 16_384);
        assert_eq!((info.width, info.height), (64, 64));
        assert_eq!(
            &bytes[16_380..],
            &4095u32.wrapping_mul(0x0101_0101).to_le_bytes()
        );
    }

    #[test]
    fn cursor_upload_time_of_a_64_by_64_shape_through_the_bus() {
        // Measurement for DISPLAY_DEVICE.md, 15.13 item 9 (4 102 register
        // writes). Run with `--release -- --nocapture` for a representative
        // figure; the test only checks the result, not the time.
        let (mut bus, handle) = setup();
        let started = std::time::Instant::now();
        set_shape(&mut bus, 64, 64, 2, (0, 0));
        write_reg(&mut bus, reg::CURSOR_IMAGE_INDEX, 0);
        for n in 0..4096u32 {
            write_reg(&mut bus, reg::CURSOR_IMAGE_DATA, n);
        }
        image_commit(&mut bus);
        let elapsed = started.elapsed();
        eprintln!("cursor 64x64 upload, 4102 writes: {elapsed:?}");
        assert_eq!(handle.cursor_state().shape.unwrap().shape_seq, 1);
    }
}
