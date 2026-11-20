#![no_std]
#![no_main]

use core::{arch::asm, mem::MaybeUninit, ptr};
use od_firmware::{
    fs::simple_fs::{SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, SimpleFileSystem},
    mem::map::{MemoryDescriptor, MemoryType},
    uefi::{
        loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
        runtime::RESET_COLD,
        status::Status,
        system_table::{BootServices, SystemTable},
        types::Handle,
    },
};
use od_firmware_test_apps::ascii_to_char16;

const MAP_CAPACITY: usize = 256;
const CHECK_FILE: &[u8] = b"\\EFI\\BOOT\\CHECK.EFI\0";
static CHECK_PATH: [u16; CHECK_FILE.len()] = ascii_to_char16(CHECK_FILE);
const MAX_PROBE_FILE: usize = 16 * 1024;
const FAT_ERROR: &[u8] = b"OD-UEFI FAT chain rejected with EFI_VOLUME_CORRUPTED\r\n";

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(image: Handle, system: *mut SystemTable) -> Status {
    let Some(system) = (unsafe { system.as_ref() }) else {
        return Status::INVALID_PARAMETER;
    };
    let (Some(boot), Some(runtime)) = (unsafe { system.boot_services.as_ref() }, unsafe {
        system.runtime_services.as_ref()
    }) else {
        return Status::LOAD_ERROR;
    };
    match unsafe { check_fault_file(image, boot) } {
        Ok(true) => return Status::SUCCESS,
        Ok(false) => {}
        Err(status) => return status,
    }
    let reset = runtime.reset_system;
    let mut map = MaybeUninit::<[MemoryDescriptor; MAP_CAPACITY]>::uninit();
    let mut size = size_of_val(&map);
    let mut key = 0;
    let mut descriptor_size = 0;
    let mut version = 0;
    let status = unsafe {
        (boot.get_memory_map)(
            &mut size,
            map.as_mut_ptr().cast::<MemoryDescriptor>(),
            &mut key,
            &mut descriptor_size,
            &mut version,
        )
    };
    if status != Status::SUCCESS {
        return status;
    }
    let exit = boot.exit_boot_services;
    if unsafe { exit(image, key ^ 1) } != Status::INVALID_PARAMETER {
        return Status::LOAD_ERROR;
    }
    let status = unsafe { exit(image, key) };
    if status != Status::SUCCESS {
        return status;
    }
    write_com1(b"OD-UEFI invalid MapKey rejected and valid key accepted\r\n");
    unsafe { reset(RESET_COLD, Status::SUCCESS, 0, ptr::null()) };
    write_com1(b"OD-UEFI ResetSystem returned\r\n");
    unsafe { asm!("cli", "2: hlt", "jmp 2b", options(nostack, noreturn)) };
}

fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

/// When the disk contains CHECK.EFI, exercise that file and return before the reset probe.
unsafe fn check_fault_file(image: Handle, boot: &BootServices) -> Result<bool, Status> {
    let mut interface = ptr::null_mut();
    check(unsafe { (boot.handle_protocol)(image, &LOADED_IMAGE_PROTOCOL_GUID, &mut interface) })?;
    let loaded =
        unsafe { interface.cast::<LoadedImageProtocol>().as_ref() }.ok_or(Status::LOAD_ERROR)?;
    check(unsafe {
        (boot.handle_protocol)(
            loaded.device_handle,
            &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
            &mut interface,
        )
    })?;
    let fs = interface.cast::<SimpleFileSystem>();
    let protocol = unsafe { fs.as_ref() }.ok_or(Status::LOAD_ERROR)?;
    let mut root = ptr::null_mut();
    check(unsafe { (protocol.open_volume)(fs, &mut root) })?;
    let directory = unsafe { root.as_ref() }.ok_or(Status::LOAD_ERROR)?;
    let mut file = ptr::null_mut();
    let opened = unsafe { (directory.open)(root, &mut file, CHECK_PATH.as_ptr(), 1, 0) };
    check(unsafe { (directory.close)(root) })?;
    if opened == Status::NOT_FOUND {
        return Ok(false);
    }
    if opened == Status::VOLUME_CORRUPTED {
        write_com1(FAT_ERROR);
        return Ok(true);
    }
    check(opened)?;
    let protocol = unsafe { file.as_ref() }.ok_or(Status::LOAD_ERROR)?;
    let mut buffer = ptr::null_mut();
    let allocated =
        unsafe { (boot.allocate_pool)(MemoryType::LOADER_DATA, MAX_PROBE_FILE, &mut buffer) };
    if allocated != Status::SUCCESS {
        unsafe { (protocol.close)(file) };
        return Err(allocated);
    }
    let mut size = MAX_PROBE_FILE;
    let read = unsafe { (protocol.read)(file, &mut size, buffer) };
    let closed = unsafe { (protocol.close)(file) };
    let mut unexpected_image = ptr::null_mut();
    let load_status = if read == Status::SUCCESS && (1..=MAX_PROBE_FILE).contains(&size) {
        Some(unsafe {
            (boot.load_image)(0, image, ptr::null(), buffer, size, &mut unexpected_image)
        })
    } else {
        None
    };
    check(unsafe { (boot.free_pool)(buffer) })?;
    if !unexpected_image.is_null() {
        check(unsafe { (boot.unload_image)(unexpected_image) })?;
    }
    check(closed)?;
    if read == Status::VOLUME_CORRUPTED {
        write_com1(FAT_ERROR);
    } else if load_status == Some(Status::LOAD_ERROR) {
        write_com1(b"OD-UEFI PE section rejected with EFI_LOAD_ERROR\r\n");
    } else {
        return Err(Status::LOAD_ERROR);
    }
    Ok(true)
}

fn write_com1(message: &[u8]) {
    for byte in message {
        unsafe {
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") *byte, options(nostack, preserves_flags))
        };
    }
}
