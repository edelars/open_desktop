//! S7-01 PE32+ build proof: an EFI application that returns `EFI_SUCCESS` and does nothing else.
//!
//! It exists only so that `tools/build/build_firmware.py --efi-probe` can check that the
//! `x86_64-unknown-uefi` toolchain produces a PE32+ image with a `.reloc` section. It is not a
//! test application (S7-06) and not the runtime image (S7-11).

#![no_std]
#![no_main]

use core::ffi::c_void;
use core::panic::PanicInfo;

/// Entry point of an EFI application: `EFI_STATUS EFIAPI efi_main(EFI_HANDLE, EFI_SYSTEM_TABLE *)`.
#[unsafe(no_mangle)]
pub extern "efiapi" fn efi_main(_image: *mut c_void, _system_table: *mut c_void) -> usize {
    0 // EFI_SUCCESS
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
