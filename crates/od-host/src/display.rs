//! C ABI for one display session over `od-machine` (S10-04).
//!
//! Contract: `docs/specs/DISPLAY_HOST_ABI_V1.md`; declarations:
//! `include/open_desktop/display_abi_v1.h`. This is the first production use of
//! `od-machine` from `od-host`.
//!
//! A handle is an opaque, never-reused identifier registered in a process-wide
//! table; it is not a dereferenceable pointer. Every call resolves the handle
//! through that table, so a stale, forged or already destroyed handle yields
//! `INVALID_HANDLE` instead of undefined behavior. Handle use is still a
//! single-caller protocol: overlapping calls on one handle are detected and
//! answered with `BUSY`.

use od_machine::{
    AccessWidth, CursorReadError, DISPLAY_FORMAT_BGRX8888, DISPLAY_FRAMEBUFFER_BASE,
    DISPLAY_FRAMEBUFFER_BYTES, DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES, DisplayDevice,
    DisplayHandle, DisplayReadError, GuestPhysicalAddress, GuestPhysicalRange, MachineBus,
    MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, MmioBinding, RamLimits,
};
use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::mem::{align_of, size_of};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

const ABI_VERSION: u32 = 0x0001_0000;
/// Identifier of the cursor extension (S10-14): the structs and functions added
/// after v1. Existing v1 symbols keep `ABI_VERSION`.
const ABI_VERSION_V2: u32 = 0x0002_0000;

const STATUS_OK: u32 = 0;
const STATUS_INVALID_ARGUMENT: u32 = 1;
const STATUS_ABI_MISMATCH: u32 = 2;
const STATUS_UNSUPPORTED_PROFILE: u32 = 3;
const STATUS_INVALID_HANDLE: u32 = 4;
const STATUS_BUSY: u32 = 5;
const STATUS_NO_ACTIVE_MODE: u32 = 6;
const STATUS_NO_FRAME: u32 = 7;
const STATUS_BUFFER_LENGTH: u32 = 8;
const STATUS_MODE_CHANGED: u32 = 9;
const STATUS_MEMORY_ERROR: u32 = 10;
const STATUS_INTERNAL_ERROR: u32 = 11;
const STATUS_NO_CURSOR_SHAPE: u32 = 12;
const STATUS_CURSOR_BUFFER_LENGTH: u32 = 13;

const CURSOR_FLAG_ENABLED: u32 = 1;
const CURSOR_FLAG_HAS_SHAPE: u32 = 2;

const PROFILE_TEST_PATTERN: u32 = 1;

/// Mode and pattern programmed by profile 1 at create: `bars`, seed 0, as in
/// `tools/build/display_pattern.py` and
/// `tests/fixtures/guest/stage10/bars_64x48_seed0.raw`.
const TEST_PATTERN_WIDTH: u32 = 64;
const TEST_PATTERN_HEIGHT: u32 = 48;
/// Bar colors as (B, G, R), left to right (white, yellow, cyan, green,
/// magenta, red, blue, black).
const BARS_BGR: [[u8; 3]; 8] = [
    [0xFF, 0xFF, 0xFF],
    [0x00, 0xFF, 0xFF],
    [0xFF, 0xFF, 0x00],
    [0x00, 0xFF, 0x00],
    [0xFF, 0x00, 0xFF],
    [0x00, 0x00, 0xFF],
    [0xFF, 0x00, 0x00],
    [0x00, 0x00, 0x00],
];

const STEP_NOTHING_TO_EXECUTE: u32 = 1;

const MODE_FLAG_ACTIVE: u32 = 1;
const MODE_FLAG_HAS_FRAME: u32 = 2;

/// Opaque type behind `od_display_session *`; never dereferenced.
#[repr(C)]
pub struct OdDisplaySession {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdDisplaySessionConfig {
    pub abi_version: u32,
    pub struct_size: u32,
    pub profile: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdDisplayMode {
    pub abi_version: u32,
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub flags: u32,
    pub reserved: u32,
    pub generation: u64,
    pub mode_change_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdDisplayFrameInfo {
    pub abi_version: u32,
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: u32,
    pub reserved0: u32,
    pub reserved1: u32,
    pub generation: u64,
    pub mode_change_count: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdDisplayStepReport {
    pub abi_version: u32,
    pub struct_size: u32,
    pub outcome: u32,
    pub reserved: u32,
    pub instructions_executed: u64,
}

/// Cursor state (ABI v2, DISPLAY_DEVICE.md 15.10): one read of the device's
/// cursor view. Without a published shape, `shape_seq`, `width`, `height`,
/// `format`, `hot_x` and `hot_y` are zero and `HAS_SHAPE` is clear.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct OdDisplayCursorState {
    pub abi_version: u32,
    pub struct_size: u32,
    pub cursor_seq: u64,
    pub shape_seq: u64,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub format: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub pos_x: u32,
    pub pos_y: u32,
    pub reserved0: u32,
    pub reserved1: u32,
}

const _: () = {
    use std::mem::offset_of;
    // Size, alignment and field offsets are checked against the numbers in
    // include/open_desktop/display_abi_v1.h. Status codes, profiles, flags and
    // outcomes below are duplicated by hand and are not compared automatically.
    assert!(size_of::<OdDisplaySessionConfig>() == 16);
    assert!(align_of::<OdDisplaySessionConfig>() == 4);
    assert!(offset_of!(OdDisplaySessionConfig, abi_version) == 0);
    assert!(offset_of!(OdDisplaySessionConfig, struct_size) == 4);
    assert!(offset_of!(OdDisplaySessionConfig, profile) == 8);
    assert!(offset_of!(OdDisplaySessionConfig, reserved) == 12);

    assert!(size_of::<OdDisplayMode>() == 48);
    assert!(align_of::<OdDisplayMode>() == 8);
    assert!(offset_of!(OdDisplayMode, abi_version) == 0);
    assert!(offset_of!(OdDisplayMode, struct_size) == 4);
    assert!(offset_of!(OdDisplayMode, width) == 8);
    assert!(offset_of!(OdDisplayMode, height) == 12);
    assert!(offset_of!(OdDisplayMode, stride) == 16);
    assert!(offset_of!(OdDisplayMode, format) == 20);
    assert!(offset_of!(OdDisplayMode, flags) == 24);
    assert!(offset_of!(OdDisplayMode, reserved) == 28);
    assert!(offset_of!(OdDisplayMode, generation) == 32);
    assert!(offset_of!(OdDisplayMode, mode_change_count) == 40);

    assert!(size_of::<OdDisplayFrameInfo>() == 48);
    assert!(align_of::<OdDisplayFrameInfo>() == 8);
    assert!(offset_of!(OdDisplayFrameInfo, abi_version) == 0);
    assert!(offset_of!(OdDisplayFrameInfo, struct_size) == 4);
    assert!(offset_of!(OdDisplayFrameInfo, width) == 8);
    assert!(offset_of!(OdDisplayFrameInfo, height) == 12);
    assert!(offset_of!(OdDisplayFrameInfo, stride) == 16);
    assert!(offset_of!(OdDisplayFrameInfo, format) == 20);
    assert!(offset_of!(OdDisplayFrameInfo, reserved0) == 24);
    assert!(offset_of!(OdDisplayFrameInfo, reserved1) == 28);
    assert!(offset_of!(OdDisplayFrameInfo, generation) == 32);
    assert!(offset_of!(OdDisplayFrameInfo, mode_change_count) == 40);

    assert!(size_of::<OdDisplayStepReport>() == 24);
    assert!(align_of::<OdDisplayStepReport>() == 8);
    assert!(offset_of!(OdDisplayStepReport, abi_version) == 0);
    assert!(offset_of!(OdDisplayStepReport, struct_size) == 4);
    assert!(offset_of!(OdDisplayStepReport, outcome) == 8);
    assert!(offset_of!(OdDisplayStepReport, reserved) == 12);
    assert!(offset_of!(OdDisplayStepReport, instructions_executed) == 16);

    assert!(size_of::<OdDisplayCursorState>() == 64);
    assert!(align_of::<OdDisplayCursorState>() == 8);
    assert!(offset_of!(OdDisplayCursorState, abi_version) == 0);
    assert!(offset_of!(OdDisplayCursorState, struct_size) == 4);
    assert!(offset_of!(OdDisplayCursorState, cursor_seq) == 8);
    assert!(offset_of!(OdDisplayCursorState, shape_seq) == 16);
    assert!(offset_of!(OdDisplayCursorState, flags) == 24);
    assert!(offset_of!(OdDisplayCursorState, width) == 28);
    assert!(offset_of!(OdDisplayCursorState, height) == 32);
    assert!(offset_of!(OdDisplayCursorState, format) == 36);
    assert!(offset_of!(OdDisplayCursorState, hot_x) == 40);
    assert!(offset_of!(OdDisplayCursorState, hot_y) == 44);
    assert!(offset_of!(OdDisplayCursorState, pos_x) == 48);
    assert!(offset_of!(OdDisplayCursorState, pos_y) == 52);
    assert!(offset_of!(OdDisplayCursorState, reserved0) == 56);
    assert!(offset_of!(OdDisplayCursorState, reserved1) == 60);
};

/// Machine behind one handle: a display device on a `MachineBus`.
struct Session {
    // Kept for the profile-specific stepping of later profiles; profile 1 has
    // no guest.
    profile: u32,
    bus: MachineBus,
    display: DisplayHandle,
}

struct Entry {
    /// Held for the duration of every call; exclusive access to `session`.
    busy: AtomicBool,
    /// Set by destroy while `busy` is held; later callers see INVALID_HANDLE.
    dead: AtomicBool,
    /// Set after a caught panic; only destroy is still accepted.
    failed: AtomicBool,
    session: UnsafeCell<Session>,
}

// SAFETY: `session` is only accessed while `busy` is held (acquire/release), so
// accesses are serialized and ordered across threads. `Session` contains only
// `DisplayDevice` (state behind `Arc<Mutex<_>>`), `DisplayHandle` and the
// `MachineBus` that owns that device: plain owned data without thread-local
// state or `Rc`. It may therefore be moved to and dropped on any thread.
unsafe impl Send for Entry {}
// SAFETY: see `Send`; shared references only reach the atomics and, through
// `Guard`, the `UnsafeCell` under the `busy` flag.
unsafe impl Sync for Entry {}

struct Guard<'a>(&'a Entry);

impl Entry {
    fn enter(&self) -> Option<Guard<'_>> {
        self.busy
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Guard(self))
    }
}

impl Guard<'_> {
    fn session(&mut self) -> &mut Session {
        // SAFETY: `busy` is held by this guard, so no other reference exists.
        unsafe { &mut *self.0.session.get() }
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

struct Registry {
    /// Identifiers are never reused; zero is never issued (NULL).
    next_id: usize,
    entries: BTreeMap<usize, Arc<Entry>>,
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    next_id: 1,
    entries: BTreeMap::new(),
});

fn registry() -> std::sync::MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

fn handle_to_id(handle: *mut OdDisplaySession) -> usize {
    handle.addr()
}

fn id_to_handle(id: usize) -> *mut OdDisplaySession {
    std::ptr::without_provenance_mut(id)
}

fn lookup(handle: *mut OdDisplaySession) -> Option<Arc<Entry>> {
    registry().entries.get(&handle_to_id(handle)).cloned()
}

fn range(start: u64, len: u64) -> Option<GuestPhysicalRange> {
    GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).ok()
}

/// Profile 1: a small RAM region, the display registers and the framebuffer
/// region; no guest code. The built-in `bars` frame is drawn at create.
fn build_test_pattern_session() -> Option<Session> {
    const RAM_BYTES: u64 = 0x1_0000;
    let (device, display) = DisplayDevice::new_with_handle();
    let map = MachineMemoryMap::new(
        36,
        vec![
            MemoryRegionSpec::new(range(0, RAM_BYTES)?, MemoryRegionKind::Ram),
            MemoryRegionSpec::new(
                range(DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES)?,
                MemoryRegionKind::Framebuffer,
            ),
            MemoryRegionSpec::new(
                range(DISPLAY_REGISTERS_BASE, DISPLAY_REGISTERS_BYTES)?,
                MemoryRegionKind::Mmio,
            ),
        ],
    )
    .ok()?;
    let bus = MachineBus::new(
        map,
        RamLimits::new(RAM_BYTES),
        vec![Box::new(device)],
        vec![MmioBinding {
            region_start: GuestPhysicalAddress::new(DISPLAY_REGISTERS_BASE),
            device_index: 0,
        }],
        vec![],
    )
    .ok()?;
    let mut session = Session {
        profile: PROFILE_TEST_PATTERN,
        bus,
        display,
    };
    program_test_pattern(&mut session)?;
    Some(session)
}

/// Does what a guest would: programs the mode through the registers, stores
/// the pixels into framebuffer memory and rings the `FRAME_GENERATION`
/// doorbell. Nothing bypasses the device.
fn program_test_pattern(session: &mut Session) -> Option<()> {
    const WIDTH: u64 = DISPLAY_REGISTERS_BASE + 0x040;
    const HEIGHT: u64 = DISPLAY_REGISTERS_BASE + 0x044;
    const FORMAT: u64 = DISPLAY_REGISTERS_BASE + 0x048;
    const MODE_COMMIT: u64 = DISPLAY_REGISTERS_BASE + 0x04C;
    const FRAME_GENERATION: u64 = DISPLAY_REGISTERS_BASE + 0x080;
    let bus = &mut session.bus;
    let mut store = |address: u64, value: u32| {
        bus.write_physical(
            GuestPhysicalAddress::new(address),
            AccessWidth::Dword,
            u64::from(value),
            |_| {},
        )
        .ok()
    };
    store(WIDTH, TEST_PATTERN_WIDTH)?;
    store(HEIGHT, TEST_PATTERN_HEIGHT)?;
    store(FORMAT, DISPLAY_FORMAT_BGRX8888)?;
    store(MODE_COMMIT, 1)?;
    let bar_count = BARS_BGR.len() as u32;
    for y in 0..TEST_PATTERN_HEIGHT {
        for x in 0..TEST_PATTERN_WIDTH {
            let [b, g, r] = BARS_BGR[(x * bar_count / TEST_PATTERN_WIDTH) as usize];
            let offset = u64::from(y * TEST_PATTERN_WIDTH * 4 + x * 4);
            store(
                DISPLAY_FRAMEBUFFER_BASE + offset,
                u32::from_le_bytes([b, g, r, 0]),
            )?;
        }
    }
    store(FRAME_GENERATION, 1)?;
    let mode = session.display.current_mode();
    (mode.width == TEST_PATTERN_WIDTH
        && mode.height == TEST_PATTERN_HEIGHT
        && session.display.has_current_frame())
    .then_some(())
}

/// Runs `body` so that a Rust panic never crosses the C boundary.
fn guarded(body: impl FnOnce() -> u32) -> u32 {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(STATUS_INTERNAL_ERROR)
}

/// Resolves `handle`, takes exclusive use and runs `body` on its session.
/// A panic inside `body` marks the session failed.
fn with_session(handle: *mut OdDisplaySession, body: impl FnOnce(&mut Session) -> u32) -> u32 {
    if handle.is_null() {
        return STATUS_INVALID_ARGUMENT;
    }
    let Some(entry) = lookup(handle) else {
        return STATUS_INVALID_HANDLE;
    };
    let Some(mut guard) = entry.enter() else {
        return STATUS_BUSY;
    };
    if entry.dead.load(Ordering::Acquire) {
        return STATUS_INVALID_HANDLE;
    }
    if entry.failed.load(Ordering::Acquire) {
        return STATUS_INTERNAL_ERROR;
    }
    let status = catch_unwind(AssertUnwindSafe(|| body(guard.session())));
    match status {
        Ok(status) => status,
        Err(_) => {
            entry.failed.store(true, Ordering::Release);
            STATUS_INTERNAL_ERROR
        }
    }
}

/// Non-null, aligned `T` pointer.
fn valid_ptr<T>(pointer: *const T) -> bool {
    !pointer.is_null() && pointer.is_aligned()
}

fn overlaps(a: *const u8, a_len: usize, b: *const u8, b_len: usize) -> bool {
    let (a, b) = (a.addr(), b.addr());
    a < b.saturating_add(b_len) && b < a.saturating_add(a_len)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_create(
    config: *const OdDisplaySessionConfig,
    out_session: *mut *mut OdDisplaySession,
) -> u32 {
    guarded(|| {
        if !valid_ptr(config) || !valid_ptr(out_session) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; accessibility and lifetime are caller
        // obligations of the ABI. The slot is read, not written, until success.
        if unsafe { !(*out_session).is_null() } {
            return STATUS_INVALID_ARGUMENT;
        }
        if overlaps(
            config.cast(),
            size_of::<OdDisplaySessionConfig>(),
            out_session.cast(),
            size_of::<*mut OdDisplaySession>(),
        ) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: as above.
        let config = unsafe { *config };
        if config.abi_version != ABI_VERSION
            || config.struct_size as usize != size_of::<OdDisplaySessionConfig>()
        {
            return STATUS_ABI_MISMATCH;
        }
        if config.reserved != 0 {
            return STATUS_INVALID_ARGUMENT;
        }
        let session = match config.profile {
            PROFILE_TEST_PATTERN => match build_test_pattern_session() {
                Some(session) => session,
                None => return STATUS_INTERNAL_ERROR,
            },
            _ => return STATUS_UNSUPPORTED_PROFILE,
        };
        let entry = Arc::new(Entry {
            busy: AtomicBool::new(false),
            dead: AtomicBool::new(false),
            failed: AtomicBool::new(false),
            session: UnsafeCell::new(session),
        });
        let id = {
            let mut registry = registry();
            let id = registry.next_id;
            let Some(next) = id.checked_add(1) else {
                return STATUS_INTERNAL_ERROR;
            };
            registry.next_id = next;
            registry.entries.insert(id, entry);
            id
        };
        // SAFETY: pointer shape checked above; success transfers the handle.
        unsafe { out_session.write(id_to_handle(id)) };
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_destroy(handle: *mut OdDisplaySession) -> u32 {
    guarded(|| {
        if handle.is_null() {
            return STATUS_INVALID_ARGUMENT;
        }
        let removed = {
            let mut registry = registry();
            let id = handle_to_id(handle);
            let Some(entry) = registry.entries.get(&id) else {
                return STATUS_INVALID_HANDLE;
            };
            // An in-flight call on this handle keeps it alive: BUSY.
            let Some(guard) = entry.enter() else {
                return STATUS_BUSY;
            };
            entry.dead.store(true, Ordering::Release);
            drop(guard);
            registry.entries.remove(&id)
        };
        // The machine (36 MiB framebuffer) is released outside the registry
        // lock, when the last reference is dropped.
        drop(removed);
        STATUS_OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_mode(
    handle: *mut OdDisplaySession,
    out_mode: *mut OdDisplayMode,
) -> u32 {
    guarded(|| {
        if !valid_ptr(out_mode) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; header fields are read before any write.
        let (abi_version, struct_size) =
            unsafe { ((*out_mode).abi_version, (*out_mode).struct_size) };
        if abi_version != ABI_VERSION || struct_size as usize != size_of::<OdDisplayMode>() {
            return STATUS_ABI_MISMATCH;
        }
        with_session(handle, |session| {
            let mode = session.display.current_mode();
            let mut flags = 0;
            if mode.is_active() {
                flags |= MODE_FLAG_ACTIVE;
            }
            if session.display.has_current_frame() {
                flags |= MODE_FLAG_HAS_FRAME;
            }
            // SAFETY: pointer shape checked above; success writes the whole struct.
            unsafe {
                out_mode.write(OdDisplayMode {
                    abi_version: ABI_VERSION,
                    struct_size: size_of::<OdDisplayMode>() as u32,
                    width: mode.width,
                    height: mode.height,
                    stride: mode.stride,
                    format: mode.format,
                    flags,
                    reserved: 0,
                    generation: session.display.frame_generation(),
                    mode_change_count: session.display.mode_change_count(),
                });
            }
            STATUS_OK
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_read_frame(
    handle: *mut OdDisplaySession,
    buffer: *mut u8,
    buffer_len: usize,
    out_info: *mut OdDisplayFrameInfo,
) -> u32 {
    guarded(|| {
        if buffer.is_null() || !valid_ptr(out_info) {
            return STATUS_INVALID_ARGUMENT;
        }
        if overlaps(
            buffer,
            buffer_len,
            out_info.cast(),
            size_of::<OdDisplayFrameInfo>(),
        ) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; header fields are read before any write.
        let (abi_version, struct_size) =
            unsafe { ((*out_info).abi_version, (*out_info).struct_size) };
        if abi_version != ABI_VERSION || struct_size as usize != size_of::<OdDisplayFrameInfo>() {
            return STATUS_ABI_MISMATCH;
        }
        with_session(handle, |session| {
            let mode = session.display.current_mode();
            if !mode.is_active() {
                return STATUS_NO_ACTIVE_MODE;
            }
            if !session.display.has_current_frame() {
                return STATUS_NO_FRAME;
            }
            // The caller's buffer is not turned into a slice, and not touched,
            // unless its length equals the frame size exactly.
            if u64::try_from(buffer_len) != Ok(mode.frame_bytes()) {
                return STATUS_BUFFER_LENGTH;
            }
            // SAFETY: non-null, `buffer_len` equals the frame size and the
            // caller guarantees `buffer_len` writable bytes exclusive to this
            // call (checked non-overlapping with `out_info`).
            let output = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_len) };
            match session.display.read_frame_into(&session.bus, output) {
                Ok(info) => {
                    // SAFETY: pointer shape checked above.
                    unsafe {
                        out_info.write(OdDisplayFrameInfo {
                            abi_version: ABI_VERSION,
                            struct_size: size_of::<OdDisplayFrameInfo>() as u32,
                            width: info.mode.width,
                            height: info.mode.height,
                            stride: info.mode.stride,
                            format: info.mode.format,
                            reserved0: 0,
                            reserved1: 0,
                            generation: info.generation,
                            mode_change_count: info.mode_change_count,
                        });
                    }
                    STATUS_OK
                }
                Err(DisplayReadError::NoActiveMode) => STATUS_NO_ACTIVE_MODE,
                Err(DisplayReadError::NoFrame) => STATUS_NO_FRAME,
                Err(DisplayReadError::BufferLength { .. }) => STATUS_BUFFER_LENGTH,
                Err(DisplayReadError::ModeChanged) => STATUS_MODE_CHANGED,
                Err(DisplayReadError::Memory(_)) => STATUS_MEMORY_ERROR,
            }
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_step(
    handle: *mut OdDisplaySession,
    budget: u64,
    out_report: *mut OdDisplayStepReport,
) -> u32 {
    guarded(|| {
        if !valid_ptr(out_report) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; header fields are read before any write.
        let (abi_version, struct_size) =
            unsafe { ((*out_report).abi_version, (*out_report).struct_size) };
        if abi_version != ABI_VERSION || struct_size as usize != size_of::<OdDisplayStepReport>() {
            return STATUS_ABI_MISMATCH;
        }
        with_session(handle, |session| {
            // Profile 1 has no guest: any budget, including zero, executes nothing.
            let _ = budget;
            match session.profile {
                PROFILE_TEST_PATTERN => {
                    // SAFETY: pointer shape checked above.
                    unsafe {
                        out_report.write(OdDisplayStepReport {
                            abi_version: ABI_VERSION,
                            struct_size: size_of::<OdDisplayStepReport>() as u32,
                            outcome: STEP_NOTHING_TO_EXECUTE,
                            reserved: 0,
                            instructions_executed: 0,
                        });
                    }
                    STATUS_OK
                }
                _ => STATUS_INTERNAL_ERROR,
            }
        })
    })
}

fn cursor_state_struct(state: &od_machine::CursorState) -> OdDisplayCursorState {
    let mut flags = 0;
    if state.enabled {
        flags |= CURSOR_FLAG_ENABLED;
    }
    let mut out = OdDisplayCursorState {
        abi_version: ABI_VERSION_V2,
        struct_size: size_of::<OdDisplayCursorState>() as u32,
        cursor_seq: state.cursor_seq,
        shape_seq: 0,
        flags,
        width: 0,
        height: 0,
        format: 0,
        hot_x: 0,
        hot_y: 0,
        pos_x: state.x,
        pos_y: state.y,
        reserved0: 0,
        reserved1: 0,
    };
    if let Some(shape) = state.shape {
        out.flags |= CURSOR_FLAG_HAS_SHAPE;
        out.shape_seq = shape.shape_seq;
        out.width = shape.width;
        out.height = shape.height;
        out.format = shape.format;
        out.hot_x = shape.hot_x;
        out.hot_y = shape.hot_y;
    }
    out
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_cursor_state(
    handle: *mut OdDisplaySession,
    out_state: *mut OdDisplayCursorState,
) -> u32 {
    guarded(|| {
        if !valid_ptr(out_state) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; header fields are read before any write.
        let (abi_version, struct_size) =
            unsafe { ((*out_state).abi_version, (*out_state).struct_size) };
        if abi_version != ABI_VERSION_V2
            || struct_size as usize != size_of::<OdDisplayCursorState>()
        {
            return STATUS_ABI_MISMATCH;
        }
        with_session(handle, |session| {
            let out = cursor_state_struct(&session.display.cursor_state());
            // SAFETY: pointer shape checked above; success writes the whole struct.
            unsafe { out_state.write(out) };
            STATUS_OK
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn od_display_session_read_cursor_image(
    handle: *mut OdDisplaySession,
    buffer: *mut u8,
    buffer_len: usize,
    out_state: *mut OdDisplayCursorState,
) -> u32 {
    guarded(|| {
        if buffer.is_null() || !valid_ptr(out_state) {
            return STATUS_INVALID_ARGUMENT;
        }
        if overlaps(
            buffer,
            buffer_len,
            out_state.cast(),
            size_of::<OdDisplayCursorState>(),
        ) {
            return STATUS_INVALID_ARGUMENT;
        }
        // SAFETY: pointer shape checked; header fields are read before any write.
        let (abi_version, struct_size) =
            unsafe { ((*out_state).abi_version, (*out_state).struct_size) };
        if abi_version != ABI_VERSION_V2
            || struct_size as usize != size_of::<OdDisplayCursorState>()
        {
            return STATUS_ABI_MISMATCH;
        }
        with_session(handle, |session| {
            let state = session.display.cursor_state();
            let Some(shape) = state.shape else {
                return STATUS_NO_CURSOR_SHAPE;
            };
            // The caller's buffer is not turned into a slice, and not touched,
            // unless its length equals the image size exactly.
            let expected = u64::from(shape.width) * u64::from(shape.height) * 4;
            if u64::try_from(buffer_len) != Ok(expected) {
                return STATUS_CURSOR_BUFFER_LENGTH;
            }
            // SAFETY: non-null, `buffer_len` equals the image size and the
            // caller guarantees `buffer_len` writable bytes exclusive to this
            // call (checked non-overlapping with `out_state`).
            let output = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_len) };
            match session.display.read_cursor_image_into(output) {
                Ok(copied) => {
                    let mut out = cursor_state_struct(&state);
                    // The copy reports the shape it copied; no quantum ran
                    // between the two reads, so both describe one state.
                    out.shape_seq = copied.shape_seq;
                    out.width = copied.width;
                    out.height = copied.height;
                    out.format = copied.format;
                    out.hot_x = copied.hot_x;
                    out.hot_y = copied.hot_y;
                    // SAFETY: pointer shape checked above.
                    unsafe { out_state.write(out) };
                    STATUS_OK
                }
                Err(CursorReadError::NoShape) => STATUS_NO_CURSOR_SHAPE,
                Err(CursorReadError::BufferLength { .. }) => STATUS_CURSOR_BUFFER_LENGTH,
            }
        })
    })
}

/// Test-only access to the machine behind a handle, for the host tests that
/// stand in for the guest. This is not part of the ABI.
#[cfg(test)]
fn with_machine_for_test<R>(
    handle: *mut OdDisplaySession,
    body: impl FnOnce(&mut MachineBus, &DisplayHandle) -> R,
) -> R {
    let entry = lookup(handle).expect("live handle");
    let mut guard = entry.enter().expect("not busy");
    let session = guard.session();
    body(&mut session.bus, &session.display)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ptr::{null, null_mut};

    const WIDTH_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x040;
    const HEIGHT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x044;
    const FORMAT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x048;
    const MODE_COMMIT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x04C;
    const FRAME_GENERATION_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x080;

    fn config(profile: u32) -> OdDisplaySessionConfig {
        OdDisplaySessionConfig {
            abi_version: ABI_VERSION,
            struct_size: size_of::<OdDisplaySessionConfig>() as u32,
            profile,
            reserved: 0,
        }
    }

    fn mode_out() -> OdDisplayMode {
        OdDisplayMode {
            abi_version: ABI_VERSION,
            struct_size: size_of::<OdDisplayMode>() as u32,
            width: 0xAAAA_AAAA,
            height: 0xAAAA_AAAA,
            stride: 0xAAAA_AAAA,
            format: 0xAAAA_AAAA,
            flags: 0xAAAA_AAAA,
            reserved: 0xAAAA_AAAA,
            generation: u64::MAX,
            mode_change_count: u64::MAX,
        }
    }

    fn info_out() -> OdDisplayFrameInfo {
        OdDisplayFrameInfo {
            abi_version: ABI_VERSION,
            struct_size: size_of::<OdDisplayFrameInfo>() as u32,
            width: 0xAAAA_AAAA,
            height: 0xAAAA_AAAA,
            stride: 0xAAAA_AAAA,
            format: 0xAAAA_AAAA,
            reserved0: 0xAAAA_AAAA,
            reserved1: 0xAAAA_AAAA,
            generation: u64::MAX,
            mode_change_count: u64::MAX,
        }
    }

    fn report_out() -> OdDisplayStepReport {
        OdDisplayStepReport {
            abi_version: ABI_VERSION,
            struct_size: size_of::<OdDisplayStepReport>() as u32,
            outcome: 0xAAAA_AAAA,
            reserved: 0xAAAA_AAAA,
            instructions_executed: u64::MAX,
        }
    }

    fn create() -> *mut OdDisplaySession {
        let mut handle = null_mut();
        let status = unsafe { od_display_session_create(&config(1), &mut handle) };
        assert_eq!(status, STATUS_OK);
        assert!(!handle.is_null());
        handle
    }

    fn put(bus: &mut MachineBus, address: u64, width: AccessWidth, value: u64) {
        bus.write_physical(GuestPhysicalAddress::new(address), width, value, |_| {})
            .unwrap();
    }

    fn commit_mode(bus: &mut MachineBus, width: u32, height: u32) {
        put(bus, WIDTH_REGISTER, AccessWidth::Dword, u64::from(width));
        put(bus, HEIGHT_REGISTER, AccessWidth::Dword, u64::from(height));
        put(
            bus,
            FORMAT_REGISTER,
            AccessWidth::Dword,
            u64::from(DISPLAY_FORMAT_BGRX8888),
        );
        put(bus, MODE_COMMIT_REGISTER, AccessWidth::Dword, 1);
    }

    fn pattern_pixel(x: u32, y: u32) -> [u8; 4] {
        [(x * 4) as u8, (y * 8) as u8, (x ^ y) as u8, 0xA5]
    }

    fn write_pattern(bus: &mut MachineBus, width: u32, height: u32) {
        for y in 0..height {
            for x in 0..width {
                let address = DISPLAY_FRAMEBUFFER_BASE + u64::from(y * width * 4 + x * 4);
                let value = u32::from_le_bytes(pattern_pixel(x, y));
                put(bus, address, AccessWidth::Dword, u64::from(value));
            }
        }
    }

    fn expected_pattern(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&pattern_pixel(x, y));
            }
        }
        bytes
    }

    fn read_frame(
        handle: *mut OdDisplaySession,
        buffer: &mut [u8],
        info: &mut OdDisplayFrameInfo,
    ) -> u32 {
        unsafe { od_display_session_read_frame(handle, buffer.as_mut_ptr(), buffer.len(), info) }
    }

    const FIXTURE_BARS_64X48: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage10/bars_64x48_seed0.raw");

    #[test]
    fn test_pattern_profile_draws_the_bars_reference_frame_at_create() {
        assert_eq!(FIXTURE_BARS_64X48.len(), 64 * 48 * 4);
        let handle = create();

        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_OK
        );
        assert_eq!(
            (mode.width, mode.height, mode.stride, mode.format),
            (64, 48, 256, DISPLAY_FORMAT_BGRX8888)
        );
        assert_eq!(mode.flags, MODE_FLAG_ACTIVE | MODE_FLAG_HAS_FRAME);
        assert_eq!((mode.generation, mode.mode_change_count), (1, 1));
        assert_eq!(mode.reserved, 0);

        let mut buffer = vec![0xEE; FIXTURE_BARS_64X48.len()];
        let mut info = info_out();
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_OK);
        assert_eq!(buffer, FIXTURE_BARS_64X48);
        assert_eq!(
            (info.width, info.height, info.stride, info.format),
            (64, 48, 256, DISPLAY_FORMAT_BGRX8888)
        );
        assert_eq!((info.reserved0, info.reserved1), (0, 0));
        assert_eq!((info.generation, info.mode_change_count), (1, 1));

        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn pattern_written_to_device_memory_is_read_back_byte_for_byte() {
        const W: u32 = 64;
        const H: u32 = 32;
        let handle = create();

        with_machine_for_test(handle, |bus, _| {
            commit_mode(bus, W, H);
            write_pattern(bus, W, H);
            put(bus, FRAME_GENERATION_REGISTER, AccessWidth::Dword, 1);
        });

        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_OK
        );
        assert_eq!(
            (mode.width, mode.height, mode.stride, mode.format),
            (W, H, W * 4, DISPLAY_FORMAT_BGRX8888)
        );
        assert_eq!(mode.flags, MODE_FLAG_ACTIVE | MODE_FLAG_HAS_FRAME);
        assert_eq!((mode.generation, mode.mode_change_count), (2, 2));

        let mut buffer = vec![0xEE; (W * H * 4) as usize];
        let mut info = info_out();
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_OK);
        assert_eq!(buffer, expected_pattern(W, H));
        assert_eq!((info.generation, info.mode_change_count), (2, 2));

        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn read_before_mode_and_before_frame_report_distinct_statuses() {
        let handle = create();
        let mut buffer = vec![0xEE; 16];
        let mut info = info_out();
        // Profile 1 starts with a mode and a frame; a device reset removes both.
        with_machine_for_test(handle, |_, display| display.reset());
        assert_eq!(
            read_frame(handle, &mut buffer, &mut info),
            STATUS_NO_ACTIVE_MODE
        );
        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_OK
        );
        assert_eq!((mode.width, mode.flags), (0, 0));
        with_machine_for_test(handle, |bus, _| commit_mode(bus, 2, 2));
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_NO_FRAME);
        assert!(buffer.iter().all(|byte| *byte == 0xEE));
        assert_eq!(info.generation, u64::MAX);

        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_OK
        );
        assert_eq!(mode.flags, MODE_FLAG_ACTIVE);
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn wrong_buffer_length_is_rejected_without_touching_buffer_or_info() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| {
            commit_mode(bus, 4, 2);
            write_pattern(bus, 4, 2);
            put(bus, FRAME_GENERATION_REGISTER, AccessWidth::Dword, 1);
        });
        let frame_bytes = 4 * 2 * 4;
        for wrong in [1, frame_bytes - 1, frame_bytes + 1, frame_bytes * 2] {
            let mut buffer = vec![0xEE; wrong];
            let mut info = info_out();
            assert_eq!(
                read_frame(handle, &mut buffer, &mut info),
                STATUS_BUFFER_LENGTH,
                "length {wrong}"
            );
            assert!(buffer.iter().all(|byte| *byte == 0xEE));
            assert_eq!(info.generation, u64::MAX);
        }
        let mut info = info_out();
        assert_eq!(
            unsafe { od_display_session_read_frame(handle, [0u8; 1].as_mut_ptr(), 0, &mut info) },
            STATUS_BUFFER_LENGTH
        );
        let mut buffer = vec![0u8; frame_bytes];
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_OK);
        assert_eq!(buffer, expected_pattern(4, 2));
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn mode_change_after_a_frame_requires_a_new_frame() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| {
            commit_mode(bus, 2, 2);
            put(bus, FRAME_GENERATION_REGISTER, AccessWidth::Dword, 1);
            commit_mode(bus, 4, 1);
        });
        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_OK
        );
        // Initial built-in mode is change 1, then 2x2, then 4x1.
        assert_eq!((mode.width, mode.height, mode.mode_change_count), (4, 1, 3));
        assert_eq!(mode.generation, 2);
        let mut buffer = vec![0u8; 16];
        let mut info = info_out();
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_NO_FRAME);
        with_machine_for_test(handle, |bus, _| {
            put(bus, FRAME_GENERATION_REGISTER, AccessWidth::Dword, 1);
        });
        assert_eq!(read_frame(handle, &mut buffer, &mut info), STATUS_OK);
        assert_eq!((info.generation, info.mode_change_count), (3, 3));
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn step_has_nothing_to_execute_for_the_test_pattern_profile() {
        let handle = create();
        for budget in [0, 1, u64::MAX] {
            let mut report = report_out();
            assert_eq!(
                unsafe { od_display_session_step(handle, budget, &mut report) },
                STATUS_OK
            );
            assert_eq!(report.outcome, STEP_NOTHING_TO_EXECUTE);
            assert_eq!(report.instructions_executed, 0);
            assert_eq!(report.reserved, 0);
            assert_eq!(report.abi_version, ABI_VERSION);
        }
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn destroying_twice_and_using_a_destroyed_handle_are_rejected() {
        let handle = create();
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
        assert_eq!(
            unsafe { od_display_session_destroy(handle) },
            STATUS_INVALID_HANDLE
        );
        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_INVALID_HANDLE
        );
        assert_eq!(mode.width, 0xAAAA_AAAA);
        let mut buffer = [0u8; 4];
        let mut info = info_out();
        assert_eq!(
            read_frame(handle, &mut buffer, &mut info),
            STATUS_INVALID_HANDLE
        );
        let mut report = report_out();
        assert_eq!(
            unsafe { od_display_session_step(handle, 1, &mut report) },
            STATUS_INVALID_HANDLE
        );
        // A later session never receives the destroyed handle's identity.
        let next = create();
        assert_ne!(next, handle);
        assert_eq!(
            unsafe { od_display_session_destroy(handle) },
            STATUS_INVALID_HANDLE
        );
        assert_eq!(unsafe { od_display_session_destroy(next) }, STATUS_OK);
    }

    #[test]
    fn null_and_unknown_handles_are_rejected() {
        let mut mode = mode_out();
        let mut info = info_out();
        let mut report = report_out();
        let mut buffer = [0u8; 4];
        assert_eq!(
            unsafe { od_display_session_destroy(null_mut()) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { od_display_session_mode(null_mut(), &mut mode) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            read_frame(null_mut(), &mut buffer, &mut info),
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { od_display_session_step(null_mut(), 1, &mut report) },
            STATUS_INVALID_ARGUMENT
        );
        let forged = std::ptr::without_provenance_mut::<OdDisplaySession>(0x1000);
        assert_eq!(
            unsafe { od_display_session_destroy(forged) },
            STATUS_INVALID_HANDLE
        );
        assert_eq!(mode.width, 0xAAAA_AAAA);
    }

    #[test]
    fn create_validates_arguments_and_leaves_the_handle_slot_on_failure() {
        let mut handle = null_mut();
        assert_eq!(
            unsafe { od_display_session_create(null(), &mut handle) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { od_display_session_create(&config(1), null_mut()) },
            STATUS_INVALID_ARGUMENT
        );
        for profile in [0, 2, 99, u32::MAX] {
            assert_eq!(
                unsafe { od_display_session_create(&config(profile), &mut handle) },
                STATUS_UNSUPPORTED_PROFILE,
                "profile {profile}"
            );
            assert!(handle.is_null());
        }
        let mut wrong_version = config(1);
        wrong_version.abi_version = ABI_VERSION + 1;
        assert_eq!(
            unsafe { od_display_session_create(&wrong_version, &mut handle) },
            STATUS_ABI_MISMATCH
        );
        let mut wrong_size = config(1);
        wrong_size.struct_size += 4;
        assert_eq!(
            unsafe { od_display_session_create(&wrong_size, &mut handle) },
            STATUS_ABI_MISMATCH
        );
        let mut reserved = config(1);
        reserved.reserved = 1;
        assert_eq!(
            unsafe { od_display_session_create(&reserved, &mut handle) },
            STATUS_INVALID_ARGUMENT
        );
        assert!(handle.is_null());

        let live = create();
        let mut occupied = live;
        assert_eq!(
            unsafe { od_display_session_create(&config(1), &mut occupied) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(occupied, live);
        assert_eq!(unsafe { od_display_session_destroy(live) }, STATUS_OK);
    }

    #[test]
    fn output_structs_with_wrong_abi_header_are_rejected_untouched() {
        let handle = create();
        let mut mode = mode_out();
        mode.abi_version = 0;
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_ABI_MISMATCH
        );
        assert_eq!(mode.width, 0xAAAA_AAAA);
        let mut info = info_out();
        info.struct_size = 0;
        let mut buffer = [0u8; 4];
        assert_eq!(
            read_frame(handle, &mut buffer, &mut info),
            STATUS_ABI_MISMATCH
        );
        let mut report = report_out();
        report.struct_size += 8;
        assert_eq!(
            unsafe { od_display_session_step(handle, 1, &mut report) },
            STATUS_ABI_MISMATCH
        );
        assert_eq!(report.outcome, 0xAAAA_AAAA);
        let mut good = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut good) },
            STATUS_OK
        );
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn overlapping_use_of_one_handle_is_busy_not_undefined() {
        let handle = create();
        let entry = lookup(handle).unwrap();
        let guard = entry.enter().unwrap();
        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_BUSY
        );
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_BUSY);
        drop(guard);
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn panic_inside_a_call_becomes_internal_error_and_fails_the_session() {
        let handle = create();
        let status = with_session(handle, |_| panic!("test panic"));
        assert_eq!(status, STATUS_INTERNAL_ERROR);
        let mut mode = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode) },
            STATUS_INTERNAL_ERROR
        );
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    const CURSOR_WIDTH_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x210;
    const CURSOR_HEIGHT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x214;
    const CURSOR_FORMAT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x218;
    const CURSOR_HOTSPOT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x21C;
    const CURSOR_IMAGE_INDEX_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x220;
    const CURSOR_IMAGE_DATA_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x224;
    const CURSOR_IMAGE_COMMIT_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x228;
    const CURSOR_POSITION_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x230;
    const CURSOR_ENABLE_REGISTER: u64 = DISPLAY_REGISTERS_BASE + 0x234;

    fn cursor_out() -> OdDisplayCursorState {
        OdDisplayCursorState {
            abi_version: ABI_VERSION_V2,
            struct_size: size_of::<OdDisplayCursorState>() as u32,
            cursor_seq: u64::MAX,
            shape_seq: u64::MAX,
            flags: 0xAAAA_AAAA,
            width: 0xAAAA_AAAA,
            height: 0xAAAA_AAAA,
            format: 0xAAAA_AAAA,
            hot_x: 0xAAAA_AAAA,
            hot_y: 0xAAAA_AAAA,
            pos_x: 0xAAAA_AAAA,
            pos_y: 0xAAAA_AAAA,
            reserved0: 0xAAAA_AAAA,
            reserved1: 0xAAAA_AAAA,
        }
    }

    fn cursor_pixel(x: u32, y: u32) -> [u8; 4] {
        // Premultiplied BGRA with a mix of alpha 0, 255 and a middle value.
        let a = [0u8, 255, 128][((x + y) % 3) as usize];
        [(x * 16) as u8 & a, (y * 16) as u8 & a, 0x40 & a, a]
    }

    fn expected_cursor(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        for y in 0..height {
            for x in 0..width {
                bytes.extend_from_slice(&cursor_pixel(x, y));
            }
        }
        bytes
    }

    /// What a guest driver does: shape through the registers, then position
    /// and visibility. No frame register is touched.
    fn guest_sets_cursor(bus: &mut MachineBus, width: u32, height: u32, hot: (u32, u32)) {
        put(
            bus,
            CURSOR_WIDTH_REGISTER,
            AccessWidth::Dword,
            u64::from(width),
        );
        put(
            bus,
            CURSOR_HEIGHT_REGISTER,
            AccessWidth::Dword,
            u64::from(height),
        );
        put(bus, CURSOR_FORMAT_REGISTER, AccessWidth::Dword, 2);
        put(
            bus,
            CURSOR_HOTSPOT_REGISTER,
            AccessWidth::Dword,
            u64::from(hot.0 | (hot.1 << 16)),
        );
        put(bus, CURSOR_IMAGE_INDEX_REGISTER, AccessWidth::Dword, 0);
        for y in 0..height {
            for x in 0..width {
                let word = u32::from_le_bytes(cursor_pixel(x, y));
                put(
                    bus,
                    CURSOR_IMAGE_DATA_REGISTER,
                    AccessWidth::Dword,
                    u64::from(word),
                );
            }
        }
        put(bus, CURSOR_IMAGE_COMMIT_REGISTER, AccessWidth::Dword, 1);
    }

    fn cursor_state_of(handle: *mut OdDisplaySession) -> OdDisplayCursorState {
        let mut state = cursor_out();
        assert_eq!(
            unsafe { od_display_session_cursor_state(handle, &mut state) },
            STATUS_OK
        );
        state
    }

    fn read_cursor(
        handle: *mut OdDisplaySession,
        buffer: &mut [u8],
        state: &mut OdDisplayCursorState,
    ) -> u32 {
        unsafe {
            od_display_session_read_cursor_image(handle, buffer.as_mut_ptr(), buffer.len(), state)
        }
    }

    #[test]
    fn cursor_state_is_empty_after_create_and_reads_back_a_guest_shape() {
        let handle = create();
        let state = cursor_state_of(handle);
        assert_eq!(state.cursor_seq, 0);
        assert_eq!((state.flags, state.shape_seq), (0, 0));
        assert_eq!(
            (
                state.width,
                state.height,
                state.format,
                state.hot_x,
                state.hot_y
            ),
            (0, 0, 0, 0, 0)
        );
        assert_eq!((state.pos_x, state.pos_y), (0, 0));
        assert_eq!((state.reserved0, state.reserved1), (0, 0));
        let mut buffer = vec![0xEE; 4];
        let mut state = cursor_out();
        assert_eq!(
            read_cursor(handle, &mut buffer, &mut state),
            STATUS_NO_CURSOR_SHAPE
        );
        assert_eq!(buffer, [0xEE; 4]);
        assert_eq!(state.cursor_seq, u64::MAX);

        with_machine_for_test(handle, |bus, _| {
            guest_sets_cursor(bus, 16, 16, (1, 2));
            put(
                bus,
                CURSOR_POSITION_REGISTER,
                AccessWidth::Dword,
                20 | (12 << 16),
            );
            put(bus, CURSOR_ENABLE_REGISTER, AccessWidth::Dword, 1);
        });
        let state = cursor_state_of(handle);
        assert_eq!(state.cursor_seq, 3);
        assert_eq!(state.flags, CURSOR_FLAG_ENABLED | CURSOR_FLAG_HAS_SHAPE);
        assert_eq!(state.shape_seq, 1);
        assert_eq!((state.width, state.height, state.format), (16, 16, 2));
        assert_eq!((state.hot_x, state.hot_y), (1, 2));
        assert_eq!((state.pos_x, state.pos_y), (20, 12));
        assert_eq!(state.abi_version, ABI_VERSION_V2);
        assert_eq!(
            state.struct_size as usize,
            size_of::<OdDisplayCursorState>()
        );

        let mut buffer = vec![0xEE; 16 * 16 * 4];
        let mut copied = cursor_out();
        assert_eq!(read_cursor(handle, &mut buffer, &mut copied), STATUS_OK);
        assert_eq!(buffer, expected_cursor(16, 16));
        assert_eq!(copied.shape_seq, state.shape_seq);
        assert_eq!(copied.cursor_seq, state.cursor_seq);
        assert_eq!(copied.flags, CURSOR_FLAG_ENABLED | CURSOR_FLAG_HAS_SHAPE);
        assert_eq!((copied.width, copied.height, copied.format), (16, 16, 2));
        // Read-only (ABI section 9): the state read after the image copy is the same, nothing counted.
        let again = cursor_state_of(handle);
        assert_eq!(
            (
                again.cursor_seq,
                again.shape_seq,
                again.flags,
                again.pos_x,
                again.pos_y
            ),
            (state.cursor_seq, state.shape_seq, state.flags, 20, 12)
        );
        assert_eq!(cursor_state_of(handle).cursor_seq, state.cursor_seq);
        assert_eq!((copied.pos_x, copied.pos_y), (20, 12));
        assert_eq!((copied.hot_x, copied.hot_y), (1, 2));
        assert_eq!((copied.reserved0, copied.reserved1), (0, 0));
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn largest_cursor_image_fits_one_16_kib_slot() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| guest_sets_cursor(bus, 64, 64, (63, 63)));
        let state = cursor_state_of(handle);
        assert_eq!((state.width, state.height), (64, 64));
        let mut buffer = vec![0u8; 16384];
        let mut copied = cursor_out();
        assert_eq!(read_cursor(handle, &mut buffer, &mut copied), STATUS_OK);
        assert_eq!(buffer, expected_cursor(64, 64));
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn cursor_move_changes_cursor_seq_only_and_not_the_frame() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| {
            guest_sets_cursor(bus, 4, 4, (0, 0));
            put(bus, CURSOR_POSITION_REGISTER, AccessWidth::Dword, 5);
        });
        let before = cursor_state_of(handle);
        let mut mode_before = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode_before) },
            STATUS_OK
        );
        with_machine_for_test(handle, |bus, _| {
            put(
                bus,
                CURSOR_POSITION_REGISTER,
                AccessWidth::Dword,
                6 | (7 << 16),
            );
        });
        let after = cursor_state_of(handle);
        assert!(after.cursor_seq > before.cursor_seq);
        assert_eq!(after.shape_seq, before.shape_seq);
        assert_eq!((after.pos_x, after.pos_y), (6, 7));
        let mut mode_after = mode_out();
        assert_eq!(
            unsafe { od_display_session_mode(handle, &mut mode_after) },
            STATUS_OK
        );
        assert_eq!(mode_after.generation, mode_before.generation);
        assert_eq!(mode_after.mode_change_count, mode_before.mode_change_count);
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn wrong_cursor_buffer_length_is_rejected_without_touching_buffer_or_state() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| guest_sets_cursor(bus, 4, 2, (0, 0)));
        let size = 4 * 2 * 4;
        for wrong in [1, size - 1, size + 1, size * 2] {
            let mut buffer = vec![0xEE; wrong];
            let mut state = cursor_out();
            assert_eq!(
                read_cursor(handle, &mut buffer, &mut state),
                STATUS_CURSOR_BUFFER_LENGTH,
                "length {wrong}"
            );
            assert!(buffer.iter().all(|byte| *byte == 0xEE));
            assert_eq!(state.cursor_seq, u64::MAX);
        }
        let mut buffer = vec![0xEE; size];
        let mut state = cursor_out();
        assert_eq!(read_cursor(handle, &mut buffer, &mut state), STATUS_OK);
        assert_eq!(buffer, expected_cursor(4, 2));
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn device_reset_removes_the_cursor_shape() {
        let handle = create();
        with_machine_for_test(handle, |bus, _| {
            guest_sets_cursor(bus, 4, 4, (1, 1));
            put(
                bus,
                CURSOR_POSITION_REGISTER,
                AccessWidth::Dword,
                3 | (3 << 16),
            );
            put(bus, CURSOR_ENABLE_REGISTER, AccessWidth::Dword, 1);
        });
        let before = cursor_state_of(handle);
        assert_eq!(before.flags, CURSOR_FLAG_ENABLED | CURSOR_FLAG_HAS_SHAPE);
        with_machine_for_test(handle, |_, display| display.reset());
        let after = cursor_state_of(handle);
        assert_eq!(after.flags, 0);
        assert_eq!(after.shape_seq, 0);
        assert_eq!(after.cursor_seq, before.cursor_seq + 1);
        assert_eq!((after.pos_x, after.pos_y), (0, 0));
        let mut buffer = vec![0xEE; 64];
        let mut state = cursor_out();
        assert_eq!(
            read_cursor(handle, &mut buffer, &mut state),
            STATUS_NO_CURSOR_SHAPE
        );
        assert!(buffer.iter().all(|byte| *byte == 0xEE));
        assert_eq!(state.cursor_seq, u64::MAX);
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
    }

    #[test]
    fn cursor_calls_validate_arguments_header_and_handle() {
        let handle = create();
        let mut state = cursor_out();
        let mut buffer = vec![0xEE; 64];
        assert_eq!(
            unsafe { od_display_session_cursor_state(handle, null_mut()) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { od_display_session_cursor_state(null_mut(), &mut state) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe { od_display_session_cursor_state(id_to_handle(0x7777), &mut state) },
            STATUS_INVALID_HANDLE
        );
        for (version, size) in [
            (ABI_VERSION, size_of::<OdDisplayCursorState>() as u32),
            (ABI_VERSION_V2, 48),
            (0, 0),
        ] {
            let mut bad = cursor_out();
            bad.abi_version = version;
            bad.struct_size = size;
            assert_eq!(
                unsafe { od_display_session_cursor_state(handle, &mut bad) },
                STATUS_ABI_MISMATCH
            );
            assert_eq!(bad.cursor_seq, u64::MAX);
            assert_eq!(
                read_cursor(handle, &mut buffer, &mut bad),
                STATUS_ABI_MISMATCH
            );
        }
        assert_eq!(
            unsafe { od_display_session_read_cursor_image(handle, null_mut(), 0, &mut state) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe {
                od_display_session_read_cursor_image(
                    handle,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    null_mut(),
                )
            },
            STATUS_INVALID_ARGUMENT
        );
        // Misaligned output struct: INVALID_ARGUMENT before anything is touched.
        let mut raw = vec![0u64; 16];
        let misaligned =
            unsafe { raw.as_mut_ptr().cast::<u8>().add(1) }.cast::<OdDisplayCursorState>();
        assert_eq!(
            unsafe { od_display_session_cursor_state(handle, misaligned) },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe {
                od_display_session_read_cursor_image(
                    handle,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    misaligned,
                )
            },
            STATUS_INVALID_ARGUMENT
        );
        assert!(buffer.iter().all(|byte| *byte == 0xEE));
        // Buffer overlapping the output struct.
        let mut overlap = vec![0u8; 128];
        let state_ptr = overlap.as_mut_ptr().cast::<OdDisplayCursorState>();
        assert_eq!(overlap.as_ptr().align_offset(8), 0);
        unsafe { state_ptr.write(cursor_out()) };
        assert_eq!(
            unsafe {
                od_display_session_read_cursor_image(
                    handle,
                    overlap.as_mut_ptr(),
                    overlap.len(),
                    state_ptr,
                )
            },
            STATUS_INVALID_ARGUMENT
        );
        assert_eq!(
            read_cursor(id_to_handle(0x7777), &mut buffer, &mut state),
            STATUS_INVALID_HANDLE
        );
        let entry = lookup(handle).unwrap();
        let guard = entry.enter().unwrap();
        assert_eq!(
            unsafe { od_display_session_cursor_state(handle, &mut state) },
            STATUS_BUSY
        );
        assert_eq!(read_cursor(handle, &mut buffer, &mut state), STATUS_BUSY);
        drop(guard);
        assert_eq!(unsafe { od_display_session_destroy(handle) }, STATUS_OK);
        assert_eq!(
            unsafe { od_display_session_cursor_state(handle, &mut state) },
            STATUS_INVALID_HANDLE
        );
        assert_eq!(state.cursor_seq, u64::MAX);
    }
}
