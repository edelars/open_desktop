//! HID boot-protocol keyboard report decoding into UEFI input keys (HID 1.11 appendix B.1,
//! HID Usage Tables page 0x07; XHCI_DEVICE.md section 10.3).
//!
//! Pure logic, covered by host tests. Layout: US keyboard.

use crate::console::text_in::InputKey;

/// Keys the queue holds before new ones are dropped (XHCI_DEVICE.md section 15, item 5).
pub(super) const KEY_QUEUE_CAPACITY: usize = 256;

const MOD_LEFT_CTRL: u8 = 1 << 0;
const MOD_LEFT_SHIFT: u8 = 1 << 1;
const MOD_RIGHT_CTRL: u8 = 1 << 4;
const MOD_RIGHT_SHIFT: u8 = 1 << 5;

const ERROR_ROLL_OVER: u8 = 0x01;
const USAGE_CAPS_LOCK: u8 = 0x39;

/// EFI_INPUT_KEY scan codes (UEFI 2.11, section 12.3, "EFI Scan Codes for EFI_SIMPLE_TEXT_INPUT_PROTOCOL").
mod scan {
    pub(super) const UP: u16 = 0x01;
    pub(super) const DOWN: u16 = 0x02;
    pub(super) const RIGHT: u16 = 0x03;
    pub(super) const LEFT: u16 = 0x04;
    pub(super) const HOME: u16 = 0x05;
    pub(super) const END: u16 = 0x06;
    pub(super) const INSERT: u16 = 0x07;
    pub(super) const DELETE: u16 = 0x08;
    pub(super) const PAGE_UP: u16 = 0x09;
    pub(super) const PAGE_DOWN: u16 = 0x0a;
    pub(super) const F1: u16 = 0x0b;
    pub(super) const ESCAPE: u16 = 0x17;
}

/// Boot report length (modifiers, reserved byte, six key slots).
pub(crate) const BOOT_REPORT_BYTES: usize = 8;

const fn character(unicode: u16) -> InputKey {
    InputKey {
        scan_code: 0,
        unicode_char: unicode,
    }
}

const fn function(scan_code: u16) -> InputKey {
    InputKey {
        scan_code,
        unicode_char: 0,
    }
}

/// The key a Usage ID produces: (unshifted, shifted). Letters are handled separately.
fn usage_key(usage: u8) -> Option<(InputKey, InputKey)> {
    const SHIFTED_DIGITS: &[u8; 10] = b"!@#$%^&*()";
    let same = |key| Some((key, key));
    match usage {
        0x1e..=0x26 => Some((
            character(u16::from(b'1' + (usage - 0x1e))),
            character(u16::from(SHIFTED_DIGITS[usize::from(usage - 0x1e)])),
        )),
        0x27 => Some((character(u16::from(b'0')), character(u16::from(b')')))),
        0x28 | 0x58 => same(character(0x0d)),
        0x29 => same(function(scan::ESCAPE)),
        0x2a => same(character(0x08)),
        0x2b => same(character(0x09)),
        0x2c => same(character(u16::from(b' '))),
        0x2d => Some((character(u16::from(b'-')), character(u16::from(b'_')))),
        0x2e => Some((character(u16::from(b'=')), character(u16::from(b'+')))),
        0x2f => Some((character(u16::from(b'[')), character(u16::from(b'{')))),
        0x30 => Some((character(u16::from(b']')), character(u16::from(b'}')))),
        0x31 => Some((character(u16::from(b'\\')), character(u16::from(b'|')))),
        0x33 => Some((character(u16::from(b';')), character(u16::from(b':')))),
        0x34 => Some((character(u16::from(b'\'')), character(u16::from(b'"')))),
        0x35 => Some((character(u16::from(b'`')), character(u16::from(b'~')))),
        0x36 => Some((character(u16::from(b',')), character(u16::from(b'<')))),
        0x37 => Some((character(u16::from(b'.')), character(u16::from(b'>')))),
        0x38 => Some((character(u16::from(b'/')), character(u16::from(b'?')))),
        0x3a..=0x45 => same(function(scan::F1 + u16::from(usage - 0x3a))),
        0x49 => same(function(scan::INSERT)),
        0x4a => same(function(scan::HOME)),
        0x4b => same(function(scan::PAGE_UP)),
        0x4c => same(function(scan::DELETE)),
        0x4d => same(function(scan::END)),
        0x4e => same(function(scan::PAGE_DOWN)),
        0x4f => same(function(scan::RIGHT)),
        0x50 => same(function(scan::LEFT)),
        0x51 => same(function(scan::DOWN)),
        0x52 => same(function(scan::UP)),
        // Keypad with NumLock on.
        0x54 => same(character(u16::from(b'/'))),
        0x55 => same(character(u16::from(b'*'))),
        0x56 => same(character(u16::from(b'-'))),
        0x57 => same(character(u16::from(b'+'))),
        0x59..=0x61 => same(character(u16::from(b'1' + (usage - 0x59)))),
        0x62 => same(character(u16::from(b'0'))),
        0x63 => same(character(u16::from(b'.'))),
        _ => None,
    }
}

/// Ring buffer of decoded keys.
struct KeyQueue {
    keys: [InputKey; KEY_QUEUE_CAPACITY],
    head: usize,
    length: usize,
}

impl KeyQueue {
    const fn new() -> Self {
        Self {
            keys: [character(0); KEY_QUEUE_CAPACITY],
            head: 0,
            length: 0,
        }
    }

    fn push(&mut self, key: InputKey) -> bool {
        if self.length == KEY_QUEUE_CAPACITY {
            return false;
        }
        self.keys[(self.head + self.length) % KEY_QUEUE_CAPACITY] = key;
        self.length += 1;
        true
    }

    fn pop(&mut self) -> Option<InputKey> {
        if self.length == 0 {
            return None;
        }
        let key = self.keys[self.head];
        self.head = (self.head + 1) % KEY_QUEUE_CAPACITY;
        self.length -= 1;
        Some(key)
    }
}

/// Decoder state: the previous report's keys (for "new key = Usage that was not there"),
/// CapsLock and the bounded key queue. There is no auto-repeat.
pub(crate) struct BootKeyboard {
    previous: [u8; 6],
    caps_lock: bool,
    queue: KeyQueue,
    dropped: u32,
}

impl BootKeyboard {
    pub(crate) const fn new() -> Self {
        Self {
            previous: [0; 6],
            caps_lock: false,
            queue: KeyQueue::new(),
            dropped: 0,
        }
    }

    pub(crate) fn pop(&mut self) -> Option<InputKey> {
        self.queue.pop()
    }

    #[allow(dead_code)] // WaitForKey of the ConIn integration (slice B)
    pub(crate) fn has_key(&self) -> bool {
        self.queue.length != 0
    }

    /// Keys lost because the queue was full.
    #[allow(dead_code)] // diagnostics of the ConIn integration (slice B)
    pub(crate) fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Decodes one boot report. A report whose key slots hold ErrorRollOver (more than six
    /// keys down) changes nothing; the next normal report is compared with the last good one.
    pub(crate) fn process_report(&mut self, report: &[u8; BOOT_REPORT_BYTES]) {
        let modifiers = report[0];
        let mut keys = [0u8; 6];
        keys.copy_from_slice(&report[2..8]);
        if keys.contains(&ERROR_ROLL_OVER) {
            return;
        }
        let shift = modifiers & (MOD_LEFT_SHIFT | MOD_RIGHT_SHIFT) != 0;
        let control = modifiers & (MOD_LEFT_CTRL | MOD_RIGHT_CTRL) != 0;
        for &usage in keys.iter().filter(|&&usage| usage != 0) {
            if self.previous.contains(&usage) {
                continue;
            }
            if usage == USAGE_CAPS_LOCK {
                self.caps_lock = !self.caps_lock;
                continue;
            }
            if let Some(key) = Self::translate(usage, shift, control, self.caps_lock)
                && !self.queue.push(key)
            {
                self.dropped = self.dropped.saturating_add(1);
            }
        }
        self.previous = keys;
    }

    fn translate(usage: u8, shift: bool, control: bool, caps_lock: bool) -> Option<InputKey> {
        if (0x04..=0x1d).contains(&usage) {
            let index = u16::from(usage - 0x04);
            if control {
                return Some(character(index + 1));
            }
            let upper = shift != caps_lock;
            let base = if upper { b'A' } else { b'a' };
            return Some(character(u16::from(base) + index));
        }
        let (plain, shifted) = usage_key(usage)?;
        Some(if shift { shifted } else { plain })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(modifiers: u8, keys: &[u8]) -> [u8; BOOT_REPORT_BYTES] {
        let mut bytes = [0u8; BOOT_REPORT_BYTES];
        bytes[0] = modifiers;
        bytes[2..2 + keys.len()].copy_from_slice(keys);
        bytes
    }

    fn drain(keyboard: &mut BootKeyboard) -> Vec<(u16, u16)> {
        let mut keys = Vec::new();
        while let Some(key) = keyboard.pop() {
            keys.push((key.scan_code, key.unicode_char));
        }
        keys
    }

    #[test]
    fn letters_digits_and_shifted_symbols() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(0, &[0x04]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(MOD_LEFT_SHIFT, &[0x04, 0x1e]));
        keyboard.process_report(&report(0, &[0x27, 0x1d]));
        assert_eq!(
            drain(&mut keyboard),
            [
                (0, u16::from(b'a')),
                (0, u16::from(b'A')),
                (0, u16::from(b'!')),
                (0, u16::from(b'0')),
                (0, u16::from(b'z')),
            ]
        );
    }

    #[test]
    fn only_newly_pressed_keys_produce_output_and_there_is_no_repeat() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(0, &[0x05]));
        keyboard.process_report(&report(0, &[0x05]));
        keyboard.process_report(&report(0, &[0x05, 0x06]));
        keyboard.process_report(&report(0, &[0x06]));
        keyboard.process_report(&report(0, &[0x06, 0x05]));
        assert_eq!(
            drain(&mut keyboard),
            [
                (0, u16::from(b'b')),
                (0, u16::from(b'c')),
                (0, u16::from(b'b')),
            ]
        );
    }

    #[test]
    fn control_letters_and_special_keys() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(MOD_RIGHT_CTRL, &[0x06]));
        keyboard.process_report(&report(0, &[0x28, 0x29]));
        keyboard.process_report(&report(0, &[0x2a, 0x2b, 0x2c]));
        keyboard.process_report(&report(0, &[0x52, 0x51, 0x4f, 0x50]));
        keyboard.process_report(&report(0, &[0x3a, 0x45, 0x4c, 0x4a]));
        assert_eq!(
            drain(&mut keyboard),
            [
                (0, 3),
                (0, 0x0d),
                (scan::ESCAPE, 0),
                (0, 0x08),
                (0, 0x09),
                (0, u16::from(b' ')),
                (scan::UP, 0),
                (scan::DOWN, 0),
                (scan::RIGHT, 0),
                (scan::LEFT, 0),
                (scan::F1, 0),
                (scan::F1 + 11, 0),
                (scan::DELETE, 0),
                (scan::HOME, 0),
            ]
        );
    }

    #[test]
    fn caps_lock_toggles_letters_and_xors_with_shift() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(0, &[USAGE_CAPS_LOCK]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[0x04]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(MOD_RIGHT_SHIFT, &[0x04]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[USAGE_CAPS_LOCK]));
        keyboard.process_report(&report(0, &[0x04]));
        assert_eq!(
            drain(&mut keyboard),
            [
                (0, u16::from(b'A')),
                (0, u16::from(b'a')),
                (0, u16::from(b'a')),
            ]
        );
    }

    #[test]
    fn error_roll_over_and_unknown_usages_are_ignored() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(0, &[0x04]));
        // Phantom state: the report is dropped and the previous keys stay remembered.
        keyboard.process_report(&report(0, &[1, 1, 1, 1, 1, 1]));
        keyboard.process_report(&report(0, &[0x04, 0x05]));
        // 0x32 (non-US #), 0x65 (Application) and 0xff have no key.
        keyboard.process_report(&report(0, &[0x32, 0x65, 0xff]));
        assert_eq!(
            drain(&mut keyboard),
            [(0, u16::from(b'a')), (0, u16::from(b'b'))]
        );
    }

    #[test]
    fn modifier_only_reports_are_silent_and_keypad_uses_digits() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(MOD_LEFT_CTRL | MOD_LEFT_SHIFT, &[]));
        assert!(!keyboard.has_key());
        keyboard.process_report(&report(0, &[0x59, 0x62, 0x58]));
        assert_eq!(
            drain(&mut keyboard),
            [(0, u16::from(b'1')), (0, u16::from(b'0')), (0, 0x0d)]
        );
    }

    #[test]
    fn full_queue_drops_new_keys_and_counts_them() {
        let mut keyboard = BootKeyboard::new();
        for round in 0..KEY_QUEUE_CAPACITY + 3 {
            keyboard.process_report(&report(0, &[0x04 + (round % 2) as u8]));
        }
        assert_eq!(keyboard.dropped(), 3);
        let first = keyboard.pop().unwrap();
        assert_eq!(first.unicode_char, u16::from(b'a'));
        let mut remaining = 1;
        while keyboard.pop().is_some() {
            remaining += 1;
        }
        assert_eq!(remaining, KEY_QUEUE_CAPACITY);
        assert!(!keyboard.has_key());
    }
}
