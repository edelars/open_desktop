//! Own display device, contract version 1 (`docs/specs/DISPLAY_DEVICE.md`).
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
const VERSION: u32 = 1;
const FORMATS: u32 = 1 << DISPLAY_FORMAT_BGRX8888;
const BYTES_PER_PIXEL: u32 = 4;

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
}

impl DisplayState {
    const fn new() -> Self {
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
        }
    }

    /// Host reset: registers and mode return to power-on values; `generation`
    /// and `frame_mode_epoch` are kept so the generation stays monotonic.
    fn reset(&mut self) {
        let was_active = self.mode.is_active();
        *self = Self {
            generation: self.generation,
            mode_change_count: self.mode_change_count,
            frame_mode_epoch: self.frame_mode_epoch,
            ..Self::new()
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
            reg::CAPS => 0,
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
            // MODE_COMMIT (write-only) and every reserved offset read as zero.
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
            // Read-only and reserved offsets: the write is ignored.
            _ => {}
        }
        Ok(())
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
        let (width, height) = (self.requested_width, self.requested_height);
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
        let format = self.requested_format;
        if format >= 32 || FORMATS & (1 << format) == 0 {
            return Err(ERR_UNSUPPORTED_FORMAT);
        }
        if self.mode_change_count == u64::MAX {
            return Err(ERR_COUNTER_EXHAUSTED);
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

    /// Host reset (DISPLAY_DEVICE.md, section 10). Framebuffer bytes are not
    /// cleared and `generation` is preserved.
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
            (reg::VERSION, 1),
            (reg::CAPS, 0),
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
}
