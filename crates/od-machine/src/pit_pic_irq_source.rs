//! The interrupt source of the PIT channel 0 line through the 8259 master (S4-39g): PIT OUT0 ->
//! master IR0 -> `INTR` -> the guest IDT vector `ICW2 + 0` of the CPL0 set loop.
//!
//! This is the PIC-mode counterpart of [`crate::pit_ioapic_irq_source`] (S4-38L8c): the kernel of
//! the Linux board runs with `APIC disabled by BIOS` and `Keep in PIC mode(8259)`, so the timer
//! reaches the CPU through the 8259, not through the IOAPIC. The OUT0 edge schedule (generations,
//! `next_out0_rising_tick`, `pit_tick_deadline_ns`, the [`EventQueue`]) is the same machinery as
//! the IOAPIC source (copied, not shared: the IOAPIC source and its tests stay as they were); only
//! the sink differs: an edge is `Pic8259Handle::raise_irq(0)`.
//!
//! # Time
//!
//! The trait questions carry no time. The source shares the board clock and catches up with it
//! before it answers (`catch_up`): it observes channel 0 of the PIT (a changed mode or count in
//! progress starts a new generation) and applies every OUT0 edge due at the clock of that moment.
//! A catch-up for the same clock value is skipped (fast path: the clock does not move inside a
//! quantum, and a guest write to a PIT port ends the quantum, so the next question comes with a
//! later clock; this holds for the board loop, which asks `next_deadline_ns` before each quantum).
//!
//! # PIC rules
//!
//! An edge sets the IRR bit of IR0 whether or not IR0 is masked (the chip latches it; unmasking
//! delivers it; edges coalesce). `pending_vector` is the `INTR` output of the master chip: the
//! highest-priority unmasked request not blocked by an in-service line, vector `ICW2 + line`
//! (`None` before the chip is initialised). [`BoardIrqSource::acknowledge`] is the INTA cycle (IRR
//! -> ISR; with AEOI no ISR). EOI is the guest's own OCW2 write to port `0x20` (the chip is a bus
//! device); until it, the line is blocked.
//!
//! # Deadline
//!
//! `next_deadline_ns` is the time of the next OUT0 edge not applied yet (after the clock: every
//! due edge was applied before the answer). A request that sits in IRR but cannot be delivered
//! (IMR, ISR, IF = 0 of the guest) does not shorten the quantum. [`BoardIrqSource::wake_ns`] is
//! the time of the next moment a halted CPU can be woken: the next edge when no request is
//! deliverable now.
//!
//! The semantics of the 8259 are the memory facts listed in `pic_8259.rs` and the contract section
//! of S4-39g.

use crate::irq_source::BoardIrqSource;
use crate::stage4_pit_irq0_probe::{next_out0_rising_tick, pit_tick_deadline_ns};
use crate::{
    EventQueue, Pic8259Handle, Pit8254Handle, PitMode, PitRun, VirtualClockHandle, pit_ticks,
};
use std::sync::{Arc, Mutex, MutexGuard};

/// Capacity of the queue: one chain of events per generation holds one event.
const QUEUE_CAPACITY: usize = 4;

/// What the source counted (evidence).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PitPicStats {
    /// OUT0 rising edges presented to IR0 (a masked one included).
    pub(crate) edges: u64,
    /// Events scheduled.
    pub(crate) scheduled: u64,
    /// Generation changes (reprogram, shutdown, reset).
    pub(crate) generation_changes: u64,
    /// Pending events dropped by those changes.
    pub(crate) cancelled: u64,
    /// INTA cycles the chip accepted.
    pub(crate) acknowledged: u64,
    /// `acknowledge` calls the chip refused (not the pending vector); never expected.
    pub(crate) ack_refused: u64,
    /// Event queue errors; never expected. The source stops scheduling after the first one.
    pub(crate) queue_faults: u64,
}

/// What the source compares to detect a reprogram of channel 0.
type ChannelState = (Option<PitMode>, Option<PitRun>);

struct Inner {
    pit: Pit8254Handle,
    pic: Pic8259Handle,
    clock: VirtualClockHandle,
    queue: EventQueue,
    seen: ChannelState,
    /// The clock at the last catch-up: the time of the PIT write that the next catch-up finds.
    last_sync_ns: u64,
    /// No catch-up has run yet (the fast path needs one).
    synced: bool,
    stats: PitPicStats,
}

impl Inner {
    fn schedule_after(&mut self, after_tick: u64) {
        let channel = self.pit.snapshot().channel0;
        let Some(tick) = next_out0_rising_tick(&channel, after_tick) else {
            return;
        };
        // The key is the tick of the edge: unique within a generation.
        match self
            .queue
            .schedule(self.queue.generation(), tick, pit_tick_deadline_ns(tick))
        {
            Ok(()) => self.stats.scheduled += 1,
            Err(_) => self.stats.queue_faults += 1,
        }
    }

    /// Detect a reprogram of channel 0 since the last catch-up and start a new generation.
    fn sync_generation(&mut self) {
        let channel = self.pit.snapshot().channel0;
        let state = (channel.mode, channel.run);
        if state == self.seen {
            return;
        }
        self.seen = state;
        self.stats.generation_changes += 1;
        self.stats.cancelled += self.queue.len() as u64;
        if self.queue.reset(self.queue.generation()).is_err() {
            self.stats.queue_faults += 1;
            return;
        }
        self.schedule_after(pit_ticks(self.last_sync_ns));
    }

    /// Apply every edge that is due at `now`, in order.
    fn deliver_due(&mut self, now: u64) {
        loop {
            let generation = self.queue.generation();
            let due = match self.queue.peek_due(generation, now) {
                Ok(Some(due)) => due,
                Ok(None) => break,
                Err(_) => {
                    self.stats.queue_faults += 1;
                    break;
                }
            };
            self.stats.edges += 1;
            self.pic.raise_irq(0);
            if self.queue.remove_due(generation, now).is_err() {
                self.stats.queue_faults += 1;
                break;
            }
            if self.queue.is_empty() {
                // The key of the event is the tick of the edge.
                self.schedule_after(due.key);
            }
        }
    }

    fn catch_up(&mut self) {
        if self.stats.queue_faults != 0 {
            return;
        }
        let now = self.clock.snapshot().now_ns();
        if self.synced && now == self.last_sync_ns {
            return;
        }
        self.sync_generation();
        if self.stats.queue_faults != 0 {
            // The old generation could not be dropped: its edges must not be applied.
            return;
        }
        self.deliver_due(now);
        self.last_sync_ns = now;
        self.synced = true;
    }

    /// The time of the next edge not applied yet, after a catch-up; `None` after a queue fault.
    fn next_edge_ns(&mut self) -> Option<u64> {
        self.catch_up();
        if self.stats.queue_faults != 0 {
            return None;
        }
        let now = self.clock.snapshot().now_ns();
        self.queue
            .next_deadline()
            .filter(|deadline| *deadline > now)
    }
}

/// The source of the PIT channel 0 line through the 8259 master of the board.
///
/// Cloning gives a second handle on the same source (the board owns one, a test keeps another to
/// read the counters).
#[derive(Clone)]
pub(crate) struct PitPicIrqSource(Arc<Mutex<Inner>>);

impl PitPicIrqSource {
    /// The source state. Its lock is taken before every device lock it reaches (PIT, PIC, APIC,
    /// clock; MACHINE_CONTRACT.md, S6-33 lock order) and no device ever takes it.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A source on the handles of one board; `clock` is the clock of the PIT.
    pub(crate) fn new(pit: Pit8254Handle, pic: Pic8259Handle, clock: VirtualClockHandle) -> Self {
        let now = clock.snapshot().now_ns();
        Self(Arc::new(Mutex::new(Inner {
            pit,
            pic,
            clock,
            queue: EventQueue::new(QUEUE_CAPACITY),
            seen: (None, None),
            last_sync_ns: now,
            synced: false,
            stats: PitPicStats::default(),
        })))
    }

    /// Counters for evidence.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stats(&self) -> PitPicStats {
        self.inner().stats
    }

    /// Events in the queue, as of the last catch-up; tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn queued(&self) -> usize {
        self.inner().queue.len()
    }
}

impl BoardIrqSource for PitPicIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        let mut inner = self.inner();
        inner.catch_up();
        inner.pic.pending_vector()
    }

    fn acknowledge(&mut self, vector: u8) {
        let mut inner = self.inner();
        if inner.pic.acknowledge(vector) {
            inner.stats.acknowledged += 1;
        } else {
            inner.stats.ack_refused += 1;
        }
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        self.inner().next_edge_ns()
    }

    fn pit_write_ends_quantum(&self) -> bool {
        true
    }

    fn irq0_edges(&self) -> u64 {
        self.inner().stats.edges
    }

    /// PERF-05: inside one call of the set loop the answer depends only on the clock (does not
    /// move), the PIT, 8259 and the queue state (a guest port access, `acknowledge`); a repeated
    /// `catch_up` for the same clock value returns at once. See CPU_CORE_PERF_REPORT.md, 7.6.
    fn stable_between_device_accesses(&self) -> bool {
        true
    }

    fn wake_ns(&self) -> Option<u64> {
        let mut inner = self.inner();
        inner.catch_up();
        if inner.pic.pending_vector().is_some() {
            return Some(inner.clock.snapshot().now_ns());
        }
        // An edge wakes the CPU only when it would be deliverable: IR0 masked or in service
        // (no EOI yet) keeps a halted CPU halted, as on the chip.
        if !inner.pic.would_deliver(0) {
            return None;
        }
        inner.next_edge_ns()
    }
}

#[cfg(test)]
mod tests;
