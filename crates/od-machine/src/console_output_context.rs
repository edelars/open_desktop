//! Bounded diagnostic capture of the live frame-pointer chain when selected console lines print.
//!
//! This records the CPU context while the guest is printing console output. It is not a guest
//! unwind and must not be presented as the origin of a previously printed panic trace.

pub(crate) const MAX_CAPTURE_COUNT: usize = 4;
const MAX_LINE_BYTES: usize = 192;
pub const LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY: usize = 32;
pub(crate) const MAX_FRAME_LINKS: usize = LINUX_PAGE_FAULT_FRAME_LINK_CAPACITY;
pub(crate) const STACK_WINDOW_BYTES: u64 = 4096;
pub const MAX_UART_PROVENANCE_BYTES: usize = 512;

const UART_PROVENANCE_MARKER: &[u8] = b"OD_NONCE=";
const UART_PROVENANCE_PREFIX: [usize; 9] = [0, 0, 0, 0, 1, 0, 0, 0, 0];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConsoleContextTrigger {
    CallTraceLine,
    OptionalFrameLine,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleContextFrame {
    pub frame_pointer: u64,
    pub return_address: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameChainEnd {
    NullFramePointer,
    NonCanonicalFramePointer,
    UnalignedFramePointer,
    OutsideStackWindow,
    RepeatedFramePointer,
    NonMonotonicFramePointer,
    ReadFailed,
    NonCanonicalReturnAddress,
    FrameLimit,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConsoleOutputContextSnapshot {
    pub trigger: ConsoleContextTrigger,
    pub rip: u64,
    pub rbp: u64,
    pub cr3: u64,
    pub frames: Vec<ConsoleContextFrame>,
    pub ended: FrameChainEnd,
}

/// CPU and bounded frame-chain context captured when a single byte is appended to COM1.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UartOutputByteContextSnapshot {
    pub offset: u64,
    pub byte: u8,
    pub retired: u64,
    pub rip: u64,
    pub rsp: u64,
    pub rbp: u64,
    pub cr3: u64,
    pub cs_selector: Option<u16>,
    pub frames: Vec<ConsoleContextFrame>,
    pub ended: FrameChainEnd,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UartOutputContextEnd {
    MarkerNotFound,
    RunEnded,
    ByteLimit,
}

/// Bounded provenance window following the first `OD_NONCE=` marker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UartOutputContextDiagnostic {
    /// COM1 offset of `O` in the matched marker.
    pub marker_offset: Option<u64>,
    /// The marker's first eight bytes precede the trigger and have no per-byte context samples.
    pub marker_prefix_before_trigger_unobserved: bool,
    pub end: UartOutputContextEnd,
    pub samples: Vec<UartOutputByteContextSnapshot>,
}

#[derive(Default)]
pub(crate) struct UartOutputContextCapture {
    marker_match: usize,
    marker_offset: Option<u64>,
    samples: Vec<UartOutputByteContextSnapshot>,
}

impl UartOutputContextCapture {
    /// Returns true when the current byte needs an immediate CPU/stack snapshot.
    pub(crate) fn observe_byte(&mut self, byte: u8, offset: u64) -> bool {
        if self.marker_offset.is_none() {
            while self.marker_match > 0 && byte != UART_PROVENANCE_MARKER[self.marker_match] {
                self.marker_match = UART_PROVENANCE_PREFIX[self.marker_match - 1];
            }
            if byte == UART_PROVENANCE_MARKER[self.marker_match] {
                self.marker_match += 1;
                if self.marker_match == UART_PROVENANCE_MARKER.len() {
                    self.marker_offset = Some(offset + 1 - UART_PROVENANCE_MARKER.len() as u64);
                    return true;
                }
            }
            return false;
        }
        self.samples.len() < MAX_UART_PROVENANCE_BYTES
    }

    pub(crate) fn push_snapshot(&mut self, snapshot: UartOutputByteContextSnapshot) {
        if self.samples.len() < MAX_UART_PROVENANCE_BYTES {
            self.samples.push(snapshot);
        }
    }

    pub(crate) fn finish(self) -> UartOutputContextDiagnostic {
        let end = if self.marker_offset.is_none() {
            UartOutputContextEnd::MarkerNotFound
        } else if self.samples.len() >= MAX_UART_PROVENANCE_BYTES {
            UartOutputContextEnd::ByteLimit
        } else {
            UartOutputContextEnd::RunEnded
        };
        UartOutputContextDiagnostic {
            marker_offset: self.marker_offset,
            marker_prefix_before_trigger_unobserved: self.marker_offset.is_some(),
            end,
            samples: self.samples,
        }
    }
}

#[cfg(test)]
mod uart_provenance_tests {
    use super::*;

    fn sample(offset: u64, byte: u8) -> UartOutputByteContextSnapshot {
        UartOutputByteContextSnapshot {
            offset,
            byte,
            retired: offset + 100,
            rip: 0x1000 + offset,
            rsp: 0x8000,
            rbp: 0x8100,
            cr3: 0x6000,
            cs_selector: Some(0x8),
            frames: Vec::new(),
            ended: FrameChainEnd::NullFramePointer,
        }
    }

    #[test]
    fn nonce_window_starts_at_equals_and_records_absolute_byte_offsets() {
        let mut capture = UartOutputContextCapture::default();
        for (offset, byte) in b"OD_NOD_NONCE=ab".iter().copied().enumerate() {
            if capture.observe_byte(byte, offset as u64) {
                capture.push_snapshot(sample(offset as u64, byte));
            }
        }
        let diagnostic = capture.finish();
        assert_eq!(diagnostic.marker_offset, Some(4));
        assert!(diagnostic.marker_prefix_before_trigger_unobserved);
        assert_eq!(diagnostic.end, UartOutputContextEnd::RunEnded);
        assert_eq!(
            diagnostic
                .samples
                .iter()
                .map(|sample| sample.byte)
                .collect::<Vec<_>>(),
            b"=ab"
        );
        assert_eq!(
            diagnostic
                .samples
                .iter()
                .map(|sample| sample.offset)
                .collect::<Vec<_>>(),
            [12, 13, 14]
        );
    }

    #[test]
    fn nonce_window_is_bounded_and_absent_marker_is_explicit() {
        let mut capture = UartOutputContextCapture::default();
        for (offset, byte) in b"OD_NONCE=".iter().copied().enumerate() {
            if capture.observe_byte(byte, offset as u64) {
                capture.push_snapshot(sample(offset as u64, byte));
            }
        }
        for index in 0..MAX_UART_PROVENANCE_BYTES + 20 {
            let offset = (b"OD_NONCE=".len() + index) as u64;
            if capture.observe_byte(b'x', offset) {
                capture.push_snapshot(sample(offset, b'x'));
            }
        }
        let diagnostic = capture.finish();
        assert_eq!(diagnostic.samples.len(), MAX_UART_PROVENANCE_BYTES);
        assert_eq!(diagnostic.end, UartOutputContextEnd::ByteLimit);

        let mut capture = UartOutputContextCapture::default();
        for (offset, byte) in b"OD_NONCE".iter().copied().enumerate() {
            assert!(!capture.observe_byte(byte, offset as u64));
        }
        let diagnostic = capture.finish();
        assert_eq!(diagnostic.marker_offset, None);
        assert_eq!(diagnostic.end, UartOutputContextEnd::MarkerNotFound);
        assert!(diagnostic.samples.is_empty());
    }
}

/// Small line recognizer. A too-long line is discarded through its newline, never truncated into
/// a false trigger. Input is a COM1 byte stream and this recognizer has no guest-facing effects.
#[derive(Default)]
pub(crate) struct ConsoleLineTrigger {
    line: Vec<u8>,
    overlong: bool,
}

impl ConsoleLineTrigger {
    pub(crate) fn push(&mut self, byte: u8) -> Option<ConsoleContextTrigger> {
        if byte != b'\n' {
            if self.line.len() < MAX_LINE_BYTES {
                self.line.push(byte);
            } else {
                self.overlong = true;
            }
            return None;
        }
        let trigger = (!self.overlong)
            .then(|| trigger_for_line(&self.line))
            .flatten();
        self.line.clear();
        self.overlong = false;
        trigger
    }
}

fn trigger_for_line(line: &[u8]) -> Option<ConsoleContextTrigger> {
    let line = trim_ascii(line);
    let line = if line.starts_with(b"[") {
        strip_printk_timestamp(line)?
    } else {
        line
    };
    if line == b"Call Trace:" {
        Some(ConsoleContextTrigger::CallTraceLine)
    } else if line.starts_with(b"? ") {
        Some(ConsoleContextTrigger::OptionalFrameLine)
    } else {
        None
    }
}

/// Removes only the kernel's bounded `[seconds.fraction] ` printk prefix. Malformed bracketed
/// prefixes are rejected instead of being treated as ordinary console lines.
fn strip_printk_timestamp(line: &[u8]) -> Option<&[u8]> {
    let close = line.iter().position(|byte| *byte == b']')?;
    let timestamp = trim_ascii(&line[1..close]);
    let mut parts = timestamp.split(|byte| *byte == b'.');
    let seconds = parts.next()?;
    let fraction = parts.next()?;
    let payload = &line[close + 1..];
    if parts.next().is_some()
        || seconds.is_empty()
        || fraction.is_empty()
        || !seconds.iter().all(u8::is_ascii_digit)
        || !fraction.iter().all(u8::is_ascii_digit)
        || !payload.first().is_some_and(u8::is_ascii_whitespace)
    {
        return None;
    }
    Some(trim_ascii(payload))
}

fn trim_ascii(mut line: &[u8]) -> &[u8] {
    while line.first().is_some_and(u8::is_ascii_whitespace) {
        line = &line[1..];
    }
    while line.last().is_some_and(u8::is_ascii_whitespace) {
        line = &line[..line.len() - 1];
    }
    line
}

pub(crate) fn capture_frame_chain(
    initial_rbp: u64,
    mut read_frame: impl FnMut(u64) -> Result<[u8; 16], ()>,
) -> (Vec<ConsoleContextFrame>, FrameChainEnd) {
    let Some(window_end) = initial_rbp.checked_add(STACK_WINDOW_BYTES) else {
        return (Vec::new(), FrameChainEnd::OutsideStackWindow);
    };
    let mut frame_pointer = initial_rbp;
    let mut frames = Vec::new();
    let mut visited = Vec::new();
    for _ in 0..MAX_FRAME_LINKS {
        if frame_pointer == 0 {
            return (frames, FrameChainEnd::NullFramePointer);
        }
        if !is_canonical48(frame_pointer) {
            return (frames, FrameChainEnd::NonCanonicalFramePointer);
        }
        if frame_pointer & 7 != 0 {
            return (frames, FrameChainEnd::UnalignedFramePointer);
        }
        let Some(frame_end) = frame_pointer.checked_add(16) else {
            return (frames, FrameChainEnd::OutsideStackWindow);
        };
        if frame_pointer < initial_rbp || frame_end > window_end {
            return (frames, FrameChainEnd::OutsideStackWindow);
        }
        if visited.contains(&frame_pointer) {
            return (frames, FrameChainEnd::RepeatedFramePointer);
        }
        visited.push(frame_pointer);
        let Ok(bytes) = read_frame(frame_pointer) else {
            return (frames, FrameChainEnd::ReadFailed);
        };
        let mut next_bytes = [0; 8];
        next_bytes.copy_from_slice(&bytes[..8]);
        let next = u64::from_le_bytes(next_bytes);
        let mut return_bytes = [0; 8];
        return_bytes.copy_from_slice(&bytes[8..]);
        let return_address = u64::from_le_bytes(return_bytes);
        if return_address != 0 && !is_canonical48(return_address) {
            return (frames, FrameChainEnd::NonCanonicalReturnAddress);
        }
        frames.push(ConsoleContextFrame {
            frame_pointer,
            return_address,
        });
        if next == 0 {
            return (frames, FrameChainEnd::NullFramePointer);
        }
        if visited.contains(&next) {
            return (frames, FrameChainEnd::RepeatedFramePointer);
        }
        if next <= frame_pointer {
            return (frames, FrameChainEnd::NonMonotonicFramePointer);
        }
        frame_pointer = next;
    }
    (frames, FrameChainEnd::FrameLimit)
}

fn is_canonical48(value: u64) -> bool {
    let high = value >> 48;
    let sign = (value >> 47) & 1;
    high == if sign == 0 { 0 } else { 0xffff }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn line_trigger_requires_complete_exact_bounded_lines() {
        let mut parser = ConsoleLineTrigger::default();
        let mut trigger = None;
        for byte in b"unrelated\n Call Trace: \n" {
            trigger = parser.push(*byte).or(trigger);
        }
        assert_eq!(trigger, Some(ConsoleContextTrigger::CallTraceLine));

        let mut parser = ConsoleLineTrigger::default();
        let mut triggers = Vec::new();
        for byte in
            b"[    2.274368] Call Trace:\n[    2.274391]  ? acct_update_integrals+0x1/0x20\n"
        {
            triggers.extend(parser.push(*byte));
        }
        assert_eq!(
            triggers,
            [
                ConsoleContextTrigger::CallTraceLine,
                ConsoleContextTrigger::OptionalFrameLine
            ]
        );

        for malformed in [
            b"[oops] Call Trace:".as_slice(),
            b"[2.x] ? acct_update_integrals+0x1/0x20",
            b"[2.3.4] Call Trace:",
            b"[2.3 Call Trace:",
            b"[2.3]Call Trace:",
        ] {
            assert_eq!(trigger_for_line(malformed), None, "{malformed:?}");
        }

        let mut parser = ConsoleLineTrigger::default();
        let mut trigger = None;
        for byte in b" ? symbol+0x1/0x2\n" {
            trigger = parser.push(*byte).or(trigger);
        }
        assert_eq!(trigger, Some(ConsoleContextTrigger::OptionalFrameLine));

        let mut parser = ConsoleLineTrigger::default();
        let mut trigger = None;
        for byte in [vec![b'x'; MAX_LINE_BYTES + 1], b"\nCall Trace:\n".to_vec()].concat() {
            trigger = parser.push(byte).or(trigger);
        }
        assert_eq!(trigger, Some(ConsoleContextTrigger::CallTraceLine));
    }

    #[test]
    fn frame_chain_checks_alignment_cycles_region_and_limit() {
        let mut memory = BTreeMap::new();
        memory.insert(0x1000, frame(0x1010, 0xffff_ffff_8100_1234));
        memory.insert(0x1010, frame(0, 0xffff_ffff_8100_5678));
        let (frames, ended) =
            capture_frame_chain(0x1000, |address| memory.get(&address).copied().ok_or(()));
        assert_eq!(frames.len(), 2);
        assert_eq!(ended, FrameChainEnd::NullFramePointer);

        let (frames, ended) = capture_frame_chain(0x1001, |_| Ok([0; 16]));
        assert!(frames.is_empty());
        assert_eq!(ended, FrameChainEnd::UnalignedFramePointer);

        let (frames, ended) = capture_frame_chain(0x1000, |_| Ok(frame(0x1000, 0x2000)));
        assert_eq!(frames.len(), 1);
        assert_eq!(ended, FrameChainEnd::RepeatedFramePointer);

        let (frames, ended) = capture_frame_chain(0x1000, |_| Ok(frame(0x0ff0, 0x2000)));
        assert_eq!(frames.len(), 1);
        assert_eq!(ended, FrameChainEnd::NonMonotonicFramePointer);

        let (frames, ended) = capture_frame_chain(0x0000_8000_0000_0000, |_| Ok([0; 16]));
        assert!(frames.is_empty());
        assert_eq!(ended, FrameChainEnd::NonCanonicalFramePointer);

        let (frames, ended) = capture_frame_chain(0x1000, |address| {
            Ok(frame(address + STACK_WINDOW_BYTES, 0x2000))
        });
        assert_eq!(frames.len(), 1);
        assert_eq!(ended, FrameChainEnd::OutsideStackWindow);

        let (frames, ended) = capture_frame_chain(0x1000, |address| {
            Ok(frame(address + 16, 0xffff_ffff_8100_1234))
        });
        assert_eq!(frames.len(), MAX_FRAME_LINKS);
        assert_eq!(ended, FrameChainEnd::FrameLimit);

        let (frames, ended) = capture_frame_chain(0x1000, |_| Err(()));
        assert!(frames.is_empty());
        assert_eq!(ended, FrameChainEnd::ReadFailed);
    }

    fn frame(next: u64, return_address: u64) -> [u8; 16] {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&next.to_le_bytes());
        bytes[8..].copy_from_slice(&return_address.to_le_bytes());
        bytes
    }
}
