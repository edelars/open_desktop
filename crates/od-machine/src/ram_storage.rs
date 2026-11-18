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
//! - S6-31: a locked read-modify-write (`atomic_compare_exchange`, `atomic_swap`,
//!   `atomic_fetch_add`) is one host atomic operation of the operand's width on a naturally aligned
//!   cell, `SeqCst` (`AtomicU8/16/32/64::compare_exchange`/`swap`/`fetch_add`; S6-06 chose these
//!   Rust std operations, which compile to `CASAL*`/`SWPAL*`/`LDADDAL*` on aarch64 with LSE and to
//!   `lock cmpxchg`/`xchg`/`lock xadd` on x86_64). A misaligned operand is not served here (the
//!   caller refuses it, `PhysicalMemory`); there is no 16-byte primitive (no consumer, see
//!   `MACHINE_CONTRACT.md`, S6-31);
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
//! file and in `operand_ptr` (S6-41c, amendment (в′): the host pointer of an aligned operand for
//! generated code); (d) target hosts are aarch64 and x86_64; (e) concurrent tests keep one width per
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

#[cfg(test)]
thread_local! {
    /// Test hook (S6-32): the host memory order of every scalar load and store made by this thread.
    static ORDERS: std::cell::RefCell<Option<Vec<Ordering>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn trace_order(order: Ordering) {
    ORDERS.with(|orders| {
        if let Some(log) = orders.borrow_mut().as_mut() {
            log.push(order);
        }
    });
}

#[cfg(not(test))]
#[inline(always)]
fn trace_order(_order: Ordering) {}

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
            trace_order(order);
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
            trace_order(order);
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
        #[inline(always)]
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
        #[inline(always)]
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

macro_rules! cell_rmw {
    ($cas:ident, $swap:ident, $add:ident, $int:ty, $atomic:ty) => {
        /// One host compare-exchange (`SeqCst`) of this width at host index `at`: `Ok(old)` when
        /// the cell held `expected` and now holds `new`, `Err(observed)` otherwise (nothing
        /// written). `at` must be naturally aligned and inside the store (hard-checked by the
        /// caller, `locked_cell`).
        #[inline(always)]
        fn $cas(&self, at: usize, expected: $int, new: $int) -> Result<$int, $int> {
            trace(at, self.lead, std::mem::size_of::<$int>());
            // SAFETY: as in `cell_access!`: `locked_cell` has checked that `at` is a multiple of
            // the width and `at + width <= lead + len`.
            let cell = unsafe { <$atomic>::from_ptr(self.base().add(at).cast::<$int>()) };
            cell.compare_exchange(expected, new, Ordering::SeqCst, Ordering::SeqCst)
        }

        /// One host swap (`SeqCst`); same preconditions as the compare-exchange.
        #[inline(always)]
        fn $swap(&self, at: usize, value: $int) -> $int {
            trace(at, self.lead, std::mem::size_of::<$int>());
            // SAFETY: as in the compare-exchange.
            let cell = unsafe { <$atomic>::from_ptr(self.base().add(at).cast::<$int>()) };
            cell.swap(value, Ordering::SeqCst)
        }

        /// One host fetch-add (`SeqCst`, wrapping); same preconditions as the compare-exchange.
        #[inline(always)]
        fn $add(&self, at: usize, value: $int) -> $int {
            trace(at, self.lead, std::mem::size_of::<$int>());
            // SAFETY: as in the compare-exchange.
            let cell = unsafe { <$atomic>::from_ptr(self.base().add(at).cast::<$int>()) };
            cell.fetch_add(value, Ordering::SeqCst)
        }
    };
}

// `nonatomic-lock-test` (S6-36) leaves the host read-modify-write cells unused.
#[cfg_attr(feature = "nonatomic-lock-test", allow(dead_code))]
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

    cell_rmw!(cell_cas_u8, cell_swap_u8, cell_add_u8, u8, AtomicU8);
    cell_rmw!(cell_cas_u16, cell_swap_u16, cell_add_u16, u16, AtomicU16);
    cell_rmw!(cell_cas_u32, cell_swap_u32, cell_add_u32, u32, AtomicU32);
    cell_rmw!(cell_cas_u64, cell_swap_u64, cell_add_u64, u64, AtomicU64);

    /// Host index of the cell of a locked operation of `width` (1, 2, 4 or 8) at region offset
    /// `offset`. Panics when the operand is outside the store or not naturally aligned on the
    /// host: a host atomic of this width on a misaligned address is undefined behaviour, so this
    /// is a hard check (the guest-visible refusal of a misaligned operand is made before, in
    /// `PhysicalMemory`; a guest-aligned operand is host-aligned because of `lead`).
    fn locked_cell(&self, offset: usize, width: usize) -> usize {
        assert!(matches!(width, 1 | 2 | 4 | 8), "locked width {width}");
        self.check(offset, width);
        let at = self.lead + offset;
        assert!(
            at % width == 0,
            "locked operation of {width} bytes at a misaligned host index {at}"
        );
        at
    }

    /// S6-31: one host compare-exchange of `width` bytes at region offset `offset`, `SeqCst`.
    /// `Ok(old)` when the cell held `expected` (truncated to the width) and now holds `new`;
    /// `Err(observed)` otherwise, nothing written. Values are zero-extended.
    pub(crate) fn atomic_compare_exchange(
        &self,
        offset: usize,
        width: usize,
        expected: u64,
        new: u64,
    ) -> Result<u64, u64> {
        let at = self.locked_cell(offset, width);
        match width {
            1 => self
                .cell_cas_u8(at, expected as u8, new as u8)
                .map(u64::from)
                .map_err(u64::from),
            2 => self
                .cell_cas_u16(at, expected as u16, new as u16)
                .map(u64::from)
                .map_err(u64::from),
            4 => self
                .cell_cas_u32(at, expected as u32, new as u32)
                .map(u64::from)
                .map_err(u64::from),
            _ => self.cell_cas_u64(at, expected, new),
        }
    }

    /// S6-31: one host swap of `width` bytes, `SeqCst`; returns the old value (zero-extended).
    pub(crate) fn atomic_swap(&self, offset: usize, width: usize, value: u64) -> u64 {
        let at = self.locked_cell(offset, width);
        match width {
            1 => u64::from(self.cell_swap_u8(at, value as u8)),
            2 => u64::from(self.cell_swap_u16(at, value as u16)),
            4 => u64::from(self.cell_swap_u32(at, value as u32)),
            _ => self.cell_swap_u64(at, value),
        }
    }

    /// S6-31: one host wrapping fetch-add of `width` bytes, `SeqCst`; returns the old value
    /// (zero-extended); the stored sum wraps at the width.
    pub(crate) fn atomic_fetch_add(&self, offset: usize, width: usize, addend: u64) -> u64 {
        let at = self.locked_cell(offset, width);
        match width {
            1 => u64::from(self.cell_add_u8(at, addend as u8)),
            2 => u64::from(self.cell_add_u16(at, addend as u16)),
            4 => u64::from(self.cell_add_u32(at, addend as u32)),
            _ => self.cell_add_u64(at, addend),
        }
    }

    scalar_access!(load_u8, store_u8, cell_load_u8, cell_store_u8, u8);
    scalar_access!(load_u16, store_u16, cell_load_u16, cell_store_u16, u16);
    scalar_access!(load_u32, store_u32, cell_load_u32, cell_store_u32, u32);
    scalar_access!(load_u64, store_u64, cell_load_u64, cell_store_u64, u64);

    /// One byte, or `None` outside the store (frame reads of the fetch cache).
    #[inline(always)]
    pub(crate) fn try_load_u8(&self, offset: usize, order: Ordering) -> Option<u8> {
        (offset < self.len).then(|| self.cell_load_u8(self.lead + offset, order))
    }

    /// Bulk read into `out` (`offset + out.len() <= len`, else panic). Not atomic as a whole: a
    /// sequence of host accesses, each of memory order `order` (S6-32: the machine's ordinary
    /// order, `Relaxed` in the interleaving mode), each naturally aligned and as wide as the
    /// position and the remaining length allow (so an aligned 1/2/4/8-byte copy is one access).
    #[inline(always)]
    pub(crate) fn copy_out(&self, offset: usize, out: &mut [u8], order: Ordering) {
        self.check(offset, out.len());
        let mut done = 0;
        while done < out.len() {
            let at = self.lead + offset + done;
            let width = chunk_width(at, out.len() - done);
            let target = &mut out[done..done + width];
            match width {
                8 => target.copy_from_slice(&self.cell_load_u64(at, order).to_le_bytes()),
                4 => target.copy_from_slice(&self.cell_load_u32(at, order).to_le_bytes()),
                2 => target.copy_from_slice(&self.cell_load_u16(at, order).to_le_bytes()),
                _ => target[0] = self.cell_load_u8(at, order),
            }
            done += width;
        }
    }

    /// Bulk write of `input`; the granularity and the (absent) atomicity are those of
    /// [`Self::copy_out`]. Writes ascend in address order.
    #[inline(always)]
    pub(crate) fn copy_in(&self, offset: usize, input: &[u8], order: Ordering) {
        self.check(offset, input.len());
        let mut done = 0;
        while done < input.len() {
            let at = self.lead + offset + done;
            let width = chunk_width(at, input.len() - done);
            let source = &input[done..done + width];
            match width {
                8 => self.cell_store_u64(at, u64::from_le_bytes(source.try_into().unwrap()), order),
                4 => self.cell_store_u32(at, u32::from_le_bytes(source.try_into().unwrap()), order),
                2 => self.cell_store_u16(at, u16::from_le_bytes(source.try_into().unwrap()), order),
                _ => self.cell_store_u8(at, source[0], order),
            }
            done += width;
        }
    }

    /// S6-41c, amendment (в′) of S6-30a: the host pointer of the naturally aligned operand of
    /// `width` (1, 2, 4 or 8) bytes at region offset `offset`, for generated code that accesses
    /// the RAM with host-atomic instructions of that width. `None` when the width is not one of
    /// those, the operand is not inside the store, or the host address is not naturally aligned
    /// (checked on the host pointer, not derived from the guest address; a guest-aligned operand
    /// is host-aligned because of `lead`).
    ///
    /// Lifetime: the cells are allocated once by [`Self::new_zeroed`] and never resized or moved,
    /// so the pointer is valid while `self` is alive. The caller keeps it only for one synchronous
    /// call (it must not be stored in a cache that outlives the `PhysicalMemory`) and accesses it
    /// only with host-atomic instructions of exactly `width` bytes at that address (invariant (а′)
    /// of the amendment); no Rust reference to the bytes is made here or may be made from it.
    /// This function and the macros above are the only code in the crate that does pointer
    /// arithmetic on the store.
    pub(crate) fn operand_ptr(&self, offset: usize, width: usize) -> Option<*mut u8> {
        if !matches!(width, 1 | 2 | 4 | 8) || offset.checked_add(width)? > self.len {
            return None;
        }
        let at = self.lead + offset;
        if at % width != 0 {
            return None;
        }
        // SAFETY: `at + width <= lead + len`, which is within the allocation of
        // `words.len() * 8` bytes (`new_zeroed`), so the offset stays inside the same allocated
        // object; no memory is read or written here.
        Some(unsafe { self.base().add(at) })
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

/// Runs `f` and returns its result with the host memory order of every scalar load and store it
/// made on this thread (S6-32).
#[cfg(test)]
pub(crate) fn traced_orders<R>(f: impl FnOnce() -> R) -> (R, Vec<Ordering>) {
    ORDERS.with(|orders| *orders.borrow_mut() = Some(Vec::new()));
    let result = f();
    let log = ORDERS.with(|orders| orders.borrow_mut().take().unwrap());
    (result, log)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn storage(len: usize, lead: usize) -> RamStorage {
        RamStorage::new_zeroed(len, lead).unwrap()
    }

    /// S6-41c: the operand pointer is the host address of the offset, naturally aligned for its
    /// width, for every lead; a misaligned, out-of-range or odd-width request gets none.
    #[test]
    fn operand_ptr_is_naturally_aligned_in_range_and_refuses_everything_else() {
        for lead in 0..8usize {
            let ram = storage(64, lead);
            for width in [1usize, 2, 4, 8] {
                for offset in 0..64usize {
                    let pointer = ram.operand_ptr(offset, width);
                    let expected = (lead + offset) % width == 0 && offset + width <= 64;
                    assert_eq!(pointer.is_some(), expected, "lead {lead} {offset}+{width}");
                    if let Some(pointer) = pointer {
                        assert_eq!(pointer as usize, ram.host_address(offset));
                        assert_eq!(pointer as usize % width, 0);
                    }
                }
            }
            for width in [0usize, 3, 5, 6, 7, 9, 16] {
                assert!(ram.operand_ptr(0, width).is_none(), "width {width}");
            }
            assert!(ram.operand_ptr(usize::MAX, 1).is_none());
            assert!(ram.operand_ptr(63, 8).is_none());
        }
    }

    /// S6-41c: a store through the operand pointer with a host atomic of the operand's width is
    /// the store of the safe API (same cell, same bytes).
    #[test]
    fn a_host_atomic_through_the_operand_pointer_is_seen_by_the_safe_loads() {
        let ram = storage(64, 3);
        let pointer = ram.operand_ptr(5, 4).unwrap();
        // SAFETY: the pointer is a naturally aligned `u32` cell of a live store (test of the
        // contract of `operand_ptr`).
        unsafe { AtomicU32::from_ptr(pointer.cast::<u32>()) }.store(0xdead_beef, Ordering::SeqCst);
        assert_eq!(ram.load_u32(5, Ordering::SeqCst), 0xdead_beef);
        assert_eq!(ram.load_u8(5, Ordering::SeqCst), 0xef);
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
            let (_, log) = traced(|| ram.copy_in(width * 2, &vec![7u8; width], Ordering::Relaxed));
            assert_eq!(log, vec![(width * 2, width)], "aligned {width}-byte copy");
            let mut out = vec![0u8; width];
            let (_, log) = traced(|| ram.copy_out(width * 2, &mut out, Ordering::Relaxed));
            assert_eq!(log, vec![(width * 2, width)]);
            assert_eq!(out, vec![7u8; width]);
        }
        // 13 bytes from offset 3: 1 (to 4), 4 (to 8), 8 (to 16); chunks ascend and tile the range.
        let (_, log) = traced(|| ram.copy_in(3, &[1u8; 13], Ordering::Relaxed));
        assert_eq!(log, vec![(3, 1), (4, 4), (8, 8)]);
        // 7 bytes from offset 8: 4, 2, 1.
        let (_, log) = traced(|| ram.copy_in(8, &[1u8; 7], Ordering::Relaxed));
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
                        ram.copy_out(0, &mut all, Ordering::Relaxed);
                        all
                    };
                    let data: Vec<u8> = (0..len)
                        .map(|i| (i as u8).wrapping_mul(31).wrapping_add(offset as u8) | 1)
                        .collect();
                    ram.copy_in(offset, &data, Ordering::Relaxed);
                    let mut after = vec![0u8; 80];
                    ram.copy_out(0, &mut after, Ordering::Relaxed);
                    let mut expected = before.clone();
                    expected[offset..offset + len].copy_from_slice(&data);
                    assert_eq!(after, expected, "lead {lead} offset {offset} len {len}");
                    let mut back = vec![0u8; len];
                    ram.copy_out(offset, &mut back, Ordering::Relaxed);
                    assert_eq!(back, data);
                    // restore
                    ram.copy_in(0, &before, Ordering::Relaxed);
                }
            }
        }
    }

    #[test]
    fn scalars_are_little_endian_and_agree_with_byte_access() {
        let ram = storage(32, 2);
        ram.store_u64(6, 0x0807_0605_0403_0201, Ordering::SeqCst);
        let mut bytes = [0u8; 8];
        ram.copy_out(6, &mut bytes, Ordering::Relaxed);
        assert_eq!(bytes, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(ram.load_u32(10, Ordering::SeqCst), 0x0807_0605);
        assert_eq!(ram.load_u16(7, Ordering::SeqCst), 0x0302); // misaligned host: byte path
        assert_eq!(ram.load_u8(13, Ordering::SeqCst), 8);
        assert_eq!(ram.try_load_u8(13, Ordering::Relaxed), Some(8));
        assert_eq!(ram.try_load_u8(32, Ordering::Relaxed), None);
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
                        ram.copy_in(32, &[x; 8], Ordering::Relaxed); // aligned 8-byte copy is one access
                    }
                });
            }
            let mut seen = 0u32;
            for _ in 0..200_000 {
                let a = ram.load_u64(0, Ordering::SeqCst).to_ne_bytes();
                let b = ram.load_u32(16, Ordering::SeqCst).to_ne_bytes();
                let c = ram.load_u16(24, Ordering::SeqCst).to_ne_bytes();
                let mut d = [0u8; 8];
                ram.copy_out(32, &mut d, Ordering::Relaxed);
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

    /// S6-31: a locked operation is exactly one host access of the operand's width, for every
    /// width, lead and aligned offset; it wraps at the width, returns the old value
    /// zero-extended and touches no neighbouring byte.
    #[test]
    fn locked_operations_are_one_host_access_of_their_width_and_touch_only_their_bytes() {
        for lead in [0usize, 3, 5] {
            for width in [1usize, 2, 4, 8] {
                let mask = if width == 8 {
                    u64::MAX
                } else {
                    (1u64 << (8 * width)) - 1
                };
                for offset in 0..40usize {
                    if (lead + offset) % width != 0 {
                        continue;
                    }
                    let ram = storage(64, lead);
                    let mut fill = [0u8; 64];
                    for (i, byte) in fill.iter_mut().enumerate() {
                        *byte = (i as u8).wrapping_mul(37) | 0x80;
                    }
                    ram.copy_in(0, &fill, Ordering::Relaxed);
                    let mut old = [0u8; 8];
                    old[..width].copy_from_slice(&fill[offset..offset + width]);
                    let old = u64::from_le_bytes(old);
                    let expect = |value: u64| {
                        let mut bytes = fill;
                        bytes[offset..offset + width]
                            .copy_from_slice(&value.to_le_bytes()[..width]);
                        bytes
                    };
                    let dump = |ram: &RamStorage| {
                        let mut all = [0u8; 64];
                        ram.copy_out(0, &mut all, Ordering::Relaxed);
                        all
                    };

                    // fetch-add wraps at the width
                    let (got, log) = traced(|| ram.atomic_fetch_add(offset, width, u64::MAX));
                    assert_eq!(got, old, "add width {width} offset {offset} lead {lead}");
                    assert_eq!(log, vec![(offset, width)]);
                    assert_eq!(dump(&ram), expect(old.wrapping_add(u64::MAX) & mask));
                    ram.copy_in(0, &fill, Ordering::Relaxed);

                    // swap
                    let (got, log) =
                        traced(|| ram.atomic_swap(offset, width, 0x0123_4567_89ab_cdef));
                    assert_eq!(got, old);
                    assert_eq!(log, vec![(offset, width)]);
                    assert_eq!(dump(&ram), expect(0x0123_4567_89ab_cdef & mask));
                    ram.copy_in(0, &fill, Ordering::Relaxed);

                    // compare-exchange: a mismatch writes nothing and reports the observed value
                    let (got, log) =
                        traced(|| ram.atomic_compare_exchange(offset, width, old ^ 1, 0x55));
                    assert_eq!(got, Err(old));
                    assert_eq!(log, vec![(offset, width)]);
                    assert_eq!(dump(&ram), fill);
                    // a match writes; the expected value is truncated to the width
                    let (got, log) =
                        traced(|| ram.atomic_compare_exchange(offset, width, old | !mask, 0x55));
                    assert_eq!(got, Ok(old));
                    assert_eq!(log, vec![(offset, width)]);
                    assert_eq!(dump(&ram), expect(0x55));
                }
            }
        }
    }

    #[test]
    #[should_panic(expected = "misaligned host index")]
    fn a_locked_operation_on_a_misaligned_cell_panics_before_touching_memory() {
        // Region offset 1 with lead 0: a 2-byte host atomic there would be undefined behaviour.
        storage(16, 0).atomic_swap(1, 2, 7);
    }

    /// S6-31: the host operations are atomic against each other at every width (four threads,
    /// one location per width): an exact fetch-add sum, and a compare-exchange increment loop
    /// that loses nothing.
    #[test]
    fn concurrent_locked_operations_do_not_lose_updates_at_any_width() {
        const THREADS: u64 = 4;
        const ROUNDS: u64 = 20_000;
        let ram = storage(64, 0);
        for (index, width) in [1usize, 2, 4, 8].into_iter().enumerate() {
            let mask = if width == 8 {
                u64::MAX
            } else {
                (1u64 << (8 * width)) - 1
            };
            let adds = index * 16;
            let counter = index * 16 + 8;
            std::thread::scope(|scope| {
                for _ in 0..THREADS {
                    let ram = &ram;
                    scope.spawn(move || {
                        for _ in 0..ROUNDS {
                            ram.atomic_fetch_add(adds, width, 1);
                            let mut seen = 0u64;
                            loop {
                                match ram.atomic_compare_exchange(
                                    counter,
                                    width,
                                    seen,
                                    seen.wrapping_add(1) & mask,
                                ) {
                                    Ok(_) => break,
                                    Err(observed) => seen = observed,
                                }
                            }
                        }
                    });
                }
            });
            let total = (THREADS * ROUNDS) & mask;
            let read = |at: usize| {
                let mut bytes = [0u8; 64];
                ram.copy_out(0, &mut bytes, Ordering::Relaxed);
                let mut value = [0u8; 8];
                value[..width].copy_from_slice(&bytes[at..at + width]);
                u64::from_le_bytes(value)
            };
            assert_eq!(read(adds), total, "width {width}: fetch-add sum");
            assert_eq!(
                read(counter),
                total,
                "width {width}: compare-exchange counter"
            );
        }
    }
}
