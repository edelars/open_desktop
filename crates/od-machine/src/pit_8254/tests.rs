use super::*;
use crate::pci_ecam::{PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES};
use crate::{
    ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES, AbsentPortRegion, AbsentPorts, AcpiPm, BusAccessError,
    FIRMWARE_COM1_PORT, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryRegionKind, MemoryRegionSpec, PIC_MASTER_BASE_PORT, PIC_PORT_BYTES, PIC_SLAVE_BASE_PORT,
    Pic8259Pair, PortAddress, PortRange, PortRegionBinding, RamLimits,
};

const BYTE: AccessWidth = AccessWidth::Byte;

fn acc(offset: u64, width: AccessWidth) -> DeviceAccess {
    DeviceAccess {
        space: IoSpace::Pio,
        offset,
        width,
    }
}

/// Devices in the reset state: gate 2 low.
fn setup_ungated() -> (Pit8254Devices, VirtualClockHandle) {
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    (Pit8254Devices::new(clock.clone()), clock)
}

/// Devices with gate 2 raised, as the guest does before loading channel 2
/// (S4-29d: loading channel 2 with gate 2 low is refused).
fn setup() -> (Pit8254Devices, VirtualClockHandle) {
    let (mut dev, clock) = setup_ungated();
    dev.port_b.write(acc(0, BYTE), 0x01).unwrap();
    (dev, clock)
}

/// Smallest virtual time (ns) at which the tick index reaches `tick`.
fn ns_for_tick(tick: u64) -> u64 {
    ((u128::from(tick) * NANOS_PER_SECOND).div_ceil(u128::from(PIT_INPUT_HZ))) as u64
}

fn advance(clock: &VirtualClockHandle, ns: u64) {
    clock.advance_standalone(ns).unwrap();
}

/// Moves the clock to the first instant of tick `tick`.
fn goto_tick(clock: &VirtualClockHandle, tick: u64) {
    let target = ns_for_tick(tick);
    let now = clock.snapshot().now_ns();
    assert!(target >= now, "clock cannot go back");
    advance(clock, target - now);
    assert_eq!(pit_ticks(clock.snapshot().now_ns()), tick);
}

fn out(pit: &mut Pit8254, offset: u64, value: u8) -> Result<(), DeviceError> {
    pit.write(acc(offset, BYTE), u64::from(value))
}

fn inb(pit: &mut Pit8254, offset: u64) -> u8 {
    pit.read(acc(offset, BYTE)).unwrap() as u8
}

/// Control word then reload; `lo` and, for LSB+MSB access, `hi`.
fn program(pit: &mut Pit8254, control: u8, lo: u8, hi: Option<u8>) {
    let port = if control >> 6 == 0 { 0 } else { 2 };
    out(pit, 3, control).unwrap();
    out(pit, port, lo).unwrap();
    if let Some(hi) = hi {
        out(pit, port, hi).unwrap();
    }
}

fn port_b(dev: &mut Pit8254Devices) -> u8 {
    dev.port_b.read(acc(0, BYTE)).unwrap() as u8
}

#[test]
fn constants_and_tick_formula() {
    assert_eq!(PIT_BASE_PORT, 0x40);
    assert_eq!(PIT_PORT_BYTES, 4);
    assert_eq!(PIT_PORT_B, 0x61);
    assert_eq!(PIT_INPUT_HZ, 1_193_182);
    assert_eq!(pit_ticks(0), 0);
    assert_eq!(pit_ticks(1_000_000), 1193); // 1193.182
    assert_eq!(pit_ticks(1_000_000_000), 1_193_182);
    assert_eq!(
        pit_ticks(u64::MAX),
        (u128::from(u64::MAX) * 1_193_182 / 1_000_000_000) as u64
    );
    // The inverse used by the tests lands exactly on tick edges.
    for tick in [1u64, 2, 1193, 11931, 1_193_182] {
        assert_eq!(pit_ticks(ns_for_tick(tick)), tick);
        assert_eq!(pit_ticks(ns_for_tick(tick) - 1), tick - 1);
    }
}

#[test]
fn reset_matrix() {
    let (mut dev, _clock) = setup_ungated();
    assert_eq!(dev.handle.snapshot(), PitSnapshot::default());
    let s = dev.handle.snapshot();
    assert_eq!(s.channel0.mode, None);
    assert_eq!(s.channel2.run, None);
    assert!(!s.gate2 && !s.speaker_data);
    assert!(!dev.handle.out(PitChannel::Channel0));
    assert!(!dev.handle.out(PitChannel::Channel2));
    assert_eq!(dev.handle.count(PitChannel::Channel0), None);
    assert_eq!(dev.handle.count(PitChannel::Channel2), None);
    assert_eq!(port_b(&mut dev), 0);
    // Counters are not initialised: neither a read nor a data write is accepted.
    assert_eq!(
        dev.pit.read(acc(0, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        dev.pit.read(acc(2, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        out(&mut dev.pit, 2, 1),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.snapshot(), PitSnapshot::default());
}

#[test]
fn reset_returns_to_the_reset_state() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x34, 10, Some(0));
    program(&mut dev.pit, 0xB0, 100, Some(0));
    dev.port_b.write(acc(0, BYTE), 0x03).unwrap();
    out(&mut dev.pit, 3, 0x80).unwrap();
    goto_tick(&clock, 50);
    assert_ne!(dev.handle.snapshot(), PitSnapshot::default());
    dev.handle.reset();
    assert_eq!(dev.handle.snapshot(), PitSnapshot::default());
    assert_eq!(port_b(&mut dev), 0);
    assert_eq!(dev.handle.count(PitChannel::Channel2), None);
}

#[test]
fn accepted_control_words_program_the_channel() {
    for (word, ch, mode, access) in [
        (0x30u8, 0usize, PitMode::TerminalCount, PitAccess::LsbMsb),
        (0x34, 0, PitMode::RateGenerator, PitAccess::LsbMsb),
        (0x10, 0, PitMode::TerminalCount, PitAccess::Lsb),
        (0x24, 0, PitMode::RateGenerator, PitAccess::Msb),
        (0xB0, 2, PitMode::TerminalCount, PitAccess::LsbMsb),
        (0xB4, 2, PitMode::RateGenerator, PitAccess::LsbMsb),
        (0x94, 2, PitMode::RateGenerator, PitAccess::Lsb),
        (0xA0, 2, PitMode::TerminalCount, PitAccess::Msb),
    ] {
        let (mut dev, _clock) = setup();
        out(&mut dev.pit, 3, word).unwrap();
        let s = dev.handle.snapshot();
        let c = if ch == 0 { s.channel0 } else { s.channel2 };
        assert_eq!(
            (c.mode, c.access),
            (Some(mode), Some(access)),
            "{word:#04x}"
        );
        assert_eq!(c.run, None);
        // The other channel is untouched.
        let other = if ch == 0 { s.channel2 } else { s.channel0 };
        assert_eq!(other, PitChannelSnapshot::default());
    }
}

#[test]
fn refused_control_words_do_not_change_the_snapshot() {
    let mut refused = Vec::new();
    for channel_bits in [0x00u8, 0x80] {
        for access in [0x10u8, 0x20, 0x30] {
            // BCD, and every mode except 0 and 2 (modes 6 and 7 alias 2 and 3 on
            // the chip; the model does not accept the aliases).
            for mode_bits in [0x01u8, 0x03, 0x05, 0x0F] {
                refused.push(channel_bits | access | mode_bits);
            }
            for mode in [1u8, 3, 4, 5, 6, 7] {
                refused.push(channel_bits | access | (mode << 1));
            }
        }
    }
    // Channel 1, every access mode and the latch, and the read-back command.
    for low in 0u8..=0x0F {
        for access in [0x00u8, 0x10, 0x20, 0x30] {
            refused.push(0x40 | access | low);
        }
    }
    for low in 0u8..=0x3F {
        refused.push(0xC0 | low);
    }
    refused.sort_unstable();
    refused.dedup();
    for word in refused {
        let (mut dev, clock) = setup();
        // Give the snapshot something that a wrong refusal would disturb.
        program(&mut dev.pit, 0x34, 10, Some(0));
        program(&mut dev.pit, 0xB0, 200, Some(0));
        goto_tick(&clock, 5);
        out(&mut dev.pit, 3, 0x80).unwrap(); // latch on channel 2
        let before = dev.handle.snapshot();
        assert_eq!(
            out(&mut dev.pit, 3, word),
            Err(DeviceError::UnsupportedConfiguration),
            "{word:#04x}"
        );
        assert_eq!(dev.handle.snapshot(), before, "{word:#04x}");
    }
}

#[test]
fn channel_one_ports_and_the_control_port_read_are_refused() {
    let (mut dev, _clock) = setup();
    program(&mut dev.pit, 0x34, 10, Some(0));
    let before = dev.handle.snapshot();
    assert_eq!(
        dev.pit.read(acc(1, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        out(&mut dev.pit, 1, 0x55),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.pit.read(acc(3, BYTE)), Err(DeviceError::WriteOnly));
    assert_eq!(dev.handle.snapshot(), before);
}

#[test]
fn reload_counts_down_with_floor_and_without_step_drift() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0xB0, 0x20, Some(0x4E)); // 20000
    assert_eq!(
        dev.handle.snapshot().channel2.run,
        Some(PitRun {
            reload: 20_000,
            start_tick: 0,
            next: None
        })
    );
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(20_000));
    // Three 333_333 ns steps: 999_999 ns is 1193.18 ticks, so 1193. A sum of
    // per-step floors would give 3 * 397 = 1191.
    for _ in 0..3 {
        advance(&clock, 333_333);
    }
    assert_eq!(clock.snapshot().now_ns(), 999_999);
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(20_000 - 1193));
    // Live reads through the ports: low byte, then high byte.
    let value = 20_000u16 - 1193;
    assert_eq!(inb(&mut dev.pit, 2), value as u8);
    assert_eq!(inb(&mut dev.pit, 2), (value >> 8) as u8);
    // One more 1 ms step: 1_999_999 ns is 2386.36 -> 2386.
    advance(&clock, 1_000_000);
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(20_000 - 2386));
}

#[test]
fn a_reload_aligns_to_the_free_running_tick_grid() {
    let (mut dev, clock) = setup();
    advance(&clock, 900_000); // 1073.86 -> tick 1073
    program(&mut dev.pit, 0xB0, 0x20, Some(0x4E));
    assert_eq!(dev.handle.snapshot().channel2.run.unwrap().start_tick, 1073);
    advance(&clock, 200_000); // 1.1 ms -> 1312.50 -> tick 1312: 239 edges crossed
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(20_000 - 239));
}

#[test]
fn reload_zero_means_65536_and_channels_are_independent() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x30, 0, Some(0)); // channel 0 mode 0, reload 0
    program(&mut dev.pit, 0xB0, 5, Some(0)); // channel 2, reload 5
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(0));
    goto_tick(&clock, 3);
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(65_533));
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(2));
    assert!(!dev.handle.out(PitChannel::Channel0));
    goto_tick(&clock, 65_535);
    assert!(!dev.handle.out(PitChannel::Channel0));
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(1));
    goto_tick(&clock, 65_536);
    assert!(dev.handle.out(PitChannel::Channel0));
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(0));
}

#[test]
fn single_byte_access_modes_load_lsb_or_msb() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x10, 200, None); // LSB only
    program(&mut dev.pit, 0xA0, 0x20, None); // channel 2, MSB only: 0x2000
    assert_eq!(dev.handle.snapshot().channel0.run.unwrap().reload, 200);
    assert_eq!(dev.handle.snapshot().channel2.run.unwrap().reload, 0x2000);
    goto_tick(&clock, 50);
    assert_eq!(inb(&mut dev.pit, 0), 150);
    assert_eq!(
        inb(&mut dev.pit, 0),
        150,
        "LSB access always reads the low byte"
    );
    assert_eq!(inb(&mut dev.pit, 2), ((0x2000 - 50) >> 8) as u8);
}

#[test]
fn mode0_out_is_low_until_terminal_count_then_high() {
    let (mut dev, clock) = setup();
    out(&mut dev.pit, 3, 0xB0).unwrap();
    assert!(
        !dev.handle.out(PitChannel::Channel2),
        "low after the control word"
    );
    out(&mut dev.pit, 2, 0xA9).unwrap();
    out(&mut dev.pit, 2, 0x04).unwrap(); // 1193
    assert!(!dev.handle.out(PitChannel::Channel2));
    goto_tick(&clock, 1191);
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(2));
    assert!(!dev.handle.out(PitChannel::Channel2));
    goto_tick(&clock, 1192);
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(1));
    assert!(!dev.handle.out(PitChannel::Channel2));
    goto_tick(&clock, 1193);
    assert_eq!(dev.handle.count(PitChannel::Channel2), Some(0));
    assert!(
        dev.handle.out(PitChannel::Channel2),
        "high at terminal count"
    );
    goto_tick(&clock, 1194);
    assert_eq!(
        dev.handle.count(PitChannel::Channel2),
        Some(0xFFFF),
        "keeps counting"
    );
    assert!(
        dev.handle.out(PitChannel::Channel2),
        "stays high until a new count"
    );
    // No IRQ, no event: nothing else changed.
    assert_eq!(
        dev.handle.snapshot().channel0,
        PitChannelSnapshot::default()
    );
}

#[test]
fn mode0_new_count_drops_out_at_the_first_byte_and_restarts_at_the_second() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x30, 2, Some(0));
    goto_tick(&clock, 5);
    assert!(dev.handle.out(PitChannel::Channel0));
    out(&mut dev.pit, 0, 7).unwrap();
    assert!(
        !dev.handle.out(PitChannel::Channel0),
        "OUT low, counting stopped"
    );
    assert_eq!(dev.handle.count(PitChannel::Channel0), None);
    goto_tick(&clock, 8);
    out(&mut dev.pit, 0, 0).unwrap();
    assert_eq!(dev.handle.snapshot().channel0.run.unwrap().start_tick, 8);
    goto_tick(&clock, 14);
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(1));
    assert!(!dev.handle.out(PitChannel::Channel0));
    goto_tick(&clock, 15);
    assert!(dev.handle.out(PitChannel::Channel0));
}

#[test]
fn mode2_out_is_high_and_low_for_one_tick_before_the_reload() {
    let (mut dev, clock) = setup();
    out(&mut dev.pit, 3, 0x34).unwrap();
    assert!(
        dev.handle.out(PitChannel::Channel0),
        "high after the control word"
    );
    out(&mut dev.pit, 0, 5).unwrap();
    out(&mut dev.pit, 0, 0).unwrap();
    for tick in 0u64..=12 {
        goto_tick(&clock, tick);
        let p = tick % 5;
        assert_eq!(
            dev.handle.count(PitChannel::Channel0),
            Some((5 - p) as u16),
            "tick {tick}"
        );
        assert_eq!(dev.handle.out(PitChannel::Channel0), p != 4, "tick {tick}");
    }
}

#[test]
fn mode2_reload_65536_and_count_one_is_refused() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x34, 0, Some(0));
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(0));
    goto_tick(&clock, 1);
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(65_535));
    goto_tick(&clock, 65_535);
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(1));
    assert!(!dev.handle.out(PitChannel::Channel0));
    goto_tick(&clock, 65_536);
    assert!(dev.handle.out(PitChannel::Channel0));
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(0));

    // Count 1: LSB-only is refused at once; LSB+MSB at the second byte, and the
    // second byte can be retried.
    let (mut dev, _clock) = setup();
    out(&mut dev.pit, 3, 0x14).unwrap();
    let before = dev.handle.snapshot();
    assert_eq!(
        out(&mut dev.pit, 0, 1),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.snapshot(), before);
    out(&mut dev.pit, 3, 0x34).unwrap();
    out(&mut dev.pit, 0, 1).unwrap();
    let before = dev.handle.snapshot();
    assert_eq!(
        out(&mut dev.pit, 0, 0),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.snapshot(), before);
    out(&mut dev.pit, 0, 1).unwrap();
    assert_eq!(dev.handle.snapshot().channel0.run.unwrap().reload, 257);
}

#[test]
fn mode2_new_count_takes_effect_at_the_next_period_boundary() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x14, 10, None);
    goto_tick(&clock, 3);
    out(&mut dev.pit, 0, 4).unwrap();
    assert_eq!(
        dev.handle.snapshot().channel0.run.unwrap().next,
        Some(PitNextReload {
            reload: 4,
            start_tick: 10
        })
    );
    goto_tick(&clock, 9);
    assert_eq!(dev.handle.count(PitChannel::Channel0), Some(1));
    assert!(!dev.handle.out(PitChannel::Channel0));
    for (tick, count, high) in [
        (10u64, 4u16, true),
        (13, 1, false),
        (14, 4, true),
        (17, 1, false),
    ] {
        goto_tick(&clock, tick);
        assert_eq!(
            dev.handle.count(PitChannel::Channel0),
            Some(count),
            "tick {tick}"
        );
        assert_eq!(dev.handle.out(PitChannel::Channel0), high, "tick {tick}");
    }
    // A write at tick 17 folds the pending reload (4 from tick 10) first; the next
    // boundary is computed from that period: 10 + 2 * 4 = 18.
    out(&mut dev.pit, 0, 6).unwrap();
    let run = dev.handle.snapshot().channel0.run.unwrap();
    assert_eq!((run.reload, run.start_tick), (4, 10));
    assert_eq!(run.next.unwrap().start_tick, 18);
}

#[test]
fn counter_latch_delivers_exactly_once() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0xB0, 0x50, Some(0xC3)); // 50000
    goto_tick(&clock, 100);
    out(&mut dev.pit, 3, 0x80).unwrap(); // latch channel 2 (access bits 00)
    assert_eq!(
        dev.handle.snapshot().channel2.latch,
        Some(PitLatch {
            value: 49_900,
            hi_pending: false
        })
    );
    goto_tick(&clock, 200);
    // The latched value, not the live one.
    assert_eq!(inb(&mut dev.pit, 2), (49_900u16 & 0xFF) as u8);
    assert_eq!(
        dev.handle.snapshot().channel2.latch.unwrap().hi_pending,
        true
    );
    assert_eq!(inb(&mut dev.pit, 2), (49_900u16 >> 8) as u8);
    assert_eq!(
        dev.handle.snapshot().channel2.latch,
        None,
        "one delivery only"
    );
    // Next reads are live: 49800 at tick 200.
    assert_eq!(inb(&mut dev.pit, 2), (49_800u16 & 0xFF) as u8);
    assert_eq!(inb(&mut dev.pit, 2), (49_800u16 >> 8) as u8);
    goto_tick(&clock, 300);
    // A fresh value appears only with a new latch; a second latch before the
    // first is read changes nothing.
    out(&mut dev.pit, 3, 0x80).unwrap();
    goto_tick(&clock, 400);
    out(&mut dev.pit, 3, 0x80).unwrap();
    assert_eq!(dev.handle.snapshot().channel2.latch.unwrap().value, 49_700);
    assert_eq!(inb(&mut dev.pit, 2), (49_700u16 & 0xFF) as u8);
    assert_eq!(inb(&mut dev.pit, 2), (49_700u16 >> 8) as u8);
    // The other channel has no latch.
    assert_eq!(dev.handle.snapshot().channel0.latch, None);
}

#[test]
fn counter_latch_follows_the_access_mode_and_a_control_word_clears_it() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0x10, 200, None); // LSB only
    program(&mut dev.pit, 0xA0, 0x20, None); // channel 2, MSB only
    goto_tick(&clock, 10);
    out(&mut dev.pit, 3, 0x00).unwrap();
    out(&mut dev.pit, 3, 0x80).unwrap();
    goto_tick(&clock, 60);
    assert_eq!(inb(&mut dev.pit, 0), 190, "one latched byte");
    assert_eq!(inb(&mut dev.pit, 0), 140, "then the live counter");
    assert_eq!(inb(&mut dev.pit, 2), ((0x2000 - 10) >> 8) as u8);
    assert_eq!(dev.handle.snapshot().channel2.latch, None);
    // A control word discards a pending latch.
    out(&mut dev.pit, 3, 0x00).unwrap();
    assert!(dev.handle.snapshot().channel0.latch.is_some());
    out(&mut dev.pit, 3, 0x10).unwrap();
    assert_eq!(
        dev.handle.snapshot().channel0,
        PitChannelSnapshot {
            mode: Some(PitMode::TerminalCount),
            access: Some(PitAccess::Lsb),
            ..PitChannelSnapshot::default()
        }
    );
}

#[test]
fn latch_without_a_count_is_refused_without_state_change() {
    let (mut dev, _clock) = setup();
    out(&mut dev.pit, 3, 0xB0).unwrap(); // programmed, but no reload yet
    for word in [0x00u8, 0x80] {
        let before = dev.handle.snapshot();
        assert_eq!(
            out(&mut dev.pit, 3, word),
            Err(DeviceError::UnsupportedConfiguration),
            "{word:#04x}"
        );
        assert_eq!(dev.handle.snapshot(), before);
    }
}

#[test]
fn live_lsb_msb_reads_alternate_and_a_control_word_resets_the_flip_flop() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0xB0, 0x00, Some(0x10)); // 4096
    goto_tick(&clock, 1);
    assert_eq!(inb(&mut dev.pit, 2), 0xFF);
    assert!(dev.handle.snapshot().channel2.read_hi);
    out(&mut dev.pit, 3, 0xB0).unwrap();
    assert!(!dev.handle.snapshot().channel2.read_hi);
}

#[test]
fn port_access_policy() {
    let (mut dev, _clock) = setup_ungated();
    let mmio = DeviceAccess {
        space: IoSpace::Mmio,
        offset: 0,
        width: BYTE,
    };
    assert_eq!(dev.pit.read(mmio), Err(DeviceError::UnsupportedSpace));
    assert_eq!(dev.pit.write(mmio, 0), Err(DeviceError::UnsupportedSpace));
    assert_eq!(dev.port_b.read(mmio), Err(DeviceError::UnsupportedSpace));
    assert_eq!(
        dev.port_b.write(mmio, 0),
        Err(DeviceError::UnsupportedSpace)
    );
    for width in [AccessWidth::Word, AccessWidth::Dword, AccessWidth::Qword] {
        assert_eq!(
            dev.pit.read(acc(0, width)),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            dev.pit.write(acc(3, width), 0),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            dev.port_b.read(acc(0, width)),
            Err(DeviceError::UnsupportedWidth)
        );
        assert_eq!(
            dev.port_b.write(acc(0, width), 0),
            Err(DeviceError::UnsupportedWidth)
        );
    }
    assert_eq!(dev.pit.read(acc(4, BYTE)), Err(DeviceError::UnknownOffset));
    assert_eq!(
        dev.pit.write(acc(4, BYTE), 0),
        Err(DeviceError::UnknownOffset)
    );
    assert_eq!(
        dev.port_b.read(acc(1, BYTE)),
        Err(DeviceError::UnknownOffset)
    );
    assert_eq!(
        dev.port_b.write(acc(1, BYTE), 0),
        Err(DeviceError::UnknownOffset)
    );
    assert_eq!(dev.handle.snapshot(), PitSnapshot::default());
}

#[test]
fn port_b_stores_gate_and_speaker_and_ignores_the_rest() {
    let (mut dev, _clock) = setup();
    let wr =
        |dev: &mut Pit8254Devices, v: u8| dev.port_b.write(acc(0, BYTE), u64::from(v)).unwrap();
    wr(&mut dev, 0x01);
    assert_eq!(port_b(&mut dev), 0x01);
    assert!(dev.handle.snapshot().gate2 && !dev.handle.snapshot().speaker_data);
    wr(&mut dev, 0x02);
    assert_eq!(port_b(&mut dev), 0x02);
    wr(&mut dev, 0x03);
    assert_eq!(port_b(&mut dev), 0x03);
    // Bits 2, 3, 6, 7 read 0; bits 4 and 5 are outputs of the model, not stored.
    wr(&mut dev, 0xFF);
    assert_eq!(port_b(&mut dev) & 0b1100_1100, 0);
    assert_eq!(port_b(&mut dev) & 0x03, 0x03);
    assert_eq!(
        port_b(&mut dev) & PORT_B_OUT2,
        0,
        "OUT2 is not what the guest wrote"
    );
    wr(&mut dev, 0x00);
    assert_eq!(port_b(&mut dev), 0);
    // The Linux read-modify-write `(inb(0x61) & ~0x02) | 0x01` writes bits 4/5 back.
    wr(&mut dev, 0x03);
    let v = (port_b(&mut dev) & !0x02) | 0x01;
    wr(&mut dev, v);
    assert_eq!(port_b(&mut dev), 0x01);
}

#[test]
fn port_b_bit5_is_out2_and_only_out2() {
    let (mut dev, clock) = setup();
    // Channel 0 high, channel 2 unprogrammed: bit 5 stays 0.
    program(&mut dev.pit, 0x34, 10, Some(0));
    assert!(dev.handle.out(PitChannel::Channel0));
    assert_eq!(port_b(&mut dev) & PORT_B_OUT2, 0);
    // Mode 0 on channel 2: bit 5 rises at terminal count (the gate is irrelevant).
    program(&mut dev.pit, 0xB0, 100, Some(0));
    goto_tick(&clock, 99);
    assert_eq!(port_b(&mut dev) & PORT_B_OUT2, 0);
    goto_tick(&clock, 100);
    assert_eq!(port_b(&mut dev) & PORT_B_OUT2, PORT_B_OUT2);
    assert_eq!(
        dev.port_b.write(acc(0, BYTE), 0x00),
        Err(DeviceError::UnsupportedConfiguration),
        "dropping gate 2 while channel 2 counts is refused (S4-29d)"
    );
    assert_eq!(
        port_b(&mut dev) & PORT_B_OUT2,
        PORT_B_OUT2,
        "a write cannot clear it"
    );
    // Mode 2 on channel 2: high, low for one tick.
    program(&mut dev.pit, 0xB4, 10, Some(0));
    let t0 = pit_ticks(clock.snapshot().now_ns());
    for k in [0u64, 3, 8, 9, 10, 19] {
        goto_tick(&clock, t0 + k);
        assert_eq!(
            port_b(&mut dev) & PORT_B_OUT2 != 0,
            k % 10 != 9,
            "tick offset {k}"
        );
    }
}

#[test]
fn port_b_bit4_toggles_every_18_ticks() {
    let (mut dev, clock) = setup_ungated();
    for (tick, bit) in [
        (0u64, 0u8),
        (17, 0),
        (18, 0x10),
        (35, 0x10),
        (36, 0),
        (54, 0x10),
        (72, 0),
    ] {
        goto_tick(&clock, tick);
        assert_eq!(port_b(&mut dev) & PORT_B_REFRESH, bit, "tick {tick}");
    }
    // Time-derived: it is not stored in the snapshot and does not disturb it.
    assert_eq!(dev.handle.snapshot(), PitSnapshot::default());
    // 18 ticks is 15.085 us.
    assert!((ns_for_tick(18) as i64 - 15_085).abs() < 2);
}

/// The sequence of Linux `pit_calibrate_tsc` driven through a real bus.
fn bus_with_everything() -> (MachineBus, Pit8254Handle, VirtualClockHandle) {
    let (dev, clock) = setup();
    let pair = Pic8259Pair::new();
    let (pm, _) = AcpiPm::new_with_handle(clock.clone());
    struct Stub;
    impl BusDevice for Stub {
        fn read(&mut self, _: DeviceAccess) -> Result<u64, DeviceError> {
            Ok(0)
        }
        fn write(&mut self, _: DeviceAccess, _: u64) -> Result<(), DeviceError> {
            Ok(())
        }
    }
    let devices: Vec<Box<dyn BusDevice>> = vec![
        Box::new(dev.pit),
        Box::new(dev.port_b),
        Box::new(pair.master),
        Box::new(pair.slave),
        Box::new(pm),
        Box::new(AbsentPorts::new(AbsentPortRegion::Cmos)),
        Box::new(AbsentPorts::new(AbsentPortRegion::IoDelay)),
        Box::new(Stub),
        Box::new(Stub),
    ];
    let range = |start: u16, len: u32| PortRange::new(PortAddress::new(start), len).unwrap();
    let ports: Vec<PortRegionBinding> = [
        (PIT_BASE_PORT, PIT_PORT_BYTES),
        (PIT_PORT_B, PIT_PORT_B_BYTES),
        (PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
        (PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
        (ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
        (
            AbsentPortRegion::Cmos.base_port(),
            AbsentPortRegion::Cmos.port_bytes(),
        ),
        (
            AbsentPortRegion::IoDelay.base_port(),
            AbsentPortRegion::IoDelay.port_bytes(),
        ),
        (FIRMWARE_COM1_PORT, 8),
        (PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES),
    ]
    .into_iter()
    .enumerate()
    .map(|(device_index, (start, len))| PortRegionBinding {
        range: range(start, len),
        device_index,
    })
    .collect();
    let map = MachineMemoryMap::new(
        36,
        vec![MemoryRegionSpec::new(
            GuestPhysicalRange::new(GuestPhysicalAddress::new(0), 1 << 20).unwrap(),
            MemoryRegionKind::Ram,
        )],
    )
    .unwrap();
    // `MachineBus::new` rejects overlapping port bindings: building proves the map.
    let bus = MachineBus::new(map, RamLimits::new(1 << 20), devices, Vec::new(), ports).unwrap();
    (bus, dev.handle, clock)
}

#[test]
fn ports_do_not_overlap_other_devices_and_fit_one_bus() {
    let (_bus, _handle, _clock) = bus_with_everything();
    let ours = [
        ("PIT 0x40..0x43", PIT_BASE_PORT, PIT_PORT_BYTES),
        ("port B 0x61", PIT_PORT_B, PIT_PORT_B_BYTES),
    ];
    let others = [
        ("PIC master", PIC_MASTER_BASE_PORT, PIC_PORT_BYTES),
        ("PIC slave", PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES),
        ("COM1", FIRMWARE_COM1_PORT, 8),
        ("conf1", PCI_CONF1_BASE_PORT, PCI_CONF1_PORT_BYTES),
        ("PM block", ACPI_PM_BASE_PORT, ACPI_PM_PORT_BYTES),
        (
            "CMOS",
            AbsentPortRegion::Cmos.base_port(),
            AbsentPortRegion::Cmos.port_bytes(),
        ),
        (
            "0xED",
            AbsentPortRegion::IoDelay.base_port(),
            AbsentPortRegion::IoDelay.port_bytes(),
        ),
    ];
    for &(mine, start, len) in &ours {
        for &(name, other, other_len) in &others {
            let a = u32::from(start)..u32::from(start) + len;
            let b = u32::from(other)..u32::from(other) + other_len;
            assert!(a.end <= b.start || b.end <= a.start, "{mine} vs {name}");
        }
    }
    assert!(u32::from(PIT_BASE_PORT) + PIT_PORT_BYTES <= u32::from(PIT_PORT_B));
}

#[test]
fn ports_through_the_machine_bus() {
    let (mut bus, handle, clock) = bus_with_everything();
    let p = PortAddress::new;
    // 0x40, 0x42 and 0x43 reach the counters; 0x41 and the control read are refused.
    bus.write_port(p(0x43), BYTE, 0x34).unwrap();
    bus.write_port(p(0x40), BYTE, 0x00).unwrap();
    bus.write_port(p(0x40), BYTE, 0x01).unwrap();
    assert_eq!(handle.snapshot().channel0.run.unwrap().reload, 256);
    assert!(matches!(
        bus.read_port(p(0x41), BYTE),
        Err(BusAccessError::Device {
            source: DeviceError::UnsupportedConfiguration,
            ..
        })
    ));
    assert!(matches!(
        bus.write_port(p(0x41), BYTE, 0),
        Err(BusAccessError::Device {
            source: DeviceError::UnsupportedConfiguration,
            ..
        })
    ));
    assert!(matches!(
        bus.read_port(p(0x43), BYTE),
        Err(BusAccessError::Device {
            source: DeviceError::WriteOnly,
            ..
        })
    ));
    // Refused control words surface as device errors and keep the snapshot.
    let before = handle.snapshot();
    for word in [0x31u32, 0x36, 0x70, 0xC2] {
        assert!(matches!(
            bus.write_port(p(0x43), BYTE, word),
            Err(BusAccessError::Device {
                source: DeviceError::UnsupportedConfiguration,
                ..
            })
        ));
    }
    assert_eq!(handle.snapshot(), before);
    // A word access to 0x40 reaches the device and is refused there.
    assert!(matches!(
        bus.read_port(p(0x40), AccessWidth::Word),
        Err(BusAccessError::Device {
            source: DeviceError::UnsupportedWidth,
            ..
        })
    ));
    // Neighbours are unmapped: 0x3F, 0x44, 0x60 (8042 trap), 0x62.
    for port in [0x3Fu16, 0x44, 0x60, 0x62] {
        assert_eq!(
            bus.read_port(p(port), BYTE),
            Err(BusAccessError::UnmappedPort(p(port))),
            "{port:#x}"
        );
    }
    // Port B through the bus.
    bus.write_port(p(0x61), BYTE, 0x03).unwrap();
    assert_eq!(bus.read_port(p(0x61), BYTE), Ok(0x03));
    assert!(handle.snapshot().gate2);
    let _ = clock;
}

#[test]
fn linux_pit_calibrate_tsc_sequence_through_the_bus() {
    let (mut bus, handle, clock) = bus_with_everything();
    let p = PortAddress::new;
    advance(&clock, 12_345);
    let start_tick = pit_ticks(clock.snapshot().now_ns());
    // outb((inb(0x61) & ~0x02) | 0x01, 0x61);
    let v = (bus.read_port(p(0x61), BYTE).unwrap() & !0x02) | 0x01;
    bus.write_port(p(0x61), BYTE, v).unwrap();
    // outb(0xb0, 0x43); outb(latch & 0xff, 0x42); outb(latch >> 8, 0x42); latch = 10 ms.
    let latch: u32 = 11_931;
    bus.write_port(p(0x43), BYTE, 0xB0).unwrap();
    bus.write_port(p(0x42), BYTE, latch & 0xFF).unwrap();
    bus.write_port(p(0x42), BYTE, latch >> 8).unwrap();
    assert_eq!(
        handle.snapshot().channel2.run.unwrap().start_tick,
        start_tick
    );
    // while ((inb(0x61) & 0x20) == 0) { advance }
    let mut polls = 0u32;
    while bus.read_port(p(0x61), BYTE).unwrap() & 0x20 == 0 {
        advance(&clock, 1_000);
        polls += 1;
        assert!(polls < 20_000, "terminal count never reached");
    }
    let end = clock.snapshot().now_ns();
    let edge = ns_for_tick(start_tick + u64::from(latch));
    assert!(
        end >= edge && end < edge + 1_000,
        "OUT2 rose at {end}, edge {edge}"
    );
    // The latch command and read, checked as a model behaviour (not a copy of a Linux sequence).
    bus.write_port(p(0x43), BYTE, 0x80).unwrap();
    let lo = bus.read_port(p(0x42), BYTE).unwrap();
    let hi = bus.read_port(p(0x42), BYTE).unwrap();
    let elapsed_past_zero = (0x1_0000 - ((hi << 8) | lo)) & 0xFFFF;
    assert!(
        elapsed_past_zero < 2,
        "counter wrapped past zero by <= 1 tick"
    );
}

// --- S4-29d: gate 2 refusals (consultation decision 6) ---------------------

#[test]
fn loading_channel2_with_gate2_low_is_refused_without_state_change() {
    let (mut dev, _clock) = setup_ungated();
    out(&mut dev.pit, 3, 0xB0).unwrap();
    let before = dev.handle.snapshot();
    for byte in [0x9Bu8, 0x2E] {
        assert_eq!(
            dev.pit.write(acc(2, BYTE), u64::from(byte)),
            Err(DeviceError::UnsupportedConfiguration)
        );
    }
    assert_eq!(dev.handle.snapshot(), before);
    // Channel 0 has no gate input and loads regardless.
    program(&mut dev.pit, 0x34, 10, Some(0));
    assert!(dev.handle.snapshot().channel0.run.is_some());
    // With gate 2 raised the same load is accepted.
    dev.port_b.write(acc(0, BYTE), 0x01).unwrap();
    out(&mut dev.pit, 2, 0x9B).unwrap();
    out(&mut dev.pit, 2, 0x2E).unwrap();
    assert_eq!(dev.handle.snapshot().channel2.run.unwrap().reload, 0x2E9B);
}

#[test]
fn dropping_gate2_while_channel2_counts_is_refused_and_the_count_keeps_running() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0xB0, 100, Some(0));
    let before = dev.handle.snapshot();
    assert_eq!(
        dev.port_b.write(acc(0, BYTE), 0x00),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        dev.port_b.write(acc(0, BYTE), 0x02),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.snapshot(), before);
    // The gate never stops the count; the count is unaffected by the refusals.
    goto_tick(&clock, 100);
    assert_eq!(port_b(&mut dev) & PORT_B_OUT2, PORT_B_OUT2);
    // Terminal count does not unload mode 0, so the gate stays pinned.
    assert_eq!(
        dev.port_b.write(acc(0, BYTE), 0x00),
        Err(DeviceError::UnsupportedConfiguration)
    );
    // Writing gate high (speaker data changes) is accepted.
    dev.port_b.write(acc(0, BYTE), 0x03).unwrap();
    // A new control word unloads the channel; then the gate may drop.
    out(&mut dev.pit, 3, 0xB0).unwrap();
    dev.port_b.write(acc(0, BYTE), 0x00).unwrap();
    assert!(!dev.handle.snapshot().gate2);
}

#[test]
fn channel2_observation_with_gate2_low_and_a_count_is_refused() {
    // The state is unreachable through the ports (loading needs gate 2 high and
    // dropping it is refused); it is forced to pin the defensive rule.
    let (mut dev, _clock) = setup();
    program(&mut dev.pit, 0xB0, 100, Some(0));
    out(&mut dev.pit, 3, 0x80).unwrap(); // a latch while gate 2 is high is fine
    assert_eq!(inb(&mut dev.pit, 2), 100);
    dev.pit.shared.lock().gate2 = false;
    let before = dev.handle.snapshot();
    assert_eq!(
        dev.pit.read(acc(2, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        dev.pit.write(acc(3, BYTE), 0x80),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(
        dev.port_b.read(acc(0, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.snapshot(), before);
    assert_eq!(dev.handle.port_b_reads(), 0, "a refused read is not a tick");
    // Channel 0 is not gated.
    program(&mut dev.pit, 0x34, 10, Some(0));
    out(&mut dev.pit, 3, 0x00).unwrap();
    // Without a count on channel 2, port B is readable at any gate level.
    out(&mut dev.pit, 3, 0xB0).unwrap();
    assert_eq!(port_b(&mut dev) & (PORT_B_GATE2 | PORT_B_OUT2), 0);
}

// --- S4-29d: the port tick (consultation decision 5, ISSUE-067) ------------

#[test]
fn every_port_b_read_queues_one_microsecond_and_the_device_does_not_move_the_clock() {
    let (mut dev, clock) = setup_ungated();
    assert_eq!(PIT_PORT_B_READ_TICK_NS, 1_000);
    assert_eq!(
        (dev.handle.port_b_reads(), dev.handle.take_port_tick_ns()),
        (0, 0)
    );
    for n in 1..=3u64 {
        port_b(&mut dev);
        assert_eq!(dev.handle.port_b_reads(), n);
    }
    assert_eq!(
        clock.snapshot().now_ns(),
        0,
        "the owner, not the device, advances"
    );
    assert_eq!(dev.handle.take_port_tick_ns(), 3_000);
    assert_eq!(dev.handle.take_port_tick_ns(), 0, "taking drains the queue");
    assert_eq!(dev.handle.port_b_reads(), 3, "the read count is cumulative");
    // Writes to port B, counter-port reads and control words are not time ports.
    dev.port_b.write(acc(0, BYTE), 0x01).unwrap();
    program(&mut dev.pit, 0xB0, 100, Some(0));
    out(&mut dev.pit, 3, 0x80).unwrap();
    inb(&mut dev.pit, 2);
    inb(&mut dev.pit, 2);
    assert_eq!(
        (dev.handle.port_b_reads(), dev.handle.take_port_tick_ns()),
        (3, 0)
    );
}

#[test]
fn the_value_is_sampled_before_the_tick_and_reset_drops_the_queue() {
    let (mut dev, clock) = setup();
    program(&mut dev.pit, 0xB0, 4, Some(0)); // OUT2 high at elapsed >= 4 ticks
    let start = pit_ticks(clock.snapshot().now_ns());
    // Owner loop: read, then apply the queued tick to the clock.
    let mut seen = Vec::new();
    for _ in 0..6 {
        seen.push(port_b(&mut dev) & PORT_B_OUT2 != 0);
        let ns = dev.handle.take_port_tick_ns();
        advance(&clock, ns);
    }
    // 1000 ns steps are 1.193 ticks: elapsed ticks at the reads are 0,1,2,3,4,5
    // (floor of k * 1.193182 minus the start tick at k = 0).
    let expected: Vec<bool> = (0..6u64)
        .map(|k| pit_ticks(k * 1_000) - start >= 4)
        .collect();
    assert_eq!(seen, expected);
    assert_eq!(seen, [false, false, false, false, true, true]);
    port_b(&mut dev);
    dev.handle.reset();
    assert_eq!(
        (dev.handle.port_b_reads(), dev.handle.take_port_tick_ns()),
        (0, 0)
    );
}

// --- S4-37a3: the counter-port tick on 0x42 (ISSUE-067) --------------------

fn setup_counter_tick() -> (Pit8254Devices, VirtualClockHandle) {
    let clock = VirtualClockHandle::new(1_000_000_000).unwrap();
    let mut dev = Pit8254Devices::new_with_counter_tick(clock.clone());
    dev.port_b.write(acc(0, BYTE), 0x01).unwrap();
    (dev, clock)
}

#[test]
fn counter_tick_is_off_by_default_and_on_for_the_board_constructor() {
    // `new` keeps the S4-29d accounting: 0x42 reads queue nothing.
    let (mut off, _) = setup();
    program(&mut off.pit, 0xB0, 100, Some(0));
    inb(&mut off.pit, 2);
    assert_eq!(off.handle.counter_port_reads(), 0);
    assert_eq!(off.handle.take_port_tick_ns(), 0);
    // The board constructor queues 1 us per channel 2 read.
    let (mut on, _) = setup_counter_tick();
    program(&mut on.pit, 0xB0, 100, Some(0));
    inb(&mut on.pit, 2);
    assert_eq!(on.handle.counter_port_reads(), 1);
    assert!(on.handle.port_tick_pending());
    assert_eq!(on.handle.take_port_tick_ns(), PIT_PORT_B_READ_TICK_NS);
    assert!(!on.handle.port_tick_pending());
}

#[test]
fn every_counter_read_of_channel_2_queues_one_microsecond_and_shares_the_queue_with_port_b() {
    let (mut dev, clock) = setup_counter_tick();
    program(&mut dev.pit, 0xB0, 0xFF, Some(0xFF));
    // Live bytes, then a latched pair: all four are counter reads.
    for _ in 0..2 {
        inb(&mut dev.pit, 2);
    }
    out(&mut dev.pit, 3, 0x80).unwrap();
    for _ in 0..2 {
        inb(&mut dev.pit, 2);
    }
    assert_eq!(dev.handle.counter_port_reads(), 4);
    assert_eq!(dev.handle.port_b_reads(), 0);
    port_b(&mut dev);
    assert_eq!(dev.handle.port_b_reads(), 1);
    assert_eq!(
        clock.snapshot().now_ns(),
        0,
        "the device never moves the clock"
    );
    assert_eq!(dev.handle.take_port_tick_ns(), 5_000, "4 x 0x42 + 1 x 0x61");
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
}

#[test]
fn the_counter_value_is_sampled_before_the_tick() {
    let (mut dev, clock) = setup_counter_tick();
    program(&mut dev.pit, 0xA0, 0x10, None); // MSB only, mode 0, reload 0x1000
    let first = inb(&mut dev.pit, 2);
    let second = inb(&mut dev.pit, 2);
    assert_eq!((first, second), (0x10, 0x10), "no clock move, no new value");
    advance(&clock, dev.handle.take_port_tick_ns());
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
    // Two queued reads are 2 us = 2 PIT ticks: the counter is 0x0FFE, MSB 0x0F.
    advance(&clock, 2 * PIT_PORT_B_READ_TICK_NS);
    let third = inb(&mut dev.pit, 2);
    assert_eq!(third, 0x0F);
}

#[test]
fn a_refused_counter_read_without_a_loaded_count_does_not_tick() {
    let (mut dev, _) = setup_counter_tick();
    out(&mut dev.pit, 3, 0xA0).unwrap(); // control word only, no count loaded
    assert_eq!(
        dev.pit.read(acc(2, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.counter_port_reads(), 0);
    assert!(!dev.handle.port_tick_pending());
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
}

#[test]
fn channel_0_reads_writes_control_words_and_refused_reads_do_not_tick() {
    let (mut dev, _) = setup_counter_tick();
    program(&mut dev.pit, 0x30, 10, Some(0));
    inb(&mut dev.pit, 0);
    inb(&mut dev.pit, 0);
    program(&mut dev.pit, 0xB0, 100, Some(0));
    out(&mut dev.pit, 3, 0x80).unwrap();
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
    // A refused read (channel 1, the write-only control port) is not a tick.
    assert!(dev.pit.read(acc(1, BYTE)).is_err());
    assert!(dev.pit.read(acc(3, BYTE)).is_err());
    assert_eq!(dev.handle.counter_port_reads(), 0);
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
    // Channel 2 holding a count with gate 2 low refuses the read (forced state).
    dev.handle.shared.lock().gate2 = false;
    assert_eq!(
        dev.pit.read(acc(2, BYTE)),
        Err(DeviceError::UnsupportedConfiguration)
    );
    assert_eq!(dev.handle.counter_port_reads(), 0);
    assert_eq!(dev.handle.take_port_tick_ns(), 0);
}

#[test]
fn reset_drops_the_counter_reads_and_the_queue_but_keeps_the_mode() {
    let (mut dev, _) = setup_counter_tick();
    program(&mut dev.pit, 0xB0, 100, Some(0));
    inb(&mut dev.pit, 2);
    dev.handle.reset();
    assert_eq!(dev.handle.counter_port_reads(), 0);
    assert!(!dev.handle.port_tick_pending());
    dev.port_b.write(acc(0, BYTE), 0x01).unwrap();
    program(&mut dev.pit, 0xB0, 100, Some(0));
    inb(&mut dev.pit, 2);
    assert_eq!(dev.handle.take_port_tick_ns(), PIT_PORT_B_READ_TICK_NS);
}
