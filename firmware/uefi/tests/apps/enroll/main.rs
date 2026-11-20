//! Enrolls test public keys through EFI SetVariable on the ESP boot path.
#![no_std]
#![no_main]

use core::{mem::MaybeUninit, ptr, ptr::NonNull};
use od_firmware::{
    fs::simple_fs::{FileProtocol, SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, SimpleFileSystem},
    uefi::{
        guid::Guid,
        loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
        status::Status,
        system_table::{RuntimeServices, SystemTable},
        types::{Char16, Handle},
    },
    var::services::GLOBAL_VARIABLE,
};
use od_firmware_test_apps::{ascii_to_char16, output};

const SETUP_MODE: [Char16; 10] = ascii_to_char16(b"SetupMode\0");
const SECURE_BOOT: [Char16; 11] = ascii_to_char16(b"SecureBoot\0");
const KEK: [Char16; 4] = ascii_to_char16(b"KEK\0");
const DB: [Char16; 3] = ascii_to_char16(b"db\0");
const PK: [Char16; 3] = ascii_to_char16(b"PK\0");
const AUTHENTICATED_ATTRIBUTES: u32 = 0x27;
const IMAGE_SECURITY_DATABASE: Guid = Guid {
    data1: 0xd719_b2cb,
    data2: 0x3d3a,
    data3: 0x4596,
    data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
};
const KEK_FILE: [Char16; 18] = ascii_to_char16(b"\\EFI\\BOOT\\KEK.AUT\0");
const DB_FILE: [Char16; 17] = ascii_to_char16(b"\\EFI\\BOOT\\DB.AUT\0");
const PK_FILE: [Char16; 17] = ascii_to_char16(b"\\EFI\\BOOT\\PK.AUT\0");
const BEFORE: &[u8] = b"OD-UEFI Secure Boot SetupMode=1 SecureBoot=0\r\n\0";
const AFTER: &[u8] = b"OD-UEFI Secure Boot enrolled PK KEK db SetupMode=0 SecureBoot=1\r\n\0";
static BEFORE_LINE: [Char16; BEFORE.len()] = ascii_to_char16(BEFORE);
static AFTER_LINE: [Char16; AFTER.len()] = ascii_to_char16(AFTER);

struct OpenFile(NonNull<FileProtocol>);

impl OpenFile {
    /// The firmware returned an open, non-null file interface.
    fn checked(file: *mut FileProtocol) -> Result<Self, Status> {
        NonNull::new(file).map(Self).ok_or(Status::DEVICE_ERROR)
    }
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        let file = self.0.as_ptr();
        unsafe { ((*file).close)(file) };
    }
}

fn check(status: Status) -> Result<(), Status> {
    if status == Status::SUCCESS {
        Ok(())
    } else {
        Err(status)
    }
}

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    unsafe { enroll(handle, system) }.map_or_else(|failure| failure, |()| Status::SUCCESS)
}

unsafe fn enroll(handle: Handle, system: *mut SystemTable) -> Result<(), Status> {
    let table = unsafe { system.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    let services = unsafe { table.boot_services.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    let runtime = unsafe { table.runtime_services.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    unsafe { check_modes(runtime, 1, 0) }?;
    check(unsafe { output(system, BEFORE_LINE.as_ptr()) })?;

    let mut interface = ptr::null_mut();
    check(unsafe {
        (services.handle_protocol)(handle, &LOADED_IMAGE_PROTOCOL_GUID, &mut interface)
    })?;
    let image =
        unsafe { interface.cast::<LoadedImageProtocol>().as_ref() }.ok_or(Status::DEVICE_ERROR)?;
    check(unsafe {
        (services.handle_protocol)(
            image.device_handle,
            &SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
            &mut interface,
        )
    })?;
    let fs = interface.cast::<SimpleFileSystem>();
    let protocol = unsafe { fs.as_ref() }.ok_or(Status::DEVICE_ERROR)?;
    let mut root = ptr::null_mut();
    check(unsafe { (protocol.open_volume)(fs, &mut root) })?;
    let opened = OpenFile::checked(root)?;

    // PK is last so the previous writes remain SetupMode operations.
    for (file, name, namespace) in [
        (&KEK_FILE[..], &KEK[..], GLOBAL_VARIABLE),
        (&DB_FILE[..], &DB[..], IMAGE_SECURITY_DATABASE),
        (&PK_FILE[..], &PK[..], GLOBAL_VARIABLE),
    ] {
        unsafe { enroll_variable(runtime, &opened, file, name, &namespace) }?;
    }
    unsafe { check_modes(runtime, 0, 1) }?;
    check(unsafe { output(system, AFTER_LINE.as_ptr()) })
}

unsafe fn enroll_variable(
    runtime: &RuntimeServices,
    root: &OpenFile,
    path: &[Char16],
    name: &[Char16],
    namespace: &Guid,
) -> Result<(), Status> {
    let root = root.0.as_ptr();
    let mut handle = ptr::null_mut();
    check(unsafe { ((*root).open)(root, &mut handle, path.as_ptr(), 1, 0) })?;
    let file = OpenFile::checked(handle)?;
    let file = file.0.as_ptr();
    let mut buffer = MaybeUninit::<[u8; 4096]>::uninit();
    let mut length = 4096;
    check(unsafe { ((*file).read)(file, &mut length, buffer.as_mut_ptr().cast()) })?;
    if !(40..4096).contains(&length) {
        return Err(Status::BAD_BUFFER_SIZE);
    }
    check(unsafe {
        (runtime.set_variable)(
            name.as_ptr(),
            namespace,
            AUTHENTICATED_ATTRIBUTES,
            length,
            buffer.as_ptr().cast(),
        )
    })
}

unsafe fn check_modes(runtime: &RuntimeServices, setup: u8, secure: u8) -> Result<(), Status> {
    for (name, expected) in [(&SETUP_MODE[..], setup), (&SECURE_BOOT[..], secure)] {
        let mut value = 0u8;
        let mut length = 1;
        check(unsafe {
            (runtime.get_variable)(
                name.as_ptr(),
                &GLOBAL_VARIABLE,
                ptr::null_mut(),
                &mut length,
                ptr::from_mut(&mut value).cast(),
            )
        })?;
        if length != 1 || value != expected {
            return Err(Status::SECURITY_VIOLATION);
        }
    }
    Ok(())
}
