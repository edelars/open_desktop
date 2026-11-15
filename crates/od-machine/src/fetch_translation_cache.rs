//! S4-13: opt-in translation cache for the instruction fetch of the CPL0 set board.
//!
//! The Linux board fetches one instruction byte at a time and, before every instruction, peeks
//! at the bytes at RIP (CPUID boundary, `IRETQ` rule). Each of those is a full four-level walk
//! (about seven per instruction, measured), although the bytes of one instruction almost always
//! lie on one 4 KiB page. This cache remembers the result of a completed fetch walk: the
//! physical frame of one linear page. A hit reads the byte from physical memory (the byte itself
//! is never cached, so self-modifying code is seen) and walks nothing.
//!
//! The cache changes no architectural effect:
//! - Accessed bits: an entry is created only after a real fetch walk that succeeded, so the
//!   accessed bit of every level of its path is already set. A repeated uncached walk finds those
//!   bits set and writes nothing; a hit writes nothing too. Failures are never cached: a faulting
//!   fetch always walks.
//! - Page-table writes: an entry watches the physical frames of the tables on its path. Every
//!   committed RAM write reaches [`FetchTranslationCache::note_write`] (it is wired into the
//!   `committed_write` callback of the adapter, which is the single notification of
//!   `PhysicalMemory::write_bytes`); a write that touches a watched frame (a guest store, or an
//!   accessed/dirty commit of another walk) drops every entry. Entries are created only when the
//!   frame and its tables are plain RAM, whose writes always notify.
//! - Context: the key holds the paging snapshot (CR0/CR3/CR4/EFER) and the access context
//!   (mode, privilege, origin). The board flushes the cache on every translation context change
//!   (`MOV CR3` or any other change the set reports).
//! - Lifetime: one cache lives inside one call of the set loop, so a device that writes guest RAM
//!   between calls (NVMe DMA) can never leave a stale entry behind.
//!
//! The cache is off unless the caller asks for it.
//!
//! S4-14h: the invalidation policy is a choice of the caller ([`FetchCacheInvalidation`]).
//! `WriteWatch` (the S4-13 behaviour, the default of every caller) drops entries on a write to a
//! watched table frame, so the cache is exact against a TLB-less reference. `GuestOnly` models a
//! TLB: a write to a page table does not touch an entry, only the invalidations the guest itself
//! performs do (`INVLPG`, `MOV CR3`, a change of CR0/CR4, and a delivered `#PF` for its own
//! page), so a stale translation may still be used until the guest asks for a flush, as on hardware.

use crate::page_walk::Long64ProbeConfig;
use crate::physical_memory::{GuestPhysicalRange, RamFrame};
use od_isa::memory::AccessContext;
use std::cell::RefCell;

/// Entries kept at once. Kernel text and the immediate callee pages are a handful of pages.
const SLOTS: usize = 4;
/// A table slot that holds no table (a 2 MiB leaf ends the walk one level early).
const NO_TABLE: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct Entry {
    linear_page: u64,
    paging: Long64ProbeConfig,
    context: AccessContext,
    /// The plain-RAM 4 KiB frame the linear page maps to.
    frame: RamFrame,
    /// Physical page numbers (address >> 12) of the tables on the path, [`NO_TABLE`] when unused.
    tables: [u64; 4],
}

/// How the cache learns that a cached translation may be stale (S4-14h).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FetchCacheInvalidation {
    /// S4-13: a committed write to a watched table frame drops every entry, besides the explicit
    /// flush of a translation context change. Needs no guest cooperation.
    #[default]
    WriteWatch,
    /// A write to a table frame drops nothing (it is only counted); the caller flushes on the
    /// guest invalidations: `MOV CR3`, `INVLPG`, a change of CR0 or CR4, and a `#PF` delivered
    /// to the guest (a page fault invalidates the translations of its address; the whole cache
    /// is dropped).
    #[cfg_attr(not(test), allow(dead_code))]
    GuestOnly,
}

/// Counters of one cache, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FetchCacheStats {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) inserts: u64,
    /// Entries dropped by a write to a watched table frame.
    pub(crate) write_flushes: u64,
    /// Explicit flushes (translation context changes).
    pub(crate) context_flushes: u64,
    /// `GuestOnly`: flushes after a retired `INVLPG`.
    pub(crate) invlpg_flushes: u64,
    /// `GuestOnly`: flushes after a change of CR0 or CR4.
    pub(crate) control_flushes: u64,
    /// `GuestOnly`: flushes at a `#PF` delivered to the guest.
    pub(crate) fault_flushes: u64,
    /// `GuestOnly`: committed writes that touched a watched table frame and dropped nothing.
    pub(crate) ignored_table_writes: u64,
}

#[derive(Default)]
struct Inner {
    entries: [Option<Entry>; SLOTS],
    /// Slot of the last hit or insert, tried first: consecutive bytes share their page.
    recent: usize,
    next: usize,
    stats: FetchCacheStats,
}

#[derive(Default)]
pub(crate) struct FetchTranslationCache {
    inner: RefCell<Inner>,
    invalidation: FetchCacheInvalidation,
}

impl FetchTranslationCache {
    /// A cache with the S4-13 policy ([`FetchCacheInvalidation::WriteWatch`]).
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_invalidation(invalidation: FetchCacheInvalidation) -> Self {
        Self {
            invalidation,
            ..Self::default()
        }
    }

    #[cfg(test)]
    pub(crate) fn invalidation(&self) -> FetchCacheInvalidation {
        self.invalidation
    }

    /// The RAM frame of `address` when its page has a live entry for exactly this paging
    /// snapshot and access context; the byte is at `address & 0xfff` in it.
    pub(crate) fn lookup(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
    ) -> Option<RamFrame> {
        let page = address >> 12;
        let mut inner = self.inner.borrow_mut();
        let matches = |slot: &Option<Entry>| {
            slot.as_ref().is_some_and(|entry| {
                entry.linear_page == page && entry.paging == *paging && entry.context == context
            })
        };
        let slot = if matches(&inner.entries[inner.recent]) {
            Some(inner.recent)
        } else {
            (0..SLOTS).find(|slot| matches(&inner.entries[*slot]))
        };
        match slot.and_then(|slot| inner.entries[slot].map(|entry| (slot, entry.frame))) {
            Some((slot, frame)) => {
                inner.recent = slot;
                inner.stats.hits += 1;
                Some(frame)
            }
            None => {
                inner.stats.misses += 1;
                None
            }
        }
    }

    /// Remember the result of a completed, successful fetch walk of `address`: `frame` is the
    /// plain-RAM frame it translated to and `tables` the physical bases of the tables walked.
    pub(crate) fn insert(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
        frame: RamFrame,
        tables: [Option<u64>; 4],
    ) {
        let mut inner = self.inner.borrow_mut();
        let page = address >> 12;
        let entry = Entry {
            linear_page: page,
            paging: *paging,
            context,
            frame,
            tables: tables.map(|table| table.map_or(NO_TABLE, |base| base >> 12)),
        };
        // A key already present is replaced in place; otherwise the oldest slot goes.
        let slot = inner
            .entries
            .iter()
            .position(|held| {
                held.is_some_and(|held| {
                    held.linear_page == page && held.paging == *paging && held.context == context
                })
            })
            .unwrap_or_else(|| {
                let next = inner.next;
                inner.next = (next + 1) % SLOTS;
                next
            });
        inner.entries[slot] = Some(entry);
        inner.recent = slot;
        inner.stats.inserts += 1;
    }

    /// A committed RAM write reached memory: with `WriteWatch` drop every entry if it touched a
    /// watched table; with `GuestOnly` only count such a write.
    pub(crate) fn note_write(&self, range: GuestPhysicalRange) {
        let first = range.start().get() >> 12;
        let last = ((range.end_exclusive() - 1) >> 12) as u64;
        let mut inner = self.inner.borrow_mut();
        let touched = inner.entries.iter().flatten().any(|entry| {
            entry
                .tables
                .iter()
                .any(|table| *table != NO_TABLE && (first..=last).contains(table))
        });
        if !touched {
            return;
        }
        match self.invalidation {
            FetchCacheInvalidation::WriteWatch => {
                inner.entries = [None; SLOTS];
                inner.stats.write_flushes += 1;
            }
            FetchCacheInvalidation::GuestOnly => inner.stats.ignored_table_writes += 1,
        }
    }

    /// Drop every entry after a retired `MOV CR3` (or another translation context change the
    /// set reports), in either policy.
    pub(crate) fn flush(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.context_flushes += 1;
    }

    /// Drop every entry after a retired `INVLPG` (`GuestOnly`; the flush is of the whole cache,
    /// not of the page the operand names).
    pub(crate) fn flush_after_invlpg(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.invlpg_flushes += 1;
    }

    /// Drop every entry when a `#PF` is delivered to the guest (`GuestOnly`).
    pub(crate) fn flush_after_page_fault(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.fault_flushes += 1;
    }

    /// Drop every entry after CR0 or CR4 changed (`GuestOnly`; a toggle of CR4.PGE is a global
    /// TLB flush, and any other change is flushed too, which is always allowed).
    pub(crate) fn flush_after_control_change(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.control_flushes += 1;
    }

    pub(crate) fn stats(&self) -> FetchCacheStats {
        self.inner.borrow().stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physical_memory::{
        GuestPhysicalAddress, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory,
        RamLimits,
    };
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};

    const CR0: u64 = 0x8001_0033;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);

    fn range(start: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(start), len).unwrap()
    }

    fn paging(cr3: u64) -> Long64ProbeConfig {
        Long64ProbeConfig::new(CR0, cr3, CR4, EFER).unwrap()
    }

    fn context(privilege: PrivilegeLevel) -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege,
            origin: AccessOrigin::Explicit,
        }
    }

    /// 64 KiB of RAM at 0 and a ROM page after it.
    fn memory() -> PhysicalMemory {
        let specs = [
            MemoryRegionSpec::new(range(0, 0x1_0000), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(range(0x1_0000, 0x1000), MemoryRegionKind::Rom),
        ];
        PhysicalMemory::new(
            &MachineMemoryMap::new(36, specs.to_vec()).unwrap(),
            RamLimits::new(1 << 20),
        )
        .unwrap()
    }

    fn frame(memory: &PhysicalMemory, base: u64) -> RamFrame {
        memory.ram_frame(GuestPhysicalAddress::new(base)).unwrap()
    }

    const TABLES: [Option<u64>; 4] = [Some(0x1000), Some(0x2000), Some(0x3000), Some(0x4000)];
    const CODE_VA: u64 = 0x0100_0000;

    #[test]
    fn a_hit_needs_the_same_page_paging_snapshot_and_context() {
        let memory = memory();
        let cache = FetchTranslationCache::new();
        let ring0 = context(PrivilegeLevel::Ring0);
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        // Any byte of the page hits; the next page, another CR3 and another privilege do not.
        for offset in [0, 1, 0x7ff, 0xfff] {
            assert_eq!(
                cache.lookup(CODE_VA + offset, &paging(0x1000), ring0),
                Some(frame(&memory, 0x8000))
            );
        }
        assert_eq!(cache.lookup(CODE_VA + 0x1000, &paging(0x1000), ring0), None);
        assert_eq!(cache.lookup(CODE_VA, &paging(0x5000), ring0), None);
        assert_eq!(
            cache.lookup(CODE_VA, &paging(0x1000), context(PrivilegeLevel::Ring3)),
            None
        );
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.misses, stats.inserts), (4, 4, 1));
    }

    #[test]
    fn a_write_to_any_watched_table_frame_drops_every_entry() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        // One frame of each table level, a write inside it or a range that merely reaches into it.
        for (write, flushes) in [
            (range(0x1000, 8), true),
            (range(0x2ff8, 8), true),
            (range(0x3008, 1), true),
            (range(0x4fff, 1), true),
            (range(0x0ff8, 16), true),
            (range(0x0ff8, 8), false),
            (range(0x5000, 0x1000), false),
            (range(0x8000, 8), false),
        ] {
            let cache = FetchTranslationCache::new();
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
            cache.insert(
                CODE_VA + 0x1000,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x9000),
                TABLES,
            );
            cache.note_write(write);
            let live = cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some()
                || cache
                    .lookup(CODE_VA + 0x1000, &paging(0x1000), ring0)
                    .is_some();
            assert_eq!(live, !flushes, "{write:?}");
            assert_eq!(cache.stats().write_flushes, u64::from(flushes), "{write:?}");
        }
    }

    #[test]
    fn guest_only_counts_a_table_write_and_keeps_every_entry_until_a_guest_flush() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::with_invalidation(FetchCacheInvalidation::GuestOnly);
        assert_eq!(cache.invalidation(), FetchCacheInvalidation::GuestOnly);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        // A write into a watched table (counted), one that misses every table (not counted).
        cache.note_write(range(0x3008, 8));
        cache.note_write(range(0x8000, 8));
        assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
        let stats = cache.stats();
        assert_eq!(
            (stats.write_flushes, stats.ignored_table_writes),
            (0, 1),
            "{stats:?}"
        );
        // Each guest invalidation drops the entry and counts on its own counter.
        for flush in [
            FetchTranslationCache::flush_after_invlpg as fn(&FetchTranslationCache),
            FetchTranslationCache::flush_after_control_change,
            FetchTranslationCache::flush_after_page_fault,
            FetchTranslationCache::flush,
        ] {
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
            flush(&cache);
            assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        }
        let stats = cache.stats();
        assert_eq!(
            (
                stats.invlpg_flushes,
                stats.control_flushes,
                stats.fault_flushes,
                stats.context_flushes
            ),
            (1, 1, 1, 1),
            "{stats:?}"
        );
    }

    #[test]
    fn write_watch_is_the_default_policy_and_ignores_nothing() {
        let cache = FetchTranslationCache::new();
        assert_eq!(cache.invalidation(), FetchCacheInvalidation::WriteWatch);
        assert_eq!(FetchCacheInvalidation::default(), cache.invalidation());
        cache.note_write(range(0x3008, 8));
        assert_eq!(cache.stats(), FetchCacheStats::default());
    }

    #[test]
    fn a_two_mib_walk_watches_three_tables_and_not_an_unused_slot() {
        let memory = memory();
        let cache = FetchTranslationCache::new();
        let ring0 = context(PrivilegeLevel::Ring0);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            [Some(0x1000), Some(0x2000), Some(0x3000), None],
        );
        // The unused slot must not match the top frame of the address space.
        cache.note_write(range(u64::MAX - 7, 8));
        assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
        cache.note_write(range(0x3000, 8));
        assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_none());
    }

    #[test]
    fn flush_drops_everything_and_counts() {
        let memory = memory();
        let cache = FetchTranslationCache::new();
        let ring0 = context(PrivilegeLevel::Ring0);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        cache.flush();
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        assert_eq!(cache.stats().context_flushes, 1);
    }

    #[test]
    fn the_oldest_slot_is_replaced_and_a_known_key_is_refreshed_in_place() {
        let memory = memory();
        let cache = FetchTranslationCache::new();
        let ring0 = context(PrivilegeLevel::Ring0);
        let page = |index: u64| CODE_VA + index * 0x1000;
        for index in 0..SLOTS as u64 {
            cache.insert(
                page(index),
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
        }
        // Re-inserting a held page takes no slot: all four stay.
        cache.insert(
            page(1),
            &paging(0x1000),
            ring0,
            frame(&memory, 0xa000),
            TABLES,
        );
        for index in 0..SLOTS as u64 {
            assert!(cache.lookup(page(index), &paging(0x1000), ring0).is_some());
        }
        assert_eq!(
            cache.lookup(page(1), &paging(0x1000), ring0),
            Some(frame(&memory, 0xa000))
        );
        // A fifth page evicts the oldest slot (page 0).
        cache.insert(
            page(9),
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        assert_eq!(cache.lookup(page(0), &paging(0x1000), ring0), None);
        assert!(cache.lookup(page(9), &paging(0x1000), ring0).is_some());
        assert!(cache.lookup(page(1), &paging(0x1000), ring0).is_some());
    }

    #[test]
    fn only_plain_ram_gets_a_frame_handle_and_it_reads_the_same_bytes() {
        let mut memory = memory();
        memory
            .write_u8(GuestPhysicalAddress::new(0x8abc), 0x5a, |_| {})
            .unwrap();
        let ram = frame(&memory, 0x8000);
        assert_eq!(memory.read_ram_frame_u8(ram, 0xabc), Some(0x5a));
        assert_eq!(memory.read_ram_frame_u8(ram, 0xabd), Some(0));
        assert_eq!(
            memory.read_ram_frame_u8(ram, 0xabc),
            memory.read_u8(GuestPhysicalAddress::new(0x8abc)).ok()
        );
        // ROM, a hole and a base that is not frame aligned: no handle.
        for base in [0x1_0000, 0x2_0000, 0x8001] {
            assert_eq!(memory.ram_frame(GuestPhysicalAddress::new(base)), None);
        }
    }
}
