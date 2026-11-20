//! Exercises Secure Boot enrollment and revocation through EFI services.
#![no_std]
#![no_main]

use core::{mem::MaybeUninit, ptr, ptr::NonNull, slice};
use od_firmware::{
    fs::simple_fs::{FileProtocol, SIMPLE_FILE_SYSTEM_PROTOCOL_GUID, SimpleFileSystem},
    uefi::{
        guid::Guid,
        loaded_image::{LOADED_IMAGE_PROTOCOL_GUID, LoadedImageProtocol},
        status::Status,
        system_table::{BootServices, RuntimeServices, SystemTable},
        types::{Char16, Handle},
    },
    var::services::GLOBAL_VARIABLE,
};
use od_firmware_test_apps::{ascii_to_char16, output};

const SETUP_MODE: [Char16; 10] = ascii_to_char16(b"SetupMode\0");
const SECURE_BOOT: [Char16; 11] = ascii_to_char16(b"SecureBoot\0");
const KEK: [Char16; 4] = ascii_to_char16(b"KEK\0");
const DB: [Char16; 3] = ascii_to_char16(b"db\0");
const DBX: [Char16; 4] = ascii_to_char16(b"dbx\0");
const PK: [Char16; 3] = ascii_to_char16(b"PK\0");
const AUTHENTICATED_ATTRIBUTES: u32 = 0x27;
const BOOT_ATTRIBUTES: u32 = 7;
const BOOT_ORDER: [Char16; 10] = ascii_to_char16(b"BootOrder\0");
const FIRST_BOOT_OPTION: [Char16; 9] = ascii_to_char16(b"Boot000A\0");
const NEXT_BOOT_OPTION: [Char16; 9] = ascii_to_char16(b"Boot000B\0");
const IMAGE_SECURITY_DATABASE: Guid = Guid {
    data1: 0xd719_b2cb,
    data2: 0x3d3a,
    data3: 0x4596,
    data4: [0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f],
};
const KEK_FILE: [Char16; 18] = ascii_to_char16(b"\\EFI\\BOOT\\KEK.AUT\0");
const DB_FILE: [Char16; 17] = ascii_to_char16(b"\\EFI\\BOOT\\DB.AUT\0");
const PK_FILE: [Char16; 17] = ascii_to_char16(b"\\EFI\\BOOT\\PK.AUT\0");
const DBX_FILE: [Char16; 18] = ascii_to_char16(b"\\EFI\\BOOT\\DBX.AUT\0");
const TARGET_FILE: [Char16; 21] = ascii_to_char16(b"\\EFI\\BOOT\\TARGET.EFI\0");
const UNSIGNED_FILE: [Char16; 21] = ascii_to_char16(b"\\EFI\\BOOT\\UNSIGN.EFI\0");
const FIRST_OPTION_FILE: [Char16; 23] = ascii_to_char16(b"\\EFI\\BOOT\\BOOT000A.OPT\0");
const NEXT_OPTION_FILE: [Char16; 23] = ascii_to_char16(b"\\EFI\\BOOT\\BOOT000B.OPT\0");
const BEFORE: &[u8] = b"OD-UEFI Secure Boot SetupMode=1 SecureBoot=0\r\n\0";
const AFTER: &[u8] = b"OD-UEFI Secure Boot enrolled PK KEK db SetupMode=0 SecureBoot=1\r\n\0";
const REVOKED: &[u8] =
    b"OD-UEFI Secure Boot rejected unsigned at LoadImage and revoked at StartImage\r\n\0";
static BEFORE_LINE: [Char16; BEFORE.len()] = ascii_to_char16(BEFORE);
static AFTER_LINE: [Char16; AFTER.len()] = ascii_to_char16(AFTER);
static REVOKED_LINE: [Char16; REVOKED.len()] = ascii_to_char16(REVOKED);

struct OpenFile(NonNull<FileProtocol>);

impl OpenFile {
    /// The firmware returned an open, non-null file interface.
    fn checked(file: *mut FileProtocol) -> Result<Self, Status> {
        NonNull::new(file).map(Self).ok_or(Status::DEVICE_ERROR)
    }

    unsafe fn open(&self, path: &[Char16]) -> Result<Self, Status> {
        let root = self.0.as_ptr();
        let mut handle = ptr::null_mut();
        check(unsafe { ((*root).open)(root, &mut handle, path.as_ptr(), 1, 0) })?;
        Self::checked(handle)
    }

    unsafe fn read<'a, const CAP: usize>(
        &self,
        buffer: &'a mut MaybeUninit<[u8; CAP]>,
        minimum: usize,
    ) -> Result<&'a [u8], Status> {
        let file = self.0.as_ptr();
        let mut length = CAP;
        check(unsafe { ((*file).read)(file, &mut length, buffer.as_mut_ptr().cast()) })?;
        if !(minimum..CAP).contains(&length) {
            return Err(Status::BAD_BUFFER_SIZE);
        }
        // Read succeeded and returned the initialized prefix of the caller-owned buffer.
        Ok(unsafe { slice::from_raw_parts(buffer.as_ptr().cast::<u8>(), length) })
    }
}

impl Drop for OpenFile {
    fn drop(&mut self) {
        let file = self.0.as_ptr();
        unsafe { ((*file).close)(file) };
    }
}

struct LoadedChild<'a> {
    boot: &'a BootServices,
    handle: Handle,
}

impl Drop for LoadedChild<'_> {
    fn drop(&mut self) {
        unsafe { (self.boot.unload_image)(self.handle) };
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
    unsafe { run(handle, system) }.map_or_else(|failure| failure, |()| Status::SUCCESS)
}

unsafe fn run(handle: Handle, system: *mut SystemTable) -> Result<(), Status> {
    let table = unsafe { system.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    let services = unsafe { table.boot_services.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    let runtime = unsafe { table.runtime_services.as_ref() }.ok_or(Status::INVALID_PARAMETER)?;
    let mode = unsafe { read_modes(runtime) }?;
    if mode == [1, 0] {
        check(unsafe { output(system, BEFORE_LINE.as_ptr()) })?;
    }

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

    match mode {
        [1, 0] => unsafe { enroll(runtime, system, &opened) },
        [0, 1] => unsafe { revoke(services, runtime, handle, system, &opened) },
        _ => Err(Status::SECURITY_VIOLATION),
    }
}

unsafe fn enroll(
    runtime: &RuntimeServices,
    system: *mut SystemTable,
    root: &OpenFile,
) -> Result<(), Status> {
    unsafe { install_boot_options(runtime, root) }?;
    // PK is last so the previous writes remain SetupMode operations.
    for (file, name, namespace) in [
        (&KEK_FILE[..], &KEK[..], GLOBAL_VARIABLE),
        (&DB_FILE[..], &DB[..], IMAGE_SECURITY_DATABASE),
        (&PK_FILE[..], &PK[..], GLOBAL_VARIABLE),
    ] {
        let file = unsafe { root.open(file) }?;
        unsafe {
            set_variable_from_file(
                runtime,
                &file,
                name,
                &namespace,
                AUTHENTICATED_ATTRIBUTES,
                40,
            )
        }?;
    }
    if unsafe { read_modes(runtime) }? != [0, 1] {
        return Err(Status::SECURITY_VIOLATION);
    }
    check(unsafe { output(system, AFTER_LINE.as_ptr()) })
}

unsafe fn load_child<'a>(
    boot: &'a BootServices,
    parent: Handle,
    root: &OpenFile,
    path: &[Char16],
    expected: Status,
) -> Result<LoadedChild<'a>, Status> {
    let source = unsafe { root.open(path) }?;
    let mut buffer = MaybeUninit::<[u8; 8192]>::uninit();
    let bytes = unsafe { source.read(&mut buffer, 1) }?;
    let mut image = ptr::null_mut();
    let status = unsafe {
        (boot.load_image)(
            0,
            parent,
            ptr::null(),
            bytes.as_ptr().cast(),
            bytes.len(),
            &mut image,
        )
    };
    if image.is_null() {
        return Err(Status::LOAD_ERROR);
    }
    let child = LoadedChild {
        boot,
        handle: image,
    };
    if status != expected {
        return Err(Status::LOAD_ERROR);
    }
    Ok(child)
}

unsafe fn revoke(
    boot: &BootServices,
    runtime: &RuntimeServices,
    parent: Handle,
    system: *mut SystemTable,
    root: &OpenFile,
) -> Result<(), Status> {
    let unsigned = unsafe {
        load_child(
            boot,
            parent,
            root,
            &UNSIGNED_FILE,
            Status::SECURITY_VIOLATION,
        )
    }?;
    drop(unsigned);
    let target = unsafe { load_child(boot, parent, root, &TARGET_FILE, Status::SUCCESS) }?;
    let update = unsafe { root.open(&DBX_FILE) }?;
    unsafe {
        set_variable_from_file(
            runtime,
            &update,
            &DBX,
            &IMAGE_SECURITY_DATABASE,
            AUTHENTICATED_ATTRIBUTES,
            40,
        )
    }?;
    if unsafe { (boot.start_image)(target.handle, ptr::null_mut(), ptr::null_mut()) }
        != Status::SECURITY_VIOLATION
    {
        return Err(Status::LOAD_ERROR);
    }
    check(unsafe { output(system, REVOKED_LINE.as_ptr()) })
}

unsafe fn install_boot_options(runtime: &RuntimeServices, root: &OpenFile) -> Result<(), Status> {
    let first = match unsafe { root.open(&FIRST_OPTION_FILE) } {
        Ok(file) => file,
        Err(Status::NOT_FOUND) => return Ok(()),
        Err(status) => return Err(status),
    };
    let next = unsafe { root.open(&NEXT_OPTION_FILE) }?;
    for (file, name) in [(&first, &FIRST_BOOT_OPTION), (&next, &NEXT_BOOT_OPTION)] {
        unsafe {
            set_variable_from_file(runtime, file, name, &GLOBAL_VARIABLE, BOOT_ATTRIBUTES, 12)
        }?;
    }
    let order = [10u16, 11];
    check(unsafe {
        (runtime.set_variable)(
            BOOT_ORDER.as_ptr(),
            &GLOBAL_VARIABLE,
            BOOT_ATTRIBUTES,
            size_of_val(&order),
            order.as_ptr().cast(),
        )
    })
}

unsafe fn set_variable_from_file(
    runtime: &RuntimeServices,
    file: &OpenFile,
    name: &[Char16],
    namespace: &Guid,
    attributes: u32,
    minimum_length: usize,
) -> Result<(), Status> {
    let mut buffer = MaybeUninit::<[u8; 4096]>::uninit();
    let value = unsafe { file.read(&mut buffer, minimum_length) }?;
    check(unsafe {
        (runtime.set_variable)(
            name.as_ptr(),
            namespace,
            attributes,
            value.len(),
            value.as_ptr().cast(),
        )
    })
}

unsafe fn read_modes(runtime: &RuntimeServices) -> Result<[u8; 2], Status> {
    let mut values = [0u8; 2];
    for (name, value) in [SETUP_MODE.as_slice(), SECURE_BOOT.as_slice()]
        .into_iter()
        .zip(&mut values)
    {
        let mut length = 1;
        check(unsafe {
            (runtime.get_variable)(
                name.as_ptr(),
                &GLOBAL_VARIABLE,
                ptr::null_mut(),
                &mut length,
                ptr::from_mut(value).cast(),
            )
        })?;
        if length != 1 {
            return Err(Status::BAD_BUFFER_SIZE);
        }
    }
    Ok(values)
}
