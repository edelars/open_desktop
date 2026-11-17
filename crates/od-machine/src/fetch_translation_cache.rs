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
//! - Lifetime: by default one cache lives inside one call of the set loop, so a device that writes
//!   guest RAM between calls (NVMe DMA) can never leave a stale entry behind. A cache owned by the
//!   caller ([`FetchTranslationCache::persistent`], S4-14i) outlives the call; see the rules below.
//!
//! The cache is off unless the caller asks for it.
//!
//! S4-13d: what one entry covers is a choice of the caller ([`FetchCacheTag`]). By default an entry
//! is one 4 KiB page. With [`FetchCacheTag::Leaf2M`] a walk that ended at a 2 MiB leaf whose whole
//! 2 MiB span is plain RAM makes ONE entry for the leaf, tagged with the 2 MiB-aligned linear
//! address; a hit from any 4 KiB page of the leaf reads the byte at the offset of the request
//! inside the leaf (the entry holds the leaf's base, not an offset). Permissions, accessed/dirty
//! bits, faults and every flush are those of the 4 KiB tag: the three tables of the walk are
//! watched, and a 4 KiB leaf stays a 4 KiB entry.
//!
//! S4-14h: the invalidation policy is a choice of the caller ([`FetchCacheInvalidation`]).
//! `WriteWatch` (the S4-13 behaviour, the default of every caller) drops entries on a write to a
//! watched table frame, so the cache is exact against a TLB-less reference. `GuestOnly` models a
//! TLB: a write to a page table does not touch an entry, only the invalidations the guest itself
//! performs do (`INVLPG`, `MOV CR3`, a change of CR0/CR4, and a delivered `#PF` for its own
//! page), so a stale translation may still be used until the guest asks for a flush, as on
//! hardware.
//!
//! S4-14i: a cache owned by the caller ([`FetchTranslationCache::persistent`]) survives the end of
//! a call of the set loop. Nothing is flushed at a call boundary by itself: the events that flush
//! are the ones of the policy (they happen inside the loop, so a flush that happened in call N is
//! already in effect when call N + 1 starts) and the host events below. Every cache the loop is
//! given is sealed when the call ends and checked when the next call starts, whatever its
//! constructor (a one-shot cache has nothing to compare, so nothing changes for it). The seal holds
//! the translation context the call left (the paging snapshot CR0/CR3/CR4/EFER and the access
//! context, [`TranslationKey`]) and the page generation of every table frame the cache watches. The
//! check drops every entry (`host_flushes`) in two cases: - the host hands over another translation
//! context than the one the seal holds (it changed   CR0/CR3/CR4/EFER, the mode or the privilege
//! between the calls, not by a guest `MOV CR3`). This   is a flush even if the cache is empty, and
//! a later return to the old context finds no entry:   writes to the old tables made meanwhile,
//! which `GuestOnly` ignores, must not be sealed away; - a watched table frame was written by
//! anyone since the seal (a loader or harness through the   host API, a device DMA between calls:
//! they never reach [`FetchTranslationCache::note_write`]).
//!
//! This is the conservative rule, valid in both policies: a host write to a watched table frame
//! always flushes, a host write to any other frame (code bytes included, they are never cached)
//! does not, and the same context between calls keeps every entry. A caller that knows of a host
//! event neither shows (a swapped machine memory, a machine reset) calls
//! [`FetchTranslationCache::flush_for_host_event`] or [`FetchTranslationCache::reset`]. A cache is
//! bound to the memory of one machine session: a new session (a reset board is a new board) needs a
//! new cache or a `reset()`.

use crate::fetch_cache_sweep::{FetchCacheSweep, LookupKind};
use crate::page_walk::Long64ProbeConfig;
use crate::physical_memory::{GuestPhysicalRange, RamFrame};
use od_isa::memory::AccessContext;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Entries kept at once. Kernel text and the immediate callee pages are a handful of pages.
const SLOTS: usize = 4;
/// A table slot that holds no table (a 2 MiB leaf ends the walk one level early).
pub(crate) const NO_TABLE: u64 = u64::MAX;

#[derive(Clone, Copy)]
struct Entry {
    /// The linear address of the entry shifted right by `shift`: the 4 KiB page, or the 2 MiB leaf
    /// of an entry of the 2 MiB tag (S4-13d).
    linear_page: u64,
    /// 12 for a 4 KiB entry, 21 for an entry that covers a whole 2 MiB leaf.
    shift: u32,
    paging: Long64ProbeConfig,
    context: AccessContext,
    /// The plain-RAM 4 KiB frame the linear page maps to (the 2 MiB span for a 2 MiB entry).
    frame: RamFrame,
    /// Physical page numbers (address >> 12) of the tables on the path, [`NO_TABLE`] when unused.
    tables: [u64; 4],
}

/// What one entry of the cache covers (S4-13d). The tag of an entry is the linear address shifted
/// by 12 (a 4 KiB page) or by 21 (a whole 2 MiB leaf); the byte offset inside the page or the leaf
/// is always taken from the address of the request, never from the entry.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum FetchCacheTag {
    /// S4-13: one entry per 4 KiB page, also for a page inside a 2 MiB leaf (the default).
    #[default]
    Page4K,
    /// A walk that ended at a 2 MiB leaf whose 2 MiB span is plain RAM makes one entry for the
    /// whole leaf (tag: the 2 MiB-aligned linear address); a 4 KiB leaf stays a 4 KiB entry.
    Leaf2M,
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
pub struct FetchCacheStats {
    pub hits: u64,
    pub misses: u64,
    pub inserts: u64,
    /// Entries dropped by a write to a watched table frame.
    pub write_flushes: u64,
    /// Explicit flushes (translation context changes).
    pub context_flushes: u64,
    /// `GuestOnly`: flushes after a retired `INVLPG`.
    pub invlpg_flushes: u64,
    /// `GuestOnly`: flushes after a change of CR0 or CR4.
    pub control_flushes: u64,
    /// `GuestOnly`: flushes at a `#PF` delivered to the guest.
    pub fault_flushes: u64,
    /// `GuestOnly`: committed writes that touched a watched table frame and dropped nothing.
    pub ignored_table_writes: u64,
    /// Flushes caused by the host between calls: a watched table frame was written outside the
    /// loop, the translation context key changed (CR0/CR3/CR4/EFER, privilege, access context),
    /// or an explicit host event. Counted for any cache passed to the call-boundary path.
    pub host_flushes: u64,
}

#[derive(Default)]
struct Inner {
    entries: [Option<Entry>; SLOTS],
    /// Slot of the last hit or insert, tried first: consecutive bytes share their page.
    recent: usize,
    next: usize,
    stats: FetchCacheStats,
    /// The seal of the end of the last call (S4-14i); `None` before the first seal and after a host
    /// event flush, when there is nothing to compare.
    sealed: Option<Sealed>,
}

/// What a call leaves behind for the check at the start of the next one (S4-14i).
struct Sealed {
    /// The translation context the cache was left in. `None`: the configuration was unusable, which
    /// no later call is trusted to continue.
    key: Option<TranslationKey>,
    /// `(physical page number of a watched table frame, its page generation)` as of the seal.
    /// Empty while there are no entries.
    pages: Vec<(u64, u64)>,
}

/// The translation context of a call as the host hands it over (S4-14i): the paging snapshot
/// (CR0/CR3/CR4/EFER, the part of the key of an entry that the host can change between calls) and
/// the access context of the loop (mode, privilege with the CPL of the state, origin). A call
/// continues the previous one only when both are equal to what the previous call left.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) struct TranslationKey {
    pub(crate) paging: Long64ProbeConfig,
    pub(crate) access: AccessContext,
}

impl TranslationKey {
    pub(crate) fn new(paging: Long64ProbeConfig, access: AccessContext) -> Self {
        Self { paging, access }
    }
}

#[derive(Default)]
pub(crate) struct FetchTranslationCache {
    inner: RefCell<Inner>,
    invalidation: FetchCacheInvalidation,
    /// S4-13d: what one entry covers; `Page4K` unless asked (the behaviour of every earlier slice).
    tag: FetchCacheTag,
    /// Built by [`Self::persistent`]: the caller keeps the cache across calls of the set loop. It
    /// only names the intent: the loop seals and checks every cache it is given, whatever the
    /// constructor (S4-14i, review), so that no cache loses a host event at a call boundary.
    persistent: bool,
    /// S4-13c: shadow caches that receive every event of this cache (diagnostics only; `None` is
    /// no sweep and costs one branch per event).
    sweep: Option<Rc<FetchCacheSweep>>,
    /// The sweep was told of the start of the call by [`Self::with_sweep`] and the first
    /// [`Self::revalidate`] of that call must not tell it again; every later `revalidate` (a cache
    /// that serves several calls) starts a call for the shadows.
    sweep_call_open: Cell<bool>,
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

    /// A cache the caller keeps across calls of the set loop (S4-14i).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn persistent(invalidation: FetchCacheInvalidation) -> Self {
        Self {
            invalidation,
            persistent: true,
            ..Self::default()
        }
    }

    /// The same cache with the 2 MiB leaf tag ([`FetchCacheTag::Leaf2M`], S4-13d). Off unless
    /// asked: the default tag is 4 KiB.
    pub(crate) fn with_tag(mut self, tag: FetchCacheTag) -> Self {
        self.tag = tag;
        self
    }

    pub(crate) fn tag(&self) -> FetchCacheTag {
        self.tag
    }

    /// Attach the shadow caches of S4-13c. Counts as the start of a call for them (a cache built
    /// per call is attached once, when it is built; one that serves several calls starts a call
    /// for the shadows at every later [`Self::revalidate`]). They observe only: this cache answers
    /// and counts exactly as without them.
    pub(crate) fn with_sweep(mut self, sweep: Rc<FetchCacheSweep>) -> Self {
        sweep.begin_call();
        self.sweep = Some(sweep);
        self.sweep_call_open.set(true);
        self
    }

    pub(crate) fn sweep_active(&self) -> bool {
        self.sweep.is_some()
    }

    /// The guest invalidations a `WriteWatch` cache does not act on, told to the shadows only
    /// (a `GuestOnly` cache flushes itself and forwards through its own flush methods).
    pub(crate) fn sweep_guest_invlpg(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.invlpg();
        }
    }

    pub(crate) fn sweep_guest_page_fault(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.page_fault();
        }
    }

    pub(crate) fn sweep_guest_control_change(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.control_change();
        }
    }

    /// The boundary of checkpoint C5 was reached: the shadows keep their counters of phase 1.
    pub(crate) fn sweep_mark_c5(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.mark_c5();
        }
    }

    pub(crate) fn invalidation(&self) -> FetchCacheInvalidation {
        self.invalidation
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn is_persistent(&self) -> bool {
        self.persistent
    }

    /// The RAM frame of `address` when its page has a live entry for exactly this paging
    /// snapshot and access context; the byte is at `address & 0xfff` in it. A peek of the loop
    /// (CPUID, `IRETQ`, `INVLPG`, `SYSRETQ`): a miss is not followed by an insert.
    pub(crate) fn lookup(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
    ) -> Option<RamFrame> {
        self.lookup_of(address, paging, context, LookupKind::Peek)
    }

    /// [`Self::lookup`] for the fetch of an instruction byte: a miss is followed by a walk and an
    /// insert. The answer and the counters are the same; only the shadow caches of S4-13c tell the
    /// two apart.
    pub(crate) fn lookup_fetch(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
    ) -> Option<RamFrame> {
        self.lookup_of(address, paging, context, LookupKind::Fetch)
    }

    fn lookup_of(
        &self,
        address: u64,
        paging: &Long64ProbeConfig,
        context: AccessContext,
        kind: LookupKind,
    ) -> Option<RamFrame> {
        let mut inner = self.inner.borrow_mut();
        let matches = |slot: &Option<Entry>| {
            slot.as_ref().is_some_and(|entry| {
                entry.linear_page == address >> entry.shift
                    && entry.paging == *paging
                    && entry.context == context
            })
        };
        let slot = if matches(&inner.entries[inner.recent]) {
            Some(inner.recent)
        } else {
            (0..SLOTS).find(|slot| matches(&inner.entries[*slot]))
        };
        // Only the frame and the tables leave the slot (the hot path copies no more).
        let hit = slot.and_then(|slot| {
            inner.entries[slot]
                .as_ref()
                .map(|entry| (slot, entry.frame, entry.tables))
        });
        match hit {
            Some((slot, frame, tables)) => {
                inner.recent = slot;
                inner.stats.hits += 1;
                drop(inner);
                if let Some(sweep) = &self.sweep {
                    sweep.lookup(address, paging, context, kind, Some(tables));
                }
                Some(frame)
            }
            None => {
                inner.stats.misses += 1;
                drop(inner);
                if let Some(sweep) = &self.sweep {
                    sweep.lookup(address, paging, context, kind, None);
                }
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
        if let Some(sweep) = &self.sweep {
            sweep.insert(address, paging, context, tables);
        }
        let mut inner = self.inner.borrow_mut();
        // A 2 MiB span handle makes a 2 MiB entry; only the 2 MiB tag is given one (the caller
        // asks `tag()`), and a 4 KiB handle is a 4 KiB entry in either tag.
        let shift = if frame.is_large() && self.tag == FetchCacheTag::Leaf2M {
            21
        } else {
            12
        };
        let page = address >> shift;
        let entry = Entry {
            linear_page: page,
            shift,
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
                    held.shift == shift
                        && held.linear_page == page
                        && held.paging == *paging
                        && held.context == context
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
        if let Some(sweep) = &self.sweep {
            sweep.note_write(first, last);
        }
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
        if let Some(sweep) = &self.sweep {
            sweep.flush();
        }
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.context_flushes += 1;
    }

    /// Drop every entry after a retired `INVLPG` (`GuestOnly`; the flush is of the whole cache,
    /// not of the page the operand names).
    pub(crate) fn flush_after_invlpg(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.invlpg();
        }
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.invlpg_flushes += 1;
    }

    /// Drop every entry when a `#PF` is delivered to the guest (`GuestOnly`).
    pub(crate) fn flush_after_page_fault(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.page_fault();
        }
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.fault_flushes += 1;
    }

    /// Drop every entry after CR0 or CR4 changed (`GuestOnly`; a toggle of CR4.PGE is a global
    /// TLB flush, and any other change is flushed too, which is always allowed).
    pub(crate) fn flush_after_control_change(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.control_change();
        }
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.stats.control_flushes += 1;
    }

    pub(crate) fn stats(&self) -> FetchCacheStats {
        self.inner.borrow().stats
    }

    /// Drop every entry on a host event the page generations cannot show (S4-14i). Counted in
    /// `host_flushes`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn flush_for_host_event(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.host_event();
        }
        let mut inner = self.inner.borrow_mut();
        inner.entries = [None; SLOTS];
        inner.sealed = None;
        inner.stats.host_flushes += 1;
    }

    /// Machine reset or a new machine session over this cache object (S4-14i): drop the entries,
    /// the seal and the counters; policy and persistence stay.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn reset(&self) {
        if let Some(sweep) = &self.sweep {
            sweep.reset();
        }
        *self.inner.borrow_mut() = Inner::default();
    }

    /// Seal the cache at the end of a call: remember the translation context it was left in and
    /// the generation of every watched table frame. A frame whose generation is unknown is stored
    /// as `0`, which no real generation equals (they start at 1), so the next check flushes.
    pub(crate) fn seal(
        &self,
        key: Option<TranslationKey>,
        generation_of: &dyn Fn(u64) -> Option<u64>,
    ) {
        if let Some(sweep) = &self.sweep {
            sweep.seal(key, generation_of);
        }
        let mut inner = self.inner.borrow_mut();
        let mut pages: Vec<u64> = inner
            .entries
            .iter()
            .flatten()
            .flat_map(|entry| entry.tables)
            .filter(|table| *table != NO_TABLE)
            .collect();
        pages.sort_unstable();
        pages.dedup();
        let pages = pages
            .into_iter()
            .map(|page| (page, generation_of(page).unwrap_or(0)))
            .collect();
        inner.sealed = Some(Sealed { key, pages });
    }

    /// The check at the start of a call. The cache is dropped (`host_flushes`, once) when
    /// - the translation context the host passes (`key`) is not the one the previous call left: the
    ///   host changed CR0/CR3/CR4/EFER or the access context between the calls without a guest
    ///   `MOV CR3`, and writes to the tables of the old context could have been made while the
    ///   other was active, which the generations below would then show as already seen; this holds
    ///   with no entries too, a changed context is a host event of the cache whatever it holds; or
    /// - any sealed table frame has a different generation now: it was written outside the loop
    ///   (host API, device DMA) since the seal.
    ///
    /// Then the cache is sealed again with the current context, so that a call which ends before
    /// anything runs leaves a valid seal. A cache that was never sealed has nothing to compare.
    pub(crate) fn revalidate(
        &self,
        key: Option<TranslationKey>,
        generation_of: &dyn Fn(u64) -> Option<u64>,
    ) {
        if let Some(sweep) = &self.sweep {
            // The call that attached the sweep already began for it; any later call of the same
            // cache begins here, before the check, so that the `PerCall` shadows are emptied.
            if !self.sweep_call_open.replace(false) {
                sweep.begin_call();
            }
            sweep.revalidate(key, generation_of);
        }
        let changed = {
            let inner = self.inner.borrow();
            inner.sealed.as_ref().is_some_and(|sealed| {
                let same_context =
                    matches!((&sealed.key, &key), (Some(held), Some(now)) if held == now);
                !same_context
                    || sealed
                        .pages
                        .iter()
                        .any(|(page, sealed)| generation_of(*page) != Some(*sealed))
            })
        };
        if changed {
            let mut inner = self.inner.borrow_mut();
            inner.entries = [None; SLOTS];
            inner.stats.host_flushes += 1;
        }
        self.seal(key, generation_of);
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

    fn key(cr3: u64, privilege: PrivilegeLevel) -> Option<TranslationKey> {
        Some(TranslationKey::new(paging(cr3), context(privilege)))
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

    /// S4-14i: a persistent cache keeps its entries across a seal and a check that finds the
    /// generations of its table frames unchanged, drops them when one of them moved, and a
    /// frame that is not watched does not matter.
    #[test]
    fn a_persistent_cache_is_dropped_only_by_a_write_to_a_watched_table_frame() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let cache = FetchTranslationCache::persistent(policy);
            assert!(cache.is_persistent());
            assert!(!FetchTranslationCache::with_invalidation(policy).is_persistent());
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
            // The generation of a page number: the pages 1..=4 are the watched tables.
            let generations = std::cell::RefCell::new([1u64; 16]);
            let generation_of = |page: u64| generations.borrow().get(page as usize).copied();
            let ctx = key(0x1000, PrivilegeLevel::Ring0);
            cache.seal(ctx, &generation_of);
            cache.revalidate(ctx, &generation_of);
            assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
            assert_eq!(cache.stats().host_flushes, 0);
            // A write to a frame that no entry watches (page 8, the code frame) keeps it.
            generations.borrow_mut()[8] += 1;
            cache.revalidate(ctx, &generation_of);
            assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
            assert_eq!(cache.stats().host_flushes, 0);
            // A write to a watched table frame (page 3) drops the entry, once.
            generations.borrow_mut()[3] += 1;
            cache.revalidate(ctx, &generation_of);
            assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
            assert_eq!(cache.stats().host_flushes, 1, "{policy:?}");
            cache.revalidate(ctx, &generation_of);
            assert_eq!(cache.stats().host_flushes, 1, "{policy:?}");
        }
    }

    /// S6-30b: a write to a watched table frame reaches the cache by both routes the contract
    /// names, with the real memory behind them. The writer's own `committed_write` callback
    /// drives `note_write` (the in-loop route); a write by another vCPU that never calls this
    /// cache's `note_write` is seen through the atomic page generation at the next check (the
    /// cross-thread route: `code_page_generation` is an `Acquire` load of what the store raised
    /// with `Release`). The old path (`write_u64`) raises the same generation; that equality is tested in `physical_memory`.
    #[test]
    fn a_write_to_a_table_frame_drops_the_entry_by_callback_and_by_generation() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let table = GuestPhysicalAddress::new(0x3008);
        let ctx = key(0x1000, PrivilegeLevel::Ring0);
        let generation_of = |page: u64| {
            memory
                .code_page_generation(GuestPhysicalAddress::new(page << 12))
                .ok()
        };
        let insert = |cache: &FetchTranslationCache| {
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            )
        };
        // In-loop route: the callback of the new store feeds `note_write`.
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::WriteWatch);
        insert(&cache);
        memory.store_u64(table, 7, |r| cache.note_write(r)).unwrap();
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        assert_eq!(cache.stats().write_flushes, 1);
        // Cross-thread route: nobody tells this cache; the generation of the frame moved.
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let cache = FetchTranslationCache::persistent(policy);
            insert(&cache);
            cache.seal(ctx, &generation_of);
            cache.revalidate(ctx, &generation_of);
            assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
            // A write to a frame no entry watches (the code frame) keeps it.
            memory
                .store_u64(GuestPhysicalAddress::new(0x8000), 1, |_| {})
                .unwrap();
            cache.revalidate(ctx, &generation_of);
            assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
            // The other vCPU writes a table frame, once through the new store.
            std::thread::scope(|scope| {
                scope.spawn(|| memory.store_u64(table, 9, |_| {}).unwrap());
            });
            cache.revalidate(ctx, &generation_of);
            assert_eq!(
                cache.lookup(CODE_VA, &paging(0x1000), ring0),
                None,
                "{policy:?}"
            );
            assert_eq!(cache.stats().host_flushes, 1, "{policy:?}");
        }
    }

    /// A frame whose generation cannot be read is sealed as unknown and the next check flushes.
    #[test]
    fn a_table_frame_of_unknown_generation_is_not_trusted() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::WriteWatch);
        cache.insert(
            CODE_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        let ctx = key(0x1000, PrivilegeLevel::Ring0);
        cache.seal(ctx, &|_| None);
        cache.revalidate(ctx, &|page| Some(page + 1));
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        assert_eq!(cache.stats().host_flushes, 1);
    }

    /// S4-14i review: the host hands over another translation context between two calls. The
    /// cache is dropped at once (not only on a generation change), and the writes to the old
    /// tables that a call under the other context made, which a policy may ignore and a seal at
    /// its end would then count as seen, cannot revive a stale entry when the old context returns.
    #[test]
    fn another_context_between_calls_drops_the_entries_and_a_return_finds_none() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let a = key(0x1000, PrivilegeLevel::Ring0);
        let b = key(0x5000, PrivilegeLevel::Ring0);
        let generations = std::cell::RefCell::new([1u64; 16]);
        let generation_of = |page: u64| generations.borrow().get(page as usize).copied();
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let cache = FetchTranslationCache::persistent(policy);
            // Call N under A: the entry, sealed.
            cache.revalidate(a, &generation_of);
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
            cache.seal(a, &generation_of);
            // Call N + 1 under B: the check drops the entry; the guest writes a table of A and the
            // call ends sealed with the new generation.
            cache.revalidate(b, &generation_of);
            assert_eq!(cache.stats().host_flushes, 1, "{policy:?}");
            generations.borrow_mut()[3] += 1;
            cache.seal(b, &generation_of);
            // The host returns to A: the same context key as the entry, but no entry is left.
            cache.revalidate(a, &generation_of);
            assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
            assert_eq!(cache.stats().host_flushes, 2, "{policy:?}");
        }
    }

    /// The context compared is the whole key: each of CR0, CR3, CR4, EFER (the paging snapshot) and
    /// the privilege and the origin of the access context drops the cache; the same key does not,
    /// however many calls it spans; an empty cache counts the flush too; a configuration the loop
    /// cannot run is never trusted; a cache that was never sealed compares nothing.
    #[test]
    fn the_context_check_compares_every_part_of_the_key() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let generation_of = |_: u64| Some(1);
        let base = key(0x1000, PrivilegeLevel::Ring0);
        let paging_with = |cr0: u64, cr3: u64, cr4: u64, efer: u64| {
            Some(TranslationKey::new(
                Long64ProbeConfig::new(cr0, cr3, cr4, efer).unwrap(),
                ring0,
            ))
        };
        let others = [
            paging_with(CR0 | (1 << 18), 0x1000, CR4, EFER),
            paging_with(CR0, 0x2000, CR4, EFER),
            paging_with(CR0, 0x1000, CR4 | (1 << 7), EFER),
            paging_with(CR0, 0x1000, CR4, EFER | 1),
            key(0x1000, PrivilegeLevel::Ring3),
            Some(TranslationKey::new(
                paging(0x1000),
                AccessContext {
                    origin: AccessOrigin::ImplicitStack,
                    ..ring0
                },
            )),
            None,
        ];
        for (index, other) in others.into_iter().enumerate() {
            let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::GuestOnly);
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            );
            cache.revalidate(base, &generation_of);
            assert_eq!(cache.stats().host_flushes, 0, "first check, {index}");
            for _ in 0..3 {
                cache.seal(base, &generation_of);
                cache.revalidate(base, &generation_of);
                assert!(cache.lookup(CODE_VA, &paging(0x1000), ring0).is_some());
            }
            assert_eq!(cache.stats().host_flushes, 0, "same key, {index}");
            cache.seal(base, &generation_of);
            cache.revalidate(other, &generation_of);
            assert_eq!(
                cache.lookup(CODE_VA, &paging(0x1000), ring0),
                None,
                "{index}"
            );
            assert_eq!(cache.stats().host_flushes, 1, "{index}");
        }
        // No entries at all: the context still counts.
        let empty = FetchTranslationCache::new();
        empty.revalidate(base, &generation_of);
        empty.seal(base, &generation_of);
        empty.revalidate(key(0x5000, PrivilegeLevel::Ring0), &generation_of);
        assert_eq!(empty.stats().host_flushes, 1);
    }

    /// The explicit host event and the reset of a cache object: the entries go, the reset also
    /// clears the counters and the seal, and policy and persistence stay.
    #[test]
    fn the_host_event_flush_and_the_reset_drop_everything() {
        let memory = memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::GuestOnly);
        let insert = |cache: &FetchTranslationCache| {
            cache.insert(
                CODE_VA,
                &paging(0x1000),
                ring0,
                frame(&memory, 0x8000),
                TABLES,
            )
        };
        insert(&cache);
        cache.seal(None, &|_| Some(1));
        cache.flush_for_host_event();
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        assert_eq!(cache.stats().host_flushes, 1);
        insert(&cache);
        cache.seal(None, &|_| Some(1));
        cache.reset();
        assert_eq!(cache.stats(), FetchCacheStats::default());
        assert_eq!(cache.lookup(CODE_VA, &paging(0x1000), ring0), None);
        assert!(cache.is_persistent());
        assert_eq!(cache.invalidation(), FetchCacheInvalidation::GuestOnly);
        // After the reset the seal is empty: a check has nothing to compare and flushes nothing.
        cache.revalidate(None, &|_| Some(99));
        assert_eq!(cache.stats().host_flushes, 0);
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
    // ---- S4-13d: the 2 MiB leaf tag ----

    /// 7 MiB of RAM at 0 (so the 2 MiB spans at 0x20_0000 and 0x40_0000 are RAM and the one at
    /// 0x60_0000 is only partly RAM) and a ROM page after it.
    fn big_memory() -> PhysicalMemory {
        let specs = [
            MemoryRegionSpec::new(range(0, 0x70_0000), MemoryRegionKind::Ram),
            MemoryRegionSpec::new(range(0x70_0000, 0x1000), MemoryRegionKind::Rom),
        ];
        PhysicalMemory::new(
            &MachineMemoryMap::new(36, specs.to_vec()).unwrap(),
            RamLimits::new(16 << 20),
        )
        .unwrap()
    }

    fn leaf(memory: &PhysicalMemory, base: u64) -> RamFrame {
        memory.ram_leaf_2m(GuestPhysicalAddress::new(base)).unwrap()
    }

    /// A walk that ended at a 2 MiB leaf has three tables.
    const LEAF_TABLES: [Option<u64>; 4] = [Some(0x1000), Some(0x2000), Some(0x3000), None];
    /// The 2 MiB-aligned linear address of the leaf of these cases.
    const LEAF_VA: u64 = 0x0100_0000;

    fn tagged() -> FetchTranslationCache {
        FetchTranslationCache::new().with_tag(FetchCacheTag::Leaf2M)
    }

    /// The default tag is 4 KiB; only a handle to a 2 MiB-aligned span that is wholly plain RAM
    /// exists (a base that is not aligned, a span ending in ROM or in a hole, and a span that
    /// leaves the RAM region have none).
    #[test]
    fn the_default_tag_is_4k_and_a_2m_handle_needs_a_wholly_ram_aligned_span() {
        assert_eq!(FetchTranslationCache::new().tag(), FetchCacheTag::Page4K);
        assert_eq!(
            FetchTranslationCache::default().tag(),
            FetchCacheTag::Page4K
        );
        assert_eq!(tagged().tag(), FetchCacheTag::Leaf2M);
        let memory = big_memory();
        for base in [0x20_0000, 0x40_0000] {
            let handle = memory.ram_leaf_2m(GuestPhysicalAddress::new(base)).unwrap();
            assert!(handle.is_large());
        }
        assert!(!frame(&memory, 0x8000).is_large());
        // Not 2 MiB aligned, partly beyond the RAM (0x60_0000 + 2 MiB > 7 MiB), in the ROM, in a
        // hole.
        for base in [0x20_1000, 0x60_0000, 0x70_0000, 0x100_0000] {
            assert_eq!(memory.ram_leaf_2m(GuestPhysicalAddress::new(base)), None);
        }
    }

    /// One entry covers the whole leaf: any 4 KiB page of it hits, and the byte is read at the
    /// offset of the REQUEST inside the leaf (never the offset of the request that made the
    /// entry), so the physical address is `leaf base + offset`. The neighbouring leaves, another
    /// CR3 and another privilege do not hit.
    #[test]
    fn a_2m_entry_serves_every_page_of_its_leaf_with_the_offset_of_the_request() {
        let mut memory = big_memory();
        for offset in [0u64, 0x1, 0x1000, 0x1234, 0x1f_f000, 0x1f_ffff] {
            memory
                .write_u8(
                    GuestPhysicalAddress::new(0x20_0000 + offset),
                    (offset % 251) as u8 + 1,
                    |_| {},
                )
                .unwrap();
        }
        let cache = tagged();
        let ring0 = context(PrivilegeLevel::Ring0);
        // The entry is made by a fetch deep inside the leaf.
        cache.insert(
            LEAF_VA + 0x1234,
            &paging(0x1000),
            ring0,
            leaf(&memory, 0x20_0000),
            LEAF_TABLES,
        );
        for offset in [0u64, 0x1, 0x1000, 0x1234, 0x1f_f000, 0x1f_ffff] {
            let frame = cache
                .lookup(LEAF_VA + offset, &paging(0x1000), ring0)
                .unwrap_or_else(|| panic!("offset {offset:#x} must hit"));
            assert_eq!(
                memory.read_ram_frame_u8(frame, LEAF_VA + offset),
                memory
                    .read_u8(GuestPhysicalAddress::new(0x20_0000 + offset))
                    .ok(),
                "offset {offset:#x}"
            );
        }
        assert_eq!(cache.lookup(LEAF_VA - 1, &paging(0x1000), ring0), None);
        assert_eq!(
            cache.lookup(LEAF_VA + 0x20_0000, &paging(0x1000), ring0),
            None
        );
        assert_eq!(cache.lookup(LEAF_VA, &paging(0x5000), ring0), None);
        assert_eq!(
            cache.lookup(LEAF_VA, &paging(0x1000), context(PrivilegeLevel::Ring3)),
            None
        );
        let stats = cache.stats();
        assert_eq!((stats.hits, stats.inserts), (6, 1));
        assert_eq!(stats.misses, 4);
    }

    /// A 4 KiB handle stays a 4 KiB entry whatever the tag (a 4 KiB leaf of a cache with the 2 MiB
    /// tag), and the default tag never widens an entry. A 2 MiB entry and a 4 KiB entry of
    /// another leaf live side by side; a key is replaced in place.
    #[test]
    fn a_4k_handle_is_a_4k_entry_in_either_tag_and_entries_of_both_sizes_coexist() {
        let memory = big_memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let cache = tagged();
        cache.insert(
            LEAF_VA,
            &paging(0x1000),
            ring0,
            frame(&memory, 0x8000),
            TABLES,
        );
        assert!(
            cache
                .lookup(LEAF_VA + 0xfff, &paging(0x1000), ring0)
                .is_some()
        );
        assert_eq!(cache.lookup(LEAF_VA + 0x1000, &paging(0x1000), ring0), None);
        // A 2 MiB entry for the next leaf next to it, then the same key again: replaced in place.
        let next = LEAF_VA + 0x20_0000;
        cache.insert(
            next,
            &paging(0x1000),
            ring0,
            leaf(&memory, 0x20_0000),
            LEAF_TABLES,
        );
        cache.insert(
            next + 0x5000,
            &paging(0x1000),
            ring0,
            leaf(&memory, 0x40_0000),
            LEAF_TABLES,
        );
        assert_eq!(
            cache.lookup(next + 0x10_0000, &paging(0x1000), ring0),
            Some(leaf(&memory, 0x40_0000))
        );
        assert!(cache.lookup(LEAF_VA, &paging(0x1000), ring0).is_some());
        // The default tag keeps even a 2 MiB handle to its 4 KiB page.
        let plain = FetchTranslationCache::new();
        plain.insert(
            next,
            &paging(0x1000),
            ring0,
            leaf(&memory, 0x20_0000),
            LEAF_TABLES,
        );
        assert!(plain.lookup(next + 0xfff, &paging(0x1000), ring0).is_some());
        assert_eq!(plain.lookup(next + 0x1000, &paging(0x1000), ring0), None);
    }

    /// Every flush of a 4 KiB entry drops a 2 MiB entry too: a write to any of the three tables of
    /// the walk (a PD frame included), `MOV CR3`, `INVLPG`, a `#PF`, a CR0/CR4 change and the host
    /// events; a write to a frame that is none of them does not.
    #[test]
    fn a_2m_entry_is_dropped_by_the_events_that_drop_a_4k_entry() {
        let memory = big_memory();
        let ring0 = context(PrivilegeLevel::Ring0);
        let fresh = |invalidation| {
            let cache = FetchTranslationCache::with_invalidation(invalidation)
                .with_tag(FetchCacheTag::Leaf2M);
            cache.insert(
                LEAF_VA + 0x3000,
                &paging(0x1000),
                ring0,
                leaf(&memory, 0x20_0000),
                LEAF_TABLES,
            );
            cache
        };
        let live = |cache: &FetchTranslationCache| {
            cache
                .lookup(LEAF_VA + 0x10_0000, &paging(0x1000), ring0)
                .is_some()
        };
        for (write, flushes) in [
            (range(0x1000, 8), true),
            (range(0x2ff8, 16), true),
            (range(0x3040, 8), true),
            // The frame a 4 KiB walk would have as its fourth table is not watched.
            (range(0x4000, 8), false),
            (range(0x20_0000, 8), false),
        ] {
            let cache = fresh(FetchCacheInvalidation::WriteWatch);
            cache.note_write(write);
            assert_eq!(live(&cache), !flushes, "{write:?}");
            assert_eq!(cache.stats().write_flushes, u64::from(flushes), "{write:?}");
        }
        // `GuestOnly`: the table write is only counted, the guest invalidations drop the entry.
        let cache = fresh(FetchCacheInvalidation::GuestOnly);
        cache.note_write(range(0x3040, 8));
        assert!(live(&cache));
        assert_eq!(cache.stats().ignored_table_writes, 1);
        let events: [(&str, fn(&FetchTranslationCache)); 5] = [
            ("mov cr3", FetchTranslationCache::flush),
            ("invlpg", FetchTranslationCache::flush_after_invlpg),
            ("#pf", FetchTranslationCache::flush_after_page_fault),
            ("cr0/cr4", FetchTranslationCache::flush_after_control_change),
            ("host", FetchTranslationCache::flush_for_host_event),
        ];
        for (name, event) in events {
            let cache = fresh(FetchCacheInvalidation::GuestOnly);
            assert!(live(&cache), "{name}");
            event(&cache);
            assert!(!live(&cache), "{name}");
        }
        // The call boundary: another CR3 handed over by the host, or a written table frame.
        let cache = fresh(FetchCacheInvalidation::WriteWatch);
        cache.seal(key(0x1000, PrivilegeLevel::Ring0), &|_| Some(1));
        cache.revalidate(key(0x1000, PrivilegeLevel::Ring0), &|_| Some(1));
        assert!(live(&cache));
        cache.revalidate(key(0x5000, PrivilegeLevel::Ring0), &|_| Some(1));
        assert!(!live(&cache));
        let cache = fresh(FetchCacheInvalidation::WriteWatch);
        cache.seal(key(0x1000, PrivilegeLevel::Ring0), &|_| Some(1));
        cache.revalidate(key(0x1000, PrivilegeLevel::Ring0), &|_| Some(2));
        assert!(!live(&cache));
        assert_eq!(cache.stats().host_flushes, 1);
    }
}
