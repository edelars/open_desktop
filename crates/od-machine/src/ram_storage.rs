//! RAM backing store of one physical-memory region (S6-30a).
//!
//! The type and the access rules follow `docs/specs/MEMORY_MODEL.md` (S6-04, S6-06):
//!
//! - the store is an array of `AtomicU64` cells, so the allocation is 8-aligned and writing
//!   through a shared reference is defined (a `Vec<u8>` has host alignment 1 and no interior
//!   mutability; the model forbids deriving host alignment from the guest address alone);
//! - `lead` unused bytes at the front make the host address of region offset `o` congruent to
//!   the guest address modulo 8, so a naturally aligned guest access of 1, 2, 4 or 8 bytes is
//!   always a naturally aligned host access;
//! - a naturally aligned scalar access is exactly one host atomic access of its own width
//!   (`AtomicU8/16/32/64::from_ptr`); a misaligned scalar access is split into ascending
//!   one-byte accesses (the model's split rule: byte-level visibility, no tearing guarantee);
//! - bulk copies are a sequence of independent relaxed accesses of up to 8 bytes (head to the
//!   next natural boundary, 8-byte body, tail). They are not atomic as a whole and not ordered
//!   against other accesses: another thread can observe any prefix of the sequence;
//! - there is no lock anywhere. Exclusion of writers is the protocol of S6-30c, not of this layer.
//!
//! Decision S6-30a (Opus review; recorded in `MEMORY_MODEL.md`, "Решение S6-30a", and in
//! `MACHINE_CONTRACT.md`, section S6-30a): variant A, byte-addressed storage with atomic views of
//! 1, 2, 4 and 8 bytes. Until S6-34 no two host threads access RAM at the same time. Non-concurrent
//! accesses of different widths are allowed by the Rust model. Concurrent accesses of different
//! widths to overlapping bytes are an accepted deviation from the Rust abstract machine; it relies
//! on the single-copy atomicity of an aligned access on the host (Arm ARM B2.2.1, Intel SDM
//! Vol. 3A 11.1.1). Invariants: (a) after creation no non-atomic access to the bytes exists (no
//! `&[u8]`/`&mut [u8]`, `ptr::copy` or memcpy); (b) every host access is aligned and at most 8
//! bytes wide (16 bytes only by the S6-31 primitive); (c) `unsafe` is only in the macros of this
//! file; (d) target hosts are aarch64 and x86_64; (e) concurrent tests keep one width per
//! location (mixing widths is for the host litmus of S6-36, not for Miri).
//!
//! Variant B (`AtomicU64` cells only, narrow stores as a CAS loop, narrow locked RMW through CAS
//! instead of `LDADD*`) was rejected by its cost, not by the text of the plan. The accounting of
//! `RamLimits` counts the guest bytes of a region; the allocation also holds `lead` bytes in front
//! and a tail up to the 8-byte word, at most 14 bytes per region (negligible, the limit is not
//! changed).

use std::sync::atomic::{AtomicU8, AtomicU16, AtomicU32, AtomicU64, Ordering};

// Scalar values are exchanged with the store in guest (little-endian) byte order: a host atomic
// access and a byte-by-byte composition give the same value only on a little-endian host.
const _: () = assert!(cfg!(target_endian = "little"));

// The decision above rests on the single-copy atomicity of aligned host accesses on these hosts
// only; any other target is not reviewed.
#[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64")))]
compile_error!("RamStorage is reviewed for aarch64 and x86_64 hosts only (S6-30a, invariant d)");

pub(crate) struct RamStorage {
    words: Vec<AtomicU64>,
    /// Unused bytes before offset 0 (`region start % 8`).
    lead: usize,
    /// Usable bytes.
    len: usize,
}

#[cfg(test)]
thread_local! {
    /// Test hook: every host access made by this thread as `(region offset, width)`.
    static TRACE: std::cell::RefCell<Option<Vec<(usize, usize)>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn trace(at: usize, lead: usize, width: usize) {
    TRACE.with(|trace| {
        if let Some(log) = trace.borrow_mut().as_mut() {
            log.push((at - lead, width));
        }
    });
}

#[cfg(not(test))]
#[inline(always)]
fn trace(_at: usize, _lead: usize, _width: usize) {}

/// Width of the next host access of a copy that starts at host index `at` with `remaining`
/// bytes left: the largest of 8, 4, 2, 1 that is naturally aligned at `at` and fits.
fn chunk_width(at: usize, remaining: usize) -> usize {
    let aligned = 1usize << at.trailing_zeros().min(3);
    let fits = 1usize << (usize::BITS - 1 - remaining.leading_zeros()).min(3);
    aligned.min(fits)
}

macro_rules! cell_access {
    ($load:ident, $store:ident, $int:ty, $atomic:ty) => {
        /// One host atomic load of this width at host index `at`.
        /// `at` must be naturally aligned and `at + width <= lead + len`.
        #[inline(always)]
        fn $load(&self, at: usize, order: Ordering) -> $int {
            const WIDTH: usize = std::mem::size_of::<$int>();
            debug_assert!(at % WIDTH == 0 && at + WIDTH <= self.lead + self.len);
            trace(at, self.lead, WIDTH);
            // SAFETY: `at` is a multiple of the width inside the allocation, `base()` is
            // 8-aligned, so the pointer is non-null, naturally aligned and valid for the
            // lifetime of `self`; it derives from the whole slice, whose cells are atomics
            // (interior mutability), so shared access is allowed. Every access to the store is
            // atomic; overlapping concurrent accesses of different widths are the accepted deviation
            // recorded in the module header (decision S6-30a).
            let cell = unsafe { <$atomic>::from_ptr(self.base().add(at).cast::<$int>()) };
            cell.load(order)
        }

        /// One host atomic store of this width; same preconditions as the load.
        #[inline(always)]
        fn $store(&self, at: usize, value: $int, order: Ordering) {
            const WIDTH: usize = std::mem::size_of::<$int>();
            debug_assert!(at % WIDTH == 0 && at + WIDTH <= self.lead + self.len);
            trace(at, self.lead, WIDTH);
            // SAFETY: as in the load.
            let cell = unsafe { <$atomic>::from_ptr(self.base().add(at).cast::<$int>()) };
            cell.store(value, order);
        }
    };
}

macro_rules! scalar_access {
    ($load:ident, $store:ident, $cell_load:ident, $cell_store:ident, $int:ty) => {
        /// Scalar load of `offset` (region offset). A naturally aligned access is one host access
        /// of this width; otherwise ascending one-byte accesses. Panics when out of range.
        pub(crate) fn $load(&self, offset: usize, order: Ordering) -> $int {
            const WIDTH: usize = std::mem::size_of::<$int>();
            self.check(offset, WIDTH);
            let at = self.lead + offset;
            if at % WIDTH == 0 {
                return self.$cell_load(at, order);
            }
            let mut bytes = [0u8; WIDTH];
            for (index, byte) in bytes.iter_mut().enumerate() {
                *byte = self.cell_load_u8(at + index, order);
            }
            <$int>::from_le_bytes(bytes)
        }

        /// Scalar store; the split rule is that of the load.
        pub(crate) fn $store(&self, offset: usize, value: $int, order: Ordering) {
            const WIDTH: usize = std::mem::size_of::<$int>();
            self.check(offset, WIDTH);
            let at = self.lead + offset;
            if at % WIDTH == 0 {
                return self.$cell_store(at, value, order);
            }
            for (index, byte) in value.to_le_bytes().into_iter().enumerate() {
                self.cell_store_u8(at + index, byte, order);
            }
        }
    };
}

impl RamStorage {
    /// A zeroed store of `len` bytes for a region whose guest start address is congruent to
    /// `lead` modulo 8. `Err` on size overflow or allocation failure.
    pub(crate) fn new_zeroed(len: usize, lead: usize) -> Result<Self, ()> {
        debug_assert!(lead < 8);
        let words_needed = lead
            .checked_add(len)
            .and_then(|total| total.checked_add(7))
            .ok_or(())?
            / 8;
        let mut words = Vec::new();
        words.try_reserve_exact(words_needed).map_err(|_| ())?;
        words.resize_with(words_needed, || AtomicU64::new(0));
        Ok(Self { words, lead, len })
    }

    fn base(&self) -> *mut u8 {
        self.words.as_ptr().cast::<u8>().cast_mut()
    }

    fn check(&self, offset: usize, width: usize) {
        assert!(
            offset.checked_add(width).is_some_and(|end| end <= self.len),
            "RAM storage access {offset}+{width} outside {} bytes",
            self.len
        );
    }

    cell_access!(cell_load_u8, cell_store_u8, u8, AtomicU8);
    cell_access!(cell_load_u16, cell_store_u16, u16, AtomicU16);
    cell_access!(cell_load_u32, cell_store_u32, u32, AtomicU32);
    cell_access!(cell_load_u64, cell_store_u64, u64, AtomicU64);

    scalar_access!(load_u8, store_u8, cell_load_u8, cell_store_u8, u8);
    scalar_access!(load_u16, store_u16, cell_load_u16, cell_store_u16, u16);
    scalar_access!(load_u32, store_u32, cell_load_u32, cell_store_u32, u32);
    scalar_access!(load_u64, store_u64, cell_load_u64, cell_store_u64, u64);

    /// One byte, or `None` outside the store (frame reads of the fetch cache).
    pub(crate) fn try_load_u8(&self, offset: usize) -> Option<u8> {
        (offset < self.len).then(|| self.cell_load_u8(self.lead + offset, Ordering::Relaxed))
    }

    /// Bulk read into `out` (`offset + out.len() <= len`, else panic). Not atomic as a whole and
    /// not ordered: a sequence of relaxed host accesses, each naturally aligned and as wide as
    /// the position and the remaining length allow (so an aligned 1/2/4/8-byte copy is one access).
    pub(crate) fn copy_out(&self, offset: usize, out: &mut [u8]) {
        self.check(offset, out.len());
        let mut done = 0;
        while done < out.len() {
            let at = self.lead + offset + done;
            let width = chunk_width(at, out.len() - done);
            let target = &mut out[done..done + width];
            match width {
                8 => {
                    target.copy_from_slice(&self.cell_load_u64(at, Ordering::Relaxed).to_le_bytes())
                }
                4 => {
                    target.copy_from_slice(&self.cell_load_u32(at, Ordering::Relaxed).to_le_bytes())
                }
                2 => {
                    target.copy_from_slice(&self.cell_load_u16(at, Ordering::Relaxed).to_le_bytes())
                }
                _ => target[0] = self.cell_load_u8(at, Ordering::Relaxed),
            }
            done += width;
        }
    }

    /// Bulk write of `input`; the granularity and the (absent) atomicity are those of
    /// [`Self::copy_out`]. Writes ascend in address order.
    pub(crate) fn copy_in(&self, offset: usize, input: &[u8]) {
        self.check(offset, input.len());
        let mut done = 0;
        while done < input.len() {
            let at = self.lead + offset + done;
            let width = chunk_width(at, input.len() - done);
            let source = &input[done..done + width];
            match width {
                8 => self.cell_store_u64(
                    at,
                    u64::from_le_bytes(source.try_into().unwrap()),
                    Ordering::Relaxed,
                ),
                4 => self.cell_store_u32(
                    at,
                    u32::from_le_bytes(source.try_into().unwrap()),
                    Ordering::Relaxed,
                ),
                2 => self.cell_store_u16(
                    at,
                    u16::from_le_bytes(source.try_into().unwrap()),
                    Ordering::Relaxed,
                ),
                _ => self.cell_store_u8(at, source[0], Ordering::Relaxed),
            }
            done += width;
        }
    }

    /// Host address of region offset `offset` (alignment checks in tests).
    #[cfg(test)]
    pub(crate) fn host_address(&self, offset: usize) -> usize {
        self.base() as usize + self.lead + offset
    }
}

/// Runs `f` and returns its result with every host access it made on this thread.
#[cfg(test)]
pub(crate) fn traced<R>(f: impl FnOnce() -> R) -> (R, Vec<(usize, usize)>) {
    TRACE.with(|trace| *trace.borrow_mut() = Some(Vec::new()));
    let result = f();
    let log = TRACE.with(|trace| trace.borrow_mut().take().unwrap());
    (result, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(len: usize, lead: usize) -> RamStorage {
        RamStorage::new_zeroed(len, lead).unwrap()
    }

    #[test]
    fn natural_guest_alignment_is_natural_host_alignment_for_every_lead() {
        for lead in 0..8usize {
            let ram = storage(64, lead);
            for offset in 0..48usize {
                for width in [1usize, 2, 4, 8] {
                    // guest address = region start + offset, region start = lead (mod 8)
                    if (lead + offset) % width == 0 {
                        assert_eq!(
                            ram.host_address(offset) % width,
                            0,
                            "lead {lead} offset {offset}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn aligned_scalar_access_is_exactly_one_host_access_of_its_width() {
        for lead in [0usize, 3, 5] {
            let ram = storage(64, lead);
            for width in [1usize, 2, 4, 8] {
                for offset in 0..40usize {
                    let aligned = (lead + offset) % width == 0;
                    let (_, log) = traced(|| match width {
                        1 => ram.store_u8(offset, 0xa5, Ordering::SeqCst),
                        2 => ram.store_u16(offset, 0xa5a5, Ordering::SeqCst),
                        4 => ram.store_u32(offset, 0xa5a5_a5a5, Ordering::SeqCst),
                        _ => ram.store_u64(offset, u64::MAX / 3, Ordering::SeqCst),
                    });
                    let (_, read_log) = traced(|| match width {
                        1 => drop(ram.load_u8(offset, Ordering::SeqCst)),
                        2 => drop(ram.load_u16(offset, Ordering::SeqCst)),
                        4 => drop(ram.load_u32(offset, Ordering::SeqCst)),
                        _ => drop(ram.load_u64(offset, Ordering::SeqCst)),
                    });
                    let expected: Vec<(usize, usize)> = if aligned {
                        vec![(offset, width)]
                    } else {
                        (0..width).map(|i| (offset + i, 1)).collect()
                    };
                    assert_eq!(
                        log, expected,
                        "store width {width} offset {offset} lead {lead}"
                    );
                    assert_eq!(
                        read_log, expected,
                        "load width {width} offset {offset} lead {lead}"
                    );
                }
            }
        }
    }

    #[test]
    fn bulk_copy_uses_one_access_for_an_aligned_scalar_and_ascending_chunks_otherwise() {
        let ram = storage(64, 0);
        for width in [1usize, 2, 4, 8] {
            let (_, log) = traced(|| ram.copy_in(width * 2, &vec![7u8; width]));
            assert_eq!(log, vec![(width * 2, width)], "aligned {width}-byte copy");
            let mut out = vec![0u8; width];
            let (_, log) = traced(|| ram.copy_out(width * 2, &mut out));
            assert_eq!(log, vec![(width * 2, width)]);
            assert_eq!(out, vec![7u8; width]);
        }
        // 13 bytes from offset 3: 1 (to 4), 4 (to 8), 8 (to 16); chunks ascend and tile the range.
        let (_, log) = traced(|| ram.copy_in(3, &[1u8; 13]));
        assert_eq!(log, vec![(3, 1), (4, 4), (8, 8)]);
        // 7 bytes from offset 8: 4, 2, 1.
        let (_, log) = traced(|| ram.copy_in(8, &[1u8; 7]));
        assert_eq!(log, vec![(8, 4), (12, 2), (14, 1)]);
    }

    #[test]
    fn bulk_copy_round_trips_every_offset_and_length_and_touches_only_its_range() {
        for lead in [0usize, 1, 6] {
            let ram = storage(80, lead);
            for offset in 0..40usize {
                for len in 1..33usize {
                    let before: Vec<u8> = {
                        let mut all = vec![0u8; 80];
                        ram.copy_out(0, &mut all);
                        all
                    };
                    let data: Vec<u8> = (0..len)
                        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(offset as u8) | 1)
                        .collect();
                    ram.copy_in(offset, &data);
                    let mut after = vec![0u8; 80];
                    ram.copy_out(0, &mut after);
                    let mut expected = before.clone();
                    expected[offset..offset + len].copy_from_slice(&data);
                    assert_eq!(after, expected, "lead {lead} offset {offset} len {len}");
                    let mut back = vec![0u8; len];
                    ram.copy_out(offset, &mut back);
                    assert_eq!(back, data);
                    // restore
                    ram.copy_in(0, &before);
                }
            }
        }
    }

    #[test]
    fn scalars_are_little_endian_and_agree_with_byte_access() {
        let ram = storage(32, 2);
        ram.store_u64(6, 0x0807_0605_0403_0201, Ordering::SeqCst);
        let mut bytes = [0u8; 8];
        ram.copy_out(6, &mut bytes);
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(ram.load_u32(10, Ordering::SeqCst), 0x0807_0605);
        assert_eq!(ram.load_u16(7, Ordering::SeqCst), 0x0302); // misaligned host: byte path
        assert_eq!(ram.load_u8(13, Ordering::SeqCst), 8);
        assert_eq!(ram.try_load_u8(13), Some(8));
        assert_eq!(ram.try_load_u8(32), None);
    }

    #[test]
    #[should_panic(expected = "outside")]
    fn access_past_the_end_panics_before_touching_memory() {
        storage(16, 0).load_u64(9, Ordering::SeqCst);
    }

    #[test]
    fn concurrent_aligned_accesses_never_tear() {
        use std::sync::atomic::AtomicBool;
        let ram = storage(64, 0);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            for writer in 0..2u8 {
                let (ram, stop) = (&ram, &stop);
                scope.spawn(move || {
                    let mut x = u8::from(writer) * 8 + 1;
                    while !stop.load(Ordering::Relaxed) {
                        x = x.wrapping_add(1) | 1;
                        ram.store_u64(0, u64::from_ne_bytes([x; 8]), Ordering::SeqCst);
                        ram.store_u32(16, u32::from_ne_bytes([x; 4]), Ordering::SeqCst);
                        ram.store_u16(24, u16::from_ne_bytes([x; 2]), Ordering::SeqCst);
                        ram.copy_in(32, &[x; 8]); // aligned 8-byte copy is one access
                    }
                });
            }
            let mut seen = 0u32;
            for _ in 0..200_000 {
                let a = ram.load_u64(0, Ordering::SeqCst).to_ne_bytes();
                let b = ram.load_u32(16, Ordering::SeqCst).to_ne_bytes();
                let c = ram.load_u16(24, Ordering::SeqCst).to_ne_bytes();
                let mut d = [0u8; 8];
                ram.copy_out(32, &mut d);
                assert!(a.iter().all(|&v| v == a[0]), "torn u64 {a:?}");
                assert!(b.iter().all(|&v| v == b[0]), "torn u32 {b:?}");
                assert!(c.iter().all(|&v| v == c[0]), "torn u16 {c:?}");
                assert!(d.iter().all(|&v| v == d[0]), "torn 8-byte copy {d:?}");
                seen += u32::from(a[0] != 0);
            }
            stop.store(true, Ordering::Relaxed);
            let _ = seen;
        });
    }
}
