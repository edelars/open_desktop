//! S7-32 slice R: the opt-in journal of `MOV CR0` in the set loop (`boot-firmware --cr0-log <n>`).
//!
//! It records the instructions that changed CR0 and the failed attempts of `MOV CR0`, with the
//! registers and the counters a reader needs to tell whether a window with CR0.WP clear ran as the
//! guest meant it (ISSUE-709: the Windows kernel clears WP, writes read-only code and calls it).
//! The journal only observes: the loop records an entry after it has decided the instruction, and
//! the run is the same with and without it. A change is seen where the loop rebuilds the walker
//! snapshot after a retired change of CR0 (always, the journal or not); a failed attempt is found
//! only with the journal, by decoding the instruction of a step that faulted or was refused. The
//! turbo engine has no template for `MOV CR0` (ADR-0022), so every such instruction is a step of
//! the general path.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// What a journaled `MOV CR0` did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Cr0WriteOutcome {
    /// The instruction retired and CR0 now holds `new`.
    Changed { new: u64 },
    /// The instruction did not retire: the set raised this fault (delivered to the guest IDT, or the
    /// stop of the run without one).
    Fault { vector: u8, error_code: Option<u32> },
    /// The instruction did not retire: the set refused it (`Unsupported`, nothing changed).
    Unsupported { opcode: u8 },
}

/// One journaled instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cr0JournalEntry {
    pub rip: u64,
    pub cpl: u8,
    /// CR0 before the instruction.
    pub old: u64,
    pub outcome: Cr0WriteOutcome,
    /// RAX, RCX and RDI after the instruction (a `MOV CR0` does not change them).
    pub rax: u64,
    pub rcx: u64,
    pub rdi: u64,
    /// Instructions the run retired before this one.
    pub retired: u64,
    /// Exceptions the run delivered through the guest IDT up to this instruction.
    pub exceptions_delivered: u64,
    /// External interrupts the run delivered up to this instruction (0 without an interrupt
    /// source).
    pub interrupts_delivered: u64,
}

/// The journal: the first `limit` entries and the count of all of them. The board holds it in an
/// `Arc` (the board is `Send`); one vCPU loop writes it at a time.
#[derive(Debug)]
pub struct Cr0Journal {
    limit: usize,
    count: AtomicU64,
    entries: Mutex<Vec<Cr0JournalEntry>>,
    /// The run's retired instructions before the current quantum of the loop.
    retired_base: AtomicU64,
    /// The run's delivered exceptions before the current call of the loop.
    exceptions_base: AtomicU64,
}

impl Cr0Journal {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            count: AtomicU64::new(0),
            entries: Mutex::new(Vec::new()),
            retired_base: AtomicU64::new(0),
            exceptions_base: AtomicU64::new(0),
        }
    }

    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Every journaled instruction, also the ones past the limit.
    pub fn count(&self) -> u64 {
        self.count.load(Ordering::Relaxed)
    }

    /// The first `limit` entries, in order.
    pub fn entries(&self) -> Vec<Cr0JournalEntry> {
        self.entries.lock().expect("the journal lock").clone()
    }

    pub(crate) fn set_retired_base(&self, retired: u64) {
        self.retired_base.store(retired, Ordering::Relaxed);
    }

    pub(crate) fn set_exceptions_base(&self, exceptions: u64) {
        self.exceptions_base.store(exceptions, Ordering::Relaxed);
    }

    pub(crate) fn retired_base(&self) -> u64 {
        self.retired_base.load(Ordering::Relaxed)
    }

    pub(crate) fn exceptions_base(&self) -> u64 {
        self.exceptions_base.load(Ordering::Relaxed)
    }

    pub(crate) fn record(&self, entry: Cr0JournalEntry) {
        self.count.fetch_add(1, Ordering::Relaxed);
        let mut entries = self.entries.lock().expect("the journal lock");
        if entries.len() < self.limit {
            entries.push(entry);
        }
    }
}

/// Whether `operation` writes CR0 (`MOV CR0, r64` in any of the set's decodings).
pub(crate) fn writes_cr0(operation: &od_isa::long_decode::Operation) -> bool {
    use od_isa::long_decode::Operation;
    matches!(
        operation,
        Operation::WriteCr0 | Operation::WriteCr0Rdi | Operation::WriteControl { cr: 0, .. }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(rip: u64) -> Cr0JournalEntry {
        Cr0JournalEntry {
            rip,
            cpl: 0,
            old: 0x8001_0033,
            outcome: Cr0WriteOutcome::Changed { new: 0x8000_0033 },
            rax: 0,
            rcx: 0,
            rdi: 0,
            retired: 0,
            exceptions_delivered: 0,
            interrupts_delivered: 0,
        }
    }

    #[test]
    fn the_journal_keeps_the_first_entries_and_counts_all() {
        let journal = Cr0Journal::new(2);
        for rip in 0..5 {
            journal.record(entry(rip));
        }
        assert_eq!(journal.count(), 5);
        assert_eq!(
            journal.entries().iter().map(|e| e.rip).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(journal.limit(), 2);
    }
}
