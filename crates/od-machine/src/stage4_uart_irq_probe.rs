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

use crate::diagnostic_vcpu::DiagnosticIrqEntry;
use crate::{
    BusBuildError, DiagnosticExit, DiagnosticHaltWait, DiagnosticHaltWakeResult,
    DiagnosticIretqResult, DiagnosticRunResult, DiagnosticStiApicQuantumResult,
    DiagnosticTerminalExit, DiagnosticVcpuSession, EventGeneration, EventQueueError,
    GuestPhysicalAddress, GuestPhysicalRange, IO_APIC_MMIO_BASE, IO_APIC_MMIO_SIZE, IoApic,
    IoApicHandle, LOCAL_APIC_MMIO_BASE, LOCAL_APIC_MMIO_SIZE, LocalApic, LocalApicHandle,
    MachineBus, MachineMemoryMap, MemoryAccessError, MemoryRegionKind, MemoryRegionSpec,
    MmioBinding, PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle,
    Uart16550PollingTx, UartRxEventPump, VirtualClock, VirtualClockError,
};
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    CodeSegmentDescriptorCache, CpuActivity, INTEGER_FLAGS, LongModeState, Register,
    StackSegmentDescriptorCache, StiShadow,
};

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

enum Cpu {
    Running(DiagnosticVcpuSession),
    Waiting(DiagnosticHaltWait),
}

pub struct Stage4UartIrqProbe {
    cpu: Option<Cpu>,
    uart: Uart16550Handle,
    io_apic: IoApicHandle,
    apic: LocalApicHandle,
    rx: UartRxEventPump,
    clock: VirtualClock,
    line: bool,
    vector_state: (bool, bool),
    tx: Vec<u8>,
    trace: Vec<UartIrqTraceEvent>,
    retired_total: u64,
    finished: Option<Stage4UartIrqProbeExit>,
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

    let mut state = LongModeState {
        rip: STAGE4_UART_IRQ_PROBE_ENTRY,
        rflags: 2,
        cs_selector: Some(8),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9b,
            flags: 2,
        }),
        ss_selector: Some(0x10),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_GPA,
            limit: 0x17,
        }),
        idtr: Some(Idtr { base: 0, limit: 0 }),
        activity: Some(CpuActivity::Running),
        sti_shadow: Some(StiShadow::Clear),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = STAGE4_UART_IRQ_PROBE_STACK_TOP;
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let session = DiagnosticVcpuSession::new(state, bus, context, INTEGER_FLAGS)
        .map_err(|error| Stage4UartIrqProbeLoadError::Session(format!("{error:?}")))?;
    let clock = VirtualClock::new(DIAGNOSTIC_TSC_HZ)
        .map_err(|error| Stage4UartIrqProbeLoadError::Session(format!("{error:?}")))?;
    Ok(Stage4UartIrqProbe {
        cpu: Some(Cpu::Running(session)),
        rx: UartRxEventPump::new(8, uart.clone()),
        uart,
        io_apic,
        apic,
        clock,
        line: false,
        vector_state: (false, false),
        tx: Vec::new(),
        trace: Vec::new(),
        retired_total: 0,
        finished: None,
    })
}

enum Flow {
    Continue,
    Exit(Stage4UartIrqProbeExit),
}

impl Stage4UartIrqProbe {
    pub fn state(&self) -> &LongModeState {
        match self.cpu.as_ref().expect("cpu is always restored") {
            Cpu::Running(session) => session.state(),
            Cpu::Waiting(wait) => wait.state(),
        }
    }

    pub fn clock_ns(&self) -> u64 {
        self.clock.now_ns()
    }

    pub fn trace(&self) -> &[UartIrqTraceEvent] {
        &self.trace
    }

    /// Bytes the guest wrote to THR; transmission completes instantly (no baud timing).
    pub fn serial_bytes(&self) -> &[u8] {
        &self.tx
    }

    pub fn uart(&self) -> &Uart16550Handle {
        &self.uart
    }

    pub fn retired_total(&self) -> u64 {
        self.retired_total
    }

    pub fn pending_rx(&self) -> usize {
        self.rx.len()
    }

    pub fn rx_generation(&self) -> EventGeneration {
        self.rx.generation()
    }

    /// `(IRR, ISR)` of the INTIN4 vector.
    pub fn vector_state(&self) -> (bool, bool) {
        self.apic.vector_state(STAGE4_UART_IRQ_PROBE_VECTOR)
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
        self.rx
            .schedule(self.rx.generation(), key, deadline_ns, value)
            .map_err(Stage4UartIrqProbeError::Queue)
    }

    /// Advance virtual time explicitly (only meaningful with RX deadlines).
    pub fn advance_time_ns(&mut self, delta_ns: u64) -> Result<(), Stage4UartIrqProbeError> {
        self.clock
            .advance_time_ns(delta_ns)
            .map_err(Stage4UartIrqProbeError::Clock)
    }

    fn push(&mut self, kind: UartIrqTraceKind) -> Result<(), Stage4UartIrqProbeExit> {
        if self.trace.len() >= STAGE4_UART_IRQ_PROBE_TRACE_LIMIT {
            return Err(Stage4UartIrqProbeExit::IntegrationError(
                "trace limit exceeded".into(),
            ));
        }
        let rip = self.state().rip;
        self.trace.push(UartIrqTraceEvent {
            seq: self.trace.len() as u64,
            clock_ns: self.clock.now_ns(),
            rip,
            kind,
        });
        Ok(())
    }

    /// Present level changes of the gated UART output to INTIN4. This is the
    /// only UART-to-IOAPIC connection and runs inside the session owner.
    fn sync_line(&mut self) -> Result<(), Stage4UartIrqProbeExit> {
        let line = self.uart.interrupt_line();
        if line == self.line {
            return Ok(());
        }
        self.line = line;
        self.push(UartIrqTraceKind::UartLine { high: line })?;
        self.io_apic.set_intin4(line).map_err(|error| {
            Stage4UartIrqProbeExit::IntegrationError(format!("set_intin4({line}): {error:?}"))
        })
    }

    /// Emit LAPIC IRR/ISR transitions for the vector observed since the last call.
    fn observe(&mut self) -> Result<(), Stage4UartIrqProbeExit> {
        let (prev_irr, prev_isr) = self.vector_state;
        let (irr, isr) = self.vector_state();
        self.vector_state = (irr, isr);
        let vector = STAGE4_UART_IRQ_PROBE_VECTOR;
        if prev_isr && !isr {
            self.push(UartIrqTraceKind::Eoi { vector })?;
        }
        if !prev_irr && irr {
            self.push(UartIrqTraceKind::Irr { vector })?;
        }
        if prev_irr && !irr && isr && !prev_isr {
            self.push(UartIrqTraceKind::IrqEntry { vector })?;
        }
        Ok(())
    }

    fn dispatch_due_rx(&mut self) -> Result<(), Stage4UartIrqProbeExit> {
        let report = self
            .rx
            .dispatch_due(self.rx.generation(), self.clock.now_ns(), 8, || false)
            .map_err(|error| Stage4UartIrqProbeExit::IntegrationError(format!("{error:?}")))?;
        if report.delivered > 0 {
            let byte = self.uart.received_byte().unwrap_or(0);
            self.push(UartIrqTraceKind::RxDelivered { byte })?;
            self.sync_line()?;
            self.observe()?;
        }
        Ok(())
    }

    /// Instant transmit: no baud timing, so a THR write completes at once.
    fn drain_tx(&mut self) {
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                self.tx.push(byte);
            }
        }
    }

    /// Run until idle halt, budget, STOP, a terminal result or an integration error.
    pub fn run(&mut self, budget: u64, mut stop: impl FnMut() -> bool) -> Stage4UartIrqProbeReport {
        let retired_before = self.retired_total;
        let mut quanta = 0;
        let exit = loop {
            if let Some(exit) = &self.finished {
                break exit.clone();
            }
            if stop() {
                break Stage4UartIrqProbeExit::Stopped;
            }
            if quanta >= budget {
                break Stage4UartIrqProbeExit::BudgetExhausted;
            }
            match self.step() {
                Ok(Flow::Continue) => quanta += 1,
                Ok(Flow::Exit(exit)) => {
                    if !matches!(exit, Stage4UartIrqProbeExit::IdleHalt) {
                        self.finished = Some(exit.clone());
                    }
                    break exit;
                }
                Err(exit) => {
                    self.finished = Some(exit.clone());
                    break exit;
                }
            }
        };
        Stage4UartIrqProbeReport {
            exit,
            quanta,
            retired: self.retired_total - retired_before,
        }
    }

    fn terminal(exit: DiagnosticTerminalExit) -> Flow {
        Flow::Exit(Stage4UartIrqProbeExit::Terminal(exit))
    }

    fn account(
        &mut self,
        result: DiagnosticRunResult,
    ) -> Result<Option<Flow>, Stage4UartIrqProbeExit> {
        self.retired_total += result.instructions_executed;
        match result.exit {
            DiagnosticExit::Terminal(cause) => Ok(Some(Self::terminal(cause))),
            DiagnosticExit::Stopped => Ok(Some(Flow::Exit(Stage4UartIrqProbeExit::Stopped))),
            DiagnosticExit::WorkLimit(_) if result.instructions_executed == 1 => Ok(None),
            DiagnosticExit::WorkLimit(reason) => Ok(Some(Flow::Exit(
                Stage4UartIrqProbeExit::IntegrationError(format!("no retirement: {reason:?}")),
            ))),
        }
    }

    fn step(&mut self) -> Result<Flow, Stage4UartIrqProbeExit> {
        self.dispatch_due_rx()?;
        let cpu = self.cpu.take().expect("cpu is always restored");
        match cpu {
            Cpu::Waiting(wait) => self.step_waiting(wait),
            Cpu::Running(session) => self.step_running(session),
        }
    }

    fn step_waiting(
        &mut self,
        mut wait: DiagnosticHaltWait,
    ) -> Result<Flow, Stage4UartIrqProbeExit> {
        let polled = wait.poll_apic(&self.apic);
        match polled {
            DiagnosticHaltWakeResult::Entered { .. } => match wait.into_running() {
                Ok(session) => {
                    self.cpu = Some(Cpu::Running(session));
                    self.observe()?;
                    Ok(Flow::Continue)
                }
                Err(wait) => {
                    self.cpu = Some(Cpu::Waiting(wait));
                    Ok(Flow::Exit(Stage4UartIrqProbeExit::IntegrationError(
                        "halt wake did not return a running session".into(),
                    )))
                }
            },
            DiagnosticHaltWakeResult::Waiting => {
                self.cpu = Some(Cpu::Waiting(wait));
                let now = self.clock.now_ns();
                match self.rx.next_deadline() {
                    Some(deadline) if deadline > now => {
                        self.clock
                            .advance_time_ns(deadline - now)
                            .map_err(|error| {
                                Stage4UartIrqProbeExit::IntegrationError(format!("{error:?}"))
                            })?;
                        self.push(UartIrqTraceKind::IdleAdvance { to_ns: deadline })?;
                        Ok(Flow::Continue)
                    }
                    // Nothing can wake the guest: a future event is absent, or
                    // a due byte is stuck behind an unread receive register.
                    _ => Ok(Flow::Exit(Stage4UartIrqProbeExit::IdleHalt)),
                }
            }
            DiagnosticHaltWakeResult::OutsideProfile(reason) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Flow::Exit(Stage4UartIrqProbeExit::Rejected(format!(
                    "{reason:?}"
                ))))
            }
            DiagnosticHaltWakeResult::Stopped => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Flow::Exit(Stage4UartIrqProbeExit::Stopped))
            }
            DiagnosticHaltWakeResult::Terminal(cause) => {
                self.cpu = Some(Cpu::Waiting(wait));
                Ok(Self::terminal(cause))
            }
        }
    }

    fn step_running(
        &mut self,
        mut session: DiagnosticVcpuSession,
    ) -> Result<Flow, Stage4UartIrqProbeExit> {
        let control = |error| Stage4UartIrqProbeExit::IntegrationError(format!("{error:?}"));
        match session.deliver_pending_irq(&self.apic) {
            Ok(Some(DiagnosticIrqEntry::Entered { .. })) => {
                self.cpu = Some(Cpu::Running(session));
                self.observe()?;
                return Ok(Flow::Continue);
            }
            Ok(Some(DiagnosticIrqEntry::OutsideProfile(reason))) => {
                self.cpu = Some(Cpu::Running(session));
                return Ok(Flow::Exit(Stage4UartIrqProbeExit::Rejected(format!(
                    "{reason:?}"
                ))));
            }
            Ok(Some(DiagnosticIrqEntry::Terminal(cause))) => {
                self.cpu = Some(Cpu::Running(session));
                return Ok(Self::terminal(cause));
            }
            Ok(None) => {}
            Err(error) => {
                self.cpu = Some(Cpu::Running(session));
                return Err(control(error));
            }
        }
        let code = session.peek_code(5);
        let rdx = session.state().gpr[Register::Rdx as usize] as u16;
        let rax = session.state().gpr[Register::Rax as usize] as u8;
        let mut event = None;
        let outcome = match code.as_slice() {
            [0xf4, ..] => {
                return match session.into_halt_wait() {
                    Ok(wait) => {
                        self.cpu = Some(Cpu::Waiting(wait));
                        self.push(UartIrqTraceKind::Hlt)?;
                        self.retired_total += 1;
                        Ok(Flow::Continue)
                    }
                    Err(failure) => {
                        self.cpu = Some(Cpu::Running(failure.session));
                        Ok(Flow::Exit(Stage4UartIrqProbeExit::Rejected(format!(
                            "HLT entry: {:?}",
                            failure.reason
                        ))))
                    }
                };
            }
            [0xfb, ..] => {
                event = Some(UartIrqTraceKind::Sti);
                match session.run_sti_apic_quantum(&self.apic) {
                    DiagnosticStiApicQuantumResult::Ordinary(result)
                    | DiagnosticStiApicQuantumResult::TerminalExecution(result) => Ok(result),
                    other => {
                        self.cpu = Some(Cpu::Running(session));
                        return Ok(Flow::Exit(Stage4UartIrqProbeExit::Rejected(format!(
                            "STI quantum: {other:?}"
                        ))));
                    }
                }
            }
            [0x0f, 0x01, 0x5c, 0x24, _] => {
                event = Some(UartIrqTraceKind::Lidt);
                session.step_flat_lidt()
            }
            [0xec | 0xee, ..] => session.step_pio_u8(),
            [0x48, 0xcf, ..] => {
                event = Some(UartIrqTraceKind::Iretq);
                match session.run_same_cpl0_iretq() {
                    Ok(DiagnosticIretqResult::Retired) => Ok(DiagnosticRunResult {
                        exit: DiagnosticExit::WorkLimit(crate::WorkLimitReason::InstructionBudget),
                        instructions_executed: 1,
                        rflags_defined: session.rflags_defined(),
                    }),
                    Ok(DiagnosticIretqResult::Terminal(cause)) => Ok(DiagnosticRunResult {
                        exit: DiagnosticExit::Terminal(cause),
                        instructions_executed: 0,
                        rflags_defined: session.rflags_defined(),
                    }),
                    Err(error) => Err(error),
                }
            }
            _ => session.run(1, || false),
        };
        let is_pio = matches!(code.first(), Some(0xec | 0xee));
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                self.cpu = Some(Cpu::Running(session));
                return Err(control(error));
            }
        };
        let al_after = session.state().gpr[Register::Rax as usize] as u8;
        self.cpu = Some(Cpu::Running(session));
        if let Some(flow) = self.account(result.clone())? {
            return Ok(flow);
        }
        if is_pio {
            let is_in = code.first() == Some(&0xec);
            if (STAGE4_UART_IRQ_PROBE_COM1..STAGE4_UART_IRQ_PROBE_COM1 + 8).contains(&rdx) {
                self.push(if is_in {
                    UartIrqTraceKind::PortIn {
                        port: rdx,
                        value: al_after,
                    }
                } else {
                    UartIrqTraceKind::PortOut {
                        port: rdx,
                        value: rax,
                    }
                })?;
            }
            self.drain_tx();
            self.sync_line()?;
        }
        if let Some(kind) = event {
            self.push(kind)?;
        }
        self.observe()?;
        Ok(Flow::Continue)
    }
}
