//! HID boot-protocol keyboard report decoding into UEFI key data (HID 1.11 appendix B.1,
//! HID Usage Tables page 0x07; XHCI_DEVICE.md section 10.3).
//!
//! Pure logic, covered by host tests. Layout: US keyboard.

use crate::console::key_queue::{
    KEY_QUEUE_CAPACITY, KeyData, KeyQueue, KeyState, shift_state, toggle_state,
};
use crate::console::text_in::InputKey;

const MOD_LEFT_CTRL: u8 = 1 << 0;
const MOD_LEFT_SHIFT: u8 = 1 << 1;
const MOD_LEFT_ALT: u8 = 1 << 2;
const MOD_LEFT_GUI: u8 = 1 << 3;
const MOD_RIGHT_CTRL: u8 = 1 << 4;
const MOD_RIGHT_SHIFT: u8 = 1 << 5;
const MOD_RIGHT_ALT: u8 = 1 << 6;
const MOD_RIGHT_GUI: u8 = 1 << 7;

const ERROR_ROLL_OVER: u8 = 0x01;
const USAGE_SCROLL_LOCK: u8 = 0x47;
const USAGE_CAPS_LOCK: u8 = 0x39;
const USAGE_NUM_LOCK: u8 = 0x53;

/// HID keyboard Output report bits (LEDs page: NumLock 1, CapsLock 2, ScrollLock 3).
const LED_NUM_LOCK: u8 = 1 << 0;
const LED_CAPS_LOCK: u8 = 1 << 1;
const LED_SCROLL_LOCK: u8 = 1 << 2;

/// Keys one boot report can add (six key slots).
pub(crate) const KEYS_PER_REPORT: usize = 6;

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

/// The key a Usage ID produces: (unshifted, shifted). Letters and the keypad digits are
/// handled separately.
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
        0x54 => same(character(u16::from(b'/'))),
        0x55 => same(character(u16::from(b'*'))),
        0x56 => same(character(u16::from(b'-'))),
        0x57 => same(character(u16::from(b'+'))),
        _ => None,
    }
}

/// Keypad digits and the point (Usage 0x59..=0x63): characters with NumLock on, navigation
/// keys with NumLock off (5 has no navigation function).
fn keypad_key(usage: u8, num_lock: bool) -> Option<InputKey> {
    if num_lock {
        return Some(match usage {
            0x59..=0x61 => character(u16::from(b'1' + (usage - 0x59))),
            0x62 => character(u16::from(b'0')),
            0x63 => character(u16::from(b'.')),
            _ => return None,
        });
    }
    Some(function(match usage {
        0x59 => scan::END,
        0x5a => scan::DOWN,
        0x5b => scan::PAGE_DOWN,
        0x5c => scan::LEFT,
        0x5e => scan::RIGHT,
        0x5f => scan::HOME,
        0x60 => scan::UP,
        0x61 => scan::PAGE_UP,
        0x62 => scan::INSERT,
        0x63 => scan::DELETE,
        _ => return None,
    }))
}

/// EFI_KEY_STATE.KeyShiftState for the HID modifier byte.
pub(crate) fn shift_state_of(modifiers: u8) -> u32 {
    const MAP: [(u8, u32); 8] = [
        (MOD_LEFT_CTRL, shift_state::LEFT_CONTROL),
        (MOD_LEFT_SHIFT, shift_state::LEFT_SHIFT),
        (MOD_LEFT_ALT, shift_state::LEFT_ALT),
        (MOD_LEFT_GUI, shift_state::LEFT_LOGO),
        (MOD_RIGHT_CTRL, shift_state::RIGHT_CONTROL),
        (MOD_RIGHT_SHIFT, shift_state::RIGHT_SHIFT),
        (MOD_RIGHT_ALT, shift_state::RIGHT_ALT),
        (MOD_RIGHT_GUI, shift_state::RIGHT_LOGO),
    ];
    let mut state = shift_state::VALID;
    for (bit, flag) in MAP {
        if modifiers & bit != 0 {
            state |= flag;
        }
    }
    state
}

/// Decoder state: the previous report's keys (for "new key = Usage that was not there"), the
/// lock toggles, the staging queue and a pending LED update. There is no auto-repeat.
///
/// The staging queue is the driver's own: reports that arrive while the driver waits for a
/// control transfer land here, and the console moves them into its shared queue.
pub(crate) struct BootKeyboard {
    previous: [u8; 6],
    toggles: u8,
    led_pending: bool,
    queue: KeyQueue,
    dropped: u32,
}

impl BootKeyboard {
    pub(crate) const fn new() -> Self {
        Self {
            previous: [0; 6],
            toggles: 0,
            led_pending: false,
            queue: KeyQueue::new(),
            dropped: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn pop(&mut self) -> Option<KeyData> {
        self.queue.pop()
    }

    #[cfg(test)]
    pub(crate) fn has_key(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Staging slots left; the driver stops taking reports from the event ring below
    /// [`KEYS_PER_REPORT`], so a full console queue backs up into the ring, not into drops.
    pub(crate) fn room(&self) -> usize {
        self.queue.room()
    }

    /// Moves staged keys into the console queue while it has room.
    pub(crate) fn drain_into(&mut self, queue: &mut KeyQueue) {
        queue.take_from(&mut self.queue);
    }

    /// Keys lost because the staging queue was full.
    #[allow(dead_code)]
    pub(crate) fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Current lock toggles ([`toggle_state`] LOCKS bits, no VALID bit).
    pub(crate) fn toggles(&self) -> u8 {
        self.toggles
    }

    /// SetState: takes the lock bits of `state` and queues an LED update.
    pub(crate) fn set_toggles(&mut self, state: u8) {
        self.toggles = state & toggle_state::LOCKS;
        self.led_pending = true;
    }

    /// The Output report to send when a lock changed since the last call (LED update queue of
    /// ISSUE-647: it is sent by the driver outside any other transfer).
    pub(crate) fn take_led(&mut self) -> Option<u8> {
        if !core::mem::take(&mut self.led_pending) {
            return None;
        }
        let mut report = 0;
        for (toggle, led) in [
            (toggle_state::NUM_LOCK, LED_NUM_LOCK),
            (toggle_state::CAPS_LOCK, LED_CAPS_LOCK),
            (toggle_state::SCROLL_LOCK, LED_SCROLL_LOCK),
        ] {
            if self.toggles & toggle != 0 {
                report |= led;
            }
        }
        Some(report)
    }

    /// Forgets pressed keys and staged keys (ConIn Reset); lock state is kept.
    pub(crate) fn reset(&mut self) {
        self.previous = [0; 6];
        self.queue.clear();
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
            let lock = match usage {
                USAGE_CAPS_LOCK => Some(toggle_state::CAPS_LOCK),
                USAGE_NUM_LOCK => Some(toggle_state::NUM_LOCK),
                USAGE_SCROLL_LOCK => Some(toggle_state::SCROLL_LOCK),
                _ => None,
            };
            if let Some(lock) = lock {
                self.toggles ^= lock;
                self.led_pending = true;
                continue;
            }
            let caps = self.toggles & toggle_state::CAPS_LOCK != 0;
            let num = self.toggles & toggle_state::NUM_LOCK != 0;
            if let Some(key) = Self::translate(usage, shift, control, caps, num) {
                let mut shifts = shift_state_of(modifiers);
                // UEFI 2.11 12.3.3 (as EDK2): Shift is already in a printable character, so the
                // shift bits are not passed along with it; keys without a character keep them.
                if (0x20..0x7f).contains(&key.unicode_char) || key.unicode_char >= 0xa0 {
                    shifts &= !(shift_state::LEFT_SHIFT | shift_state::RIGHT_SHIFT);
                }
                let data = KeyData {
                    key,
                    state: KeyState {
                        shift_state: shifts,
                        toggle_state: toggle_state::VALID | self.toggles,
                    },
                };
                if !self.queue.push(data) {
                    self.dropped = self.dropped.saturating_add(1);
                }
            }
        }
        self.previous = keys;
    }

    fn translate(
        usage: u8,
        shift: bool,
        control: bool,
        caps_lock: bool,
        num_lock: bool,
    ) -> Option<InputKey> {
        if (0x04..=0x1d).contains(&usage) {
            let index = u16::from(usage - 0x04);
            if control {
                return Some(character(index + 1));
            }
            let upper = shift != caps_lock;
            let base = if upper { b'A' } else { b'a' };
            return Some(character(u16::from(base) + index));
        }
        if (0x59..=0x63).contains(&usage) {
            return keypad_key(usage, num_lock);
        }
        let (plain, shifted) = usage_key(usage)?;
        Some(if shift { shifted } else { plain })
    }
}

const _: () = assert!(KEYS_PER_REPORT <= KEY_QUEUE_CAPACITY);

#[cfg(test)]
mod tests {
    use super::*;

    const LEDS_NONE: u8 = 0;

    fn report(modifiers: u8, keys: &[u8]) -> [u8; BOOT_REPORT_BYTES] {
        let mut bytes = [0u8; BOOT_REPORT_BYTES];
        bytes[0] = modifiers;
        bytes[2..2 + keys.len()].copy_from_slice(keys);
        bytes
    }

    fn drain(keyboard: &mut BootKeyboard) -> Vec<(u16, u16)> {
        let mut keys = Vec::new();
        while let Some(data) = keyboard.pop() {
            keys.push((data.key.scan_code, data.key.unicode_char));
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
    fn caps_lock_toggles_letters_xors_with_shift_and_queues_an_led_update() {
        let mut keyboard = BootKeyboard::new();
        assert_eq!(keyboard.take_led(), None);
        keyboard.process_report(&report(0, &[USAGE_CAPS_LOCK]));
        assert_eq!(keyboard.take_led(), Some(LED_CAPS_LOCK));
        assert_eq!(keyboard.take_led(), None, "one update per change");
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[0x04]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(MOD_RIGHT_SHIFT, &[0x04]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[USAGE_CAPS_LOCK]));
        keyboard.process_report(&report(0, &[0x04]));
        assert_eq!(keyboard.take_led(), Some(LEDS_NONE));
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
    fn num_and_scroll_lock_set_leds_and_numlock_selects_the_keypad_function() {
        let mut keyboard = BootKeyboard::new();
        // NumLock off: the keypad navigates.
        keyboard.process_report(&report(0, &[0x59, 0x5e, 0x62, 0x5d]));
        assert_eq!(
            drain(&mut keyboard),
            [(scan::END, 0), (scan::RIGHT, 0), (scan::INSERT, 0)]
        );
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[USAGE_NUM_LOCK]));
        assert_eq!(keyboard.take_led(), Some(LED_NUM_LOCK));
        keyboard.process_report(&report(0, &[USAGE_SCROLL_LOCK]));
        assert_eq!(keyboard.take_led(), Some(LED_NUM_LOCK | LED_SCROLL_LOCK));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(0, &[0x59, 0x62, 0x63, 0x58, 0x54]));
        assert_eq!(
            drain(&mut keyboard),
            [
                (0, u16::from(b'1')),
                (0, u16::from(b'0')),
                (0, u16::from(b'.')),
                (0, 0x0d),
                (0, u16::from(b'/')),
            ]
        );
        assert_eq!(
            keyboard.toggles(),
            toggle_state::NUM_LOCK | toggle_state::SCROLL_LOCK
        );
    }

    #[test]
    fn set_state_replaces_the_locks_and_queues_one_led_update() {
        let mut keyboard = BootKeyboard::new();
        keyboard.set_toggles(toggle_state::VALID | toggle_state::CAPS_LOCK | 0x40);
        assert_eq!(keyboard.toggles(), toggle_state::CAPS_LOCK);
        assert_eq!(keyboard.take_led(), Some(LED_CAPS_LOCK));
        keyboard.set_toggles(0);
        assert_eq!(keyboard.take_led(), Some(0));
    }

    #[test]
    fn key_state_carries_modifiers_and_toggles() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(
            MOD_LEFT_CTRL | MOD_RIGHT_ALT | MOD_LEFT_GUI,
            &[0x04],
        ));
        let data = keyboard.pop().unwrap();
        assert_eq!(
            data.state.shift_state,
            shift_state::VALID
                | shift_state::LEFT_CONTROL
                | shift_state::RIGHT_ALT
                | shift_state::LEFT_LOGO
        );
        assert_eq!(data.state.toggle_state, toggle_state::VALID);
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(
            MOD_RIGHT_SHIFT | MOD_LEFT_SHIFT,
            &[USAGE_CAPS_LOCK, 0x05],
        ));
        let data = keyboard.pop().unwrap();
        // Shift is applied (here Shift and CapsLock cancel out: 'b'): a printable character
        // comes without the shift bits (UEFI 2.11 12.3.3).
        assert_eq!(data.key.unicode_char, u16::from(b'b'));
        assert_eq!(data.state.shift_state, shift_state::VALID);
        assert_eq!(
            data.state.toggle_state,
            toggle_state::VALID | toggle_state::CAPS_LOCK
        );
        assert_eq!(shift_state_of(0), shift_state::VALID);
    }

    #[test]
    fn shift_bits_stay_with_keys_that_have_no_printable_character() {
        let mut keyboard = BootKeyboard::new();
        // Shift+1 -> '!' without shift bits; Shift+F1, Shift+Up and Shift+Enter keep them; the
        // Ctrl+Shift+letter control code keeps both.
        keyboard.process_report(&report(MOD_LEFT_SHIFT, &[0x1e, 0x3a, 0x52, 0x28]));
        keyboard.process_report(&report(0, &[]));
        keyboard.process_report(&report(MOD_LEFT_SHIFT | MOD_RIGHT_CTRL, &[0x04]));
        let mut seen = Vec::new();
        while let Some(data) = keyboard.pop() {
            seen.push((data.key.unicode_char, data.key.scan_code, data.state.shift_state));
        }
        let shifted = shift_state::VALID | shift_state::LEFT_SHIFT;
        assert_eq!(
            seen,
            [
                (u16::from(b'!'), 0, shift_state::VALID),
                (0, scan::F1, shifted),
                (0, scan::UP, shifted),
                (0x0d, 0, shifted),
                (1, 0, shifted | shift_state::RIGHT_CONTROL),
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
    fn modifier_only_reports_are_silent() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(MOD_LEFT_CTRL | MOD_LEFT_SHIFT, &[]));
        assert!(!keyboard.has_key());
    }

    #[test]
    fn full_staging_queue_drops_new_keys_and_counts_them() {
        let mut keyboard = BootKeyboard::new();
        for round in 0..KEY_QUEUE_CAPACITY + 3 {
            keyboard.process_report(&report(0, &[0x04 + (round % 2) as u8]));
        }
        assert_eq!(keyboard.dropped(), 3);
        assert_eq!(keyboard.room(), 0);
        assert_eq!(keyboard.pop().unwrap().key.unicode_char, u16::from(b'a'));
        assert_eq!(keyboard.room(), 1);
    }

    #[test]
    fn drain_moves_keys_in_order_and_stops_when_the_console_queue_is_full() {
        let mut keyboard = BootKeyboard::new();
        for round in 0..10 {
            keyboard.process_report(&report(0, &[0x04 + (round % 2) as u8]));
        }
        let mut console = KeyQueue::new();
        for _ in 0..KEY_QUEUE_CAPACITY - 4 {
            console.push(KeyData::plain(0, u16::from(b'x')));
        }
        keyboard.drain_into(&mut console);
        assert_eq!(console.room(), 0);
        assert!(keyboard.has_key());
        assert_eq!(keyboard.room(), KEY_QUEUE_CAPACITY - 6);
        // The undelivered keys keep their order for the next drain.
        console.clear();
        keyboard.drain_into(&mut console);
        let keys: Vec<u16> = std::iter::from_fn(|| console.pop())
            .map(|data| data.key.unicode_char)
            .collect();
        assert_eq!(keys, [b'a', b'b', b'a', b'b', b'a', b'b'].map(u16::from));
    }

    #[test]
    fn reset_forgets_pressed_keys_and_staged_keys_but_keeps_the_locks() {
        let mut keyboard = BootKeyboard::new();
        keyboard.process_report(&report(0, &[USAGE_CAPS_LOCK, 0x04]));
        keyboard.reset();
        assert!(!keyboard.has_key());
        assert_eq!(keyboard.toggles(), toggle_state::CAPS_LOCK);
        // The key that was down before the reset counts as new again.
        keyboard.process_report(&report(0, &[0x04]));
        assert_eq!(drain(&mut keyboard), [(0, u16::from(b'A'))]);
    }
}
