//! Byte I/O for the boot firmware's COM1 console.

use core::arch::asm;

const COM1_BASE: u16 = 0x3F8;
const LSR_DR: u8 = 0x01;
const LSR_THRE: u8 = 0x20;
/// Polling limit before a byte is dropped (FIRMWARE_BOOT.md, section 11).
const TX_POLL_LIMIT: u32 = 65536;

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

pub fn init() {
    // 8N1, divisor 1; IER/FCR/MCR are not written (FIRMWARE_BOOT.md, section 11).
    unsafe {
        outb(COM1_BASE + 3, 0x80);
        outb(COM1_BASE, 0x01);
        outb(COM1_BASE + 1, 0x00);
        outb(COM1_BASE + 3, 0x03);
    }
}

fn transmit(byte: u8) -> bool {
    for _ in 0..TX_POLL_LIMIT {
        if unsafe { inb(COM1_BASE + 5) } & LSR_THRE != 0 {
            unsafe { outb(COM1_BASE, byte) };
            return true;
        }
    }
    false
}

/// Diagnostic output drops a byte on timeout, keeping the boot trace bounded.
pub fn write(bytes: &[u8]) {
    for &byte in bytes {
        let _ = transmit(byte);
    }
}

pub(super) fn write_checked(bytes: &[u8]) -> bool {
    for &byte in bytes {
        if !transmit(byte) {
            return false;
        }
    }
    true
}

/// Polls once and reads the receiver buffer only when data is ready.
pub fn try_read() -> Option<u8> {
    // SAFETY: the firmware runs at CPL0 and COM1 has DLAB clear after init.
    unsafe { (inb(COM1_BASE + 5) & LSR_DR != 0).then(|| inb(COM1_BASE)) }
}

/// Receives one byte or reports the UART line error associated with it.
pub(super) fn receive_checked() -> Result<Option<u8>, ()> {
    const LSR_RX_ERRORS: u8 = 0x1e;
    // SAFETY: the firmware owns this UART with DLAB clear.
    unsafe {
        let status = inb(COM1_BASE + 5);
        let byte = (status & LSR_DR != 0).then(|| inb(COM1_BASE));
        if status & LSR_RX_ERRORS != 0 {
            Err(())
        } else {
            Ok(byte)
        }
    }
}
