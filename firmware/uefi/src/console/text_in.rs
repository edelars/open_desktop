//! UEFI Simple Text Input and its bounded COM1 byte parser.

use crate::{
    bs::event::Event,
    uefi::{guid::Guid, status::Status, types::Char16},
};

pub const SIMPLE_TEXT_INPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x3874_77c1,
    data2: 0x69c7,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(C)]
pub struct InputKey {
    pub scan_code: u16,
    pub unicode_char: Char16,
}

#[repr(C)]
pub struct SimpleTextInputProtocol {
    pub reset: unsafe extern "efiapi" fn(*mut Self, u8) -> Status,
    pub read_key_stroke: unsafe extern "efiapi" fn(*mut Self, *mut InputKey) -> Status,
    pub wait_for_key: Event,
}

#[cfg(any(target_os = "none", test))]
mod decoder {
    use super::InputKey;
    use crate::console::supported_char;

    const KEY_CAPACITY: usize = 16;
    const ESCAPE: u8 = 0x1b;
    const SCAN_ESCAPE: u16 = 0x17;
    const ESCAPE_TICKS: u8 = 2;
    const EMPTY_KEY: InputKey = InputKey {
        scan_code: 0,
        unicode_char: 0,
    };

    #[derive(Clone, Copy)]
    enum Sequence {
        Ready,
        Escape { prefix: bool, age: u8 },
        Utf8 { value: u16, minimum: u16, left: u8 },
    }

    pub(super) struct Decoder {
        keys: [InputKey; KEY_CAPACITY],
        head: usize,
        length: usize,
        sequence: Sequence,
    }

    impl Decoder {
        pub(super) const fn new() -> Self {
            Self {
                keys: [EMPTY_KEY; KEY_CAPACITY],
                head: 0,
                length: 0,
                sequence: Sequence::Ready,
            }
        }

        pub(super) fn has_key(&self) -> bool {
            self.length != 0
        }

        /// The current byte and a preceding unrecognized escape can each
        /// make one key available.
        pub(super) fn can_receive(&self) -> bool {
            self.length <= KEY_CAPACITY - 2
        }

        pub(super) fn pop(&mut self) -> Option<InputKey> {
            if self.length == 0 {
                return None;
            }
            let key = self.keys[self.head];
            self.head = (self.head + 1) % KEY_CAPACITY;
            self.length -= 1;
            Some(key)
        }

        pub(super) fn tick(&mut self) {
            let Sequence::Escape { prefix, age } = self.sequence else {
                return;
            };
            let age = age.saturating_add(1);
            if age >= ESCAPE_TICKS && self.length < KEY_CAPACITY {
                self.sequence = Sequence::Ready;
                self.push(SCAN_ESCAPE, 0);
            } else {
                self.sequence = Sequence::Escape { prefix, age };
            }
        }

        pub(super) fn feed(&mut self, byte: u8) {
            match core::mem::replace(&mut self.sequence, Sequence::Ready) {
                Sequence::Ready => self.first_byte(byte),
                Sequence::Escape { prefix: false, age } if matches!(byte, b'[' | b'O') => {
                    self.sequence = Sequence::Escape { prefix: true, age };
                }
                Sequence::Escape { prefix: true, .. } if (b'A'..=b'D').contains(&byte) => {
                    const SCAN: [u16; 4] = [1, 2, 3, 4];
                    self.push(SCAN[usize::from(byte - b'A')], 0);
                }
                Sequence::Escape { .. } => {
                    self.push(SCAN_ESCAPE, 0);
                    self.first_byte(byte);
                }
                Sequence::Utf8 {
                    value,
                    minimum,
                    left,
                } if byte & 0xc0 == 0x80 => {
                    let value = (value << 6) | u16::from(byte & 0x3f);
                    if left == 2 {
                        self.sequence = Sequence::Utf8 {
                            value,
                            minimum,
                            left: 1,
                        };
                    } else if value >= minimum && supported_char(value) {
                        self.push(0, value);
                    }
                }
                Sequence::Utf8 { .. } => self.first_byte(byte),
            }
        }

        fn first_byte(&mut self, byte: u8) {
            match byte {
                ESCAPE => {
                    self.sequence = Sequence::Escape {
                        prefix: false,
                        age: 0,
                    }
                }
                0x7f => self.push(0, 8),
                0x01..=0x7e if supported_char(u16::from(byte)) => self.push(0, u16::from(byte)),
                0xc2..=0xdf => {
                    self.sequence = Sequence::Utf8 {
                        value: u16::from(byte & 0x1f),
                        minimum: 0x80,
                        left: 1,
                    }
                }
                0xe0..=0xef => {
                    self.sequence = Sequence::Utf8 {
                        value: u16::from(byte & 0x0f),
                        minimum: 0x800,
                        left: 2,
                    }
                }
                _ => {}
            }
        }

        fn push(&mut self, scan_code: u16, unicode_char: u16) {
            let index = (self.head + self.length) % KEY_CAPACITY;
            self.keys[index] = InputKey {
                scan_code,
                unicode_char,
            };
            self.length += 1;
        }
    }
}

#[cfg(target_os = "none")]
pub(super) use terminal::{prepare, release};

#[cfg(target_os = "none")]
mod terminal {
    use super::{decoder::Decoder, *};
    use crate::bs::event::{
        EVT_NOTIFY_SIGNAL, EVT_NOTIFY_WAIT, EVT_TIMER, TIMER_PERIODIC, TPL_CALLBACK, TPL_NOTIFY,
        check_event, close_event, create_event, poll_due_events, set_timer, signal_event,
    };
    use crate::console::serial_term::receive_checked;
    use core::{cell::UnsafeCell, ffi::c_void, ptr};

    const POLL_100NS: u64 = 100_000;
    const POLL_BYTES: usize = 32;

    struct InputState {
        decoder: Decoder,
        fault: bool,
    }

    struct ConsoleInput {
        protocol: UnsafeCell<SimpleTextInputProtocol>,
        state: UnsafeCell<InputState>,
    }

    // Only BSP code can access these cells. Event dispatch runs after input
    // state borrows have been released.
    unsafe impl Sync for ConsoleInput {}

    static INPUT: ConsoleInput = ConsoleInput {
        protocol: UnsafeCell::new(SimpleTextInputProtocol {
            reset,
            read_key_stroke,
            wait_for_key: ptr::null_mut(),
        }),
        state: UnsafeCell::new(InputState {
            decoder: Decoder::new(),
            fault: false,
        }),
    };

    pub(in super::super) unsafe fn prepare() -> Result<(*mut SimpleTextInputProtocol, Event), Status>
    {
        let mut wait = ptr::null_mut();
        let mut timer = ptr::null_mut();
        unsafe {
            let status = create_event(
                EVT_NOTIFY_WAIT,
                TPL_NOTIFY,
                Some(notify_wait),
                ptr::null_mut(),
                &mut wait,
            );
            if status != Status::SUCCESS {
                return Err(status);
            }
            let status = create_event(
                EVT_TIMER | EVT_NOTIFY_SIGNAL,
                TPL_CALLBACK,
                Some(notify_timer),
                ptr::null_mut(),
                &mut timer,
            );
            if status != Status::SUCCESS {
                close_event(wait);
                return Err(status);
            }
            let protocol = INPUT.protocol.get();
            ptr::addr_of_mut!((*protocol).wait_for_key).write(wait);
            let status = set_timer(timer, TIMER_PERIODIC, POLL_100NS);
            if status != Status::SUCCESS {
                release(timer);
                return Err(status);
            }
            Ok((protocol, timer))
        }
    }

    pub(in super::super) unsafe fn release(timer: Event) {
        unsafe {
            close_event(timer);
            let wait = ptr::addr_of_mut!((*INPUT.protocol.get()).wait_for_key);
            let event = wait.replace(ptr::null_mut());
            close_event(event);
        }
    }

    unsafe extern "efiapi" fn reset(this: *mut SimpleTextInputProtocol, _: u8) -> Status {
        if this != INPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        unsafe {
            INPUT.state.get().write(InputState {
                decoder: Decoder::new(),
                fault: false,
            })
        };
        let mut drained = false;
        for _ in 0..POLL_BYTES {
            if receive_checked() == Ok(None) {
                drained = true;
                break;
            }
        }
        let wait = unsafe { ptr::addr_of!((*this).wait_for_key).read() };
        unsafe { check_event(wait) };
        if drained {
            Status::SUCCESS
        } else {
            Status::DEVICE_ERROR
        }
    }

    unsafe extern "efiapi" fn read_key_stroke(
        this: *mut SimpleTextInputProtocol,
        key: *mut InputKey,
    ) -> Status {
        if this != INPUT.protocol.get() || key.is_null() {
            return Status::INVALID_PARAMETER;
        }
        unsafe { poll_due_events() };
        let (status, empty) = unsafe {
            let state = &mut *INPUT.state.get();
            poll(state);
            let status = if let Some(value) = state.decoder.pop() {
                key.write(value);
                Status::SUCCESS
            } else if core::mem::take(&mut state.fault) {
                Status::DEVICE_ERROR
            } else {
                Status::NOT_READY
            };
            (status, !state.decoder.has_key())
        };
        if empty && status == Status::SUCCESS {
            let wait = unsafe { ptr::addr_of!((*this).wait_for_key).read() };
            unsafe { check_event(wait) };
        }
        status
    }

    unsafe extern "efiapi" fn notify_wait(_: Event, _: *mut c_void) {
        unsafe { poll_and_signal(false) };
    }

    unsafe extern "efiapi" fn notify_timer(_: Event, _: *mut c_void) {
        unsafe { poll_and_signal(true) };
    }

    unsafe fn poll_and_signal(timer_tick: bool) {
        let ready = unsafe {
            let state = &mut *INPUT.state.get();
            poll(state);
            if timer_tick {
                state.decoder.tick();
            }
            state.decoder.has_key()
        };
        if ready {
            let wait = unsafe { ptr::addr_of!((*INPUT.protocol.get()).wait_for_key).read() };
            unsafe { signal_event(wait) };
        }
    }

    fn poll(state: &mut InputState) {
        for _ in 0..POLL_BYTES {
            if !state.decoder.can_receive() {
                break;
            }
            match receive_checked() {
                Ok(Some(byte)) => state.decoder.feed(byte),
                Ok(None) => break,
                Err(()) => {
                    state.fault = true;
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decoder::Decoder, *};

    #[test]
    fn split_unicode_and_escape_sequences_produce_only_complete_keys() {
        let mut decoder = Decoder::new();
        decoder.feed(0xc3);
        assert_eq!(decoder.pop(), None);
        decoder.tick();
        decoder.feed(0xa9);
        assert_eq!(
            decoder.pop(),
            Some(InputKey {
                scan_code: 0,
                unicode_char: 0xe9
            })
        );
        decoder.feed(0xe2);
        decoder.feed(0x94);
        assert_eq!(decoder.pop(), None);
        decoder.feed(0x80);
        assert_eq!(
            decoder.pop(),
            Some(InputKey {
                scan_code: 0,
                unicode_char: 0x2500
            })
        );
        decoder.feed(0x1b);
        decoder.tick();
        decoder.feed(b'[');
        assert_eq!(decoder.pop(), None);
        decoder.feed(b'A');
        assert_eq!(
            decoder.pop(),
            Some(InputKey {
                scan_code: 1,
                unicode_char: 0
            })
        );
        decoder.feed(0x1b);
        decoder.tick();
        assert_eq!(decoder.pop(), None);
        decoder.tick();
        assert_eq!(
            decoder.pop(),
            Some(InputKey {
                scan_code: 0x17,
                unicode_char: 0
            })
        );
    }

    #[test]
    fn queue_reserves_room_for_escape_and_reprocessed_byte() {
        let mut decoder = Decoder::new();
        for _ in 0..14 {
            assert!(decoder.can_receive());
            decoder.feed(b'a');
        }
        decoder.feed(0x1b);
        assert!(decoder.can_receive());
        decoder.feed(b'x');
        assert!(!decoder.can_receive());
        for _ in 0..14 {
            assert!(decoder.has_key());
            assert_eq!(decoder.pop().unwrap().unicode_char, u16::from(b'a'));
        }
        assert_eq!(decoder.pop().unwrap().scan_code, 0x17);
        assert_eq!(decoder.pop().unwrap().unicode_char, u16::from(b'x'));
        assert!(!decoder.has_key());
        assert!(decoder.can_receive());
    }
}
