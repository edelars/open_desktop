//! PFP 1.06 crypto-agile event log for the active PCR banks.

use core::ptr;

use super::banks::{EventDigests, MAX_TAGGED_BYTES, PcrBanks};

pub(super) const CAPACITY: usize = 32 * 1024;
pub(super) const SECURE_BOOT_CAPACITY: usize = 256 * 1024;
pub(super) const EV_NO_ACTION: u32 = 3;
const MAX_EVENT2_HEADER: usize = 16 + MAX_TAGGED_BYTES;
const FINAL_HEADER: usize = 16;

fn spec_id(banks: PcrBanks, entry: &mut [u8; 69]) -> usize {
    let active = banks.algorithms();
    entry[4..8].copy_from_slice(&EV_NO_ACTION.to_le_bytes());
    entry[32..48].copy_from_slice(b"Spec ID Event03\0");
    entry[53] = 2; // TPM family version 2.0
    entry[54] = 106; // PFP version 1.06
    entry[55] = 2; // x86-64 UINTN
    entry[56..60].copy_from_slice(&(active.len() as u32).to_le_bytes());
    let mut end = 60;
    for &(algorithm, width) in active {
        entry[end..end + 2].copy_from_slice(&[algorithm, 0]);
        entry[end + 2..end + 4].copy_from_slice(&width.to_le_bytes());
        end += 4;
    }
    let length = end + 1; // Vendor info size, zero.
    entry[28..32].copy_from_slice(&((length - 32) as u32).to_le_bytes());
    length
}

pub(super) struct EventLog {
    capacity: usize,
    primary: *mut u8,
    primary_used: usize,
    last: usize,
    truncated: bool,
    final_events: *mut u8,
    final_used: usize,
    final_count: u64,
    final_active: bool,
    final_full: bool,
}

impl EventLog {
    /// Both buffers are distinct live allocations of `capacity` bytes, at least 69.
    pub(super) unsafe fn new(
        primary: *mut u8,
        final_events: *mut u8,
        capacity: usize,
        banks: PcrBanks,
    ) -> Self {
        let mut entry = [0; 69];
        let primary_used = spec_id(banks, &mut entry);
        unsafe {
            ptr::copy_nonoverlapping(entry.as_ptr(), primary, primary_used);
            final_events.cast::<u64>().write(1);
            final_events.add(8).cast::<u64>().write(0);
        }
        Self {
            capacity,
            primary,
            primary_used,
            last: 0,
            truncated: false,
            final_events,
            final_used: FINAL_HEADER,
            final_count: 0,
            final_active: false,
            final_full: false,
        }
    }

    pub(super) fn get(&mut self) -> (u64, u64, bool) {
        self.final_active = true;
        (
            self.primary as u64,
            self.primary as u64 + self.last as u64,
            self.truncated,
        )
    }

    pub(super) fn full(&self) -> bool {
        self.truncated || self.final_full
    }

    pub(super) fn append(
        &mut self,
        pcr: u32,
        kind: u32,
        digests: &EventDigests,
        data: &[&[u8]],
    ) -> bool {
        let mut header = [0; MAX_EVENT2_HEADER];
        header[..4].copy_from_slice(&pcr.to_le_bytes());
        header[4..8].copy_from_slice(&kind.to_le_bytes());
        header[8..12].copy_from_slice(&digests.count().to_le_bytes());
        let offset = 12 + digests.write_tagged::<false>(&mut header[12..]);
        let header_len = offset + 4;
        let Some(len) = data
            .iter()
            .try_fold(header_len, |size, part| size.checked_add(part.len()))
        else {
            self.truncated = true;
            self.final_full |= self.final_active;
            return false;
        };
        header[offset..header_len].copy_from_slice(&((len - header_len) as u32).to_le_bytes());
        let header = &header[..header_len];
        if !self.truncated && len <= self.capacity - self.primary_used {
            self.last = self.primary_used;
            // SAFETY: the count bounds this entire entry within the exclusive log buffer.
            unsafe { entry_at(self.primary.add(self.last), header, data) };
            self.primary_used += len;
        } else {
            self.truncated = true;
        }
        if self.final_active {
            if !self.final_full && len <= self.capacity - self.final_used {
                unsafe {
                    entry_at(self.final_events.add(self.final_used), header, data);
                    self.final_count += 1;
                    self.final_events
                        .add(8)
                        .cast::<u64>()
                        .write(self.final_count);
                }
                self.final_used += len;
            } else {
                self.final_full = true;
            }
        }
        !self.full()
    }
}

unsafe fn entry_at(out: *mut u8, header: &[u8], data: &[&[u8]]) {
    // Initialize the record before its start is published. Bytes beyond the
    // records are not part of either event log.
    unsafe { ptr::copy_nonoverlapping(header.as_ptr(), out, header.len()) };
    let mut offset = header.len();
    for part in data {
        // Use the same bounded copier as Boot Services. A GPT event can exceed
        // the firmware CPU profile's per-instruction REP work limit.
        unsafe {
            crate::bs::misc::copy_mem(out.add(offset).cast(), part.as_ptr().cast(), part.len())
        };
        offset += part.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::slice;

    #[test]
    fn first_log_pointer_and_final_entries_are_independent() {
        let mut main = [0u64; CAPACITY / 8];
        let mut tail = [0u64; CAPACITY / 8];
        let main_ptr = main.as_mut_ptr().cast::<u8>();
        let tail_ptr = tail.as_mut_ptr().cast::<u8>();
        let mut log = unsafe { EventLog::new(main_ptr, tail_ptr, CAPACITY, PcrBanks::Sha256) };
        assert_eq!(
            unsafe { slice::from_raw_parts(main_ptr.add(32), 16) },
            b"Spec ID Event03\0"
        );
        let digests = EventDigests::hash_parts(PcrBanks::Sha256, &[], Some([1; 32]));
        assert!(log.append(0, 8, &digests, &[b"a"]));
        assert_eq!(tail[1], 0);
        assert_eq!(log.get(), (main_ptr as u64, main_ptr as u64 + 65, false));
        assert!(log.append(4, 5, &digests, &[b"b", b"c"]));
        assert_eq!(tail[1], 1);
        assert_eq!(log.get().1 - main_ptr as u64, 116);
        assert!(log.append(5, 5, &digests, &[b"d"]));
        assert_eq!(tail[1], 2);
        let oversized = [0u8; CAPACITY];
        assert!(!log.append(0, 3, &digests, &[&oversized]));
        assert!(log.get().2);
        assert!(!log.append(0, 3, &digests, &[b"after truncation"]));
        assert_eq!(tail[1], 2);
    }

    #[test]
    fn mixed_width_events_match_spec_id_in_both_logs() {
        let mut main = [0u64; 32];
        let mut tail = [0u64; 32];
        let main_ptr = main.as_mut_ptr().cast::<u8>();
        let tail_ptr = tail.as_mut_ptr().cast::<u8>();
        let banks = PcrBanks::Both;
        let mut log = unsafe { EventLog::new(main_ptr, tail_ptr, 256, banks) };
        log.get();
        let digests = EventDigests::hash_parts(banks, &[b"a", b"bc"], None);
        assert!(log.append(4, 5, &digests, &[b"event"]));
        assert_eq!(log.get().1 - main_ptr as u64, 69);
        let first = unsafe { slice::from_raw_parts(main_ptr, 69) };
        assert_eq!(&first[28..32], &37u32.to_le_bytes());
        assert_eq!(&first[56..69], &[2, 0, 0, 0, 11, 0, 32, 0, 12, 0, 48, 0, 0]);
        let entry = unsafe { slice::from_raw_parts(main_ptr.add(69), 105) };
        assert_eq!(entry, unsafe {
            slice::from_raw_parts(tail_ptr.add(16), 105)
        });
        assert_eq!(&entry[..14], &[4, 0, 0, 0, 5, 0, 0, 0, 2, 0, 0, 0, 11, 0]);
        assert_eq!(&entry[46..48], &[12, 0]);
        assert_eq!(&entry[96..], b"\x05\0\0\0event");
        assert_eq!(
            &entry[48..96],
            &[
                0xcb, 0x00, 0x75, 0x3f, 0x45, 0xa3, 0x5e, 0x8b, 0xb5, 0xa0, 0x3d, 0x69, 0x9a, 0xc6,
                0x50, 0x07, 0x27, 0x2c, 0x32, 0xab, 0x0e, 0xde, 0xd1, 0x63, 0x1a, 0x8b, 0x60, 0x5a,
                0x43, 0xff, 0x5b, 0xed, 0x80, 0x86, 0x07, 0x2b, 0xa1, 0xe7, 0xcc, 0x23, 0x58, 0xba,
                0xec, 0xa1, 0x34, 0xc8, 0x25, 0xa7,
            ]
        );

        let mut spec = [0; 69];
        assert_eq!(spec_id(PcrBanks::Sha384, &mut spec), 65);
        assert_eq!(&spec[56..65], &[1, 0, 0, 0, 12, 0, 48, 0, 0]);
    }
}
