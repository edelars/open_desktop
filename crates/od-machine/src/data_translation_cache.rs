//! PERF-11 (ADR-0016, prototype, step 5 of PERF-11): the translation cache of the data reads of
//! the CPL0 set loop. Only qword reads inside one 4 KiB page are served; nothing else.
//!
//! The policy is the `WriteWatch` policy of the fetch cache (S4-13), with these rules (ADR-0016,
//! section 2):
//!
//! - An entry is made only after a successful tracked read walk (`access_with`): by then the walk
//!   has set the accessed bit of every level, so a hit (which writes nothing) is exactly what the
//!   next walk would do. A failed walk of any kind makes no entry.
//! - The key is the linear 4 KiB page, the page privilege of the normalized context and the whole
//!   paging snapshot (`Long64ProbeConfig`: CR0 with WP, CR3, CR4, EFER with NXE). The class is
//!   always Read: the prototype serves no write.
//! - Only plain RAM: the operand frame and every table frame of the path are `Ram` regions with
//!   backing, so a write to any of them always reaches `note_write` (a `Framebuffer` write does
//!   not notify and is never a cached frame), and a hit never reaches a device.
//! - A committed RAM write (guest store, walker A/D commit, exception frame: every one passes
//!   through the `watch` of the set loop) that touches a table frame of an entry drops every entry
//!   that watches that frame (frame granularity, ADR-0016 section 2 item 6 (a)).
//! - A translation context change of the loop drops everything (`flush`); a lookup or an insert
//!   under another paging snapshot than the one of the entries misses or drops everything first.
//! - The cache lives in one call of the set loop (one quantum), like the fetch cache of that path:
//!   device DMA between quanta cannot leave a stale entry.
//!
//! Layout, for the speed of a hit: 64 direct-mapped sets indexed by the low bits of the linear page
//! number (one compare of a tag, then one compare of the snapshot), and a separate table of up to
//! 16 distinct watched table frames, each with the bit mask of the sets that watch it, plus the
//! lowest and highest watched frame, so that a write outside `[lo, hi]` (nearly every data store)
//! returns after two compares.

use std::cell::Cell;

use od_isa::memory::PrivilegeLevel;

use crate::page_walk::Long64ProbeConfig;
use crate::physical_memory::{GuestPhysicalRange, RamFrame};

/// Direct-mapped sets (a power of two; the watcher masks are `u64`).
const SETS: usize = 64;
/// Distinct table frames watched at once.
const FRAMES: usize = 16;
const _: () = assert!(SETS.is_power_of_two() && SETS <= 64);
const _: () = assert!(FRAMES <= 16);

/// The tag of a set: page base | (privilege << 1) | valid. 0 is the empty set.
#[inline(always)]
fn tag(address: u64, privilege: PrivilegeLevel) -> u64 {
    let privilege = match privilege {
        PrivilegeLevel::Ring0 => 0,
        PrivilegeLevel::Ring1 => 1,
        PrivilegeLevel::Ring2 => 2,
        PrivilegeLevel::Ring3 => 3,
    };
    (address & !0xfff) | (privilege << 1) | 1
}

#[inline(always)]
fn set_of(address: u64) -> usize {
    ((address >> 12) as usize) & (SETS - 1)
}

/// The data translation cache of one call of the set loop (see the module documentation).
/// Interior mutability through `Cell` only: the adapter and the loop's `watch` hold it at once,
/// on one thread (the type is not `Sync`).
pub(crate) struct DataTranslationCache {
    tags: [Cell<u64>; SETS],
    frames: [Cell<Option<RamFrame>>; SETS],
    /// The paging snapshot of every live entry (`None` when there are none).
    paging: Cell<Option<Long64ProbeConfig>>,
    /// Page number of each watched table frame; meaningful when its watcher mask is not 0.
    table_frames: [Cell<u64>; FRAMES],
    /// The sets that watch each table frame (bit `s` = set `s`).
    watchers: [Cell<u64>; FRAMES],
    /// Lowest and highest watched table page; `(u64::MAX, 0)` when nothing is watched. They only
    /// widen between full clears (a superset is exact: the frame table is checked after them).
    lo: Cell<u64>,
    hi: Cell<u64>,
}

impl Default for DataTranslationCache {
    fn default() -> Self {
        Self::new()
    }
}

impl DataTranslationCache {
    pub(crate) fn new() -> Self {
        Self {
            tags: [const { Cell::new(0) }; SETS],
            frames: [const { Cell::new(None) }; SETS],
            paging: Cell::new(None),
            table_frames: [const { Cell::new(0) }; FRAMES],
            watchers: [const { Cell::new(0) }; FRAMES],
            lo: Cell::new(u64::MAX),
            hi: Cell::new(0),
        }
    }

    /// The frame of the cached translation of `address` for this snapshot and page privilege,
    /// or `None` (a miss). The caller has checked that the qword does not cross the page.
    #[inline(always)]
    pub(crate) fn lookup(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
    ) -> Option<RamFrame> {
        let set = set_of(address);
        if self.tags[set].get() == tag(address, privilege) && self.paging.get() == Some(*paging) {
            debug_counters::note(|c| c.hits += 1);
            return self.frames[set].get();
        }
        debug_counters::note(|c| c.misses += 1);
        None
    }

    /// Remembers the translation of `address` made by a successful read walk under `paging` at
    /// `privilege`: `frame` is the plain-RAM 4 KiB frame of the operand, `tables` the page
    /// numbers of the table frames of the path (all plain RAM; the caller checked both).
    pub(crate) fn insert(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
        frame: RamFrame,
        tables: [Option<u64>; 4],
    ) {
        if self.paging.get() != Some(*paging) {
            self.clear();
            self.paging.set(Some(*paging));
        }
        let set = set_of(address);
        self.evict(set);
        if !self.watch(set, tables) {
            // No room for a new table frame: start again with everything dropped (the 16 slots
            // hold the at most 4 frames of one path).
            self.clear();
            self.paging.set(Some(*paging));
            let watched = self.watch(set, tables);
            debug_assert!(watched);
        }
        self.tags[set].set(tag(address, privilege));
        self.frames[set].set(Some(frame));
        debug_counters::note(|c| c.inserts += 1);
    }

    /// Adds `set` to the watchers of each table frame; `false` (with the watchers of the frames
    /// before the one without room already added, which the caller's clear drops) when a frame
    /// found no free slot.
    fn watch(&self, set: usize, tables: [Option<u64>; 4]) -> bool {
        let bit = 1u64 << set;
        for table in tables.into_iter().flatten() {
            let mut free = None;
            let mut found = None;
            for slot in 0..FRAMES {
                let watchers = self.watchers[slot].get();
                if watchers == 0 {
                    free = free.or(Some(slot));
                } else if self.table_frames[slot].get() == table {
                    found = Some(slot);
                    break;
                }
            }
            let Some(slot) = found.or(free) else {
                return false;
            };
            self.table_frames[slot].set(table);
            self.watchers[slot].set(self.watchers[slot].get() | bit);
            self.lo.set(self.lo.get().min(table));
            self.hi.set(self.hi.get().max(table));
        }
        true
    }

    /// Empties `set` and removes it from every watcher mask.
    fn evict(&self, set: usize) {
        if self.tags[set].get() == 0 {
            return;
        }
        self.tags[set].set(0);
        self.frames[set].set(None);
        let keep = !(1u64 << set);
        for watchers in &self.watchers {
            watchers.set(watchers.get() & keep);
        }
    }

    fn clear(&self) {
        for tag in &self.tags {
            tag.set(0);
        }
        for watchers in &self.watchers {
            watchers.set(0);
        }
        self.paging.set(None);
        self.lo.set(u64::MAX);
        self.hi.set(0);
    }

    /// A committed RAM write reached memory: every entry that watches a table frame the write
    /// touched is dropped.
    #[inline(always)]
    pub(crate) fn note_write(&self, range: GuestPhysicalRange) {
        let first = range.start().get() >> 12;
        if first > self.hi.get() {
            return;
        }
        let last = ((range.end_exclusive() - 1) >> 12) as u64;
        if last < self.lo.get() {
            return;
        }
        self.note_table_write(first, last);
    }

    #[cold]
    #[inline(never)]
    fn note_table_write(&self, first: u64, last: u64) {
        let mut dropped = 0u64;
        for slot in 0..FRAMES {
            let watchers = self.watchers[slot].get();
            if watchers != 0 && (first..=last).contains(&self.table_frames[slot].get()) {
                dropped |= watchers;
            }
        }
        if dropped == 0 {
            return;
        }
        let mut sets = dropped;
        while sets != 0 {
            let set = sets.trailing_zeros() as usize;
            self.tags[set].set(0);
            self.frames[set].set(None);
            sets &= sets - 1;
        }
        for watchers in &self.watchers {
            watchers.set(watchers.get() & !dropped);
        }
        debug_counters::note(|c| c.write_flushes += 1);
    }

    /// A translation context change of the loop (`MOV CR3`, an EFER or CR0/CR4 change): drop
    /// every entry.
    pub(crate) fn flush(&self) {
        self.clear();
        debug_counters::note(|c| c.context_flushes += 1);
    }

    /// The number of live entries (tests).
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.tags.iter().filter(|tag| tag.get() != 0).count()
    }
}

/// PERF-11: counters of the data translation cache of this thread, kept in debug builds only (a
/// release build carries no counter on the hit path). All zero in a release build.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCacheCounters {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub write_flushes: u64,
    pub context_flushes: u64,
}

/// Whether [`data_cache_counters`] counts (debug builds).
pub const DATA_CACHE_COUNTED: bool = cfg!(debug_assertions);

/// The counters of this thread since the last [`reset_data_cache_counters`].
pub fn data_cache_counters() -> DataCacheCounters {
    debug_counters::get()
}

/// Clears [`data_cache_counters`] of this thread.
pub fn reset_data_cache_counters() {
    debug_counters::reset();
}

mod debug_counters {
    use super::DataCacheCounters;

    #[cfg(debug_assertions)]
    thread_local! {
        static COUNTERS: std::cell::Cell<DataCacheCounters> =
            const { std::cell::Cell::new(DataCacheCounters {
                hits: 0,
                misses: 0,
                inserts: 0,
                write_flushes: 0,
                context_flushes: 0,
            }) };
    }

    #[inline(always)]
    pub(super) fn note(update: impl FnOnce(&mut DataCacheCounters)) {
        #[cfg(debug_assertions)]
        COUNTERS.with(|cell| {
            let mut counters = cell.get();
            update(&mut counters);
            cell.set(counters);
        });
        #[cfg(not(debug_assertions))]
        let _ = update;
    }

    pub(super) fn get() -> DataCacheCounters {
        #[cfg(debug_assertions)]
        {
            COUNTERS.with(std::cell::Cell::get)
        }
        #[cfg(not(debug_assertions))]
        {
            DataCacheCounters::default()
        }
    }

    pub(super) fn reset() {
        #[cfg(debug_assertions)]
        COUNTERS.with(|cell| cell.set(DataCacheCounters::default()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        GuestPhysicalAddress, MachineMemoryMap, MemoryRegionKind, MemoryRegionSpec, PhysicalMemory,
        RamLimits,
    };

    const CR0: u64 = 0x8001_0033;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);

    fn paging(cr3: u64) -> Long64ProbeConfig {
        Long64ProbeConfig::new(CR0, cr3, CR4, EFER).unwrap()
    }

    fn memory() -> PhysicalMemory {
        let range = GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 0x40_0000).unwrap();
        let map = MachineMemoryMap::new(
            36,
            vec![MemoryRegionSpec::new(range, MemoryRegionKind::Ram)],
        )
        .unwrap();
        PhysicalMemory::new(&map, RamLimits::new(0x40_0000)).unwrap()
    }

    fn frame(memory: &PhysicalMemory, base: u64) -> RamFrame {
        memory.ram_frame(GuestPhysicalAddress::new(base)).unwrap()
    }

    fn write(address: u64, len: u64) -> GuestPhysicalRange {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(address), len).unwrap()
    }

    const TABLES: [Option<u64>; 4] = [Some(1), Some(2), Some(3), Some(4)];

    #[test]
    fn a_hit_needs_the_page_the_privilege_and_the_snapshot_of_the_entry() {
        let memory = memory();
        let cache = DataTranslationCache::new();
        let page = 0x0080_0000;
        cache.insert(
            page,
            &paging(0x1000),
            PrivilegeLevel::Ring0,
            frame(&memory, 0x7000),
            TABLES,
        );
        assert_eq!(
            cache.lookup(page + 0xff8, &paging(0x1000), PrivilegeLevel::Ring0),
            Some(frame(&memory, 0x7000))
        );
        assert_eq!(
            cache.lookup(page + 0x1000, &paging(0x1000), PrivilegeLevel::Ring0),
            None
        );
        // Another page of the same set.
        assert_eq!(
            cache.lookup(page + 0x4_0000, &paging(0x1000), PrivilegeLevel::Ring0),
            None
        );
        assert_eq!(
            cache.lookup(page, &paging(0x1000), PrivilegeLevel::Ring3),
            None
        );
        assert_eq!(
            cache.lookup(page, &paging(0x9000), PrivilegeLevel::Ring0),
            None
        );
        let no_wp = Long64ProbeConfig::new(CR0 & !(1 << 16), 0x1000, CR4, EFER).unwrap();
        assert_eq!(cache.lookup(page, &no_wp, PrivilegeLevel::Ring0), None);
        let no_nxe = Long64ProbeConfig::new(CR0, 0x1000, CR4, EFER & !(1 << 11)).unwrap();
        assert_eq!(cache.lookup(page, &no_nxe, PrivilegeLevel::Ring0), None);
        // An insert under another snapshot drops the entries of the old one.
        cache.insert(
            page + 0x1000,
            &paging(0x9000),
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        assert_eq!(
            cache.lookup(page, &paging(0x1000), PrivilegeLevel::Ring0),
            None
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn a_write_to_a_watched_table_frame_drops_only_the_entries_that_watch_it() {
        let memory = memory();
        let cache = DataTranslationCache::new();
        let config = paging(0x1000);
        let (a, b) = (0x0080_0000, 0x0080_1000);
        cache.insert(
            a,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x7000),
            [Some(1), Some(2), Some(3), Some(4)],
        );
        cache.insert(
            b,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            [Some(1), Some(2), Some(3), Some(5)],
        );
        // A data write (outside every table frame) and a write next to the frames change nothing.
        cache.note_write(write(0x7000, 8));
        cache.note_write(write(0x6ff8, 8));
        assert_eq!(cache.len(), 2);
        // The last byte of the PT of `b` drops `b` alone.
        cache.note_write(write(0x5fff, 1));
        assert!(cache.lookup(a, &config, PrivilegeLevel::Ring0).is_some());
        assert!(cache.lookup(b, &config, PrivilegeLevel::Ring0).is_none());
        // A write that spans from below into the shared PML4 frame drops everything.
        cache.insert(
            b,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            [Some(1), Some(2), Some(3), Some(5)],
        );
        cache.note_write(write(0x0ffc, 8));
        assert_eq!(cache.len(), 0);
        // The frame slots are free again: 16 new frames fit.
        for index in 0..4u64 {
            let tables = [
                Some(0x100 + index * 4),
                Some(0x101 + index * 4),
                Some(0x102 + index * 4),
                Some(0x103 + index * 4),
            ];
            cache.insert(
                a + index * 0x1000,
                &config,
                PrivilegeLevel::Ring0,
                frame(&memory, 0x7000),
                tables,
            );
        }
        assert_eq!(cache.len(), 4);
        // A 17th frame clears everything and keeps the new entry.
        cache.insert(
            a + 0x10_000,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x7000),
            [Some(0x200), None, None, None],
        );
        assert_eq!(cache.len(), 1);
        assert!(
            cache
                .lookup(a + 0x10_000, &config, PrivilegeLevel::Ring0)
                .is_some()
        );
    }

    #[test]
    fn an_evicted_entry_no_longer_holds_its_frames_and_a_flush_drops_everything() {
        let memory = memory();
        let cache = DataTranslationCache::new();
        let config = paging(0x1000);
        let a = 0x0080_0000;
        let same_set = a + (SETS as u64) * 0x1000;
        cache.insert(
            a,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x7000),
            [Some(0x10), Some(0x11), None, None],
        );
        cache.insert(
            same_set,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            [Some(0x20), Some(0x21), None, None],
        );
        assert!(cache.lookup(a, &config, PrivilegeLevel::Ring0).is_none());
        // The frames of the evicted entry are not watched any more: writing them drops nothing.
        cache.note_write(write(0x10_000, 0x2000));
        assert!(
            cache
                .lookup(same_set, &config, PrivilegeLevel::Ring0)
                .is_some()
        );
        cache.flush();
        assert_eq!(cache.len(), 0);
        assert!(
            cache
                .lookup(same_set, &config, PrivilegeLevel::Ring0)
                .is_none()
        );
    }
}
