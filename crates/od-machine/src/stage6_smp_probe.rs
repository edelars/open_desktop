//! S6-16e: loader and runner of the `Stage6SmpProbeV1` guest (INIT/SIPI start of an application
//! processor by guest code).
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
//!   `CR3` = the image's PML4 (the board's `page_table_root` too), `EFER` = SCE | LME | LMA | NXE;
//! * gives the application processors nothing: they stay in `WaitForSipi` until the guest's own ICR
//!   writes start them, run the trampoline in the executor of S6-16c and the long-mode code in the
//!   quantum of S6-16d with the page tables the trampoline loaded into their own `CR3`.
//!
//! [`Stage6SmpProbe::run`] drives the scheduler of S6-15 over the board and returns the COM1
//! transcript and the scheduler counters. Every line of the transcript is written by guest code.

use crate::smp_scheduler::{
    BoardSmpHost, Finished, QuantumPolicy, SmpError, SmpScheduler, StopRequest,
};
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeQuantum {
    /// Every turn is this many steps (at least 1).
    Fixed(u64),
    /// Every turn is drawn from SplitMix64(seed) between 1 and 256 steps.
    Seeded(u64),
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeFinish {
    /// Every processor halted or waits for SIPI and nothing can wake one.
    AllIdle,
    BudgetExhausted,
    Stopped,
}

/// What the scheduler counted for one vCPU, and where the vCPU stands.
#[derive(Debug, Eq, PartialEq)]
pub struct Stage6SmpProbeCpu {
    pub retired: u64,
    pub init_applied: u64,
    pub started: u64,
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
}

#[derive(Debug, Eq, PartialEq)]
pub enum Stage6SmpProbeRunError {
    /// A host call ended in a board error; among them `ApLegacyRefused` (the trampoline ran a form
    /// outside S6-14a) and `VcpuQuantum` (the long-mode code left the CPL0 set). The COM1 bytes the
    /// guest wrote up to the error are in `com1`.
    Board {
        error: FirmwareBoardError,
        com1: Vec<u8>,
    },
    /// The scheduler refused the run for a reason of its own (policy, vCPU count, no progress).
    Scheduler(String),
}

pub struct Stage6SmpProbe {
    board: LoadedFirmwareBoard,
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

/// Place a `Stage6SmpProbeV1` image on a board with `vcpu_count` vCPUs and write the info block
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
            .read_bytes(GuestPhysicalAddress::new(gpa), &mut bytes)
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

    /// Run the scheduler of S6-15 over the board until nothing can run, `budget` instructions
    /// (all vCPUs together) were retired, or an error.
    pub fn run(
        &mut self,
        quantum: Stage6SmpProbeQuantum,
        budget: u64,
    ) -> Result<Stage6SmpProbeReport, Stage6SmpProbeRunError> {
        let policy = match quantum {
            Stage6SmpProbeQuantum::Fixed(steps) => QuantumPolicy::Fixed(steps),
            Stage6SmpProbeQuantum::Seeded(seed) => QuantumPolicy::seeded(seed),
        };
        let vcpus = self.board.vcpu_count();
        let mut scheduler = SmpScheduler::new(vcpus, policy, StopRequest::new())
            .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
        let outcome = {
            let mut host = BoardSmpHost::new(
                &mut self.board,
                |board: &mut LoadedFirmwareBoard, cpu, budget, stop: &mut dyn FnMut() -> bool| {
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
                },
            )
            .map_err(|error| Stage6SmpProbeRunError::Scheduler(format!("{error:?}")))?;
            scheduler.run(&mut host, budget)
        };
        let com1 = self.board.com1_transcript().to_vec();
        let finished = match outcome {
            Ok(Finished::AllIdle) => Stage6SmpProbeFinish::AllIdle,
            Ok(Finished::BudgetExhausted) => Stage6SmpProbeFinish::BudgetExhausted,
            Ok(Finished::Stopped) => Stage6SmpProbeFinish::Stopped,
            Ok(Finished::ScheduleEnd) => {
                return Err(Stage6SmpProbeRunError::Scheduler(
                    "the probe has no explicit schedule".to_string(),
                ));
            }
            Err(SmpError::Host(error)) => {
                return Err(Stage6SmpProbeRunError::Board { error, com1 });
            }
            Err(other) => return Err(Stage6SmpProbeRunError::Scheduler(format!("{other:?}"))),
        };
        let cpus = scheduler
            .counters()
            .iter()
            .enumerate()
            .map(|(cpu, counters)| {
                let vcpu = self.board.vcpu(cpu).expect("a vCPU of the board");
                Stage6SmpProbeCpu {
                    retired: counters.retired,
                    init_applied: counters.init_applied,
                    started: counters.started,
                    activity: vcpu.activity(),
                    legacy: vcpu.legacy.is_some(),
                }
            })
            .collect();
        Ok(Stage6SmpProbeReport {
            finished,
            com1,
            cpus,
        })
    }
}
