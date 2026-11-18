//! Binary of the firmware image. The S7-04 entry parses and validates the boot handoff, then
//! reports that result on COM1 and stops. Own page tables, allocators and UEFI services are not
//! implemented yet. The entry `_start` is in `start.S`; the panic handler reports stop code `0x30`.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

use od_firmware::HANDOFF_GPA;
use od_firmware::handoff::{HANDOFF_WINDOW_SIZE, Handoff};
use od_firmware::stop::{STOP_INTERNAL, STOP_INVALID_RDI, format_stop_line};

const COM1_BASE: u16 = 0x3F8;
const LSR_THRE: u8 = 0x20;
/// Polling limit before a byte is dropped (FIRMWARE_BOOT.md, section 11).
const TX_POLL_LIMIT: u32 = 65536;

// Keep this equal to the image header's handoff_minor_max in build_firmware.py.
const HANDOFF_MINOR_MAX: u16 = 4;
const HANDOFF_VALIDATED_LINE: &[u8] = b"OD-FW S7-04 handoff validated\r\n";

/// # Safety
/// Port I/O: the caller must be at CPL0 (or have I/O permission) and `port` must be a real device
/// register of the machine.
unsafe fn outb(port: u16, value: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
    }
}

/// # Safety
/// Same as [`outb`].
unsafe fn inb(port: u16) -> u8 {
    let value: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn com1_init() {
    // 8N1, divisor 1; IER/FCR/MCR are not written (FIRMWARE_BOOT.md, section 11).
    unsafe {
        outb(COM1_BASE + 3, 0x80);
        outb(COM1_BASE, 0x01);
        outb(COM1_BASE + 1, 0x00);
        outb(COM1_BASE + 3, 0x03);
    }
}

fn com1_write(bytes: &[u8]) {
    for &byte in bytes {
        for _ in 0..TX_POLL_LIMIT {
            if unsafe { inb(COM1_BASE + 5) } & LSR_THRE != 0 {
                unsafe { outb(COM1_BASE, byte) };
                break;
            }
        }
        // On timeout the byte is dropped, never a hang.
    }
}

fn halt_forever() -> ! {
    loop {
        unsafe { asm!("cli", "hlt", options(nomem, nostack)) };
    }
}

fn stop(code: u8, detail: u64) -> ! {
    com1_init();
    com1_write(&format_stop_line(code, detail));
    halt_forever()
}

/// Rust entry point, called by `_start` with `RDI` as the first argument.
#[unsafe(no_mangle)]
pub extern "C" fn od_fw_main(handoff_gpa: u64) -> ! {
    if handoff_gpa != HANDOFF_GPA {
        stop(STOP_INVALID_RDI, handoff_gpa);
    }

    // SAFETY: the firmware entry contract guarantees the fixed 64 KiB handoff window is
    // identity-mapped, readable RAM at HANDOFF_GPA for the duration of firmware execution.
    let bytes =
        unsafe { core::slice::from_raw_parts(handoff_gpa as *const u8, HANDOFF_WINDOW_SIZE) };
    let _handoff = match Handoff::parse_for_boot(bytes, HANDOFF_MINOR_MAX) {
        Ok(handoff) => handoff,
        Err(error) => {
            let (code, detail) = error.stop_code_detail();
            stop(code, detail);
        }
    };

    com1_init();
    com1_write(HANDOFF_VALIDATED_LINE);
    halt_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let line = info
        .location()
        .map_or(0, |location| u64::from(location.line()));
    stop(STOP_INTERNAL, line)
}
