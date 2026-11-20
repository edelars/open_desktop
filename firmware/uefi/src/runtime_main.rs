//! Entry point of the firmware's separately linked EFI runtime driver.

#![no_std]
#![no_main]

use od_firmware::uefi::{status::Status, system_table::SystemTable, types::Handle};

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(image: Handle, system: *mut SystemTable) -> Status {
    // SAFETY: the firmware loads and starts this embedded runtime driver.
    unsafe { od_firmware::rt::driver::entry(image, system) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    unsafe { core::arch::asm!("ud2", options(noreturn, nostack)) }
}
