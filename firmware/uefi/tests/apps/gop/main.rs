#![no_std]
#![no_main]

use core::{ffi::c_void, mem::size_of, ptr, slice};

use od_firmware::{
    bs::event::{EVT_NOTIFY_SIGNAL, EVT_TIMER, Event, TIMER_RELATIVE, TPL_CALLBACK},
    device_path::DEVICE_PATH_PROTOCOL_GUID,
    gop::protocol::{
        BLT_BUFFER_TO_VIDEO, BLT_VIDEO_FILL, BLT_VIDEO_TO_BUFFER, BLT_VIDEO_TO_VIDEO, BltPixel,
        EDID_ACTIVE_PROTOCOL_GUID, EDID_DISCOVERED_PROTOCOL_GUID, EdidProtocol,
        GRAPHICS_OUTPUT_PROTOCOL_GUID, GraphicsOutputProtocol, ModeInformation,
        PIXEL_BLUE_GREEN_RED_RESERVED_8BIT_PER_COLOR,
    },
    uefi::{
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};
use od_firmware_test_apps::{ascii_to_char16, output};

const BLACK: BltPixel = pixel(0, 0, 0);
const RED: BltPixel = pixel(0xff, 0, 0);
const GREEN: BltPixel = pixel(0, 0xff, 0);
const BLUE: BltPixel = pixel(0, 0, 0xff);
const WHITE: BltPixel = pixel(0xff, 0xff, 0xff);
const BARS: [BltPixel; 8] = [
    WHITE,
    pixel(0xff, 0xff, 0),
    pixel(0, 0xff, 0xff),
    GREEN,
    pixel(0xff, 0, 0xff),
    RED,
    BLUE,
    BLACK,
];

const fn pixel(red: u8, green: u8, blue: u8) -> BltPixel {
    BltPixel {
        blue,
        green,
        red,
        reserved: 0,
    }
}

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    match unsafe { inspect(system) } {
        Some(()) => Status::SUCCESS,
        None => Status::LOAD_ERROR,
    }
}

unsafe fn inspect(system: *mut SystemTable) -> Option<()> {
    let boot = unsafe { (*system).boot_services.as_ref() }?;
    let mut count = 0;
    let mut handles = ptr::null_mut::<Handle>();
    ok(unsafe {
        (boot.locate_handle_buffer)(
            2,
            &GRAPHICS_OUTPUT_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut count,
            &mut handles,
        )
    })?;
    let handle = if count == 1 && !handles.is_null() {
        unsafe { handles.read() }
    } else {
        ptr::null_mut()
    };
    ok(unsafe { (boot.free_pool)(handles.cast()) })?;
    if handle.is_null() {
        return None;
    }

    let mut interface = ptr::null_mut();
    for guid in [&EDID_DISCOVERED_PROTOCOL_GUID, &EDID_ACTIVE_PROTOCOL_GUID] {
        ok(unsafe { (boot.handle_protocol)(handle, guid, &mut interface) })?;
        let edid = unsafe { interface.cast::<EdidProtocol>().as_ref() }?;
        if edid.size_of_edid != 0 || !edid.edid.is_null() {
            return None;
        }
    }
    ok(unsafe { (boot.handle_protocol)(handle, &DEVICE_PATH_PROTOCOL_GUID, &mut interface) })?;
    if interface.is_null() {
        return None;
    }
    let path = unsafe { slice::from_raw_parts(interface.cast::<u8>(), 28) };
    if path != b"\x01\x03\x18\0\x0b\0\0\0\0\0\0\xfc\0\0\0\0\xff\x0f\0\xfc\0\0\0\0\x7f\xff\x04\0" {
        return None;
    }
    ok(unsafe { (boot.handle_protocol)(handle, &GRAPHICS_OUTPUT_PROTOCOL_GUID, &mut interface) })?;
    let gop = interface.cast::<GraphicsOutputProtocol>();
    let protocol = unsafe { gop.as_ref() }?;
    let mode = unsafe { protocol.mode.as_ref() }?;
    let max_mode = mode.max_mode;
    let active_info = mode.info;
    if max_mode != 14 || mode.mode != 0 || !unsafe { check_active(gop, 640, 480) } {
        return None;
    }
    let mut previous = (0, 0);
    for number in 0..max_mode {
        let mut size = 0;
        let mut info = ptr::null_mut();
        ok(unsafe { (protocol.query_mode)(gop, number, &mut size, &mut info) })?;
        let owned = unsafe { info.as_ref() }.copied();
        if info.cast_const() == active_info {
            return None;
        }
        ok(unsafe { (boot.free_pool)(info.cast()) })?;
        let info = owned?;
        let current = (info.horizontal_resolution, info.vertical_resolution);
        if size != size_of::<ModeInformation>()
            || info.version != 0
            || info.pixel_format != PIXEL_BLUE_GREEN_RED_RESERVED_8BIT_PER_COLOR
            || info.pixels_per_scan_line != info.horizontal_resolution
            || current <= previous
        {
            return None;
        }
        previous = current;
    }
    let mut size = usize::MAX;
    let mut info = ptr::null_mut();
    if unsafe { (protocol.query_mode)(gop, max_mode, &mut size, &mut info) }
        != Status::INVALID_PARAMETER
        || size != usize::MAX
        || !info.is_null()
        || unsafe { (protocol.set_mode)(gop, max_mode) } != Status::UNSUPPORTED
    {
        return None;
    }
    let line = ascii_to_char16::<{ b"OD-UEFI gop modes and device path verified\r\n\0".len() }>(
        b"OD-UEFI gop modes and device path verified\r\n\0",
    );
    ok(unsafe { output(system, line.as_ptr()) })?;
    const UNSUPPORTED: &[u8] = b"OD-UEFI gop SetMode out of range: EFI_UNSUPPORTED\r\n\0";
    let line = ascii_to_char16::<{ UNSUPPORTED.len() }>(UNSUPPORTED);
    ok(unsafe { output(system, line.as_ptr()) })?;
    unsafe { check_gop_timer(boot, gop, max_mode) }?;

    ok(unsafe { (protocol.set_mode)(gop, 1) })?;
    if !unsafe { check_active(gop, 800, 600) } || unsafe { read_pixel(gop, 0, 0) }? != BLACK {
        return None;
    }
    unsafe { check_blt(gop) }?;
    ok(unsafe { (protocol.set_mode)(gop, 1) })?;
    if unsafe { read_pixel(gop, 1, 1) }? != BLACK {
        return None;
    }
    ok(unsafe { (protocol.set_mode)(gop, 0) })?;
    if !unsafe { check_active(gop, 640, 480) } {
        return None;
    }
    for (index, mut color) in BARS.into_iter().enumerate() {
        ok(unsafe {
            (protocol.blt)(
                gop,
                &mut color,
                BLT_VIDEO_FILL,
                0,
                0,
                index * 80,
                0,
                80,
                480,
                0,
            )
        })?;
    }
    let line = ascii_to_char16::<{ b"OD-UEFI gop Blt and frame verified\r\n\0".len() }>(
        b"OD-UEFI gop Blt and frame verified\r\n\0",
    );
    ok(unsafe { output(system, line.as_ptr()) })
}

unsafe fn check_gop_timer(
    boot: &BootServices,
    gop: *mut GraphicsOutputProtocol,
    invalid_mode: u32,
) -> Option<()> {
    let mut fired = 0u8;
    let flag = ptr::addr_of_mut!(fired);
    let mut event = ptr::null_mut();
    ok(unsafe {
        (boot.create_event)(
            EVT_TIMER | EVT_NOTIFY_SIGNAL,
            TPL_CALLBACK,
            Some(timer_fired),
            flag.cast(),
            &mut event,
        )
    })?;
    let mut observed = false;
    if unsafe { (boot.set_timer)(event, TIMER_RELATIVE, 200) } == Status::SUCCESS {
        for _ in 0..2048 {
            if unsafe { ((*gop).set_mode)(gop, invalid_mode) } != Status::UNSUPPORTED {
                break;
            }
            if unsafe { flag.read_volatile() } != 0 {
                observed = true;
                break;
            }
        }
    }
    ok(unsafe { (boot.close_event)(event) })?;
    observed.then_some(())
}

unsafe extern "efiapi" fn timer_fired(_: Event, context: *mut c_void) {
    unsafe { context.cast::<u8>().write_volatile(1) };
}

unsafe fn check_active(gop: *mut GraphicsOutputProtocol, width: u32, height: u32) -> bool {
    let mode = unsafe { &*(*gop).mode };
    let Some(info) = (unsafe { mode.info.as_ref() }) else {
        return false;
    };
    (
        info.horizontal_resolution,
        info.vertical_resolution,
        info.pixels_per_scan_line,
    ) == (width, height, width)
        && info.pixel_format == PIXEL_BLUE_GREEN_RED_RESERVED_8BIT_PER_COLOR
        && mode.size_of_info == size_of::<ModeInformation>()
        && mode.frame_buffer_base == 0xf800_0000
        && mode.frame_buffer_size == (width * 4 * height) as usize
}

unsafe fn check_blt(gop: *mut GraphicsOutputProtocol) -> Option<()> {
    let blt = unsafe { (*gop).blt };
    let mut color = WHITE;
    ok(unsafe { blt(gop, &mut color, BLT_VIDEO_FILL, 0, 0, 0, 0, 4, 4, 0) })?;
    let mut source = [0xffu8; 40];
    for (offset, pixel) in [(17, RED), (21, GREEN), (30, BLUE), (34, BLACK)] {
        unsafe {
            source
                .as_mut_ptr()
                .add(offset)
                .cast::<BltPixel>()
                .write(pixel)
        };
    }
    let buffer = source.as_mut_ptr().cast::<BltPixel>();
    if unsafe { blt(gop, buffer, BLT_BUFFER_TO_VIDEO, 1, 1, 0, 0, 2, 2, 0) }
        != Status::INVALID_PARAMETER
    {
        return None;
    }
    ok(unsafe { blt(gop, buffer, BLT_BUFFER_TO_VIDEO, 1, 1, 0, 0, 2, 2, 13) })?;
    ok(unsafe {
        blt(
            gop,
            ptr::null_mut(),
            BLT_VIDEO_TO_VIDEO,
            0,
            0,
            1,
            1,
            2,
            2,
            0,
        )
    })?;
    let mut destination = [0xffu8; 40];
    ok(unsafe {
        blt(
            gop,
            destination.as_mut_ptr().cast(),
            BLT_VIDEO_TO_BUFFER,
            1,
            1,
            1,
            1,
            2,
            2,
            13,
        )
    })?;
    if source != destination || unsafe { read_pixel(gop, 3, 3) }? != WHITE {
        return None;
    }
    Some(())
}

unsafe fn read_pixel(gop: *mut GraphicsOutputProtocol, x: usize, y: usize) -> Option<BltPixel> {
    let mut pixel = BLACK;
    ok(unsafe { ((*gop).blt)(gop, &mut pixel, BLT_VIDEO_TO_BUFFER, x, y, 0, 0, 1, 1, 0) })?;
    Some(pixel)
}

fn ok(status: Status) -> Option<()> {
    (status == Status::SUCCESS).then_some(())
}
