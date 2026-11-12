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
//! LSB/MSB/LSB+MSB, counter latch and the channel 0 read-back command are
//! modelled. A control word with BCD, another mode, channel 1 or any other
//! read-back command is refused with [`DeviceError::UnsupportedConfiguration`]
//! before any state changes.
//!
//! # Read-back (S4-38L7bl)
//!
//! The semantics below are the Intel 8254 datasheet read from memory; the
//! primary source was not re-checked. A control word with `D7:D6 = 11` is the
//! read-back command: `D5` = `COUNT#` (0 latches the count), `D4` = `STATUS#`
//! (0 latches the status), `D3` selects channel 2, `D2` channel 1, `D1`
//! channel 0, and `D0` is reserved (0). Only the word with `D3:D0 = 0010`
//! (channel 0 alone, so `0xC2`, `0xD2`, `0xE2`, `0xF2`) is accepted; channel 1
//! or 2 selected, an empty selection and `D0 = 1` are refused. `0xF2` (neither
//! `COUNT#` nor `STATUS#` low) latches nothing.
//!
//! * The count latch is the ordinary counter latch above: a second latch
//!   before the first is read is ignored, and a channel without a count is
//!   refused.
//! * The status latch holds one byte taken at the command: `D7` = `OUT` (from
//!   the model's `OUT` computation), `D6` = null count, `D5:D4` = the
//!   access mode (`01` LSB, `10` MSB, `11` LSB+MSB), `D3:D1` = the mode (`000`
//!   or `010`), `D0` = BCD (always 0, BCD is refused). Null count is 1 while the
//!   channel holds no count (programmed but not loaded, or mode 0 stopped by a
//!   first byte) and, in mode 2, while a reload waits for the next period
//!   boundary; the model loads a count at the tick of its write, so a mode 0
//!   count is never reported null. A second status latch before the first is
//!   read is ignored; a channel that no control word programmed is refused.
//! * The next read of `0x40` returns the status byte (and clears the status
//!   latch); further reads return the latched count by the access mode, then
//!   the live counter. A control word for the channel clears both latches.
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
//! The gate input never stops or restarts channel 2: the count keeps running
//! regardless of gate 2 (S4-29d, consultation decision 6). What is refused
//! instead, with [`DeviceError::UnsupportedConfiguration`] and no state change:
//! a data-port write to channel 2 (loading it) while gate 2 is low; a counter
//! read, a counter latch, or a read of port B (which exposes `OUT2`) while
//! channel 2 holds a count and gate 2 is low; and a write to port B that
//! drops gate 2 while channel 2 holds a count. A read of port B while channel 2
//! holds no count is allowed at any gate level. Channel 0 has no gate input.
//!
//! # Port tick (S4-29d, consultation decision 5; ISSUE-067)
//!
//! Every successful read of port B is a "time port" read: the device counts it
//! and queues [`PIT_PORT_B_READ_TICK_NS`] (1 us) of virtual time. The value
//! returned by the read is sampled before the tick. The device does not move
//! the clock itself (the clock has one advancer, the vCPU session); the clock
//! owner takes the queued time with [`Pit8254Handle::take_port_tick_ns`] after
//! the instruction and advances the clock by it. The PM timer is a separate
//! device (S4-37a2).
//!
//! # Counter-port tick (S4-37a3; ISSUE-067)
//!
//! The same applies to every successful read of the channel 2 counter (`0x42`,
//! a live byte or a latched byte): [`PIT_PORT_B_READ_TICK_NS`] is queued in the
//! same queue, after the value is sampled, and the device still does not move the
//! clock. It is opt-in: [`Pit8254Devices::new`] leaves it off (the S4-29d probe
//! profile keeps its own `0x61`-only accounting); the Linux profile of the
//! firmware board builds the devices with [`Pit8254Devices::new_with_counter_tick`].
//! Channel 0 reads, writes and control words never tick; a refused read does not.

use crate::{AccessWidth, BusDevice, DeviceAccess, DeviceError, IoSpace, VirtualClockHandle};
use std::sync::atomic::{AtomicBool, Ordering};
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
/// Virtual time added by one read of port B (the port tick), in nanoseconds.
pub const PIT_PORT_B_READ_TICK_NS: u64 = 1_000;
/// Input ticks between two toggles of port B bit 4.
pub const PIT_REFRESH_TOGGLE_TICKS: u64 = 18;

const NANOS_PER_SECOND: u128 = 1_000_000_000;
const PORT_B_GATE2: u8 = 1 << 0;
const PORT_B_SPEAKER: u8 = 1 << 1;
const PORT_B_REFRESH: u8 = 1 << 4;
const PORT_B_OUT2: u8 = 1 << 5;
const CTRL_BCD: u8 = 1 << 0;
/// Read-back word: `D5` = `COUNT#`, `D4` = `STATUS#`, `D3:D0` = channel 2,
/// channel 1, channel 0, reserved.
const READ_BACK_NOT_COUNT: u8 = 1 << 5;
const READ_BACK_NOT_STATUS: u8 = 1 << 4;
const READ_BACK_TARGET_MASK: u8 = 0x0F;
const READ_BACK_CHANNEL0: u8 = 0b0010;

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
    /// Read-back status byte waiting to be read (S4-38L7bl); it is delivered
    /// before the count latch.
    pub status_latch: Option<u8>,
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

    /// Whether the count register holds a value the counting element has not
    /// loaded yet: no count at all, or a mode 2 reload waiting for its boundary.
    fn null_count(&self, tick: u64) -> bool {
        match self.run {
            None => true,
            Some(run) => run.next.is_some_and(|next| tick < next.start_tick),
        }
    }

    /// The read-back status byte at `tick`; `None` until a control word
    /// programmed the channel.
    fn status(&self, tick: u64) -> Option<u8> {
        let (mode, access) = self.mode.zip(self.access)?;
        let access_bits = match access {
            PitAccess::Lsb => 0b01,
            PitAccess::Msb => 0b10,
            PitAccess::LsbMsb => 0b11,
        };
        let mode_bits = match mode {
            PitMode::TerminalCount => 0b000,
            PitMode::RateGenerator => 0b010,
        };
        Some(
            u8::from(self.out(tick)) << 7
                | u8::from(self.null_count(tick)) << 6
                | access_bits << 4
                | mode_bits << 1,
        )
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
        if let Some(status) = self.status_latch.take() {
            return Ok(status);
        }
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
            3 => return self.read_back(word, tick),
            // Channel 1 is not modelled.
            _ => return Err(DeviceError::UnsupportedConfiguration),
        };
        let access = match (word >> 4) & 3 {
            0 => {
                // Counter latch; the low nibble is ignored by the chip.
                if channel == PitChannel::Channel2 && self.channel2_ungated_with_count() {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
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

    /// The read-back command (`D7:D6 = 11`), channel 0 only. Both latches are
    /// validated before either is set.
    fn read_back(&mut self, word: u8, tick: u64) -> Result<(), DeviceError> {
        if word & READ_BACK_TARGET_MASK != READ_BACK_CHANNEL0 {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let ch = &mut self.channel0;
        let status = if word & READ_BACK_NOT_STATUS == 0 && ch.status_latch.is_none() {
            Some(
                ch.status(tick)
                    .ok_or(DeviceError::UnsupportedConfiguration)?,
            )
        } else {
            None
        };
        let count = if word & READ_BACK_NOT_COUNT == 0 && ch.latch.is_none() {
            Some(
                ch.count(tick)
                    .ok_or(DeviceError::UnsupportedConfiguration)?,
            )
        } else {
            None
        };
        if let Some(status) = status {
            ch.status_latch = Some(status);
        }
        if let Some(value) = count {
            ch.latch = Some(PitLatch {
                value,
                hi_pending: false,
            });
        }
        Ok(())
    }

    /// Channel 2 holds a count while gate 2 is low: its counter and `OUT2` are
    /// not observable and a port B write may not drop the gate (decision 6).
    fn channel2_ungated_with_count(&self) -> bool {
        !self.gate2 && self.channel2.run.is_some()
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

/// Time-port reads seen by the device and the virtual time queued by them.
#[derive(Clone, Copy, Debug, Default)]
struct PortTick {
    /// Successful port B reads.
    reads: u64,
    /// Successful channel 2 counter reads that queued a tick (S4-37a3).
    counter_reads: u64,
    pending_ns: u64,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<PitSnapshot>,
    port_tick: Mutex<PortTick>,
    /// Whether a channel 2 counter read queues a port tick (S4-37a3).
    counter_tick: AtomicBool,
    clock: VirtualClockHandle,
}

impl Shared {
    fn tick(&self) -> u64 {
        pit_ticks(self.clock.snapshot().now_ns())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PitSnapshot> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn lock_port_tick(&self) -> std::sync::MutexGuard<'_, PortTick> {
        self.port_tick.lock().unwrap_or_else(|e| e.into_inner())
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

    /// Successful reads of port B since construction or reset.
    pub fn port_b_reads(&self) -> u64 {
        self.shared.lock_port_tick().reads
    }

    /// Successful channel 2 counter reads (`0x42`) that queued a tick since
    /// construction or reset (S4-37a3).
    pub fn counter_port_reads(&self) -> u64 {
        self.shared.lock_port_tick().counter_reads
    }

    /// Whether a time-port read has queued time that nobody has taken yet.
    pub fn port_tick_pending(&self) -> bool {
        self.shared.lock_port_tick().pending_ns != 0
    }

    /// Test injection (S4-37a3): switches the `0x42` tick on or off. Never
    /// called in production code.
    #[cfg(test)]
    pub(crate) fn set_counter_port_tick(&self, enabled: bool) {
        self.shared.counter_tick.store(enabled, Ordering::Relaxed);
    }

    /// Takes the virtual time queued by port B reads and, when enabled, by
    /// channel 2 counter reads (the port tick), leaving zero. The clock owner
    /// advances its clock by the returned amount.
    pub fn take_port_tick_ns(&self) -> u64 {
        std::mem::take(&mut self.shared.lock_port_tick().pending_ns)
    }

    /// Machine reset: counters uninitialised, `OUT` low (mode 0) or high
    /// (mode 2) only after a control word, gate 2 and speaker data cleared,
    /// the port B read count, the counter-port read count and the queued port
    /// tick dropped.
    pub fn reset(&self) {
        *self.shared.lock() = PitSnapshot::default();
        *self.shared.lock_port_tick() = PortTick::default();
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
    /// The devices with the port tick on `0x61` only.
    pub fn new(clock: VirtualClockHandle) -> Self {
        Self::build(clock, false)
    }

    /// As [`Self::new`], plus the port tick on channel 2 counter reads `0x42`
    /// (S4-37a3, the Linux profile of the firmware board).
    pub fn new_with_counter_tick(clock: VirtualClockHandle) -> Self {
        Self::build(clock, true)
    }

    fn build(clock: VirtualClockHandle, counter_tick: bool) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(PitSnapshot::default()),
            port_tick: Mutex::new(PortTick::default()),
            counter_tick: AtomicBool::new(counter_tick),
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
        if channel == PitChannel::Channel2 && state.channel2_ungated_with_count() {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let mut next = *state;
        let byte = next.channel_mut(channel).read_data(tick)?;
        *state = next;
        drop(state);
        // The byte is sampled; a channel 2 read then costs one port tick (S4-37a3).
        if channel == PitChannel::Channel2 && self.shared.counter_tick.load(Ordering::Relaxed) {
            let mut port_tick = self.shared.lock_port_tick();
            port_tick.counter_reads += 1;
            port_tick.pending_ns += PIT_PORT_B_READ_TICK_NS;
        }
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
            2 => {
                // Loading channel 2 needs gate 2 high (decision 6).
                if !next.gate2 {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                next.channel_mut(PitChannel::Channel2)
                    .write_data(byte, tick)?
            }
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
        let value = {
            let state = self.shared.lock();
            if state.channel2_ungated_with_count() {
                return Err(DeviceError::UnsupportedConfiguration);
            }
            state.port_b(tick)
        };
        // The value is sampled; the read then costs one port tick.
        let mut port_tick = self.shared.lock_port_tick();
        port_tick.reads += 1;
        port_tick.pending_ns += PIT_PORT_B_READ_TICK_NS;
        Ok(u64::from(value))
    }

    fn write(&mut self, access: DeviceAccess, value: u64) -> Result<(), DeviceError> {
        check_byte_pio(access, PIT_PORT_B_BYTES)?;
        let byte = value as u8;
        let mut state = self.shared.lock();
        let gate2 = byte & PORT_B_GATE2 != 0;
        // Dropping gate 2 while channel 2 counts is refused (decision 6).
        if !gate2 && state.channel2.run.is_some() {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        state.gate2 = gate2;
        state.speaker_data = byte & PORT_B_SPEAKER != 0;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
