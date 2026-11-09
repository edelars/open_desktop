//! Intel 8254 programmable interval timer, channels 0 and 2, and the system
//! control port B `0x61` (S4-29c). There are no IRQ lines and no events: the
//! model only answers port accesses, computing every counter from the shared
//! [`VirtualClockHandle`] at the moment of the access.
//!
//! # Counting formula
//!
//! The PIT input runs at [`PIT_INPUT_HZ`] = 1 193 182 Hz, free-running from
//! virtual time 0. The tick index at virtual time `t` is
//! `T(t) = floor(t_ns * 1_193_182 / 1_000_000_000)` (one `u128` division of the
//! absolute time, never a sum of per-step results, so stepping the clock in
//! many small steps cannot drift from one big step). A reload completes at
//! tick `T0 = T(t_write)`; the elapsed count is `e = T(now) - T0`, an integer
//! number of input-clock edges crossed since the load, as on the chip whose
//! oscillator does not restart with the write. The effective reload `N` is the
//! written 16-bit value, with 0 meaning 65536.
//!
//! * Mode 0 (interrupt on terminal count): the counter reads
//!   `(N - e) mod 65536` (it keeps counting through zero), `OUT` is low until
//!   `e >= N` and high afterwards, until the next write.
//! * Mode 2 (rate generator): with `p = e mod N` the counter reads `N - p`
//!   (`1..=N`), `OUT` is high except for one input tick, when the counter is
//!   `1` (`p = N - 1`), just before the reload.
//!
//! Only binary counting, modes 0 and 2, channels 0 and 2, access modes
//! LSB/MSB/LSB+MSB and counter latch are modelled. A control word with BCD,
//! another mode, channel 1 or the read-back command is refused with
//! [`DeviceError::UnsupportedConfiguration`] before any state changes.
//!
//! # Port B (`0x61`)
//!
//! Bit 0 (gate 2) and bit 1 (speaker data) are stored. Bit 5 is the current
//! `OUT` of channel 2. Bit 4 (refresh toggle) is not stored: it is derived from
//! the clock as `(T(now) / 18) & 1`, the output of the refresh channel 1 that
//! the real machine programs as mode 2 with count 18. Bits 2, 3, 6 and 7
//! (parity/IOCHK) read 0; a write to them, and to the read-only bits 4 and 5,
//! is ignored, so the usual read-modify-write of Linux works.
//!
//! The gate input does not stop or restart channel 2 in this slice (it is
//! stored and visible only in port B), and channel 0 has no gate input at all.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, VirtualClockHandle};
use std::sync::{Arc, Mutex};

/// First port of the counters (`0x40` channel 0, `0x42` channel 2, `0x43` control).
pub const PIT_BASE_PORT: u16 = 0x40;
/// Ports of the counter block `0x40..=0x43`.
pub const PIT_PORT_BYTES: u32 = 4;
/// System control port B.
pub const PIT_PORT_B: u16 = 0x61;
/// Ports of the port B binding.
pub const PIT_PORT_B_BYTES: u32 = 1;
/// PIT input clock in Hz (the project value, rounded from 1 193 181.8).
pub const PIT_INPUT_HZ: u64 = 1_193_182;
/// Input ticks between two toggles of port B bit 4.
pub const PIT_REFRESH_TOGGLE_TICKS: u64 = 18;

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const PORT_B_GATE2: u8 = 1 << 0;
const PORT_B_SPEAKER: u8 = 1 << 1;
const PORT_B_REFRESH: u8 = 1 << 4;
const PORT_B_OUT2: u8 = 1 << 5;
const CTRL_BCD: u8 = 1 << 0;

/// Tick index of the PIT input clock at `now_ns`: `floor(now_ns * f / 1e9)`.
pub const fn pit_ticks(now_ns: u64) -> u64 {
    (now_ns as u128 * PIT_INPUT_HZ as u128 / NANOS_PER_SECOND) as u64
}

/// The two modelled channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PitChannel {
    Channel0,
    Channel2,
}

/// Operating mode (`M2:M0` of the control word).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PitMode {
    /// Mode 0: interrupt on terminal count.
    TerminalCount,
    /// Mode 2: rate generator.
    RateGenerator,
}

/// Read/write access mode (`RW1:RW0` of the control word).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PitAccess {
    Lsb,
    Msb,
    LsbMsb,
}

/// A count in progress. `reload` is the effective reload (`1..=65536`) and
/// `start_tick` the tick at which it completed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PitRun {
    pub reload: u32,
    pub start_tick: u64,
    /// Mode 2 only: a reload written while counting takes effect at the next
    /// period boundary.
    pub next: Option<PitNextReload>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PitNextReload {
    pub reload: u32,
    pub start_tick: u64,
}

/// A counter latch waiting to be read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PitLatch {
    pub value: u16,
    /// LSB+MSB: the low byte has been delivered, the high byte is next.
    pub hi_pending: bool,
}

/// Guest-visible state of one channel.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PitChannelSnapshot {
    /// `None` until a control word programmed the channel.
    pub mode: Option<PitMode>,
    pub access: Option<PitAccess>,
    /// LSB+MSB: the low byte of a reload written and not yet completed.
    pub pending_lsb: Option<u8>,
    /// `None`: no counter value (not loaded, or mode 0 stopped by a new reload).
    pub run: Option<PitRun>,
    pub latch: Option<PitLatch>,
    /// LSB+MSB live read: the high byte is next.
    pub read_hi: bool,
}

/// Guest-visible state of the whole block; equality of two snapshots is the
/// "no state change" check of the refused accesses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PitSnapshot {
    pub channel0: PitChannelSnapshot,
    pub channel2: PitChannelSnapshot,
    pub gate2: bool,
    pub speaker_data: bool,
}

impl PitChannelSnapshot {
    /// The reload and start tick in force at `tick`.
    fn segment(&self, tick: u64) -> Option<(u32, u64)> {
        let run = self.run?;
        Some(match run.next {
            Some(next) if tick >= next.start_tick => (next.reload, next.start_tick),
            _ => (run.reload, run.start_tick),
        })
    }

    /// Folds an already effective pending reload into `run`.
    fn normalize(&mut self, tick: u64) {
        if let Some(run) = &mut self.run
            && let Some(next) = run.next
            && tick >= next.start_tick
        {
            *run = PitRun {
                reload: next.reload,
                start_tick: next.start_tick,
                next: None,
            };
        }
    }

    fn out(&self, tick: u64) -> bool {
        match (self.mode, self.segment(tick)) {
            (None, _) | (Some(PitMode::TerminalCount), None) => false,
            (Some(PitMode::RateGenerator), None) => true,
            (Some(PitMode::TerminalCount), Some((reload, start))) => {
                tick - start >= u64::from(reload)
            }
            (Some(PitMode::RateGenerator), Some((reload, start))) => {
                (tick - start) % u64::from(reload) != u64::from(reload) - 1
            }
        }
    }

    fn count(&self, tick: u64) -> Option<u16> {
        let (reload, start) = self.segment(tick)?;
        let e = tick - start;
        Some(match self.mode? {
            PitMode::TerminalCount => (u64::from(reload).wrapping_sub(e) & 0xFFFF) as u16,
            PitMode::RateGenerator => (u64::from(reload) - e % u64::from(reload)) as u16,
        })
    }

    /// One byte written to the channel's data port at `tick`.
    fn write_data(&mut self, byte: u8, tick: u64) -> Result<(), DeviceError> {
        let (mode, access) = self
            .mode
            .zip(self.access)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        let reload = |value: u16| if value == 0 { 65_536 } else { u32::from(value) };
        match access {
            PitAccess::Lsb => self.complete(mode, reload(u16::from(byte)), tick),
            PitAccess::Msb => self.complete(mode, reload(u16::from(byte) << 8), tick),
            PitAccess::LsbMsb => match self.pending_lsb.take() {
                Some(lsb) => {
                    self.complete(mode, reload(u16::from(lsb) | u16::from(byte) << 8), tick)
                }
                None => {
                    self.pending_lsb = Some(byte);
                    if mode == PitMode::TerminalCount {
                        // The first byte of a new count stops mode 0 and drops OUT.
                        self.run = None;
                    }
                    Ok(())
                }
            },
        }
    }

    fn complete(&mut self, mode: PitMode, reload: u32, tick: u64) -> Result<(), DeviceError> {
        match mode {
            PitMode::TerminalCount => {
                self.run = Some(PitRun {
                    reload,
                    start_tick: tick,
                    next: None,
                });
            }
            PitMode::RateGenerator => {
                // A count of 1 is not a valid rate generator count.
                if reload == 1 {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                self.normalize(tick);
                match &mut self.run {
                    None => {
                        self.run = Some(PitRun {
                            reload,
                            start_tick: tick,
                            next: None,
                        });
                    }
                    Some(run) => {
                        let period = u64::from(run.reload);
                        let boundary =
                            run.start_tick + ((tick - run.start_tick) / period + 1) * period;
                        run.next = Some(PitNextReload {
                            reload,
                            start_tick: boundary,
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn read_data(&mut self, tick: u64) -> Result<u8, DeviceError> {
        if let Some(latch) = self.latch {
            let byte = match self.access {
                Some(PitAccess::Msb) => {
                    self.latch = None;
                    (latch.value >> 8) as u8
                }
                Some(PitAccess::LsbMsb) if !latch.hi_pending => {
                    self.latch = Some(PitLatch {
                        hi_pending: true,
                        ..latch
                    });
                    latch.value as u8
                }
                Some(PitAccess::LsbMsb) => {
                    self.latch = None;
                    (latch.value >> 8) as u8
                }
                _ => {
                    self.latch = None;
                    latch.value as u8
                }
            };
            return Ok(byte);
        }
        let value = self
            .count(tick)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        Ok(
            match self.access.ok_or(DeviceError::UnsupportedConfiguration)? {
                PitAccess::Lsb => value as u8,
                PitAccess::Msb => (value >> 8) as u8,
                PitAccess::LsbMsb => {
                    let hi = self.read_hi;
                    self.read_hi = !hi;
                    if hi { (value >> 8) as u8 } else { value as u8 }
                }
            },
        )
    }
}

impl PitSnapshot {
    fn channel_mut(&mut self, channel: PitChannel) -> &mut PitChannelSnapshot {
        match channel {
            PitChannel::Channel0 => &mut self.channel0,
            PitChannel::Channel2 => &mut self.channel2,
        }
    }

    fn channel(&self, channel: PitChannel) -> &PitChannelSnapshot {
        match channel {
            PitChannel::Channel0 => &self.channel0,
            PitChannel::Channel2 => &self.channel2,
        }
    }

    /// The control word at port `0x43`, validated completely before it applies.
    fn write_control(&mut self, word: u8, tick: u64) -> Result<(), DeviceError> {
        let channel = match word >> 6 {
            0 => PitChannel::Channel0,
            2 => PitChannel::Channel2,
            // Channel 1 is not modelled; `0b11` is the read-back command.
            _ => return Err(DeviceError::UnsupportedConfiguration),
        };
        let access = match (word >> 4) & 3 {
            0 => {
                // Counter latch; the low nibble is ignored by the chip.
                let ch = self.channel_mut(channel);
                if ch.latch.is_some() {
                    // A second latch before the first is read has no effect.
                    return Ok(());
                }
                let value = ch
                    .count(tick)
                    .ok_or(DeviceError::UnsupportedConfiguration)?;
                ch.latch = Some(PitLatch {
                    value,
                    hi_pending: false,
                });
                return Ok(());
            }
            1 => PitAccess::Lsb,
            2 => PitAccess::Msb,
            _ => PitAccess::LsbMsb,
        };
        if word & CTRL_BCD != 0 {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let mode = match (word >> 1) & 7 {
            0 => PitMode::TerminalCount,
            2 => PitMode::RateGenerator,
            _ => return Err(DeviceError::UnsupportedConfiguration),
        };
        // A control word resets the channel: no count, no latch, flip-flops cleared.
        *self.channel_mut(channel) = PitChannelSnapshot {
            mode: Some(mode),
            access: Some(access),
            ..PitChannelSnapshot::default()
        };
        Ok(())
    }

    fn port_b(&self, tick: u64) -> u8 {
        let mut value = 0;
        if self.gate2 {
            value |= PORT_B_GATE2;
        }
        if self.speaker_data {
            value |= PORT_B_SPEAKER;
        }
        if (tick / PIT_REFRESH_TOGGLE_TICKS) & 1 != 0 {
            value |= PORT_B_REFRESH;
        }
        if self.channel2.out(tick) {
            value |= PORT_B_OUT2;
        }
        value
    }
}

#[derive(Debug)]
struct Shared {
    state: Mutex<PitSnapshot>,
    clock: VirtualClockHandle,
}

impl Shared {
    fn tick(&self) -> u64 {
        pit_ticks(self.clock.snapshot().now_ns())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PitSnapshot> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Cloneable observer of the PIT block and port B.
#[derive(Clone, Debug)]
pub struct Pit8254Handle {
    shared: Arc<Shared>,
}

impl Pit8254Handle {
    pub fn snapshot(&self) -> PitSnapshot {
        *self.shared.lock()
    }

    /// `OUT` of a channel at the current virtual time.
    pub fn out(&self, channel: PitChannel) -> bool {
        let tick = self.shared.tick();
        self.shared.lock().channel(channel).out(tick)
    }

    /// The counter of a channel at the current virtual time; `None` while the
    /// channel has no count.
    pub fn count(&self, channel: PitChannel) -> Option<u16> {
        let tick = self.shared.tick();
        self.shared.lock().channel(channel).count(tick)
    }

    /// Machine reset: counters uninitialised, `OUT` low (mode 0) or high
    /// (mode 2) only after a control word, gate 2 and speaker data cleared.
    pub fn reset(&self) {
        *self.shared.lock() = PitSnapshot::default();
    }
}

/// The counter block, bound to `0x40..=0x43`.
pub struct Pit8254 {
    shared: Arc<Shared>,
}

/// System control port B, bound to `0x61`.
pub struct PitPortB {
    shared: Arc<Shared>,
}

/// The two bus devices of the slice with their observer. They are separate
/// devices because a device sees offsets relative to its own binding.
pub struct Pit8254Devices {
    pub pit: Pit8254,
    pub port_b: PitPortB,
    pub handle: Pit8254Handle,
}

impl Pit8254Devices {
    pub fn new(clock: VirtualClockHandle) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(PitSnapshot::default()),
            clock,
        });
        Self {
            pit: Pit8254 {
                shared: Arc::clone(&shared),
            },
            port_b: PitPortB {
                shared: Arc::clone(&shared),
            },
            handle: Pit8254Handle { shared },
        }
    }
}

fn check_byte_pio(access: DeviceAccess, ports: u32) -> Result<(), DeviceError> {
    if access.space != IoSpace::Pio {
        return Err(DeviceError::UnsupportedSpace);
    }
    if access.width != AccessWidth::Byte {
        return Err(DeviceError::UnsupportedWidth);
    }
    if access.offset >= u64::from(ports) {
        return Err(DeviceError::UnknownOffset);
    }
    Ok(())
}

impl BusDevice for Pit8254 {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        check_byte_pio(access, PIT_PORT_BYTES)?;
        let channel = match access.offset {
            0 => PitChannel::Channel0,
            2 => PitChannel::Channel2,
            1 => return Err(DeviceError::UnsupportedConfiguration),
            _ => return Err(DeviceError::WriteOnly),
        };
        let tick = self.shared.tick();
        let mut state = self.shared.lock();
        let mut next = *state;
        let byte = next.channel_mut(channel).read_data(tick)?;
        *state = next;
        Ok(u64::from(byte))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        check_byte_pio(access, PIT_PORT_BYTES)?;
        let byte = value as u8;
        let tick = self.shared.tick();
        let mut state = self.shared.lock();
        // Work on a copy: a refused access must leave the state untouched.
        let mut next = *state;
        match access.offset {
            0 => next
                .channel_mut(PitChannel::Channel0)
                .write_data(byte, tick)?,
            2 => next
                .channel_mut(PitChannel::Channel2)
                .write_data(byte, tick)?,
            1 => return Err(DeviceError::UnsupportedConfiguration),
            _ => next.write_control(byte, tick)?,
        }
        *state = next;
        Ok(())
    }
}

impl BusDevice for PitPortB {
    fn read(&mut self, access: DeviceAccess) -> Result<u64, DeviceError> {
        check_byte_pio(access, PIT_PORT_B_BYTES)?;
        let tick = self.shared.tick();
        Ok(u64::from(self.shared.lock().port_b(tick)))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        check_byte_pio(access, PIT_PORT_B_BYTES)?;
        let byte = value as u8;
        let mut state = self.shared.lock();
        state.gate2 = byte & PORT_B_GATE2 != 0;
        state.speaker_data = byte & PORT_B_SPEAKER != 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
