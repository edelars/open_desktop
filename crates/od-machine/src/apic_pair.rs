//! Atomic state transitions for the Local APICs (one per vCPU, S6-11) and the IOAPIC routes.
//!
//! The IOAPIC route and every Local APIC sit behind one lock, because a level request couples
//! them: delivery sets IRR/TMR in the destination LAPIC and Remote IRR in the route, and the
//! destination's EOI clears Remote IRR. That lock is the APIC lock of the lock order of
//! MACHINE_CONTRACT.md (S6-33): an ICR write is decoded and delivered under it, so there is never a
//! second Local APIC lock to take while holding one, and nothing under it takes the device-table
//! lock of the bus or the lock of an interrupt source. Operations that name no processor (`level`,
//! `set_line`, `set_line2`, `set_low`, `set_low2`) act on the LAPIC the route's destination field
//! selects; the others take the index of the LAPIC they act on.
use crate::{DeviceError, EventQueue, VirtualClock, VirtualClockHandle, local_apic::State};

pub(crate) const MASK: u32 = 1 << 16;
pub(crate) const PERIODIC: u32 = 1 << 17;
pub(crate) const LEVEL: u32 = 1 << 15;
pub(crate) const REMOTE_IRR: u32 = 1 << 14;
pub(crate) const POLARITY_LOW: u32 = 1 << 13;
/// The input the MADT interrupt source override (S4-31f) describes as level, active low: the SCI
/// (ISA IRQ 9 -> GSI 9, flags 0x000F).
pub(crate) const SCI_PIN: usize = 9;
/// Entry bits of the SCI form: trigger level (bit 15) and polarity active low (bit 13).
pub(crate) const SCI_LEVEL_LOW: u32 = LEVEL | POLARITY_LOW;
/// The Linux PCI NVMe INTA route consumes a dedicated level, active-low input outside ISA space.
pub(crate) const NVME_PIN: usize = 16;
/// Inputs with Remote IRR and EOI-driven redelivery.
const LEVEL_PINS: [usize; 3] = [4, SCI_PIN, NVME_PIN];
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
    /// The SCI source (S4-38L8e) drives its input (GSI 9): `true` while the line is asserted,
    /// the physical level being low as the MADT declares. Meaningful for the level/active-low
    /// entry form (`SCI_LEVEL_LOW`) only.
    pub sci_asserted: bool,
    /// Remote IRR of the SCI entry: set by a level delivery, cleared by the destination's EOI.
    pub sci_remote_irr: bool,
    pub nvme_asserted: bool,
    pub nvme_remote_irr: bool,
    /// IOAPIC ID register bits 27:24 (4 bits); Arbitration ID mirrors it.
    pub id: u8,
    /// Stored low dwords of entries other than INTIN4. INTIN2 is an edge input; the
    /// SCI and the NVMe INTA route are level, active low.
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
            sci_asserted: false,
            sci_remote_irr: false,
            nvme_asserted: false,
            nvme_remote_irr: false,
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
    /// Accepted guest writes to the timer registers (LVT Timer `0x320`, Initial Count `0x380`,
    /// Divide Configuration `0x3E0`) since power-on. A counter, not state of the device: nothing
    /// reads it back into the registers. The host of a vCPU quantum ends its call when it moves
    /// (S6-16d, SMP_CONTRACT.md 8), because a new deadline may be nearer than the quantum.
    pub timer_writes: u64,
}

/// INIT and SIPI waiting in the LAPIC of one processor (S6-13). The processor applies them on
/// its own boundary; INIT comes first.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
pub(crate) struct PendingStartup {
    pub init: bool,
    pub sipi: Option<u8>,
}

/// The wake sink of the vCPU threads (S6-34): called with the index of a Local APIC every time a
/// request is posted to it (a Fixed or timer vector in IRR, an INIT, a SIPI, an NMI), so that the
/// thread of that vCPU, if it sleeps (`HLT`, waiting for SIPI), looks at its APIC again. It is
/// called under the APIC lock and must take no lock of its own that is above the APIC (the sink of
/// the thread runner sets an atomic flag and unparks a thread: nothing else). `None` in the
/// interleaved mode and whenever no thread runner is active. Not part of the state of the device:
/// it is neither compared nor in a snapshot, and a restore keeps the one the pair has.
#[derive(Clone, Default)]
pub(crate) struct WakeSink(pub(crate) Option<std::sync::Arc<dyn Fn(usize) + Send + Sync>>);

impl std::fmt::Debug for WakeSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "WakeSink(set)"
        } else {
            "WakeSink(none)"
        })
    }
}

impl PartialEq for WakeSink {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for WakeSink {}

impl WakeSink {
    fn notify(&self, cpu: usize) {
        if let Some(sink) = &self.0 {
            sink(cpu);
        }
    }
}

/// All APIC state of the board: one [`CpuApic`] per vCPU (index = APIC ID) and the IOAPIC route.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct PairState {
    pub cpus: Vec<CpuApic>,
    pub route: Route,
    /// The wake sink of the vCPU threads (see [`WakeSink`]).
    pub wake: WakeSink,
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
            wake: WakeSink::default(),
        }
    }

    /// Low dword of the level-capable entry `pin` (INTIN4 or the SCI).
    fn level_low(&self, pin: usize) -> u32 {
        if pin == 4 {
            self.route.low
        } else {
            self.route.pins[pin]
        }
    }
    /// Whether `low` is an entry form with level semantics on `pin`: any level entry on INTIN4,
    /// only the level/active-low form (as the MADT declares it) on the SCI.
    fn is_level_form(pin: usize, low: u32) -> bool {
        if pin == 4 {
            low & LEVEL != 0
        } else {
            low & SCI_LEVEL_LOW == SCI_LEVEL_LOW
        }
    }
    fn is_level(&self, pin: usize) -> bool {
        Self::is_level_form(pin, self.level_low(pin))
    }
    fn level_asserted(&self, pin: usize) -> bool {
        match pin {
            4 => self.route.high_line,
            SCI_PIN => self.route.sci_asserted,
            NVME_PIN => self.route.nvme_asserted,
            _ => unreachable!("non-level pin"),
        }
    }
    fn level_remote_irr(&self, pin: usize) -> bool {
        match pin {
            4 => self.route.remote_irr,
            SCI_PIN => self.route.sci_remote_irr,
            NVME_PIN => self.route.nvme_remote_irr,
            _ => unreachable!("non-level pin"),
        }
    }
    fn set_level_remote_irr(&mut self, pin: usize, value: bool) {
        match pin {
            4 => self.route.remote_irr = value,
            SCI_PIN => self.route.sci_remote_irr = value,
            NVME_PIN => self.route.nvme_remote_irr = value,
            _ => unreachable!("non-level pin"),
        }
    }

    /// Whether a level route (INTIN4 or the SCI) awaits an EOI from this very LAPIC.
    pub fn awaits_eoi(&self, cpu: usize) -> bool {
        LEVEL_PINS
            .iter()
            .any(|&pin| self.level_remote_irr(pin) && usize::from(self.route.dest[pin]) == cpu)
    }

    /// Whether the level entry `pin` reserves `vector` in the LAPIC `cpu`.
    fn pin_level_reserves(&self, pin: usize, cpu: usize, vector: u8) -> bool {
        self.is_level(pin)
            && self.level_low(pin) as u8 == vector
            && usize::from(self.route.dest[pin]) == cpu
    }

    /// Whether a level entry other than `pin` reserves `vector` in the LAPIC `cpu`.
    fn other_level_reserves(&self, pin: usize, cpu: usize, vector: u8) -> bool {
        LEVEL_PINS
            .iter()
            .any(|&other| other != pin && self.pin_level_reserves(other, cpu, vector))
    }

    /// Whether `vector` is reserved against edges in the LAPIC `cpu` by a level route (S4-26b:
    /// level reserves its vector, even while masked).
    pub fn level_vector_reserved(&self, cpu: usize, vector: u8) -> bool {
        LEVEL_PINS
            .iter()
            .any(|&pin| self.pin_level_reserves(pin, cpu, vector))
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

    /// Queue an INIT assert in the LAPIC `cpu` (S6-13). A globally disabled LAPIC refuses, like a
    /// Fixed IPI (SMP_CONTRACT.md 5). The INIT drops a SIPI accepted before it.
    pub fn queue_init(&mut self, cpu: usize) -> Result<(), DeviceError> {
        let local = &mut self.cpus[cpu].local;
        if !local.global_enabled {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        local.pending_init = true;
        local.pending_sipi = None;
        self.wake.notify(cpu);
        Ok(())
    }

    /// Queue a start-up vector in the LAPIC `cpu` (S6-13). A globally disabled LAPIC refuses. A
    /// vector already waiting is kept: the first SIPI starts the processor, later ones are ignored.
    pub fn queue_sipi(&mut self, cpu: usize, vector: u8) -> Result<(), DeviceError> {
        let local = &mut self.cpus[cpu].local;
        if !local.global_enabled {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        local.pending_sipi.get_or_insert(vector);
        self.wake.notify(cpu);
        Ok(())
    }

    /// Remember one NMI in the LAPIC of every target (S6-12b): all or none. A target whose LAPIC
    /// is globally disabled refuses the whole NMI before any LAPIC changes, as for Fixed, INIT and
    /// SIPI. The latch is one bit: an NMI that finds one already waiting is the same event, and
    /// is lost (SMP_CONTRACT.md 7).
    pub fn queue_nmi(&mut self, targets: &[usize]) -> Result<(), DeviceError> {
        if targets
            .iter()
            .any(|&cpu| !self.cpus[cpu].local.global_enabled)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        for &cpu in targets {
            self.cpus[cpu].local.pending_nmi = true;
            self.wake.notify(cpu);
        }
        Ok(())
    }

    /// Take the pending INIT and SIPI of the LAPIC `cpu`. A pending INIT puts the LAPIC into its
    /// INIT-reset state (SMP_CONTRACT.md 4, SDM 13.4.7.3): every register, request and timer
    /// returns to its reset value except that the LAPIC stays globally enabled and keeps its APIC
    /// ID, which is its index. A Remote IRR of a level route to this LAPIC is not touched: as on
    /// hardware, only the EOI clears it (SMP_CONTRACT.md 3, 6). On an error nothing is taken.
    pub fn take_startup(&mut self, cpu: usize) -> Result<PendingStartup, DeviceError> {
        let local = &self.cpus[cpu].local;
        let pending = PendingStartup {
            init: local.pending_init,
            sipi: local.pending_sipi,
        };
        if pending.init {
            if let Some(timer) = &mut self.cpus[cpu].timer {
                timer.reset()?;
            }
            self.cpus[cpu].local = State {
                global_enabled: self.cpus[cpu].local.global_enabled,
                ..State::default()
            };
        } else {
            self.cpus[cpu].local.pending_sipi = None;
        }
        Ok(pending)
    }

    pub fn reset_io(&mut self) -> Result<(), DeviceError> {
        if LEVEL_PINS.into_iter().any(|pin| self.level_remote_irr(pin)) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.route = Route::default();
        Ok(())
    }

    /// The power-on state of the whole APIC complex (S6-24, the board reset): every Local APIC
    /// (not only one) returns to its reset state with its pending INIT, SIPI, NMI and requests
    /// dropped and its timer queue emptied, the guest timer-write counters go back to 0, and the
    /// IOAPIC route returns to its reset state, Remote IRR of every level entry included. Unlike
    /// [`Self::reset_io`] and [`Self::reset_local`] it is not vetoed by a Remote IRR: hardware reset
    /// clears it, and a vCPU that an INIT reset keeps the owner's decision that INIT does not
    /// (SMP_CONTRACT.md 3), so nobody could ever send the EOI that clears it. The Local APICs
    /// stay globally enabled and keep their APIC IDs and timer configuration (clock, frequency).
    /// All or nothing: the change is built on a copy.
    pub fn reset_all(&mut self) -> Result<(), DeviceError> {
        let mut next = self.clone();
        for apic in &mut next.cpus {
            if let Some(timer) = &mut apic.timer {
                timer.reset()?;
            }
            apic.local = State::default();
            apic.timer_writes = 0;
        }
        next.route = Route::default();
        *self = next;
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
        self.wake.notify(cpu);
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
    /// Level delivery of the entry `pin`: IRR and TMR in the destination LAPIC, Remote IRR in the
    /// route. Refused (nothing changes) while the LAPIC is disabled, SVR software-disabled or the
    /// vector is already in IRR/ISR.
    fn level_pin(&mut self, pin: usize) -> Result<(), DeviceError> {
        let vector = self.level_low(pin) as u8;
        let cpu = usize::from(self.route.dest[pin]);
        let local = &self.cpus[cpu].local;
        if !local.global_enabled || local.svr & 0x100 == 0 || self.busy(cpu, vector) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let (i, bit) = Self::bit(vector);
        let local = &mut self.cpus[cpu].local;
        local.irr[i] |= bit;
        local.tmr[i] |= bit;
        self.set_level_remote_irr(pin, true);
        self.wake.notify(cpu);
        Ok(())
    }
    pub fn set_line(&mut self, high: bool) -> Result<(), DeviceError> {
        self.set_level_line(4, high)
    }
    /// The SCI line (S4-38L8e, GSI 9): `asserted` is the source's request, the physical level
    /// being low (MADT flags 0x000F). With the entry in the level/active-low form an unmasked
    /// entry takes a level request (Remote IRR until the destination's EOI, redelivered by the
    /// EOI while the line stays asserted); a masked entry only records the line. An unmasked
    /// entry of any other form is refused before any change.
    pub fn set_sci_line(&mut self, asserted: bool) -> Result<(), DeviceError> {
        self.set_active_low_level_line(SCI_PIN, asserted)
    }

    pub fn set_nvme_line(&mut self, asserted: bool) -> Result<(), DeviceError> {
        self.set_active_low_level_line(NVME_PIN, asserted)
    }

    fn set_active_low_level_line(&mut self, pin: usize, asserted: bool) -> Result<(), DeviceError> {
        if asserted && self.route.pins[pin] & MASK == 0 && !self.is_level(pin) {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.set_level_line(pin, asserted)
    }
    fn set_level_line(&mut self, pin: usize, high: bool) -> Result<(), DeviceError> {
        let low = self.level_low(pin);
        if high && low & MASK == 0 {
            if Self::is_level_form(pin, low) {
                if !self.level_remote_irr(pin) {
                    self.level_pin(pin)?;
                }
            } else if !self.level_asserted(pin) {
                self.edge(usize::from(self.route.dest[pin]), low as u8)?;
            }
        }
        match pin {
            4 => self.route.high_line = high,
            SCI_PIN => self.route.sci_asserted = high,
            NVME_PIN => self.route.nvme_asserted = high,
            _ => unreachable!("non-level pin"),
        }
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
        self.set_level_low(4, value)
    }
    /// Low dword of the SCI entry (S4-38L8e). Entry forms other than level/active-low are only
    /// stored; the level/active-low form follows the INTIN4 level rules.
    pub fn set_sci_low(&mut self, value: u32) -> Result<(), DeviceError> {
        self.set_level_low(SCI_PIN, value)
    }
    pub(crate) fn set_level_low(&mut self, pin: usize, value: u32) -> Result<(), DeviceError> {
        let cpu = usize::from(self.route.dest[pin]);
        let old = self.level_low(pin);
        let (was, now) = (
            Self::is_level_form(pin, old),
            Self::is_level_form(pin, value),
        );
        if now
            && (self.entry2_uses_vector_on(cpu, value as u8)
                || self.timer_uses_vector(cpu, value as u8)
                || self.other_level_reserves(pin, cpu, value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        let changed = was != now || (was && old as u8 != value as u8);
        if changed
            && (self.level_asserted(pin)
                || self.level_remote_irr(pin)
                || self.busy(cpu, old as u8)
                || self.busy(cpu, value as u8))
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        // Validate delivery before publishing any configuration change.
        let needs_delivery =
            now && value & MASK == 0 && self.level_asserted(pin) && !self.level_remote_irr(pin);
        if needs_delivery {
            let local = &self.cpus[cpu].local;
            if !local.global_enabled || local.svr & 0x100 == 0 || self.busy(cpu, value as u8) {
                return Err(DeviceError::UnsupportedConfiguration);
            }
        }
        if pin == 4 {
            self.route.low = value;
        } else {
            self.route.pins[pin] = value;
        }
        if needs_delivery {
            self.level_pin(pin)?;
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
                4 | SCI_PIN | NVME_PIN if self.is_level(pin) => {
                    let vector = self.level_low(pin) as u8;
                    if self.level_asserted(pin)
                        || self.level_remote_irr(pin)
                        || self.busy(old, vector)
                        || self.busy(new, vector)
                        || self.timer_uses_vector(new, vector)
                        || self.entry2_uses_vector_on(new, vector)
                        || self.other_level_reserves(pin, new, vector)
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
        let matching = LEVEL_PINS.into_iter().find(|&pin| {
            local.tmr[i] & bit != 0
                && self.level_remote_irr(pin)
                && usize::from(self.route.dest[pin]) == cpu
                && self.level_low(pin) as u8 == v
        });
        let retry =
            matching.filter(|&pin| self.level_asserted(pin) && self.level_low(pin) & MASK == 0);
        if retry.is_some()
            && (!local.global_enabled || local.svr & 0x100 == 0 || local.irr[i] & bit != 0)
        {
            return Err(DeviceError::UnsupportedConfiguration);
        }
        self.cpus[cpu].local.isr[i] &= !bit;
        if let Some(pin) = matching {
            self.set_level_remote_irr(pin, false);
            if retry.is_some() {
                self.level_pin(pin)?;
            }
        }
        Ok(())
    }
}
