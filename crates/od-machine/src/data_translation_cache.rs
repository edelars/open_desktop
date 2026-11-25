//! PERF-11 (ADR-0016) and PERF-12 (E8 of the second part of stage E, ADR-0016, section "Срез
//! записи PERF-12"): the translation cache of the data accesses of the CPL0 set loop. Reads of 1, 2,
//! 4 and 8 bytes and `Explicit` writes of 1, 2, 4 and 8 bytes inside one 4 KiB page are served;
//! nothing else.
//!
//! The policy is the `WriteWatch` policy of the fetch cache (S4-13), with these rules (ADR-0016,
//! section 2):
//!
//! - An entry is made only after a successful tracked walk (`access_with`) of a read or of a
//!   write. A read walk has set the accessed bit of every level, so a read hit (which writes
//!   nothing) is exactly what the next walk would do. A write walk has also set the dirty bit of
//!   the leaf (`commit_leaf_dirty`), so a write hit writes only the bytes of the operand, through
//!   `PhysicalMemory::store_bytes` and the loop's `watch`, as the walk does. A failed walk of any
//!   kind makes no entry.
//! - The key is the linear 4 KiB page, the page privilege of the normalized context and the whole
//!   paging snapshot (`Long64ProbeConfig`: CR0 with WP, CR3, CR4, EFER with NXE). The class is in
//!   the tag: a Write entry serves reads and writes, a Read entry serves reads only (a write misses,
//!   walks, and its Write entry replaces the Read one).
//! - Only plain RAM: the operand frame and every table frame of the path are `Ram` regions with
//!   backing, so a write to any of them always reaches `note_write` (a `Framebuffer` write does
//!   not notify and is never a cached frame), and a hit never reaches a device. A Write entry is
//!   not made when the operand frame is one of the table frames of its own path (the stored bytes
//!   may have changed an entry the walk read).
//! - A committed RAM write (guest store, walker A/D commit, exception frame: every one passes
//!   through the `watch` of the set loop) that touches a table frame of an entry drops every entry
//!   that watches that frame (frame granularity, ADR-0016 section 2 item 6 (a)).
//! - A translation context change of the loop drops everything (`flush`); a lookup or an insert
//!   under another paging snapshot than the one of the entries misses or drops everything first.
//! - The cache lives in one call of the set loop (one quantum), like the fetch cache of that path:
//!   device DMA between quanta cannot leave a stale entry (ADR-0016, decision Д6).
//! - With the check (`--data-cache-verify`, [`DataTranslationCache::checking`]) every hit is
//!   compared, before its entry is used, with a walk that writes nothing (see [`check`]).
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

/// The table page number of a level past a 2 MiB leaf in [`DataTranslationCache::tables`].
const NO_TABLE: u64 = u64::MAX;

/// The class bit of a tag: the entry was made by a write walk (its leaf has D set) and serves
/// writes as well as reads.
const WRITE_CLASS: u64 = 1 << 3;

/// The tag of a set: page base | (privilege << 1) | valid, plus [`WRITE_CLASS`] for a Write entry.
/// 0 is the empty set.
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

/// E8: what the board asks of the data translation cache of one call of the set loop.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum DataCacheUse {
    /// No data cache (every caller but the board's unified run).
    #[default]
    Off,
    /// `--data-cache-prototype`.
    On,
    /// `--data-cache-verify`: the cache, with every hit checked ([`DataTranslationCache::checking`]).
    Verify,
}

impl DataCacheUse {
    /// The cache of one call, or `None`.
    pub(crate) fn cache(self) -> Option<DataTranslationCache> {
        match self {
            Self::Off => None,
            Self::On => Some(DataTranslationCache::new()),
            Self::Verify => Some(DataTranslationCache::checking()),
        }
    }
}

/// What an entry holds, for the check of a hit ([`DataTranslationCache::entry`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DataCacheEntry {
    pub(crate) frame: Option<RamFrame>,
    /// Physical base of the 4 KiB frame of the page.
    pub(crate) physical: u64,
    /// Page numbers of the table frames of the path (PML4 first; `None` past a 2 MiB leaf).
    pub(crate) tables: [Option<u64>; 4],
    pub(crate) write: bool,
}

/// The data translation cache of one call of the set loop (see the module documentation).
/// Interior mutability through `Cell` only: the adapter and the loop's `watch` hold it at once,
/// on one thread (the type is not `Sync`).
pub(crate) struct DataTranslationCache {
    tags: [Cell<u64>; SETS],
    frames: [Cell<Option<RamFrame>>; SETS],
    /// Physical base of the 4 KiB frame of each entry (the store of a write hit, the check).
    physical: [Cell<u64>; SETS],
    /// The table page numbers of each entry's path, [`NO_TABLE`] past a 2 MiB leaf (the check
    /// compares them with a fresh walk).
    tables: [Cell<[u64; 4]>; SETS],
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
    /// `--data-cache-verify`: every hit is checked against a walk that writes nothing.
    verify: bool,
    /// Test-only mutation switches (E8, step 10): each drops one rule, for the tests that prove it
    /// is needed. A release build has none of them.
    #[cfg(test)]
    mutations: Cell<DataCacheMutations>,
}

/// E8 (step 10): the test-only mutations of the data translation cache.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct DataCacheMutations {
    /// A Read entry serves a write (the dirty bit of the leaf is never checked).
    pub(crate) read_serves_write: bool,
    /// A write into a watched table frame does not drop the Write entries that watch it.
    pub(crate) write_entries_unwatched: bool,
    /// A Write entry is made even when the operand frame is a table frame of its own path.
    pub(crate) own_table_frame_allowed: bool,
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
            physical: [const { Cell::new(0) }; SETS],
            tables: [const { Cell::new([NO_TABLE; 4]) }; SETS],
            paging: Cell::new(None),
            table_frames: [const { Cell::new(0) }; FRAMES],
            watchers: [const { Cell::new(0) }; FRAMES],
            lo: Cell::new(u64::MAX),
            hi: Cell::new(0),
            verify: false,
            #[cfg(test)]
            mutations: Cell::new(DataCacheMutations::default()),
        }
    }

    /// E8 (step 8): the cache of `--data-cache-verify`: it serves as [`Self::new`], and the adapter
    /// checks every hit before it uses the entry ([`check`]).
    pub(crate) fn checking() -> Self {
        Self {
            verify: true,
            ..Self::new()
        }
    }

    /// Whether every hit is checked (`--data-cache-verify`).
    #[inline(always)]
    pub(crate) fn verify(&self) -> bool {
        self.verify
    }

    #[cfg(test)]
    pub(crate) fn mutate_for_test(&self, mutations: DataCacheMutations) {
        self.mutations.set(mutations);
    }

    /// The test-only mutations of this cache (none in a release build).
    #[cfg(test)]
    pub(crate) fn mutations(&self) -> DataCacheMutations {
        self.mutations.get()
    }

    /// The set of the cached translation of `address` for a READ under this snapshot and page
    /// privilege (an entry of either class), or `None` (a miss). The caller has checked that the
    /// access does not cross the page.
    #[inline(always)]
    pub(crate) fn lookup(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
    ) -> Option<usize> {
        let set = set_of(address);
        if self.tags[set].get() & !WRITE_CLASS == tag(address, privilege)
            && self.paging.get() == Some(*paging)
        {
            debug_counters::note(|c| c.hits += 1);
            return Some(set);
        }
        debug_counters::note(|c| c.misses += 1);
        None
    }

    /// The set of the cached translation of `address` for a WRITE: only a Write entry serves it.
    #[inline(always)]
    pub(crate) fn lookup_write(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
    ) -> Option<usize> {
        let set = set_of(address);
        #[cfg(test)]
        let wanted = if self.mutations.get().read_serves_write {
            self.tags[set].get() | WRITE_CLASS
        } else {
            self.tags[set].get()
        };
        #[cfg(not(test))]
        let wanted = self.tags[set].get();
        if wanted == tag(address, privilege) | WRITE_CLASS && self.paging.get() == Some(*paging) {
            debug_counters::note(|c| c.write_hits += 1);
            return Some(set);
        }
        debug_counters::note(|c| c.write_misses += 1);
        None
    }

    /// The plain-RAM frame handle of the entry in `set` (a read hit).
    #[inline(always)]
    pub(crate) fn frame(&self, set: usize) -> Option<RamFrame> {
        self.frames[set].get()
    }

    /// The physical base of the 4 KiB frame of the entry in `set` (a write hit).
    #[inline(always)]
    pub(crate) fn physical(&self, set: usize) -> u64 {
        self.physical[set].get()
    }

    /// Everything the entry in `set` holds (the check of a hit).
    pub(crate) fn entry(&self, set: usize) -> DataCacheEntry {
        DataCacheEntry {
            frame: self.frames[set].get(),
            physical: self.physical[set].get(),
            tables: self.tables[set]
                .get()
                .map(|table| (table != NO_TABLE).then_some(table)),
            write: self.tags[set].get() & WRITE_CLASS != 0,
        }
    }

    /// Remembers the translation of `address` made by a successful walk under `paging` at
    /// `privilege`: `frame` is the plain-RAM 4 KiB frame of the operand and `physical` its base,
    /// `tables` the page numbers of the table frames of the path (all plain RAM; the caller checked
    /// both). `write`: the walk was a write (its leaf has D set), the entry serves writes too.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
        frame: RamFrame,
        physical: u64,
        tables: [Option<u64>; 4],
        write: bool,
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
        let class = if write { WRITE_CLASS } else { 0 };
        self.tags[set].set(tag(address, privilege) | class);
        self.frames[set].set(Some(frame));
        self.physical[set].set(physical);
        self.tables[set].set(tables.map(|table| table.unwrap_or(NO_TABLE)));
        if write {
            debug_counters::note(|c| c.write_inserts += 1);
        } else {
            debug_counters::note(|c| c.inserts += 1);
        }
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
        #[cfg(test)]
        if self.mutations.get().write_entries_unwatched {
            let mut sets = dropped;
            while sets != 0 {
                let set = sets.trailing_zeros() as usize;
                if self.tags[set].get() & WRITE_CLASS != 0 {
                    dropped &= !(1u64 << set);
                }
                sets &= sets - 1;
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

    /// The number of live Write entries (tests).
    #[cfg(test)]
    pub(crate) fn write_len(&self) -> usize {
        self.tags
            .iter()
            .filter(|tag| tag.get() & WRITE_CLASS != 0)
            .count()
    }
}

/// PERF-11: counters of the data translation cache of this thread, kept in debug builds only (a
/// release build carries no counter on the hit path). All zero in a release build. `hits`,
/// `misses` and `inserts` count reads (of every width since E8), the `write_*` ones writes (E8).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataCacheCounters {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    pub write_hits: u64,
    pub write_misses: u64,
    pub write_inserts: u64,
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
                write_hits: 0,
                write_misses: 0,
                write_inserts: 0,
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

/// E8 (step 8, ADR-0016 decision Д5): the check of `--data-cache-verify`. The adapter checks each
/// hit before it uses the entry (`PagedMovMemoryAdapter::check_data_cache_hit`) with a walk that
/// writes nothing and counts here; the first divergence is kept and ends the run (the access
/// returns `MemoryError::Backend` with its text). The loop runs on the thread of its caller, so the
/// counters are that thread's (as those of the block check).
pub mod check {
    use std::cell::RefCell;

    /// What the check did since the last [`reset_data_cache_check`].
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct DataCacheCheckCounters {
        /// Read hits checked.
        pub verified_reads: u64,
        /// Write hits checked.
        pub verified_writes: u64,
        pub divergences: u64,
    }

    /// The first hit whose entry differs from the walk.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct DataCacheCheckDivergence {
        /// The linear address of the access.
        pub linear: u64,
        /// The RIP of the instruction.
        pub rip: u64,
        pub write: bool,
        /// What differs, with both values.
        pub reason: String,
    }

    impl std::fmt::Display for DataCacheCheckDivergence {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(
                f,
                "data cache check: the {} hit at linear {:#x} (rip {:#x}) differs from the walk: {}",
                if self.write { "write" } else { "read" },
                self.linear,
                self.rip,
                self.reason
            )
        }
    }

    thread_local! {
        static CHECK: RefCell<(DataCacheCheckCounters, Option<DataCacheCheckDivergence>)> =
            const { RefCell::new((
                DataCacheCheckCounters { verified_reads: 0, verified_writes: 0, divergences: 0 },
                None,
            )) };
    }

    /// Clears the counters and the divergence of this thread.
    pub fn reset_data_cache_check() {
        CHECK.with(|check| *check.borrow_mut() = (DataCacheCheckCounters::default(), None));
    }

    /// The counters of this thread.
    pub fn data_cache_check_counters() -> DataCacheCheckCounters {
        CHECK.with(|check| check.borrow().0)
    }

    /// The first divergence of this thread, taken.
    pub fn take_data_cache_check_divergence() -> Option<DataCacheCheckDivergence> {
        CHECK.with(|check| check.borrow_mut().1.take())
    }

    /// One checked hit: `divergence` is `None` when the entry agrees with the walk.
    pub(crate) fn note(write: bool, divergence: Option<DataCacheCheckDivergence>) {
        CHECK.with(|check| {
            let mut check = check.borrow_mut();
            if write {
                check.0.verified_writes += 1;
            } else {
                check.0.verified_reads += 1;
            }
            if let Some(divergence) = divergence {
                check.0.divergences += 1;
                if check.1.is_none() {
                    check.1 = Some(divergence);
                }
            }
        });
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

    fn hit(
        cache: &DataTranslationCache,
        address: u64,
        paging: &Long64ProbeConfig,
        privilege: PrivilegeLevel,
    ) -> Option<RamFrame> {
        cache
            .lookup(address, paging, privilege)
            .and_then(|set| cache.frame(set))
    }

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
            0x7000,
            TABLES,
            false,
        );
        assert_eq!(
            hit(&cache, page + 0xff8, &paging(0x1000), PrivilegeLevel::Ring0),
            Some(frame(&memory, 0x7000))
        );
        assert_eq!(
            hit(
                &cache,
                page + 0x1000,
                &paging(0x1000),
                PrivilegeLevel::Ring0
            ),
            None
        );
        // Another page of the same set.
        assert_eq!(
            hit(
                &cache,
                page + 0x4_0000,
                &paging(0x1000),
                PrivilegeLevel::Ring0
            ),
            None
        );
        assert_eq!(
            hit(&cache, page, &paging(0x1000), PrivilegeLevel::Ring3),
            None
        );
        assert_eq!(
            hit(&cache, page, &paging(0x9000), PrivilegeLevel::Ring0),
            None
        );
        let no_wp = Long64ProbeConfig::new(CR0 & !(1 << 16), 0x1000, CR4, EFER).unwrap();
        assert_eq!(hit(&cache, page, &no_wp, PrivilegeLevel::Ring0), None);
        let no_nxe = Long64ProbeConfig::new(CR0, 0x1000, CR4, EFER & !(1 << 11)).unwrap();
        assert_eq!(hit(&cache, page, &no_nxe, PrivilegeLevel::Ring0), None);
        // An insert under another snapshot drops the entries of the old one.
        cache.insert(
            page + 0x1000,
            &paging(0x9000),
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            0x8000,
            TABLES,
            false,
        );
        assert_eq!(
            hit(&cache, page, &paging(0x1000), PrivilegeLevel::Ring0),
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
            0x7000,
            [Some(1), Some(2), Some(3), Some(4)],
            false,
        );
        cache.insert(
            b,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            0x8000,
            [Some(1), Some(2), Some(3), Some(5)],
            false,
        );
        // A data write (outside every table frame) and a write next to the frames change nothing.
        cache.note_write(write(0x7000, 8));
        cache.note_write(write(0x6ff8, 8));
        assert_eq!(cache.len(), 2);
        // The last byte of the PT of `b` drops `b` alone.
        cache.note_write(write(0x5fff, 1));
        assert!(hit(&cache, a, &config, PrivilegeLevel::Ring0).is_some());
        assert!(hit(&cache, b, &config, PrivilegeLevel::Ring0).is_none());
        // A write that spans from below into the shared PML4 frame drops everything.
        cache.insert(
            b,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            0x8000,
            [Some(1), Some(2), Some(3), Some(5)],
            false,
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
                0x7000,
                tables,
                false,
            );
        }
        assert_eq!(cache.len(), 4);
        // A 17th frame clears everything and keeps the new entry.
        cache.insert(
            a + 0x10_000,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x7000),
            0x7000,
            [Some(0x200), None, None, None],
            false,
        );
        assert_eq!(cache.len(), 1);
        assert!(hit(&cache, a + 0x10_000, &config, PrivilegeLevel::Ring0).is_some());
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
            0x7000,
            [Some(0x10), Some(0x11), None, None],
            false,
        );
        cache.insert(
            same_set,
            &config,
            PrivilegeLevel::Ring0,
            frame(&memory, 0x8000),
            0x8000,
            [Some(0x20), Some(0x21), None, None],
            false,
        );
        assert!(hit(&cache, a, &config, PrivilegeLevel::Ring0).is_none());
        // The frames of the evicted entry are not watched any more: writing them drops nothing.
        cache.note_write(write(0x10_000, 0x2000));
        assert!(hit(&cache, same_set, &config, PrivilegeLevel::Ring0).is_some());
        cache.flush();
        assert_eq!(cache.len(), 0);
        assert!(hit(&cache, same_set, &config, PrivilegeLevel::Ring0).is_none());
    }
}
