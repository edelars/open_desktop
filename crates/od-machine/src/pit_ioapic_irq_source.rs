//! The interrupt source of the PIT channel 0 line of the board (S4-38L8c): PIT OUT0 -> IOAPIC
//! INTIN2 (GSI 2, the interrupt source override of ISA IRQ 0) -> Local APIC IRR -> the guest IDT
//! vector of the CPL0 set loop.
//!
//! The source is the board-side form of the bridge that the flat probe `Stage4PitIrq0ProbeV1`
//! proved (S4-29f, `stage4_pit_irq0_probe`): the same rules, the same OUT0 formulas
//! (`next_out0_rising_tick`, `pit_tick_deadline_ns`), the same [`EventQueue`] with generations.
//! The Local APIC and IOAPIC models are the existing ones (`apic_pair`, `local_apic`, `io_apic`);
//! nothing of them is copied. The 8259 pair gets nothing: IR0 never rises (S4-29a).
//!
//! # How the source works without a time argument
//!
//! The trait questions carry no time. The source shares the board clock and, before it answers
//! any question, catches up with it (`catch_up`): it observes channel 0 of the PIT, and applies
//! every OUT0 edge that is due at the clock of that moment. The catch-up is idempotent for one
//! clock value and one PIT state, so the questions stay questions (`&self`); the interior
//! mutability only moves the already elapsed time into the Local APIC.
//!
//! # Generations (S4-29f)
//!
//! Every change of the guest-visible state of channel 0 (mode, count in progress: a control
//! word, a new count, the shutdown control word `0x30`, a PIT reset) is a reprogram. It starts a
//! new generation of the queue, which drops every pending event of the old one, and schedules the
//! next OUT0 rising edge of the new state. The write is taken at the time the PIT saw it (the
//! board clock does not move inside a quantum, so that is the start of the quantum of the write,
//! and the last catch-up is that time): the edges of the new count are counted from there. An
//! edge of the old generation is never applied after the reprogram. After an edge was applied
//! the next one is scheduled (only when the queue is empty, one chain of events per generation).
//!
//! # Edge, mask, IRR, ISR, EOI
//!
//! An edge is presented to the IOAPIC as a pulse on INTIN2 (`set_intin2(true)`, then `false`).
//! The redirection entry 2 (the guest wrote it through the IOAPIC window) decides, by the rules
//! of S4-29e: unmasked, the vector of the entry is requested on the Local APIC as an edge (IRR
//! bit; an edge on a set bit coalesces); masked, the edge is lost and unmasking never replays it;
//! the IRR bit that is already set stays set when the entry is masked afterwards. The Local APIC
//! decides what is deliverable (`eligible`): the highest IRR vector whose priority class is above
//! the processor priority (TPR and ISR), the Local APIC enabled by the software enable bit of
//! SVR. [`BoardIrqSource::acknowledge`] is the INTA-like step (`acknowledge_vector`: IRR -> ISR);
//! the EOI register of the Local APIC clears the highest ISR bit. Without an EOI the same vector,
//! and every vector of the same or a lower class, stays blocked.
//!
//! # Deadline
//!
//! [`BoardIrqSource::next_deadline_ns`] is the deadline of the next OUT0 edge that is not applied
//! yet, so it is after the clock: every edge that is due was applied by the catch-up before the
//! answer. A vector that was applied and cannot be accepted (the guest has IF = 0, the vector is
//! blocked by ISR or TPR) stays in IRR and does not shorten the quantum: the source never answers
//! with a time in the past; the code filters the answer (`> now`) and gives `None` after a queue
//! fault, so the guarantee does not rest on the invariant alone. A pulse that the Local APIC refuses (it is globally disabled) loses
//! the edge (counted in `edges_refused`); the probe keeps such an event, here a kept event would
//! stay in the past forever and cut every quantum to one instruction.
//!
//! # Wake of a halted CPU (S4-38L8f)
//!
//! [`BoardIrqSource::wake_ns`] is the time at which a halted CPU with IF = 1 can next be woken:
//! now when a vector is deliverable now, else the next OUT0 edge when that edge would be
//! deliverable (entry 2 unmasked and routed to this Local APIC, the Local APIC enabled, the class
//! of the vector above the processor priority), else `None`. A masked entry or a vector blocked by
//! TPR or ISR therefore never wakes the CPU (it would pass its `HLT` with no interrupt, as in
//! ISSUE-070). The Local APIC timer is not part of this answer: its deadline belongs to the clock
//! owner (S4-23j).
//!
//! The semantics of the IOAPIC and the Local APIC that the repository does not hold are SDM and
//! datasheet facts from memory (see the section of `CPU_CONTRACT.md`).

use crate::irq_source::BoardIrqSource;
use crate::stage4_pit_irq0_probe::{next_out0_rising_tick, pit_tick_deadline_ns};
use crate::{
    EventQueue, IoApicHandle, LocalApicHandle, Pit8254Handle, PitMode, PitRun, VirtualClockHandle,
    pit_ticks,
};
use std::sync::{Arc, Mutex, MutexGuard};

/// Capacity of the queue: one chain of events per generation holds one event.
const QUEUE_CAPACITY: usize = 4;

/// What the source counted (evidence).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct PitIoapicStats {
    /// OUT0 rising edges presented to INTIN2 (a masked edge and a refused one included).
    pub(crate) edges: u64,
    /// Events scheduled.
    pub(crate) scheduled: u64,
    /// Generation changes (reprogram, shutdown, reset).
    pub(crate) generation_changes: u64,
    /// Pending events dropped by those changes.
    pub(crate) cancelled: u64,
    /// Edges the Local APIC refused (globally disabled); the edge is lost.
    pub(crate) edges_refused: u64,
    /// Vectors moved from IRR to ISR by `acknowledge`.
    pub(crate) acknowledged: u64,
    /// `acknowledge` calls the Local APIC refused (not the eligible vector); never expected.
    pub(crate) ack_refused: u64,
    /// Event queue errors (full, generation exhausted); never expected. The source stops
    /// scheduling after the first one.
    pub(crate) queue_faults: u64,
}

/// What the source compares to detect a reprogram of channel 0.
type ChannelState = (Option<PitMode>, Option<PitRun>);

struct Inner {
    pit: Pit8254Handle,
    io_apic: IoApicHandle,
    lapic: LocalApicHandle,
    clock: VirtualClockHandle,
    queue: EventQueue,
    seen: ChannelState,
    /// The clock at the last catch-up: the time of the PIT write that the next catch-up finds.
    last_sync_ns: u64,
    stats: PitIoapicStats,
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

    /// Present the OUT0 rising edge to INTIN2 as a pulse. `Err` when the Local APIC refused it.
    fn pulse(&mut self) -> Result<(), ()> {
        self.io_apic.set_intin2(true).map_err(|_| ())?;
        self.io_apic.set_intin2(false).map_err(|_| ())
    }

    /// Apply every edge that is due at `now`, in order. An edge is removed from the queue in any
    /// case (a refused one is lost), so nothing stays in the past.
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
            if self.pulse().is_err() {
                self.stats.edges_refused += 1;
            }
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
        self.sync_generation();
        if self.stats.queue_faults != 0 {
            // The old generation could not be dropped: its edges must not be applied.
            return;
        }
        self.deliver_due(now);
        self.last_sync_ns = now;
    }
}

/// The source of the PIT channel 0 line through the IOAPIC and the Local APIC of the board.
///
/// Cloning gives a second handle on the same source (the board owns one, a test or a later host
/// keeps another to read the counters).
#[derive(Clone)]
pub(crate) struct PitIoapicIrqSource(Arc<Mutex<Inner>>);

impl PitIoapicIrqSource {
    /// The source state. Its lock is taken before every device lock it reaches (PIT, PIC, APIC,
    /// clock; MACHINE_CONTRACT.md, S6-33 lock order) and no device ever takes it.
    fn inner(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A source on the handles of one board; `clock` is the clock of the PIT.
    pub(crate) fn new(
        pit: Pit8254Handle,
        io_apic: IoApicHandle,
        lapic: LocalApicHandle,
        clock: VirtualClockHandle,
    ) -> Self {
        let now = clock.snapshot().now_ns();
        Self(Arc::new(Mutex::new(Inner {
            pit,
            io_apic,
            lapic,
            clock,
            queue: EventQueue::new(QUEUE_CAPACITY),
            // The state of a reset PIT: the first catch-up of a channel that holds a count is a
            // reprogram, the first one of a reset channel is none.
            seen: (None, None),
            last_sync_ns: now,
            stats: PitIoapicStats::default(),
        })))
    }

    /// Counters for evidence; read by tests only until a host reports them.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn stats(&self) -> PitIoapicStats {
        self.inner().stats
    }

    /// The count in progress of PIT channel 0 (what the next edge is computed from); tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn channel0_run(&self) -> Option<PitRun> {
        self.inner().pit.snapshot().channel0.run
    }

    /// Events in the queue, as of the last catch-up; tests only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn queued(&self) -> usize {
        self.inner().queue.len()
    }
}

impl BoardIrqSource for PitIoapicIrqSource {
    fn pending_vector(&self) -> Option<u8> {
        let mut inner = self.inner();
        inner.catch_up();
        inner.lapic.eligible()
    }

    fn acknowledge(&mut self, vector: u8) {
        let mut inner = self.inner();
        if inner.lapic.acknowledge_vector(vector, || {}) {
            inner.stats.acknowledged += 1;
        } else {
            inner.stats.ack_refused += 1;
        }
    }

    fn next_deadline_ns(&self) -> Option<u64> {
        let mut inner = self.inner();
        inner.catch_up();
        // After a queue fault (never expected) the source has stopped: a leftover event may be
        // in the past, so no deadline is given. The filter holds the guarantee in the code, not
        // only in the invariant of `deliver_due`: the answer is never a time that has passed.
        if inner.stats.queue_faults != 0 {
            return None;
        }
        let now = inner.clock.snapshot().now_ns();
        inner
            .queue
            .next_deadline()
            .filter(|deadline| *deadline > now)
    }

    fn pit_write_ends_quantum(&self) -> bool {
        true
    }

    fn irq0_edges(&self) -> u64 {
        self.inner().stats.edges
    }

    /// S4-38L8f: the time a halted CPU with IF = 1 can next be woken, as for the 8259 source: now
    /// when a vector is deliverable now; else the time of the next OUT0 edge when that edge would
    /// be deliverable (entry 2 unmasked, routed to this Local APIC, and the vector's class above
    /// the processor priority); else `None`. A masked entry, a vector blocked by TPR or ISR (no
    /// EOI yet) or a disabled Local APIC give no wake: the edge would be lost or stay in IRR, and
    /// the CPU would pass its `HLT` with no interrupt.
    fn wake_ns(&self) -> Option<u64> {
        let mut inner = self.inner();
        inner.catch_up();
        if inner.lapic.eligible().is_some() {
            return Some(inner.clock.snapshot().now_ns());
        }
        let (cpu, vector) = inner.io_apic.intin2_route()?;
        if cpu != inner.lapic.cpu_index() || !inner.lapic.would_accept(vector) {
            return None;
        }
        // After a queue fault (never expected) the source has stopped: no wake time.
        if inner.stats.queue_faults != 0 {
            return None;
        }
        let now = inner.clock.snapshot().now_ns();
        inner
            .queue
            .next_deadline()
            .filter(|deadline| *deadline > now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        AccessWidth, BusDevice, DeviceAccess, IoApic, IoSpace, LocalApic, Pit8254, Pit8254Devices,
    };

    const VECTOR: u8 = 0x30;
    /// Entry 2 low dword with the mask bit.
    const MASKED: u64 = 1 << 16;
    const NS: u64 = 1_000_000_000;
    const HZ: u64 = 1_193_182;

    /// The devices of the board that the source touches, without a bus: PIT block, Local APIC and
    /// IOAPIC windows are driven through their `BusDevice` registers.
    struct Rig {
        clock: VirtualClockHandle,
        pit_device: Pit8254,
        pit: Pit8254Handle,
        lapic_device: LocalApic,
        lapic: LocalApicHandle,
        io_device: IoApic,
        source: PitIoapicIrqSource,
    }

    impl Rig {
        /// Entry 2 holds `VECTOR` (unmasked, edge, fixed) and the Local APIC is enabled by SVR.
        fn new() -> Self {
            let clock = VirtualClockHandle::new(NS).unwrap();
            let devices = Pit8254Devices::new(clock.clone());
            let lapic_device = LocalApic::new();
            let lapic = lapic_device.handle();
            let io_device = IoApic::new(lapic.clone());
            let source = PitIoapicIrqSource::new(
                devices.handle.clone(),
                io_device.handle(),
                lapic.clone(),
                clock.clone(),
            );
            let mut rig = Self {
                clock,
                pit_device: devices.pit,
                pit: devices.handle,
                lapic_device,
                lapic,
                io_device,
                source,
            };
            rig.lapic_write(0xf0, 0x1ff);
            rig.entry2(u64::from(VECTOR));
            rig
        }

        fn pit_write(&mut self, offset: u64, value: u8) {
            let access = DeviceAccess {
                space: IoSpace::Pio,
                offset,
                width: AccessWidth::Byte,
            };
            self.pit_device.write(access, u64::from(value)).unwrap();
        }

        /// Channel 0, LSB+MSB, `mode` (`0x30` mode 0, `0x34` mode 2), reload `n`.
        fn program(&mut self, control: u8, n: u16) {
            self.pit_write(3, control);
            self.pit_write(0, n as u8);
            self.pit_write(0, (n >> 8) as u8);
        }

        fn lapic_write(&mut self, offset: u64, value: u64) {
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: AccessWidth::Dword,
            };
            self.lapic_device.write(access, value).unwrap();
        }

        fn io_write(&mut self, offset: u64, value: u64) {
            let access = DeviceAccess {
                space: IoSpace::Mmio,
                offset,
                width: AccessWidth::Dword,
            };
            self.io_device.write(access, value).unwrap();
        }

        /// The low dword of redirection entry 2 (IOREGSEL `0x14`).
        fn entry2(&mut self, value: u64) {
            self.io_write(0, 0x14);
            self.io_write(0x10, value);
        }

        fn eoi(&mut self) {
            self.lapic_write(0xb0, 0);
        }

        fn now(&self) -> u64 {
            self.clock.snapshot().now_ns()
        }

        fn advance_to(&self, ns: u64) {
            let now = self.now();
            assert!(ns >= now, "time does not go back");
            self.clock.advance_standalone(ns - now).unwrap();
        }

        /// Advance to the next deadline of the source and return it.
        fn step_to_deadline(&self) -> u64 {
            let deadline = self
                .source
                .next_deadline_ns()
                .expect("an edge is scheduled");
            assert!(deadline > self.now(), "the deadline is in the future");
            self.advance_to(deadline);
            // The loop asks at its next boundary; the question applies the edge.
            let _ = self.source.next_deadline_ns();
            deadline
        }

        fn irr_isr(&self) -> (bool, bool) {
            self.lapic.vector_state(VECTOR)
        }
    }

    /// The deadline of the PIT tick `tick`, by hand: `ceil(tick * 1e9 / 1_193_182)`.
    fn deadline(tick: u64) -> u64 {
        (tick * NS).div_ceil(HZ)
    }

    #[test]
    fn the_source_asks_for_the_pit_writes_and_starts_empty() {
        let rig = Rig::new();
        assert!(rig.source.pit_write_ends_quantum());
        assert_eq!(rig.source.irq0_edges(), 0);
        // A channel 0 that nothing programmed has no edge, however much time passes.
        rig.advance_to(10 * NS);
        assert_eq!(rig.source.next_deadline_ns(), None);
        assert_eq!(rig.source.pending_vector(), None);
        assert_eq!(rig.source.stats(), PitIoapicStats::default());
    }

    #[test]
    fn mode_2_gives_exactly_one_vector_per_period_for_a_thousand_periods() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        // By hand: 10 ticks = 10e9 / 1_193_182 = 8380.9 ns, so the first edge is at 8381 ns, the
        // second (20 ticks, 16761.8) at 16762 ns.
        assert_eq!(rig.step_to_deadline(), 8381);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        rig.eoi();
        assert_eq!(rig.step_to_deadline(), 16762);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        rig.eoi();
        for k in 3..=1000u64 {
            let at = rig.step_to_deadline();
            assert_eq!(at, deadline(10 * k), "edge {k}");
            assert_eq!(rig.source.pending_vector(), Some(VECTOR), "edge {k}");
            assert_eq!(rig.irr_isr(), (true, false), "edge {k}");
            rig.source.clone().acknowledge(VECTOR);
            assert_eq!(rig.irr_isr(), (false, true), "edge {k}");
            assert_eq!(
                rig.source.pending_vector(),
                None,
                "in service: nothing more"
            );
            rig.eoi();
            assert_eq!(rig.irr_isr(), (false, false), "edge {k}");
            assert_eq!(rig.source.pending_vector(), None, "edge {k}");
        }
        let stats = rig.source.stats();
        assert_eq!(stats.edges, 1000);
        assert_eq!(rig.source.irq0_edges(), 1000);
        assert_eq!(stats.acknowledged, 1000);
        assert_eq!(stats.generation_changes, 1);
        assert_eq!(
            (stats.cancelled, stats.edges_refused, stats.ack_refused),
            (0, 0, 0)
        );
        assert_eq!(rig.source.queued(), 1, "one event of the next edge");
    }

    #[test]
    fn without_an_eoi_the_second_edge_is_pending_but_not_deliverable() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.step_to_deadline();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        // The second and the third edge: the vector is in service, its IRR bit is set again (and
        // coalesces), and nothing is deliverable.
        rig.step_to_deadline();
        rig.step_to_deadline();
        assert_eq!(rig.source.stats().edges, 3);
        assert_eq!(rig.irr_isr(), (true, true));
        assert_eq!(rig.lapic.pending(), Some(VECTOR));
        assert_eq!(rig.source.pending_vector(), None);
        // The EOI makes the single coalesced request deliverable, once.
        rig.eoi();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        assert_eq!(rig.irr_isr(), (false, true));
        assert_eq!(rig.source.pending_vector(), None);
    }

    #[test]
    fn a_task_priority_at_the_class_of_the_vector_blocks_it_in_irr() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.lapic_write(0x80, 0x30);
        rig.step_to_deadline();
        assert_eq!(rig.lapic.pending(), Some(VECTOR));
        assert_eq!(
            rig.source.pending_vector(),
            None,
            "class 3 is not above TPR class 3"
        );
        rig.lapic_write(0x80, 0x20);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    }

    /// S4-38L8f: an unprogrammed channel has no wake; a programmed one wakes at its next edge;
    /// a due vector wakes now; the wake asks nothing of the state beyond the catch-up.
    #[test]
    fn wake_ns_is_the_next_edge_and_now_when_a_vector_is_deliverable() {
        let mut rig = Rig::new();
        assert_eq!(rig.source.wake_ns(), None, "no count loaded");
        rig.program(0x34, 10);
        assert_eq!(rig.source.wake_ns(), Some(8381));
        assert_eq!(rig.source.wake_ns(), Some(8381), "a question, idempotent");
        // The edge came and nothing took the vector: deliverable now, at the clock.
        rig.advance_to(9000);
        assert_eq!(rig.source.wake_ns(), Some(9000));
        // In service (acknowledged, no EOI): the next edge cannot wake the CPU.
        rig.source.clone().acknowledge(VECTOR);
        assert_eq!(rig.source.wake_ns(), None);
        // The EOI gives the next edge back.
        rig.eoi();
        assert_eq!(rig.source.wake_ns(), Some(16762));
    }

    /// S4-38L8f: a masked entry 2 gives no wake time (the edge would be lost), and unmasking
    /// gives it back; a vector blocked by TPR gives none either (ISSUE-070 form of the check).
    #[test]
    fn wake_ns_is_none_for_a_masked_entry_or_a_vector_blocked_by_the_priority() {
        let mut rig = Rig::new();
        rig.entry2(MASKED | u64::from(VECTOR));
        rig.program(0x34, 10);
        assert_eq!(rig.source.wake_ns(), None, "masked");
        rig.entry2(u64::from(VECTOR));
        assert_eq!(rig.source.wake_ns(), Some(8381), "unmasked");
        rig.lapic_write(0x80, 0x30);
        assert_eq!(
            rig.source.wake_ns(),
            None,
            "class 3 is not above TPR class 3"
        );
        rig.lapic_write(0x80, 0x20);
        assert_eq!(rig.source.wake_ns(), Some(8381));
        // The Local APIC disabled by the software enable bit: nothing is accepted.
        rig.lapic_write(0xf0, 0xff);
        assert_eq!(rig.source.wake_ns(), None, "SVR software enable is off");
    }

    #[test]
    fn a_masked_entry_loses_the_edge_and_an_unmask_does_not_replay_it() {
        let mut rig = Rig::new();
        rig.entry2(MASKED | u64::from(VECTOR));
        rig.program(0x34, 10);
        rig.step_to_deadline();
        // The edge was presented (counted) and lost at the IOAPIC.
        assert_eq!(rig.source.stats().edges, 1);
        assert_eq!(rig.irr_isr(), (false, false));
        assert_eq!(rig.source.pending_vector(), None);
        // Unmasking replays nothing; the next edge goes through.
        rig.entry2(u64::from(VECTOR));
        assert_eq!(rig.source.pending_vector(), None);
        rig.step_to_deadline();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        // Masking the entry afterwards does not clear the IRR bit that is already set.
        rig.entry2(MASKED | u64::from(VECTOR));
        assert_eq!(rig.irr_isr(), (true, false));
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        // But the next edge is lost again.
        rig.source.clone().acknowledge(VECTOR);
        rig.eoi();
        rig.step_to_deadline();
        assert_eq!(rig.irr_isr(), (false, false));
        assert_eq!(rig.source.stats().edges, 3);
    }

    #[test]
    fn the_shutdown_control_word_cancels_the_event_and_no_edge_follows() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        assert_eq!(rig.source.next_deadline_ns(), Some(8381));
        assert_eq!(rig.source.queued(), 1);
        // Control word 0x30: channel 0, LSB+MSB, mode 0, no count loaded.
        rig.pit_write(3, 0x30);
        assert_eq!(rig.pit.snapshot().channel0.run, None);
        assert_eq!(rig.source.next_deadline_ns(), None);
        assert_eq!(rig.source.queued(), 0);
        rig.advance_to(NS);
        assert_eq!(rig.source.pending_vector(), None);
        let stats = rig.source.stats();
        assert_eq!(
            (stats.edges, stats.generation_changes, stats.cancelled),
            (0, 2, 1)
        );
    }

    #[test]
    fn a_reprogram_drops_the_old_generation_and_the_new_period_gives_the_edges() {
        let mut rig = Rig::new();
        rig.program(0x34, 100);
        // Old first edge: tick 100 (83 800 ns); 5 ticks pass first (5000 ns is tick 5).
        assert_eq!(rig.source.next_deadline_ns(), Some(deadline(100)));
        rig.advance_to(5000);
        assert_eq!(pit_ticks(5000), 5);
        rig.program(0x34, 10);
        // New count loaded at tick 5: edges at ticks 15, 25, 35, ...
        let mut edges = Vec::new();
        while let Some(at) = rig.source.next_deadline_ns() {
            if at > deadline(100) + 1000 {
                break;
            }
            rig.advance_to(at);
            assert_eq!(rig.source.pending_vector(), Some(VECTOR));
            rig.source.clone().acknowledge(VECTOR);
            rig.eoi();
            edges.push(at);
        }
        let expected: Vec<u64> = (0..)
            .map(|k| deadline(5 + 10 * (k + 1)))
            .take_while(|at| *at <= deadline(100) + 1000)
            .collect();
        assert_eq!(edges, expected);
        assert!(!edges.contains(&deadline(100)), "the old edge never fires");
        // Tick 100 of the old count is not a new edge (95 is not a multiple of 10) and no
        // second edge appeared for it: one edge per new period.
        assert_eq!(rig.source.stats().edges, expected.len() as u64);
        let stats = rig.source.stats();
        assert_eq!((stats.generation_changes, stats.cancelled), (2, 1));
    }

    #[test]
    fn a_reprogram_after_an_applied_edge_drops_only_what_is_ahead() {
        // The old edge is applied by the catch-up that precedes the reprogram of the loop; the
        // reprogram then drops only what is still ahead.
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.step_to_deadline();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        rig.eoi();
        rig.program(0x34, 20);
        assert_eq!(rig.source.stats().edges, 1);
        assert_eq!(rig.source.queued(), 1);
        let t0 = pit_ticks(rig.now());
        assert_eq!(rig.source.next_deadline_ns(), Some(deadline(t0 + 20)));
    }

    #[test]
    fn mode_0_gives_one_edge_per_programming_and_none_after() {
        let mut rig = Rig::new();
        rig.program(0x30, 10);
        assert_eq!(rig.step_to_deadline(), 8381);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        rig.source.clone().acknowledge(VECTOR);
        rig.eoi();
        // No second edge, however much time passes.
        assert_eq!(rig.source.next_deadline_ns(), None);
        rig.advance_to(NS);
        assert_eq!(rig.source.pending_vector(), None);
        assert_eq!(rig.source.stats().edges, 1);
        // A new count is a new generation and a new edge, counted from its own write.
        rig.program(0x30, 10);
        let t0 = pit_ticks(rig.now());
        assert_eq!(rig.source.next_deadline_ns(), Some(deadline(t0 + 10)));
        rig.step_to_deadline();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        assert_eq!(rig.source.stats().edges, 2);
    }

    #[test]
    fn a_pit_reset_cancels_the_event_like_a_reprogram() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        assert_eq!(rig.source.next_deadline_ns(), Some(8381));
        rig.pit.reset();
        assert_eq!(rig.source.next_deadline_ns(), None);
        assert_eq!(rig.source.queued(), 0);
        rig.advance_to(NS);
        assert_eq!(rig.source.pending_vector(), None);
        assert_eq!(rig.source.stats().cancelled, 1);
    }

    #[test]
    fn an_edge_that_is_not_accepted_never_puts_the_deadline_in_the_past() {
        // The guest does not accept (IF = 0 is the loop's view; here nothing acknowledges): the
        // applied edges stay in IRR and the deadline is always the next edge, in the future.
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        // 5 periods and 3 ns: edges 1..=5 are due, the 6th is ahead.
        rig.advance_to(deadline(50) + 3);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        assert_eq!(rig.source.stats().edges, 5);
        let next = rig.source.next_deadline_ns().unwrap();
        assert_eq!(next, deadline(60));
        assert!(next > rig.now());
        // Asking again and again changes nothing.
        for _ in 0..3 {
            assert_eq!(rig.source.next_deadline_ns(), Some(next));
            assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        }
        assert_eq!(rig.source.stats().edges, 5);
        assert_eq!(rig.irr_isr(), (true, false));
    }

    #[test]
    fn a_refused_pulse_loses_the_edge_and_the_chain_goes_on() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.lapic.set_global_enabled(false).unwrap();
        rig.step_to_deadline();
        assert_eq!(
            rig.source.next_deadline_ns().map(|at| at > rig.now()),
            Some(true)
        );
        let stats = rig.source.stats();
        assert_eq!((stats.edges, stats.edges_refused), (1, 1));
        // The Local APIC is back: the next edge works.
        rig.lapic.set_global_enabled(true).unwrap();
        rig.lapic_write(0xf0, 0x1ff);
        rig.step_to_deadline();
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
        assert_eq!(rig.source.stats().edges_refused, 1);
    }

    #[test]
    fn acknowledge_of_a_vector_that_is_not_eligible_changes_nothing_and_is_counted() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.step_to_deadline();
        let mut source = rig.source.clone();
        source.acknowledge(VECTOR + 1);
        assert_eq!(rig.irr_isr(), (true, false));
        assert_eq!(rig.source.stats().ack_refused, 1);
        source.acknowledge(VECTOR);
        assert_eq!(rig.irr_isr(), (false, true));
        assert_eq!(rig.source.stats().acknowledged, 1);
    }

    #[test]
    fn a_software_disabled_local_apic_has_no_deliverable_vector() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        rig.lapic_write(0xf0, 0xff);
        rig.step_to_deadline();
        assert_eq!(rig.lapic.pending(), Some(VECTOR));
        assert_eq!(rig.source.pending_vector(), None);
        rig.lapic_write(0xf0, 0x1ff);
        assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    }

    #[test]
    fn after_a_queue_fault_the_deadline_is_none_and_the_old_edge_is_not_applied() {
        let mut rig = Rig::new();
        rig.program(0x34, 10);
        assert_eq!(rig.source.next_deadline_ns(), Some(8381));
        // An artificial fault: the generation counter is exhausted, so the next reprogram cannot
        // drop the old generation.
        rig.source.inner().queue.exhaust_generation_for_test();
        // The old edge (8381 ns) is past by now; the reprogram is the fault.
        rig.advance_to(20_000);
        rig.program(0x34, 20);
        assert_eq!(
            rig.source.next_deadline_ns(),
            None,
            "no deadline, none in the past"
        );
        assert_eq!(
            rig.source.queued(),
            1,
            "the old event is still there, in the past"
        );
        let stats = rig.source.stats();
        assert_eq!((stats.queue_faults, stats.edges), (1, 0));
        // The old generation's edge was not applied, and later time changes nothing.
        rig.advance_to(NS);
        assert_eq!(rig.source.pending_vector(), None);
        assert_eq!(rig.source.next_deadline_ns(), None);
        assert_eq!(rig.source.stats().edges, 0);
    }
}
