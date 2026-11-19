//! UEFI Graphics Output Protocol and primary display handle, UEFI 2.11 §12.9.

use core::{
    cell::{Cell, UnsafeCell},
    ffi::c_void,
    mem::size_of,
    ptr::{self, NonNull},
    sync::atomic::{AtomicPtr, Ordering},
};

use crate::{
    device_path::{DEVICE_PATH_PROTOCOL_GUID, END_PATH_BYTES},
    handoff::Handoff,
    mem::map::MemoryType,
    uefi::{guid::Guid, status::Status, system_table::SystemTable, types::Handle},
};

use super::{
    blt,
    device::{Display, Geometry, PIXEL_BYTES, REGISTERS_BASE, REGISTERS_BYTES},
};

pub const GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x9042_a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};
pub const EDID_DISCOVERED_PROTOCOL_GUID: Guid = Guid {
    data1: 0x1c0c_34f6,
    data2: 0xd380,
    data3: 0x41fa,
    data4: [0xa0, 0x49, 0x8a, 0xd0, 0x6c, 0x1a, 0x66, 0xaa],
};
pub const EDID_ACTIVE_PROTOCOL_GUID: Guid = Guid {
    data1: 0xbd8c_1056,
    data2: 0x9f36,
    data3: 0x44ec,
    data4: [0x92, 0xa8, 0xa6, 0x33, 0x7f, 0x81, 0x79, 0x86],
};

pub const PIXEL_BLUE_GREEN_RED_RESERVED_8BIT_PER_COLOR: u32 = 1;
pub const BLT_VIDEO_FILL: u32 = blt::VIDEO_FILL;
pub const BLT_VIDEO_TO_BUFFER: u32 = blt::VIDEO_TO_BUFFER;
pub const BLT_BUFFER_TO_VIDEO: u32 = blt::BUFFER_TO_VIDEO;
pub const BLT_VIDEO_TO_VIDEO: u32 = blt::VIDEO_TO_VIDEO;

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeInformation {
    pub version: u32,
    pub horizontal_resolution: u32,
    pub vertical_resolution: u32,
    pub pixel_format: u32,
    pub pixel_information: [u32; 4],
    pub pixels_per_scan_line: u32,
}

impl ModeInformation {
    fn from_geometry(mode: Geometry) -> Self {
        Self {
            version: 0,
            horizontal_resolution: mode.width,
            vertical_resolution: mode.height,
            pixel_format: PIXEL_BLUE_GREEN_RED_RESERVED_8BIT_PER_COLOR,
            pixel_information: [0; 4],
            pixels_per_scan_line: mode.stride / PIXEL_BYTES as u32,
        }
    }
}

#[repr(C)]
pub struct Mode {
    pub max_mode: u32,
    pub mode: u32,
    pub info: *const ModeInformation,
    pub size_of_info: usize,
    pub frame_buffer_base: u64,
    pub frame_buffer_size: usize,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BltPixel {
    pub blue: u8,
    pub green: u8,
    pub red: u8,
    pub reserved: u8,
}

#[repr(C)]
pub struct EdidProtocol {
    pub size_of_edid: u32,
    pub edid: *const u8,
}

pub type QueryMode = unsafe extern "efiapi" fn(
    *mut GraphicsOutputProtocol,
    u32,
    *mut usize,
    *mut *mut ModeInformation,
) -> Status;
pub type SetMode = unsafe extern "efiapi" fn(*mut GraphicsOutputProtocol, u32) -> Status;
pub type Blt = unsafe extern "efiapi" fn(
    *mut GraphicsOutputProtocol,
    *mut BltPixel,
    u32,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
    usize,
) -> Status;

#[repr(C)]
pub struct GraphicsOutputProtocol {
    pub query_mode: QueryMode,
    pub set_mode: SetMode,
    pub blt: Blt,
    pub mode: *const Mode,
}

#[repr(C)]
struct State {
    protocol: GraphicsOutputProtocol,
    mode: UnsafeCell<Mode>,
    info: UnsafeCell<ModeInformation>,
    edid: EdidProtocol,
    path: [u8; 28],
    display: Display,
    geometry: Cell<Geometry>,
}

static STATE: AtomicPtr<State> = AtomicPtr::new(ptr::null_mut());

impl State {
    fn framebuffer(&self) -> *mut u8 {
        ptr::with_exposed_provenance_mut(self.display.framebuffer as usize)
    }

    fn activate(&self, number: u32, geometry: Geometry) -> Result<(), Status> {
        self.display.commit_mode(geometry)?;
        // SAFETY: the display adopted this fully validated geometry.
        unsafe { blt::clear(self.framebuffer(), geometry.bytes) };
        self.geometry.set(geometry);
        // SAFETY: the BSP owns mode changes. Clients read Mode and Info between calls.
        unsafe {
            self.info
                .get()
                .write(ModeInformation::from_geometry(geometry));
            (*self.mode.get()).mode = number;
            (*self.mode.get()).frame_buffer_size = geometry.bytes;
        }
        self.display.publish_frame()
    }
}

unsafe fn state_for(this: *mut GraphicsOutputProtocol) -> Result<NonNull<State>, Status> {
    let state = STATE.load(Ordering::Acquire);
    if this.is_null() || state.cast() != this {
        return Err(Status::INVALID_PARAMETER);
    }
    // Timer notifications can call GOP again. Poll before borrowing protocol state.
    #[cfg(target_os = "none")]
    unsafe {
        crate::bs::event::poll_due_events()
    };
    // SAFETY: EFI callbacks execute on the BSP. After EBS the pool may be reclaimed.
    if !unsafe { crate::bs::boot_services_active() } {
        return Err(Status::NOT_READY);
    }
    Ok(NonNull::new(state).unwrap())
}

unsafe extern "efiapi" fn query_mode(
    this: *mut GraphicsOutputProtocol,
    number: u32,
    size: *mut usize,
    info: *mut *mut ModeInformation,
) -> Status {
    if size.is_null() || info.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let state = match unsafe { state_for(this) } {
        Ok(state) => state,
        Err(status) => return status,
    };
    let display = unsafe { &state.as_ref().display };
    if number >= display.mode_count {
        return Status::INVALID_PARAMETER;
    }
    let mode = match display.mode(number) {
        Ok(mode) => mode,
        Err(status) => return status,
    };
    let mut buffer = ptr::null_mut();
    let status = unsafe {
        crate::bs::memory::allocate_pool(
            MemoryType::BOOT_SERVICES_DATA,
            size_of::<ModeInformation>(),
            &mut buffer,
        )
    };
    if status == Status::SUCCESS {
        let buffer = buffer.cast::<ModeInformation>();
        // SAFETY: AllocatePool succeeded, and the caller owns the output cells.
        unsafe {
            buffer.write(ModeInformation::from_geometry(mode));
            size.write(size_of::<ModeInformation>());
            info.write(buffer);
        }
    }
    status
}

unsafe extern "efiapi" fn set_mode(this: *mut GraphicsOutputProtocol, number: u32) -> Status {
    let result = (|| {
        let state = unsafe { state_for(this)?.as_ref() };
        state.activate(number, state.display.mode(number)?)
    })();
    result.err().unwrap_or(Status::SUCCESS)
}

unsafe extern "efiapi" fn blt(
    this: *mut GraphicsOutputProtocol,
    buffer: *mut BltPixel,
    operation: u32,
    source_x: usize,
    source_y: usize,
    destination_x: usize,
    destination_y: usize,
    width: usize,
    height: usize,
    delta: usize,
) -> Status {
    let result = (|| {
        let state = unsafe { state_for(this) }?;
        let (base, geometry) = {
            let state = unsafe { state.as_ref() };
            (state.framebuffer(), state.geometry.get())
        };
        let request = blt::Request {
            buffer: buffer.cast(),
            operation,
            source_x,
            source_y,
            destination_x,
            destination_y,
            width,
            height,
            delta,
        };
        if unsafe { request.execute(base, geometry) }? {
            unsafe { state.as_ref().display.publish_frame() }?;
        }
        Ok(())
    })();
    result.err().unwrap_or(Status::SUCCESS)
}

fn display_path() -> [u8; 28] {
    // Hardware, MemoryMapped, 24-byte node, inclusive register addresses.
    let mut path = [0; 28];
    path[..4].copy_from_slice(&[1, 3, 24, 0]);
    path[4..8].copy_from_slice(&MemoryType::MEMORY_MAPPED_IO.raw().to_le_bytes());
    path[8..16].copy_from_slice(&REGISTERS_BASE.to_le_bytes());
    path[16..24].copy_from_slice(&(REGISTERS_BASE + REGISTERS_BYTES - 1).to_le_bytes());
    path[24..].copy_from_slice(&END_PATH_BYTES);
    path
}

/// Install one GOP handle for the primary display and initialize mode zero.
///
/// # Safety
/// The BSP owns the live Boot Services tables, and `handoff` describes the active identity map.
pub unsafe fn install(handoff: &Handoff<'_>, system: *mut SystemTable) -> Status {
    let result = (|| {
        let Some(display) = (unsafe { Display::discover(handoff) })? else {
            return Ok(());
        };
        if !STATE.load(Ordering::Acquire).is_null() {
            return Err(Status::ALREADY_STARTED);
        }
        let geometry = display.mode(0)?;
        let boot = unsafe { &*(*system).boot_services };
        let publish = boot
            .install_multiple_protocol_interfaces
            .ok_or(Status::UNSUPPORTED)?;
        let mut memory = ptr::null_mut::<c_void>();
        let status = unsafe {
            (boot.allocate_pool)(
                MemoryType::BOOT_SERVICES_DATA,
                size_of::<State>(),
                &mut memory,
            )
        };
        if status != Status::SUCCESS {
            return Err(status);
        }
        let state = memory.cast::<State>();
        let info = ModeInformation::from_geometry(geometry);
        // SAFETY: the pool allocation owns the entire State. Interior pointers are initialized
        // at its final address before publication and remain live through Boot Services.
        unsafe {
            state.write(State {
                protocol: GraphicsOutputProtocol {
                    query_mode,
                    set_mode,
                    blt,
                    mode: ptr::null(),
                },
                mode: UnsafeCell::new(Mode {
                    max_mode: display.mode_count,
                    mode: 0,
                    info: ptr::null(),
                    size_of_info: size_of::<ModeInformation>(),
                    frame_buffer_base: display.framebuffer,
                    frame_buffer_size: geometry.bytes,
                }),
                info: UnsafeCell::new(info),
                edid: EdidProtocol {
                    size_of_edid: 0,
                    edid: ptr::null(),
                },
                path: display_path(),
                geometry: Cell::new(geometry),
                display,
            });
            let mode = UnsafeCell::raw_get(ptr::addr_of!((*state).mode));
            (*state).protocol.mode = mode;
            (*mode).info = UnsafeCell::raw_get(ptr::addr_of!((*state).info));
            if let Err(error) = (*state).activate(0, geometry) {
                (boot.free_pool)(memory);
                return Err(error);
            }
            STATE.store(state, Ordering::Release);
            let mut handle = ptr::null_mut::<c_void>() as Handle;
            let edid = ptr::addr_of!((*state).edid).cast::<c_void>();
            let status = publish(
                &mut handle,
                &GRAPHICS_OUTPUT_PROTOCOL_GUID as *const Guid,
                state.cast::<c_void>(),
                &DEVICE_PATH_PROTOCOL_GUID as *const Guid,
                ptr::addr_of!((*state).path).cast::<c_void>(),
                &EDID_DISCOVERED_PROTOCOL_GUID as *const Guid,
                edid,
                &EDID_ACTIVE_PROTOCOL_GUID as *const Guid,
                edid,
                ptr::null::<Guid>(),
            );
            if status != Status::SUCCESS {
                STATE.store(ptr::null_mut(), Ordering::Release);
                (boot.free_pool)(memory);
                return Err(status);
            }
        }
        Ok(())
    })();
    result.err().unwrap_or(Status::SUCCESS)
}
