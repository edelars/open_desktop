//! Atomic state transitions for the single Local APIC and INTIN4 route.
use crate::{DeviceError, EventQueue, VirtualClock, VirtualClockHandle, local_apic::State};

pub(crate) const MASK: u32 = 1 << 16;
pub(crate) const PERIODIC: u32 = 1 << 17;
pub(crate) const LEVEL: u32 = 1 << 15;
pub(crate) const REMOTE_IRR: u32 = 1 << 14;
/// Number of IOAPIC inputs (MRE = 23); MADT (S4-31f) declares the same count.
pub(crate) const IO_APIC_PINS: usize = 24;
/// IOAPIC ID loaded at reset: the value the firmware MADT declares (S4-31f).
pub(crate) const IO_APIC_RESET_ID: u8 = 1;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Route {
    pub selector: u8,
    pub low: u32,
    pub high_line: bool,
    /// Input level of INTIN2 (S4-29e); edge only, no Remote IRR.
    pub intin2_high: bool,
    pub remote_irr: bool,
    /// IOAPIC ID register bits 27:24 (4 bits); Arbitration ID mirrors it.
    pub id: u8,
    /// Stored low dwords of entries other than INTIN4 (no input line, never delivers).
    pub pins: [u32; IO_APIC_PINS],
}
impl Default for Route {
    fn default() -> Self {
        Self {
            selector: 0,
            low: MASK,
            high_line: false,
            intin2_high: false,
            remote_irr: false,
            id: IO_APIC_RESET_ID,
            pins: [MASK; IO_APIC_PINS],
        }
    }
}
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct PairState {
    pub local: State,
    pub route: Route,
    pub timer: Option<Timer>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct Timer {
    pub clock: VirtualClockHandle,
    pub can_advance_clock: bool,
    pub hz: u64,
    pub queue: EventQueue,
    pub lvt: u32,
    pub initial: u32,
    pub divide: u32,
    pub start: u64,
}

impl Timer {
    pub fn new(clock: VirtualClock, hz: u64) -> Result<Self, DeviceError> {
        Self::new_with_clock(VirtualClockHandle::from(clock), hz, true)
    }

    pub fn new_shared(clock: VirtualClockHandle, hz: u64) -> Result<Self, DeviceError> {
        Self::new_with_clock(clock, hz, false)
    }

    fn new_with_clock(
        clock: VirtualClockHandle,
        hz: u64,
        can_advance_clock: bool,
    ) -> Result<Self, DeviceError> {
        if hz == 0 {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        Ok(Self {
            clock,
            can_advance_clock,
            hz,
            queue: EventQueue::new(1),
            lvt: MASK,
            initial: 0,
            divide: 0,
            start: 0,
        })
    }
    fn divisor(&self) -> u128 {
        let n = (self.divide & 3) | ((self.divide >> 1) & 4);
        if n == 7 { 1 } else { 2u128 << n }
    }
    pub fn current(&self) -> u32 {
        if self.queue.is_empty() {
            return 0;
        }
        let elapsed = u128::from(self.clock.snapshot().now_ns() - self.start);
        let ticks = elapsed * u128::from(self.hz) / (1_000_000_000u128 * self.divisor());
        if self.lvt & PERIODIC != 0 {
            (u128::from(self.initial) - ticks % u128::from(self.initial)) as u32
        } else {
            u128::from(self.initial).saturating_sub(ticks) as u32
        }
    }
    pub fn due_at(&self, now_ns: u64) -> bool {
        self.queue.next_deadline().is_some_and(|d| d <= now_ns)
    }
    pub fn next_periodic_deadline(&self, now_ns: u64) -> Result<u64, DeviceError> {
        let period = u128::from(self.initial)
            .checked_mul(self.divisor())
            .and_then(|v| v.checked_mul(1_000_000_000))
            .filter(|v| *v != 0)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        let elapsed = u128::from(now_ns - self.start)
            .checked_mul(u128::from(self.hz))
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        let remaining = period - elapsed % period;
        let duration = remaining.div_ceil(u128::from(self.hz));
        let deadline = u128::from(now_ns)
            .checked_add(duration)
            .ok_or(DeviceError::UnsupportedConfiguration)?;
        u64::try_from(deadline).map_err(|_| DeviceError::UnsupportedConfiguration)
    }
    pub fn arm_at(&mut self, now_ns: u64, count: u32) -> Result<(), DeviceError> {
        let deadline = if count == 0 {
            None
        } else {
            let numerator = u128::from(count)
                .checked_mul(self.divisor())
                .and_then(|v| v.checked_mul(1_000_000_000))
                .ok_or(DeviceError::UnsupportedConfiguration)?;
            let hz = u128::from(self.hz);
            let duration = numerator.div_ceil(hz);
            let deadline = u128::from(now_ns)
                .checked_add(duration)
                .ok_or(DeviceError::UnsupportedConfiguration)?;
            Some(u64::try_from(deadline).map_err(|_| DeviceError::UnsupportedConfiguration)?)
        };
        self.queue
            .replace_single(self.queue.generation(), deadline)
            .map_err(|_| DeviceError::UnsupportedConfiguration)?;
        self.initial = count;
        self.start = now_ns;
        Ok(())
    }
    pub fn reset(&mut self) -> Result<(), DeviceError> {
        self.queue
            .replace_single(self.queue.generation(), None)
            .map_err(|_| DeviceError::UnsupportedConfiguration)?;
        self.lvt = MASK;
        self.initial = 0;
        self.divide = 0;
        self.start = self.clock.snapshot().now_ns();
        Ok(())
    }
}
impl PairState {
    pub fn reset_local(&mut self) -> Result<(), DeviceError> {
        if self.route.remote_irr {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        if let Some(timer) = &mut self.timer {
            timer.reset()?;
        }
        self.local = State::default();
        Ok(())
    }

    pub fn reset_io(&mut self) -> Result<(), DeviceError> {
        if self.route.remote_irr {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route = Route::default();
        Ok(())
    }

    fn bit(vector: u8) -> (usize, u32) {
        (usize::from(vector / 32), 1 << (vector % 32))
    }
    fn busy(&self, vector: u8) -> bool {
        let (i, bit) = Self::bit(vector);
        (self.local.irr[i] | self.local.isr[i]) & bit != 0
    }
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), DeviceError> {
        if self.local.global_enabled != enabled {
            if !enabled {
                if self.route.remote_irr {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                if let Some(timer) = &mut self.timer {
                    timer.reset()?;
                }
                self.local = State::default();
            }
            self.local.global_enabled = enabled;
        }
        Ok(())
    }
    pub fn edge(&mut self, vector: u8) -> Result<(), DeviceError> {
        if vector < 16
            || !self.local.global_enabled
            || (self.route.low & LEVEL != 0 && self.route.low as u8 == vector)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        self.local.irr[i] |= bit;
        self.local.tmr[i] &= !bit;
        Ok(())
    }
    pub fn level(&mut self) -> Result<(), DeviceError> {
        let vector = self.route.low as u8;
        if !self.local.global_enabled || self.local.svr & 0x100 == 0 || self.busy(vector) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        self.local.irr[i] |= bit;
        self.local.tmr[i] |= bit;
        self.route.remote_irr = true;
        Ok(())
    }
    pub fn set_line(&mut self, high: bool) -> Result<(), DeviceError> {
        if high && self.route.low & MASK == 0 {
            if self.route.low & LEVEL != 0 {
                if !self.route.remote_irr {
                    self.level()?;
                }
            } else if !self.route.high_line {
                self.edge(self.route.low as u8)?;
            }
        }
        self.route.high_line = high;
        Ok(())
    }
    /// INTIN2 (S4-29e): rising level with an unmasked entry requests the entry-2 vector as an
    /// edge; a masked edge is lost and unmasking never replays it. A refused delivery does not
    /// consume the transition.
    pub fn set_line2(&mut self, high: bool) -> Result<(), DeviceError> {
        let low = self.route.pins[2];
        if high && !self.route.intin2_high && low & MASK == 0 {
            self.edge(low as u8)?;
        }
        self.route.intin2_high = high;
        Ok(())
    }
    /// Entry-2 low dword. An unmasked entry must not share its vector with a level INTIN4
    /// route (S4-26b: level reserves its vector against edges, even while masked).
    pub fn set_low2(&mut self, value: u32) -> Result<(), DeviceError> {
        if value & MASK == 0
            && (value & LEVEL != 0
                || (self.route.low & LEVEL != 0 && self.route.low as u8 == value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route.pins[2] = value;
        Ok(())
    }
    pub fn set_low(&mut self, value: u32) -> Result<(), DeviceError> {
        if value & LEVEL != 0
            && self.route.pins[2] & MASK == 0
            && self.route.pins[2] as u8 == value as u8
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        if value & LEVEL != 0
            && self
                .timer
                .as_ref()
                .is_some_and(|t| t.lvt & MASK == 0 && t.lvt as u8 == value as u8)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let old = self.route.low;
        let changed =
            (old ^ value) & LEVEL != 0 || (old & LEVEL != 0 && (old as u8 != value as u8));
        if changed
            && (self.route.high_line
                || self.route.remote_irr
                || self.busy(old as u8)
                || self.busy(value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        // Validate delivery before publishing any configuration change.
        let needs_delivery =
            value & (LEVEL | MASK) == LEVEL && self.route.high_line && !self.route.remote_irr;
        if needs_delivery
            && (!self.local.global_enabled || self.local.svr & 0x100 == 0 || self.busy(value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route.low = value;
        if needs_delivery {
            self.level()?;
        }
        Ok(())
    }
    pub fn eoi(&mut self) -> Result<(), DeviceError> {
        let Some(v) = State::highest(&self.local.isr) else {
            return Ok(());
        };
        let (i, bit) = Self::bit(v);
        let matching =
            self.local.tmr[i] & bit != 0 && self.route.remote_irr && self.route.low as u8 == v;
        let retry = matching && self.route.high_line && self.route.low & MASK == 0;
        if retry
            && (!self.local.global_enabled
                || self.local.svr & 0x100 == 0
                || self.local.irr[i] & bit != 0)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.local.isr[i] &= !bit;
        if matching {
            self.route.remote_irr = false;
            if retry {
                self.level()?;
            }
        }
        Ok(())
    }
}
