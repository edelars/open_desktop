//! UEFI Simple Text Output backed by the COM1 terminal.

use crate::uefi::{guid::Guid, status::Status, types::Char16};

pub const SIMPLE_TEXT_OUTPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x3874_77c2,
    data2: 0x69c7,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

#[repr(C)]
pub struct SimpleTextOutputProtocol {
    pub reset: unsafe extern "efiapi" fn(*mut Self, u8) -> Status,
    pub output_string: unsafe extern "efiapi" fn(*mut Self, *const Char16) -> Status,
    pub test_string: unsafe extern "efiapi" fn(*mut Self, *const Char16) -> Status,
    pub query_mode: unsafe extern "efiapi" fn(*mut Self, usize, *mut usize, *mut usize) -> Status,
    pub set_mode: unsafe extern "efiapi" fn(*mut Self, usize) -> Status,
    pub set_attribute: unsafe extern "efiapi" fn(*mut Self, usize) -> Status,
    pub clear_screen: unsafe extern "efiapi" fn(*mut Self) -> Status,
    pub set_cursor_position: unsafe extern "efiapi" fn(*mut Self, usize, usize) -> Status,
    pub enable_cursor: unsafe extern "efiapi" fn(*mut Self, u8) -> Status,
    pub mode: *mut SimpleTextOutputMode,
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct SimpleTextOutputMode {
    pub max_mode: i32,
    pub mode: i32,
    pub attribute: i32,
    pub cursor_column: i32,
    pub cursor_row: i32,
    pub cursor_visible: u8,
}

#[cfg(target_os = "none")]
pub(super) use terminal::interface;

#[cfg(target_os = "none")]
mod terminal {
    use super::*;
    use crate::bs::event::poll_due_events;
    use crate::console::{serial_term::write_checked, supported_char};
    use core::{cell::UnsafeCell, ptr};

    const COLUMNS: usize = 80;
    const ROWS: usize = 25;
    const CLEAR: &[u8] = b"\x1b[2J\x1b[H";
    const RESET: &[u8] = b"\x1b[0m\x1b[?25h\x1b[2J\x1b[H";
    const CURSOR_DIGITS: [[u8; 2]; COLUMNS + 1] = {
        let mut digits = [[b'0'; 2]; COLUMNS + 1];
        let mut index = 1;
        while index <= COLUMNS {
            digits[index] = [b'0' + (index / 10) as u8, b'0' + (index % 10) as u8];
            index += 1;
        }
        digits
    };
    const INITIAL_MODE: SimpleTextOutputMode = SimpleTextOutputMode {
        max_mode: 1,
        mode: 0,
        attribute: 7,
        cursor_column: 0,
        cursor_row: 0,
        cursor_visible: 1,
    };

    struct ConsoleOutput {
        protocol: UnsafeCell<SimpleTextOutputProtocol>,
        mode: UnsafeCell<SimpleTextOutputMode>,
    }

    // The BSP owns the mode and publishes one stable read-only EFI interface.
    unsafe impl Sync for ConsoleOutput {}

    static OUTPUT: ConsoleOutput = ConsoleOutput {
        protocol: UnsafeCell::new(SimpleTextOutputProtocol {
            reset,
            output_string,
            test_string,
            query_mode,
            set_mode,
            set_attribute,
            clear_screen,
            set_cursor_position,
            enable_cursor,
            mode: ptr::null_mut(),
        }),
        mode: UnsafeCell::new(INITIAL_MODE),
    };

    /// Call once on the BSP before the interface is published.
    pub(in super::super) unsafe fn interface() -> *mut SimpleTextOutputProtocol {
        let protocol = OUTPUT.protocol.get();
        unsafe { ptr::addr_of_mut!((*protocol).mode).write(OUTPUT.mode.get()) };
        protocol
    }

    unsafe extern "efiapi" fn reset(this: *mut SimpleTextOutputProtocol, _: u8) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        if !write_checked(RESET) {
            return Status::DEVICE_ERROR;
        }
        unsafe { OUTPUT.mode.get().write(INITIAL_MODE) };
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn query_mode(
        this: *mut SimpleTextOutputProtocol,
        mode: usize,
        columns: *mut usize,
        rows: *mut usize,
    ) -> Status {
        if this != OUTPUT.protocol.get() || columns.is_null() || rows.is_null() || columns == rows {
            return Status::INVALID_PARAMETER;
        }
        if mode != 0 {
            return Status::UNSUPPORTED;
        }
        unsafe {
            columns.write(COLUMNS);
            rows.write(ROWS);
        }
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn set_mode(this: *mut SimpleTextOutputProtocol, mode: usize) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        if mode != 0 {
            return Status::UNSUPPORTED;
        }
        unsafe { clear_screen(this) }
    }

    unsafe extern "efiapi" fn set_attribute(
        this: *mut SimpleTextOutputProtocol,
        attribute: usize,
    ) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        if attribute > 0x7f {
            return Status::UNSUPPORTED;
        }
        const TO_ANSI: [u8; 8] = *b"04261537";
        let foreground = if attribute & 8 != 0 { b'9' } else { b'3' };
        let sequence = [
            0x1b,
            b'[',
            foreground,
            TO_ANSI[attribute & 7],
            b';',
            b'4',
            TO_ANSI[attribute >> 4],
            b'm',
        ];
        if !write_checked(&sequence) {
            return Status::DEVICE_ERROR;
        }
        unsafe { ptr::addr_of_mut!((*OUTPUT.mode.get()).attribute).write(attribute as i32) };
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn clear_screen(this: *mut SimpleTextOutputProtocol) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        unsafe { poll_due_events() };
        if !write_checked(CLEAR) {
            return Status::DEVICE_ERROR;
        }
        unsafe { set_position(0, 0) };
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn set_cursor_position(
        this: *mut SimpleTextOutputProtocol,
        column: usize,
        row: usize,
    ) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        if column >= COLUMNS || row >= ROWS {
            return Status::UNSUPPORTED;
        }
        unsafe { poll_due_events() };
        let [y_tens, y_units] = CURSOR_DIGITS[row + 1];
        let [x_tens, x_units] = CURSOR_DIGITS[column + 1];
        let sequence = [0x1b, b'[', y_tens, y_units, b';', x_tens, x_units, b'H'];
        if !write_checked(&sequence) {
            return Status::DEVICE_ERROR;
        }
        unsafe { set_position(column as i32, row as i32) };
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn enable_cursor(
        this: *mut SimpleTextOutputProtocol,
        visible: u8,
    ) -> Status {
        if this != OUTPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        let visible = visible != 0;
        let sequence = if visible { b"\x1b[?25h" } else { b"\x1b[?25l" };
        if !write_checked(sequence) {
            return Status::DEVICE_ERROR;
        }
        unsafe { ptr::addr_of_mut!((*OUTPUT.mode.get()).cursor_visible).write(u8::from(visible)) };
        Status::SUCCESS
    }

    unsafe extern "efiapi" fn output_string(
        this: *mut SimpleTextOutputProtocol,
        mut text: *const Char16,
    ) -> Status {
        if this != OUTPUT.protocol.get() || text.is_null() {
            return Status::INVALID_PARAMETER;
        }
        unsafe { poll_due_events() };
        let mut warning = false;
        loop {
            let unit = unsafe { text.read() };
            if unit == 0 {
                return if warning {
                    Status::WARN_UNKNOWN_GLYPH
                } else {
                    Status::SUCCESS
                };
            }
            let printable = if supported_char(unit) {
                unit
            } else {
                warning = true;
                u16::from(b'?')
            };
            if !unsafe { output_unit(printable) } {
                return Status::DEVICE_ERROR;
            }
            text = unsafe { text.add(1) };
        }
    }

    unsafe extern "efiapi" fn test_string(
        this: *mut SimpleTextOutputProtocol,
        mut text: *const Char16,
    ) -> Status {
        if this != OUTPUT.protocol.get() || text.is_null() {
            return Status::INVALID_PARAMETER;
        }
        loop {
            let unit = unsafe { text.read() };
            if unit == 0 {
                return Status::SUCCESS;
            }
            if !supported_char(unit) {
                return Status::UNSUPPORTED;
            }
            text = unsafe { text.add(1) };
        }
    }

    unsafe fn output_unit(unit: Char16) -> bool {
        let mode = unsafe { OUTPUT.mode.get().read() };
        let (column, row) = (mode.cursor_column, mode.cursor_row);
        let last_row = ROWS as i32 - 1;
        let (column, row) = match unit {
            8 if column == 0 => return true,
            8 if write_checked(b"\x08") => (column - 1, row),
            10 if write_checked(b"\n") => (column, (row + 1).min(last_row)),
            13 if write_checked(b"\r") => (0, row),
            8 | 10 | 13 => return false,
            9 => {
                let mut spaces = 8 - column % 8;
                while spaces != 0 {
                    if !unsafe { output_unit(u16::from(b' ')) } {
                        return false;
                    }
                    spaces -= 1;
                }
                return true;
            }
            _ => {
                let ch = char::from_u32(u32::from(unit)).unwrap_or('?');
                let mut bytes = [0; 4];
                if !write_checked(ch.encode_utf8(&mut bytes).as_bytes()) {
                    return false;
                }
                if column + 1 == COLUMNS as i32 {
                    if !write_checked(b"\r\n") {
                        return false;
                    }
                    (0, (row + 1).min(last_row))
                } else {
                    (column + 1, row)
                }
            }
        };
        unsafe { set_position(column, row) };
        true
    }

    unsafe fn set_position(column: i32, row: i32) {
        let mode = OUTPUT.mode.get();
        unsafe {
            ptr::addr_of_mut!((*mode).cursor_column).write(column);
            ptr::addr_of_mut!((*mode).cursor_row).write(row);
        }
    }
}
