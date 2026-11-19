//! Final stop line of the firmware (FIRMWARE_BOOT.md, section 12).
//!
//! Format: `OD-FW STOP code=0xNN detail=0xDDDDDDDDDDDDDDDD\r\n`.

/// `RDI` at entry is not the handoff block address.
pub const STOP_INVALID_RDI: u8 = 0x01;
/// Handoff block magic does not match `ODHANDOF`.
pub const STOP_HANDOFF_MAGIC: u8 = 0x02;
/// Handoff version is not supported by this image.
pub const STOP_HANDOFF_VERSION: u8 = 0x03;
/// Handoff header or record sizes are invalid.
pub const STOP_HANDOFF_SIZE: u8 = 0x04;
/// Handoff CRC does not match.
pub const STOP_HANDOFF_CRC: u8 = 0x05;
/// Handoff table bounds or offsets are invalid.
pub const STOP_HANDOFF_TABLE: u8 = 0x06;
/// Handoff region or board memory metadata is invalid.
pub const STOP_HANDOFF_REGION: u8 = 0x07;
/// Handoff device metadata is invalid.
pub const STOP_HANDOFF_DEVICE: u8 = 0x08;
/// Handoff CPU count is invalid.
pub const STOP_HANDOFF_CPU_COUNT: u8 = 0x09;
/// Handoff TSC frequency is invalid.
pub const STOP_HANDOFF_TSC: u8 = 0x0A;
/// Handoff serial number is invalid.
pub const STOP_HANDOFF_SERIAL: u8 = 0x0B;
/// Firmware shadow reservation does not match the linked image range.
pub const STOP_HANDOFF_SHADOW: u8 = 0x0C;
/// Unhandled exception delivered through the firmware IDT.
pub const STOP_EXCEPTION: u8 = 0x10;
/// Internal firmware error (panic handler).
pub const STOP_INTERNAL: u8 = 0x30;
/// Firmware-owned page tables could not be prepared.
pub const STOP_PAGING: u8 = 0x31;
/// The initial EFI memory map could not be produced.
pub const STOP_MEMORY_MAP: u8 = 0x32;
/// The bootstrap page allocator failed.
pub const STOP_PAGE_ALLOCATOR: u8 = 0x33;
/// The firmware pool allocator failed its boot verification.
pub const STOP_POOL_ALLOCATOR: u8 = 0x34;
/// A private UEFI Boot Services utility failed the guest smoke check.
pub const STOP_BOOT_SERVICES: u8 = 0x35;

const PREFIX: &[u8] = b"OD-FW STOP code=0x";
const MIDDLE: &[u8] = b" detail=0x";
const SUFFIX: &[u8] = b"\r\n";

/// Length of the stop line in bytes.
pub const STOP_LINE_LEN: usize = PREFIX.len() + 2 + MIDDLE.len() + 16 + SUFFIX.len();

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// Renders the stop line without `core::fmt` (upper-case hex, fixed width).
pub fn format_stop_line(code: u8, detail: u64) -> [u8; STOP_LINE_LEN] {
    let mut line = [0u8; STOP_LINE_LEN];
    let mut at = 0;
    let mut put = |bytes: &[u8], line: &mut [u8; STOP_LINE_LEN]| {
        line[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(PREFIX, &mut line);
    put(
        &[HEX[(code >> 4) as usize], HEX[(code & 0xF) as usize]],
        &mut line,
    );
    put(MIDDLE, &mut line);
    put(&fixed_hex(detail), &mut line);
    put(SUFFIX, &mut line);
    debug_assert_eq!(at, STOP_LINE_LEN);
    line
}

/// Renders 16 upper-case hexadecimal digits without a prefix.
pub fn fixed_hex(value: u64) -> [u8; 16] {
    let mut digits = [0u8; 16];
    for (index, digit) in digits.iter_mut().enumerate() {
        *digit = HEX[((value >> (60 - 4 * index)) & 0xf) as usize];
    }
    digits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_documented_line() {
        let line = format_stop_line(STOP_INTERNAL, 0x0000_0000_0000_002A);
        assert_eq!(
            &line[..],
            b"OD-FW STOP code=0x30 detail=0x000000000000002A\r\n".as_slice()
        );
        assert_eq!(line.len(), 48);
    }

    #[test]
    fn formats_full_width_detail_and_low_code() {
        let line = format_stop_line(STOP_INVALID_RDI, 0xFEDC_BA98_7654_3210);
        assert_eq!(
            &line[..],
            b"OD-FW STOP code=0x01 detail=0xFEDCBA9876543210\r\n".as_slice()
        );
    }
}
