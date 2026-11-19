//! UEFI text console over the board's COM1 port.

#[cfg(target_os = "none")]
pub mod serial_term;
pub mod text_in;
pub mod text_out;

/// Vendor hardware node (C0938024-BC5E-4EE6-ADF5-89166E13589A), followed by End Entire.
pub static COM1_DEVICE_PATH: [u8; 24] = [
    0x01, 0x04, 0x14, 0x00, 0x24, 0x80, 0x93, 0xc0, 0x5e, 0xbc, 0xe6, 0x4e, 0xad, 0xf5, 0x89, 0x16,
    0x6e, 0x13, 0x58, 0x9a, 0x7f, 0xff, 0x04, 0x00,
];

/// The same character repertoire is used for sending and receiving text.
#[cfg(any(target_os = "none", test))]
fn supported_char(unit: u16) -> bool {
    matches!(
        unit,
        8 | 9 | 10 | 13
            | 0x20..=0x7e
            | 0xa0..=0xac
            | 0xae..=0xff
            | 0x2191
            | 0x2193
            | 0x2500..=0x256c
            | 0x2588
            | 0x2591
            | 0x25b2
            | 0x25ba
            | 0x25bc
            | 0x25c4
    )
}

/// Install the COM1 text protocols and publish all three console pointers.
///
/// # Safety
/// Call once on the BSP after initializing the UART and Boot Services.
#[cfg(target_os = "none")]
pub unsafe fn install() -> crate::uefi::status::Status {
    use crate::uefi::status::Status;

    let (input, timer) = match unsafe { text_in::prepare() } {
        Ok(value) => value,
        Err(status) => return status,
    };
    let output = unsafe { text_out::interface() };
    // Establish a known cursor position before the table is visible to images.
    let status = unsafe { ((*output).reset)(output, 0) };
    let status = if status == Status::SUCCESS {
        unsafe { crate::bs::publish_console(input, output) }
    } else {
        status
    };
    if status != Status::SUCCESS {
        unsafe { text_in::release(timer) };
    }
    status
}
