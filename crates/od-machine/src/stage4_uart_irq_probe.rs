//! S4-24e: COM1 IRQ4 -> IOAPIC INTIN4 -> Local APIC -> guest handler.
//!
//! `Stage4UartIrqProbeV1` is a flat identity-mapped Long64/CPL0 guest (no
//! paging: the existing maskable-interrupt entry, STI, HLT and IRETQ profiles
//! are flat-only). The session owner is the only place where the UART line is
//! connected to INTIN4: after every operation that can change UART state it
//! samples [`Uart16550Handle::interrupt_line`] and presents level changes to
//! the IOAPIC through `set_intin4`. There is no scheduler and no thread: the
//! caller supplies a step budget and a STOP predicate, and virtual time moves
//! only when the guest is halted and the next RX deadline is in the future.
//!
//! S4-27i5: the run loop (STOP, budget, boundary delivery, STI/HLT/IRETQ steps,
//! halt wake, idle advance, trace of IRR/entry/EOI) is the [`InterruptPump`].
//! What remains here is the UART owner adapter ([`UartOwner`], a [`PumpHost`]):
//! the RX event queue, the UART-to-INTIN4 line sampling, instant transmit, the
//! two instructions the ordinary interpreter does not execute (`LIDT [RSP+0]`
//! and bare `IN`/`OUT`) and the mapping of pump results to the S4-24e trace and
//! exit types.

use crate::interrupt_pump::{
    PumpEntrySource, PumpExit, PumpHost, PumpHostEvent, PumpHostStep, PumpRejection, PumpTraceKind,
    PumpTraceMode, PumpTraceModeError,
};
use crate::stage4_interrupt_probe_common::{flat_access_context, flat_entry_state};
use crate::{
    BusBuildError, DiagnosticExit, DiagnosticHaltWaitError, DiagnosticRunResult,
    DiagnosticTerminalExit, DiagnosticVcpuControlError, DiagnosticVcpuSession, EventGeneration,
    EventQueueError, GuestPhysicalAddress, GuestPhysicalRange, IO_APIC_MMIO_BASE,
    IO_APIC_MMIO_SIZE, InterruptPump, IoApic, IoApicHandle, LOCAL_APIC_MMIO_BASE,
    LOCAL_APIC_MMIO_SIZE, LocalApic, MachineBus, MachineMemoryMap, MemoryAccessError,
    MemoryRegionKind, MemoryRegionSpec, MmioBinding, PortAddress, PortRange, PortRegionBinding,
    RamLimits, Uart16550Handle, Uart16550PollingTx, UartRxEventPump, VirtualClockError,
    VirtualClockHandle,
};
use od_isa::execute::RdtscControls;
use od_isa::state::Idtr;
use od_isa::{INTEGER_FLAGS, LongModeState, Register};
use std::any::Any;

pub const STAGE4_UART_IRQ_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_UART_IRQ_PROBE_IMAGE_BYTES: usize = 0x6000;
pub const STAGE4_UART_IRQ_PROBE_RAM_BYTES: u64 = 0x7000;
pub const STAGE4_UART_IRQ_PROBE_ENTRY: u64 = 0x4000;
pub const STAGE4_UART_IRQ_PROBE_STACK_TOP: u64 = 0x6ff0;
pub const STAGE4_UART_IRQ_PROBE_VECTOR: u8 = 0x51;
pub const STAGE4_UART_IRQ_PROBE_COM1: u16 = 0x3f8;
/// Tracing bound; exceeding it ends the run instead of growing without limit.
pub const STAGE4_UART_IRQ_PROBE_TRACE_LIMIT: usize = 512;
const DIAGNOSTIC_TSC_HZ: u64 = 1_000_000_000;
const GDT_GPA: u64 = 0x2000;
const IDT_GPA: u64 = 0x3000;
const CODE_GPA: u64 = 0x4000;
const IDTR_GPA: u64 = 0x6ff0;
const GDT: [u64; 3] = [0, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff];
const LIDT_RSP0: [u8; 5] = [0x0f, 0x01, 0x5c, 0x24, 0x00];

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4UartIrqProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4UartIrqProbeError {
    Queue(EventQueueError),
    Clock(VirtualClockError),
    /// The session clock cannot be advanced now (the CPU is stopped or
    /// terminal; S4-27i5: the clock is owned by the session).
    ClockState(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UartIrqTraceKind {
    /// The RX event queue delivered a byte to the UART receive register.
    RxDelivered {
        byte: u8,
    },
    /// The gated UART interrupt output changed level (owner-side sample).
    UartLine {
        high: bool,
    },
    /// The vector was latched in the Local APIC IRR.
    Irr {
        vector: u8,
    },
    /// The CPU accepted the vector: IRR cleared, ISR set, handler entered.
    IrqEntry {
        vector: u8,
    },
    /// The ISR bit was cleared (guest wrote the Local APIC EOI).
    Eoi {
        vector: u8,
    },
    PortIn {
        port: u16,
        value: u8,
    },
    PortOut {
        port: u16,
        value: u8,
    },
    Lidt,
    Sti,
    Hlt,
    Iretq,
    /// The halted guest had no wake source; virtual time jumped to the next RX deadline.
    IdleAdvance {
        to_ns: u64,
    },
}

impl UartIrqTraceKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::RxDelivered { .. } => "rx_delivered",
            Self::UartLine { .. } => "uart_line",
            Self::Irr { .. } => "irr",
            Self::IrqEntry { .. } => "irq_entry",
            Self::Eoi { .. } => "eoi",
            Self::PortIn { .. } => "port_in",
            Self::PortOut { .. } => "port_out",
            Self::Lidt => "lidt",
            Self::Sti => "sti",
            Self::Hlt => "hlt",
            Self::Iretq => "iretq",
            Self::IdleAdvance { .. } => "idle_advance",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UartIrqTraceEvent {
    pub seq: u64,
    pub clock_ns: u64,
    pub rip: u64,
    pub kind: UartIrqTraceKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4UartIrqProbeExit {
    /// The guest is halted and neither a due nor a future RX event can wake it.
    IdleHalt,
    BudgetExhausted,
    Stopped,
    Terminal(DiagnosticTerminalExit),
    /// A profile admission or interrupt-profile rejection (no state change).
    Rejected(String),
    IntegrationError(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4UartIrqProbeReport {
    pub exit: Stage4UartIrqProbeExit,
    /// Host steps taken by this call (instruction, delivery or idle step).
    pub quanta: u64,
    /// Guest instructions retired by this call.
    pub retired: u64,
}

fn layout(reason: &'static str) -> Stage4UartIrqProbeLoadError {
    Stage4UartIrqProbeLoadError::Layout(reason)
}

fn qword(image: &[u8], gpa: u64) -> u64 {
    let offset = (gpa - STAGE4_UART_IRQ_PROBE_IMAGE_GPA) as usize;
    u64::from_le_bytes(image[offset..offset + 8].try_into().unwrap())
}

fn validate_image(image: &[u8]) -> Result<(), Stage4UartIrqProbeLoadError> {
    if image.len() != STAGE4_UART_IRQ_PROBE_IMAGE_BYTES {
        return Err(Stage4UartIrqProbeLoadError::ImageSize {
            expected: STAGE4_UART_IRQ_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let at = |gpa: u64, len: usize| {
        let offset = (gpa - STAGE4_UART_IRQ_PROBE_IMAGE_GPA) as usize;
        &image[offset..offset + len]
    };
    for (index, descriptor) in GDT.iter().enumerate() {
        if qword(image, GDT_GPA + 8 * index as u64) != *descriptor {
            return Err(layout("GDT descriptors do not match the fixed profile"));
        }
    }
    // Exactly one gate (the INTIN4 vector): present, DPL0, 64-bit interrupt
    // gate, selector 8, IST 0, handler inside the code page.
    let vector = usize::from(STAGE4_UART_IRQ_PROBE_VECTOR);
    let gate = at(IDT_GPA + 16 * vector as u64, 16);
    let offset_low = u64::from(u16::from_le_bytes([gate[0], gate[1]]));
    let selector = u16::from_le_bytes([gate[2], gate[3]]);
    if selector != 8
        || gate[4] != 0
        || gate[5] != 0x8e
        || gate[6..16] != [0; 10]
        || !(CODE_GPA + LIDT_RSP0.len() as u64..CODE_GPA + 0x1000).contains(&offset_low)
    {
        return Err(layout(
            "IDT gate for the INTIN4 vector is outside the profile",
        ));
    }
    let idt = at(IDT_GPA, 0x1000);
    if idt[..vector * 16].iter().any(|b| *b != 0)
        || idt[(vector + 1) * 16..].iter().any(|b| *b != 0)
    {
        return Err(layout(
            "IDT must contain no gate other than the INTIN4 vector",
        ));
    }
    if at(CODE_GPA, LIDT_RSP0.len()) != LIDT_RSP0 {
        return Err(layout("entry must begin with LIDT [RSP+0]"));
    }
    let idtr = at(IDTR_GPA, 10);
    if u16::from_le_bytes([idtr[0], idtr[1]]) != 0x0fff
        || u64::from_le_bytes(idtr[2..10].try_into().unwrap()) != IDT_GPA
    {
        return Err(layout("IDTR pseudo-descriptor is outside the profile"));
    }
    let permitted = [
        (GDT_GPA, 0x18_u64),
        (IDT_GPA, 0x1000),
        (CODE_GPA, 0x1000),
        (IDTR_GPA, 10),
    ];
    for (offset, byte) in image.iter().enumerate() {
        let gpa = STAGE4_UART_IRQ_PROBE_IMAGE_GPA + offset as u64;
        if *byte != 0 && !permitted.iter().any(|(s, l)| (*s..*s + *l).contains(&gpa)) {
            return Err(layout("non-zero byte outside the fixed layout"));
        }
    }
    Ok(())
}

const LIDT_PATTERN: [u8; 4] = [0x0f, 0x01, 0x5c, 0x24];

/// UART-side owner of the pump (S4-27i5): RX queue, INTIN4 line, transmit.
struct UartOwner {
    uart: Uart16550Handle,
    io_apic: IoApicHandle,
    rx: UartRxEventPump,
    line: bool,
    tx: Vec<u8>,
}

impl UartOwner {
    fn event(name: &'static str, a: u64, b: u64) -> PumpHostEvent {
        PumpHostEvent { name, a, b }
    }

    /// Present level changes of the gated UART output to INTIN4. This is the
    /// only UART-to-IOAPIC connection and runs inside the session owner.
    fn sync_line(&mut self, events: &mut Vec<PumpHostEvent>) -> Result<(), PumpRejection> {
        let line = self.uart.interrupt_line();
        if line == self.line {
            return Ok(());
        }
        self.line = line;
        events.push(Self::event("uart_line", u64::from(line), 0));
        self.io_apic
            .set_intin4(line)
            .map_err(|error| PumpRejection::Host(format!("set_intin4({line}): {error:?}")))
    }

    /// Instant transmit: no baud timing, so a THR write completes at once.
    fn drain_tx(&mut self) {
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                self.tx.push(byte);
            }
        }
    }
}

impl PumpHost for UartOwner {
    fn dispatch(&mut self, now_ns: u64) -> Result<Vec<PumpHostEvent>, String> {
        let report = self
            .rx
            .dispatch_due(self.rx.generation(), now_ns, 8, || false)
            .map_err(|error| format!("{error:?}"))?;
        let mut events = Vec::new();
        if report.delivered > 0 {
            let byte = self.uart.received_byte().unwrap_or(0);
            events.push(Self::event("rx_delivered", u64::from(byte), 0));
            self.sync_line(&mut events).map_err(|error| match error {
                PumpRejection::Host(detail) => detail,
                other => format!("{other:?}"),
            })?;
        }
        Ok(events)
    }

    /// Only a future deadline can wake the guest: a due byte that is stuck
    /// behind an unread receive register does not count.
    fn next_deadline(&self, now_ns: u64) -> Option<u64> {
        self.rx
            .next_deadline()
            .filter(|deadline| *deadline > now_ns)
    }

    fn step(
        &mut self,
        session: &mut DiagnosticVcpuSession,
    ) -> Option<Result<PumpHostStep, PumpRejection>> {
        let code = session.peek_code(5);
        let rdx = session.state().gpr[Register::Rdx as usize] as u16;
        let rax = session.state().gpr[Register::Rax as usize] as u8;
        let is_lidt = code.len() == 5 && code[..4] == LIDT_PATTERN;
        let is_pio = matches!(code.first(), Some(0xec | 0xee));
        if !is_lidt && !is_pio {
            return None;
        }
        let outcome = if is_lidt {
            session.step_flat_lidt()
        } else {
            session.step_pio_u8()
        };
        let result: DiagnosticRunResult = match outcome {
            Ok(result) => result,
            Err(error) => return Some(Err(PumpRejection::Control(error))),
        };
        let mut events = Vec::new();
        let retired = matches!(result.exit, DiagnosticExit::WorkLimit(_))
            && result.instructions_executed == 1;
        if retired {
            if is_lidt {
                events.push(Self::event("lidt", 0, 0));
            } else {
                let is_in = code.first() == Some(&0xec);
                if (STAGE4_UART_IRQ_PROBE_COM1..STAGE4_UART_IRQ_PROBE_COM1 + 8).contains(&rdx) {
                    events.push(if is_in {
                        let al_after = session.state().gpr[Register::Rax as usize] as u8;
                        Self::event("port_in", u64::from(rdx), u64::from(al_after))
                    } else {
                        Self::event("port_out", u64::from(rdx), u64::from(rax))
                    });
                }
                self.drain_tx();
                if let Err(error) = self.sync_line(&mut events) {
                    return Some(Err(error));
                }
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

pub struct Stage4UartIrqProbe {
    pump: InterruptPump,
    uart: Uart16550Handle,
    trace: Vec<UartIrqTraceEvent>,
    /// `seq` of the first pump trace event not yet mapped into `trace`
    /// (S4-37a5: a seq cursor, so a `Ring` pump trace cannot shift it).
    next_pump_seq: u64,
    /// Pump events dropped by a `Ring` trace before they were mapped.
    lost_pump_events: u64,
}

fn trace_kind(kind: &PumpTraceKind) -> Option<UartIrqTraceKind> {
    Some(match kind {
        PumpTraceKind::Irr { vector } => UartIrqTraceKind::Irr { vector: *vector },
        PumpTraceKind::Entry { vector, source } => {
            let _: &PumpEntrySource = source;
            UartIrqTraceKind::IrqEntry { vector: *vector }
        }
        PumpTraceKind::Eoi { vector } => UartIrqTraceKind::Eoi { vector: *vector },
        PumpTraceKind::Sti => UartIrqTraceKind::Sti,
        PumpTraceKind::Hlt => UartIrqTraceKind::Hlt,
        PumpTraceKind::Iretq => UartIrqTraceKind::Iretq,
        PumpTraceKind::Host(event) => match event.name {
            "rx_delivered" => UartIrqTraceKind::RxDelivered {
                byte: event.a as u8,
            },
            "uart_line" => UartIrqTraceKind::UartLine { high: event.a != 0 },
            "port_in" => UartIrqTraceKind::PortIn {
                port: event.a as u16,
                value: event.b as u8,
            },
            "port_out" => UartIrqTraceKind::PortOut {
                port: event.a as u16,
                value: event.b as u8,
            },
            "lidt" => UartIrqTraceKind::Lidt,
            "idle_advance" => UartIrqTraceKind::IdleAdvance { to_ns: event.a },
            _ => return None,
        },
        // The UART probe has no CLI step and no IRQ event queue; the pump
        // records them for other owners.
        PumpTraceKind::Cli | PumpTraceKind::Coalesced { .. } => return None,
    })
}

fn map_exit(exit: PumpExit) -> Stage4UartIrqProbeExit {
    match exit {
        PumpExit::IdleHalt => Stage4UartIrqProbeExit::IdleHalt,
        PumpExit::Budget => Stage4UartIrqProbeExit::BudgetExhausted,
        PumpExit::Stopped => Stage4UartIrqProbeExit::Stopped,
        PumpExit::Terminal(cause) => Stage4UartIrqProbeExit::Terminal(cause),
        PumpExit::Rejected(rejection) => match rejection {
            PumpRejection::HaltEntry(reason) => {
                Stage4UartIrqProbeExit::Rejected(format!("HLT entry: {reason:?}"))
            }
            PumpRejection::HaltWake(reason) | PumpRejection::InterruptEntry(reason) => {
                Stage4UartIrqProbeExit::Rejected(format!("{reason:?}"))
            }
            PumpRejection::StiQuantumProfile => {
                Stage4UartIrqProbeExit::Rejected("STI quantum: ProfileRejected".into())
            }
            PumpRejection::ApicQuantumProfile => {
                Stage4UartIrqProbeExit::Rejected("APIC quantum: ProfileRejected".into())
            }
            PumpRejection::TraceLimit => {
                Stage4UartIrqProbeExit::IntegrationError("trace limit exceeded".into())
            }
            PumpRejection::NoRetirement(reason) => {
                Stage4UartIrqProbeExit::IntegrationError(format!("no retirement: {reason:?}"))
            }
            PumpRejection::Host(detail) => Stage4UartIrqProbeExit::IntegrationError(detail),
            other => Stage4UartIrqProbeExit::IntegrationError(format!("{other:?}")),
        },
    }
}

/// Validate a `Stage4UartIrqProbeV1` raw image, place it at GPA `0x1000` and
/// return the fixed Long64/CPL0 entry state with the interrupt-route owner.
pub fn load_stage4_uart_irq_probe(
    image: &[u8],
) -> Result<Stage4UartIrqProbe, Stage4UartIrqProbeLoadError> {
    validate_image(image)?;
    let region = |base: u64, len: u64, kind| {
        GuestPhysicalRange::new(GuestPhysicalAddress::new(base), len)
            .map(|range| MemoryRegionSpec::new(range, kind))
            .map_err(Stage4UartIrqProbeLoadError::Memory)
    };
    let map = MachineMemoryMap::new(
        36,
        vec![
            region(0, STAGE4_UART_IRQ_PROBE_RAM_BYTES, MemoryRegionKind::Ram)?,
            region(IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, MemoryRegionKind::Mmio)?,
            region(
                LOCAL_APIC_MMIO_BASE,
                LOCAL_APIC_MMIO_SIZE,
                MemoryRegionKind::Mmio,
            )?,
        ],
    )
    .map_err(Stage4UartIrqProbeLoadError::Memory)?;
    let local = LocalApic::new();
    let apic = local.handle();
    let io = IoApic::new(apic.clone());
    let io_apic = io.handle();
    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let uart_range = PortRange::new(PortAddress::new(STAGE4_UART_IRQ_PROBE_COM1), 8)
        .map_err(Stage4UartIrqProbeLoadError::BusBuild)?;
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_UART_IRQ_PROBE_RAM_BYTES),
        vec![Box::new(io), Box::new(local), Box::new(uart_device)],
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
        vec![PortRegionBinding {
            range: uart_range,
            device_index: 2,
        }],
    )
    .map_err(Stage4UartIrqProbeLoadError::BusBuild)?;
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(STAGE4_UART_IRQ_PROBE_IMAGE_GPA),
        image,
    )
    .map_err(Stage4UartIrqProbeLoadError::Memory)?;

    // IDTR is loaded by the guest's own `LIDT [RSP+0]`, executed by the owner.
    let state = flat_entry_state(
        STAGE4_UART_IRQ_PROBE_ENTRY,
        STAGE4_UART_IRQ_PROBE_STACK_TOP,
        Idtr { base: 0, limit: 0 },
    );
    let clock = VirtualClockHandle::new(DIAGNOSTIC_TSC_HZ)
        .map_err(|error| Stage4UartIrqProbeLoadError::Session(format!("{error:?}")))?;
    let session = DiagnosticVcpuSession::new_with_clock_handle(
        state,
        bus,
        flat_access_context(),
        INTEGER_FLAGS,
        clock,
        RdtscControls { cr4_tsd: false },
    )
    .map_err(|error| Stage4UartIrqProbeLoadError::Session(format!("{error:?}")))?;
    let owner = UartOwner {
        rx: UartRxEventPump::new(8, uart.clone()),
        uart: uart.clone(),
        io_apic,
        line: false,
        tx: Vec::new(),
    };
    let pump = InterruptPump::new(session, apic)
        .with_watched_vectors(&[STAGE4_UART_IRQ_PROBE_VECTOR])
        .with_host(Box::new(owner))
        .with_entry_only_delivery();
    Ok(Stage4UartIrqProbe {
        pump,
        uart,
        trace: Vec::new(),
        next_pump_seq: 0,
        lost_pump_events: 0,
    })
}

impl Stage4UartIrqProbe {
    fn owner(&self) -> &UartOwner {
        self.pump
            .host::<UartOwner>()
            .expect("the UART probe always has its owner")
    }

    fn owner_mut(&mut self) -> &mut UartOwner {
        self.pump
            .host_mut::<UartOwner>()
            .expect("the UART probe always has its owner")
    }

    pub fn state(&self) -> &LongModeState {
        self.pump.state()
    }

    pub fn clock_ns(&self) -> u64 {
        self.pump.virtual_time_ns().unwrap_or(0)
    }

    pub fn trace(&self) -> &[UartIrqTraceEvent] {
        &self.trace
    }

    /// Choose the trace mode of the underlying pump (S4-37a5); only before the
    /// first run. The probe's own `trace` keeps every mapped event either way.
    pub fn set_pump_trace_mode(&mut self, mode: PumpTraceMode) -> Result<(), PumpTraceModeError> {
        self.pump.set_trace_mode(mode)
    }

    /// Pump events a `Ring` trace dropped before `run` mapped them (S4-37a5);
    /// always 0 in `Bounded`, and in `Ring` when a run records at most `n`
    /// events.
    pub fn lost_pump_events(&self) -> u64 {
        self.lost_pump_events
    }

    /// Bytes the guest wrote to THR; transmission completes instantly (no baud timing).
    pub fn serial_bytes(&self) -> &[u8] {
        &self.owner().tx
    }

    pub fn uart(&self) -> &Uart16550Handle {
        &self.uart
    }

    pub fn retired_total(&self) -> u64 {
        self.pump.retired_total()
    }

    pub fn pending_rx(&self) -> usize {
        self.owner().rx.len()
    }

    pub fn rx_generation(&self) -> EventGeneration {
        self.owner().rx.generation()
    }

    /// `(IRR, ISR)` of the INTIN4 vector.
    pub fn vector_state(&self) -> (bool, bool) {
        self.pump.apic().vector_state(STAGE4_UART_IRQ_PROBE_VECTOR)
    }

    pub fn count(&self, probe: impl Fn(&UartIrqTraceKind) -> bool) -> usize {
        self.trace.iter().filter(|e| probe(&e.kind)).count()
    }

    /// Schedule one RX byte at an absolute virtual-time deadline.
    pub fn schedule_rx(
        &mut self,
        key: u64,
        deadline_ns: u64,
        value: u8,
    ) -> Result<(), Stage4UartIrqProbeError> {
        let rx = &mut self.owner_mut().rx;
        rx.schedule(rx.generation(), key, deadline_ns, value)
            .map_err(Stage4UartIrqProbeError::Queue)
    }

    /// Advance virtual time explicitly (only meaningful with RX deadlines).
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), Stage4UartIrqProbeError> {
        self.pump
            .advance_time_ns(delta_ns)
            .map_err(|rejection| match rejection {
                PumpRejection::Control(DiagnosticVcpuControlError::Clock(error))
                | PumpRejection::TimerAdvance(DiagnosticHaltWaitError::Clock(error)) => {
                    Stage4UartIrqProbeError::Clock(error)
                }
                other => Stage4UartIrqProbeError::ClockState(format!("{other:?}")),
            })
    }

    /// Map pump trace events recorded since the last call into the S4-24e trace.
    fn sync_trace(&mut self) {
        let first = self.pump.trace_first_seq();
        if self.next_pump_seq < first {
            self.lost_pump_events += first - self.next_pump_seq;
        }
        for event in self.pump.trace_since(self.next_pump_seq) {
            if let Some(kind) = trace_kind(&event.kind) {
                self.trace.push(UartIrqTraceEvent {
                    seq: self.trace.len() as u64,
                    clock_ns: event.clock_ns.unwrap_or(0),
                    rip: event.rip,
                    kind,
                });
            }
        }
        self.next_pump_seq = self.pump.trace_next_seq();
    }

    /// Run until idle halt, budget, STOP, a terminal result or an integration error.
    pub fn run(&mut self, budget: u64, stop: impl FnMut() -> bool) -> Stage4UartIrqProbeReport {
        let result = self.pump.run(budget, stop);
        self.sync_trace();
        Stage4UartIrqProbeReport {
            exit: map_exit(result.exit),
            quanta: result.quanta,
            retired: result.retired,
        }
    }
}
