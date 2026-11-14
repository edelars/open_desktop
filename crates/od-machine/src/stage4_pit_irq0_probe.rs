//! S4-29f: PIT channel 0 -> OUT0 -> IOAPIC INTIN2 (GSI 2) -> Local APIC -> guest
//! handler on the flat `Stage4PitIrq0ProbeV1` kernel.
//!
//! # The bridge (finding of the slice)
//!
//! The pump's own IRQ queue (S4-27i3) holds `(key, LAPIC vector, due)` and takes
//! a due event with `request_fixed_edge(vector)` straight on the Local APIC. That
//! path bypasses the IOAPIC (redirection entry, mask, vector chosen by the
//! guest), so it cannot carry OUT0. The bridge is therefore an owner hook
//! ([`PumpHost`], S4-27i5) that owns its own [`EventQueue`] with generations (the
//! same primitive, the same rules as the UART RX queue): a due OUT0 edge is
//! presented to the IOAPIC as a pulse on INTIN2 (`set_intin2(true)`, then
//! `set_intin2(false)`); the accepted S4-29e rules decide what happens (vector of
//! entry 2, a masked edge is lost, a refused delivery keeps the event).
//!
//! # Events
//!
//! After every guest PIO instruction and at the start of every pump step the
//! bridge compares the guest-visible state of channel 0 (mode and the count in
//! progress) with the one it last saw. Any change (a control word, a new count, a
//! shutdown, a PIT reset) is a reprogram: the queue starts a new generation (every
//! pending event of the old one is dropped) and, if channel 0 holds a count, the
//! next OUT0 rising edge is scheduled. After an edge is delivered the next one is
//! scheduled (only when the queue is empty, so one chain of events exists per
//! generation).
//!
//! OUT0 rising edges (formulas of S4-29c, `T(t) = floor(t * 1_193_182 / 1e9)`):
//! mode 2 with reload `N` loaded at tick `T0`: edges at `T0 + k*N`, `k >= 1`
//! (OUT is low for one tick before each; a reload written while counting takes
//! effect at the next period boundary, which is itself an edge); mode 0: one edge
//! at `T0 + N` and none afterwards. The deadline of an edge at tick `E` is the
//! first nanosecond with `T(t) >= E`: `ceil(E * 1e9 / 1_193_182)`.
//!
//! OUT0 goes to INTIN2 only. The 8259 pair has no input lines (S4-29a): the PIC
//! IR0 receives nothing. Time per instruction is 0 ns; the clock moves by the
//! pump's idle advance to the bridge's next deadline.
//!
//! The probe image is a flat identity-mapped Long64/CPL0 guest (GDT, an IDT with
//! the INTIN2 vector, one code page, a counter dword at 0x5000). The loader
//! installs IDTR.

use crate::interrupt_pump::{
    PumpEntrySource, PumpHost, PumpHostEvent, PumpHostStep, PumpRejection, PumpResult,
};
use crate::stage4_interrupt_probe_common::{
    FLAT_IDT_GPA, FlatProbeClock, FlatProbeError, FlatProbeSpec, flat_access_context,
    flat_entry_state, validate_flat_image,
};
use crate::{
    BusBuildError, DiagnosticExit, DiagnosticRunResult, DiagnosticVcpuSession, EventGeneration,
    EventQueue, EventQueueError, GuestPhysicalAddress, GuestPhysicalRange, IO_APIC_MMIO_BASE,
    IO_APIC_MMIO_SIZE, InterruptPump, IoApic, IoApicHandle, LOCAL_APIC_MMIO_BASE,
    LOCAL_APIC_MMIO_SIZE, LocalApic, MachineBus, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, MmioBinding, PIC_MASTER_BASE_PORT, PIC_PORT_BYTES,
    PIC_SLAVE_BASE_PORT, PIT_BASE_PORT, PIT_INPUT_HZ, PIT_PORT_B, PIT_PORT_B_BYTES, PIT_PORT_BYTES,
    Pic8259Handle, Pic8259Pair, Pit8254Devices, Pit8254Handle, PitChannelSnapshot, PitMode, PitRun,
    PortAddress, PortRange, PortRegionBinding, RamLimits, VirtualClockHandle, isa_irq_vector,
    pit_ticks,
};
use od_isa::execute::RdtscControls;
use od_isa::state::Idtr;
use od_isa::{INTEGER_FLAGS, Register};
use std::any::Any;

pub const STAGE4_PIT_IRQ0_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES: usize = 0x4000;
pub const STAGE4_PIT_IRQ0_PROBE_RAM_BYTES: u64 = 0x7000;
pub const STAGE4_PIT_IRQ0_PROBE_ENTRY: u64 = 0x4000;
pub const STAGE4_PIT_IRQ0_PROBE_STACK_TOP: u64 = 0x6ff0;
/// Vector of IOAPIC entry 2 in the guest (priority class 6); distinct from the
/// S4-27i vectors 0x41, 0x51, 0x61 and 0x62 and from the PIC IR0 vector 0x30.
pub const STAGE4_PIT_IRQ0_PROBE_VECTOR: u8 = 0x60;
/// Guest RAM address of the handler's counter dword.
pub const STAGE4_PIT_IRQ0_PROBE_COUNTER_GPA: u64 = 0x5000;
/// Frequency of the virtual TSC (1 GHz: one tick per nanosecond).
pub const STAGE4_PIT_IRQ0_PROBE_TSC_HZ: u64 = 1_000_000_000;
/// Image GPA of the `MOV EAX,0x60` that is the value written to the entry-2
/// low dword (bytes `B8 60 00 00 00`); the `MaskedEntry2` injection sets bit 16.
pub const STAGE4_PIT_IRQ0_PROBE_UNMASK_GPA: u64 = 0x4044;
/// Image GPA of the EOI store `MOV [RDI+0xB0],EAX` (bytes `89 87 B0 00 00 00`);
/// the `NoEoi` injection redirects it to the TPR (offset 0x80).
pub const STAGE4_PIT_IRQ0_PROBE_EOI_GPA: u64 = 0x40b1;
const UNMASK_PATTERN: [u8; 5] = [0xb8, 0x60, 0x00, 0x00, 0x00];
const EOI_PATTERN: [u8; 6] = [0x89, 0x87, 0xb0, 0x00, 0x00, 0x00];
const BRIDGE_QUEUE_CAPACITY: usize = 16;
const NANOS_PER_SECOND: u128 = 1_000_000_000;

const SPEC: FlatProbeSpec = FlatProbeSpec {
    image_gpa: STAGE4_PIT_IRQ0_PROBE_IMAGE_GPA,
    image_bytes: STAGE4_PIT_IRQ0_PROBE_IMAGE_BYTES,
    ram_bytes: STAGE4_PIT_IRQ0_PROBE_RAM_BYTES,
    entry: STAGE4_PIT_IRQ0_PROBE_ENTRY,
    stack_top: STAGE4_PIT_IRQ0_PROBE_STACK_TOP,
    vectors: &[STAGE4_PIT_IRQ0_PROBE_VECTOR],
    extra_gate_message: "IDT must contain no gate other than the INTIN2 vector",
    clock: Some(FlatProbeClock {
        tsc_hz: STAGE4_PIT_IRQ0_PROBE_TSC_HZ,
        timer_hz: None,
    }),
};

/// Deliberate fault injection for regression evidence; never part of the
/// accepted configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PitIrq0Injection {
    /// The guest writes entry 2 masked (the image's unmask value gets bit 16):
    /// every OUT0 edge is lost at the IOAPIC (S4-29e).
    MaskedEntry2,
    /// The handler's EOI store is redirected to the TPR: the vector stays in
    /// service and blocks the next edge.
    NoEoi,
    /// The bridge never starts a new generation: a reprogram, shutdown or reset
    /// leaves the old events in the queue, and they are delivered.
    NoGeneration,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4PitIrq0ProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

impl From<FlatProbeError> for Stage4PitIrq0ProbeLoadError {
    fn from(error: FlatProbeError) -> Self {
        match error {
            FlatProbeError::ImageSize { expected, actual } => Self::ImageSize { expected, actual },
            FlatProbeError::Layout(reason) => Self::Layout(reason),
            FlatProbeError::Memory(error) => Self::Memory(error),
            FlatProbeError::BusBuild(error) => Self::BusBuild(error),
            FlatProbeError::Session(detail) => Self::Session(detail),
        }
    }
}

/// Deadline in virtual nanoseconds of the PIT input tick `tick`: the first
/// `t` with `pit_ticks(t) >= tick`, that is `ceil(tick * 1e9 / PIT_INPUT_HZ)`.
pub const fn pit_tick_deadline_ns(tick: u64) -> u64 {
    let numerator = tick as u128 * NANOS_PER_SECOND;
    numerator.div_ceil(PIT_INPUT_HZ as u128) as u64
}

/// Smallest tick `> after_tick` at which OUT0 rises, for the current state of
/// channel 0; `None` when no edge follows.
pub fn next_out0_rising_tick(channel: &PitChannelSnapshot, after_tick: u64) -> Option<u64> {
    let (mode, run) = (channel.mode?, channel.run?);
    match mode {
        PitMode::TerminalCount => {
            let edge = run.start_tick + u64::from(run.reload);
            (edge > after_tick).then_some(edge)
        }
        PitMode::SoftwareStrobe => {
            // OUT is low for the one tick `start + reload` and rises one tick later; a single
            // edge per count (S4-39g2).
            let edge = run.start_tick + u64::from(run.reload) + 1;
            (edge > after_tick).then_some(edge)
        }
        PitMode::RateGenerator => {
            let PitRun {
                reload,
                start_tick,
                next,
            } = run;
            // Smallest `start + k * reload > after` with `k >= 1`.
            let first = |reload: u32, start: u64| {
                let period = u64::from(reload);
                start + ((after_tick.max(start) - start) / period + 1) * period
            };
            let edge = first(reload, start_tick);
            Some(match next {
                Some(next) if edge > next.start_tick => first(next.reload, next.start_tick),
                _ => edge,
            })
        }
    }
}

/// What the bridge compares to detect a reprogram.
type ChannelState = (Option<PitMode>, Option<PitRun>);

fn channel_state(channel: &PitChannelSnapshot) -> ChannelState {
    (channel.mode, channel.run)
}

/// Counters of the bridge (evidence).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PitIrq0Stats {
    /// OUT0 edges presented to INTIN2.
    pub edges: u64,
    /// Events scheduled.
    pub scheduled: u64,
    /// Generation changes (reprogram, shutdown, reset).
    pub generation_changes: u64,
    /// Pending events dropped by those changes.
    pub cancelled: u64,
}

/// The bridge: channel 0 OUT0 -> event queue -> IOAPIC INTIN2. Also the pump
/// host that executes the guest's `IN AL,DX` / `OUT DX,AL`.
struct PitIrq0Owner {
    pit: Pit8254Handle,
    io_apic: IoApicHandle,
    clock: VirtualClockHandle,
    queue: EventQueue,
    next_key: u64,
    seen: ChannelState,
    no_generation: bool,
    stats: PitIrq0Stats,
}

impl PitIrq0Owner {
    fn event(name: &'static str, a: u64, b: u64) -> PumpHostEvent {
        PumpHostEvent { name, a, b }
    }

    fn now_ns(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    fn schedule_after(
        &mut self,
        after_tick: u64,
        events: &mut Vec<PumpHostEvent>,
    ) -> Result<(), String> {
        let channel = self.pit.snapshot().channel0;
        let Some(tick) = next_out0_rising_tick(&channel, after_tick) else {
            return Ok(());
        };
        let deadline = pit_tick_deadline_ns(tick);
        let key = self.next_key;
        self.next_key += 1;
        self.queue
            .schedule(self.queue.generation(), key, deadline)
            .map_err(|error| format!("PIT event queue: {error:?}"))?;
        self.stats.scheduled += 1;
        events.push(Self::event("pit_schedule", deadline, tick));
        Ok(())
    }

    /// Detect a reprogram of channel 0 since the last observation.
    fn sync(&mut self, events: &mut Vec<PumpHostEvent>) -> Result<(), String> {
        let channel = self.pit.snapshot().channel0;
        let state = channel_state(&channel);
        if state == self.seen {
            return Ok(());
        }
        self.seen = state;
        if !self.no_generation {
            let dropped = self.queue.len() as u64;
            self.queue
                .reset(self.queue.generation())
                .map_err(|error| format!("PIT event queue reset: {error:?}"))?;
            self.stats.generation_changes += 1;
            self.stats.cancelled += dropped;
            events.push(Self::event(
                "pit_cancel",
                dropped,
                self.stats.generation_changes,
            ));
        }
        self.schedule_after(pit_ticks(self.now_ns()), events)
    }

    /// Present the OUT0 rising edge to INTIN2 as a pulse.
    fn pulse(&mut self) -> Result<(), String> {
        self.io_apic
            .set_intin2(true)
            .map_err(|error| format!("set_intin2(true): {error:?}"))?;
        self.io_apic
            .set_intin2(false)
            .map_err(|error| format!("set_intin2(false): {error:?}"))
    }

    /// Deliver every due edge: the event is removed only after the INTIN2 pulse
    /// was accepted, so a refused delivery keeps it (S4-29e).
    fn deliver_due(&mut self, now: u64, events: &mut Vec<PumpHostEvent>) -> Result<(), String> {
        for _ in 0..BRIDGE_QUEUE_CAPACITY {
            let generation = self.queue.generation();
            let queue_error = |error: EventQueueError| format!("PIT event queue: {error:?}");
            let Some(due) = self.queue.peek_due(generation, now).map_err(queue_error)? else {
                break;
            };
            self.pulse()?;
            self.queue
                .remove_due(generation, now)
                .map_err(queue_error)?;
            self.stats.edges += 1;
            events.push(Self::event("out0_edge", due.deadline_ns, 0));
            if self.queue.is_empty() {
                self.schedule_after(pit_ticks(due.deadline_ns), events)?;
            }
        }
        Ok(())
    }
}

impl PumpHost for PitIrq0Owner {
    fn dispatch(&mut self, now_ns: u64) -> Result<Vec<PumpHostEvent>, String> {
        let mut events = Vec::new();
        self.sync(&mut events)?;
        self.deliver_due(now_ns, &mut events)?;
        Ok(events)
    }

    /// Only a future deadline can wake the guest.
    fn next_deadline(&self, now_ns: u64) -> Option<u64> {
        self.queue
            .next_deadline()
            .filter(|deadline| *deadline > now_ns)
    }

    fn step(
        &mut self,
        session: &mut DiagnosticVcpuSession,
    ) -> Option<Result<PumpHostStep, PumpRejection>> {
        let code = session.peek_code(1);
        let opcode = *code.first()?;
        if !matches!(opcode, 0xec | 0xee) {
            return None;
        }
        let port = session.state().gpr[Register::Rdx as usize] as u16;
        let al_before = session.state().gpr[Register::Rax as usize] as u8;
        let result: DiagnosticRunResult = match session.step_pio_u8() {
            Ok(result) => result,
            Err(error) => return Some(Err(PumpRejection::Control(error))),
        };
        let mut events = Vec::new();
        let retired = matches!(result.exit, DiagnosticExit::WorkLimit(_))
            && result.instructions_executed == 1;
        if retired {
            events.push(if opcode == 0xec {
                let al_after = session.state().gpr[Register::Rax as usize] as u8;
                Self::event("port_in", u64::from(port), u64::from(al_after))
            } else {
                Self::event("port_out", u64::from(port), u64::from(al_before))
            });
            if let Err(detail) = self.sync(&mut events) {
                return Some(Err(PumpRejection::Host(detail)));
            }
        }
        Some(Ok(PumpHostStep { result, events }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

pub struct Stage4PitIrq0Probe {
    pump: InterruptPump,
    pit: Pit8254Handle,
    pic_master: Pic8259Handle,
    pic_slave: Pic8259Handle,
}

fn layout(reason: &'static str) -> Stage4PitIrq0ProbeLoadError {
    Stage4PitIrq0ProbeLoadError::Layout(reason)
}

/// Apply an injection to a private copy of the validated image; the bytes at the
/// patch site must be the profile's own (otherwise the layout is refused).
fn inject(
    image: &mut [u8],
    injection: Stage4PitIrq0Injection,
) -> Result<(), Stage4PitIrq0ProbeLoadError> {
    let base = STAGE4_PIT_IRQ0_PROBE_IMAGE_GPA;
    match injection {
        Stage4PitIrq0Injection::MaskedEntry2 => {
            let at = (STAGE4_PIT_IRQ0_PROBE_UNMASK_GPA - base) as usize;
            if image[at..at + UNMASK_PATTERN.len()] != UNMASK_PATTERN {
                return Err(layout(
                    "entry-2 unmask value is not where the profile puts it",
                ));
            }
            image[at + 3] = 0x01; // imm32 byte 2: mask bit 16 of the low dword
        }
        Stage4PitIrq0Injection::NoEoi => {
            let at = (STAGE4_PIT_IRQ0_PROBE_EOI_GPA - base) as usize;
            if image[at..at + EOI_PATTERN.len()] != EOI_PATTERN {
                return Err(layout("EOI store is not where the profile puts it"));
            }
            image[at + 2] = 0x80; // TPR instead of EOI
        }
        Stage4PitIrq0Injection::NoGeneration => {}
    }
    Ok(())
}

/// Validate a `Stage4PitIrq0ProbeV1` raw image, place it at GPA `0x1000` and
/// return the pump at the fixed Long64/CPL0 entry state with the PIT bridge.
pub fn load_stage4_pit_irq0_probe(
    image: &[u8],
    injection: Option<Stage4PitIrq0Injection>,
) -> Result<Stage4PitIrq0Probe, Stage4PitIrq0ProbeLoadError> {
    validate_flat_image(image, &SPEC)?;
    let mut image = image.to_vec();
    if let Some(injection) = injection {
        inject(&mut image, injection)?;
    }
    let region = |base: u64, len: u64, kind| {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len)
            .map(|range| MemoryRegionSpec::new(range, kind))
            .map_err(Stage4PitIrq0ProbeLoadError::Memory)
    };
    let map = MachineMemoryMap::new(
        36,
        vec![
            region(0, STAGE4_PIT_IRQ0_PROBE_RAM_BYTES, MemoryRegionKind::Ram)?,
            region(IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, MemoryRegionKind::Mmio)?,
            region(
                LOCAL_APIC_MMIO_BASE,
                LOCAL_APIC_MMIO_SIZE,
                MemoryRegionKind::Mmio,
            )?,
        ],
    )
    .map_err(Stage4PitIrq0ProbeLoadError::Memory)?;
    let session_error =
        |error: &dyn std::fmt::Debug| Stage4PitIrq0ProbeLoadError::Session(format!("{error:?}"));
    let clock = VirtualClockHandle::new(STAGE4_PIT_IRQ0_PROBE_TSC_HZ)
        .map_err(|error| session_error(&error))?;
    let local = LocalApic::new();
    let apic = local.handle();
    let io = IoApic::new(apic.clone());
    let io_apic = io.handle();
    let pit_devices = Pit8254Devices::new(clock.clone());
    let pit = pit_devices.handle.clone();
    let pics = Pic8259Pair::new();
    let (pic_master, pic_slave) = (pics.master_handle.clone(), pics.slave_handle.clone());
    let port = |base: u16, len: u32, device_index: usize| {
        PortRange::new(PortAddress::new(base), len)
            .map(|range| PortRegionBinding {
                range,
                device_index,
            })
            .map_err(Stage4PitIrq0ProbeLoadError::BusBuild)
    };
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_PIT_IRQ0_PROBE_RAM_BYTES),
        vec![
            Box::new(io),
            Box::new(local),
            Box::new(pit_devices.pit),
            Box::new(pit_devices.port_b),
            Box::new(pics.master),
            Box::new(pics.slave),
        ],
        vec![
            MmioBinding {
                region_start: GuestPhysicalAddress::new(IO_APIC_MMIO_BASE),
                device_index: 0,
            },
            MmioBinding {
                region_start: GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE),
                device_index: 1,
            },
        ],
        vec![
            port(PIT_BASE_PORT, PIT_PORT_BYTES, 2)?,
            port(PIT_PORT_B, PIT_PORT_B_BYTES, 3)?,
            port(PIC_MASTER_BASE_PORT, PIC_PORT_BYTES, 4)?,
            port(PIC_SLAVE_BASE_PORT, PIC_PORT_BYTES, 5)?,
        ],
    )
    .map_err(Stage4PitIrq0ProbeLoadError::BusBuild)?;
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(STAGE4_PIT_IRQ0_PROBE_IMAGE_GPA),
        &image,
    )
    .map_err(Stage4PitIrq0ProbeLoadError::Memory)?;

    let state = flat_entry_state(
        STAGE4_PIT_IRQ0_PROBE_ENTRY,
        STAGE4_PIT_IRQ0_PROBE_STACK_TOP,
        Idtr {
            base: FLAT_IDT_GPA,
            limit: 0x0fff,
        },
    );
    let session = DiagnosticVcpuSession::new_with_clock_handle(
        state,
        bus,
        flat_access_context(),
        INTEGER_FLAGS,
        clock.clone(),
        RdtscControls { cr4_tsd: false },
    )
    .map_err(|error| session_error(&error))?;
    let owner = PitIrq0Owner {
        pit: pit.clone(),
        io_apic,
        clock,
        queue: EventQueue::new(BRIDGE_QUEUE_CAPACITY),
        next_key: 0,
        seen: (None, None),
        no_generation: injection == Some(Stage4PitIrq0Injection::NoGeneration),
        stats: PitIrq0Stats::default(),
    };
    // The PIC IR0 vector is watched too: it must never rise (S4-29f).
    let pump = InterruptPump::new(session, apic)
        .with_watched_vectors(&[STAGE4_PIT_IRQ0_PROBE_VECTOR, isa_irq_vector(0)])
        .with_host(Box::new(owner));
    Ok(Stage4PitIrq0Probe {
        pump,
        pit,
        pic_master,
        pic_slave,
    })
}

impl Stage4PitIrq0Probe {
    fn owner(&self) -> &PitIrq0Owner {
        self.pump
            .host::<PitIrq0Owner>()
            .expect("the PIT probe always has its bridge")
    }

    pub fn pump(&self) -> &InterruptPump {
        &self.pump
    }

    pub fn pump_mut(&mut self) -> &mut InterruptPump {
        &mut self.pump
    }

    pub fn run(&mut self, budget: u64, stop: impl FnMut() -> bool) -> PumpResult {
        self.pump.run(budget, stop)
    }

    pub fn pit(&self) -> &Pit8254Handle {
        &self.pit
    }

    pub fn pic_master(&self) -> Pic8259Handle {
        self.pic_master.clone()
    }

    pub fn pic_slave(&self) -> Pic8259Handle {
        self.pic_slave.clone()
    }

    /// The handler's counter dword.
    pub fn counter(&mut self) -> u32 {
        self.pump
            .peek_ram_qword(STAGE4_PIT_IRQ0_PROBE_COUNTER_GPA)
            .map_or(0, |word| word as u32)
    }

    pub fn time_ns(&self) -> u64 {
        self.pump.virtual_time_ns().unwrap_or(0)
    }

    /// Token of the bridge queue's current generation.
    pub fn generation(&self) -> EventGeneration {
        self.owner().queue.generation()
    }

    /// Whether a token of an earlier generation is refused by the queue.
    pub fn is_stale(&self, generation: EventGeneration) -> bool {
        self.owner().queue.validate_generation(generation).is_err()
    }

    /// Pending OUT0 events.
    pub fn pending_events(&self) -> usize {
        self.owner().queue.len()
    }

    pub fn next_event_deadline(&self) -> Option<u64> {
        self.owner().queue.next_deadline()
    }

    pub fn stats(&self) -> PitIrq0Stats {
        self.owner().stats
    }

    /// Machine reset of the PIT (counters uninitialised, OUT low or high only
    /// after a control word): the bridge sees it at once and drops the events.
    pub fn reset_pit(&mut self) -> Result<(), String> {
        self.pit.reset();
        let owner = self
            .pump
            .host_mut::<PitIrq0Owner>()
            .expect("the PIT probe always has its bridge");
        owner.sync(&mut Vec::new())
    }

    /// Number of halt-wake entries of the INTIN2 vector.
    pub fn wakeups(&self) -> usize {
        self.pump
            .trace()
            .iter()
            .filter(|event| {
                matches!(
                    event.kind,
                    crate::PumpTraceKind::Entry {
                        vector: STAGE4_PIT_IRQ0_PROBE_VECTOR,
                        source: PumpEntrySource::HaltWake | PumpEntrySource::Boundary,
                    }
                )
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PitAccess, PitNextReload};

    fn channel(mode: PitMode, run: Option<PitRun>) -> PitChannelSnapshot {
        PitChannelSnapshot {
            mode: Some(mode),
            access: Some(PitAccess::LsbMsb),
            run,
            ..PitChannelSnapshot::default()
        }
    }

    fn run(reload: u32, start_tick: u64, next: Option<PitNextReload>) -> Option<PitRun> {
        Some(PitRun {
            reload,
            start_tick,
            next,
        })
    }

    #[test]
    fn mode2_edges_are_one_period_apart_after_the_load() {
        let ch = channel(PitMode::RateGenerator, run(100, 7, None));
        assert_eq!(next_out0_rising_tick(&ch, 7), Some(107));
        assert_eq!(next_out0_rising_tick(&ch, 106), Some(107));
        assert_eq!(next_out0_rising_tick(&ch, 107), Some(207));
        assert_eq!(next_out0_rising_tick(&ch, 3), Some(107));
    }

    #[test]
    fn mode2_pending_reload_applies_from_the_period_boundary() {
        // Old period 100 from tick 0, new period 30 from the boundary at 200.
        let next = PitNextReload {
            reload: 30,
            start_tick: 200,
        };
        let ch = channel(PitMode::RateGenerator, run(100, 0, Some(next)));
        assert_eq!(next_out0_rising_tick(&ch, 150), Some(200), "boundary edge");
        assert_eq!(next_out0_rising_tick(&ch, 200), Some(230));
        assert_eq!(next_out0_rising_tick(&ch, 231), Some(260));
    }

    #[test]
    fn mode0_has_one_edge_and_no_count_has_none() {
        let ch = channel(PitMode::TerminalCount, run(50, 10, None));
        assert_eq!(next_out0_rising_tick(&ch, 10), Some(60));
        assert_eq!(next_out0_rising_tick(&ch, 60), None);
        assert_eq!(
            next_out0_rising_tick(&channel(PitMode::RateGenerator, None), 0),
            None
        );
        assert_eq!(
            next_out0_rising_tick(&PitChannelSnapshot::default(), 0),
            None
        );
    }

    #[test]
    fn mode4_has_one_edge_one_tick_after_the_strobe_and_no_count_has_none() {
        // Count 50 loaded at tick 10: OUT low at tick 60, rising at tick 61 (S4-39g2).
        let ch = channel(PitMode::SoftwareStrobe, run(50, 10, None));
        assert_eq!(next_out0_rising_tick(&ch, 10), Some(61));
        assert_eq!(next_out0_rising_tick(&ch, 60), Some(61));
        assert_eq!(next_out0_rising_tick(&ch, 61), None);
        assert_eq!(
            next_out0_rising_tick(&channel(PitMode::SoftwareStrobe, None), 0),
            None
        );
    }

    #[test]
    fn edges_agree_with_the_core_out_line() {
        // The edge tick is the first tick whose OUT0 is high after a low one.
        let pit_edge = |reload: u32, start: u64, tick: u64| {
            let low = (tick - 1 - start) % u64::from(reload) == u64::from(reload) - 1;
            let high = (tick - start) % u64::from(reload) != u64::from(reload) - 1;
            low && high
        };
        let ch = channel(PitMode::RateGenerator, run(5, 3, None));
        let mut after = 3;
        for _ in 0..5 {
            let edge = next_out0_rising_tick(&ch, after).unwrap();
            assert!(pit_edge(5, 3, edge));
            assert!((after + 1..edge).all(|tick| !pit_edge(5, 3, tick)));
            after = edge;
        }
    }
}
