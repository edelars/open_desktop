//! Binary of the firmware image. S7-01: proof that the freestanding build works, not firmware.
//!
//! It initialises COM1 (FIRMWARE_BOOT.md, section 11), prints one line and stops. The entry
//! `_start` is in `start.S`; the handoff block is not parsed, only its address is checked and kept.
//! The panic handler prints the stop line with code `0x30` (section 12) and halts.

#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicU64, Ordering};

use od_firmware::HANDOFF_GPA;
use od_firmware::stop::{STOP_INTERNAL, STOP_INVALID_RDI, format_stop_line};

const COM1_BASE: u16 = 0x3F8;
const LSR_THRE: u8 = 0x20;
/// Polling limit before a byte is dropped (FIRMWARE_BOOT.md, section 11).
const TX_POLL_LIMIT: u32 = 65536;

const BUILD_PROOF_LINE: &[u8] = b"OD-FW S7-01 build proof (not a firmware)\r\n";

/// GPA of the handoff block as received in `RDI`. Not parsed in S7-01.
static HANDOFF_BLOCK: AtomicU64 = AtomicU64::new(0);

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
    HANDOFF_BLOCK.store(handoff_gpa, Ordering::Relaxed);
    if handoff_gpa != HANDOFF_GPA {
        stop(STOP_INVALID_RDI, handoff_gpa);
    }
    com1_init();
    com1_write(BUILD_PROOF_LINE);
    halt_forever()
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    let line = info
        .location()
        .map_or(0, |location| u64::from(location.line()));
    stop(STOP_INTERNAL, line)
}
