//! S4-13b: observation of the fetch translation cache and of the page walks on the real Linux
//! path. Measurement only: nothing here is read by the loop, the CPU or a device, so a run with
//! the report retires the same instructions as the run without it, and the default stop record
//! carries no trace of it.
//!
//! What is counted:
//! - [`LinuxWalkCounts`]: the tracked page walks of the instruction adapter of the CPL0 set loop,
//!   by what the walk was for: an instruction-fetch byte that was not served by the cache
//!   (`fetch`), a data read (`data_read`, including the descriptor-table pseudo-descriptor read)
//!   and a data write (`data_write`). One walk is one call of the tracked walker, successful or
//!   not. Not counted: the read-only peeks of the loop (CPUID, `IRETQ`, `INVLPG`, the checkpoint
//!   recorder) and the second read-only walk that fills the cache after a miss. Exception
//!   delivery (`#PF`, `#GP`: IDT, GDT, TSS and frame accesses) IS counted as `data_read` and
//!   `data_write`, because the instruction adapter performs those accesses; external interrupt
//!   delivery is not (it runs on an adapter without walk counts). The cache counters
//!   ([`FetchCacheStats`]) do include the lookups of the peeks.
//! - [`FetchCacheStats`] of every call of the set loop, summed. The board builds one cache per
//!   call (per quantum), so every call starts cold.
//!
//! The phase rule: phase 1 is everything before the boundary at which checkpoint C5 (the first
//! fetch at `e_entry`, `startup_64`) is recorded, phase 2 everything from that boundary on. The
//! split is taken at the instruction boundary inside the call that records C5 (not at a call
//! boundary), so it is exact to the instruction. It needs the checkpoint recorder: without it
//! nothing is split and everything stays in phase 1 (`c5_reached` is false and `phase_rule`
//! says so). The peeks of the C5 boundary itself belong to phase 1.

use std::cell::Cell;
use std::ops::{AddAssign, Sub};

pub use crate::fetch_translation_cache::FetchCacheStats;

/// `phase_rule` of a report built with the checkpoint recorder.
pub const FETCH_CACHE_PHASE_RULE_C5: &str = "c5_first_fetch_at_e_entry";
/// `phase_rule` of a report built without the checkpoint recorder: no split.
pub const FETCH_CACHE_PHASE_RULE_NONE: &str = "none_checkpoints_disabled";

/// Tracked page walks of one stretch of the run, by purpose (see the module doc for what is not
/// counted).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxWalkCounts {
    /// Instruction-fetch bytes that took a tracked walk.
    pub fetch: u64,
    /// Data reads.
    pub data_read: u64,
    /// Data writes.
    pub data_write: u64,
}

impl AddAssign for LinuxWalkCounts {
    fn add_assign(&mut self, other: Self) {
        self.fetch += other.fetch;
        self.data_read += other.data_read;
        self.data_write += other.data_write;
    }
}

impl Sub for LinuxWalkCounts {
    type Output = Self;

    /// The counters only grow, so `self - earlier` is what happened in between.
    fn sub(self, earlier: Self) -> Self {
        Self {
            fetch: self.fetch.saturating_sub(earlier.fetch),
            data_read: self.data_read.saturating_sub(earlier.data_read),
            data_write: self.data_write.saturating_sub(earlier.data_write),
        }
    }
}

impl AddAssign for FetchCacheStats {
    /// Every counter is summed (hits, misses, inserts and every kind of flush).
    fn add_assign(&mut self, other: Self) {
        self.hits += other.hits;
        self.misses += other.misses;
        self.inserts += other.inserts;
        self.write_flushes += other.write_flushes;
        self.context_flushes += other.context_flushes;
        self.invlpg_flushes += other.invlpg_flushes;
        self.control_flushes += other.control_flushes;
        self.fault_flushes += other.fault_flushes;
        self.ignored_table_writes += other.ignored_table_writes;
        self.host_flushes += other.host_flushes;
    }
}

impl Sub for FetchCacheStats {
    type Output = Self;

    /// The counters of one cache only grow, so `self - earlier` is what happened in between.
    fn sub(self, earlier: Self) -> Self {
        Self {
            hits: self.hits.saturating_sub(earlier.hits),
            misses: self.misses.saturating_sub(earlier.misses),
            inserts: self.inserts.saturating_sub(earlier.inserts),
            write_flushes: self.write_flushes.saturating_sub(earlier.write_flushes),
            context_flushes: self.context_flushes.saturating_sub(earlier.context_flushes),
            invlpg_flushes: self.invlpg_flushes.saturating_sub(earlier.invlpg_flushes),
            control_flushes: self.control_flushes.saturating_sub(earlier.control_flushes),
            fault_flushes: self.fault_flushes.saturating_sub(earlier.fault_flushes),
            ignored_table_writes: self
                .ignored_table_writes
                .saturating_sub(earlier.ignored_table_writes),
            host_flushes: self.host_flushes.saturating_sub(earlier.host_flushes),
        }
    }
}

/// The counters of the adapter of one set-loop call (shared by every per-instruction adapter of
/// the call).
#[derive(Debug, Default)]
pub(crate) struct WalkCounter {
    fetch: Cell<u64>,
    data_read: Cell<u64>,
    data_write: Cell<u64>,
}

impl WalkCounter {
    pub(crate) fn note_fetch(&self) {
        self.fetch.set(self.fetch.get() + 1);
    }

    pub(crate) fn note_read(&self) {
        self.data_read.set(self.data_read.get() + 1);
    }

    pub(crate) fn note_write(&self) {
        self.data_write.set(self.data_write.get() + 1);
    }

    pub(crate) fn snapshot(&self) -> LinuxWalkCounts {
        LinuxWalkCounts {
            fetch: self.fetch.get(),
            data_read: self.data_read.get(),
            data_write: self.data_write.get(),
        }
    }
}

/// The counters of the loop at the boundary where checkpoint C5 was recorded, inside one call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct C5Split {
    /// Instructions retired in the call before the boundary.
    pub(crate) retired: u64,
    pub(crate) walks: LinuxWalkCounts,
    /// The call's cache counters at the boundary (`None` without a cache).
    pub(crate) cache: Option<FetchCacheStats>,
}

/// One stretch of the run (a phase or the whole run).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LinuxFetchCachePhase {
    /// Set-loop calls (quanta) that started in this stretch; a call that records C5 counts in
    /// phase 1.
    pub calls: u64,
    /// Instructions retired.
    pub retired: u64,
    pub walks: LinuxWalkCounts,
    /// Cache counters summed over the calls (all zero when the cache is off).
    pub cache: FetchCacheStats,
}

impl AddAssign for LinuxFetchCachePhase {
    fn add_assign(&mut self, other: Self) {
        self.calls += other.calls;
        self.retired += other.retired;
        self.walks += other.walks;
        self.cache += other.cache;
    }
}

/// The opt-in report of a run: totals, and the two phases around C5.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxFetchCacheReport {
    /// Whether the fetch cache was on (a run with `OD_NO_FETCH_CACHE` is told apart by this).
    pub cache_enabled: bool,
    /// [`FETCH_CACHE_PHASE_RULE_C5`] or [`FETCH_CACHE_PHASE_RULE_NONE`].
    pub phase_rule: &'static str,
    /// C5 was recorded during the run (always false without the checkpoint recorder).
    pub c5_reached: bool,
    pub total: LinuxFetchCachePhase,
    /// Before the boundary of C5 (everything, when C5 was not reached).
    pub before_c5: LinuxFetchCachePhase,
    /// From the boundary of C5 on (zero when C5 was not reached).
    pub after_c5: LinuxFetchCachePhase,
}

/// What one call of the set loop shows the accumulator.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CallObservation {
    pub(crate) retired: u64,
    pub(crate) walks: LinuxWalkCounts,
    pub(crate) cache: Option<FetchCacheStats>,
    /// C5 was already recorded when the call started.
    pub(crate) c5_at_start: bool,
    /// C5 was recorded during the call, at this boundary.
    pub(crate) c5_split: Option<C5Split>,
}

/// Sums the calls of one run into a [`LinuxFetchCacheReport`].
#[derive(Debug)]
pub(crate) struct FetchCacheAccumulator {
    cache_enabled: bool,
    checkpoints_enabled: bool,
    c5_reached: bool,
    total: LinuxFetchCachePhase,
    before: LinuxFetchCachePhase,
    after: LinuxFetchCachePhase,
}

impl FetchCacheAccumulator {
    pub(crate) fn new(cache_enabled: bool, checkpoints_enabled: bool) -> Self {
        Self {
            cache_enabled,
            checkpoints_enabled,
            c5_reached: false,
            total: LinuxFetchCachePhase::default(),
            before: LinuxFetchCachePhase::default(),
            after: LinuxFetchCachePhase::default(),
        }
    }

    pub(crate) fn add_call(&mut self, call: &CallObservation) {
        let whole = LinuxFetchCachePhase {
            calls: 1,
            retired: call.retired,
            walks: call.walks,
            cache: call.cache.unwrap_or_default(),
        };
        self.total += whole;
        if call.c5_at_start {
            self.c5_reached = true;
            self.after += whole;
        } else if let Some(split) = &call.c5_split {
            self.c5_reached = true;
            let before = LinuxFetchCachePhase {
                calls: 1,
                retired: split.retired,
                walks: split.walks,
                cache: split.cache.unwrap_or_default(),
            };
            self.before += before;
            self.after += LinuxFetchCachePhase {
                calls: 0,
                retired: whole.retired.saturating_sub(before.retired),
                walks: whole.walks - before.walks,
                cache: whole.cache - before.cache,
            };
        } else {
            self.before += whole;
        }
    }

    pub(crate) fn finish(self) -> LinuxFetchCacheReport {
        LinuxFetchCacheReport {
            cache_enabled: self.cache_enabled,
            phase_rule: if self.checkpoints_enabled {
                FETCH_CACHE_PHASE_RULE_C5
            } else {
                FETCH_CACHE_PHASE_RULE_NONE
            },
            c5_reached: self.c5_reached,
            total: self.total,
            before_c5: self.before,
            after_c5: self.after,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(seed: u64) -> FetchCacheStats {
        FetchCacheStats {
            hits: seed,
            misses: seed + 1,
            inserts: seed + 2,
            write_flushes: seed + 3,
            context_flushes: seed + 4,
            invlpg_flushes: seed + 5,
            control_flushes: seed + 6,
            fault_flushes: seed + 7,
            ignored_table_writes: seed + 8,
            host_flushes: seed + 9,
        }
    }

    fn walks(fetch: u64, read: u64, write: u64) -> LinuxWalkCounts {
        LinuxWalkCounts {
            fetch,
            data_read: read,
            data_write: write,
        }
    }

    fn call(
        retired: u64,
        walks: LinuxWalkCounts,
        cache: Option<FetchCacheStats>,
    ) -> CallObservation {
        CallObservation {
            retired,
            walks,
            cache,
            c5_at_start: false,
            c5_split: None,
        }
    }

    #[test]
    fn stats_of_several_calls_are_summed_field_by_field() {
        let mut sum = FetchCacheStats::default();
        sum += stats(10);
        sum += stats(100);
        sum += stats(1000);
        // hits, misses and the flushes all add up.
        assert_eq!(sum.hits, 10 + 100 + 1000);
        assert_eq!(sum.misses, 11 + 101 + 1001);
        assert_eq!(sum.inserts, 12 + 102 + 1002);
        assert_eq!(sum.write_flushes, 13 + 103 + 1003);
        assert_eq!(sum.context_flushes, 14 + 104 + 1004);
        assert_eq!(sum.invlpg_flushes, 15 + 105 + 1005);
        assert_eq!(sum.control_flushes, 16 + 106 + 1006);
        assert_eq!(sum.fault_flushes, 17 + 107 + 1007);
        assert_eq!(sum.ignored_table_writes, 18 + 108 + 1008);
        assert_eq!(sum.host_flushes, 19 + 109 + 1009);
        assert_eq!(sum - stats(1000) - stats(100), stats(10));
    }

    #[test]
    fn accumulator_sums_calls_and_keeps_everything_in_phase_one_without_c5() {
        let mut acc = FetchCacheAccumulator::new(true, false);
        acc.add_call(&call(100, walks(5, 6, 7), Some(stats(1))));
        acc.add_call(&call(50, walks(1, 2, 3), Some(stats(2))));
        let report = acc.finish();
        assert!(report.cache_enabled);
        assert_eq!(report.phase_rule, FETCH_CACHE_PHASE_RULE_NONE);
        assert!(!report.c5_reached);
        assert_eq!(report.total.calls, 2);
        assert_eq!(report.total.retired, 150);
        assert_eq!(report.total.walks, walks(6, 8, 10));
        let mut expected = stats(1);
        expected += stats(2);
        assert_eq!(report.total.cache, expected);
        assert_eq!(report.before_c5, report.total);
        assert_eq!(report.after_c5, LinuxFetchCachePhase::default());
    }

    #[test]
    fn accumulator_splits_the_call_that_records_c5_and_puts_later_calls_after_it() {
        let mut acc = FetchCacheAccumulator::new(true, true);
        acc.add_call(&call(100, walks(10, 20, 30), Some(stats(1))));
        // The call that records C5 after 40 of its 90 instructions.
        let mut straddling = call(90, walks(9, 8, 7), Some(stats(100)));
        straddling.c5_split = Some(C5Split {
            retired: 40,
            walks: walks(4, 3, 2),
            cache: Some(stats(40)),
        });
        acc.add_call(&straddling);
        let mut later = call(60, walks(1, 1, 1), Some(stats(5)));
        later.c5_at_start = true;
        acc.add_call(&later);
        let report = acc.finish();
        assert_eq!(report.phase_rule, FETCH_CACHE_PHASE_RULE_C5);
        assert!(report.c5_reached);
        assert_eq!(report.total.calls, 3);
        assert_eq!(report.total.retired, 250);
        assert_eq!(report.before_c5.calls, 2);
        assert_eq!(report.before_c5.retired, 140);
        assert_eq!(report.before_c5.walks, walks(14, 23, 32));
        let mut before_cache = stats(1);
        before_cache += stats(40);
        assert_eq!(report.before_c5.cache, before_cache);
        assert_eq!(report.after_c5.calls, 1);
        assert_eq!(report.after_c5.retired, 50 + 60);
        assert_eq!(report.after_c5.walks, walks(5 + 1, 5 + 1, 5 + 1));
        let mut after_cache = stats(100) - stats(40);
        after_cache += stats(5);
        assert_eq!(report.after_c5.cache, after_cache);
        // The two phases add up to the total, field by field.
        let mut both = report.before_c5;
        both += report.after_c5;
        assert_eq!(both, report.total);
    }

    #[test]
    fn a_run_without_a_cache_reports_zero_cache_counters_and_its_walks() {
        let mut acc = FetchCacheAccumulator::new(false, true);
        acc.add_call(&call(10, walks(70, 3, 2), None));
        let report = acc.finish();
        assert!(!report.cache_enabled);
        assert_eq!(report.total.cache, FetchCacheStats::default());
        assert_eq!(report.total.walks.fetch, 70);
        assert!(!report.c5_reached);
    }

    #[test]
    fn walk_counter_counts_each_kind_separately() {
        let counter = WalkCounter::default();
        counter.note_fetch();
        counter.note_fetch();
        counter.note_read();
        counter.note_write();
        counter.note_write();
        counter.note_write();
        assert_eq!(counter.snapshot(), walks(2, 1, 3));
    }
}
