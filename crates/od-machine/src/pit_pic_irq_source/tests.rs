//! Tests of the PIT -> 8259 master IR0 source (S4-39g). The devices are driven through their own
//! `BusDevice` registers without a bus; the oracles are computed by hand: PIT clock 1_193_182 Hz,
//! the clock of the board 1 GHz, so 10 ticks are 8380.9 ns and the edge is due at 8381 ns.

use super::*;
use crate::{AccessWidth, BusDevice, DeviceAccess, IoSpace, Pic8259, Pic8259Role, Pit8254Devices};

const NS: u64 = 1_000_000_000;
const HZ: u64 = 1_193_182;
const VECTOR: u8 = 0x30;

/// The deadline of the PIT tick, by hand: `ceil(tick * 1e9 / 1_193_182)`.
fn deadline(tick: u64) -> u64 {
    (tick * NS).div_ceil(HZ)
}

struct Rig {
    clock: VirtualClockHandle,
    pit_device: crate::Pit8254,
    pic_device: Pic8259,
    pic: Pic8259Handle,
    source: PitPicIrqSource,
}

impl Rig {
    /// A master initialised as Linux does (ICW1..ICW4 `0x01`) with `imr`; PIT reset.
    fn new(imr: u8) -> Self {
        let clock = VirtualClockHandle::new(NS).unwrap();
        let devices = Pit8254Devices::new(clock.clone());
        let (pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
        let source = PitPicIrqSource::new(devices.handle.clone(), pic.clone(), clock.clone());
        let mut rig = Self {
            clock,
            pit_device: devices.pit,
            pic_device,
            pic,
            source,
        };
        rig.pic_write(0, 0x11);
        rig.pic_write(1, 0x30);
        rig.pic_write(1, 0x04);
        rig.pic_write(1, 0x01);
        rig.pic_write(1, imr);
        rig
    }

    fn access(offset: u64) -> DeviceAccess {
        DeviceAccess {
            space: IoSpace::Pio,
            offset,
            width: AccessWidth::Byte,
        }
    }

    fn pit_write(&mut self, offset: u64, value: u8) {
        self.pit_device
            .write(Self::access(offset), u64::from(value))
            .unwrap();
    }

    fn pic_write(&mut self, offset: u64, value: u8) {
        self.pic_device
            .write(Self::access(offset), u64::from(value))
            .unwrap();
    }

    /// Channel 0, LSB+MSB, control `0x34` (mode 2) or `0x30` (mode 0), reload `n`.
    fn program(&mut self, control: u8, n: u16) {
        self.pit_write(3, control);
        self.pit_write(0, n as u8);
        self.pit_write(0, (n >> 8) as u8);
    }

    fn now(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    fn advance_to(&self, ns: u64) {
        let now = self.now();
        assert!(ns >= now, "time does not go back");
        self.clock.advance_standalone(ns - now).unwrap();
    }

    /// Advance to the next deadline of the source and return it; the question that follows
    /// applies the edge, as the board loop does at its next boundary.
    fn step_to_deadline(&self) -> u64 {
        let deadline = self
            .source
            .next_deadline_ns()
            .expect("an edge is scheduled");
        assert!(deadline > self.now(), "the deadline is in the future");
        self.advance_to(deadline);
        let _ = self.source.pending_vector();
        deadline
    }

    fn irr_isr(&self) -> (u8, u8) {
        let s = self.pic.snapshot();
        (s.irr, s.isr)
    }

    fn eoi(&mut self) {
        self.pic_write(0, 0x20);
    }
}

#[test]
fn the_helper_formula_matches_the_model_tick_function() {
    for tick in [1, 4, 10, 1003, 1_000_000] {
        assert_eq!(pit_ticks(deadline(tick)), tick);
        assert!(pit_ticks(deadline(tick) - 1) < tick);
    }
}

#[test]
fn a_reset_pit_gives_no_edge_no_vector_no_deadline() {
    let rig = Rig::new(0x00);
    assert_eq!(rig.source.pending_vector(), None);
    assert_eq!(rig.source.next_deadline_ns(), None);
    assert_eq!(rig.source.wake_ns(), None);
    assert_eq!(rig.source.irq0_edges(), 0);
}

#[test]
fn an_edge_raises_ir0_and_the_vector_is_the_icw2_base() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    assert_eq!(rig.source.next_deadline_ns(), Some(8381));
    assert_eq!(rig.source.pending_vector(), None, "no edge before its time");
    assert_eq!(rig.step_to_deadline(), 8381);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    // A question changes nothing.
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert_eq!(rig.irr_isr(), (1, 0));
    assert_eq!(rig.source.irq0_edges(), 1);
}

#[test]
fn inta_moves_irr_to_isr_and_the_eoi_clears_isr() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    rig.step_to_deadline();
    let mut source = rig.source.clone();
    source.acknowledge(VECTOR);
    assert_eq!(rig.irr_isr(), (0, 1));
    assert_eq!(rig.source.pending_vector(), None);
    rig.eoi();
    assert_eq!(rig.irr_isr(), (0, 0));
    let stats = rig.source.stats();
    assert_eq!((stats.acknowledged, stats.ack_refused), (1, 0));
}

#[test]
fn without_an_eoi_the_second_edge_waits_in_irr_and_is_delivered_once_after_it() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    assert_eq!(rig.step_to_deadline(), 8381);
    let mut source = rig.source.clone();
    source.acknowledge(VECTOR);
    // Second and third edges at 16762 and 25143 (30 ticks = 25142.7 ns): they coalesce.
    assert_eq!(rig.step_to_deadline(), 16762);
    assert_eq!(rig.irr_isr(), (1, 1));
    assert_eq!(rig.source.pending_vector(), None, "IR0 is in service");
    assert_eq!(rig.step_to_deadline(), 25143);
    assert_eq!(rig.irr_isr(), (1, 1), "one bit for two edges");
    assert_eq!(rig.source.irq0_edges(), 3);
    rig.eoi();
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    source.acknowledge(VECTOR);
    assert_eq!(rig.irr_isr(), (0, 1));
    assert_eq!(rig.source.pending_vector(), None);
}

#[test]
fn a_thousand_periods_give_a_thousand_vectors_with_an_eoi_each() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 4);
    let mut source = rig.source.clone();
    for k in 1..=1000u64 {
        let at = rig.step_to_deadline();
        assert_eq!(at, deadline(4 * k), "edge {k}");
        assert_eq!(rig.source.pending_vector(), Some(VECTOR), "edge {k}");
        source.acknowledge(VECTOR);
        assert_eq!(rig.irr_isr(), (0, 1));
        rig.eoi();
        assert_eq!(rig.irr_isr(), (0, 0));
    }
    let stats = rig.source.stats();
    assert_eq!(stats.edges, 1000);
    assert_eq!(stats.acknowledged, 1000);
    assert_eq!((stats.ack_refused, stats.queue_faults), (0, 0));
}

#[test]
fn a_masked_line_latches_the_edge_and_the_unmask_delivers_it() {
    let mut rig = Rig::new(0xFF);
    rig.program(0x34, 10);
    rig.step_to_deadline();
    assert_eq!(rig.irr_isr(), (1, 0), "latched although masked");
    assert_eq!(rig.source.pending_vector(), None);
    rig.pic_write(1, 0xFE);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
}

#[test]
fn the_wake_time_is_now_when_a_vector_waits_the_next_edge_when_idle_and_none_when_blocked() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    // Idle, an edge would be delivered: the wake is the edge.
    assert_eq!(rig.source.wake_ns(), Some(8381));
    rig.step_to_deadline();
    // A vector waits: the wake is now.
    assert_eq!(rig.source.wake_ns(), Some(8381));
    let mut source = rig.source.clone();
    source.acknowledge(VECTOR);
    // In service without an EOI: no edge can wake a halted CPU.
    assert_eq!(rig.source.wake_ns(), None);
    rig.eoi();
    assert_eq!(rig.source.wake_ns(), Some(16762));
    // Masked: the edge would be latched, not delivered.
    rig.pic_write(1, 0x01);
    assert_eq!(rig.source.wake_ns(), None);
}

#[test]
fn before_the_pic_is_initialised_nothing_wakes_and_the_edge_is_latched() {
    let clock = VirtualClockHandle::new(NS).unwrap();
    let devices = Pit8254Devices::new(clock.clone());
    let (_pic_device, pic) = Pic8259::new_with_handle(Pic8259Role::Master);
    let source = PitPicIrqSource::new(devices.handle.clone(), pic.clone(), clock.clone());
    let mut pit = devices.pit;
    for (offset, value) in [(3u64, 0x34u8), (0, 10), (0, 0)] {
        pit.write(Rig::access(offset), u64::from(value)).unwrap();
    }
    assert_eq!(source.wake_ns(), None);
    clock.advance_standalone(8381).unwrap();
    assert_eq!(source.pending_vector(), None);
    assert_eq!(pic.snapshot().irr, 1, "the chip latches the edge");
}

#[test]
fn a_shutdown_leaves_no_event_and_no_wake() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    assert_eq!(rig.source.next_deadline_ns(), Some(8381));
    rig.pit_write(3, 0x30);
    // The board clock moves by at least one ns between a guest write and the next question.
    rig.advance_to(1);
    assert_eq!(rig.source.next_deadline_ns(), None);
    assert_eq!(rig.source.queued(), 0);
    assert_eq!(rig.source.wake_ns(), None);
    assert_eq!(rig.source.stats().cancelled, 1);
}

#[test]
fn a_reprogram_drops_the_old_edge_and_counts_from_the_write() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 1003);
    let _ = rig.source.next_deadline_ns();
    // Reprogram at 1000 ns, before the first edge (1003 ticks = 840_000 ns).
    rig.advance_to(1000);
    rig.program(0x34, 10);
    let next = rig.source.next_deadline_ns().expect("scheduled");
    assert_eq!(next, deadline(pit_ticks(1000) + 10));
    // The first program (three writes, one sample) and the reprogram.
    assert_eq!(rig.source.stats().generation_changes, 2);
    assert_eq!(rig.source.stats().cancelled, 1);
}

#[test]
fn asking_twice_at_one_clock_value_applies_each_edge_once() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    rig.step_to_deadline();
    let first = rig.source.stats();
    for _ in 0..5 {
        let _ = rig.source.pending_vector();
        let _ = rig.source.next_deadline_ns();
        let _ = rig.source.wake_ns();
    }
    assert_eq!(rig.source.stats(), first);
    assert_eq!(rig.irr_isr(), (1, 0));
}

#[test]
fn a_false_acknowledge_is_counted_and_changes_nothing() {
    let mut rig = Rig::new(0x00);
    rig.program(0x34, 10);
    rig.step_to_deadline();
    let before = rig.pic.snapshot();
    let mut source = rig.source.clone();
    source.acknowledge(0x31);
    assert_eq!(rig.pic.snapshot(), before);
    assert_eq!(rig.source.stats().ack_refused, 1);
}

#[test]
fn a_mode4_count_gives_one_edge_one_tick_after_the_strobe_and_none_after() {
    // S4-39g2: control word 0x38 (channel 0, LSB+MSB, mode 4), count 10 written at 0 ns: the
    // strobe is at tick 10 (8381 ns), OUT rises at tick 11 (ceil(11e9 / 1_193_182) = 9220 ns).
    let mut rig = Rig::new(0x00);
    rig.source.inner().pit.enable_software_strobe();
    rig.program(0x38, 10);
    assert_eq!(rig.source.next_deadline_ns(), Some(9220));
    rig.advance_to(8381);
    assert_eq!(
        rig.source.pending_vector(),
        None,
        "no edge during the strobe"
    );
    assert_eq!(rig.irr_isr(), (0, 0));
    assert_eq!(rig.source.irq0_edges(), 0);
    assert_eq!(rig.step_to_deadline(), 9220);
    assert_eq!(rig.source.pending_vector(), Some(VECTOR));
    assert_eq!(rig.irr_isr(), (1, 0));
    assert_eq!(rig.source.irq0_edges(), 1);
    // One shot: no further edge is scheduled.
    rig.eoi();
    rig.advance_to(1_000_000);
    assert_eq!(rig.source.next_deadline_ns(), None);
    assert_eq!(rig.source.irq0_edges(), 1);
    // A new count re-arms it from the write.
    let write_ns = rig.now();
    rig.program(0x38, 10);
    rig.advance_to(write_ns + 1);
    assert_eq!(
        rig.source.next_deadline_ns(),
        Some(deadline(pit_ticks(write_ns) + 11))
    );
}
