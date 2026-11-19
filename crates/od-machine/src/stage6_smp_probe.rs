//! S6-16e/f: loader and runner of the `Stage6SmpProbeV2` guest (INIT/SIPI start of an application
//! processor by guest code, Fixed IPI exchange and the negative cases of S6-16f; S6-17: the
//! `shootdown` scenario, a guest TLB shootdown between two processors on shared page tables).
//!
//! The image is built by `tools/build/build_stage6_smp_probe.py` (hand-encoded bytes, GPA
//! `0x2000..0xD000`). It holds the page tables (identity map of the first 2 MiB and the Local APIC
//! window), the GDT, the trampoline of the application processor, the strings and the code of both
//! processors. This module
//!
//! * builds a board with `vcpu_count` vCPUs (default firmware configuration, the Local APIC MMIO
//!   windows on, the clock owner on), places the image and writes the 64-byte info block at
//!   [`STAGE6_SMP_PROBE_INFO_GPA`] (`magic, version, vcpu_count, nonce_len, nonce`): the nonce is
//!   never part of the image;
//! * gives vCPU 0 (the boot processor) the state of the stage 4 probes: 64-bit, CPL0, IF = 0,
//!   `CR3` = the image's PML4 (the board's `page_table_root` too), `EFER` = SCE | LME | LMA | NXE,
//!   (the guest loads its own `IDTR` with `LIDT [RSP+0]`, the application processor in the
//!   trampoline);
//! * gives the application processors nothing: they stay in `WaitForSipi` until the guest's own ICR
//!   writes start them, run the trampoline in the executor of S6-16c and the long-mode code in the
//!   quantum of S6-16d with the page tables the trampoline loaded into their own `CR3`.
//!
//! [`Stage6SmpProbe::run`] drives the scheduler of S6-15 over the board and returns the COM1
//! transcript and the scheduler counters. Every line of the transcript is written by guest code.

use crate::firmware_board::vcpu_quantum::run_vcpu_set_quantum_shared;
use crate::parallel_run::{
    ParallelConfig, ParallelControl, ParallelFinish, ParallelReport, ParallelStopError,
    QuantumCall, run_threads,
};
use crate::smp_scheduler::{
    BoardSmpHost, Executed, Finished, QuantumPolicy, SafePoint, SmpError, SmpScheduler, StopError,
    StopRequest,
};
use crate::vm_snapshot::VmSnapshot;
use crate::{
    FirmwareBoardConfig, FirmwareBoardError, GuestPhysicalAddress, LoadedFirmwareBoard, PciEcam,
    firmware_board::{FIRMWARE_CR0, FIRMWARE_CR4, assemble_board},
};
use od_isa::execute::{ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1};
use od_isa::state::Gdtr;
use od_isa::{
    CliProfile, CodeSegmentDescriptorCache, CpuActivity, HaltProfile, LongModeState, Register,
    StackSegmentDescriptorCache, StiProfile, StiShadow,
};

pub const STAGE6_SMP_PROBE_IMAGE_GPA: u64 = 0x2000;
pub const STAGE6_SMP_PROBE_IMAGE_BYTES: usize = 0xB000;
/// First byte of the page tables (PML4): the `CR3` of the boot processor, and the value the
/// trampoline loads into the `CR3` of the application processor.
pub const STAGE6_SMP_PROBE_PML4_GPA: u64 = 0x2000;
pub const STAGE6_SMP_PROBE_INFO_GPA: u64 = 0x7000;
pub const STAGE6_SMP_PROBE_INFO_BYTES: usize = 64;
pub const STAGE6_SMP_PROBE_INFO_MAGIC: u32 = 0x3650_4D53;
pub const STAGE6_SMP_PROBE_INFO_VERSION: u32 = 1;
/// The nonce is ASCII `0x21..=0x7E`, 1 to 47 bytes: it sits at info bytes 16..64, one NUL ends it.
pub const STAGE6_SMP_PROBE_NONCE_MAX: usize = 47;
/// The SIPI vector of the trampoline (page `0x8000`).
pub const STAGE6_SMP_PROBE_SIPI_VECTOR: u8 = 0x08;
pub const STAGE6_SMP_PROBE_TRAMPOLINE_GPA: u64 = 0x8000;
/// "AP entered": the operand of the trampoline's `LOCK BTS`.
pub const STAGE6_SMP_PROBE_LOCK_GPA: u64 = 0x8100;
pub const STAGE6_SMP_PROBE_FLAG_GPA: u64 = 0xC008;
pub const STAGE6_SMP_PROBE_COUNT_GPA: u64 = 0xC010;
/// Marker of the boot processor: `magic u32, apic id register u32, 8 reserved, nonce[16]`.
pub const STAGE6_SMP_PROBE_MARKER_BSP_GPA: u64 = 0xC040;
pub const STAGE6_SMP_PROBE_MARKER_AP_GPA: u64 = 0xC080;
/// Counters of the IPI exchange: the AP handler of vector [`STAGE6_SMP_PROBE_VECTOR_AP`], the BSP
/// handler of [`STAGE6_SMP_PROBE_VECTOR_BSP`], and the AP handler of the NMI (vector 2).
pub const STAGE6_SMP_PROBE_AP_RX_GPA: u64 = 0xC018;
pub const STAGE6_SMP_PROBE_BSP_RX_GPA: u64 = 0xC020;
pub const STAGE6_SMP_PROBE_NMI_GPA: u64 = 0xC028;
/// S6-17 scenario `shootdown`: the AP handler of [`STAGE6_SMP_PROBE_VECTOR_SHOOTDOWN`] counts its
/// acknowledgements, the AP main loop counts its posts (one read of X after the handler); the
/// values of X the handler and the main loop read last; the low dword of the PTE of X (the BSP
/// writes it, the walks of the AP set its accessed bit) and the linear address the AP reads.
pub const STAGE6_SMP_PROBE_SD_ACK_GPA: u64 = 0xC030;
pub const STAGE6_SMP_PROBE_SD_POST_GPA: u64 = 0xC038;
pub const STAGE6_SMP_PROBE_SD_SEEN_HANDLER_GPA: u64 = 0xC0C0;
pub const STAGE6_SMP_PROBE_SD_SEEN_MAIN_GPA: u64 = 0xC0C8;
pub const STAGE6_SMP_PROBE_SD_PTE_GPA: u64 = 0x6008;
pub const STAGE6_SMP_PROBE_SD_X_LINEAR: u64 = 0x20_1100;
pub const STAGE6_SMP_PROBE_VECTOR_SHOOTDOWN: u8 = 0x32;
pub const STAGE6_SMP_PROBE_IDT_GPA: u64 = 0x9800;
pub const STAGE6_SMP_PROBE_IDT_LIMIT: u16 = 0x3FF;
/// The 10-byte descriptor (limit u16, base u64) the boot processor loads with `LIDT [RSP+0]`.
pub const STAGE6_SMP_PROBE_IDTR_BSP_GPA: u64 = 0x8150;
pub const STAGE6_SMP_PROBE_VECTOR_AP: u8 = 0x30;
pub const STAGE6_SMP_PROBE_VECTOR_BSP: u8 = 0x31;
/// The largest `Fixed` quantum the image's wait loops are sized for (see the builder).
pub const STAGE6_SMP_PROBE_MAX_FIXED_QUANTUM: u64 = 4_000_000;
pub const STAGE6_SMP_PROBE_MARKER_MAGIC: u32 = 0x4B52_414D;
pub const STAGE6_SMP_PROBE_MARKER_BYTES: usize = 64;
const BSP_ENTRY: u64 = 0xA000;
const BSP_STACK_TOP: u64 = 0xE000;
const GDT_GPA: u64 = 0x9000;
const GDT_LIMIT: u16 = 0x1F;
/// `IA32_EFER` of a long-mode processor of the probe: SCE | LME | LMA | NXE (what the trampoline
/// leaves in the application processor).
const PROBE_EFER: u64 = 0xD01;
const PAGE_TABLE_BYTES: u64 = 0x5000;
/// The work limit of a parallel run that is stopped by its controller (the probe ends far below it).
const BUDGET_OF_A_STOPPED_RUN: u64 = 20_000_000;

#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeLoadError {
    ImageSize {
        expected: usize,
        actual: usize,
    },
    /// `vcpu_count` is outside `1..=4`.
    VcpuCount(usize),
    Nonce(&'static str),
    Board(FirmwareBoardError),
    Memory(String),
}

/// How the scheduler cuts the turns of the run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeQuantum {
    /// Every turn is this many steps (at least 1).
    Fixed(u64),
    /// Every turn is drawn from SplitMix64(seed) between 1 and 256 steps.
    Seeded(u64),
    /// The vCPU of every step, in order (a turn is one step of the named vCPU, which must be
    /// running at that point); the run ends with [`Stage6SmpProbeFinish::ScheduleEnd`] after the
    /// last entry.
    Schedule(Vec<usize>),
}

impl Stage6SmpProbeQuantum {
    /// The policy as one line: `fixed:Q`, `seeded:S:1..=256`, `schedule:<steps>`.
    pub fn describe(&self) -> String {
        match self {
            Self::Fixed(steps) => format!("fixed:{steps}"),
            Self::Seeded(seed) => format!("seeded:{seed}:1..=256"),
            Self::Schedule(list) => format!("schedule:{}", list.len()),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeFinish {
    /// Every processor halted or waits for SIPI and nothing can wake one.
    AllIdle,
    BudgetExhausted,
    Stopped,
    /// An explicit schedule ran out.
    ScheduleEnd,
}

/// What the scheduler counted for one vCPU, and where the vCPU stands.
#[derive(Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeCpu {
    pub retired: u64,
    pub turns: u64,
    pub steps: u64,
    pub wakes: u64,
    pub init_applied: u64,
    pub started: u64,
    pub nmi_delivered: u64,
    pub activity: CpuActivity,
    /// The vCPU still runs the trampoline's legacy model (it did not reach long mode).
    pub legacy: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeReport {
    pub finished: Stage6SmpProbeFinish,
    /// Every byte the guest wrote to COM1.
    pub com1: Vec<u8>,
    pub cpus: Vec<Stage6SmpProbeCpu>,
    /// Turns of all vCPUs together.
    pub turns: u64,
    /// Virtual nanoseconds the scheduler skipped while nothing ran, and the number of jumps.
    pub idle_ns: u64,
    pub idle_jumps: u64,
    /// The executed schedule as runs `(vCPU, steps)`; only for [`Stage6SmpProbe::run_recorded`].
    pub schedule: Option<Vec<(usize, u64)>>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeRunError {
    /// A host call ended in a board error; among them `ApLegacyRefused` (the trampoline ran a form
    /// outside S6-14a) and `VcpuQuantum` (the long-mode code left the CPL0 set). The COM1 bytes the
    /// guest wrote up to the error are in `com1`.
    Board {
        error: FirmwareBoardError,
        com1: Vec<u8>,
        /// The counters of the scheduler and the state of every vCPU at the error.
        cpus: Vec<Stage6SmpProbeCpu>,
    },
    /// The scheduler refused the run for a reason of its own (policy, vCPU count, no progress).
    Scheduler(String),
}

pub struct Stage6SmpProbe {
    board: LoadedFirmwareBoard,
}

/// How a parallel run of the probe ended.
#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeParallelFinish {
    /// Every processor halted or waits for SIPI and nothing can wake one.
    AllIdle,
    BudgetExhausted,
    GuestStop(crate::MachineStopReason),
    /// The controller ended the run (the wait for the end timed out: the run was still going).
    Timeout,
    /// A host call of a vCPU ended in a board error (the text of the error).
    Error {
        cpu: usize,
        error: String,
    },
    Panicked {
        cpu: usize,
    },
}

/// What a vCPU thread of a parallel run did, and where the vCPU stands.
#[derive(Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeParallelCpu {
    pub retired: u64,
    pub calls: u64,
    pub wakes: u64,
    pub init_applied: u64,
    pub started: u64,
    pub nmi_delivered: u64,
    pub sleeps: u64,
    pub split_locks: u64,
    pub split_losses: u64,
    pub activity: CpuActivity,
    pub legacy: bool,
}

#[derive(Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeParallelReport {
    pub finished: Stage6SmpProbeParallelFinish,
    /// Every byte the guest wrote to COM1.
    pub com1: Vec<u8>,
    pub cpus: Vec<Stage6SmpProbeParallelCpu>,
    pub idle_ns: u64,
    pub idle_jumps: u64,
    #[cfg(feature = "lock-wait-metrics")]
    pub lock_wait_metrics: crate::BoardLockWaitMetrics,
}

fn long_boot_state() -> LongModeState {
    let mut state = LongModeState {
        rip: BSP_ENTRY,
        rflags: 2,
        cr0: Some(FIRMWARE_CR0),
        cr4: Some(FIRMWARE_CR4),
        cr3: Some(STAGE6_SMP_PROBE_PML4_GPA),
        cs_selector: Some(0x10),
        cs_cache: Some(CodeSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x9B,
            flags: 2,
        }),
        ss_selector: Some(0x18),
        ss_cache: Some(StackSegmentDescriptorCache {
            base: 0,
            limit: u32::MAX,
            access: 0x93,
            flags: 0,
        }),
        gdtr: Some(Gdtr {
            base: GDT_GPA,
            limit: GDT_LIMIT,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = BSP_STACK_TOP;
    state
}

/// Validate the nonce and build the info block.
fn info_block(
    vcpu_count: usize,
    nonce: &[u8],
) -> Result<[u8; STAGE6_SMP_PROBE_INFO_BYTES], Stage6SmpProbeLoadError> {
    if nonce.is_empty() || nonce.len() > STAGE6_SMP_PROBE_NONCE_MAX {
        return Err(Stage6SmpProbeLoadError::Nonce("nonce is 1..=47 bytes"));
    }
    if nonce.iter().any(|byte| !(0x21..=0x7E).contains(byte)) {
        return Err(Stage6SmpProbeLoadError::Nonce("nonce is ASCII 0x21..=0x7E"));
    }
    let mut block = [0u8; STAGE6_SMP_PROBE_INFO_BYTES];
    block[0..4].copy_from_slice(&STAGE6_SMP_PROBE_INFO_MAGIC.to_le_bytes());
    block[4..8].copy_from_slice(&STAGE6_SMP_PROBE_INFO_VERSION.to_le_bytes());
    block[8..12].copy_from_slice(&(vcpu_count as u32).to_le_bytes());
    block[12..16].copy_from_slice(&(nonce.len() as u32).to_le_bytes());
    block[16..16 + nonce.len()].copy_from_slice(nonce);
    Ok(block)
}

/// Place a `Stage6SmpProbeV2` image (the builder's `probe.bin`) on a board with `vcpu_count` vCPUs and write the info block
/// for `nonce`. Only the boot processor is made runnable; the others wait for SIPI.
pub fn load_stage6_smp_probe(
    image: &[u8],
    vcpu_count: usize,
    nonce: &[u8],
) -> Result<Stage6SmpProbe, Stage6SmpProbeLoadError> {
    if image.len() != STAGE6_SMP_PROBE_IMAGE_BYTES {
        return Err(Stage6SmpProbeLoadError::ImageSize {
            expected: STAGE6_SMP_PROBE_IMAGE_BYTES,
            actual: image.len(),
        });
    }
    if !(1..=4).contains(&vcpu_count) {
        return Err(Stage6SmpProbeLoadError::VcpuCount(vcpu_count));
    }
    let info = info_block(vcpu_count, nonce)?;
    let config = FirmwareBoardConfig {
        vcpu_count,
        ..FirmwareBoardConfig::default()
    };
    let mut board = assemble_board(&config, PciEcam::new(), None, None)
        .and_then(|assembled| {
            assembled.into_linux_board(STAGE6_SMP_PROBE_PML4_GPA, PAGE_TABLE_BYTES)
        })
        .map_err(Stage6SmpProbeLoadError::Board)?;
    board.bus_mut().enable_apic_mmio_windows();
    board
        .enable_clock_owner()
        .map_err(Stage6SmpProbeLoadError::Board)?;
    for (gpa, bytes) in [
        (STAGE6_SMP_PROBE_IMAGE_GPA, image),
        (STAGE6_SMP_PROBE_INFO_GPA, &info[..]),
    ] {
        board
            .bus_mut()
            .patch_physical_ram(GuestPhysicalAddress::new(gpa), bytes)
            .map_err(|error| Stage6SmpProbeLoadError::Memory(format!("{error:?}")))?;
    }
    let bsp = board.vcpu_mut(0).expect("a board has vCPU 0");
    bsp.state = long_boot_state();
    bsp.guest_efer = PROBE_EFER;
    bsp.legacy = None;
    Ok(Stage6SmpProbe { board })
}

impl Stage6SmpProbe {
    /// Read `len` bytes of guest RAM (for the cells of the protocol).
    pub fn read_ram(&mut self, gpa: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0; len];
        let (memory, _) = self.board.bus_mut().split_for_pio_step();
        memory
            .load_bytes(GuestPhysicalAddress::new(gpa), &mut bytes)
            .expect("the probe reads RAM of the board");
        bytes
    }

    /// Write guest RAM before a run (a negative test sets a cell the guest checks).
    pub fn write_ram(&mut self, gpa: u64, bytes: &[u8]) -> Result<(), Stage6SmpProbeLoadError> {
        self.board
            .bus_mut()
            .patch_physical_ram(GuestPhysicalAddress::new(gpa), bytes)
            .map_err(|error| Stage6SmpProbeLoadError::Memory(format!("{error:?}")))
    }

    /// The `(IRR, ISR)` bits of `vector` in the Local APIC of vCPU `cpu`.
    pub fn vector_state(&self, cpu: usize, vector: u8) -> Option<(bool, bool)> {
        self.board
            .local_apic_of(cpu)
            .map(|lapic| lapic.vector_state(vector))
    }

    /// The `(IRR, ISR)` bits of every vector of vCPU `cpu` that is set, as `(vector, irr, isr)`.
    pub fn pending_vectors(&self, cpu: usize) -> Vec<(u8, bool, bool)> {
        (0..=255u8)
            .filter_map(|vector| {
                let (irr, isr) = self.vector_state(cpu, vector)?;
                (irr || isr).then_some((vector, irr, isr))
            })
            .collect()
    }

    /// Run the scheduler of S6-15 over the board until nothing can run, `budget` instructions
    /// (all vCPUs together) were retired, or an error.
    pub fn run(
        &mut self,
        quantum: Stage6SmpProbeQuantum,
        budget: u64,
    ) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        self.run_inner(quantum, budget, false)
    }

    /// [`Self::run`] that also records the executed schedule (one entry per turn: memory grows
    /// with the number of turns).
    pub fn run_recorded(
        &mut self,
        quantum: Stage6SmpProbeQuantum,
        budget: u64,
    ) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        self.run_inner(quantum, budget, true)
    }

    fn cpu_reports(&self, scheduler: &SmpScheduler) -> Vec<Stage6SmpProbeCpu> {
        scheduler
            .counters()
            .iter()
            .enumerate()
            .map(|(cpu, counters)| {
                let vcpu = self.board.vcpu(cpu).expect("a vCPU of the board");
                Stage6SmpProbeCpu {
                    retired: counters.retired,
                    turns: counters.turns,
                    steps: counters.steps,
                    wakes: counters.wakes,
                    init_applied: counters.init_applied,
                    started: counters.started,
                    nmi_delivered: counters.nmi_delivered,
                    activity: vcpu.activity(),
                    legacy: vcpu.legacy.is_some(),
                }
            })
            .collect()
    }

    /// One quantum of one vCPU of the probe: the CPL0 set with `CLI`, `STI`, `HLT`, byte port I/O
    /// and the vCPU's own `CR3` (what the scheduler's host runs).
    fn probe_quantum(
        board: &mut LoadedFirmwareBoard,
        cpu: usize,
        budget: u64,
        stop: &mut dyn FnMut() -> bool,
    ) -> Result<Executed, FirmwareBoardError> {
        let (cli, sti, halt) = (
            CliProfile::NonPviLong64V1,
            StiProfile::NonPviLong64V1,
            HaltProfile::Long64V1,
        );
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(&cli)
            .with_cpu_owned_cr3(MovCr3Long64V1)
            .with_pio_u8()
            .with_halt(&halt)
            .with_sti(&sti);
        board.run_vcpu_set_quantum(cpu, budget, stop, &set)
    }

    /// One quantum of one vCPU of the probe on a vCPU thread of the parallel mode: the same CPL0
    /// set as [`Self::probe_quantum`], on the shared machine (S6-34).
    fn probe_quantum_shared(call: QuantumCall<'_, '_>) -> Result<Executed, FirmwareBoardError> {
        let (cli, sti, halt) = (
            CliProfile::NonPviLong64V1,
            StiProfile::NonPviLong64V1,
            HaltProfile::Long64V1,
        );
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(&cli)
            .with_cpu_owned_cr3(MovCr3Long64V1)
            .with_pio_u8()
            .with_halt(&halt)
            .with_sti(&sti);
        run_vcpu_set_quantum_shared(
            call.env,
            call.vcpu,
            call.cpu,
            call.lapic,
            call.source,
            Some(call.gate),
            call.budget,
            call.stop,
            &set,
            call.consecutive_faults,
        )
    }

    /// Run the probe in the parallel mode (S6-34): one host thread per vCPU over the shared
    /// machine, `quantum` steps per call, `budget` retired instructions of all vCPUs together,
    /// until nothing can run, the budget is used up, a vCPU fails, or `timeout` passes (then every
    /// thread is stopped at its next boundary and the run is reported as `Timeout`). Every thread
    /// has been joined when this returns. The interleaved scheduler (`run`) stays the default and
    /// is not changed by this.
    pub fn run_parallel(
        &mut self,
        quantum: u64,
        budget: u64,
        timeout: std::time::Duration,
    ) -> Result<Stage6SmpProbeParallelReport, Stage6SmpProbeRunError> {
        #[cfg(feature = "lock-wait-metrics")]
        self.board.reset_lock_wait_metrics();
        let config = ParallelConfig {
            quantum,
            budget,
            ..ParallelConfig::default()
        };
        let (report, ended) = self
            .parallel(config, |control| control.wait_finished(timeout))
            .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        Ok(self.parallel_report(report, ended))
    }

    /// Run the probe in the parallel mode until the vCPUs together have retired
    /// `stop_after_retired` instructions, stop every vCPU on a safe point (S6-34), take the state
    /// of the processors there ([`VmSnapshot::take_parallel`]) and end the run (S6-35). The machine
    /// then stands exactly where it was stopped: no vCPU moved after its acknowledgement, RAM and
    /// the devices are what the guest left. A session ([`Self::session`]) can then reset the CPU
    /// complex and put the snapshot back, and the guest goes on in the interleaved mode with
    /// `continue_with`: the snapshot carries the scheduler of that run at its start. The result of
    /// the guest is checked, not the interleaving: the parallel mode is not deterministic. Not a
    /// snapshot of the machine and not a file (`S9-24`/`S9-25`).
    pub fn run_parallel_to_a_stop(
        &mut self,
        quantum: u64,
        stop_after_retired: u64,
        timeout: std::time::Duration,
        continue_with: Stage6SmpProbeQuantum,
    ) -> Result<Stage6SmpProbeParallelStop, Stage6SmpProbeRunError> {
        const STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);
        let scheduler = SmpScheduler::new(
            self.board.vcpu_count(),
            Self::policy_of(continue_with),
            StopRequest::new(),
        )
        .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        let lapic = self.board.local_apic();
        let config = ParallelConfig {
            quantum,
            budget: BUDGET_OF_A_STOPPED_RUN,
            ..ParallelConfig::default()
        };
        let (report, outcome) = self
            .parallel(config, |control| {
                let reached = control.wait_until(timeout, || {
                    control.is_finished()
                        || control.retired().iter().sum::<u64>() >= stop_after_retired
                });
                if !reached {
                    return Err(format!(
                        "the probe did not retire {stop_after_retired} instructions in {timeout:?}"
                    ));
                }
                match control.stop_all(STOP_TIMEOUT) {
                    Ok(proof) => VmSnapshot::take_parallel(control, &lapic, &proof, &scheduler)
                        .map(Some)
                        .map_err(|error| format!("{error:?}")),
                    Err(ParallelStopError::Finished) => Ok(None),
                    Err(error) => Err(format!("{error:?}")),
                }
            })
            .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        let threads_joined = report.threads_started == report.threads_exited;
        match report.finished {
            ParallelFinish::Error { cpu, error } => {
                return Err(Stage6SmpProbeRunError::Scheduler(format!(
                    "vCPU {cpu} failed: {error:?}"
                )));
            }
            ParallelFinish::Panicked { cpu } => {
                return Err(Stage6SmpProbeRunError::Scheduler(format!(
                    "vCPU {cpu} panicked"
                )));
            }
            _ => {}
        }
        let snapshot = outcome.map_err(Stage6SmpProbeRunError::Scheduler)?;
        Ok(Stage6SmpProbeParallelStop {
            snapshot: snapshot.map(|inner| Stage6SmpProbeSnapshot { inner }),
            retired: report.cpus.iter().map(|cpu| cpu.retired).collect(),
            com1: self.board.com1_transcript().to_vec(),
            threads_joined,
        })
    }

    /// The parallel run with a controller of the caller's (the tests of S6-34 stop, look and
    /// resume from it).
    pub(crate) fn parallel<R>(
        &mut self,
        config: ParallelConfig,
        controller: impl FnOnce(&ParallelControl<'_, '_>) -> R,
    ) -> Result<(ParallelReport, R), FirmwareBoardError> {
        run_threads(
            &mut self.board,
            config,
            &Self::probe_quantum_shared,
            controller,
        )
    }

    pub(crate) fn parallel_report(
        &self,
        report: ParallelReport,
        ended: bool,
    ) -> Stage6SmpProbeParallelReport {
        let finished = match report.finished {
            ParallelFinish::AllIdle => Stage6SmpProbeParallelFinish::AllIdle,
            ParallelFinish::BudgetExhausted => Stage6SmpProbeParallelFinish::BudgetExhausted,
            ParallelFinish::GuestStop(reason) => Stage6SmpProbeParallelFinish::GuestStop(reason),
            ParallelFinish::Stopped if !ended => Stage6SmpProbeParallelFinish::Timeout,
            ParallelFinish::Stopped => Stage6SmpProbeParallelFinish::AllIdle,
            ParallelFinish::Error { cpu, error } => Stage6SmpProbeParallelFinish::Error {
                cpu,
                error: format!("{error:?}"),
            },
            ParallelFinish::Panicked { cpu } => Stage6SmpProbeParallelFinish::Panicked { cpu },
        };
        let cpus = report
            .cpus
            .iter()
            .enumerate()
            .map(|(cpu, counters)| {
                let vcpu = self.board.vcpu(cpu).expect("a vCPU of the board");
                Stage6SmpProbeParallelCpu {
                    retired: counters.retired,
                    calls: counters.calls,
                    wakes: counters.wakes,
                    init_applied: counters.init_applied,
                    started: counters.started,
                    nmi_delivered: counters.nmi_delivered,
                    sleeps: counters.sleeps,
                    split_locks: counters.split_locks,
                    split_losses: counters.split_losses,
                    activity: vcpu.activity(),
                    legacy: vcpu.legacy.is_some(),
                }
            })
            .collect();
        Stage6SmpProbeParallelReport {
            finished,
            com1: self.board.com1_transcript().to_vec(),
            cpus,
            idle_ns: report.idle_ns,
            idle_jumps: report.idle_jumps,
            #[cfg(feature = "lock-wait-metrics")]
            lock_wait_metrics: self.board.lock_wait_metrics(),
        }
    }

    fn policy_of(quantum: Stage6SmpProbeQuantum) -> QuantumPolicy {
        match quantum {
            Stage6SmpProbeQuantum::Fixed(steps) => QuantumPolicy::Fixed(steps),
            Stage6SmpProbeQuantum::Seeded(seed) => QuantumPolicy::seeded(seed),
            Stage6SmpProbeQuantum::Schedule(list) => QuantumPolicy::Schedule(list),
        }
    }

    fn run_inner(
        &mut self,
        quantum: Stage6SmpProbeQuantum,
        budget: u64,
        record: bool,
    ) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        let vcpus = self.board.vcpu_count();
        let mut scheduler = SmpScheduler::new(vcpus, Self::policy_of(quantum), StopRequest::new())
            .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        if record {
            scheduler.record_turns();
        }
        let outcome = {
            let mut host = BoardSmpHost::new(&mut self.board, Self::probe_quantum)
                .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
            scheduler.run(&mut host, budget)
        };
        self.finish(&scheduler, outcome, record)
    }

    /// The report of a run that ended with `outcome`.
    fn finish(
        &mut self,
        scheduler: &SmpScheduler,
        outcome: Result<Finished, SmpError<FirmwareBoardError>>,
        record: bool,
    ) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        let com1 = self.board.com1_transcript().to_vec();
        let finished = match outcome {
            Ok(Finished::AllIdle) => Stage6SmpProbeFinish::AllIdle,
            Ok(Finished::BudgetExhausted) => Stage6SmpProbeFinish::BudgetExhausted,
            Ok(Finished::Stopped) => Stage6SmpProbeFinish::Stopped,
            Ok(Finished::ScheduleEnd) => Stage6SmpProbeFinish::ScheduleEnd,
            Err(SmpError::Host(error)) => {
                let cpus = self.cpu_reports(scheduler);
                return Err(Stage6SmpProbeRunError::Board { error, com1, cpus });
            }
            Err(other) => return Err(Stage6SmpProbeRunError::Scheduler(format!("{other:?}"))),
        };
        let cpus = self.cpu_reports(scheduler);
        let (idle_ns, idle_jumps) = scheduler.idle();
        let schedule = record.then(|| {
            let mut runs: Vec<(usize, u64)> = Vec::new();
            for turn in scheduler.turns() {
                match runs.last_mut() {
                    Some((cpu, steps)) if *cpu == turn.cpu => *steps += turn.steps,
                    _ => runs.push((turn.cpu, turn.steps)),
                }
            }
            runs
        });
        Ok(Stage6SmpProbeReport {
            finished,
            com1,
            turns: cpus.iter().map(|cpu| cpu.turns).sum(),
            cpus,
            idle_ns,
            idle_jumps,
            schedule,
        })
    }
}

/// Why [`Stage6SmpProbeSession::pause`] gave no stop.
#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeStopError {
    /// These vCPUs did not reach a safe point before the round limit; the stop request was
    /// withdrawn and nothing changed.
    Timeout { unacked: Vec<usize>, rounds: u64 },
    /// Anything else (a host error, a scheduler that does not match the machine).
    Other(String),
}

/// The state of the vCPUs, the Local APICs with their event queues and the scheduler of a stopped
/// probe machine, as a value in memory (S6-24). Not a snapshot of the machine: no RAM, no devices,
/// no file format (`S9-24`/`S9-25`). Valid for the probe it was taken from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeSnapshot {
    inner: VmSnapshot,
}

impl Stage6SmpProbeSnapshot {
    /// The virtual time (ns) at which the machine stood.
    pub fn now_ns(&self) -> u64 {
        self.inner.now_ns()
    }

    /// Whether the other snapshot holds the same processors: every vCPU, every Local APIC (IRR,
    /// ISR, pending INIT/SIPI/NMI, the timers with their event queues) and the virtual time. The
    /// scheduler is not compared: a snapshot taken on a machine stopped by the parallel mode
    /// carries the scheduler of the interleaved run that follows, which has not run yet.
    pub fn same_processor_state(&self, other: &Self) -> bool {
        self.inner.vcpus() == other.inner.vcpus()
            && self.inner.apic() == other.inner.apic()
            && self.inner.now_ns() == other.inner.now_ns()
    }

    /// The `(vector, irr, isr)` bits that are set in Local APIC `cpu` at the snapshot (see
    /// [`Stage6SmpProbe::pending_vectors`]).
    pub fn pending_vectors(&self, cpu: usize) -> Vec<(u8, bool, bool)> {
        let Some(apic) = self.inner.apic().cpus.get(cpu) else {
            return Vec::new();
        };
        let bit = |words: &[u32; 8], vector: u8| {
            words[usize::from(vector / 32)] >> (vector % 32) & 1 != 0
        };
        (0..=255u8)
            .filter_map(|vector| {
                let (irr, isr) = (bit(&apic.local.irr, vector), bit(&apic.local.isr, vector));
                (irr || isr).then_some((vector, irr, isr))
            })
            .collect()
    }
}

/// What a parallel run of the probe found when it was stopped (S6-35).
#[derive(Debug)]
pub struct Stage6SmpProbeParallelStop {
    /// The state of the processors at the stop; `None` when the probe had ended by itself before
    /// the stop was asked for (there was nothing to stop).
    pub snapshot: Option<Stage6SmpProbeSnapshot>,
    /// Retired instructions of each vCPU when the run ended (at the stop, if there was one).
    pub retired: Vec<u64>,
    /// The bytes the guest had written to COM1 by then.
    pub com1: Vec<u8>,
    /// Every thread of the run has been joined (the run does not outlive this call).
    pub threads_joined: bool,
}

/// A run of the probe in several steps over one scheduler (S6-24): run, stop every vCPU, take the
/// state, reset, restore, resume, run on. The scheduler keeps its position between the steps, so a
/// run that was stopped and resumed (with or without a restore) is the run that was never
/// stopped.
pub struct Stage6SmpProbeSession<'a> {
    probe: &'a mut Stage6SmpProbe,
    scheduler: SmpScheduler,
    record: bool,
    point: Option<SafePoint>,
}

impl Stage6SmpProbe {
    /// Start a session with the given quantum policy. `record` keeps the turn record of the
    /// scheduler (the report's `schedule`).
    pub fn session(
        &mut self,
        quantum: Stage6SmpProbeQuantum,
        record: bool,
    ) -> Result<Stage6SmpProbeSession<'_>, Stage6SmpProbeRunError> {
        let mut scheduler = SmpScheduler::new(
            self.board.vcpu_count(),
            Self::policy_of(quantum),
            StopRequest::new(),
        )
        .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        if record {
            scheduler.record_turns();
        }
        Ok(Stage6SmpProbeSession {
            probe: self,
            scheduler,
            record,
            point: None,
        })
    }

    /// The virtual time of the board in nanoseconds.
    pub fn now_ns(&self) -> u64 {
        self.board.clock().now_ns()
    }
}

impl Stage6SmpProbeSession<'_> {
    /// Run until the end of `budget` (retired instructions, from now), nothing can run, or STOP;
    /// the report counts everything since the session began.
    pub fn run(&mut self, budget: u64) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        let outcome = {
            let mut host = BoardSmpHost::new(&mut self.probe.board, Stage6SmpProbe::probe_quantum)
                .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
            self.scheduler.run(&mut host, budget)
        };
        self.probe.finish(&self.scheduler, outcome, self.record)
    }

    /// Stop every vCPU on a safe point (SMP_CONTRACT.md 9): each one must acknowledge within
    /// `round_limit` rounds. The clock does not move. On success the stop stays in force (a `run`
    /// ends as `Stopped`) until [`Self::resume`]; on a timeout nothing is stopped and nothing is
    /// published.
    pub fn pause(&mut self, round_limit: u64) -> Result<(), Stage6SmpProbeStopError> {
        let mut host = BoardSmpHost::new(&mut self.probe.board, Stage6SmpProbe::probe_quantum)
            .map_err(|error| Stage6SmpProbeStopError::Other(format!("{error:?}")))?;
        match self.scheduler.stop_all(&mut host, round_limit) {
            Ok(point) => {
                self.point = Some(point);
                Ok(())
            }
            Err(StopError::Timeout { unacked, rounds }) => {
                Err(Stage6SmpProbeStopError::Timeout { unacked, rounds })
            }
            Err(other) => Err(Stage6SmpProbeStopError::Other(format!("{other:?}"))),
        }
    }

    /// The explicit resume after [`Self::pause`].
    pub fn resume(&mut self) {
        self.scheduler.resume();
        self.point = None;
    }

    /// Whether a stop is in force.
    pub fn paused(&self) -> bool {
        self.scheduler.paused()
    }

    /// Take the state of the stopped machine. Refused unless [`Self::pause`] succeeded and the
    /// machine has not moved since.
    pub fn snapshot(&self) -> Result<Stage6SmpProbeSnapshot, String> {
        let point = self
            .point
            .as_ref()
            .ok_or_else(|| "the machine is not stopped".to_string())?;
        VmSnapshot::take(&self.probe.board, &self.scheduler, point)
            .map(|inner| Stage6SmpProbeSnapshot { inner })
            .map_err(|error| format!("{error:?}"))
    }

    /// The reset of the CPU complex and of the scheduler to the state of power-on (the vCPUs and
    /// the Local APICs by `LoadedFirmwareBoard::reset_cpu_complex`, with the boot processor on
    /// the probe's entry state, then the scheduler). RAM, devices and the clock are not touched.
    pub fn reset(&mut self) -> Result<(), String> {
        self.probe
            .board
            .reset_cpu_complex(long_boot_state(), PROBE_EFER)
            .map_err(|error| format!("{error:?}"))?;
        self.scheduler.reset();
        Ok(())
    }

    /// Put the state of a snapshot of this machine back (the machine must be stopped).
    pub fn restore(&mut self, snapshot: &Stage6SmpProbeSnapshot) -> Result<(), String> {
        let point = self
            .point
            .as_ref()
            .ok_or_else(|| "the machine is not stopped".to_string())?;
        snapshot
            .inner
            .restore(&mut self.probe.board, &mut self.scheduler, point)
            .map_err(|error| format!("{error:?}"))
    }

    /// The `(vector, irr, isr)` bits that are set in Local APIC `cpu` (see
    /// [`Stage6SmpProbe::pending_vectors`]).
    pub fn pending_vectors(&self, cpu: usize) -> Vec<(u8, bool, bool)> {
        self.probe.pending_vectors(cpu)
    }

    /// The activity of vCPU `cpu`.
    pub fn activity(&self, cpu: usize) -> Option<CpuActivity> {
        self.probe.board.vcpu(cpu).map(|vcpu| vcpu.activity())
    }

    /// Whether Local APIC `cpu` holds a pending INIT, a pending SIPI, a pending NMI or any
    /// request in IRR or ISR.
    pub fn lapic_pending(&self, cpu: usize) -> Option<bool> {
        let lapic = self.probe.board.local_apic_of(cpu)?;
        let startup = lapic.pending_startup();
        Some(
            startup.init
                || startup.sipi.is_some()
                || lapic.pending_nmi()
                || !self.probe.pending_vectors(cpu).is_empty(),
        )
    }
}
