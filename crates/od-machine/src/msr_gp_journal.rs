//! S7-32 slice T: the journal of the `RDMSR`/`WRMSR` the CPL0 set answered with `#GP(0)` because the
//! virtual CPU does not have the MSR (rules R1/R2 of ADR-0013, addendum «срез T»,
//! `od_isa::msr::windows_msr_absent`).
//!
//! Such a `#GP(0)` is architectural and the guest handles it (`intelppm.sys` reads the package
//! C-state residency counters of other models inside `__try`); the journal keeps the first `limit`
//! of them with the index, the direction, RIP, CPL and the run's retired count, and the count of
//! all, so a report shows which MSRs a run was refused. It only observes: the set loop records an
//! entry after the set decided the instruction (a step that faulted with vector 13 on a decoded
//! `RDMSR`/`WRMSR` of such an index at CPL0), and the run is the same with and without it. The path
//! is cold (a fault of the general step: the turbo engine has no template for `RDMSR`/`WRMSR`).
//! It lives on the machine bus, as the log of `--device-errors log`; the single-vCPU unified run of
//! the Windows guest installs it (`run_firmware_unified_guest`).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// One journaled `RDMSR`/`WRMSR`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MsrGpEntry {
    /// ECX of the instruction.
    pub index: u32,
    /// `WRMSR` (true) or `RDMSR`.
    pub write: bool,
    pub rip: u64,
    pub cpl: u8,
    /// Instructions the run retired before this one.
    pub retired: u64,
}

/// The journal: the first `limit` entries and the count of all of them.
#[derive(Debug)]
pub struct MsrGpJournal {
    limit: usize,
    count: AtomicU64,
    entries: Mutex<Vec<MsrGpEntry>>,
    /// The run's retired instructions before the current quantum of the loop.
    retired_base: AtomicU64,
}

impl MsrGpJournal {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicU64::new(0),
            entries: Mutex::new(Vec::new()),
            retired_base: AtomicU64::new(0),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Every journaled instruction, also those past the limit.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// The first `limit` entries, in order.
    pub fn entries(&self) -> Vec<MsrGpEntry> {
        self.entries.lock().expect("the journal lock").clone()
    }

    pub(crate) fn set_retired_base(&self, retired: u64) {
        self.retired_base.store(retired, Ordering::Relaxed);
    }

    pub(crate) fn retired_base(&self) -> u64 {
        self.retired_base.load(Ordering::Relaxed)
    }

    pub(crate) fn record(&self, entry: MsrGpEntry) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock().expect("the journal lock");
        if entries.len() < self.limit {
            entries.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_journal_keeps_the_first_entries_and_counts_all() {
        let journal = MsrGpJournal::new(2);
        for index in 0..5u32 {
            journal.record(MsrGpEntry {
                index,
                write: false,
                rip: u64::from(index),
                cpl: 0,
                retired: 0,
            });
        }
        assert_eq!(journal.count(), 5);
        assert_eq!(
            journal
                .entries()
                .iter()
                .map(|entry| entry.index)
                .collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(journal.limit(), 2);
    }
}
