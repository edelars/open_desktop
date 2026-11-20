#![no_std]
#![no_main]

use core::{arch::asm, mem::MaybeUninit, ptr};
use od_firmware::uefi::{
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware::{
    gop::protocol::{GRAPHICS_OUTPUT_PROTOCOL_GUID, GraphicsOutputProtocol},
    mem::map::{MemoryDescriptor, MemoryType},
};
use od_firmware_test_apps::{ascii_to_char16, output};

const TEXT: &str = "OD-UEFI ebs\r\n\0";
const FRAME_TEXT: &[u8] = b"OD-UEFI ebs checking GOP framebuffer\r\n\0";
const MAP_CAPACITY: usize = 256;
const TEST_PIXEL: u32 = 0x0012_3456;
static MESSAGE: [Char16; TEXT.len()] = ascii_to_char16(TEXT.as_bytes());
// Keep one absolute pointer in the PE so the same app can exercise DIR64.
static MESSAGE_ADDRESS: &[Char16; TEXT.len()] = &MESSAGE;

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let boot_services = unsafe { (*system).boot_services };
    if boot_services.is_null() {
        return Status::INVALID_PARAMETER;
    }
    let message = unsafe { ptr::read_volatile(&MESSAGE_ADDRESS) };
    let status = unsafe { output(system, message.as_ptr()) };
    if status.is_error() {
        return status;
    }
    let mut interface = ptr::null_mut();
    let status = unsafe {
        ((*boot_services).locate_protocol)(
            &GRAPHICS_OUTPUT_PROTOCOL_GUID,
            ptr::null_mut(),
            &mut interface,
        )
    };
    let framebuffer = match status {
        Status::SUCCESS => {
            let Some(mode) = (unsafe {
                interface
                    .cast::<GraphicsOutputProtocol>()
                    .as_ref()
                    .and_then(|gop| gop.mode.as_ref())
            }) else {
                return Status::LOAD_ERROR;
            };
            let base = mode.frame_buffer_base;
            let pixel = ptr::with_exposed_provenance_mut::<u32>(base as usize);
            let line = ascii_to_char16::<{ FRAME_TEXT.len() }>(FRAME_TEXT);
            let status = unsafe { output(system, line.as_ptr()) };
            if status.is_error() {
                return status;
            }
            unsafe { pixel.write_volatile(TEST_PIXEL) };
            Some((base, pixel))
        }
        Status::NOT_FOUND => None,
        _ => return status,
    };

    let mut map = MaybeUninit::<[MemoryDescriptor; MAP_CAPACITY]>::uninit();
    for _ in 0..2 {
        let mut size = size_of_val(&map);
        let mut key = 0;
        let mut descriptor_size = 0;
        let mut version = 0;
        let status = unsafe {
            ((*boot_services).get_memory_map)(
                &mut size,
                map.as_mut_ptr().cast::<MemoryDescriptor>(),
                &mut key,
                &mut descriptor_size,
                &mut version,
            )
        };
        if status.is_error() {
            return status;
        }
        if let Some((base, _)) = framebuffer
            && !unsafe { contains_framebuffer(&map, size, descriptor_size, base) }
        {
            return Status::LOAD_ERROR;
        }
        let status = unsafe { ((*boot_services).exit_boot_services)(handle, key) };
        if status == Status::SUCCESS {
            let marker = if framebuffer
                .is_none_or(|(_, pixel)| unsafe { pixel.read_volatile() } == TEST_PIXEL)
            {
                b'!'
            } else {
                b'?'
            };
            unsafe {
                asm!("out dx, al", "cli", "2: hlt", "jmp 2b", in("dx") 0x3f8u16,
                    in("al") marker, options(nomem, nostack, noreturn))
            }
        }
        if status != Status::INVALID_PARAMETER {
            return status;
        }
    }
    Status::INVALID_PARAMETER
}

// On success GetMemoryMap initialized `size` bytes in `map`.
unsafe fn contains_framebuffer(
    map: &MaybeUninit<[MemoryDescriptor; MAP_CAPACITY]>,
    mut size: usize,
    descriptor_size: usize,
    base: u64,
) -> bool {
    if descriptor_size != size_of::<MemoryDescriptor>() || size > size_of_val(map) {
        return false;
    }
    let mut entry = map.as_ptr().cast::<MemoryDescriptor>();
    while size >= descriptor_size {
        let record = unsafe { &*entry };
        if record.physical_start == base {
            // EFI_MEMORY_UC, without EFI_MEMORY_RUNTIME.
            return record.memory_type == MemoryType::MEMORY_MAPPED_IO.raw()
                && record.attribute == 1;
        }
        entry = unsafe { entry.add(1) };
        size -= descriptor_size;
    }
    false
}
