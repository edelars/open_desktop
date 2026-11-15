//! Atomic state transitions for the Local APICs (one per vCPU, S6-11) and the IOAPIC routes.
//!
//! The IOAPIC route and every Local APIC sit behind one lock, because a level request couples
//! them: delivery sets IRR/TMR in the destination LAPIC and Remote IRR in the route, and the
//! destination's EOI clears Remote IRR. Operations that name no processor (`level`, `set_line`,
//! `set_line2`, `set_low`, `set_low2`) act on the LAPIC the route's destination field selects;
//! the others take the index of the LAPIC they act on.
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
    /// Destination field (high dword bits 31:24, a physical APIC ID) of every entry. Always an
    /// existing vCPU index; a write naming another one is refused (SMP_CONTRACT.md 6).
    pub dest: [u8; IO_APIC_PINS],
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
            dest: [0; IO_APIC_PINS],
        }
    }
}
/// The Local APIC of one processor: its registers and its own timer with its own event queue.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub(crate) struct CpuApic {
    pub local: State,
    pub timer: Option<Timer>,
}

/// All APIC state of the board: one [`CpuApic`] per vCPU (index = APIC ID) and the IOAPIC route.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct PairState {
    pub cpus: Vec<CpuApic>,
    pub route: Route,
}

impl Default for PairState {
    /// One processor without a timer: the pre-SMP configuration.
    fn default() -> Self {
        Self::new(vec![CpuApic::default()])
    }
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
    pub fn new(cpus: Vec<CpuApic>) -> Self {
        Self {
            cpus,
            route: Route::default(),
        }
    }

    /// Whether the INTIN4 route awaits an EOI from this very LAPIC.
    pub fn awaits_eoi(&self, cpu: usize) -> bool {
        self.route.remote_irr && usize::from(self.route.dest[4]) == cpu
    }

    /// Whether `vector` is reserved against edges in the LAPIC `cpu` by the level INTIN4 route
    /// (S4-26b: level reserves its vector, even while masked).
    pub fn level_vector_reserved(&self, cpu: usize, vector: u8) -> bool {
        self.route.low & LEVEL != 0
            && self.route.low as u8 == vector
            && usize::from(self.route.dest[4]) == cpu
    }

    pub fn reset_local(&mut self, cpu: usize) -> Result<(), DeviceError> {
        if self.awaits_eoi(cpu) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let apic = &mut self.cpus[cpu];
        if let Some(timer) = &mut apic.timer {
            timer.reset()?;
        }
        apic.local = State::default();
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
    fn busy(&self, cpu: usize, vector: u8) -> bool {
        let (i, bit) = Self::bit(vector);
        let local = &self.cpus[cpu].local;
        (local.irr[i] | local.isr[i]) & bit != 0
    }
    pub fn set_enabled(&mut self, cpu: usize, enabled: bool) -> Result<(), DeviceError> {
        if self.cpus[cpu].local.global_enabled != enabled {
            if !enabled {
                if self.awaits_eoi(cpu) {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                let apic = &mut self.cpus[cpu];
                if let Some(timer) = &mut apic.timer {
                    timer.reset()?;
                }
                apic.local = State::default();
            }
            self.cpus[cpu].local.global_enabled = enabled;
        }
        Ok(())
    }
    fn edge_refused(&self, cpu: usize, vector: u8) -> bool {
        vector < 16
            || !self.cpus[cpu].local.global_enabled
            || self.level_vector_reserved(cpu, vector)
    }
    fn raise_edge(&mut self, cpu: usize, vector: u8) {
        let (i, bit) = Self::bit(vector);
        let local = &mut self.cpus[cpu].local;
        local.irr[i] |= bit;
        local.tmr[i] &= !bit;
    }
    pub fn edge(&mut self, cpu: usize, vector: u8) -> Result<(), DeviceError> {
        if self.edge_refused(cpu, vector) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.raise_edge(cpu, vector);
        Ok(())
    }
    /// One IPI (S6-12): the same edge request in every LAPIC of `targets`, all or none. A target
    /// that would refuse the request refuses the whole IPI before any LAPIC changes.
    pub fn edge_all(&mut self, targets: &[usize], vector: u8) -> Result<(), DeviceError> {
        if targets.iter().any(|&cpu| self.edge_refused(cpu, vector)) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        for &cpu in targets {
            self.raise_edge(cpu, vector);
        }
        Ok(())
    }
    pub fn level(&mut self) -> Result<(), DeviceError> {
        let vector = self.route.low as u8;
        let cpu = usize::from(self.route.dest[4]);
        let local = &self.cpus[cpu].local;
        if !local.global_enabled || local.svr & 0x100 == 0 || self.busy(cpu, vector) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        let local = &mut self.cpus[cpu].local;
        local.irr[i] |= bit;
        local.tmr[i] |= bit;
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
                self.edge(usize::from(self.route.dest[4]), self.route.low as u8)?;
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
            self.edge(usize::from(self.route.dest[2]), low as u8)?;
        }
        self.route.intin2_high = high;
        Ok(())
    }
    /// Entry-2 low dword. An unmasked entry must not share its vector with a level INTIN4
    /// route in the same LAPIC (S4-26b: level reserves its vector against edges, even while
    /// masked).
    pub fn set_low2(&mut self, value: u32) -> Result<(), DeviceError> {
        if value & MASK == 0
            && (value & LEVEL != 0
                || self.level_vector_reserved(usize::from(self.route.dest[2]), value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route.pins[2] = value;
        Ok(())
    }
    pub fn set_low(&mut self, value: u32) -> Result<(), DeviceError> {
        let cpu = usize::from(self.route.dest[4]);
        if value & LEVEL != 0
            && (self.entry2_uses_vector_on(cpu, value as u8)
                || self.timer_uses_vector(cpu, value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let old = self.route.low;
        let changed =
            (old ^ value) & LEVEL != 0 || (old & LEVEL != 0 && (old as u8 != value as u8));
        if changed
            && (self.route.high_line
                || self.route.remote_irr
                || self.busy(cpu, old as u8)
                || self.busy(cpu, value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        // Validate delivery before publishing any configuration change.
        let needs_delivery =
            value & (LEVEL | MASK) == LEVEL && self.route.high_line && !self.route.remote_irr;
        if needs_delivery {
            let local = &self.cpus[cpu].local;
            if !local.global_enabled || local.svr & 0x100 == 0 || self.busy(cpu, value as u8) {
                return Err(DeviceError::UnsupportedConfiguration);
            }
        }
        self.route.low = value;
        if needs_delivery {
            self.level()?;
        }
        Ok(())
    }

    /// Whether the unmasked entry 2 takes `vector` into the LAPIC `cpu`.
    fn entry2_uses_vector_on(&self, cpu: usize, vector: u8) -> bool {
        self.route.pins[2] & MASK == 0
            && self.route.pins[2] as u8 == vector
            && usize::from(self.route.dest[2]) == cpu
    }

    /// Whether the unmasked LVT Timer of the LAPIC `cpu` uses `vector`.
    fn timer_uses_vector(&self, cpu: usize, vector: u8) -> bool {
        self.cpus[cpu]
            .timer
            .as_ref()
            .is_some_and(|t| t.lvt & MASK == 0 && t.lvt as u8 == vector)
    }

    /// Destination dword of redirection entry `pin` (APIC ID in bits 31:24, physical mode).
    /// Refused, before any change, when no vCPU has that APIC ID (SMP_CONTRACT.md 6). Moving
    /// INTIN4 or INTIN2 follows the rules of a vector change: a level INTIN4 entry keeps its
    /// destination while its line is high, a request awaits EOI or either LAPIC holds the vector,
    /// and no move may make a vector collide with an LVT Timer or entry 2 of the new LAPIC.
    pub fn set_dest(&mut self, pin: usize, apic_id: u8) -> Result<(), DeviceError> {
        let new = usize::from(apic_id);
        if new >= self.cpus.len() {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let old = usize::from(self.route.dest[pin]);
        if new != old {
            match pin {
                4 if self.route.low & LEVEL != 0 => {
                    let vector = self.route.low as u8;
                    if self.route.high_line
                        || self.route.remote_irr
                        || self.busy(old, vector)
                        || self.busy(new, vector)
                        || self.timer_uses_vector(new, vector)
                        || self.entry2_uses_vector_on(new, vector)
                    {
                        return Err(DeviceError::UnsupportedConfiguration);
                    }
                }
                2 if self.route.pins[2] & MASK == 0
                    && self.level_vector_reserved(new, self.route.pins[2] as u8) =>
                {
                    return Err(DeviceError::UnsupportedConfiguration);
                }
                _ => {}
            }
        }
        self.route.dest[pin] = apic_id;
        Ok(())
    }

    /// EOI of the LAPIC `cpu`.
    pub fn eoi(&mut self, cpu: usize) -> Result<(), DeviceError> {
        let local = &self.cpus[cpu].local;
        let Some(v) = State::highest(&local.isr) else {
            return Ok(());
        };
        let (i, bit) = Self::bit(v);
        let matching = local.tmr[i] & bit != 0 && self.awaits_eoi(cpu) && self.route.low as u8 == v;
        let retry = matching && self.route.high_line && self.route.low & MASK == 0;
        if retry && (!local.global_enabled || local.svr & 0x100 == 0 || local.irr[i] & bit != 0) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.cpus[cpu].local.isr[i] &= !bit;
        if matching {
            self.route.remote_irr = false;
            if retry {
                self.level()?;
            }
        }
        Ok(())
    }
}
