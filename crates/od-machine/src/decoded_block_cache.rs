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
//!
//! PERF-33 (E5): the tests drop each source of the invalidation on its own ([`BlockMutations`],
//! test builds only) to prove it is needed; `--block-cache-verify` ([`DecodedBlockCache::verify`])
//! checks every decode of a block against a fresh decode of the general step.

use crate::op_histogram::{OpHistogram, OpHistogramEntry};
use crate::physical_memory::RamCodePage;
use od_isa::long_decode::{DecodedBlock, Instruction, Operation, Prefixes};
use od_isa::memory::AccessContext;
use od_isa::predecoded_block::BlockEnd;
use std::cell::{Cell, RefCell, RefMut};

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

/// PERF-33 (E4, step 2): why a run of a block ended, the keys of [`BlockEndReport::end_reasons`]
/// (the runner's `unified.block_stats.end_reasons`). The run got to the end of its block:
/// `branch` (the block ends with a Jcc/JMP/CALL/RET, taken or not: the norm),
/// `outside_fast_path` (the block ends before an instruction `fast_block_role` does not admit),
/// `branch_prefix_filtered` (before a Jcc/JMP/CALL/RET with a LOCK/REP/REPNE or FS/GS prefix, which
/// the filter does not admit), `page_end`, `page_crossing` (before an instruction that needs a byte
/// of the next page), `length_limit` (`MAX_BLOCK_INSTRUCTIONS`), `decode_error`. The run was cut
/// inside its block: `refused` (the fast path refused an instruction the filter admitted),
/// `branch_prefix_refused` (the refused one is a Jcc/JMP/CALL/RET with other non-default prefixes:
/// 66, a CS/DS/ES/SS segment, REX ...), `not_retired` (a fault, an error or another exit of the
/// step), `holds_failed`, `call_end` (the batched call ended inside the block: the batch boundary,
/// the budget, a stop). `unknown`: a block built without its end recorded (counting turned on after
/// the build, a test mutation).
pub const BLOCK_RUN_END_REASONS: [&str; 13] = [
    "branch",
    "outside_fast_path",
    "branch_prefix_filtered",
    "page_end",
    "page_crossing",
    "length_limit",
    "decode_error",
    "refused",
    "branch_prefix_refused",
    "not_retired",
    "holds_failed",
    "call_end",
    "unknown",
];
const END_BRANCH: usize = 0;
const END_OUTSIDE_FAST_PATH: usize = 1;
const END_BRANCH_PREFIX_FILTERED: usize = 2;
const END_PAGE_END: usize = 3;
const END_PAGE_CROSSING: usize = 4;
const END_LENGTH_LIMIT: usize = 5;
const END_DECODE_ERROR: usize = 6;
const END_REFUSED: usize = 7;
const END_BRANCH_PREFIX_REFUSED: usize = 8;
const END_NOT_RETIRED: usize = 9;
const END_HOLDS_FAILED: usize = 10;
const END_CALL_END: usize = 11;
const END_UNKNOWN: usize = 12;

/// PERF-33 (E4, step 2): why an instruction went the general step instead of a block, the
/// reasons of [`BlockEndReport::general`] (the runner's `unified.block_stats.general_by_key`):
/// `no_lookup` (the gate of the lookup did not pass: the translation is not in the fetch cache, not
/// RAM, ...), `not_block_form` (no block starts at the RIP: the instruction is outside the filter),
/// `branch_prefix_filtered` (the same for a Jcc/JMP/CALL/RET with a LOCK/REP/REPNE or FS/GS prefix),
/// `page_crossing` and `decode_error` (no block starts at the RIP for that reason), `refused` and
/// `branch_prefix_refused` (as in [`BLOCK_RUN_END_REASONS`]), `single_step` (a one-instruction
/// call of the set loop, which has no blocks: CPL3, RF, the fetch cache shadows).
pub const BLOCK_GENERAL_REASONS: [&str; 8] = [
    "no_lookup",
    "not_block_form",
    "branch_prefix_filtered",
    "page_crossing",
    "decode_error",
    "refused",
    "branch_prefix_refused",
    "single_step",
];
const GENERAL_NO_LOOKUP: usize = 0;
const GENERAL_NOT_BLOCK_FORM: usize = 1;
const GENERAL_BRANCH_PREFIX_FILTERED: usize = 2;
const GENERAL_PAGE_CROSSING: usize = 3;
const GENERAL_DECODE_ERROR: usize = 4;
const GENERAL_REFUSED: usize = 5;
const GENERAL_BRANCH_PREFIX_REFUSED: usize = 6;
const GENERAL_SINGLE_STEP: usize = 7;

/// PERF-33 (E4, step 2): what the cache counted of the ends of the runs and of the general step
/// (only while it counts, see [`DecodedBlockCache::end_report`]).
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlockEndReport {
    /// Runs by why they ended, indexed as [`BLOCK_RUN_END_REASONS`]; the sum is
    /// [`DecodedBlockCacheStats::runs`].
    pub end_reasons: [u64; BLOCK_RUN_END_REASONS.len()],
    /// The runs that ended with `outside_fast_path` or `branch_prefix_filtered`, by the key of
    /// PERF-20 (`Operation` variant, operand kind, operand size, CPL) of the instruction the block
    /// ended before; by descending count.
    pub end_outside_fast_path: Vec<OpHistogramEntry>,
    /// The instructions the general step retired, by reason, indexed as [`BLOCK_GENERAL_REASONS`].
    pub general_by_reason: [u64; BLOCK_GENERAL_REASONS.len()],
    /// The same by reason and the key of PERF-20 (`<undecoded>` when the read-only decode failed);
    /// by descending count.
    pub general: Vec<(&'static str, OpHistogramEntry)>,
    /// Their sum: with [`DecodedBlockCacheStats::retired`] the instructions the counted runs
    /// retired.
    pub general_retired: u64,
}

/// The block of the open run, for the reason its end gets.
#[derive(Clone, Copy)]
struct CurrentBlock {
    len: usize,
    /// `None`: not recorded at the build.
    end: Option<BlockEnd>,
    cpl: u8,
}

/// PERF-33 (E4, step 2): the counters of the ends and of the general step (allocated on the first
/// use while the cache counts).
#[derive(Default)]
struct EndDetail {
    /// Why the block (or the empty entry) of each slot ended, written with the slot.
    ends: Vec<Option<BlockEnd>>,
    current: Option<CurrentBlock>,
    /// The index of the last instruction the open run retired.
    last_index: usize,
    /// Why the open run was cut, when it was.
    cut: Option<usize>,
    /// Why the next instruction of the general step goes there.
    general_reason: usize,
    end_counts: [u64; BLOCK_RUN_END_REASONS.len()],
    end_keys: Option<Box<OpHistogram>>,
    general_counts: [u64; BLOCK_GENERAL_REASONS.len()],
    general_keys: [Option<Box<OpHistogram>>; BLOCK_GENERAL_REASONS.len()],
}

fn is_branch(operation: &Operation) -> bool {
    matches!(
        operation,
        Operation::Branch { .. }
            | Operation::Jump { .. }
            | Operation::Call { .. }
            | Operation::Return { .. }
    )
}

/// The reason of a run that got to the end of a block that ended with `end`.
fn end_reason(end: &BlockEnd) -> usize {
    match end {
        BlockEnd::Branch => END_BRANCH,
        BlockEnd::OutsideFastPath { instruction, .. } if is_branch(&instruction.operation) => {
            END_BRANCH_PREFIX_FILTERED
        }
        BlockEnd::OutsideFastPath { .. } => END_OUTSIDE_FAST_PATH,
        BlockEnd::PageEnd => END_PAGE_END,
        BlockEnd::PageCrossing => END_PAGE_CROSSING,
        BlockEnd::LengthLimit => END_LENGTH_LIMIT,
        BlockEnd::DecodeError => END_DECODE_ERROR,
    }
}

/// The reason of the general step at a RIP where no block starts because of `end`.
fn general_reason(end: Option<&BlockEnd>) -> usize {
    match end {
        Some(BlockEnd::OutsideFastPath { instruction, .. })
            if is_branch(&instruction.operation) =>
        {
            GENERAL_BRANCH_PREFIX_FILTERED
        }
        Some(BlockEnd::PageCrossing) => GENERAL_PAGE_CROSSING,
        Some(BlockEnd::DecodeError) => GENERAL_DECODE_ERROR,
        _ => GENERAL_NOT_BLOCK_FORM,
    }
}

fn cpl_of(context: AccessContext) -> u8 {
    context.privilege as u8
}

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
    /// PERF-33 (E5, step 4): the runner's `--block-cache-verify`: the set loop looks blocks up and
    /// walks them as usual, but runs every instruction through the general step with a fresh decode
    /// and compares that decode with the one the block holds. Set only under `&mut self`.
    verify: bool,
    /// PERF-33 (E4, step 2): the ends of the runs and the general step, only while `counted`.
    detail: RefCell<Option<Box<EndDetail>>>,
    /// Test-only mutation switches (PERF-33, E5): each drops one source of the invalidation, for the
    /// tests that prove it is needed. A release build has none of them.
    #[cfg(test)]
    mutations: Cell<BlockMutations>,
}

/// PERF-33 (E5, step 1): the test-only mutations of the block cache, one per source of its
/// invalidation. Each one, alone, makes the cache ignore that source.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BlockMutations {
    /// (a) The page generation at the lookup: a stale entry hits.
    pub(crate) lookup_generation: bool,
    /// (b) The page generation inside an active block (`block_holds`).
    pub(crate) holds_generation: bool,
    /// (c) The count of fetch cache drops inside an active block (`block_holds`).
    pub(crate) holds_fetch_drops: bool,
    /// (d) The epoch: a translation context change of the guest flushes nothing.
    pub(crate) epoch: bool,
    /// (e) The page fence of the build (`PageFence` of `predecode_block`): an instruction that
    /// needs a byte past the page of the first RIP is taken, its bytes read from the same frame.
    pub(crate) page_fence: bool,
    /// (f) `reset` (the board reset, the snapshot restore, another memory) empties nothing.
    pub(crate) reset: bool,
}

#[cfg(test)]
impl BlockMutations {
    /// The mutation of PERF-31: every check of the page generations and the fetch cache drops.
    pub(crate) const INVALIDATION: Self = Self {
        lookup_generation: true,
        holds_generation: true,
        holds_fetch_drops: true,
        epoch: false,
        page_fence: false,
        reset: false,
    };
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
            verify: false,
            detail: RefCell::new(None),
            #[cfg(test)]
            mutations: Cell::new(BlockMutations::default()),
        }
    }

    /// PERF-33 (E4, step 2): the counters of the ends (only while `counted`).
    fn detail(&self) -> RefMut<'_, EndDetail> {
        RefMut::map(self.detail.borrow_mut(), |detail| {
            &mut **detail.get_or_insert_with(Box::default)
        })
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

    /// PERF-33 (E5, step 4): the set loop checks the blocks instead of running from them (the
    /// runner's `--block-cache-verify`, see [`Self::verify`]).
    pub(crate) fn enable_verify(&mut self) {
        self.verify = true;
    }

    /// PERF-33 (E5, step 4): whether the set loop checks the blocks (every instruction runs through
    /// the general step with a fresh decode, compared with the decode of the block).
    pub(crate) fn verify(&self) -> bool {
        self.verify
    }

    /// The counters, with the run still open counted as ended (all zero when the cache does not
    /// count).
    pub(crate) fn stats(&self) -> DecodedBlockCacheStats {
        self.close_run();
        self.counters.snapshot()
    }

    /// Empties the table (the board reset and the snapshot restore call this). The counters stay.
    pub(crate) fn reset(&self) {
        #[cfg(test)]
        let skip = self.mutations.get().reset;
        #[cfg(not(test))]
        let skip = false;
        if !skip && self.live.get() != 0 {
            self.table
                .borrow_mut()
                .iter_mut()
                .for_each(|slot| *slot = None);
            self.live.set(0);
            if self.counted {
                self.detail().ends.iter_mut().for_each(|end| *end = None);
            }
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
        #[cfg(test)]
        let skip = self.mutations.get().epoch;
        #[cfg(not(test))]
        let skip = false;
        if !skip {
            self.epoch.set(self.epoch.get() + 1);
        }
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
        if self.mutations.get().lookup_generation {
            return true;
        }
        held == now
    }

    /// Whether a further instruction of an active block may run from it: the page generation and
    /// the count of fetch cache drops (`(held, now)` each) are those of the lookup that answered it.
    #[inline(always)]
    pub(crate) fn block_holds(&self, generation: (u64, u64), fetch_drops: (u64, u64)) -> bool {
        #[cfg(test)]
        {
            let mutations = self.mutations.get();
            (mutations.holds_generation || generation.0 == generation.1)
                && (mutations.holds_fetch_drops || fetch_drops.0 == fetch_drops.1)
        }
        #[cfg(not(test))]
        {
            generation.0 == generation.1 && fetch_drops.0 == fetch_drops.1
        }
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
        let slot = self.index(page.region, page.offset);
        let found = {
            let table = self.table.borrow();
            match table.get(slot).and_then(Option::as_ref) {
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
            let mut detail = self.detail();
            let end = detail.ends.get(slot).copied().flatten();
            match &found {
                BlockLookup::Block(block) => {
                    detail.current = Some(CurrentBlock {
                        len: block.len(),
                        end,
                        cpl: cpl_of(context),
                    });
                }
                BlockLookup::Empty => detail.general_reason = general_reason(end.as_ref()),
                BlockLookup::Miss => {}
            }
        }
        found
    }

    /// Stores the block decoded from the bytes of `page` (its generation read before them), or
    /// that none starts at `rip`, replacing whatever held the slot.
    #[cfg(test)]
    pub(crate) fn insert(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
        block: Option<DecodedBlock>,
    ) {
        self.insert_with_end(rip, context, page, block, None);
    }

    /// [`Self::insert`] with why the build ended (`None`: not known), for the counters of
    /// PERF-33 (E4, step 2) only.
    pub(crate) fn insert_with_end(
        &self,
        rip: u64,
        context: AccessContext,
        page: RamCodePage,
        block: Option<DecodedBlock>,
        end: Option<BlockEnd>,
    ) {
        let slot = self.index(page.region, page.offset);
        if self.counted {
            if let Some(block) = &block {
                add(&self.counters.built_blocks, 1);
                add(&self.counters.built_instructions, block.len() as u64);
            }
            let slots = self.slots();
            let mut detail = self.detail();
            if detail.ends.is_empty() {
                detail.ends.resize(slots, None);
            }
            detail.ends[slot] = end;
            match &block {
                Some(block) => {
                    detail.current = Some(CurrentBlock {
                        len: block.len(),
                        end,
                        cpl: cpl_of(context),
                    });
                }
                None => detail.general_reason = general_reason(end.as_ref()),
            }
        }
        let mut table = self.table.borrow_mut();
        if table.is_empty() {
            table.resize(self.slots(), None);
        }
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
        self.detail().last_index = index;
    }

    /// A run ended because the block no longer held.
    #[inline(always)]
    pub(crate) fn note_holds_failed(&self) {
        if self.counted {
            add(&self.counters.holds_failed, 1);
            if self.run.get() > 0 {
                self.detail().cut = Some(END_HOLDS_FAILED);
            }
        }
    }

    /// PERF-33 (E4, step 2): the set loop asks for a block (the run of the last one, if any, has
    /// ended); until a lookup says otherwise, a general step goes there for `no_lookup`.
    pub(crate) fn note_lookup_asked(&self) {
        if !self.counted {
            return;
        }
        self.close_run();
        let mut detail = self.detail();
        detail.current = None;
        detail.general_reason = GENERAL_NO_LOOKUP;
    }

    /// PERF-33 (E4, step 2): `instruction` of the active block did not retire from it: the fast
    /// path refused it (`refused`) or its step ended otherwise.
    pub(crate) fn note_run_cut(&self, instruction: &Instruction, refused: bool) {
        if !self.counted {
            return;
        }
        let mut detail = self.detail();
        let end = if refused {
            let prefixed_branch =
                is_branch(&instruction.operation) && instruction.prefixes != Prefixes::default();
            detail.general_reason = if prefixed_branch {
                GENERAL_BRANCH_PREFIX_REFUSED
            } else {
                GENERAL_REFUSED
            };
            if prefixed_branch {
                END_BRANCH_PREFIX_REFUSED
            } else {
                END_REFUSED
            }
        } else {
            END_NOT_RETIRED
        };
        if self.run.get() > 0 {
            detail.cut = Some(end);
        }
    }

    /// PERF-33 (E4, step 2): the general step retired `instruction` (decoded read-only before or
    /// after the step; `None`: it did not decode so) at `rip` in `cpl`, for the reason the last
    /// lookup or cut gave.
    pub(crate) fn note_general_retired(&self, rip: u64, instruction: Option<Instruction>, cpl: u8) {
        if self.counted {
            let reason = self.detail().general_reason;
            self.count_general(reason, rip, instruction, cpl, 1);
        }
    }

    /// PERF-33 (E4, step 2): a one-instruction call of the set loop (no blocks) retired `retired`
    /// instructions of `instruction` at `rip` in `cpl`.
    pub(crate) fn note_single_step_retired(
        &self,
        rip: u64,
        instruction: Option<Instruction>,
        cpl: u8,
        retired: u64,
    ) {
        if self.counted {
            self.count_general(GENERAL_SINGLE_STEP, rip, instruction, cpl, retired);
        }
    }

    fn count_general(
        &self,
        reason: usize,
        rip: u64,
        instruction: Option<Instruction>,
        cpl: u8,
        retired: u64,
    ) {
        let mut detail = self.detail();
        detail.general_counts[reason] += retired;
        detail.general_keys[reason]
            .get_or_insert_with(|| Box::new(OpHistogram::new()))
            .record(rip, instruction, cpl, retired);
    }

    /// PERF-33 (E4, step 2): the ends of the runs and the general step, with the run still open
    /// counted as ended; `None` when the cache does not count.
    pub(crate) fn end_report(&self) -> Option<BlockEndReport> {
        if !self.counted {
            return None;
        }
        self.close_run();
        let detail = self.detail();
        let mut general = Vec::new();
        for (reason, keys) in detail.general_keys.iter().enumerate() {
            if let Some(keys) = keys {
                general.extend(
                    keys.report()
                        .entries
                        .into_iter()
                        .map(|entry| (BLOCK_GENERAL_REASONS[reason], entry)),
                );
            }
        }
        general.sort_by(|a, b| {
            b.1.count.cmp(&a.1.count).then_with(|| {
                (
                    a.0,
                    &a.1.operation,
                    a.1.operands,
                    a.1.operand_bytes,
                    a.1.cpl,
                )
                    .cmp(&(
                        b.0,
                        &b.1.operation,
                        b.1.operands,
                        b.1.operand_bytes,
                        b.1.cpl,
                    ))
            })
        });
        Some(BlockEndReport {
            end_reasons: detail.end_counts,
            end_outside_fast_path: detail
                .end_keys
                .as_ref()
                .map(|keys| keys.report().entries)
                .unwrap_or_default(),
            general_by_reason: detail.general_counts,
            general,
            general_retired: detail.general_counts.iter().sum(),
        })
    }

    /// PERF-33 (E4, step 2): the reason of the run that just closed.
    fn note_run_end(&self) {
        let mut detail = self.detail();
        let reason = match (detail.cut.take(), detail.current) {
            (Some(cut), _) => cut,
            (None, Some(current)) if detail.last_index + 1 >= current.len => {
                current.end.as_ref().map_or(END_UNKNOWN, end_reason)
            }
            (None, Some(_)) => END_CALL_END,
            (None, None) => END_UNKNOWN,
        };
        detail.end_counts[reason] += 1;
        if matches!(reason, END_OUTSIDE_FAST_PATH | END_BRANCH_PREFIX_FILTERED)
            && let Some(CurrentBlock {
                end: Some(BlockEnd::OutsideFastPath { rip, instruction }),
                cpl,
                ..
            }) = detail.current
        {
            detail
                .end_keys
                .get_or_insert_with(|| Box::new(OpHistogram::new()))
                .record(rip, Some(instruction), cpl, 1);
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
        self.note_run_end();
    }

    /// Test-only (PERF-33, E5): the cache drops the sources of its invalidation `mutations` names.
    #[cfg(test)]
    pub(crate) fn mutate_for_test(&self, mutations: BlockMutations) {
        self.mutations.set(mutations);
    }

    /// Test-only (PERF-33, E5): whether the build ignores the page fence ([`BlockMutations::page_fence`]).
    #[cfg(test)]
    pub(crate) fn page_fence_mutated(&self) -> bool {
        self.mutations.get().page_fence
    }
}

/// Test-only (PERF-33, E5, mutation (e)): `od_isa::predecoded_block::predecode_block` without its
/// page fence: the same loop (the end of the page of `rip`, `MAX_BLOCK_INSTRUCTIONS`, the role of
/// each instruction), but a fetch past the page is not refused, so an instruction that needs a byte
/// of the next page is taken with whatever `code` answers for it.
#[cfg(test)]
pub(crate) fn predecode_block_without_page_fence<M: od_isa::memory::GuestMemory>(
    code: &mut M,
    rip: u64,
    context: AccessContext,
    max_len: usize,
) -> Vec<od_isa::long_decode::Instruction> {
    use od_isa::predecoded_block::{FastBlockRole, fast_block_role};
    let page = rip >> 12;
    let mut block = Vec::new();
    let mut at = rip;
    while block.len() < max_len && at >> 12 == page {
        let Ok(instruction) = od_isa::long_decode::decode(&mut *code, at, context) else {
            break;
        };
        let Some(role) = fast_block_role(&instruction) else {
            break;
        };
        block.push(instruction);
        at = at.wrapping_add(u64::from(instruction.length));
        if role == FastBlockRole::Terminator {
            break;
        }
    }
    block
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

    /// PERF-33 (E4, step 2): every run gets one reason for its end (their sum is `runs`), every
    /// general step one reason and the PERF-20 key of its instruction; a cache that does not count
    /// keeps none of it.
    #[test]
    fn the_ends_of_the_runs_and_the_general_steps_are_counted_by_reason() {
        let cache = DecodedBlockCache::new(1 << 10);
        let ring0 = context(PrivilegeLevel::Ring0);
        let out = decoded(&[0xEE]).unwrap(); // OUT DX, AL
        let mov = decoded(&[0x89, 0xC0]).unwrap(); // MOV EAX, EAX
        let prefixed_jnz = decoded(&[0x2E, 0x75, 0x00]).unwrap(); // CS JNZ +0
        let before_out = Some(BlockEnd::OutsideFastPath {
            rip: 0x1006,
            instruction: out,
        });
        let reason = |report: &BlockEndReport, name: &str| {
            let index = BLOCK_RUN_END_REASONS
                .iter()
                .position(|n| *n == name)
                .unwrap();
            report.end_reasons[index]
        };
        let general = |report: &BlockEndReport, name: &str| {
            let index = BLOCK_GENERAL_REASONS
                .iter()
                .position(|n| *n == name)
                .unwrap();
            report.general_by_reason[index]
        };
        // A block of three before OUT, run to its end: `outside_fast_path`.
        cache.note_lookup_asked();
        cache.insert_with_end(0x1000, ring0, page(0x100, 1), Some(block(3)), before_out);
        (0..3).for_each(|index| cache.note_retired(index));
        // The same block from a hit, cut by a refusal of its third instruction: `refused`.
        cache.note_lookup_asked();
        assert_eq!(hit(&cache, 0x1000, page(0x100, 1)), Some(3));
        (0..2).for_each(|index| cache.note_retired(index));
        cache.note_run_cut(&mov, true);
        cache.note_general_retired(0x1004, Some(mov), 0);
        // OUT at 0x1006 starts no block (built, then a hit): `not_block_form` twice.
        cache.note_lookup_asked();
        cache.insert_with_end(0x1006, ring0, page(0x106, 1), None, before_out);
        cache.note_general_retired(0x1006, Some(out), 0);
        cache.note_lookup_asked();
        assert_eq!(hit(&cache, 0x1006, page(0x106, 1)), Some(0));
        cache.note_general_retired(0x1006, Some(out), 0);
        // A block of two ending with a branch, run to its end: `branch`; then cut by the end of
        // the call, then by a block that no longer holds, then by a refused prefixed Jcc.
        let ends_with_branch = |cache: &DecodedBlockCache| {
            cache.note_lookup_asked();
            cache.insert_with_end(
                0x3000,
                ring0,
                page(0x300, 1),
                Some(block(2)),
                Some(BlockEnd::Branch),
            );
            cache.note_retired(0);
        };
        ends_with_branch(&cache);
        cache.note_retired(1);
        ends_with_branch(&cache);
        // The gate of the next lookup does not pass: `no_lookup`, the instruction undecoded.
        cache.note_lookup_asked();
        cache.note_general_retired(0x5000, None, 0);
        ends_with_branch(&cache);
        cache.note_holds_failed();
        ends_with_branch(&cache);
        cache.note_run_cut(&prefixed_jnz, true);
        cache.note_general_retired(0x3002, Some(prefixed_jnz), 0);
        // A fault inside a block: `not_retired`.
        ends_with_branch(&cache);
        cache.note_run_cut(&mov, false);
        // A one-instruction call at CPL3.
        cache.note_single_step_retired(0x4000, Some(mov), 3, 1);
        let report = cache.end_report().expect("a debug build counts");
        let stats = cache.stats();
        assert_eq!(stats.runs, 7, "{stats:?}");
        assert_eq!(report.end_reasons.iter().sum::<u64>(), stats.runs);
        for (name, count) in [
            ("outside_fast_path", 1),
            ("refused", 1),
            ("branch", 1),
            ("call_end", 1),
            ("holds_failed", 1),
            ("branch_prefix_refused", 1),
            ("not_retired", 1),
            ("unknown", 0),
        ] {
            assert_eq!(reason(&report, name), count, "{name}: {report:?}");
        }
        assert_eq!(report.end_outside_fast_path.len(), 1);
        assert_eq!(report.end_outside_fast_path[0].count, 1);
        for (name, count) in [
            ("refused", 1),
            ("not_block_form", 2),
            ("no_lookup", 1),
            ("branch_prefix_refused", 1),
            ("single_step", 1),
        ] {
            assert_eq!(general(&report, name), count, "{name}: {report:?}");
        }
        assert_eq!(report.general_retired, 6);
        assert_eq!(
            report
                .general
                .iter()
                .map(|(_, entry)| entry.count)
                .sum::<u64>(),
            report.general_retired
        );
        assert_eq!(report.general[0].0, "not_block_form", "the top key first");
        assert!(
            report
                .general
                .iter()
                .any(|(reason, entry)| *reason == "single_step"
                    && entry.cpl == 3
                    && entry.operands == "reg")
        );
        assert!(
            report
                .general
                .iter()
                .any(|(reason, entry)| *reason == "no_lookup" && entry.operation == "<undecoded>")
        );
        // A cache that does not count has no report and keeps nothing.
        let mut quiet = DecodedBlockCache::new(16);
        quiet.disable_stats_for_test();
        quiet.note_lookup_asked();
        quiet.insert_with_end(0x1000, ring0, page(0x100, 1), Some(block(3)), before_out);
        quiet.note_retired(0);
        quiet.note_run_cut(&mov, true);
        quiet.note_general_retired(0x1000, Some(mov), 0);
        quiet.note_single_step_retired(0x1000, Some(mov), 0, 1);
        assert!(quiet.detail.borrow().is_none(), "nothing allocated");
        assert_eq!(quiet.end_report(), None);
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
