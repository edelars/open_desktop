#![no_std]

use core::{arch::asm, ffi::c_void, panic::PanicInfo};
use od_firmware::uefi::{status::Status, system_table::SystemTable, types::Char16};

// UEFI 2.11 §12.4.1. Only the first two slots are needed by these apps.
#[repr(C)]
struct SimpleTextOutputPrefix {
    reset: unsafe extern "efiapi" fn(*mut c_void, u8) -> Status,
    output_string: unsafe extern "efiapi" fn(*mut c_void, *const Char16) -> Status,
}

pub const fn ascii_to_char16<const N: usize>(text: &[u8]) -> [Char16; N] {
    assert!(text.len() == N);
    let mut output = [0; N];
    let mut i = 0;
    while i < N {
        assert!(text[i].is_ascii());
        output[i] = text[i] as Char16;
        i += 1;
    }
    output
}

/// # Safety
/// `system` must point to the live EFI system table. `message` must point to
/// a valid NUL-terminated Char16 string.
pub unsafe fn output(system: *mut SystemTable, message: *const Char16) -> Status {
    let console = unsafe { (*system).console_out() };
    if console.is_null() {
        return Status::SUCCESS;
    }
    let prefix = console.cast::<SimpleTextOutputPrefix>();
    unsafe { ((*prefix).output_string)(console, message) }
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    unsafe { asm!("cli", "2: hlt", "jmp 2b", options(nomem, nostack, noreturn)) }
}
