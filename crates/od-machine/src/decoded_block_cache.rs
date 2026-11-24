//! PERF-31 (ADR-0018, variant (a), prototype): the cache of predecoded straight-line blocks of the
//! set loop (see `od_isa::predecoded_block`).
//!
//! It is the decoded-instruction cache of PERF-10 ([`crate::decoded_instruction_cache`]) for whole
//! blocks: an entry is keyed by the linear RIP of the first instruction, the physical place of its
//! first byte (RAM region and byte offset inside it), the generation of the 4 KiB code page (read
//! BEFORE the bytes of the decode) and the `AccessContext`, and the adapter of the set loop adds the
//! same gate (the fetch translation cache still holds the translation of the page for the current
//! paging snapshot). A block never leaves the page of its first RIP, so the one page generation
//! covers every byte of it: a committed write to the page (self-modifying code, a device, a DMA)
//! makes the entry a miss. Inside a call the adapter checks, before every further instruction of a
//! block, that the page generation and the fetch cache are still those of the entry of the block
//! (see `PagedMovMemoryAdapter::block_holds`). An entry may also say that no block starts at the
//! RIP (the first instruction is not a block form), so that such an instruction is not decoded
//! again at every visit.
//!
//! The cache is owned by the board and lives between quanta. It is bound to one `PhysicalMemory`
//! (another memory, a board reset and a snapshot restore empty it); a translation context change
//! of the guest (`MOV CR3`, an EFER write) flushes it in O(1) (an epoch). The table is direct-mapped
//! and allocated on the first insert. The counters ([`DecodedBlockCacheStats`]) are the diagnostic
//! of ADR-0018, section 5 (M1). They count only when asked for (PERF-33, E2): the board turns them on
//! for the runner's `--block-stats` ([`DecodedBlockCache::enable_stats`]), and a debug build
//! (the tests, [`BLOCK_CACHE_COUNTED`]) counts from the start. A cache that does not count writes
//! no counter: every update is behind one test of a plain `bool` field.

use crate::physical_memory::RamCodePage;
use od_isa::long_decode::DecodedBlock;
use od_isa::memory::AccessContext;
use std::cell::{Cell, RefCell};

/// Whether a new [`DecodedBlockCache`] counts before [`DecodedBlockCache::enable_stats`] (debug
/// builds: the tests read the counters), by the pattern of `DATA_CACHE_COUNTED`. In a release
/// build only `--block-stats` turns them on.
pub const BLOCK_CACHE_COUNTED: bool = cfg!(debug_assertions);

/// The default size of the table: 2^13 slots (PERF-33, E2 step 5: with blocks of up to
/// `MAX_BLOCK_INSTRUCTIONS` = 64 the memory is bounded by 38 404 096 bytes, under 64 MiB; 2^14
/// slots gave 76 808 192 bytes).
pub(crate) const DEFAULT_DECODED_BLOCK_SLOTS: usize = 1 << 13;

/// The buckets of [`DecodedBlockCacheStats::run_lengths`]: the lower bound of each.
pub const BLOCK_RUN_LENGTH_BUCKETS: [u64; 7] = [1, 2, 3, 4, 8, 16, 32];

#[derive(Clone)]
struct Entry {
    rip: u64,
    region: usize,
    offset: usize,
    generation: u64,
    context: AccessContext,
    epoch: u64,
    /// `None`: no block starts at this RIP.
    block: Option<DecodedBlock>,
}

/// What the cache counted (ADR-0018, section 5, M1). "Run": one execution of a block from its first
/// instruction until the run ends (a branch, the end of the block, a refusal of the fast path, a
/// fault, a block that no longer holds, the end of the call); only runs that retired at least one
/// instruction are counted.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecodedBlockCacheStats {
    /// Lookups the gate allowed.
    pub lookups: u64,
    /// Lookups that returned a block.
    pub hits: u64,
    /// Lookups that found an entry saying that no block starts at the RIP.
    pub empty_hits: u64,
    /// Lookups that found no valid entry (and then decoded the block).
    pub misses: u64,
    /// Misses on an entry of the same RIP, place, context and epoch but an older page generation
    /// (the page was written since the block was built).
    pub stale: u64,
    /// Blocks stored (not counting the entries that say there is none).
    pub built_blocks: u64,
    /// Their instructions.
    pub built_instructions: u64,
    /// Flushes by a translation context change of the guest.
    pub flushes: u64,
    /// Times the whole table was emptied (`reset`, or another memory bound).
    pub resets: u64,
    /// Runs ended before an instruction because the block no longer held (its page was written or
    /// the fetch cache dropped its translations).
    pub holds_failed: u64,
    /// Runs that retired at least one instruction.
    pub runs: u64,
    /// Instructions retired from blocks.
    pub retired: u64,
    /// Of them, the instructions of runs that retired at least two.
    pub retired_in_runs_of_two_or_more: u64,
    /// Runs by the number of instructions they retired, in the buckets of
    /// [`BLOCK_RUN_LENGTH_BUCKETS`] (1, 2, 3, 4-7, 8-15, 16-31, 32 and more).
    pub run_lengths: [u64; 7],
}

/// The counters in place: one `Cell` each, so that a count is one add with no borrow flag to check
/// and set (the `RefCell` of PERF-31 did both at every count).
#[derive(Default)]
struct Counters {
    lookups: Cell<u64>,
    hits: Cell<u64>,
    empty_hits: Cell<u64>,
    misses: Cell<u64>,
    stale: Cell<u64>,
    built_blocks: Cell<u64>,
    built_instructions: Cell<u64>,
    flushes: Cell<u64>,
    resets: Cell<u64>,
    holds_failed: Cell<u64>,
    runs: Cell<u64>,
    retired: Cell<u64>,
    retired_in_runs_of_two_or_more: Cell<u64>,
    run_lengths: [Cell<u64>; BLOCK_RUN_LENGTH_BUCKETS.len()],
}

#[inline(always)]
fn add(counter: &Cell<u64>, amount: u64) {
    counter.set(counter.get() + amount);
}

impl Counters {
    fn snapshot(&self) -> DecodedBlockCacheStats {
        DecodedBlockCacheStats {
            lookups: self.lookups.get(),
            hits: self.hits.get(),
            empty_hits: self.empty_hits.get(),
            misses: self.misses.get(),
            stale: self.stale.get(),
            built_blocks: self.built_blocks.get(),
            built_instructions: self.built_instructions.get(),
            flushes: self.flushes.get(),
            resets: self.resets.get(),
            holds_failed: self.holds_failed.get(),
            runs: self.runs.get(),
            retired: self.retired.get(),
            retired_in_runs_of_two_or_more: self.retired_in_runs_of_two_or_more.get(),
            run_lengths: self.run_lengths.each_ref().map(Cell::get),
        }
    }
}

/// The answer of [`DecodedBlockCache::lookup`].
pub(crate) enum BlockLookup {
    Block(DecodedBlock),
    /// A valid entry: no block starts here.
    Empty,
    Miss,
}

pub(crate) struct DecodedBlockCache {
    shift: u32,
    table: RefCell<Vec<Option<Entry>>>,
    live: Cell<usize>,
    memory: Cell<usize>,
    epoch: Cell<u64>,
    /// The instructions retired so far by the current run (0: none open); kept only while
    /// `counted`.
    run: Cell<u64>,
    /// Whether the counters count (see [`BLOCK_CACHE_COUNTED`], [`Self::enable_stats`]). Set only
    /// under `&mut self`, so the hot path reads a plain field.
    counted: bool,
    counters: Counters,
    /// Test-only mutation switch: a cache that ignores its invalidation (the page generation at the
    /// lookup and inside a block, the fetch cache drops inside a block), for the tests that prove
    /// the invalidation is needed.
    #[cfg(test)]
    ignore_invalidation: Cell<bool>,
}

impl Default for DecodedBlockCache {
    fn default() -> Self {
        Self::new(DEFAULT_DECODED_BLOCK_SLOTS)
    }
}

impl DecodedBlockCache {
    /// A cache of `slots` entries, rounded up to a power of two (at least 2). Nothing is allocated
    /// until the first entry is stored.
    pub(crate) fn new(slots: usize) -> Self {
        let slots = slots.max(2).next_power_of_two();
        Self {
            shift: 64 - slots.trailing_zeros(),
            table: RefCell::new(Vec::new()),
            live: Cell::new(0),
            memory: Cell::new(0),
            epoch: Cell::new(0),
            run: Cell::new(0),
            counted: BLOCK_CACHE_COUNTED,
            counters: Counters::default(),
            #[cfg(test)]
            ignore_invalidation: Cell::new(false),
        }
    }

    fn slots(&self) -> usize {
        1usize << (64 - self.shift)
    }

    /// PERF-33 (E2): the counters count from now on (the runner's `--block-stats`).
    pub(crate) fn enable_stats(&mut self) {
        self.counted = true;
    }

    /// Test-only: a cache that does not count, as a release build without `--block-stats`.
    #[cfg(test)]
    pub(crate) fn disable_stats_for_test(&mut self) {
        self.counted = false;
    }

    /// Whether the counters count.
    #[inline(always)]
    pub(crate) fn counted(&self) -> bool {
        self.counted
    }

    /// The counters, with the run still open counted as ended (all zero when the cache does not
    /// count).
    pub(crate) fn stats(&self) -> DecodedBlockCacheStats {
        self.close_run();
        self.counters.snapshot()
    }

    /// Empties the table (the board reset and the snapshot restore call this). The counters stay.
    pub(crate) fn reset(&self) {
        if self.live.get() != 0 {
            self.table
                .borrow_mut()
                .iter_mut()
                .for_each(|slot| *slot = None);
            self.live.set(0);
        }
        self.memory.set(0);
        if self.counted {
            add(&self.counters.resets, 1);
        }
    }

    /// Ties the cache to the memory of the run that is about to start (`identity` is the address
    /// of its `PhysicalMemory`): another memory empties the table.
    pub(crate) fn bind_memory(&self, identity: usize) {
        let bound = self.memory.get();
        if bound == identity {
            return;
        }
        if bound != 0 {
            self.reset();
        }
        self.memory.set(identity);
    }

    /// A translation context change of the guest: every entry is dropped (in O(1): the entries of
    /// an older epoch never hit and are replaced in place).
    pub(crate) fn flush(&self) {
        self.epoch.set(self.epoch.get() + 1);
        if self.counted {
            add(&self.counters.flushes, 1);
        }
    }

    fn index(&self, region: usize, offset: usize) -> usize {
        let key = (offset as u64) ^ ((region as u64) << 48);
        (key.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> self.shift) as usize
    }

    /// Whether the page generation `now` is the one an entry or a block was made under (`held`).
    #[inline(always)]
    fn generation_holds(&self, held: u64, now: u64) -> bool {
        #[cfg(test)]
        if self.ignore_invalidation.get() {
            return true;
        }
        held == now
    }

    /// Whether a further instruction of an active block may run from it: the page generation and
    /// the count of fetch cache drops (`(held, now)` each) are those of the lookup that answered it.
    #[inline(always)]
    pub(crate) fn block_holds(&self, generation: (u64, u64), fetch_drops: (u64, u64)) -> bool {
        #[cfg(test)]
        if self.ignore_invalidation.get() {
            return true;
        }
        generation.0 == generation.1 && fetch_drops.0 == fetch_drops.1
    }

    /// The entry for exactly this key (`page` carries the generation the caller read before it
    /// would read the bytes).
    pub(crate) fn lookup(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
    ) -> BlockLookup {
        let epoch = self.epoch.get();
        let found = {
            let table = self.table.borrow();
            match table
                .get(self.index(page.region, page.offset))
                .and_then(Option::as_ref)
            {
                Some(entry)
                    if entry.rip == rip
                        && entry.region == page.region
                        && entry.offset == page.offset
                        && entry.context == context
                        && entry.epoch == epoch =>
                {
                    if self.generation_holds(entry.generation, page.generation) {
                        match &entry.block {
                            Some(block) => BlockLookup::Block(DecodedBlock::clone(block)),
                            None => BlockLookup::Empty,
                        }
                    } else {
                        if self.counted {
                            add(&self.counters.stale, 1);
                        }
                        BlockLookup::Miss
                    }
                }
                _ => BlockLookup::Miss,
            }
        };
        if self.counted {
            let counters = &self.counters;
            add(&counters.lookups, 1);
            add(
                match found {
                    BlockLookup::Block(_) => &counters.hits,
                    BlockLookup::Empty => &counters.empty_hits,
                    BlockLookup::Miss => &counters.misses,
                },
                1,
            );
        }
        found
    }

    /// Stores the block decoded from the bytes of `page` (its generation read before them), or
    /// that none starts at `rip`, replacing whatever held the slot.
    pub(crate) fn insert(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
        block: Option<DecodedBlock>,
    ) {
        if self.counted
            && let Some(block) = &block
        {
            add(&self.counters.built_blocks, 1);
            add(&self.counters.built_instructions, block.len() as u64);
        }
        let mut table = self.table.borrow_mut();
        if table.is_empty() {
            table.resize(self.slots(), None);
        }
        let slot = self.index(page.region, page.offset);
        if table[slot].is_none() {
            self.live.set(self.live.get() + 1);
        }
        table[slot] = Some(Entry {
            rip,
            region: page.region,
            offset: page.offset,
            generation: page.generation,
            context,
            epoch: self.epoch.get(),
            block,
        });
    }

    /// The instruction at `index` of a block retired from it; `index` 0 starts a new run.
    #[inline(always)]
    pub(crate) fn note_retired(&self, index: usize) {
        if !self.counted {
            return;
        }
        if index == 0 {
            self.close_run();
        }
        self.run.set(self.run.get() + 1);
    }

    /// A run ended because the block no longer held.
    #[inline(always)]
    pub(crate) fn note_holds_failed(&self) {
        if self.counted {
            add(&self.counters.holds_failed, 1);
        }
    }

    fn close_run(&self) {
        let length = self.run.replace(0);
        if length == 0 {
            return;
        }
        let bucket = BLOCK_RUN_LENGTH_BUCKETS
            .iter()
            .rposition(|low| length >= *low)
            .expect("a run retired at least one");
        let counters = &self.counters;
        add(&counters.runs, 1);
        add(&counters.retired, length);
        if length >= 2 {
            add(&counters.retired_in_runs_of_two_or_more, length);
        }
        add(&counters.run_lengths[bucket], 1);
    }

    /// Test-only: the mutation of the invalidation (see `ignore_invalidation`).
    #[cfg(test)]
    pub(crate) fn ignore_invalidation_for_test(&self) {
        self.ignore_invalidation.set(true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoded_instruction_cache::test_support::{context, decoded, page};
    use od_isa::memory::PrivilegeLevel;

    fn block(count: usize) -> DecodedBlock {
        let nop = decoded(&[0x89, 0xC0]).unwrap(); // MOV EAX, EAX
        std::iter::repeat_n(nop, count).collect()
    }

    fn hit(cache: &DecodedBlockCache, rip: u64, place: RamCodePage) -> Option<usize> {
        match cache.lookup(rip, context(PrivilegeLevel::Ring0), place) {
            BlockLookup::Block(block) => Some(block.len()),
            BlockLookup::Empty => Some(0),
            BlockLookup::Miss => None,
        }
    }

    #[test]
    fn a_hit_needs_rip_place_generation_context_and_epoch() {
        let cache = DecodedBlockCache::new(16);
        let ring0 = context(PrivilegeLevel::Ring0);
        assert_eq!(hit(&cache, 0x1000, page(0x40, 1)), None);
        cache.insert(0x1000, ring0, page(0x40, 1), Some(block(3)));
        assert_eq!(hit(&cache, 0x1000, page(0x40, 1)), Some(3));
        assert_eq!(hit(&cache, 0x2000, page(0x40, 1)), None, "RIP");
        assert_eq!(hit(&cache, 0x1000, page(0x41, 1)), None, "offset");
        assert_eq!(hit(&cache, 0x1000, page(0x40, 2)), None, "generation");
        assert!(matches!(
            cache.lookup(0x1000, context(PrivilegeLevel::Ring3), page(0x40, 1)),
            BlockLookup::Miss
        ));
        cache.flush();
        assert_eq!(hit(&cache, 0x1000, page(0x40, 1)), None, "epoch");
        cache.insert(0x1000, ring0, page(0x40, 1), None);
        assert_eq!(hit(&cache, 0x1000, page(0x40, 1)), Some(0), "no block here");
        let stats = cache.stats();
        assert_eq!(
            (stats.hits, stats.empty_hits, stats.stale, stats.flushes),
            (1, 1, 1, 1),
            "{stats:?}"
        );
        assert_eq!((stats.built_blocks, stats.built_instructions), (1, 3));
    }

    #[test]
    fn reset_and_another_memory_empty_the_table() {
        let cache = DecodedBlockCache::new(16);
        let ring0 = context(PrivilegeLevel::Ring0);
        cache.bind_memory(0x1000);
        cache.insert(0x10, ring0, page(0x10, 1), Some(block(1)));
        cache.bind_memory(0x1000);
        assert_eq!(hit(&cache, 0x10, page(0x10, 1)), Some(1));
        cache.bind_memory(0x2000);
        assert_eq!(hit(&cache, 0x10, page(0x10, 1)), None);
        cache.insert(0x10, ring0, page(0x10, 1), Some(block(1)));
        cache.reset();
        assert_eq!(hit(&cache, 0x10, page(0x10, 1)), None);
        assert_eq!(cache.stats().resets, 2);
    }

    /// PERF-33 (E2): a cache that does not count (a release build without `--block-stats`) keeps
    /// every counter at zero through every operation that counts, and behaves the same; turned on,
    /// it counts from then on.
    #[test]
    fn a_cache_that_does_not_count_writes_no_counter() {
        let mut cache = DecodedBlockCache::new(16);
        cache.disable_stats_for_test();
        assert!(!cache.counted());
        let ring0 = context(PrivilegeLevel::Ring0);
        let exercise = |cache: &DecodedBlockCache| {
            cache.bind_memory(0x1000);
            assert_eq!(hit(cache, 0x10, page(0x10, 1)), None);
            cache.insert(0x10, ring0, page(0x10, 1), Some(block(2)));
            assert_eq!(hit(cache, 0x10, page(0x10, 1)), Some(2));
            assert_eq!(hit(cache, 0x10, page(0x10, 2)), None, "stale");
            cache.insert(0x20, ring0, page(0x20, 1), None);
            assert_eq!(hit(cache, 0x20, page(0x20, 1)), Some(0));
            cache.note_retired(0);
            cache.note_retired(1);
            cache.note_holds_failed();
            cache.flush();
            cache.reset();
        };
        exercise(&cache);
        assert_eq!(cache.stats(), DecodedBlockCacheStats::default());
        assert_eq!(cache.run.get(), 0, "no run is kept either");
        cache.enable_stats();
        exercise(&cache);
        let stats = cache.stats();
        assert_eq!(
            (
                stats.lookups,
                stats.hits,
                stats.empty_hits,
                stats.misses,
                stats.stale
            ),
            (4, 1, 1, 2, 1),
            "{stats:?}"
        );
        assert_eq!((stats.built_blocks, stats.built_instructions), (1, 2));
        assert_eq!((stats.runs, stats.retired, stats.holds_failed), (1, 2, 1));
        assert_eq!((stats.flushes, stats.resets), (1, 1));
    }

    /// PERF-33 (E2, step 5): the memory of the table. The size of `Instruction` is written here as
    /// a constant so that a growth of the type shows; the bound of the table follows from it:
    /// every slot full with a block of `MAX_BLOCK_INSTRUCTIONS`, and it stays under 64 MiB.
    #[test]
    fn the_memory_of_the_table_is_bounded() {
        use od_isa::long_decode::Instruction;
        use od_isa::predecoded_block::MAX_BLOCK_INSTRUCTIONS;
        const INSTRUCTION_BYTES: usize = 72;
        const SLOT_BYTES: usize = 64;
        // 2^13 × 64 + 2^13 × (16 + 64 × 72).
        const BOUND_BYTES: usize = 38_404_096;
        let instruction = std::mem::size_of::<Instruction>();
        let slot = std::mem::size_of::<Option<Entry>>();
        // An `Arc<[T]>` allocation: the two counts, then the elements.
        let block_header = 2 * std::mem::size_of::<usize>();
        let slots = DEFAULT_DECODED_BLOCK_SLOTS;
        let table = slots * slot;
        let blocks = slots * (block_header + MAX_BLOCK_INSTRUCTIONS * instruction);
        println!(
            "size_of::<Instruction>() = {instruction}, size_of::<Option<Entry>>() = {slot}, \
             table = {table} bytes, blocks at most = {blocks} bytes, bound = {} bytes",
            table + blocks
        );
        assert_eq!(instruction, INSTRUCTION_BYTES);
        assert_eq!(slot, SLOT_BYTES);
        assert_eq!(table + blocks, BOUND_BYTES);
        assert!(table + blocks <= 64 << 20);
    }

    #[test]
    fn runs_are_counted_by_the_instructions_they_retired() {
        let cache = DecodedBlockCache::new(16);
        // Runs of 1, 3 and 40 instructions; the last one is still open when the counters are read.
        for length in [1usize, 3, 40] {
            for index in 0..length {
                cache.note_retired(index);
            }
        }
        let stats = cache.stats();
        assert_eq!((stats.runs, stats.retired), (3, 44));
        assert_eq!(stats.retired_in_runs_of_two_or_more, 43);
        assert_eq!(stats.run_lengths, [1, 0, 1, 0, 0, 0, 1]);
        // Reading the counters closed the open run: nothing more is added by a second read.
        assert_eq!(cache.stats(), stats);
    }
}
