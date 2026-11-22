//! UEFI Simple Text Input and its bounded COM1 byte parser.

use core::ffi::c_void;

use crate::{
    bs::event::Event,
    console::key_queue::KeyData,
    uefi::{guid::Guid, status::Status, types::Char16},
};

pub const SIMPLE_TEXT_INPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x3874_77c1,
    data2: 0x69c7,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL_GUID (UEFI 2.11, 12.2.1).
pub const SIMPLE_TEXT_INPUT_EX_PROTOCOL_GUID: Guid = Guid {
    data1: 0xdd9e_7534,
    data2: 0x7762,
    data3: 0x4698,
    data4: [0x8c, 0x14, 0xf5, 0x85, 0x17, 0xa6, 0x25, 0xaa],
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

/// EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL.
#[repr(C)]
pub struct SimpleTextInputExProtocol {
    pub reset: unsafe extern "efiapi" fn(*mut Self, u8) -> Status,
    pub read_key_stroke_ex: unsafe extern "efiapi" fn(*mut Self, *mut KeyData) -> Status,
    pub wait_for_key_ex: Event,
    pub set_state: unsafe extern "efiapi" fn(*mut Self, *mut u8) -> Status,
    pub register_key_notify:
        unsafe extern "efiapi" fn(*mut Self, *mut KeyData, *mut c_void, *mut *mut c_void) -> Status,
    pub unregister_key_notify: unsafe extern "efiapi" fn(*mut Self, *mut c_void) -> Status,
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

#[cfg(any(target_os = "none", test))]
mod shared {
    use super::decoder::Decoder;
    use crate::console::key_queue::{KeyData, KeyQueue};

    /// The console input state behind ConIn and ConInEx: the COM1 byte parser, the one bounded
    /// key queue every source feeds, and the COM1 fault flag. Keys from COM1 enter the queue
    /// as the parser completes them; the USB keyboard's staged keys are appended right after
    /// ([`KeyQueue::take_from`]), so one poll cycle keeps COM1 keys ahead of USB keys and each
    /// source keeps its own order.
    pub(super) struct Shared {
        decoder: Decoder,
        pub(super) queue: KeyQueue,
        pub(super) fault: bool,
    }

    impl Shared {
        pub(super) const fn new() -> Self {
            Self {
                decoder: Decoder::new(),
                queue: KeyQueue::new(),
                fault: false,
            }
        }

        pub(super) fn has_key(&self) -> bool {
            !self.queue.is_empty()
        }

        pub(super) fn pop(&mut self) -> Option<KeyData> {
            self.queue.pop()
        }

        /// ConIn Reset: forget parsed and queued keys.
        pub(super) fn clear(&mut self) {
            *self = Self::new();
        }

        fn move_decoded(&mut self) {
            while self.queue.room() != 0 {
                let Some(key) = self.decoder.pop() else {
                    break;
                };
                self.queue
                    .push(KeyData::plain(key.scan_code, key.unicode_char));
            }
        }

        /// Reads up to `budget` bytes from COM1 while the parser and the queue have room: a
        /// full queue leaves the bytes in the UART (backpressure, nothing is dropped).
        pub(super) fn poll_bytes(
            &mut self,
            budget: usize,
            mut next: impl FnMut() -> Result<Option<u8>, ()>,
        ) {
            for _ in 0..budget {
                self.move_decoded();
                if !self.decoder.can_receive() {
                    break;
                }
                match next() {
                    Ok(Some(byte)) => self.decoder.feed(byte),
                    Ok(None) => break,
                    Err(()) => {
                        self.fault = true;
                        break;
                    }
                }
            }
            self.move_decoded();
        }

        /// Timer tick of the single-ESC rule.
        pub(super) fn tick(&mut self) {
            self.decoder.tick();
            self.move_decoded();
        }
    }
}

#[cfg(target_os = "none")]
pub(super) use terminal::{install_ex, prepare, release};

#[cfg(target_os = "none")]
mod terminal {
    use super::{shared::Shared, *};
    use crate::bs::event::{
        EVT_NOTIFY_SIGNAL, EVT_NOTIFY_WAIT, EVT_TIMER, TIMER_PERIODIC, TPL_CALLBACK, TPL_NOTIFY,
        check_event, close_event, create_event, poll_due_events, set_timer, signal_event,
    };
    use crate::console::key_queue::toggle_state;
    use crate::console::serial_term::receive_checked;
    use crate::uefi::system_table::SystemTable;
    use core::{cell::UnsafeCell, ffi::c_void, ptr};

    const POLL_100NS: u64 = 100_000;
    const POLL_BYTES: usize = 32;

    struct ConsoleInput {
        protocol: UnsafeCell<SimpleTextInputProtocol>,
        extended: UnsafeCell<SimpleTextInputExProtocol>,
        state: UnsafeCell<Shared>,
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
        extended: UnsafeCell::new(SimpleTextInputExProtocol {
            reset: reset_ex,
            read_key_stroke_ex,
            wait_for_key_ex: ptr::null_mut(),
            set_state,
            register_key_notify,
            unregister_key_notify,
        }),
        state: UnsafeCell::new(Shared::new()),
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

    /// Publishes EFI_SIMPLE_TEXT_INPUT_EX_PROTOCOL on the console input handle, next to the
    /// Simple Text Input protocol and over the same key queue.
    ///
    /// # Safety
    /// BSP, after `install` published the console; `system` is the live System Table.
    pub(in super::super) unsafe fn install_ex(system: *mut SystemTable) -> Status {
        let boot = unsafe { &*(*system).boot_services };
        let mut handle = unsafe { (*system).console_in_handle() };
        if handle.is_null() {
            return Status::NOT_READY;
        }
        let mut wait = ptr::null_mut();
        let status = unsafe {
            create_event(
                EVT_NOTIFY_WAIT,
                TPL_NOTIFY,
                Some(notify_wait),
                ptr::null_mut(),
                &mut wait,
            )
        };
        if status != Status::SUCCESS {
            return status;
        }
        let extended = INPUT.extended.get();
        unsafe { ptr::addr_of_mut!((*extended).wait_for_key_ex).write(wait) };
        let status = unsafe {
            (boot.install_protocol_interface)(
                &mut handle,
                &SIMPLE_TEXT_INPUT_EX_PROTOCOL_GUID,
                0,
                extended.cast(),
            )
        };
        if status != Status::SUCCESS {
            unsafe {
                ptr::addr_of_mut!((*extended).wait_for_key_ex).write(ptr::null_mut());
                close_event(wait);
            }
        }
        status
    }

    fn wait_events() -> [Event; 2] {
        unsafe {
            [
                ptr::addr_of!((*INPUT.protocol.get()).wait_for_key).read(),
                ptr::addr_of!((*INPUT.extended.get()).wait_for_key_ex).read(),
            ]
        }
    }

    /// One poll cycle: the USB keyboard's event ring and LED update first (that may wait for a
    /// control transfer and run callbacks, so no console state is borrowed meanwhile), then
    /// COM1, then the USB keys behind the COM1 keys.
    unsafe fn refresh(timer_tick: bool) {
        crate::usb::service();
        let state = unsafe { &mut *INPUT.state.get() };
        state.poll_bytes(POLL_BYTES, receive_checked);
        crate::usb::take_keys(&mut state.queue);
        if timer_tick {
            state.tick();
        }
    }

    unsafe fn clear_stale_signal() {
        for wait in wait_events() {
            if !wait.is_null() {
                unsafe { check_event(wait) };
            }
        }
    }

    unsafe fn do_reset() -> Status {
        unsafe { (*INPUT.state.get()).clear() };
        crate::usb::reset_keys();
        let mut drained = false;
        for _ in 0..POLL_BYTES {
            if receive_checked() == Ok(None) {
                drained = true;
                break;
            }
        }
        unsafe { clear_stale_signal() };
        if drained {
            Status::SUCCESS
        } else {
            Status::DEVICE_ERROR
        }
    }

    /// Next key for either protocol; `NOT_READY` on an empty queue, `DEVICE_ERROR` once after a
    /// COM1 fault.
    unsafe fn next_key() -> Result<KeyData, Status> {
        unsafe { poll_due_events() };
        unsafe { refresh(false) };
        let (result, empty) = unsafe {
            let state = &mut *INPUT.state.get();
            let result = if let Some(value) = state.pop() {
                Ok(value)
            } else if core::mem::take(&mut state.fault) {
                Err(Status::DEVICE_ERROR)
            } else {
                Err(Status::NOT_READY)
            };
            (result, !state.has_key())
        };
        if empty && result.is_ok() {
            unsafe { clear_stale_signal() };
        }
        result
    }

    unsafe extern "efiapi" fn reset(this: *mut SimpleTextInputProtocol, _: u8) -> Status {
        if this != INPUT.protocol.get() {
            return Status::INVALID_PARAMETER;
        }
        unsafe { do_reset() }
    }

    unsafe extern "efiapi" fn read_key_stroke(
        this: *mut SimpleTextInputProtocol,
        key: *mut InputKey,
    ) -> Status {
        if this != INPUT.protocol.get() || key.is_null() {
            return Status::INVALID_PARAMETER;
        }
        match unsafe { next_key() } {
            Ok(data) => {
                unsafe { key.write(data.key) };
                Status::SUCCESS
            }
            Err(status) => status,
        }
    }

    unsafe extern "efiapi" fn reset_ex(this: *mut SimpleTextInputExProtocol, _: u8) -> Status {
        if this != INPUT.extended.get() {
            return Status::INVALID_PARAMETER;
        }
        unsafe { do_reset() }
    }

    unsafe extern "efiapi" fn read_key_stroke_ex(
        this: *mut SimpleTextInputExProtocol,
        key_data: *mut KeyData,
    ) -> Status {
        if this != INPUT.extended.get() || key_data.is_null() {
            return Status::INVALID_PARAMETER;
        }
        match unsafe { next_key() } {
            Ok(data) => {
                unsafe { key_data.write(data) };
                Status::SUCCESS
            }
            Err(status) => status,
        }
    }

    /// SetState: the lock bits go to the keyboard and its LEDs (UEFI 2.11, 12.2.5; without the
    /// VALID bit the request is refused as EFI_UNSUPPORTED, as EDK2's USB keyboard does).
    unsafe extern "efiapi" fn set_state(
        this: *mut SimpleTextInputExProtocol,
        state: *mut u8,
    ) -> Status {
        if this != INPUT.extended.get() || state.is_null() {
            return Status::INVALID_PARAMETER;
        }
        let requested = unsafe { state.read() };
        if requested & toggle_state::VALID == 0 {
            return Status::UNSUPPORTED;
        }
        crate::usb::set_toggles(requested);
        crate::usb::service();
        Status::SUCCESS
    }

    /// Key notification is not supported (UEFI_PROFILE.md): applications poll ReadKeyStrokeEx.
    unsafe extern "efiapi" fn register_key_notify(
        _: *mut SimpleTextInputExProtocol,
        _: *mut KeyData,
        _: *mut c_void,
        _: *mut *mut c_void,
    ) -> Status {
        Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn unregister_key_notify(
        _: *mut SimpleTextInputExProtocol,
        _: *mut c_void,
    ) -> Status {
        Status::UNSUPPORTED
    }

    unsafe extern "efiapi" fn notify_wait(event: Event, _: *mut c_void) {
        unsafe { refresh(false) };
        if unsafe { (*INPUT.state.get()).has_key() } {
            unsafe { signal_event(event) };
        }
    }

    unsafe extern "efiapi" fn notify_timer(_: Event, _: *mut c_void) {
        unsafe { refresh(true) };
        if unsafe { (*INPUT.state.get()).has_key() } {
            for wait in wait_events() {
                if !wait.is_null() {
                    unsafe { signal_event(wait) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decoder::Decoder, shared::Shared, *};
    use crate::console::key_queue::{KEY_QUEUE_CAPACITY, KeyQueue};

    fn bytes(input: &[u8]) -> impl FnMut() -> Result<Option<u8>, ()> + '_ {
        let mut index = 0;
        move || {
            let byte = input.get(index).copied();
            index += 1;
            Ok(byte)
        }
    }

    fn drain(shared: &mut Shared) -> Vec<(u16, u16)> {
        std::iter::from_fn(|| shared.pop())
            .map(|data| (data.key.scan_code, data.key.unicode_char))
            .collect()
    }

    #[test]
    fn com1_keys_come_first_then_usb_keys_and_each_source_keeps_its_order() {
        let mut shared = Shared::new();
        shared.poll_bytes(32, bytes(b"ab"));
        let mut staging = KeyQueue::new();
        staging.push(KeyData::plain(0, u16::from(b'X')));
        staging.push(KeyData::plain(0, u16::from(b'Y')));
        shared.queue.take_from(&mut staging);
        shared.poll_bytes(32, bytes(b"c"));
        assert_eq!(
            drain(&mut shared),
            [b'a', b'b', b'X', b'Y', b'c'].map(|c| (0, u16::from(c)))
        );
        assert!(!shared.has_key());
    }

    #[test]
    fn a_full_queue_stops_reading_com1_and_loses_nothing() {
        let mut shared = Shared::new();
        let input = [b'q'; 600];
        let index = std::cell::Cell::new(0usize);
        let mut source = || {
            let byte = input.get(index.get()).copied();
            if byte.is_some() {
                index.set(index.get() + 1);
            }
            Ok(byte)
        };
        for _ in 0..40 {
            shared.poll_bytes(32, &mut source);
        }
        // The queue holds 256, the parser's own queue a few more, the rest stays in the UART.
        assert!(index.get() < input.len());
        assert_eq!(shared.queue.room(), 0);
        let consumed = index.get();
        let mut seen = 0;
        while let Some(data) = shared.pop() {
            assert_eq!(data.key.unicode_char, u16::from(b'q'));
            seen += 1;
        }
        assert!(seen >= KEY_QUEUE_CAPACITY);
        // After the reader made room, polling resumes where it stopped.
        for _ in 0..40 {
            shared.poll_bytes(32, &mut source);
            while shared.pop().is_some() {
                seen += 1;
            }
        }
        assert_eq!(index.get(), input.len());
        assert_eq!(seen, input.len());
        assert!(consumed < input.len());
    }

    #[test]
    fn a_fault_is_reported_once_and_clear_resets_everything() {
        let mut shared = Shared::new();
        shared.poll_bytes(32, || Err(()));
        assert!(shared.fault);
        shared.poll_bytes(32, bytes(b"z"));
        shared.clear();
        assert!(!shared.fault);
        assert!(!shared.has_key());
        assert_eq!(drain(&mut shared), []);
    }

    #[test]
    fn the_escape_timer_key_reaches_the_queue() {
        let mut shared = Shared::new();
        shared.poll_bytes(32, bytes(&[0x1b]));
        assert!(!shared.has_key());
        shared.tick();
        shared.tick();
        assert_eq!(drain(&mut shared), [(0x17, 0)]);
    }

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
