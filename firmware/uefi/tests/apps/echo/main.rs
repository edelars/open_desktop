#![no_std]
#![no_main]

use core::ptr;
use od_firmware::console::text_in::{InputKey, SimpleTextInputProtocol};
use od_firmware::uefi::{
    status::Status,
    system_table::SystemTable,
    types::{Char16, Handle},
};
use od_firmware_test_apps::{ascii_to_char16, output};

static BANNER: [Char16; 15] = ascii_to_char16(b"OD-UEFI echo\r\n\0");
static BANNER_ADDRESS: &[Char16; 15] = &BANNER;
const UP: [Char16; 5] = ascii_to_char16(b"[UP]\0");
const DOWN: [Char16; 7] = ascii_to_char16(b"[DOWN]\0");
const RIGHT: [Char16; 8] = ascii_to_char16(b"[RIGHT]\0");
const LEFT: [Char16; 7] = ascii_to_char16(b"[LEFT]\0");
const ESC: [Char16; 6] = ascii_to_char16(b"[ESC]\0");
const NEWLINE: [Char16; 3] = ascii_to_char16(b"\r\n\0");

#[unsafe(no_mangle)]
unsafe extern "efiapi" fn efi_main(handle: Handle, system: *mut SystemTable) -> Status {
    if handle.is_null() || system.is_null() {
        return Status::INVALID_PARAMETER;
    }
    unsafe {
        let input = (*system).console_in().cast::<SimpleTextInputProtocol>();
        let boot = (*system).boot_services;
        if input.is_null() || boot.is_null() || (*system).console_out().is_null() {
            return Status::NOT_READY;
        }
        let banner = ptr::read_volatile(&BANNER_ADDRESS);
        let status = output(system, banner.as_ptr());
        if status != Status::SUCCESS {
            return status;
        }
        let event = (*input).wait_for_key;
        loop {
            let mut index = usize::MAX;
            let status = ((*boot).wait_for_event)(1, &event, &mut index);
            if status != Status::SUCCESS {
                return status;
            }
            let mut key = InputKey {
                scan_code: 0,
                unicode_char: 0,
            };
            let status = ((*input).read_key_stroke)(input, &mut key);
            if status == Status::NOT_READY {
                continue;
            }
            if status != Status::SUCCESS {
                return status;
            }
            if key.scan_code == 0 && matches!(key.unicode_char, 10 | 13) {
                return output(system, NEWLINE.as_ptr());
            }
            let character = [key.unicode_char, 0];
            let text: &[Char16] = match key.scan_code {
                0 => &character,
                1 => &UP,
                2 => &DOWN,
                3 => &RIGHT,
                4 => &LEFT,
                0x17 => &ESC,
                _ => continue,
            };
            let status = output(system, text.as_ptr());
            if status != Status::SUCCESS {
                return status;
            }
        }
    }
}
