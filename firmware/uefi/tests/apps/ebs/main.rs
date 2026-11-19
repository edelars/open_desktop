#![no_std]
#![no_main]

use core::{arch::asm, mem::MaybeUninit, ptr};
use od_firmware::mem::map::MemoryDescriptor;
use od_firmware::uefi::{
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware_test_apps::{ascii_to_char16, output};

const TEXT: &str = "OD-UEFI ebs\r\n\0";
const MAP_CAPACITY: usize = 256;
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
        let status = unsafe { ((*boot_services).exit_boot_services)(handle, key) };
        if status == Status::SUCCESS {
            unsafe {
                asm!("out dx, al", "cli", "2: hlt", "jmp 2b", in("dx") 0x3f8u16,
                    in("al") b'!', options(nomem, nostack, noreturn))
            }
        }
        if status != Status::INVALID_PARAMETER {
            return status;
        }
    }
    Status::INVALID_PARAMETER
}
