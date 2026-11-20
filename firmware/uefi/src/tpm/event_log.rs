//! PFP 1.06 crypto-agile event log with the SHA-256 bank.

use core::ptr;

pub(super) const CAPACITY: usize = 32 * 1024;
pub(super) const SECURE_BOOT_CAPACITY: usize = 256 * 1024;
pub(super) const EV_NO_ACTION: u32 = 3;
const SHA256: u16 = 0x000b;
const EVENT2_HEADER: usize = 50;
const FINAL_HEADER: usize = 16;

const SPEC_ID: [u8; 65] = {
    let mut entry = [0; 65];
    entry[4] = EV_NO_ACTION as u8;
    entry[28] = 33;
    let signature = b"Spec ID Event03\0";
    let mut i = 0;
    while i < signature.len() {
        entry[32 + i] = signature[i];
        i += 1;
    }
    entry[53] = 2; // TPM family version 2.0
    entry[54] = 106; // PFP version 1.06
    entry[55] = 2; // x86-64 UINTN
    entry[56] = 1; // algorithm count
    entry[60] = SHA256 as u8;
    entry[62] = 32;
    entry
};

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
    /// Both buffers are distinct live allocations of `capacity` bytes, at least SPEC_ID.len().
    pub(super) unsafe fn new(primary: *mut u8, final_events: *mut u8, capacity: usize) -> Self {
        unsafe {
            ptr::copy_nonoverlapping(SPEC_ID.as_ptr(), primary, SPEC_ID.len());
            final_events.cast::<u64>().write(1);
            final_events.add(8).cast::<u64>().write(0);
        }
        Self {
            capacity,
            primary,
            primary_used: SPEC_ID.len(),
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
        digest: &[u8; 32],
        data: &[&[u8]],
    ) -> bool {
        let Some(len) = data
            .iter()
            .try_fold(EVENT2_HEADER, |size, part| size.checked_add(part.len()))
        else {
            self.truncated = true;
            self.final_full |= self.final_active;
            return false;
        };
        if !self.truncated && len <= self.capacity - self.primary_used {
            self.last = self.primary_used;
            // SAFETY: the count bounds this entire entry within the exclusive log buffer.
            unsafe { entry_at(self.primary.add(self.last), len, pcr, kind, digest, data) };
            self.primary_used += len;
        } else {
            self.truncated = true;
        }
        if self.final_active {
            if !self.final_full && len <= self.capacity - self.final_used {
                unsafe {
                    entry_at(
                        self.final_events.add(self.final_used),
                        len,
                        pcr,
                        kind,
                        digest,
                        data,
                    );
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

unsafe fn entry_at(
    out: *mut u8,
    len: usize,
    pcr: u32,
    kind: u32,
    digest: &[u8; 32],
    data: &[&[u8]],
) {
    // Initialize the record before its start is published. Bytes beyond the
    // records are not part of either event log.
    unsafe {
        out.cast::<u32>().write_unaligned(pcr.to_le());
        out.add(4).cast::<u32>().write_unaligned(kind.to_le());
        out.add(8).cast::<u32>().write_unaligned(1u32.to_le());
        out.add(12).cast::<u16>().write_unaligned(SHA256.to_le());
        ptr::copy_nonoverlapping(digest.as_ptr(), out.add(14), digest.len());
        out.add(46)
            .cast::<u32>()
            .write_unaligned(((len - EVENT2_HEADER) as u32).to_le());
    }
    let mut offset = EVENT2_HEADER;
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
        let mut log = unsafe { EventLog::new(main_ptr, tail_ptr, CAPACITY) };
        assert_eq!(
            unsafe { slice::from_raw_parts(main_ptr.add(32), 16) },
            b"Spec ID Event03\0"
        );
        assert!(log.append(0, 8, &[1; 32], &[b"a"]));
        assert_eq!(tail[1], 0);
        assert_eq!(log.get(), (main_ptr as u64, main_ptr as u64 + 65, false));
        assert!(log.append(4, 5, &[2; 32], &[b"b", b"c"]));
        assert_eq!(tail[1], 1);
        assert_eq!(log.get().1 - main_ptr as u64, 116);
        assert!(log.append(5, 5, &[3; 32], &[b"d"]));
        assert_eq!(tail[1], 2);
        let oversized = [0u8; CAPACITY];
        assert!(!log.append(0, 3, &[0; 32], &[&oversized]));
        assert!(log.get().2);
        assert!(!log.append(0, 3, &[0; 32], &[b"after truncation"]));
        assert_eq!(tail[1], 2);
    }
}
