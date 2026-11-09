//! S4-29d: guest TSC calibration on the 8254 channel 2 in virtual time.
//!
//! `Stage4PitCalibrateProbeV1` is a flat identity-mapped Long64/CPL0 guest that
//! repeats Linux `pit_calibrate_tsc` and the MSB form of `quick_pit_calibrate`
//! (see the fixture `entry.S`). The session owner built here is the only place
//! where the port tick is applied: the PIT port-B device counts every read of
//! `0x61` and queues one microsecond; after the instruction retires, the owner
//! takes the queue and advances the one virtual clock by it (consultation
//! decision 5, ISSUE-067). RDTSC reads the same clock, so the polling loop sees
//! both OUT2 and the TSC move. Nothing else moves time: instructions, writes and
//! reads of the counter ports take no virtual time in this probe.
//!
//! The injection [`Stage4PitCalibrateInjection::NoPortTick`] makes the owner
//! drop the queued tick, which is the pre-S4-29d behaviour: time stands still
//! inside the polling loop and the guest spins until the step budget runs out.

use crate::{
    BusBuildError, DiagnosticExit, DiagnosticRunResult, DiagnosticTerminalExit,
    DiagnosticVcpuSession, GuestPhysicalAddress, GuestPhysicalRange, MachineBus, MachineMemoryMap,
    MemoryAccessError, MemoryRegionKind, MemoryRegionSpec, PIT_BASE_PORT, PIT_PORT_B,
    PIT_PORT_B_BYTES, PIT_PORT_B_READ_TICK_NS, PIT_PORT_BYTES, Pit8254Devices, Pit8254Handle,
    PortAddress, PortRange, PortRegionBinding, RamLimits, Uart16550Handle, Uart16550PollingTx,
    VirtualClockHandle,
};
use od_isa::execute::RdtscControls;
use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
use od_isa::state::{Gdtr, Idtr};
use od_isa::{
    CodeSegmentDescriptorCache, CpuActivity, INTEGER_FLAGS, LongModeState, Register,
    StackSegmentDescriptorCache, StiShadow,
};

pub const STAGE4_PIT_CALIBRATE_PROBE_IMAGE_GPA: u64 = 0x1000;
pub const STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES: usize = 0x4000;
pub const STAGE4_PIT_CALIBRATE_PROBE_RAM_BYTES: u64 = 0x7000;
pub const STAGE4_PIT_CALIBRATE_PROBE_ENTRY: u64 = 0x4000;
pub const STAGE4_PIT_CALIBRATE_PROBE_STACK_TOP: u64 = 0x6ff0;
pub const STAGE4_PIT_CALIBRATE_PROBE_COM1: u16 = 0x3f8;
/// One TSC tick per nanosecond of virtual time.
pub const STAGE4_PIT_CALIBRATE_PROBE_TSC_HZ: u64 = 1_000_000_000;
/// Length of the serial record the guest writes on success.
pub const STAGE4_PIT_CALIBRATE_RECORD_BYTES: usize = 77;
/// A frequency the guest reports when its sanity check or threshold failed.
pub const STAGE4_PIT_CALIBRATE_FAILED_FREQUENCY: u64 = u64::MAX;
const GDT_GPA: u64 = 0x2000;
const CODE_GPA: u64 = 0x4000;
const GDT: [u64; 3] = [0, 0x00af_9b00_0000_ffff, 0x00cf_9300_0000_ffff];

#[derive(Debug, Eq, PartialEq)]
pub enum Stage4PitCalibrateProbeLoadError {
    ImageSize { expected: usize, actual: usize },
    Layout(&'static str),
    Memory(MemoryAccessError),
    BusBuild(BusBuildError),
    Session(String),
}

/// Injected regressions of the case, applied by the session owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage4PitCalibrateInjection {
    None,
    /// The owner drops the queued port tick: reads of `0x61` do not move time.
    NoPortTick,
    /// Negative control: the owner advances `ns_per_read` per read of `0x61`
    /// instead of 1 us, so the guest's own thresholds (`loopmin`, the MSB pass
    /// count) must trip.
    CoarsePortTick {
        ns_per_read: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage4PitCalibrateExit {
    /// The guest executed HLT (it has written its serial record).
    Halted,
    /// The step budget ran out; the guest is still polling.
    BudgetExhausted,
    Stopped,
    /// A fault, unsupported opcode or backend/port error ended the guest.
    Terminal(DiagnosticTerminalExit),
    IntegrationError(String),
}

impl Stage4PitCalibrateExit {
    /// Stable STOP-reason name used in reports.
    pub fn name(&self) -> &'static str {
        match self {
            Self::Halted => "halted",
            Self::BudgetExhausted => "budget_exhausted",
            Self::Stopped => "stopped",
            Self::Terminal(_) => "terminal",
            Self::IntegrationError(_) => "integration_error",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage4PitCalibrateReport {
    pub exit: Stage4PitCalibrateExit,
    /// Host steps taken by this call (one retired instruction each).
    pub steps: u64,
    /// Guest instructions retired by this call.
    pub retired: u64,
}

/// The serial record of a completed run, decoded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Stage4PitCalibrateRecord {
    /// 0 ok, 1 sanity failure (`pitcnt < 1000` or `tscmax > 10 * tscmin`).
    pub status_a: u8,
    pub freq_khz_a: u64,
    pub delta_tsc_a: u64,
    pub pitcnt: u64,
    pub tscmin: u64,
    pub tscmax: u64,
    /// 0 ok, 2 stage 1 had at most 5 passes, 3 stage 2 had at most 5 passes
    /// (3 wins when both failed).
    pub status_b: u8,
    pub freq_khz_b: u64,
    pub delta_tsc_b: u64,
    pub stage1_passes: u64,
    pub stage2_passes: u64,
}

impl Stage4PitCalibrateRecord {
    /// Decodes exactly the 77-byte record `A s [5 x u64] B s [4 x u64] Z`.
    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() != STAGE4_PIT_CALIBRATE_RECORD_BYTES
            || bytes[0] != b'A'
            || bytes[42] != b'B'
            || bytes[76] != b'Z'
        {
            return None;
        }
        let word = |at: usize| u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap());
        Some(Self {
            status_a: bytes[1],
            freq_khz_a: word(2),
            delta_tsc_a: word(10),
            pitcnt: word(18),
            tscmin: word(26),
            tscmax: word(34),
            status_b: bytes[43],
            freq_khz_b: word(44),
            delta_tsc_b: word(52),
            stage1_passes: word(60),
            stage2_passes: word(68),
        })
    }

    /// Both calibrations passed their own sanity checks and thresholds.
    pub fn guest_accepted(&self) -> bool {
        self.status_a == 0 && self.status_b == 0
    }
}

pub struct Stage4PitCalibrateProbe {
    session: DiagnosticVcpuSession,
    pit: Pit8254Handle,
    uart: Uart16550Handle,
    clock: VirtualClockHandle,
    injection: Stage4PitCalibrateInjection,
    tx: Vec<u8>,
    retired_total: u64,
    steps_total: u64,
    finished: Option<Stage4PitCalibrateExit>,
}

fn layout(reason: &'static str) -> Stage4PitCalibrateProbeLoadError {
    Stage4PitCalibrateProbeLoadError::Layout(reason)
}

fn validate_image(image: &[u8]) -> Result<(), Stage4PitCalibrateProbeLoadError> {
    if image.len() != STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES {
        return Err(Stage4PitCalibrateProbeLoadError::ImageSize {
            expected: STAGE4_PIT_CALIBRATE_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    let at = |gpa: u64, len: usize| {
        let offset = (gpa - STAGE4_PIT_CALIBRATE_PROBE_IMAGE_GPA) as usize;
        &image[offset..offset + len]
    };
    for (index, descriptor) in GDT.iter().enumerate() {
        let bytes = at(GDT_GPA + 8 * index as u64, 8);
        if u64::from_le_bytes(bytes.try_into().unwrap()) != *descriptor {
            return Err(layout("GDT descriptors do not match the fixed profile"));
        }
    }
    if at(CODE_GPA, 1)[0] == 0 {
        return Err(layout("the code page must start with the entry code"));
    }
    let permitted = [(GDT_GPA, 0x18_u64), (CODE_GPA, 0x1000)];
    for (offset, byte) in image.iter().enumerate() {
        let gpa = STAGE4_PIT_CALIBRATE_PROBE_IMAGE_GPA + offset as u64;
        if *byte != 0 && !permitted.iter().any(|(s, l)| (*s..*s + *l).contains(&gpa)) {
            return Err(layout("non-zero byte outside the fixed layout"));
        }
    }
    Ok(())
}

/// Validate a `Stage4PitCalibrateProbeV1` raw image, place it at GPA `0x1000`
/// and return the fixed Long64/CPL0 entry state with its clock owner.
pub fn load_stage4_pit_calibrate_probe(
    image: &[u8],
    injection: Stage4PitCalibrateInjection,
) -> Result<Stage4PitCalibrateProbe, Stage4PitCalibrateProbeLoadError> {
    validate_image(image)?;
    let ram = GuestPhysicalRange::new(
        GuestPhysicalAddress::new(0),
        STAGE4_PIT_CALIBRATE_PROBE_RAM_BYTES,
    )
    .map_err(Stage4PitCalibrateProbeLoadError::Memory)?;
    let map = MachineMemoryMap::new(36, vec![MemoryRegionSpec::new(ram, MemoryRegionKind::Ram)])
        .map_err(Stage4PitCalibrateProbeLoadError::Memory)?;
    let clock = VirtualClockHandle::new(STAGE4_PIT_CALIBRATE_PROBE_TSC_HZ)
        .map_err(|error| Stage4PitCalibrateProbeLoadError::Session(format!("{error:?}")))?;
    let devices = Pit8254Devices::new(clock.clone());
    let pit = devices.handle.clone();
    let (uart_device, uart) = Uart16550PollingTx::new_with_handle();
    let port = |base: u16, len: u32, device_index: usize| {
        PortRange::new(PortAddress::new(base), len)
            .map(|range| PortRegionBinding {
                range,
                device_index,
            })
            .map_err(Stage4PitCalibrateProbeLoadError::BusBuild)
    };
    let mut bus = MachineBus::new(
        map,
        RamLimits::new(STAGE4_PIT_CALIBRATE_PROBE_RAM_BYTES),
        vec![
            Box::new(devices.pit),
            Box::new(devices.port_b),
            Box::new(uart_device),
        ],
        vec![],
        vec![
            port(PIT_BASE_PORT, PIT_PORT_BYTES, 0)?,
            port(PIT_PORT_B, PIT_PORT_B_BYTES, 1)?,
            port(STAGE4_PIT_CALIBRATE_PROBE_COM1, 8, 2)?,
        ],
    )
    .map_err(Stage4PitCalibrateProbeLoadError::BusBuild)?;
    bus.patch_physical_ram(
        GuestPhysicalAddress::new(STAGE4_PIT_CALIBRATE_PROBE_IMAGE_GPA),
        image,
    )
    .map_err(Stage4PitCalibrateProbeLoadError::Memory)?;

    let mut state = LongModeState {
        rip: STAGE4_PIT_CALIBRATE_PROBE_ENTRY,
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
    state.gpr[Register::Rsp as usize] = STAGE4_PIT_CALIBRATE_PROBE_STACK_TOP;
    let context = AccessContext {
        mode: InstructionMode::Long64,
        privilege: PrivilegeLevel::Ring0,
        origin: AccessOrigin::Explicit,
    };
    let session = DiagnosticVcpuSession::new_with_clock_handle(
        state,
        bus,
        context,
        INTEGER_FLAGS,
        clock.clone(),
        RdtscControls { cr4_tsd: false },
    )
    .map_err(|error| Stage4PitCalibrateProbeLoadError::Session(format!("{error:?}")))?;
    Ok(Stage4PitCalibrateProbe {
        session,
        pit,
        uart,
        clock,
        injection,
        tx: Vec::new(),
        retired_total: 0,
        steps_total: 0,
        finished: None,
    })
}

impl Stage4PitCalibrateProbe {
    pub fn state(&self) -> &LongModeState {
        self.session.state()
    }

    pub fn clock_ns(&self) -> u64 {
        self.clock.snapshot().now_ns()
    }

    pub fn pit(&self) -> &Pit8254Handle {
        &self.pit
    }

    /// Successful reads of port `0x61` so far.
    pub fn port_b_reads(&self) -> u64 {
        self.pit.port_b_reads()
    }

    /// Bytes the guest wrote to COM1 THR; transmission completes instantly.
    pub fn serial_bytes(&self) -> &[u8] {
        &self.tx
    }

    pub fn record(&self) -> Option<Stage4PitCalibrateRecord> {
        Stage4PitCalibrateRecord::decode(&self.tx)
    }

    pub fn retired_total(&self) -> u64 {
        self.retired_total
    }

    pub fn steps_total(&self) -> u64 {
        self.steps_total
    }

    pub fn injection(&self) -> Stage4PitCalibrateInjection {
        self.injection
    }

    /// Run until HLT, the step budget, STOP, a terminal result or an integration error.
    pub fn run(&mut self, budget: u64, mut stop: impl FnMut() -> bool) -> Stage4PitCalibrateReport {
        let retired_before = self.retired_total;
        let mut steps = 0;
        let exit = loop {
            if let Some(exit) = &self.finished {
                break exit.clone();
            }
            if stop() {
                break Stage4PitCalibrateExit::Stopped;
            }
            if steps >= budget {
                break Stage4PitCalibrateExit::BudgetExhausted;
            }
            steps += 1;
            self.steps_total += 1;
            if let Some(exit) = self.step() {
                self.finished = Some(exit.clone());
                break exit;
            }
        };
        Stage4PitCalibrateReport {
            exit,
            steps,
            retired: self.retired_total - retired_before,
        }
    }

    /// Instant transmit: no baud timing, so a THR write completes at once.
    fn drain_tx(&mut self) {
        while self.uart.transfer_holding_to_shift() {
            if let Some(byte) = self.uart.complete_transmission() {
                self.tx.push(byte);
            }
        }
    }

    fn step(&mut self) -> Option<Stage4PitCalibrateExit> {
        let code = self.session.peek_code(1);
        if code.first() == Some(&0xf4) {
            // HLT with interrupts masked and no wake source: the guest is done.
            // The diagnostic run loop does not execute HLT outside a halt wait,
            // so the owner retires it and ends the run.
            self.retired_total += 1;
            return Some(Stage4PitCalibrateExit::Halted);
        }
        let is_pio = matches!(code.first(), Some(0xec | 0xee));
        let outcome: Result<DiagnosticRunResult, _> = if is_pio {
            self.session.step_pio_u8()
        } else {
            self.session.run(1, || false)
        };
        let result = match outcome {
            Ok(result) => result,
            Err(error) => {
                return Some(Stage4PitCalibrateExit::IntegrationError(format!(
                    "{error:?}"
                )));
            }
        };
        self.retired_total += result.instructions_executed;
        match result.exit {
            DiagnosticExit::Terminal(cause) => {
                return Some(Stage4PitCalibrateExit::Terminal(cause));
            }
            DiagnosticExit::Stopped => return Some(Stage4PitCalibrateExit::Stopped),
            DiagnosticExit::WorkLimit(_) if result.instructions_executed == 1 => {}
            DiagnosticExit::WorkLimit(reason) => {
                return Some(Stage4PitCalibrateExit::IntegrationError(format!(
                    "no retirement: {reason:?}"
                )));
            }
        }
        if is_pio {
            self.drain_tx();
            // The port tick: the device queued the time of any 0x61 read; the
            // clock owner applies it after the instruction (or drops it under
            // the injected regression).
            let queued = self.pit.take_port_tick_ns();
            let ns = match self.injection {
                Stage4PitCalibrateInjection::None => queued,
                Stage4PitCalibrateInjection::NoPortTick => 0,
                Stage4PitCalibrateInjection::CoarsePortTick { ns_per_read } => {
                    queued / PIT_PORT_B_READ_TICK_NS * ns_per_read
                }
            };
            if ns > 0
                && let Err(error) = self.session.advance_time_ns(ns)
            {
                return Some(Stage4PitCalibrateExit::IntegrationError(format!(
                    "port tick: {error:?}"
                )));
            }
        }
        None
    }
}
