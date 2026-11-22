//! The bounded key queue shared by every console input source (COM1 and the USB keyboard) and
//! the key data it carries (UEFI 2.11, section 12.2: EFI_KEY_DATA, EFI_KEY_STATE).
//!
//! Pure logic, covered by host tests.

use super::text_in::InputKey;

/// Keys the console queue holds (XHCI_DEVICE.md section 15, item 5).
pub const KEY_QUEUE_CAPACITY: usize = 256;

/// EFI_KEY_STATE.KeyShiftState bits (UEFI 2.11, section 12.2.1).
pub mod shift_state {
    pub const VALID: u32 = 0x8000_0000;
    pub const RIGHT_SHIFT: u32 = 0x01;
    pub const LEFT_SHIFT: u32 = 0x02;
    pub const RIGHT_CONTROL: u32 = 0x04;
    pub const LEFT_CONTROL: u32 = 0x08;
    pub const RIGHT_ALT: u32 = 0x10;
    pub const LEFT_ALT: u32 = 0x20;
    pub const RIGHT_LOGO: u32 = 0x40;
    pub const LEFT_LOGO: u32 = 0x80;
}

/// EFI_KEY_STATE.KeyToggleState bits.
pub mod toggle_state {
    pub const SCROLL_LOCK: u8 = 0x01;
    pub const NUM_LOCK: u8 = 0x02;
    pub const CAPS_LOCK: u8 = 0x04;
    pub const VALID: u8 = 0x80;
    /// The lock bits a caller may set with SetState.
    pub const LOCKS: u8 = SCROLL_LOCK | NUM_LOCK | CAPS_LOCK;
}

/// EFI_KEY_STATE: shift and toggle state at the moment of the key press. All zero (no VALID
/// bit) for keys from COM1, which has no modifier state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(C)]
pub struct KeyState {
    pub shift_state: u32,
    pub toggle_state: u8,
}

/// EFI_KEY_DATA.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct KeyData {
    pub key: InputKey,
    pub state: KeyState,
}

impl KeyData {
    pub const EMPTY: Self = Self {
        key: InputKey {
            scan_code: 0,
            unicode_char: 0,
        },
        state: KeyState {
            shift_state: 0,
            toggle_state: 0,
        },
    };

    /// A key without modifier state (COM1).
    pub const fn plain(scan_code: u16, unicode_char: u16) -> Self {
        Self {
            key: InputKey {
                scan_code,
                unicode_char,
            },
            state: KeyState {
                shift_state: 0,
                toggle_state: 0,
            },
        }
    }
}

/// FIFO ring of [`KeyData`]; a full queue refuses the key (the caller keeps it).
pub struct KeyQueue {
    keys: [KeyData; KEY_QUEUE_CAPACITY],
    head: usize,
    length: usize,
}

impl KeyQueue {
    pub const fn new() -> Self {
        Self {
            keys: [KeyData::EMPTY; KEY_QUEUE_CAPACITY],
            head: 0,
            length: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Free slots.
    pub const fn room(&self) -> usize {
        KEY_QUEUE_CAPACITY - self.length
    }

    pub fn push(&mut self, key: KeyData) -> bool {
        if self.length == KEY_QUEUE_CAPACITY {
            return false;
        }
        self.keys[(self.head + self.length) % KEY_QUEUE_CAPACITY] = key;
        self.length += 1;
        true
    }

    pub fn pop(&mut self) -> Option<KeyData> {
        if self.length == 0 {
            return None;
        }
        let key = self.keys[self.head];
        self.head = (self.head + 1) % KEY_QUEUE_CAPACITY;
        self.length -= 1;
        Some(key)
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.length = 0;
    }

    /// Moves keys from `source` while this queue has room; the rest stays in `source`.
    pub fn take_from(&mut self, source: &mut KeyQueue) {
        while self.room() != 0 {
            let Some(key) = source.pop() else {
                break;
            };
            self.push(key);
        }
    }
}

impl Default for KeyQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(unicode: u16) -> KeyData {
        KeyData::plain(0, unicode)
    }

    #[test]
    fn fifo_order_capacity_and_wraparound() {
        let mut queue = KeyQueue::new();
        assert!(queue.is_empty());
        for round in 0..3u16 {
            for index in 0..KEY_QUEUE_CAPACITY as u16 {
                assert!(queue.push(key(round * 1000 + index)));
            }
            assert_eq!(queue.room(), 0);
            assert!(!queue.push(key(0xffff)));
            for index in 0..KEY_QUEUE_CAPACITY as u16 {
                assert_eq!(queue.pop(), Some(key(round * 1000 + index)));
            }
            assert_eq!(queue.pop(), None);
        }
    }

    #[test]
    fn clear_empties_and_take_from_respects_room_and_order() {
        let mut shared = KeyQueue::new();
        let mut staging = KeyQueue::new();
        for index in 0..10 {
            staging.push(key(index));
        }
        for index in 0..KEY_QUEUE_CAPACITY as u16 - 4 {
            shared.push(key(500 + index));
        }
        shared.take_from(&mut staging);
        assert_eq!(shared.room(), 0);
        assert_eq!(staging.len(), 6);
        assert_eq!(staging.pop(), Some(key(4)));
        shared.clear();
        assert!(shared.is_empty());
        assert_eq!(shared.room(), KEY_QUEUE_CAPACITY);
        shared.take_from(&mut staging);
        assert_eq!(shared.pop(), Some(key(5)));
        assert_eq!(shared.len(), 4);
    }

    #[test]
    fn key_data_matches_the_uefi_layout() {
        assert_eq!(core::mem::size_of::<KeyData>(), 12);
        assert_eq!(core::mem::offset_of!(KeyData, state), 4);
        assert_eq!(core::mem::offset_of!(KeyState, toggle_state), 4);
    }
}
