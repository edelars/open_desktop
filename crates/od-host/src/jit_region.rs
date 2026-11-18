//! S6-42: the process `MAP_JIT` region partitioned between vCPU threads
//! (JIT_CONTRACT S6-40, rules A1 and M1 to M6; ADR-0010, proposed).
//!
//! # Shape
//!
//! * [`SharedJitRegion`] is the one `Sync` owner of the process's single
//!   `MAP_JIT` region and of its [`crate::MapJitLease`] (M1). It is created before
//!   the vCPU threads start and dropped after they are joined (or, with parts
//!   moved into threads, when the last part is gone: the mapping is released
//!   only after the owner and every part dropped).
//! * The region is cut into `parts` disjoint parts of `slots_per_part` slots of
//!   [`ARENA_SLOT_BYTES`], one part per vCPU (M2). A part is handed out exactly
//!   once ([`SharedJitRegion::take_part`]) as a [`JitRegionPart`], a `Send` value
//!   with no executable handle in it. Parts are never rebalanced and no thread
//!   takes a slot of another part while threads run.
//! * In its thread the part becomes an [`ActiveJitPart`] (`!Send`, `Rc`-based, A1):
//!   the slot table, the free list and the generation counters live in that
//!   thread only. Slots are written and released by that thread only (M3, A4):
//!   a slot is released when the [`crate::PublishedCode`] that owns it is dropped,
//!   and the type cannot leave the thread because it is `!Send`. There is no
//!   cross-thread release and no grace period: no other thread can enter a block
//!   of this part.
//!
//! # Memory rules used here (Apple and Arm, by memory, see JIT_CONTRACT S6-40 4.1)
//!
//! * `pthread_jit_write_protect_np` acts on the calling thread only, so a thread
//!   writing its own part does not disturb threads that execute from other parts.
//! * After the write: `sys_icache_invalidate` on the written range, then an
//!   explicit `ISB` (M4: whether `sys_icache_invalidate` ends with `ISB` is not
//!   verified, so the barrier is issued here). Writer and executor are the same
//!   thread, so M5 (cross-thread publication) never applies.
//!
//! # Not covered
//!
//! The admission path of the session maps to a slot, but only the paged
//! production profile (one instruction per block) is wired
//! ([`crate::PagedFetchedLoadSession::with_region_part`]). Fixture backends
//! (`PublishedArenaBackend`, `MultiEntryBlockCache`) keep their own region and
//! therefore get `MappingAlreadyLive` while a [`SharedJitRegion`] lives (M6).

use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, ThreadId};

use crate::{
    ARENA_SLOT_BYTES, ExecutableMemoryError, MappedRegion, PublishedCode, system_page_size,
};

/// Most parts (vCPUs) one region is cut into. Provisional bound, no measurement
/// behind it (S6-38 chooses the sizes).
pub const MAX_JIT_REGION_PARTS: usize = 64;
/// Most slots in one part.
pub const MAX_JIT_PART_SLOTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JitRegionError {
    /// `parts` or `slots_per_part` is zero or above the bound.
    InvalidShape {
        parts: usize,
        slots_per_part: usize,
    },
    SizeOverflow,
    /// The `MAP_JIT` mapping failed, or the process lease is taken
    /// ([`ExecutableMemoryError::MappingAlreadyLive`], M6).
    Mapping(ExecutableMemoryError),
    /// Every part was already handed out (M2: a part is handed out once).
    NoPartLeft,
}

struct RegionShared {
    region: MappedRegion,
    parts: usize,
    slots_per_part: usize,
    /// Bytes between the starts of two parts (a multiple of the system page).
    part_stride: usize,
}

// SAFETY: the raw pointer inside `MappedRegion` is used only to compute the
// address of a slot. A slot belongs to exactly one part, a part is handed out
// once and is used by one thread (`ActiveJitPart` is `!Send`), so no two
// threads write or release the same bytes. The mapping is unmapped only when
// the last `Arc` is gone, after every part and the owner are dropped.
unsafe impl Send for RegionShared {}
unsafe impl Sync for RegionShared {}

impl RegionShared {
    fn slot_ptr(&self, part: usize, slot: usize) -> NonNull<u8> {
        debug_assert!(part < self.parts && slot < self.slots_per_part);
        let offset = part * self.part_stride + slot * ARENA_SLOT_BYTES;
        debug_assert!(offset + ARENA_SLOT_BYTES <= self.region.len);
        // SAFETY: the offset is inside the region by the shape checks of `new`.
        unsafe { NonNull::new_unchecked(self.region.ptr().as_ptr().add(offset)) }
    }
}

/// The `Sync` owner of the process's `MAP_JIT` region and its lease (M1).
pub struct SharedJitRegion {
    shared: Arc<RegionShared>,
    next_part: Mutex<usize>,
}

impl SharedJitRegion {
    /// Maps the process's one `MAP_JIT` region for `parts` vCPUs of
    /// `slots_per_part` slots each. While it lives (or any part lives) the
    /// process lease is taken: an owned [`PublishedCode`] or arena gets
    /// [`ExecutableMemoryError::MappingAlreadyLive`] (M6).
    pub fn new(parts: usize, slots_per_part: usize) -> Result<Self, JitRegionError> {
        if parts == 0
            || parts > MAX_JIT_REGION_PARTS
            || slots_per_part == 0
            || slots_per_part > MAX_JIT_PART_SLOTS
        {
            return Err(JitRegionError::InvalidShape {
                parts,
                slots_per_part,
            });
        }
        let page = system_page_size().ok_or(JitRegionError::SizeOverflow)?;
        let used = slots_per_part
            .checked_mul(ARENA_SLOT_BYTES)
            .ok_or(JitRegionError::SizeOverflow)?;
        let part_stride = used
            .checked_next_multiple_of(page)
            .ok_or(JitRegionError::SizeOverflow)?;
        let total = part_stride
            .checked_mul(parts)
            .ok_or(JitRegionError::SizeOverflow)?;
        let region = MappedRegion::allocate(total).map_err(JitRegionError::Mapping)?;
        Ok(Self {
            shared: Arc::new(RegionShared {
                region,
                parts,
                slots_per_part,
                part_stride,
            }),
            next_part: Mutex::new(0),
        })
    }

    pub fn parts(&self) -> usize {
        self.shared.parts
    }

    pub fn slots_per_part(&self) -> usize {
        self.shared.slots_per_part
    }

    /// Reserved bytes of the whole mapping (page-rounded parts).
    pub fn reserved_bytes(&self) -> usize {
        self.shared.region.len
    }

    /// Hands out the next unused part (M2). Each part is handed out once; the
    /// caller moves it to the thread of its vCPU.
    pub fn take_part(&self) -> Result<JitRegionPart, JitRegionError> {
        let mut next = self
            .next_part
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *next >= self.shared.parts {
            return Err(JitRegionError::NoPartLeft);
        }
        let index = *next;
        *next += 1;
        Ok(JitRegionPart {
            shared: Arc::clone(&self.shared),
            index,
        })
    }
}

/// One vCPU's part of the region, before it reaches its thread. `Send`, and it
/// carries no executable handle: the slot table is created by
/// [`Self::activate`] in the thread that will own it.
pub struct JitRegionPart {
    shared: Arc<RegionShared>,
    index: usize,
}

impl JitRegionPart {
    pub fn index(&self) -> usize {
        self.index
    }

    pub fn slots(&self) -> usize {
        self.shared.slots_per_part
    }

    /// Start address and length of this part in the process's address space.
    /// For tests and reports: parts of one region never overlap.
    pub fn address_range(&self) -> (usize, usize) {
        let start = self.shared.slot_ptr(self.index, 0).as_ptr() as usize;
        (start, self.shared.slots_per_part * ARENA_SLOT_BYTES)
    }

    /// Makes the part the `!Send` arena of the calling thread (A1, M2). The
    /// calling thread becomes the only thread that writes and releases slots of
    /// the part.
    pub fn activate(self) -> ActiveJitPart {
        let slots = self.shared.slots_per_part;
        let mut free = Vec::with_capacity(slots);
        free.extend((0..slots).rev());
        let mut pending = Vec::with_capacity(slots);
        pending.resize_with(slots, || Cell::new(0));
        ActiveJitPart(Rc::new(PartInner {
            shared: self.shared,
            index: self.index,
            owner: thread::current().id(),
            state: RefCell::new(PartState {
                generations: vec![0; slots],
                live: vec![false; slots],
                free,
            }),
            pending,
            allocations: Cell::new(0),
            releases: Cell::new(0),
            failed_allocations: Cell::new(0),
        }))
    }
}

struct PartState {
    /// Grows by one on every allocation of the slot: a reused slot never has the
    /// generation of an earlier use (S3-25c).
    generations: Vec<u64>,
    live: Vec<bool>,
    free: Vec<usize>,
}

struct PartInner {
    shared: Arc<RegionShared>,
    index: usize,
    owner: ThreadId,
    state: RefCell<PartState>,
    /// Generation of a lease that dropped while the state was borrowed, or 0.
    /// Applied at the next safe point or allocation.
    pending: Vec<Cell<u64>>,
    allocations: Cell<u64>,
    releases: Cell<u64>,
    failed_allocations: Cell<u64>,
}

/// Counters of one part, read on the owner thread.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JitPartStats {
    pub slots: usize,
    /// Slots currently owned by a [`PublishedCode`].
    pub live: usize,
    pub allocations: u64,
    pub releases: u64,
    /// Releases deferred because the slot table was borrowed. Zero after
    /// [`ActiveJitPart::safe_point`].
    pub pending_release: usize,
    pub failed_allocations: u64,
    /// The largest slot generation: it counts the reuses of the busiest slot.
    pub max_slot_generation: u64,
}

impl PartInner {
    fn release_locked(&self, state: &mut PartState, slot: usize, generation: u64) {
        if state.generations.get(slot) == Some(&generation) && state.live[slot] {
            state.live[slot] = false;
            state.free.push(slot);
            // Test builds only: a released slot holds `BRK #0`, so executing a
            // released block would trap instead of running stale code.
            #[cfg(test)]
            {
                let brk = 0xD420_0000u32.to_le_bytes();
                let poison: Vec<u8> = brk.iter().cycle().take(64).copied().collect();
                // SAFETY: the slot of this part was just released by its owner
                // thread; nothing executes it (its owner is the caller).
                unsafe {
                    crate::write_mapping_bytes(self.shared.slot_ptr(self.index, slot), &poison)
                };
                instruction_synchronization_barrier();
            }
            self.releases.set(self.releases.get() + 1);
        }
    }

    fn drain_pending(&self) {
        let Ok(mut state) = self.state.try_borrow_mut() else {
            return;
        };
        for slot in 0..self.pending.len() {
            let generation = self.pending[slot].replace(0);
            if generation != 0 {
                self.release_locked(&mut state, slot, generation);
            }
        }
    }

    fn release(&self, slot: usize, generation: u64) {
        // A4: only the owner releases. `ActiveJitPart` and `SlotLease` are
        // `Rc`-based and so cannot be dropped on another thread.
        debug_assert_eq!(thread::current().id(), self.owner);
        match self.state.try_borrow_mut() {
            Ok(mut state) => {
                self.release_locked(&mut state, slot, generation);
            }
            Err(_) => self.pending[slot].set(generation),
        }
        self.drain_pending();
    }
}

/// The part in its thread: `!Send`, `Clone` (an `Rc`). The session of the vCPU
/// holds one and publishes its blocks into the slots.
#[derive(Clone)]
pub struct ActiveJitPart(Rc<PartInner>);

impl ActiveJitPart {
    pub fn index(&self) -> usize {
        self.0.index
    }

    pub fn slots(&self) -> usize {
        self.0.pending.len()
    }

    /// The safe point of S3-25c for this part: applies releases that were
    /// deferred while the slot table was borrowed. The session calls it between
    /// native calls; after it `pending_release` is zero.
    pub fn safe_point(&self) {
        self.0.drain_pending();
    }

    pub fn stats(&self) -> JitPartStats {
        let state = self.0.state.borrow();
        JitPartStats {
            slots: state.generations.len(),
            live: state.live.iter().filter(|live| **live).count(),
            allocations: self.0.allocations.get(),
            releases: self.0.releases.get(),
            pending_release: self.0.pending.iter().filter(|cell| cell.get() != 0).count(),
            failed_allocations: self.0.failed_allocations.get(),
            max_slot_generation: state.generations.iter().copied().max().unwrap_or(0),
        }
    }

    /// Address range of the part, see [`JitRegionPart::address_range`].
    pub fn address_range(&self) -> (usize, usize) {
        let start = self.0.shared.slot_ptr(self.0.index, 0).as_ptr() as usize;
        (start, self.slots() * ARENA_SLOT_BYTES)
    }

    /// Publishes raw bytes into a free slot of this part. The bytes are written
    /// in this thread's write window, the range is invalidated and an `ISB` is
    /// issued (M4). Returns `AllocationFailed` when no slot is free and
    /// `InvalidBlockArtifact` for an empty or oversized block. Calling the
    /// bytes stays unsafe and needs its own entry contract.
    pub fn publish(&self, code: &[u8]) -> Result<PublishedCode, ExecutableMemoryError> {
        PublishedCode::publish_in(code, Some(self))
    }

    /// Takes a free slot and writes `code` into it.
    pub(crate) fn allocate_and_write(
        &self,
        code: &[u8],
    ) -> Result<SlotLease, ExecutableMemoryError> {
        if code.is_empty() || code.len() > ARENA_SLOT_BYTES {
            return Err(ExecutableMemoryError::InvalidBlockArtifact);
        }
        // Safe point: the previous block of this part is not executing.
        self.0.drain_pending();
        let (slot, generation) = {
            let mut state = self.0.state.borrow_mut();
            let Some(slot) = state.free.pop() else {
                self.0
                    .failed_allocations
                    .set(self.0.failed_allocations.get() + 1);
                return Err(ExecutableMemoryError::AllocationFailed);
            };
            let Some(generation) = state.generations[slot].checked_add(1) else {
                state.free.push(slot);
                self.0
                    .failed_allocations
                    .set(self.0.failed_allocations.get() + 1);
                return Err(ExecutableMemoryError::AllocationFailed);
            };
            state.generations[slot] = generation;
            state.live[slot] = true;
            (slot, generation)
        };
        self.0.allocations.set(self.0.allocations.get() + 1);
        let ptr = self.0.shared.slot_ptr(self.0.index, slot);
        // The lease exists before the write, so a panic cannot leak the slot.
        let lease = SlotLease {
            part: Rc::clone(&self.0),
            slot,
            generation,
            ptr,
        };
        // SAFETY: `ptr` is the start of a slot of this part that this thread
        // just took off the free list; the slot has `ARENA_SLOT_BYTES` bytes of
        // which `code.len()` are written, and the region is live (the part owns
        // an `Arc` of it).
        unsafe { crate::write_mapping_bytes(ptr, code) };
        instruction_synchronization_barrier();
        Ok(lease)
    }
}

/// One slot taken by one [`PublishedCode`]. Dropping it (on the owner thread,
/// which is the only thread it can be dropped on) frees the slot.
pub(crate) struct SlotLease {
    part: Rc<PartInner>,
    slot: usize,
    generation: u64,
    ptr: NonNull<u8>,
}

impl SlotLease {
    pub(crate) fn ptr(&self) -> NonNull<u8> {
        self.ptr
    }

    /// Generation of this use of the slot (grows on every reuse).
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.generation
    }
}

impl Drop for SlotLease {
    fn drop(&mut self) {
        self.part.release(self.slot, self.generation);
    }
}

/// `ISB` on the calling core: the context synchronization the executing thread
/// needs before its first entry into freshly written code (M4).
fn instruction_synchronization_barrier() {
    // SAFETY: `isb` touches no memory and no register the compiler tracks.
    unsafe {
        std::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}
