//! S4-13c: shadow caches that sweep the capacity and the policy of the fetch translation cache on
//! the real Linux path. Diagnostics only: nothing here is read back by the loop, the CPU or a
//! device, and a run with the sweep retires the same instructions and leaves the same guest RAM
//! and report as a run without it.
//!
//! The real cache ([`crate::fetch_translation_cache::FetchTranslationCache`]) forwards every event
//! it receives to an attached [`FetchCacheSweep`]: `lookup` (hit or miss is decided by each shadow
//! on its own), `insert` (the real cache inserts after a miss; only the shadows that missed that
//! lookup take the insert, a shadow that hit takes nothing), `note_write`, the explicit flushes,
//! and the call boundary (`begin_call`, `revalidate`, `seal`). The loop also tells the sweep of the
//! three guest invalidations (`INVLPG`, a delivered `#PF`, a change of CR0/CR4) which a
//! `WriteWatch` cache does not act on; only the `GuestOnly` shadows do.
//!
//! A shadow is parameterised by:
//! - capacity (entries kept at once),
//! - replacement: FIFO (the oldest insertion goes; a hit changes nothing) or LRU (a hit and an
//!   insert make the entry the most recent),
//! - tag granularity: `Page4K` (every entry tags one 4 KiB page, as the real cache does) or
//!   `Leaf2M` (an entry of a walk that ended at a 2 MiB leaf tags the whole 2 MiB page; a walk that
//!   ended at a 4 KiB leaf still tags a 4 KiB page),
//! - lifecycle: `PerCall` (the shadow is emptied at the start of every call of the set loop, as the
//!   board does with the real cache) or `Persistent` (it survives the call boundary and follows the
//!   S4-14i rules: a changed translation context or a write to a watched table frame outside the
//!   loop flushes it),
//! - invalidation: `WriteWatch` (a committed write to a table frame of an entry drops every entry:
//!   the S4-13 behaviour) or `GuestOnly` (such a write is only counted; the guest's own
//!   invalidations flush). `GuestOnly` is a hypothesis: a shadow keeps serving a translation that
//!   the real run would not trust, so its hit counts are what a TLB-like cache would see, not what
//!   a run that used it would produce.
//!
//! Each shadow keeps the ten counters of [`FetchCacheStats`], so the reference shadow
//! (4 / FIFO / 4 KiB / per call / write watch) can be compared with the real cache counter by
//! counter.
//!
//! To keep the cost of 112 shadows low, a lookup of the same 4 KiB page, in the same translation
//! context, right after a hit in every shadow (or an insert, which puts the entry at the most
//! recent position of every shadow) is a hit in every shadow and changes no order: it is counted
//! once for all (`bulk_hits`). Any flush, any write that drops an entry in some shadow, and every
//! call start ends that run.

use crate::fetch_translation_cache::{FetchCacheStats, NO_TABLE, TranslationKey};
use crate::page_walk::Long64ProbeConfig;
use od_isa::memory::AccessContext;
use std::cell::RefCell;
use std::collections::BTreeSet;

/// Which entry leaves a full shadow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SweepReplacement {
    Fifo,
    Lru,
}

/// What a lookup of the real cache is for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LookupKind {
    /// The byte fetch of an instruction: a miss walks and inserts.
    Fetch,
    /// A read-only peek of the loop (CPUID, `IRETQ`, `INVLPG`, `SYSRETQ`): a miss inserts nothing.
    Peek,
}

/// What one entry tags.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SweepTag {
    /// One 4 KiB page per entry.
    Page4K,
    /// A 2 MiB leaf is one entry for the whole 2 MiB page; a 4 KiB leaf stays one 4 KiB page.
    Leaf2M,
}

/// How long a shadow lives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SweepLifecycle {
    /// Emptied at the start of every call of the set loop.
    PerCall,
    /// Kept across calls; the rules of S4-14i apply at the call boundary.
    Persistent,
}

/// What drops entries on a write to a table frame of an entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SweepInvalidation {
    /// The write drops every entry (S4-13).
    WriteWatch,
    /// The write is counted and drops nothing; `INVLPG`, a delivered `#PF` and a change of CR0/CR4
    /// flush (S4-14h).
    GuestOnly,
}

/// The parameters of one shadow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SweepConfig {
    pub capacity: usize,
    pub replacement: SweepReplacement,
    pub tag: SweepTag,
    pub lifecycle: SweepLifecycle,
    pub invalidation: SweepInvalidation,
}

impl SweepConfig {
    /// The shadow that must count exactly like the real cache of the board: 4 slots, FIFO, 4 KiB
    /// tags, one cache per call, write watch.
    pub const REFERENCE: SweepConfig = SweepConfig {
        capacity: 4,
        replacement: SweepReplacement::Fifo,
        tag: SweepTag::Page4K,
        lifecycle: SweepLifecycle::PerCall,
        invalidation: SweepInvalidation::WriteWatch,
    };

    /// A stable name, used as the key of the JSON table.
    pub fn label(&self) -> String {
        format!(
            "cap{}-{}-{}-{}-{}",
            self.capacity,
            match self.replacement {
                SweepReplacement::Fifo => "fifo",
                SweepReplacement::Lru => "lru",
            },
            match self.tag {
                SweepTag::Page4K => "4k",
                SweepTag::Leaf2M => "2m",
            },
            match self.lifecycle {
                SweepLifecycle::PerCall => "per_call",
                SweepLifecycle::Persistent => "persistent",
            },
            match self.invalidation {
                SweepInvalidation::WriteWatch => "write_watch",
                SweepInvalidation::GuestOnly => "guest_only",
            },
        )
    }

    /// The capacities of the grid.
    pub const CAPACITIES: [usize; 7] = [1, 2, 4, 8, 16, 32, 64];

    /// The whole grid: capacity x replacement x tag x lifecycle x invalidation (112 shadows).
    pub fn full_grid() -> Vec<SweepConfig> {
        let mut grid = Vec::new();
        for invalidation in [SweepInvalidation::WriteWatch, SweepInvalidation::GuestOnly] {
            for lifecycle in [SweepLifecycle::PerCall, SweepLifecycle::Persistent] {
                for tag in [SweepTag::Page4K, SweepTag::Leaf2M] {
                    for replacement in [SweepReplacement::Fifo, SweepReplacement::Lru] {
                        for capacity in Self::CAPACITIES {
                            grid.push(SweepConfig {
                                capacity,
                                replacement,
                                tag,
                                lifecycle,
                                invalidation,
                            });
                        }
                    }
                }
            }
        }
        grid
    }
}

#[derive(Clone, Copy)]
struct SimEntry {
    /// Interned translation context (paging snapshot and access context).
    key: u32,
    /// 12 for a 4 KiB tag, 21 for a 2 MiB leaf tag.
    shift: u8,
    tag: u64,
    /// Physical page numbers of the tables on the path, [`NO_TABLE`] when unused.
    tables: [u64; 4],
}

/// One shadow cache.
struct Sim {
    config: SweepConfig,
    /// Oldest (FIFO) or least recently used (LRU) first.
    entries: Vec<SimEntry>,
    stats: FetchCacheStats,
    /// S4-14i seal of a persistent shadow: `None` before the first seal; `Some(None)` is a seal
    /// with an unusable configuration, which no later call is trusted to continue.
    sealed_key: Option<Option<u32>>,
    /// The fetch lookup that just missed here and in the real cache is followed by the real
    /// cache's insert; this shadow takes it (a shadow that hit takes nothing).
    pending_fill: bool,
}

impl Sim {
    fn new(config: SweepConfig) -> Self {
        Self {
            config,
            entries: Vec::with_capacity(config.capacity),
            stats: FetchCacheStats::default(),
            sealed_key: None,
            pending_fill: false,
        }
    }

    /// Whether `address` has an entry, and that entry is the one a repeated lookup would touch
    /// last (the most recent one of an LRU shadow), without counting or moving anything.
    fn holds_most_recent(&self, key: u32, address: u64) -> bool {
        let found = self
            .entries
            .iter()
            .rposition(|entry| entry.key == key && address >> entry.shift == entry.tag);
        match found {
            Some(position) => {
                self.config.replacement == SweepReplacement::Fifo
                    || position + 1 == self.entries.len()
            }
            None => false,
        }
    }

    /// Returns whether the lookup hit.
    fn lookup(&mut self, key: u32, address: u64) -> bool {
        let found = self
            .entries
            .iter()
            .rposition(|entry| entry.key == key && address >> entry.shift == entry.tag);
        match found {
            Some(position) => {
                self.stats.hits += 1;
                if self.config.replacement == SweepReplacement::Lru
                    && position + 1 != self.entries.len()
                {
                    let entry = self.entries.remove(position);
                    self.entries.push(entry);
                }
                true
            }
            None => {
                self.stats.misses += 1;
                false
            }
        }
    }

    fn insert(&mut self, key: u32, address: u64, tables: [u64; 4]) {
        // A walk that stopped at a 2 MiB leaf visited three tables (the fourth is `NO_TABLE`).
        let shift = if self.config.tag == SweepTag::Leaf2M && tables[3] == NO_TABLE {
            21
        } else {
            12
        };
        let entry = SimEntry {
            key,
            shift,
            tag: address >> shift,
            tables,
        };
        let held = self
            .entries
            .iter()
            .position(|held| held.key == key && held.shift == shift && held.tag == entry.tag);
        match held {
            // A key already present is replaced in place (FIFO keeps its place); an insert is a
            // use for LRU.
            Some(position) => {
                if self.config.replacement == SweepReplacement::Lru {
                    self.entries.remove(position);
                    self.entries.push(entry);
                } else {
                    self.entries[position] = entry;
                }
            }
            None => {
                self.entries.push(entry);
                if self.entries.len() > self.config.capacity {
                    self.entries.remove(0);
                }
            }
        }
        self.stats.inserts += 1;
    }

    /// Returns whether entries were dropped.
    fn note_write(&mut self, first: u64, last: u64) -> bool {
        let touched = self.entries.iter().any(|entry| {
            entry
                .tables
                .iter()
                .any(|table| *table != NO_TABLE && (first..=last).contains(table))
        });
        if !touched {
            return false;
        }
        match self.config.invalidation {
            SweepInvalidation::WriteWatch => {
                self.entries.clear();
                self.stats.write_flushes += 1;
                true
            }
            SweepInvalidation::GuestOnly => {
                self.stats.ignored_table_writes += 1;
                false
            }
        }
    }

    fn flush_with(&mut self, count: fn(&mut FetchCacheStats)) {
        self.entries.clear();
        count(&mut self.stats);
    }
}

/// Events the sweep saw, for the cost of the sweep and for the report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SweepEventCounts {
    pub calls: u64,
    pub lookups: u64,
    /// Lookups counted once for every shadow (see the module doc).
    pub bulk_lookups: u64,
    pub inserts: u64,
    pub writes: u64,
    /// Writes that reached some shadow's table check (the rest touch no frame ever watched).
    pub writes_checked: u64,
}

struct SweepInner {
    sims: Vec<Sim>,
    /// Interned translation contexts.
    keys: Vec<(Long64ProbeConfig, AccessContext)>,
    last_key: Option<u32>,
    /// `(key, 4 KiB page)` that is the most recent entry of every shadow.
    fast: Option<(u32, u64)>,
    /// `(key, 4 KiB page)` of a fetch lookup that missed in the real cache (whether or not any
    /// shadow missed too): the insert that follows is for it, and only the shadows that missed take
    /// it.
    pending_fetch: Option<(u32, u64)>,
    /// Off only in a test that checks the bulk count against every shadow looked up one by one.
    bulk_enabled: bool,
    bulk_hits: u64,
    /// Every table frame (physical page number) any shadow ever watched: a superset of the live
    /// ones, used to skip the per-shadow check of the many writes that touch no table.
    watched: BTreeSet<u64>,
    /// `(watched page, its generation at the last seal)`; `0` is an unknown generation.
    snapshot: Vec<(u64, u64)>,
    snapshot_pages: usize,
    events: SweepEventCounts,
    c5: Option<Vec<FetchCacheStats>>,
}

impl SweepInner {
    fn intern(&mut self, paging: &Long64ProbeConfig, context: AccessContext) -> u32 {
        if let Some(last) = self.last_key {
            let (held_paging, held_context) = &self.keys[last as usize];
            if held_paging == paging && *held_context == context {
                return last;
            }
        }
        let id = match self.keys.iter().rposition(|(held_paging, held_context)| {
            held_paging == paging && *held_context == context
        }) {
            Some(position) => position,
            None => {
                self.keys.push((*paging, context));
                self.keys.len() - 1
            }
        } as u32;
        self.last_key = Some(id);
        id
    }

    fn key_of(&mut self, key: Option<TranslationKey>) -> Option<u32> {
        key.map(|key| self.intern(&key.paging, key.access))
    }

    fn stats_of(&self, sim: &Sim) -> FetchCacheStats {
        let mut stats = sim.stats;
        stats.hits += self.bulk_hits;
        stats
    }

    fn take_snapshot(&mut self, generation_of: &dyn Fn(u64) -> Option<u64>) {
        self.snapshot.clear();
        for page in &self.watched {
            self.snapshot
                .push((*page, generation_of(*page).unwrap_or(0)));
        }
        self.snapshot_pages = self.watched.len();
    }
}

/// The shadow caches of one run. Shared by the per-call real caches of the board through an `Rc`;
/// every method takes `&self`.
pub(crate) struct FetchCacheSweep {
    inner: RefCell<SweepInner>,
}

impl FetchCacheSweep {
    pub(crate) fn new(configs: &[SweepConfig]) -> Self {
        Self {
            inner: RefCell::new(SweepInner {
                sims: configs.iter().copied().map(Sim::new).collect(),
                keys: Vec::new(),
                last_key: None,
                fast: None,
                pending_fetch: None,
                bulk_enabled: true,
                bulk_hits: 0,
                watched: BTreeSet::new(),
                snapshot: Vec::new(),
                snapshot_pages: 0,
                events: SweepEventCounts::default(),
                c5: None,
            }),
        }
    }

    #[cfg(test)]
    fn without_bulk(configs: &[SweepConfig]) -> Self {
        let sweep = Self::new(configs);
        sweep.inner.borrow_mut().bulk_enabled = false;
        sweep
    }

    /// A new call of the set loop starts (a new real cache is built): the `PerCall` shadows are
    /// emptied, the counters stay.
    pub(crate) fn begin_call(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.events.calls += 1;
        inner.fast = None;
        inner.pending_fetch = None;
        for sim in &mut inner.sims {
            if sim.config.lifecycle == SweepLifecycle::PerCall {
                sim.entries.clear();
            }
        }
    }

    /// A lookup of the real cache. `kind` says whether a fetch lookup (a miss is followed by a walk
    /// and an insert) or a peek of the loop (nothing is inserted); `hit_tables` are the tables of
    /// the entry the real cache hit, `None` when it missed.
    ///
    /// Each shadow decides hit or miss on its own state. On a fetch miss it would walk and insert:
    /// when the real cache hit, the tables of its entry give what the walk would have found and the
    /// shadow inserts at once; when the real cache missed too, the insert event that follows tells
    /// the shadows that missed (and only those) what to insert.
    pub(crate) fn lookup(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
        kind: LookupKind,
        hit_tables: Option<[u64; 4]>,
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.events.lookups += 1;
        let key = inner.intern(paging, context);
        let page = address >> 12;
        // A fetch lookup that missed in the real cache is followed by the real cache's insert,
        // whether or not any shadow missed too: the insert is then for this lookup, and only the
        // shadows that missed take it (none, when all of them hit).
        let real_fetch_miss = kind == LookupKind::Fetch && hit_tables.is_none();
        if inner.fast == Some((key, page)) {
            inner.bulk_hits += 1;
            inner.events.bulk_lookups += 1;
            inner.pending_fetch = real_fetch_miss.then_some((key, page));
            return;
        }
        let tables = hit_tables.filter(|_| kind == LookupKind::Fetch);
        if let Some(tables) = tables {
            for table in tables {
                if table != NO_TABLE {
                    inner.watched.insert(table);
                }
            }
        }
        let mut all_present = true;
        for sim in &mut inner.sims {
            sim.pending_fill = false;
            if sim.lookup(key, address) {
                continue;
            }
            match (kind, tables) {
                (LookupKind::Fetch, Some(tables)) => sim.insert(key, address, tables),
                (LookupKind::Fetch, None) => {
                    sim.pending_fill = true;
                    all_present = false;
                }
                (LookupKind::Peek, _) => all_present = false,
            }
        }
        inner.pending_fetch = real_fetch_miss.then_some((key, page));
        inner.fast = (all_present && inner.bulk_enabled).then_some((key, page));
    }

    /// The insert of the real cache after a completed fetch walk. The shadows that missed the
    /// preceding fetch lookup insert; if there was none (an insert on its own), every shadow does.
    pub(crate) fn insert(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
        tables: [Option<u64>; 4],
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.events.inserts += 1;
        let key = inner.intern(paging, context);
        let tables = tables.map(|table| table.map_or(NO_TABLE, |base| base >> 12));
        for table in tables {
            if table != NO_TABLE {
                inner.watched.insert(table);
            }
        }
        let solicited = inner.pending_fetch == Some((key, address >> 12));
        inner.pending_fetch = None;
        let mut all_present = true;
        for sim in &mut inner.sims {
            if !solicited || sim.pending_fill {
                sim.insert(key, address, tables);
            }
            sim.pending_fill = false;
            all_present &= sim.holds_most_recent(key, address);
        }
        inner.fast = (all_present && inner.bulk_enabled).then_some((key, address >> 12));
    }

    /// A committed RAM write covering the physical pages `first..=last`.
    pub(crate) fn note_write(&self, first: u64, last: u64) {
        let mut inner = self.inner.borrow_mut();
        inner.events.writes += 1;
        if inner.watched.range(first..=last).next().is_none() {
            return;
        }
        inner.events.writes_checked += 1;
        let mut dropped = false;
        for sim in &mut inner.sims {
            dropped |= sim.note_write(first, last);
        }
        if dropped {
            inner.fast = None;
        }
    }

    /// An explicit flush (translation context change), every shadow.
    pub(crate) fn flush(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.fast = None;
        for sim in &mut inner.sims {
            sim.flush_with(|stats| stats.context_flushes += 1);
        }
    }

    /// A retired `INVLPG`: only the `GuestOnly` shadows act on it.
    pub(crate) fn invlpg(&self) {
        self.guest_flush(|stats| stats.invlpg_flushes += 1);
    }

    /// A `#PF` delivered to the guest.
    pub(crate) fn page_fault(&self) {
        self.guest_flush(|stats| stats.fault_flushes += 1);
    }

    /// CR0 or CR4 changed.
    pub(crate) fn control_change(&self) {
        self.guest_flush(|stats| stats.control_flushes += 1);
    }

    fn guest_flush(&self, count: fn(&mut FetchCacheStats)) {
        let mut inner = self.inner.borrow_mut();
        inner.fast = None;
        for sim in &mut inner.sims {
            if sim.config.invalidation == SweepInvalidation::GuestOnly {
                sim.flush_with(count);
            }
        }
    }

    /// An explicit host event the generations cannot show (`flush_for_host_event`).
    pub(crate) fn host_event(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.fast = None;
        for sim in &mut inner.sims {
            sim.entries.clear();
            sim.sealed_key = None;
            sim.stats.host_flushes += 1;
        }
    }

    /// Machine reset or a new session over the real cache: shadows, counters and seals are dropped.
    pub(crate) fn reset(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.fast = None;
        inner.pending_fetch = None;
        inner.bulk_hits = 0;
        inner.snapshot.clear();
        inner.snapshot_pages = 0;
        inner.c5 = None;
        for sim in &mut inner.sims {
            sim.entries.clear();
            sim.stats = FetchCacheStats::default();
            sim.sealed_key = None;
        }
    }

    /// Seal at the end of a call (see `FetchTranslationCache::seal`): the `Persistent` shadows
    /// remember the context the call left and the generation of every frame they could watch.
    pub(crate) fn seal(
        &self,
        key: Option<TranslationKey>,
        generation_of: &dyn Fn(u64) -> Option<u64>,
    ) {
        let mut inner = self.inner.borrow_mut();
        let key = inner.key_of(key);
        inner.take_snapshot(generation_of);
        for sim in &mut inner.sims {
            if sim.config.lifecycle == SweepLifecycle::Persistent {
                sim.sealed_key = Some(key);
            }
        }
    }

    /// The check at the start of a call: a `Persistent` shadow is dropped (`host_flushes`) when the
    /// context differs from the sealed one or when a table frame of one of its entries has another
    /// generation than at the seal. The real cache seals again right after (`seal`), which is the
    /// seal of the shadows too.
    pub(crate) fn revalidate(
        &self,
        key: Option<TranslationKey>,
        generation_of: &dyn Fn(u64) -> Option<u64>,
    ) {
        let mut inner = self.inner.borrow_mut();
        let key_id = inner.key_of(key);
        {
            let changed: Vec<u64> = inner
                .snapshot
                .iter()
                .filter(|(page, sealed)| generation_of(*page) != Some(*sealed))
                .map(|(page, _)| *page)
                .collect();
            let mut dropped = false;
            for sim in &mut inner.sims {
                if sim.config.lifecycle != SweepLifecycle::Persistent {
                    continue;
                }
                let Some(sealed) = sim.sealed_key else {
                    continue;
                };
                let same_context =
                    matches!((sealed, key_id), (Some(held), Some(now)) if held == now);
                let table_changed = !changed.is_empty()
                    && sim.entries.iter().any(|entry| {
                        entry
                            .tables
                            .iter()
                            .any(|table| *table != NO_TABLE && changed.contains(table))
                    });
                if !same_context || table_changed {
                    sim.entries.clear();
                    sim.stats.host_flushes += 1;
                    dropped = true;
                }
            }
            if dropped {
                inner.fast = None;
            }
        }
    }

    /// The boundary of checkpoint C5 (the first fetch at `e_entry`): the counters now are the end
    /// of phase 1. Only the first call counts.
    pub(crate) fn mark_c5(&self) {
        let mut inner = self.inner.borrow_mut();
        if inner.c5.is_some() {
            return;
        }
        let snapshot = inner.sims.iter().map(|sim| inner.stats_of(sim)).collect();
        inner.c5 = Some(snapshot);
    }

    /// The counters of the shadow at `index` of the configs the sweep was built with.
    #[cfg(test)]
    pub(crate) fn stats(&self, index: usize) -> FetchCacheStats {
        let inner = self.inner.borrow();
        inner.stats_of(&inner.sims[index])
    }

    pub(crate) fn report(&self) -> FetchSweepReport {
        let inner = self.inner.borrow();
        let configs = inner
            .sims
            .iter()
            .enumerate()
            .map(|(index, sim)| {
                let total = inner.stats_of(sim);
                let (before_c5, after_c5) = match &inner.c5 {
                    Some(at_c5) => (at_c5[index], total - at_c5[index]),
                    None => (total, FetchCacheStats::default()),
                };
                FetchSweepConfigReport {
                    config: sim.config,
                    label: sim.config.label(),
                    before_c5,
                    after_c5,
                    total,
                }
            })
            .collect();
        FetchSweepReport {
            c5_reached: inner.c5.is_some(),
            events: inner.events,
            watched_table_pages: inner.watched.len() as u64,
            distinct_contexts: inner.keys.len() as u64,
            configs,
        }
    }
}

/// The counters of one shadow, before and after the boundary of C5 and in total.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchSweepConfigReport {
    pub config: SweepConfig,
    pub label: String,
    pub before_c5: FetchCacheStats,
    pub after_c5: FetchCacheStats,
    pub total: FetchCacheStats,
}

/// The result of a sweep.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FetchSweepReport {
    pub c5_reached: bool,
    pub events: SweepEventCounts,
    /// Distinct table frames any shadow ever watched.
    pub watched_table_pages: u64,
    /// Distinct translation contexts (paging snapshot and access context) seen.
    pub distinct_contexts: u64,
    pub configs: Vec<FetchSweepConfigReport>,
}

impl FetchSweepReport {
    /// The shadow with these parameters.
    pub fn find(&self, config: SweepConfig) -> Option<&FetchSweepConfigReport> {
        self.configs.iter().find(|entry| entry.config == config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch_translation_cache::{FetchCacheInvalidation, FetchTranslationCache};
    use crate::physical_memory::{
        GuestPhysicalAddress, GuestPhysicalRange, MachineMemoryMap, MemoryRegionKind,
        MemoryRegionSpec, PhysicalMemory, RamFrame, RamLimits,
    };
    use od_isa::memory::{AccessOrigin, InstructionMode, PrivilegeLevel};
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::rc::Rc;

    const CR0: u64 = 0x8001_0033;
    const CR4: u64 = 1 << 5;
    const EFER: u64 = (1 << 8) | (1 << 10) | (1 << 11);
    const CODE_VA: u64 = 0x0100_0000;

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

    fn key(cr3: u64) -> Option<TranslationKey> {
        Some(TranslationKey::new(
            paging(cr3),
            context(PrivilegeLevel::Ring0),
        ))
    }

    /// 64 KiB of RAM at 0.
    fn memory() -> PhysicalMemory {
        let specs = [MemoryRegionSpec::new(
            range(0, 0x1_0000),
            MemoryRegionKind::Ram,
        )];
        PhysicalMemory::new(
            &MachineMemoryMap::new(36, specs.to_vec()).unwrap(),
            RamLimits::new(1 << 20),
        )
        .unwrap()
    }

    fn frame(memory: &PhysicalMemory, base: u64) -> RamFrame {
        memory.ram_frame(GuestPhysicalAddress::new(base)).unwrap()
    }

    /// A walk through four tables (a 4 KiB leaf) or through three (a 2 MiB leaf).
    fn tables_of(leaf_2m: bool) -> [Option<u64>; 4] {
        [
            Some(0x1000),
            Some(0x2000),
            Some(0x3000),
            (!leaf_2m).then_some(0x4000),
        ]
    }

    /// Deterministic generator (a 64-bit LCG), no dependency.
    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: u64) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (self.0 >> 33) % bound
        }
    }

    /// The generation of every table frame, bumped by writes (a stand-in for the bus).
    #[derive(Default)]
    struct Generations(RefCell<HashMap<u64, u64>>);

    impl Generations {
        fn of(&self, page: u64) -> Option<u64> {
            Some(*self.0.borrow().get(&page).unwrap_or(&1))
        }

        fn bump(&self, page: u64) {
            *self.0.borrow_mut().entry(page).or_insert(1) += 1;
        }
    }

    /// One call of the loop on a real cache: a stream of lookups (with an insert after a miss, as
    /// the fetch does), writes, flushes and guest invalidations. The sweep sees them through the
    /// cache only, as on the board.
    fn drive_call(
        cache: &FetchTranslationCache,
        generations: &Generations,
        rng: &mut Lcg,
        steps: u64,
        cr3: u64,
        guest_only: bool,
    ) {
        let memory = memory();
        let ring = |n: u64| {
            context(if n == 0 {
                PrivilegeLevel::Ring0
            } else {
                PrivilegeLevel::Ring3
            })
        };
        let mut page = rng.next(12);
        for _ in 0..steps {
            match rng.next(100) {
                0..=64 => {
                    // Mostly the page of the last lookup (instruction bytes), sometimes a jump.
                    if rng.next(5) == 0 {
                        page = rng.next(12);
                    }
                    let address = CODE_VA + page * 0x1000 + rng.next(0x1000);
                    let privilege = ring(u64::from(page.is_multiple_of(5)));
                    // One lookup in five is a peek of the loop: it inserts nothing.
                    if rng.next(5) == 0 {
                        cache.lookup(address, &paging(cr3), privilege);
                        continue;
                    }
                    // A fetch that misses walks; one walk in ten fails and inserts nothing.
                    if cache
                        .lookup_fetch(address, &paging(cr3), privilege)
                        .is_none()
                        && rng.next(10) != 0
                    {
                        cache.insert(
                            address,
                            &paging(cr3),
                            privilege,
                            frame(&memory, 0x8000 + (page % 4) * 0x1000),
                            tables_of(page.is_multiple_of(3)),
                        );
                    }
                }
                65..=82 => {
                    // A committed write: into a table frame or somewhere else.
                    let target = [0x1000, 0x2000, 0x3000, 0x4000, 0x5000, 0x8000, 0x9000]
                        [rng.next(7) as usize];
                    cache.note_write(range(target + rng.next(0xff0), 8));
                    generations.bump(target >> 12);
                }
                83..=87 => cache.flush(),
                88..=91 => {
                    if guest_only {
                        cache.flush_after_invlpg();
                    } else {
                        cache.sweep_guest_invlpg();
                    }
                }
                92..=95 => {
                    if guest_only {
                        cache.flush_after_page_fault();
                    } else {
                        cache.sweep_guest_page_fault();
                    }
                }
                _ => {
                    if guest_only {
                        cache.flush_after_control_change();
                    } else {
                        cache.sweep_guest_control_change();
                    }
                }
            }
        }
    }

    fn config(
        capacity: usize,
        replacement: SweepReplacement,
        tag: SweepTag,
        lifecycle: SweepLifecycle,
        invalidation: SweepInvalidation,
    ) -> SweepConfig {
        SweepConfig {
            capacity,
            replacement,
            tag,
            lifecycle,
            invalidation,
        }
    }

    fn index_of(configs: &[SweepConfig], wanted: SweepConfig) -> usize {
        configs.iter().position(|config| *config == wanted).unwrap()
    }

    #[test]
    fn the_grid_has_112_distinct_shadows_and_the_reference_is_in_it() {
        let grid = SweepConfig::full_grid();
        assert_eq!(grid.len(), 7 * 2 * 2 * 2 * 2);
        let mut labels: Vec<String> = grid.iter().map(SweepConfig::label).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels.len(), 112);
        assert!(grid.contains(&SweepConfig::REFERENCE));
        assert_eq!(
            SweepConfig::REFERENCE.label(),
            "cap4-fifo-4k-per_call-write_watch"
        );
    }

    /// The reference shadow counts exactly like the real cache of the board (one cache per call,
    /// the `WriteWatch` policy): all ten counters, summed over many calls, on a stream with
    /// lookups, inserts after a miss, table writes, explicit flushes and both 4 KiB and 2 MiB
    /// walks.
    #[test]
    fn the_reference_shadow_equals_the_real_per_call_cache_counter_by_counter() {
        for seed in [1, 2, 3, 0xdead_beef] {
            let configs = SweepConfig::full_grid();
            let reference = index_of(&configs, SweepConfig::REFERENCE);
            let sweep = Rc::new(FetchCacheSweep::new(&configs));
            let generations = Generations::default();
            let mut rng = Lcg(seed);
            let mut real = FetchCacheStats::default();
            for call in 0..300 {
                let cache = FetchTranslationCache::new().with_sweep(Rc::clone(&sweep));
                // The board hands over the context at the start and at the end of every call.
                cache.revalidate(key(0x1000), &|page| generations.of(page));
                drive_call(
                    &cache,
                    &generations,
                    &mut rng,
                    40,
                    0x1000 + (call % 3) * 0x1000,
                    false,
                );
                cache.seal(key(0x1000), &|page| generations.of(page));
                real += cache.stats();
            }
            let shadow = sweep.stats(reference);
            assert_eq!(shadow, real, "seed {seed}");
            // The stream was not trivial.
            assert!(
                real.hits > 100 && real.misses > 100 && real.inserts > 100,
                "{real:?}"
            );
            assert!(
                real.write_flushes > 10 && real.context_flushes > 10,
                "{real:?}"
            );
            assert_eq!(real.host_flushes, 0);
        }
    }

    /// A `GuestOnly` shadow counts like a real `GuestOnly` cache fed the same stream, including
    /// `INVLPG`, `#PF` and CR0/CR4 flushes.
    #[test]
    fn the_guest_only_shadow_equals_a_real_guest_only_cache() {
        let wanted = config(
            4,
            SweepReplacement::Fifo,
            SweepTag::Page4K,
            SweepLifecycle::PerCall,
            SweepInvalidation::GuestOnly,
        );
        for seed in [7, 8, 9] {
            let configs = SweepConfig::full_grid();
            let shadow_index = index_of(&configs, wanted);
            let sweep = Rc::new(FetchCacheSweep::new(&configs));
            let generations = Generations::default();
            let mut rng = Lcg(seed);
            let mut real = FetchCacheStats::default();
            for _ in 0..300 {
                let cache =
                    FetchTranslationCache::with_invalidation(FetchCacheInvalidation::GuestOnly)
                        .with_sweep(Rc::clone(&sweep));
                cache.revalidate(key(0x1000), &|page| generations.of(page));
                drive_call(&cache, &generations, &mut rng, 40, 0x1000, true);
                cache.seal(key(0x1000), &|page| generations.of(page));
                real += cache.stats();
            }
            assert_eq!(sweep.stats(shadow_index), real, "seed {seed}");
            assert!(real.ignored_table_writes > 10, "{real:?}");
            assert!(
                real.invlpg_flushes > 5 && real.fault_flushes > 5 && real.control_flushes > 5,
                "{real:?}"
            );
            assert_eq!(real.write_flushes, 0);
        }
    }

    /// A persistent real cache (S4-14i) and the persistent 4 / FIFO / 4 KiB shadow of the same
    /// policy count the same, through call boundaries where the host changes the translation
    /// context or writes to a table frame.
    #[test]
    fn persistent_shadows_equal_a_real_persistent_cache_across_host_events() {
        for invalidation in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let wanted = config(
                4,
                SweepReplacement::Fifo,
                SweepTag::Page4K,
                SweepLifecycle::Persistent,
                if invalidation == FetchCacheInvalidation::GuestOnly {
                    SweepInvalidation::GuestOnly
                } else {
                    SweepInvalidation::WriteWatch
                },
            );
            let configs = SweepConfig::full_grid();
            let shadow_index = index_of(&configs, wanted);
            let sweep = Rc::new(FetchCacheSweep::new(&configs));
            let generations = Generations::default();
            let mut rng = Lcg(99);
            let cache =
                FetchTranslationCache::persistent(invalidation).with_sweep(Rc::clone(&sweep));
            let guest_only = invalidation == FetchCacheInvalidation::GuestOnly;
            let mut cr3 = 0x1000;
            for call in 0..400 {
                match rng.next(10) {
                    // The host hands over another context.
                    0 => cr3 = 0x1000 + rng.next(3) * 0x1000,
                    // A device or the host writes to a table frame between the calls.
                    1 | 2 => generations.bump(0x1 + rng.next(4)),
                    _ => {}
                }
                cache.revalidate(key(cr3), &|page| generations.of(page));
                drive_call(&cache, &generations, &mut rng, 30, 0x1000, guest_only);
                cache.seal(key(cr3), &|page| generations.of(page));
                let _ = call;
            }
            let real = cache.stats();
            assert_eq!(sweep.stats(shadow_index), real, "{invalidation:?}");
            assert!(real.host_flushes > 10, "{real:?}");
            assert!(real.hits > 100, "{real:?}");
        }
    }

    /// An explicit host event and a reset reach the shadows like the real cache.
    #[test]
    fn host_event_and_reset_reach_the_shadows() {
        let configs = [SweepConfig::REFERENCE];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::WriteWatch)
            .with_sweep(Rc::clone(&sweep));
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            tables_of(false),
        );
        cache.flush_for_host_event();
        assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_none());
        assert_eq!(sweep.stats(0), cache.stats());
        assert_eq!(sweep.stats(0).host_flushes, 1);
        cache.reset();
        assert_eq!(sweep.stats(0), FetchCacheStats::default());
        assert_eq!(cache.stats(), FetchCacheStats::default());
    }

    /// Counting a repeated lookup once for all shadows gives every shadow the counters it has when
    /// each one is looked up one by one (all 112, three call shapes of stream).
    #[test]
    fn the_bulk_count_equals_looking_every_shadow_up_one_by_one() {
        let configs = SweepConfig::full_grid();
        for seed in [11, 12, 13] {
            let bulk = Rc::new(FetchCacheSweep::new(&configs));
            let plain = Rc::new(FetchCacheSweep::without_bulk(&configs));
            for sweep in [&bulk, &plain] {
                let generations = Generations::default();
                let mut rng = Lcg(seed);
                for call in 0..200 {
                    let cache = FetchTranslationCache::new().with_sweep(Rc::clone(sweep));
                    cache.revalidate(key(0x1000), &|page| generations.of(page));
                    drive_call(
                        &cache,
                        &generations,
                        &mut rng,
                        60,
                        0x1000 + (call % 2) * 0x1000,
                        false,
                    );
                    cache.seal(key(0x1000), &|page| generations.of(page));
                }
            }
            let (bulk_report, plain_report) = (bulk.report(), plain.report());
            assert!(
                bulk_report.events.bulk_lookups > 100,
                "{:?}",
                bulk_report.events
            );
            assert_eq!(plain_report.events.bulk_lookups, 0);
            assert_eq!(bulk_report.configs, plain_report.configs, "seed {seed}");
        }
    }

    /// A shadow of the sweep changes nothing in the cache it watches.
    #[test]
    fn the_real_cache_answers_and_counts_the_same_with_and_without_a_sweep() {
        let configs = SweepConfig::full_grid();
        let run = |with_sweep: bool| {
            let generations = Generations::default();
            let mut rng = Lcg(5);
            let sweep = Rc::new(FetchCacheSweep::new(&configs));
            let mut stats = Vec::new();
            for _ in 0..100 {
                let cache = if with_sweep {
                    FetchTranslationCache::new().with_sweep(Rc::clone(&sweep))
                } else {
                    FetchTranslationCache::new()
                };
                drive_call(&cache, &generations, &mut rng, 50, 0x1000, false);
                stats.push(cache.stats());
            }
            stats
        };
        assert_eq!(run(true), run(false));
    }

    /// A cycle of six pages through the same stream for every shadow: the stream is what a 4-slot
    /// real cache produces (it misses every lookup of a six-page cycle, so it inserts every time).
    fn cycle_stream(
        sweep: &Rc<FetchCacheSweep>,
        pages: u64,
        rounds: u64,
        tables: [Option<u64>; 4],
        stride: u64,
    ) {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::new().with_sweep(Rc::clone(sweep));
        for _ in 0..rounds {
            for page in 0..pages {
                let address = CODE_VA + page * stride;
                if cache
                    .lookup_fetch(address, &paging(0x1000), ring0)
                    .is_none()
                {
                    cache.insert(
                        address,
                        &paging(0x1000),
                        ring0,
                        frame(&memory, 0x8000),
                        tables,
                    );
                }
            }
        }
        // The driver cache never hit (4 slots, a cycle longer than 4).
        assert_eq!(cache.stats().hits, 0, "{:?}", cache.stats());
    }

    /// On a cyclic stream longer than 4 pages the hits of LRU never fall as the capacity grows, and
    /// a capacity that holds the cycle misses only the cold lookups. This is a property of this
    /// stream (LRU on a cycle), not a general one: the shadows share a stream that was produced by
    /// another cache.
    #[test]
    fn on_a_cyclic_stream_hits_do_not_fall_with_capacity() {
        let configs: Vec<SweepConfig> = SweepConfig::CAPACITIES
            .iter()
            .flat_map(|capacity| {
                [SweepReplacement::Fifo, SweepReplacement::Lru].map(|replacement| {
                    config(
                        *capacity,
                        replacement,
                        SweepTag::Page4K,
                        SweepLifecycle::PerCall,
                        SweepInvalidation::WriteWatch,
                    )
                })
            })
            .collect();
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        cycle_stream(&sweep, 6, 50, tables_of(false), 0x1000);
        for replacement in [SweepReplacement::Fifo, SweepReplacement::Lru] {
            let mut last = 0;
            for capacity in SweepConfig::CAPACITIES {
                let index = index_of(
                    &configs,
                    config(
                        capacity,
                        replacement,
                        SweepTag::Page4K,
                        SweepLifecycle::PerCall,
                        SweepInvalidation::WriteWatch,
                    ),
                );
                let stats = sweep.stats(index);
                assert!(stats.hits >= last, "{replacement:?} {capacity}: {stats:?}");
                assert_eq!(stats.hits + stats.misses, 300);
                last = stats.hits;
                if capacity >= 6 {
                    assert_eq!(stats.misses, 6, "{replacement:?} {capacity}: {stats:?}");
                } else {
                    // A cycle longer than the cache defeats FIFO and LRU alike.
                    assert_eq!(stats.hits, 0, "{replacement:?} {capacity}: {stats:?}");
                }
            }
        }
    }

    /// The real cache holds a cycle of three pages and hits every lookup after the cold ones; a
    /// one-entry shadow misses every lookup and walks again, so it inserts every time (from the
    /// tables of the real entry), a peek of the loop inserts nothing in it, and the reference
    /// shadow follows the real cache.
    #[test]
    fn a_smaller_shadow_fills_itself_where_the_real_cache_hits() {
        let one = SweepConfig {
            capacity: 1,
            ..SweepConfig::REFERENCE
        };
        let configs = [SweepConfig::REFERENCE, one];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::new().with_sweep(Rc::clone(&sweep));
        for _ in 0..20 {
            for page in 0..3u64 {
                let address = CODE_VA + page * 0x1000;
                if cache
                    .lookup_fetch(address, &paging(0x1000), ring0)
                    .is_none()
                {
                    cache.insert(
                        address,
                        &paging(0x1000),
                        ring0,
                        frame(&memory, 0x8000),
                        tables_of(false),
                    );
                }
            }
        }
        assert_eq!(sweep.stats(0), cache.stats());
        assert_eq!((cache.stats().misses, cache.stats().inserts), (3, 3));
        let small = sweep.stats(1);
        assert_eq!((small.hits, small.misses, small.inserts), (0, 60, 60));
        // A peek that misses inserts nothing, and the next fetch lookup still finds the entry the
        // fetch before it filled.
        let before = sweep.stats(1);
        cache.lookup(CODE_VA + 0x7000, &paging(0x1000), ring0);
        assert_eq!(sweep.stats(1).inserts, before.inserts);
        assert_eq!(sweep.stats(1).misses, before.misses + 1);
        // The entry the last fetch (page 2) filled is still there for the next fetch lookup.
        cache.lookup_fetch(CODE_VA + 0x2000, &paging(0x1000), ring0);
        assert_eq!(sweep.stats(1).hits, 1);
    }

    /// The real cache misses a fetch lookup that every shadow hits: the insert that follows is for
    /// the real cache only. A shadow that hit takes nothing (it was inserted on its own before, and
    /// inserted on every lookup of the cycle after that).
    #[test]
    fn an_insert_after_a_fetch_that_every_shadow_hit_goes_to_no_shadow() {
        let shadow = SweepConfig {
            capacity: 8,
            ..SweepConfig::REFERENCE
        };
        let sweep = Rc::new(FetchCacheSweep::new(&[shadow]));
        // Six pages in a loop: the 4-slot real cache misses every lookup (300 inserts), the 8-slot
        // shadow misses the six cold ones and hits the rest.
        cycle_stream(&sweep, 6, 50, tables_of(false), 0x1000);
        let stats = sweep.stats(0);
        assert_eq!(
            (stats.misses, stats.hits, stats.inserts),
            (6, 294, 6),
            "{stats:?}"
        );
        assert_eq!(sweep.report().events.inserts, 300);

        // The same on the bulk path: a lookup of the page of the last insert is counted once for
        // all shadows, and the real cache's insert after it is still not the shadow's.
        let sweep = Rc::new(FetchCacheSweep::new(&[shadow]));
        let ring0 = context(PrivilegeLevel::Ring0);
        sweep.insert(CODE_VA, &paging(0x1000), ring0, tables_of(false));
        sweep.lookup(CODE_VA, &paging(0x1000), ring0, LookupKind::Fetch, None);
        assert_eq!(sweep.report().events.bulk_lookups, 1);
        sweep.insert(CODE_VA, &paging(0x1000), ring0, tables_of(false));
        assert_eq!(sweep.stats(0).inserts, 1);
    }

    /// A real cache that serves several calls starts a call for the shadows at every revalidate:
    /// a `PerCall` shadow is emptied there, a `Persistent` one is not.
    #[test]
    fn a_cache_that_serves_several_calls_starts_a_call_for_the_shadows_each_time() {
        let make = |lifecycle| {
            config(
                4,
                SweepReplacement::Fifo,
                SweepTag::Page4K,
                lifecycle,
                SweepInvalidation::WriteWatch,
            )
        };
        let configs = [
            make(SweepLifecycle::PerCall),
            make(SweepLifecycle::Persistent),
        ];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        let generations = Generations::default();
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::WriteWatch)
            .with_sweep(Rc::clone(&sweep));
        let generation_of = |page: u64| generations.of(page);
        for call in 0..3u64 {
            cache.revalidate(key(0x1000), &generation_of);
            if cache
                .lookup_fetch(CODE_VA, &paging(0x1000), ring0)
                .is_none()
            {
                cache.insert(
                    CODE_VA,
                    &paging(0x1000),
                    ring0,
                    frame(&memory, 0x8000),
                    tables_of(false),
                );
            }
            cache.seal(key(0x1000), &generation_of);
            // Every call is counted once, the first one by `with_sweep`, not twice by `revalidate`.
            assert_eq!(sweep.report().events.calls, call + 1);
        }
        // The real cache is persistent and hits in calls 2 and 3; the persistent shadow follows it,
        // the per-call shadow starts empty in every call and misses every time.
        assert_eq!((cache.stats().hits, cache.stats().misses), (2, 1));
        assert_eq!((sweep.stats(1).hits, sweep.stats(1).misses), (2, 1));
        assert_eq!((sweep.stats(0).hits, sweep.stats(0).misses), (0, 3));
        assert_eq!(sweep.stats(0).inserts, 3);
    }

    /// 100 different 4 KiB pages of one 2 MiB leaf: one entry for the leaf with the 2 MiB tag, a
    /// miss every time with the 4 KiB tag.
    #[test]
    fn a_two_mib_tag_covers_the_whole_leaf() {
        let tag = |tag| {
            config(
                1,
                SweepReplacement::Lru,
                tag,
                SweepLifecycle::PerCall,
                SweepInvalidation::WriteWatch,
            )
        };
        let configs = [tag(SweepTag::Page4K), tag(SweepTag::Leaf2M)];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        cycle_stream(&sweep, 100, 3, tables_of(true), 0x1000);
        assert_eq!(sweep.stats(0).hits, 0);
        assert_eq!((sweep.stats(1).misses, sweep.stats(1).hits), (1, 299));
        // The same pages walked down to 4 KiB leaves are one entry each with either tag.
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        cycle_stream(&sweep, 100, 3, tables_of(false), 0x1000);
        assert_eq!(sweep.stats(0), sweep.stats(1));
    }

    /// A persistent shadow keeps the entry across a call boundary, a per-call one does not.
    #[test]
    fn a_persistent_shadow_survives_the_call_boundary_and_a_changed_context_flushes_it() {
        let make = |lifecycle| {
            config(
                4,
                SweepReplacement::Fifo,
                SweepTag::Page4K,
                lifecycle,
                SweepInvalidation::WriteWatch,
            )
        };
        let configs = [
            make(SweepLifecycle::PerCall),
            make(SweepLifecycle::Persistent),
        ];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        let generations = Generations::default();
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let call = |cr3: u64, insert: bool| {
            let cache = FetchTranslationCache::new().with_sweep(Rc::clone(&sweep));
            cache.revalidate(key(cr3), &|page| generations.of(page));
            if insert {
                cache.insert(
                    CODE_VA,
                    &paging(0x1000),
                    ring0,
                    frame(&memory, 0x8000),
                    tables_of(false),
                );
            } else {
                cache.lookup(CODE_VA, &paging(0x1000), ring0);
            }
            cache.seal(key(cr3), &|page| generations.of(page));
        };
        call(0x1000, true);
        call(0x1000, false);
        assert_eq!((sweep.stats(0).hits, sweep.stats(0).misses), (0, 1));
        assert_eq!((sweep.stats(1).hits, sweep.stats(1).misses), (1, 0));
        // The host hands over another CR3: the persistent shadow is dropped, once.
        call(0x2000, false);
        assert_eq!((sweep.stats(1).hits, sweep.stats(1).misses), (1, 1));
        assert_eq!(sweep.stats(1).host_flushes, 1);
        assert_eq!(sweep.stats(0).host_flushes, 0);
        // A write to a table frame of an entry outside the loop drops it too.
        call(0x2000, true);
        generations.bump(0x3000 >> 12);
        call(0x2000, false);
        assert_eq!((sweep.stats(1).hits, sweep.stats(1).misses), (1, 2));
        assert_eq!(sweep.stats(1).host_flushes, 2);
    }

    #[test]
    fn the_report_splits_at_c5_and_the_phases_add_up() {
        let configs = [SweepConfig::REFERENCE];
        let sweep = Rc::new(FetchCacheSweep::new(&configs));
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::new().with_sweep(Rc::clone(&sweep));
        cache.lookup(CODE_VA, &paging(0x1000), ring0);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            tables_of(false),
        );
        // No boundary yet: everything is phase 1.
        let report = sweep.report();
        assert!(!report.c5_reached);
        assert_eq!(report.configs[0].before_c5, report.configs[0].total);
        assert_eq!(report.configs[0].after_c5, FetchCacheStats::default());
        cache.sweep_mark_c5();
        // A second mark is ignored (only the first boundary counts).
        cache.lookup(CODE_VA, &paging(0x1000), ring0);
        cache.sweep_mark_c5();
        cache.lookup(CODE_VA, &paging(0x1000), ring0);
        let report = sweep.report();
        assert!(report.c5_reached);
        let entry = &report.configs[0];
        assert_eq!((entry.before_c5.hits, entry.before_c5.misses), (0, 1));
        assert_eq!((entry.after_c5.hits, entry.after_c5.misses), (2, 0));
        assert_eq!(entry.total, cache.stats());
        assert_eq!(report.events.lookups, 3);
        assert_eq!(report.events.bulk_lookups, 2);
        assert!(report.find(SweepConfig::REFERENCE).is_some());
    }
}
