//! Public entry of the Linux direct-boot loader (S4-37L6).
//!
//! The loader pieces of L1-L5 are `pub(crate)`; this module is the one door the runner uses. It
//! takes the raw bzImage, initrd, command line and ACPI blob, validates every input in a fixed
//! order, loads a board and hands back a plain-data description of what was placed. Loading never
//! runs the guest: the board comes back at the entry state, with no instruction retired.
//! [`run_linux_direct_board`] (S4-38L1) is the separate, explicit step that runs it for a bounded
//! number of instructions and returns a [`LinuxStopRecord`].
//!
//! Order of checks (each failure has a stable [`LinuxDirectError::code`]; none of them has
//! assembled a board, except the handoff comparison, which still writes nothing):
//!
//! ```text
//! 1. bzImage header           bzimage_truncated | bzimage_payload_out_of_file | bzimage_header_invalid
//! 2. board configuration      board_assembly_failed (the config of the board is invalid)
//!    and its manifest hash
//! 3. ACPI blob: the expected  acpi_manifest_mismatch (the operator's hash is not the board's)
//!    hash must be the BOARD'S | acpi_manifest_mismatch | acpi_blob_invalid
//!    (the SAME validate_acpi_blob_for_vcpus call gives the bytes loaded)
//! 4. command line             cmdline_invalid | cmdline_too_long
//! 5. placement / initrd       initrd_too_large | placement_invalid
//! 6. E820 and boot_params     e820_invalid | boot_params_invalid
//! 7. board assembly, E820     e820_handoff_mismatch | board_assembly_failed
//!    against the handoff
//! 8. guest memory writes and  guest_memory_access | load_failed | entry_state_invalid
//!    entry state validation   (load_failed: a sanity limit of the loader itself, such as page
//!                             tables that do not fit; not an input the operator can fix)
//!                             (bzimage_header_invalid also comes from here: a protected-mode
//!                             code size below the 64-bit entry offset 0x200, EntryOutsideKernel,
//!                             is found while the kernel is sliced, after placement)
//! 9. read-back for the report e820_invalid (more than 128 entries or none in the page)
//!                             | guest_memory_access
//! ```
//!
//! The report reads `boot_params`, the E820 table, the command line, the RSDP signature and the
//! kernel and initrd bytes back from guest memory, so it describes what the guest will see, not
//! what the loader meant to write.

use crate::PagedProbeExit;
use crate::acpi_blob::{
    AcpiBlobError, AcpiManifestHash, ValidatedAcpiBlob, sha256, validate_acpi_blob_for_vcpus,
};
use crate::firmware_board::{FirmwareBoardConfig, FirmwareRunReport, LoadedFirmwareBoard};
use crate::irq_source::IrqCounters;
use crate::linux_boot::{LinuxBzImageHeader, LinuxHeaderError, parse_linux_bzimage_header};
use crate::linux_boot_params::{LINUX_E820_MAX_ENTRIES, LinuxBootParamsError};
use crate::linux_checkpoints::{LinuxCheckpointRecord, LinuxCheckpointTable};
use crate::linux_load::{LinuxLoadError, LinuxLoadInput, load_linux_direct};
use crate::linux_placement::LinuxPlacementError;
use crate::vcpu::BOOT_VCPU_INDEX;
use crate::{GuestPhysicalAddress, LinuxExecutionObservations, MachineStopReason, NvmeConfig};
use od_isa::cpuid::CpuidProfile;
use od_isa::execute::TranslationContextChange;
use od_isa::{IretqUserEntry, Register};
use profile::{LinuxCpl0Profile, LinuxVcpuCells};
use std::cell::Cell;

mod interleaved;
mod observed;
mod profile;
mod threaded;
pub use interleaved::{
    LinuxInterleavedRunReport, LinuxInterleavedVcpu,
    run_linux_direct_board_set_user_mode_interleaved,
};
pub use threaded::{
    LinuxParallelRunReport, LinuxThreadedOptions, LinuxThreadedRunReport,
    run_linux_direct_board_set_user_mode_parallel, run_linux_direct_board_set_user_mode_threaded,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinuxVcpuRunMode {
    Real,
    Protected,
    Long64,
}

#[derive(Debug, Eq, PartialEq)]
pub struct LinuxVcpuRunState {
    pub index: usize,
    pub retired: u64,
    pub activity: od_isa::CpuActivity,
    pub mode: LinuxVcpuRunMode,
    pub rip: u64,
    pub rflags: u64,
    pub gpr: [u64; 16],
    pub cr3: Option<u64>,
    pub guest_efer: u64,
}

impl LinuxVcpuRunState {
    fn capture(board: &LoadedFirmwareBoard, index: usize, retired: u64) -> Self {
        use crate::vcpu::LegacyCpu;
        let vcpu = board.vcpu(index).expect("scheduled vCPU");
        let state = &vcpu.state;
        let legacy_gpr = |low: [u32; 8]| {
            let mut gpr = [0; 16];
            gpr[..8].copy_from_slice(&low.map(u64::from));
            gpr
        };
        let (mode, rip, rflags, gpr, cr3, guest_efer) = match &vcpu.legacy {
            Some(LegacyCpu::Real(cpu)) => (
                LinuxVcpuRunMode::Real,
                u64::from(cpu.mode.eip),
                u64::from(cpu.mode.eflags),
                legacy_gpr(cpu.gpr),
                None,
                0,
            ),
            Some(LegacyCpu::Protected(cpu)) => (
                LinuxVcpuRunMode::Protected,
                u64::from(cpu.rip_high) << 32 | u64::from(cpu.eip),
                u64::from(cpu.eflags),
                legacy_gpr(cpu.gpr),
                Some(u64::from(cpu.cr3)),
                cpu.efer,
            ),
            None => (
                LinuxVcpuRunMode::Long64,
                state.rip,
                state.rflags,
                state.gpr,
                state.cr3,
                vcpu.guest_efer,
            ),
        };
        Self {
            index,
            retired,
            activity: vcpu.activity(),
            mode,
            rip,
            rflags,
            gpr,
            cr3,
            guest_efer,
        }
    }
}

/// `boot_params.acpi_rsdp_addr` (u64) offset.
const OFF_ACPI_RSDP_ADDR: usize = 0x070;
/// `boot_params.e820_entries` (u8) offset.
const OFF_E820_ENTRIES: usize = 0x1E8;
/// `setup_header.cmd_line_ptr` (u32) offset.
const OFF_CMD_LINE_PTR: usize = 0x228;
/// `boot_params.e820_table` offset and entry size.
const OFF_E820_TABLE: usize = 0x2D0;
const E820_ENTRY_BYTES: usize = 20;
const BOOT_PARAMS_BYTES: usize = 0x1000;

/// Inputs of [`load_linux_direct_board`]. `ram_bytes` is the full guest RAM size; the board is
/// the Linux profile board (`acpi_pm`, `acpi_tables`, `pci_ecam`, `linux_profile`).
#[derive(Clone, Copy, Debug)]
pub struct LinuxDirectInput<'a> {
    pub bzimage: &'a [u8],
    /// The initrd; empty means no initrd.
    pub initrd: &'a [u8],
    /// The command line without the terminating NUL.
    pub cmdline: &'a [u8],
    /// The raw ACPI blob (version 2).
    pub acpi_blob: &'a [u8],
    /// The manifest hash the operator expects. It must equal the hash of the board this module
    /// assembles (`FirmwareBoardConfig::acpi_manifest_hash` of the Linux profile config); the
    /// blob is validated against the BOARD'S hash, never against an arbitrary one.
    pub expected_acpi_manifest_hash: AcpiManifestHash,
    pub ram_bytes: u64,
}

/// One zone of the placement plan, `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxDirectZone {
    pub name: &'static str,
    pub start: u64,
    pub end: u64,
}

/// One E820 entry as the guest reads it from `boot_params`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxDirectE820 {
    pub addr: u64,
    pub size: u64,
    /// 1 RAM, 2 reserved, 3 ACPI reclaim, 4 NVS.
    pub kind: u32,
}

/// The register state the guest starts with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxDirectEntryState {
    pub rip: u64,
    pub rsi: u64,
    pub rsp: u64,
    pub rflags: u64,
    pub cs: Option<u16>,
    pub ss: Option<u16>,
    pub ds: u16,
    pub es: u16,
    pub cr0: Option<u64>,
    /// The board's page table root (the state itself carries no CR3).
    pub cr3: u64,
    pub cr4: Option<u64>,
    pub efer: u64,
    pub gdtr_base: u64,
    pub gdtr_limit: u16,
}

/// A loaded board and everything the evidence needs. No instruction was retired.
pub struct LinuxDirectBoot {
    pub board: LoadedFirmwareBoard,
    pub header: LinuxBzImageHeader,
    pub acpi: ValidatedAcpiBlob,
    /// Zones in plan order: kernel, initrd (if any), boot_params, cmdline, gdt, page_tables,
    /// stack, acpi_tables, acpi_facs.
    pub zones: Vec<LinuxDirectZone>,
    /// E820 table read back from the `boot_params` page in guest memory.
    pub e820: Vec<LinuxDirectE820>,
    /// `boot_params.acpi_rsdp_addr` read back from guest memory.
    pub acpi_rsdp_addr: u64,
    /// The command line read back from guest memory (up to the NUL).
    pub cmdline_in_memory: Vec<u8>,
    /// The 8 bytes at `acpi_rsdp_addr` read back from guest memory (`"RSD PTR "` when the RSDP
    /// really lies there).
    pub rsdp_signature_in_memory: [u8; 8],
    /// SHA-256 of the protected-mode kernel code (`pm_code_size` bytes at the kernel zone), read
    /// back from guest memory.
    pub kernel_sha256_in_memory: [u8; 32],
    /// SHA-256 of the initrd at its zone, read back from guest memory; `None` without an initrd.
    pub initrd_sha256_in_memory: Option<[u8; 32]>,
    pub entry: LinuxDirectEntryState,
    /// Instructions the guest retired while loading: always 0 (the loader never runs it; a run
    /// reports its own count in [`LinuxStopRecord::retired`]).
    pub retired: u64,
}

/// A refused input. `code` is stable (see the module documentation), `stage` names the step,
/// `detail` is the `Debug` text of the underlying error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxDirectError {
    pub code: &'static str,
    pub stage: &'static str,
    pub detail: String,
    /// Instructions the guest retired before the error, when the failing step had already run
    /// it (`run_failed`); `None` for every error of the loader, which never runs the guest.
    pub retired: Option<u64>,
}

impl LinuxDirectError {
    fn new(code: &'static str, stage: &'static str, detail: impl std::fmt::Debug) -> Self {
        Self {
            code,
            stage,
            detail: format!("{detail:?}"),
            retired: None,
        }
    }

    /// Like [`Self::new`] with a detail that is already text (no `Debug` quoting).
    fn text(code: &'static str, stage: &'static str, detail: String) -> Self {
        Self {
            code,
            stage,
            detail,
            retired: None,
        }
    }
}

impl std::fmt::Display for LinuxDirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.code, self.stage, self.detail)
    }
}

impl std::error::Error for LinuxDirectError {}

fn header_error(error: LinuxHeaderError) -> LinuxDirectError {
    let code = match error {
        LinuxHeaderError::TooShort { .. }
        | LinuxHeaderError::SetupOutOfFile { .. }
        | LinuxHeaderError::PmCodeOutOfFile { .. } => "bzimage_truncated",
        LinuxHeaderError::PayloadEmpty | LinuxHeaderError::PayloadOutOfFile { .. } => {
            "bzimage_payload_out_of_file"
        }
        _ => "bzimage_header_invalid",
    };
    LinuxDirectError::new(code, "bzimage_header", error)
}

fn acpi_error(error: AcpiBlobError) -> LinuxDirectError {
    let code = match error {
        AcpiBlobError::ManifestMismatch { .. } => "acpi_manifest_mismatch",
        _ => "acpi_blob_invalid",
    };
    LinuxDirectError::new(code, "acpi_blob", error)
}

fn load_error(error: LinuxLoadError) -> LinuxDirectError {
    use LinuxLoadError as E;
    let (code, stage) = match &error {
        E::Header(inner) => return header_error(*inner),
        E::CmdlineContainsNul { .. } => ("cmdline_invalid", "cmdline"),
        E::Placement(LinuxPlacementError::CmdlineTooLong { .. }) => ("cmdline_too_long", "cmdline"),
        E::Placement(
            LinuxPlacementError::InitrdTooLarge { .. }
            | LinuxPlacementError::NoRoomForInitrd { .. }
            | LinuxPlacementError::InitrdAboveLimit { .. },
        ) => ("initrd_too_large", "placement"),
        E::Placement(_) | E::InputTooLarge { .. } => ("placement_invalid", "placement"),
        E::KernelOutOfFile => ("bzimage_truncated", "bzimage_header"),
        E::AcpiBlobTooShort { .. } => ("acpi_blob_invalid", "acpi_blob"),
        E::BootParams(LinuxBootParamsError::InitrdAbove4G { .. }) => {
            ("initrd_too_large", "boot_params")
        }
        E::BootParams(
            LinuxBootParamsError::EcamNotReserved { .. }
            | LinuxBootParamsError::WindowNotReserved { .. }
            | LinuxBootParamsError::AcpiNotCovered { .. }
            | LinuxBootParamsError::LowRamMismatch { .. }
            | LinuxBootParamsError::HighRamMismatch { .. }
            | LinuxBootParamsError::ZoneNotRam { .. }
            | LinuxBootParamsError::LoaderZoneInRam { .. }
            | LinuxBootParamsError::BadEntryCount { .. }
            | LinuxBootParamsError::BadEntry { .. }
            | LinuxBootParamsError::UnknownType { .. }
            | LinuxBootParamsError::NotSorted { .. }
            | LinuxBootParamsError::EmptyRange { .. }
            | LinuxBootParamsError::AcpiBaseMismatch { .. }
            | LinuxBootParamsError::AcpiEntryUnaligned { .. },
        ) => ("e820_invalid", "e820"),
        E::BootParams(_) => ("boot_params_invalid", "boot_params"),
        E::Handoff(_) => ("e820_handoff_mismatch", "handoff"),
        E::NotLinuxProfile | E::Board(_) => ("board_assembly_failed", "board"),
        E::AcpiBlobMismatch => ("acpi_blob_invalid", "acpi_blob"),
        E::PageTablesTooLarge { .. }
        | E::LowRamNotAligned { .. }
        | E::PageTableRootInvalid { .. }
        | E::LowRamTooLarge { .. } => ("load_failed", "load"),
        // The protected-mode code is too small to hold the 64-bit entry at offset 0x200.
        E::EntryOutsideKernel { .. } => ("bzimage_header_invalid", "bzimage_header"),
        E::Write { .. } | E::Read { .. } => ("guest_memory_access", "load"),
        // Every variant is listed (no wildcard): a new `LinuxLoadError` variant must pick its
        // code here, not fall silently under `entry_state_invalid`.
        E::EntryRip { .. }
        | E::EntryRsi { .. }
        | E::EntryGpr { .. }
        | E::EntryRsp { .. }
        | E::EntryCs { .. }
        | E::EntryCsCache
        | E::EntryDataSegment { .. }
        | E::EntryInterruptsEnabled { .. }
        | E::EntryRflags { .. }
        | E::EntryCr0 { .. }
        | E::EntryCr4La57 { .. }
        | E::EntryCr4 { .. }
        | E::EntryEfer { .. }
        | E::EntryCr3 { .. }
        | E::EntryActivity
        | E::EntryGdtr { .. }
        | E::GdtMismatch
        | E::PageNotMapped { .. }
        | E::PageBadEntry { .. }
        | E::PageNotIdentity { .. }
        | E::HighWindowMapped { .. }
        | E::PageTablePointerOutsideZone { .. }
        | E::PageTablesMismatch { .. } => ("entry_state_invalid", "entry_state"),
    };
    LinuxDirectError::new(code, stage, error)
}

/// Validates the inputs, loads the board and returns the report. See the module documentation
/// for the order of checks. The guest is never started.
pub fn load_linux_direct_board(
    input: &LinuxDirectInput<'_>,
) -> Result<LinuxDirectBoot, LinuxDirectError> {
    load_linux_direct_board_with_vcpus(input, 1)
}

/// Load a diskless board whose vCPU count matches its ACPI MADT.
pub fn load_linux_direct_board_with_vcpus(
    input: &LinuxDirectInput<'_>,
    vcpu_count: usize,
) -> Result<LinuxDirectBoot, LinuxDirectError> {
    load_linux_direct_board_inner(input, None, vcpu_count)
}

/// As the diskless loader, with a PCI NVMe endpoint and its own ACPI manifest.
/// Attach the namespace backend through the returned board's NVMe handle before running it.
/// Interrupt delivery uses the APIC Linux execution profile.
pub fn load_linux_direct_board_with_nvme(
    input: &LinuxDirectInput<'_>,
    nvme: NvmeConfig,
) -> Result<LinuxDirectBoot, LinuxDirectError> {
    load_linux_direct_board_with_nvme_and_vcpus(input, nvme, 1)
}

/// Load a LinuxDirect PCI/NVMe board whose ACPI manifest and MADT match the supplied topology.
/// Attach the namespace to the resulting NVMe handle before starting the APIC Linux profile.
pub fn load_linux_direct_board_with_nvme_and_vcpus(
    input: &LinuxDirectInput<'_>,
    nvme: NvmeConfig,
    vcpu_count: usize,
) -> Result<LinuxDirectBoot, LinuxDirectError> {
    load_linux_direct_board_inner(input, Some(nvme), vcpu_count)
}

fn load_linux_direct_board_inner(
    input: &LinuxDirectInput<'_>,
    nvme: Option<NvmeConfig>,
    vcpu_count: usize,
) -> Result<LinuxDirectBoot, LinuxDirectError> {
    // 1. Header first: a truncated or foreign kernel is refused before anything else is looked at.
    parse_linux_bzimage_header(input.bzimage).map_err(header_error)?;
    // 2. The board this module assembles, and its own manifest hash.
    let config = FirmwareBoardConfig {
        vcpu_count,
        ram_bytes: input.ram_bytes,
        nvme_pci: nvme.is_some(),
        nvme,
        acpi_pm: true,
        acpi_tables: true,
        pci_ecam: true,
        linux_profile: true,
        ..FirmwareBoardConfig::default()
    };
    let board_hash = config
        .acpi_manifest_hash()
        .map_err(|error| LinuxDirectError::new("board_assembly_failed", "board", error))?;
    // 3. The operator's hash must be the board's: a blob and a hash of another configuration
    //    (other devices) are refused here, before the blob is looked at.
    if input.expected_acpi_manifest_hash != board_hash {
        return Err(LinuxDirectError::text(
            "acpi_manifest_mismatch",
            "acpi_blob",
            format!(
                "the expected manifest hash {} is not the hash of this board ({})",
                hex(&input.expected_acpi_manifest_hash),
                hex(&board_hash)
            ),
        ));
    }
    //    The validated description and the bytes copied to the guest come from this one call;
    //    `load_linux_direct` slices the very same `input.acpi_blob` (and checks its length).
    let acpi = validate_acpi_blob_for_vcpus(input.acpi_blob, &board_hash, vcpu_count)
        .map_err(acpi_error)?;
    let initrd = (!input.initrd.is_empty()).then_some(input.initrd);
    let mut loaded = load_linux_direct(&LinuxLoadInput {
        config: &config,
        bzimage: input.bzimage,
        initrd,
        cmdline: input.cmdline,
        acpi: &acpi,
        acpi_blob: input.acpi_blob,
    })
    .map_err(load_error)?;

    let zones = loaded
        .plan
        .zones()
        .into_iter()
        .map(|(zone, range)| LinuxDirectZone {
            name: zone_name(zone),
            start: range.start,
            end: range.end,
        })
        .collect();
    let boot_params_at = loaded.plan.boot_params.start;
    let mut page = vec![0u8; BOOT_PARAMS_BYTES];
    read(&mut loaded.board, boot_params_at, &mut page)?;
    let acpi_rsdp_addr = le64(&page, OFF_ACPI_RSDP_ADDR);
    let e820 = read_e820(&page)?;
    let cmdline_ptr = u64::from(le32(&page, OFF_CMD_LINE_PTR));
    let cmdline_slot = usize::try_from(loaded.plan.cmdline.len()).unwrap_or(0);
    let mut buffer = vec![0u8; cmdline_slot];
    read(&mut loaded.board, cmdline_ptr, &mut buffer)?;
    let cmdline_in_memory = buffer
        .split(|&byte| byte == 0)
        .next()
        .unwrap_or_default()
        .to_vec();

    let mut rsdp_signature_in_memory = [0u8; 8];
    read(
        &mut loaded.board,
        acpi_rsdp_addr,
        &mut rsdp_signature_in_memory,
    )?;
    // The kernel and the initrd as the guest will find them, not as the loader was given them.
    let kernel_len = usize::try_from(loaded.header.pm_code_size).unwrap_or(0);
    let mut kernel = vec![0u8; kernel_len];
    read(&mut loaded.board, loaded.plan.kernel.start, &mut kernel)?;
    let kernel_sha256_in_memory = sha256(&kernel);
    drop(kernel);
    let initrd_sha256_in_memory = match loaded.plan.initrd {
        Some(range) if !input.initrd.is_empty() => {
            let mut bytes = vec![0u8; input.initrd.len()];
            read(&mut loaded.board, range.start, &mut bytes)?;
            Some(sha256(&bytes))
        }
        _ => None,
    };

    let state = loaded.board.state();
    let entry = LinuxDirectEntryState {
        rip: state.rip,
        rsi: state.gpr[Register::Rsi as usize],
        rsp: state.gpr[Register::Rsp as usize],
        rflags: state.rflags,
        cs: state.cs_selector,
        ss: state.ss_selector,
        ds: state.ds_selector,
        es: state.es_selector,
        cr0: state.cr0,
        cr3: loaded.board.cr3(),
        cr4: state.cr4,
        efer: loaded.board.efer(),
        gdtr_base: state.gdtr.map_or(0, |g| g.base),
        gdtr_limit: state.gdtr.map_or(0, |g| g.limit),
    };
    Ok(LinuxDirectBoot {
        header: loaded.header,
        board: loaded.board,
        acpi,
        zones,
        e820,
        acpi_rsdp_addr,
        cmdline_in_memory,
        rsdp_signature_in_memory,
        kernel_sha256_in_memory,
        initrd_sha256_in_memory,
        entry,
        retired: 0,
    })
}

/// Bytes of the instruction window the stop record shows at RIP.
pub const LINUX_STOP_RIP_BYTES: usize = 16;

/// Hashes of the inputs of a run (S4-38L1). The kernel and initrd hashes are of the files given
/// to the loader; the in-memory hashes are the ones read back after loading.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxStopInputs {
    pub bzimage_sha256: [u8; 32],
    /// `None` without an initrd.
    pub initrd_sha256: Option<[u8; 32]>,
    pub acpi_blob_sha256: [u8; 32],
    pub acpi_manifest_hash: [u8; 32],
    pub kernel_pm_code_sha256_in_memory: [u8; 32],
    pub initrd_sha256_in_memory: Option<[u8; 32]>,
}

/// Where and why a bounded Linux run stopped. One schema for every S4-38 subslice: the exit
/// kind, the instruction window at RIP (GVA and GPA), the control state, the registers, the
/// counters, COM1, the profile and the input hashes.
#[derive(Debug, Eq, PartialEq)]
pub struct LinuxStopRecord {
    /// The exit of the paged interpreter, as it came (no new variants).
    pub exit: PagedProbeExit,
    /// Stable name of `exit`: `budget_exhausted`, `stopped`, `halted`, `shutdown`, `rejected`,
    /// `integration_error`, `unsupported`, `fault`, `backend` or `port_io`; or `cpuid_divergence`
    /// (S4-38L4: the set path stopped before a `CPUID` of leaf 1 or 0x8000_0001; `exit` is
    /// `Stopped` then, see `cpuid_divergence_leaf`); or `user_entry` (S4-38L9I: the set path
    /// stopped at an `IRETQ` returning to CPL3; `exit` is `Stopped` then, see `user_entry`);
    /// or, in a user-mode run that stopped in CPL3 (S4-39c), `user_mode_fault` (`exit` is
    /// `Fault`) or `user_mode_unsupported` (`exit` is `Unsupported`).
    pub exit_kind: &'static str,
    /// The `Debug` text of `exit` (carries the fault, rejection or backend details).
    pub exit_detail: String,
    /// Typed ACPI PM stop request, when the board stopped because the guest requested Reset or
    /// Shutdown. It is independent of `exit_kind`, which remains `stopped` for either PM request.
    pub stop_reason: Option<MachineStopReason>,
    /// The opcode byte of an `Unsupported` exit.
    pub unsupported_opcode: Option<u8>,
    pub rip: u64,
    /// The GPA of RIP through the board CR3 and the 2 MiB walk (`None` when it does not translate).
    pub rip_gpa: Option<u64>,
    /// Up to [`LINUX_STOP_RIP_BYTES`] bytes at RIP, each translated through the same walk as an
    /// instruction fetch (no accessed/dirty bit is written); shorter when a byte does not
    /// translate or has no backing. Any region with backing is read (ROM and the framebuffer
    /// too, not only RAM), and the window may cross a 4 KiB boundary (each byte is translated
    /// on its own); the CPU profile never fetches across one, so the window is wider than a
    /// single fetch.
    pub rip_bytes: Vec<u8>,
    /// Why `rip_bytes` is short, if it is.
    pub rip_bytes_error: Option<String>,
    /// `long64` (the board only runs 64-bit mode).
    pub mode: &'static str,
    /// The RPL (low two bits) of the CS selector, 0 when the state carries none. The interpreter
    /// itself always executes with Ring0 (`run_quantum`), so with a CS whose RPL is not 0 this
    /// value and the privilege the instructions ran with differ; the loader's CS has RPL 0.
    pub cpl: u8,
    pub cr0: Option<u64>,
    /// The board CR3 (the CPU state carries none). On the `cpl0_set` path it is the CR3 of
    /// the last retired `MOV CR3` (the board CR3 when there was none), also when that
    /// instruction ended the run in an integration error (`Cr3EffectMismatch`,
    /// `Cr3SnapshotRejected`); see `cr3_changes`.
    pub cr3: u64,
    pub cr4: Option<u64>,
    pub efer: u64,
    /// The guest-visible IA32_EFER (`efer` plus the `SCE` a guest `WRMSR` may have set,
    /// S4-38L7f); `efer` stays the walker's EFER.
    pub guest_efer: u64,
    /// The guest PCI CONFIG_ADDRESS (port `0xCF8`, S4-38L7w) after the run; `0` when never written.
    pub pci_config_address: u32,
    /// The guest `IA32_STAR` (S4-38L7ao) after the run; `0` when never written. Write-only for the
    /// guest (`RDMSR` of STAR is refused).
    pub guest_star: u64,
    /// The guest `IA32_LSTAR` (S4-38L7ap) after the run; `0` when never written. Write-only for
    /// the guest (`RDMSR` of LSTAR is refused).
    pub guest_lstar: u64,
    /// The guest `IA32_CSTAR` (S4-38L7aq) after the run; `0` when never written. Write-only for
    /// the guest (`RDMSR` of CSTAR is refused).
    pub guest_cstar: u64,
    /// The guest `IA32_SYSENTER_CS` (S4-38L7ar) after the run; `0` when never written. The full
    /// 64-bit value written (the real register is a 16-bit selector, not modelled as such).
    /// Write-only for the guest (`RDMSR` of SYSENTER_CS is refused).
    pub guest_sysenter_cs: u64,
    /// The guest `IA32_SYSENTER_ESP` (S4-38L7as) after the run; `0` when never written. The full
    /// 64-bit value written. Write-only for the guest (`RDMSR` of SYSENTER_ESP is refused).
    pub guest_sysenter_esp: u64,
    /// The guest `IA32_SYSENTER_EIP` (S4-38L7as) after the run; `0` when never written. The full
    /// 64-bit value written. Write-only for the guest (`RDMSR` of SYSENTER_EIP is refused).
    pub guest_sysenter_eip: u64,
    /// The guest `IA32_SYSCALL_MASK` (S4-38L7at) after the run; `0` when never written. The full
    /// 64-bit value written (the flag clearing of the real FMASK is not modelled). Write-only for
    /// the guest (`RDMSR` of SYSCALL_MASK is refused).
    pub guest_syscall_mask: u64,
    /// The guest `IA32_KERNEL_GS_BASE` (S4-38L7au) after the run; `0` when never written. The full
    /// 64-bit value written (no canonical check, `SWAPGS` not modelled). Write-only for the guest
    /// (`RDMSR` of KERNEL_GS_BASE is refused).
    pub guest_kernel_gs_base: u64,
    pub rflags: u64,
    /// RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15 (the order of `od_isa::Register`).
    pub gpr: [u64; 16],
    pub retired: u64,
    pub quanta: u64,
    pub virtual_ns: u64,
    /// Successful reads of the ACPI PM timer during this run; `None` when the board has no PM block.
    pub pm_timer_reads: Option<u64>,
    /// Successful guest RDTSC reads during this run; `None` without the Linux-profile source.
    pub rdtsc_reads: Option<u64>,
    /// Successful guest RDTSC reads by CPL ring 0..3; `None` without the Linux-profile source.
    pub rdtsc_reads_by_cpl: Option<[u64; 4]>,
    /// Present only when requested by the one-shot execution-observations diagnostic.
    pub execution_observations: Option<LinuxExecutionObservations>,
    /// Successful guest reads of PIT port B (`0x61`) during this run; `None` without a PIT.
    pub pit_port_b_reads: Option<u64>,
    /// Successful guest reads of PIT channel 2 (`0x42`) during this run; `None` without a PIT.
    pub pit_counter_port_reads: Option<u64>,
    pub tsc_hz: u64,
    /// The bytes the guest transmitted through COM1.
    pub com1: Vec<u8>,
    /// The board has the Linux profile (PIT, PM_TMR and RDTSC ticks are applied as in `run_boot`);
    /// read from the board (its RDTSC time source exists only under the profile).
    pub linux_profile: bool,
    /// The last architectural page fault of the walker, as `Debug` text.
    pub page_fault: Option<String>,
    /// The last non-architectural RAM/backend classification, as `Debug` text.
    pub memory_error: Option<String>,
    pub inputs: LinuxStopInputs,
    /// How the run executed: `paged_probe` ([`run_linux_direct_board`], the base profile) or
    /// `cpl0_set` ([`run_linux_direct_board_set`], S4-38L3).
    pub path: &'static str,
    /// The members of the CPL0 profile set, in builder order (empty on the `paged_probe` path).
    pub set_members: Vec<String>,
    /// The `Debug` text of the set's own final exit, before it was expressed as `exit` (`None`
    /// on the `paged_probe` path).
    pub set_exit_detail: Option<String>,
    /// How many `MOV CR3` retired (0 on the `paged_probe` path, where `MOV CR3` is
    /// `Unsupported`). A `MOV CR3` that ended the run in an integration error
    /// (`Cr3EffectMismatch`, `Cr3SnapshotRejected`) retired too and is counted, so `cr3` is
    /// always the CR3 of the last one.
    pub cr3_changes: u64,
    /// The last of them as `(old_cr3, new_cr3)`.
    pub last_cr3_change: Option<(u64, u64)>,
    /// S4-38L4: the EAX leaf of the `CPUID` the run stopped before (`exit_kind` is
    /// `cpuid_divergence`): 1 or 0x8000_0001. The instruction did not execute: RIP is on it,
    /// `retired` counts what came before, no register changed, nothing was answered. `None` for
    /// every other exit.
    pub cpuid_divergence_leaf: Option<u32>,
    /// S4-38L9I: the validated frame of the `IRETQ` the run stopped at (`exit_kind` is
    /// `user_entry`): a return to CPL3 the set does not execute. The instruction did not retire:
    /// RIP is on it, `retired` counts what came before and no register changed. `None` for every
    /// other exit.
    pub user_entry: Option<IretqUserEntry>,
    /// S4-39c: the run was made with the user mode of the board ([`run_linux_direct_board_set_user_mode`]):
    /// a committed `IRETQ` to CPL3 continues with the CPL3 entry instead of stopping. `false` on
    /// every other run. With it a stop in CPL3 (`cpl` 3) is `user_mode_fault` or
    /// `user_mode_unsupported` instead of `fault` / `unsupported`.
    pub user_mode: bool,
    /// S4-39c: instructions retired in CPL3 (through the CPL3 entry); part of `retired`. `0`
    /// without `user_mode` and while CPL3 was not reached.
    pub cpl3_retired: u64,
    /// S4-39f: the last system calls of a user-mode run (a ring of
    /// [`SYSCALL_JOURNAL_CAPACITY`] entries); empty without `user_mode`.
    pub syscall_journal: SyscallJournal,
    /// How many guest `#PF` and `#GP(0)` exceptions the CPL0 set board delivered through the guest
    /// IDT in this run (0 on the `paged_probe` path). A delivery is not a retired instruction.
    pub exceptions_delivered: u64,
    /// Guest `#PF` exception deliveries in this run (0 on the `paged_probe` path).
    pub page_faults_delivered: u64,
    /// Guest `#GP(0)` exception deliveries in this run (0 on the `paged_probe` path).
    pub general_protection_faults_delivered: u64,
    /// S4-38L8b: external interrupts the CPL0 set board delivered through the guest IDT in this
    /// run. `0` while the board has no interrupt source (and on the `paged_probe` path).
    pub irqs_delivered: u64,
    /// S4-38L8b: IRQ 0 edges the board source reported; `0` while there is no source.
    pub irq0_edges: u64,
    /// S4-38L8b: instruction boundaries at which a pending vector was held back by RFLAGS.IF = 0.
    pub irq_refused_masked: u64,
    /// S4-38L8b: the same because of the one-instruction STI shadow.
    pub irq_refused_shadow: u64,
    /// S4-38L8b: quanta the board shortened to the deadline of the interrupt source.
    pub quantum_cut_by_deadline: u64,
    /// S4-39g: times the board woke a CPU halted by `HLT` (IF = 1) from the source's wake time;
    /// `0` without a source that models the wake.
    pub halt_wakes: u64,
    /// S4-39g: virtual nanoseconds the board clock skipped while the CPU was halted.
    pub halt_idle_ns: u64,
    /// S4-38L7: the run was made with the diagnostic mode past C7
    /// ([`LinuxCpl0SetMembers::diagnostic_past_c7`]): the CPUID boundary was off, so the result is
    /// diagnostic only and is NOT counted. `false` on every other run.
    pub diagnostic_past_c7: bool,
    /// S4-38L4i: the checkpoints C0..C17 reached by the run, in id order (only those reached), when
    /// the run was made with a checkpoint table ([`run_linux_direct_board_set_observed`]); `None`
    /// otherwise, and then the record is what it always was.
    pub checkpoints: Option<Vec<LinuxCheckpointRecord>>,
}

/// S4-39f: how many system calls the journal of a stop record keeps (the last ones).
pub const SYSCALL_JOURNAL_CAPACITY: usize = 32;

/// S4-39f: one system call of a user-mode run: the number (RAX at `SYSCALL`), the return address
/// (RCX at `SYSCALL`) and the result (RAX at the `SYSRETQ` that returned; `None` while the call has
/// not returned, for example at the stop).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SyscallJournalEntry {
    pub nr: u64,
    pub rip: u64,
    pub rax_return: Option<u64>,
}

/// S4-39f: the ring of the last [`SYSCALL_JOURNAL_CAPACITY`] system calls of a user-mode run, oldest
/// first, with the totals. Written at `SYSCALL` (entry) and `SYSRETQ` (exit) only.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyscallJournal {
    pub entries: std::collections::VecDeque<SyscallJournalEntry>,
    /// `SYSCALL` instructions retired in the run (not capped).
    pub entered: u64,
    /// `SYSRETQ` instructions retired in the run (not capped).
    pub returned: u64,
}

impl SyscallJournal {
    pub(crate) fn record(&mut self, event: crate::paged_memory_adapter::SyscallEvent) {
        self.record_matching(event, |_| true);
    }

    fn record_matching(
        &mut self,
        event: crate::paged_memory_adapter::SyscallEvent,
        mut matches: impl FnMut(usize) -> bool,
    ) {
        use crate::paged_memory_adapter::SyscallEvent;
        match event {
            SyscallEvent::Enter { nr, caller } => {
                if self.entries.len() == SYSCALL_JOURNAL_CAPACITY {
                    self.entries.pop_front();
                }
                self.entries.push_back(SyscallJournalEntry {
                    nr,
                    rip: caller.rip,
                    rax_return: None,
                });
                self.entered += 1;
            }
            SyscallEvent::Exit { rax, .. } => {
                // The return belongs to the latest call that has not returned yet.
                if let Some((_, entry)) = self
                    .entries
                    .iter_mut()
                    .enumerate()
                    .rev()
                    .find(|(index, entry)| entry.rax_return.is_none() && matches(*index))
                {
                    entry.rax_return = Some(rax);
                }
                self.returned += 1;
            }
        }
    }
}

fn exit_kind(exit: &PagedProbeExit) -> &'static str {
    match exit {
        PagedProbeExit::BudgetExhausted => "budget_exhausted",
        PagedProbeExit::Stopped => "stopped",
        PagedProbeExit::Halted => "halted",
        PagedProbeExit::Shutdown => "shutdown",
        PagedProbeExit::Rejected(_) => "rejected",
        PagedProbeExit::IntegrationError(_) => "integration_error",
        PagedProbeExit::Unsupported { .. } => "unsupported",
        PagedProbeExit::Fault(_) => "fault",
        PagedProbeExit::Backend(_) => "backend",
        PagedProbeExit::PortIo(_) => "port_io",
    }
}

/// Runs a loaded Linux board for at most `instruction_budget` instructions through the 2 MiB
/// walk (`run_paged_probe_2m_ram`, base CPU profile, the board CR3) and describes where it
/// stopped (S4-38L1). The ticks of the Linux profile are applied as in `run_boot`. Nothing is
/// executed that the base profile does not admit: the first inadmissible instruction is the stop
/// (`Unsupported`). `input` must be the input `boot` was loaded from (it is hashed here).
///
/// The board is left at the stop (a second call continues from it); `boot.retired` stays the
/// loader's 0, the record carries the count.
pub fn run_linux_direct_board(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    check_single_vcpu(boot)?;
    check_nvme_interrupt_profile(boot, false)?;
    let pm_timer_reads_before = boot.board.acpi_pm().map(|pm| pm.timer_reads());
    let report = boot
        .board
        .run_linux_boot(instruction_budget)
        .map_err(|failure| {
            let mut error = LinuxDirectError::new("run_failed", "run", failure.error);
            error.retired = Some(failure.retired);
            error
        })?;
    Ok(stop_record(
        boot,
        input,
        report,
        pm_timer_reads_before,
        SetExtras {
            path: "paged_probe",
            members: Vec::new(),
            exit_detail: None,
            changes: &[],
            cpuid_divergence_leaf: None,
            user_entry: None,
            user_mode: false,
            cpl3_retired: 0,
            syscall_journal: SyscallJournal::default(),
            exceptions_delivered: 0,
            page_faults_delivered: 0,
            general_protection_faults_delivered: 0,
            execution_observations: None,
            irq: IrqCounters::default(),
            diagnostic_past_c7: false,
            checkpoints: None,
        },
    ))
}

fn check_nvme_interrupt_profile(
    boot: &LinuxDirectBoot,
    apic: bool,
) -> Result<(), LinuxDirectError> {
    if !apic
        && boot
            .board
            .nvme()
            .is_some_and(|nvme| nvme.has_pci_transport())
    {
        let mut error = LinuxDirectError::text(
            "run_failed",
            "run",
            "PCI NVMe requires the X8664V1Stage4AApic execution profile".into(),
        );
        error.retired = Some(0);
        return Err(error);
    }
    Ok(())
}

fn check_single_vcpu(boot: &LinuxDirectBoot) -> Result<(), LinuxDirectError> {
    if boot.board.vcpu_count() != 1 {
        return Err(linux_irq_setup_error(
            "multiple vCPUs require interleaved LinuxDirect",
        ));
    }
    Ok(())
}

fn prepare_apic_profile(
    board: &LoadedFirmwareBoard,
    members: &mut LinuxCpl0SetMembers,
) -> Result<(), LinuxDirectError> {
    if !board.pit_ioapic_irq_available() {
        return Err(linux_irq_setup_error(
            "the X8664V1Stage4AApic profile needs the Linux-profile PIT and no 8259 source",
        ));
    }
    members.halt = true;
    members.irq = true;
    members.clock_owner = true;
    // Linux reads IA32_APIC_BASE when it identifies the BSP.
    members.msr_apic_base = true;
    Ok(())
}

fn install_linux_ioapic_sources(board: &mut LoadedFirmwareBoard) -> Result<(), LinuxDirectError> {
    if !board.enable_pit_ioapic_irq() {
        return Err(linux_irq_setup_error(
            "the X8664V1Stage4AApic profile could not install the IOAPIC timer source",
        ));
    }
    if !board.enable_uart_ioapic_irq4() {
        return Err(linux_irq_setup_error(
            "the X8664V1Stage4AApic profile could not install the IOAPIC UART source",
        ));
    }
    Ok(())
}

fn linux_irq_setup_error(detail: &str) -> LinuxDirectError {
    let mut error = LinuxDirectError::text("run_failed", "run", detail.to_owned());
    error.retired = Some(0);
    error
}

/// What the set path adds to a stop record.
struct SetExtras<'a> {
    path: &'static str,
    members: Vec<String>,
    exit_detail: Option<String>,
    changes: &'a [TranslationContextChange],
    cpuid_divergence_leaf: Option<u32>,
    user_entry: Option<IretqUserEntry>,
    user_mode: bool,
    cpl3_retired: u64,
    syscall_journal: SyscallJournal,
    exceptions_delivered: u64,
    page_faults_delivered: u64,
    general_protection_faults_delivered: u64,
    execution_observations: Option<LinuxExecutionObservations>,
    irq: IrqCounters,
    diagnostic_past_c7: bool,
    checkpoints: Option<Vec<LinuxCheckpointRecord>>,
}

fn stop_record(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    report: FirmwareRunReport,
    pm_timer_reads_before: Option<u64>,
    extras: SetExtras<'_>,
) -> LinuxStopRecord {
    let view = boot
        .board
        .fetch_view_2m(boot.board.state().rip, LINUX_STOP_RIP_BYTES);
    stop_record_with_view(
        boot,
        input,
        report,
        pm_timer_reads_before,
        extras,
        view,
        BOOT_VCPU_INDEX,
    )
}

fn stop_record_with_view(
    boot: &LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    report: FirmwareRunReport,
    pm_timer_reads_before: Option<u64>,
    extras: SetExtras<'_>,
    (rip_gpa, rip_bytes, rip_bytes_error): (Option<u64>, Vec<u8>, Option<String>),
    cpu: usize,
) -> LinuxStopRecord {
    let vcpu = boot.board.vcpu(cpu).expect("reporting vCPU exists");
    let state = &vcpu.state;
    let unsupported_opcode = match report.exit {
        PagedProbeExit::Unsupported { opcode } => Some(opcode),
        _ => None,
    };
    let pm_timer_reads = pm_timer_reads_before
        .zip(boot.board.acpi_pm().map(|pm| pm.timer_reads()))
        .map(|(before, after)| after.saturating_sub(before));
    LinuxStopRecord {
        exit_kind: if extras.cpuid_divergence_leaf.is_some() {
            "cpuid_divergence"
        } else if extras.user_entry.is_some() {
            "user_entry"
        } else if extras.user_mode && state.cs_selector.is_some_and(|cs| cs & 3 == 3) {
            // S4-39c: a stop in CPL3 of a user-mode run.
            match exit_kind(&report.exit) {
                "fault" => "user_mode_fault",
                "unsupported" => "user_mode_unsupported",
                other => other,
            }
        } else {
            exit_kind(&report.exit)
        },
        exit_detail: format!("{:?}", report.exit),
        stop_reason: report.stop_reason,
        exit: report.exit,
        unsupported_opcode,
        rip: state.rip,
        rip_gpa,
        rip_bytes,
        rip_bytes_error,
        mode: "long64",
        cpl: state.cs_selector.map_or(0, |cs| (cs & 3) as u8),
        cr0: state.cr0,
        cr3: if cpu == BOOT_VCPU_INDEX {
            boot.board.cr3()
        } else {
            state.cr3.unwrap_or(0)
        },
        cr4: state.cr4,
        efer: boot.board.efer(),
        guest_efer: vcpu.guest_efer,
        pci_config_address: boot.board.pci_config_address(),
        guest_star: vcpu.guest_star,
        guest_lstar: vcpu.guest_lstar,
        guest_cstar: vcpu.guest_cstar,
        guest_sysenter_cs: vcpu.guest_sysenter_cs,
        guest_sysenter_esp: vcpu.guest_sysenter_esp,
        guest_sysenter_eip: vcpu.guest_sysenter_eip,
        guest_syscall_mask: vcpu.guest_syscall_mask,
        guest_kernel_gs_base: vcpu.guest_kernel_gs_base,
        rflags: state.rflags,
        gpr: state.gpr,
        retired: report.retired,
        quanta: report.quanta,
        virtual_ns: report.virtual_ns,
        pm_timer_reads,
        rdtsc_reads: report.rdtsc_reads,
        rdtsc_reads_by_cpl: report.rdtsc_reads_by_cpl,
        execution_observations: extras.execution_observations,
        pit_port_b_reads: report.pit_port_b_reads,
        pit_counter_port_reads: report.pit_counter_port_reads,
        tsc_hz: report.tsc_hz,
        com1: report.com1,
        linux_profile: boot.board.has_linux_profile(),
        page_fault: report.page_fault.map(|record| format!("{record:?}")),
        memory_error: report.memory_error.map(|error| format!("{error:?}")),
        inputs: LinuxStopInputs {
            bzimage_sha256: sha256(input.bzimage),
            initrd_sha256: (!input.initrd.is_empty()).then(|| sha256(input.initrd)),
            acpi_blob_sha256: sha256(input.acpi_blob),
            acpi_manifest_hash: boot.acpi.manifest_hash,
            kernel_pm_code_sha256_in_memory: boot.kernel_sha256_in_memory,
            initrd_sha256_in_memory: boot.initrd_sha256_in_memory,
        },
        path: extras.path,
        set_members: extras.members,
        set_exit_detail: extras.exit_detail,
        cpuid_divergence_leaf: extras.cpuid_divergence_leaf,
        user_entry: extras.user_entry,
        user_mode: extras.user_mode,
        cpl3_retired: extras.cpl3_retired,
        syscall_journal: extras.syscall_journal,
        exceptions_delivered: extras.exceptions_delivered,
        page_faults_delivered: extras.page_faults_delivered,
        general_protection_faults_delivered: extras.general_protection_faults_delivered,
        irqs_delivered: extras.irq.irqs_delivered,
        irq0_edges: extras.irq.irq0_edges,
        irq_refused_masked: extras.irq.irq_refused_masked,
        irq_refused_shadow: extras.irq.irq_refused_shadow,
        quantum_cut_by_deadline: extras.irq.quantum_cut_by_deadline,
        halt_wakes: extras.irq.halt_wakes,
        halt_idle_ns: extras.irq.halt_idle_ns,
        diagnostic_past_c7: extras.diagnostic_past_c7,
        checkpoints: extras.checkpoints,
        cr3_changes: extras.changes.len() as u64,
        // The set loop records `MOV CR3` only; anything else it stops on as an integration error.
        last_cr3_change: extras
            .changes
            .iter()
            .rev()
            .find_map(|change| match *change {
                TranslationContextChange::Cr3Write { old_cr3, new_cr3 } => Some((old_cr3, new_cr3)),
                _ => None,
            }),
    }
}

/// The members of the CPL0 profile set a Linux run is built with (S4-38L3). `CLI` is always
/// there (`startup_64` begins `CLD; CLI`). [`Self::minimal`] adds nothing for a form the kernel has
/// not yet reached; each further member is added by the slice that moves the first stop onto the
/// form that needs it. [`Self::linux_default`] is the set of the runner (`--set-cpl0`): since
/// S4-38L4 it also has `cpuid`, with the CPUID boundary (see [`run_linux_direct_board_set`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LinuxCpl0SetMembers {
    /// `with_cpu_owned_cr3`: `MOV CR3, RAX` retires and the walker follows it. The board CR3 is
    /// handed to the CPU either way; without this member the set refuses the state
    /// (`Cr3NotAdmitted`, a negative control).
    pub cpu_owned_cr3: bool,
    /// `with_cpuid(<profile>)` (the profile is [`Self::cpuid_profile`]): only what that profile
    /// declares, nothing more. With the default profile `DiagnosticV1` the run also stops before a
    /// `CPUID` of leaf 1 or 0x8000_0001 (`cpuid_divergence`, S4-38L4): that profile does not answer
    /// the feature leaves of the target. With `X8664V1Stage4A` there is no such stop (see
    /// [`Self::cpuid_profile`]).
    pub cpuid: bool,
    /// The CPUID profile of the `cpuid` member (S4-38L9a). [`CpuidProfile::DiagnosticV1`] (the
    /// default of [`Self::minimal`] and [`Self::linux_default`]) keeps the C7 boundary above.
    /// [`CpuidProfile::X8664V1Stage4A`] is the declared subset of the target (TSC, since S4-38L9d also FPU and FXSR): it
    /// answers leaf 1 and 0x8000_0001 honestly for that subset, so the C7 boundary is OFF for it
    /// (nothing made-up to stop on) and the result is still not counted as the target CPU. Has no
    /// effect without `cpuid`. Not a separate set member: it names the `cpuid:<profile>` member.
    pub cpuid_profile: CpuidProfile,
    /// `with_rdtsc`: the source is the board clock, which also queues the 1 us RDTSC tick like
    /// `run_boot`; admitted only under the board's Linux profile.
    pub rdtsc: bool,
    /// `with_msr(EferReadOnlyV1(board EFER))` (S4-38L7e: in [`Self::linux_default`]): `RDMSR` of
    /// IA32_EFER at CPL0 gives the board EFER (`EDX:EAX`); `WRMSR` of it and every other MSR
    /// stay `Unsupported` (the EFER value is not changed by the run).
    pub msr: bool,
    /// `with_msr_fs_gs_base` (S4-38L6b): `WRMSR`/`RDMSR` of IA32_FS_BASE and IA32_GS_BASE at
    /// CPL0 (`EDX:EAX`, canonical 48-bit, stored in the CPU state); no other MSR. Independent of
    /// [`Self::msr`] (the read-only EFER profile), which it does not replace.
    pub msr_fs_gs_base: bool,
    /// `with_msr_star_write` (S4-38L7ao): `WRMSR` of IA32_STAR at CPL0 stores `EDX:EAX` in the
    /// board (`guest_star` of the stop record). Write-only: `RDMSR` of STAR and every other MSR
    /// stay `Unsupported`; `SYSCALL`/`SYSRET` are not modelled. Independent of [`Self::msr`].
    pub msr_star: bool,
    /// `with_msr_lstar_write` (S4-38L7ap): `WRMSR` of IA32_LSTAR at CPL0 stores `EDX:EAX` in the
    /// board (`guest_lstar` of the stop record). Write-only: `RDMSR` of LSTAR and every other MSR
    /// (CSTAR, SYSCALL_MASK included) stay `Unsupported`; `SYSCALL`/`SYSRET` are not modelled.
    /// Independent of [`Self::msr`] and [`Self::msr_star`].
    pub msr_lstar: bool,
    /// `with_msr_cstar_write` (S4-38L7aq): `WRMSR` of IA32_CSTAR at CPL0 stores `EDX:EAX` in the
    /// board (`guest_cstar` of the stop record). Write-only: `RDMSR` of CSTAR and every other MSR
    /// (SYSCALL_MASK included) stay `Unsupported`; `SYSCALL`/`SYSRET` are not modelled.
    /// Independent of [`Self::msr`], [`Self::msr_star`] and [`Self::msr_lstar`].
    pub msr_cstar: bool,
    /// `with_msr_sysenter_cs_write` (S4-38L7ar): `WRMSR` of IA32_SYSENTER_CS (`0x174`) at CPL0 stores
    /// `EDX:EAX` (the full 64-bit value) in the board (`guest_sysenter_cs` of the stop record).
    /// Write-only: `RDMSR` of it, SYSENTER_ESP/EIP and every other MSR stay `Unsupported`;
    /// `SYSENTER`/`SYSEXIT` are not modelled. Independent of [`Self::msr`] and the SYSCALL members.
    /// With `MsrProfile::SysenterProbeV1` as the MSR profile the member would have no effect (the
    /// profile's `#GP(0)` wins); the board set uses `EferReadOnlyV1`.
    pub msr_sysenter_cs: bool,
    /// `with_msr_sysenter_stack_write` (S4-38L7as): `WRMSR` of IA32_SYSENTER_ESP (`0x175`) and
    /// IA32_SYSENTER_EIP (`0x176`) at CPL0 stores `EDX:EAX` (the full 64-bit value) in the board
    /// (`guest_sysenter_esp` / `guest_sysenter_eip` of the stop record); one member, two cells, a
    /// write changes only its own cell. Write-only: `RDMSR` of them and every other MSR stay
    /// `Unsupported`; `SYSENTER`/`SYSEXIT` are not modelled. Independent of [`Self::msr`], the
    /// SYSCALL members and [`Self::msr_sysenter_cs`]. With `MsrProfile::SysenterProbeV1` as the MSR
    /// profile the member would have no effect (the profile's `#GP(0)` wins); the board set uses
    /// `EferReadOnlyV1`.
    pub msr_sysenter_stack: bool,
    /// `with_msr_syscall_mask_write` (S4-38L7at): `WRMSR` of IA32_SYSCALL_MASK (`0xC000_0084`) at
    /// CPL0 stores `EDX:EAX` (the full 64-bit value, no bit semantics) in the board
    /// (`guest_syscall_mask` of the stop record). Write-only: `RDMSR` of it and every other MSR
    /// stay `Unsupported`; `SYSCALL`/`SYSRET` are not modelled. Independent of [`Self::msr`], the
    /// other MSR members. No MSR profile covers the index today, so nothing overrides the member.
    pub msr_syscall_mask: bool,
    /// `with_msr_kernel_gs_base_write` (S4-38L7au): `WRMSR` of IA32_KERNEL_GS_BASE (`0xC000_0102`)
    /// at CPL0 stores `EDX:EAX` (the full 64-bit value, no canonical check) in the board
    /// (`guest_kernel_gs_base` of the stop record). Write-only: `RDMSR` of it and every other MSR
    /// stay `Unsupported`; `SWAPGS` is not modelled. Independent of [`Self::msr`], the other MSR
    /// members and the FS/GS base member (which covers `0xC000_0100`/`0xC000_0101` only). The board
    /// set's MSR profile is `EferReadOnlyV1`, which does not handle the index.
    pub msr_kernel_gs_base: bool,
    /// `with_msr_tdx_keyid_partitioning_read` (S4-38L9c): `RDMSR` of IA32_MKTME_KEYID_PARTITIONING
    /// (`0x87`) at CPL0 returns `EDX:EAX = 0:0` (no MKTME and no TDX private KeyIDs; the kernel's
    /// `tdx_init` then leaves TDX off, as it does on a CPU where the MSR gives `#GP`). No cell, no
    /// `WRMSR`; every other MSR keeps its refusal. `false` in [`Self::minimal`] and in
    /// [`Self::linux_default`] (so a default run is unchanged); the runner turns it on only with
    /// `--cpuid-profile stage4a`. Independent of [`Self::msr`] and the other MSR members.
    pub msr_tdx_keyid_partitioning_read: bool,
    /// `with_msr_apic_base` (S4-38L8g): `RDMSR` of IA32_APIC_BASE (`0x1B`) at CPL0 returns
    /// `EDX:EAX = 0:0xFEE00900` (base `0xFEE00000`, EN, BSP); `WRMSR` accepts exactly that value and
    /// refuses every other one (`Unsupported`) before any change. `false` in [`Self::minimal`] and
    /// [`Self::linux_default`]; `run_linux_direct_board_set_observed` turns it on only for the APIC
    /// profile (`X8664V1Stage4AApic`), whose board has the Local APIC enabled at that base, so the
    /// member list of `stage4a` and of the default run does not change.
    pub msr_apic_base: bool,
    /// `with_fp_state` (S4-38L9d): the FP member of the set: `FNINIT`, the plain RIP-relative
    /// `FXSAVE` and `MOV CR0, RDI` (the `fpu__init_system` path of the kernel), with the guest FP
    /// state kept in the board between runs (`guest_fp`). It needs a CPUID profile that declares
    /// FPU and FXSR (`X8664V1Stage4A`; with `DiagnosticV1` the set refuses the run). `false` in
    /// [`Self::minimal`] and in [`Self::linux_default`] (so a default run is unchanged); the
    /// runner turns it on only with `--cpuid-profile stage4a`.
    pub fp_state: bool,
    /// `with_int3` (S4-38L9e): the bare `INT3` (`CC`) delivered as `#BP` (vector 3) through the
    /// guest IDT gate (the kernel installs an interrupt gate, type 0xE, DPL 3, IST 0), with the
    /// five-qword frame holding the address after the `INT3`, and the instruction retires; the
    /// kernel's `int3_selftest` needs it. `false` in [`Self::minimal`] and in
    /// [`Self::linux_default`] (so a default run, its member list included, is unchanged); the
    /// runner turns it on only with `--cpuid-profile stage4a`, like `fp_state`. Independent of
    /// the other members. A missing IDTR/GDTR, a gate that is not deliverable and a shadow that
    /// is not `Clear` make the `INT3` itself `Unsupported` (opcode `0xcc`) with no write.
    pub int3: bool,
    /// `with_halt`.
    pub halt: bool,
    /// `with_sti` (S4-38L7bp): the bare `STI` (`FB`): IF := 1 and the one-instruction shadow. The
    /// board delivers no interrupt (IRQ delivery is S4-38, ISSUE-069); the member models the
    /// instruction only. `false` in [`Self::minimal`], `true` in [`Self::linux_default`].
    pub sti: bool,
    /// `with_pio_u8`: `IN AL, DX` / `OUT DX, AL` through the machine bus: every port bound on the
    /// board bus (COM1, PM block with the timer, and with the Linux profile PIT, port B, PIC,
    /// CMOS, `0xED`, `0x80`, and the byte accesses to PCI conf1 `0xCF8..=0xCFF`).
    /// `false` in [`Self::minimal`], `true` in [`Self::linux_default`] (S4-38L7x).
    pub pio_u8: bool,
    /// `with_popf_cpl0` (S2-50): the qword `POPFQ` with the CPL0 IOPL/IF rule. TF stays outside.
    pub popf_cpl0: bool,
    /// `with_pci_config` (S4-38L7w): the dword/word `OUT DX, EAX`/`IN EAX, DX` (`EF`/`ED`, `66 EF`/
    /// `66 ED`) for PCI mechanism #1: CONFIG_ADDRESS (`0xCF8`, dword access only; the word form is
    /// refused) is stored in the board, CONFIG_DATA (`0xCFC..=0xCFF`) has no device (`IN` gives all
    /// ones, `OUT` is discarded). Independent of
    /// [`Self::pio_u8`].
    pub pci_config: bool,
    /// S4-13: the opt-in translation cache of the instruction fetch (one cache per call of the
    /// set loop, invalidated on every guest write to a table of a cached translation and on
    /// every translation context change). It is not a member of the CPU profile (it adds no
    /// instruction and is not in `set_members`) and changes no result: only the number of page
    /// walks. `false` in [`Self::minimal`], `true` in [`Self::linux_default`]; a diagnostic run
    /// turns it off to compare.
    pub fetch_cache: bool,
    /// S4-38L7: opt-in DIAGNOSTIC mode past checkpoint C7. With it the CPUID boundary of S4-38L4
    /// is off: `CPUID` leaf 1 and 0x8000_0001 execute with the DiagnosticV1 answer like every
    /// other leaf (no substituted "better" values, no kernel patch) and the run goes on. The result
    /// is NOT counted (DiagnosticV1 is not a profile of the target CPU); the stop record says so
    /// (`diagnostic_past_c7`). Not a member of the CPU profile and not in `set_members`. `false` in
    /// [`Self::minimal`] and [`Self::linux_default`]. Has no effect without `cpuid`.
    pub diagnostic_past_c7: bool,
    /// S4-38L8b: the interrupt hook of the CPL0 set loop. With it each instruction boundary asks
    /// the board's interrupt source for a pending vector and delivers it through the guest IDT
    /// when RFLAGS.IF = 1 and the STI shadow is `Clear` (the loop holds the vector back
    /// otherwise, and counts it). The CPU activity is checked by the delivery itself: with
    /// IF = 1, a `Clear` shadow and an activity other than `Running` it refuses
    /// (`ActivityOutsideProfile`) and the run stops with `run_failed`, not a quiet skip. A quantum is cut to
    /// the source's next deadline and a guest write to the PIT ports ends the quantum when the
    /// source asks for it. The board has no source yet (an empty one that never gives a vector),
    /// so with or without the member a run is the same. Not a CPU instruction, but a member of
    /// the board set: it is listed in `set_members` as `irq`. `false` in [`Self::minimal`], `true`
    /// in [`Self::linux_default`].
    pub irq: bool,
    /// S4-23j: the single clock owner drives the clock of the set loop (`ClockOwner`, decision D7:
    /// retired instructions one ns each, device ticks, the idle jump of a halted CPU through the
    /// idle arbiter, the quantum cut by `ClockOwner::step_budget`). Opt-in: `false` in
    /// [`Self::minimal`] and [`Self::linux_default`], and the member is named `clock_owner` in
    /// `set_members` only when it is on, so a default run (and every manifest) is byte-for-byte
    /// the same. The board claims the clock for good (a later standalone advance fails with
    /// `ClockAlreadyOwned`); the run is refused as `run_failed` when the board cannot give it
    /// (a timer on another clock, an owner already present). The Linux-visible frequency and the
    /// LAPIC timer as a wake source stay S4-38.
    pub clock_owner: bool,
}

impl LinuxCpl0SetMembers {
    /// CLI and the CPU-owned CR3 only.
    pub const fn minimal() -> Self {
        Self {
            cpu_owned_cr3: true,
            cpuid: false,
            cpuid_profile: CpuidProfile::DiagnosticV1,
            rdtsc: false,
            msr: false,
            msr_fs_gs_base: false,
            msr_star: false,
            msr_lstar: false,
            msr_cstar: false,
            msr_sysenter_cs: false,
            msr_sysenter_stack: false,
            msr_syscall_mask: false,
            msr_kernel_gs_base: false,
            msr_tdx_keyid_partitioning_read: false,
            msr_apic_base: false,
            fp_state: false,
            int3: false,
            halt: false,
            sti: false,
            pio_u8: false,
            popf_cpl0: false,
            pci_config: false,
            fetch_cache: false,
            diagnostic_past_c7: false,
            irq: false,
            clock_owner: false,
        }
    }

    /// The set of the Linux board run (`--set-cpl0`): CLI, the CPU-owned CR3, CPUID
    /// (DiagnosticV1 with the divergence boundary, S4-38L4), RDTSC from the board clock (S4-38L7bh),
    /// the qword `POPFQ` (S2-50), the FS/GS
    /// base MSRs (S4-38L6b), the read-only EFER (S4-38L7e), the PCI configuration ports
    /// (S4-38L7w) and the byte PIO (S4-38L7x: `pio_u8`, all ports bound on the board bus; a default
    /// decision that may be revisited: without COM1 and the timer the kernel does not go further).
    /// Known limit: byte accesses to `0xCF8..` use the bus model's own conf1 latch, dword accesses
    /// the `pci_config` cell; they agree while the bus has no PCI function.
    /// [`Self::minimal`] stays without it.
    pub const fn linux_default() -> Self {
        Self {
            cpuid: true,
            rdtsc: true,
            popf_cpl0: true,
            sti: true,
            pio_u8: true,
            pci_config: true,
            msr: true,
            msr_fs_gs_base: true,
            msr_star: true,
            msr_lstar: true,
            msr_cstar: true,
            msr_sysenter_cs: true,
            msr_sysenter_stack: true,
            msr_syscall_mask: true,
            msr_kernel_gs_base: true,
            msr_tdx_keyid_partitioning_read: false,
            msr_apic_base: false,
            fp_state: false,
            int3: false,
            fetch_cache: true,
            diagnostic_past_c7: false,
            irq: true,
            ..Self::minimal()
        }
    }
}

/// [`run_linux_direct_board`] through the CPL0 profile set of od-isa (S2-45) instead of the
/// base probe profile (S4-38L3). The walker, the board CR3, the Linux-profile ticks and the stop
/// condition are the same; the differences are the CPU profile (`members`) and that a `MOV CR3`
/// is a continuation (the walker is rebuilt for the new CR3) whose count and last value the
/// record carries. The board CR3 afterwards is the CR3 of the last retired `MOV CR3`.
///
/// Nothing is executed that the set does not admit: the first inadmissible instruction is the
/// stop (`Unsupported`), and the segment caches the loader left are not touched.
///
/// CPUID boundary (S4-38L4, checkpoint C7), for the `DiagnosticV1` profile only (the default; for
/// `X8664V1Stage4A` see the profile paragraph below): with `members.cpuid` the run stops BEFORE
/// retiring a `CPUID` whose EAX is 1 or 0x8000_0001 (`exit_kind` `cpuid_divergence`,
/// `cpuid_divergence_leaf` set). The board does not answer those leaves with the DiagnosticV1 values (a made-up feature
/// word would steer the kernel) and does not patch the kernel: the run ends at the honest edge.
/// Other leaves (0, 0x8000_0000, ...) are answered by DiagnosticV1 as the set does. Results past
/// the edge are diagnostic only. Without the member `CPUID` stays `Unsupported` (0xa2).
///
/// Diagnostic mode (S4-38L7, `members.diagnostic_past_c7`): the boundary is off. Leaf 1 and
/// 0x8000_0001 are answered by DiagnosticV1 as every other leaf and the run goes on; the record
/// carries `diagnostic_past_c7: true` and the result is NOT counted.
///
/// CPUID profile (S4-38L9a, `members.cpuid_profile`): the boundary above belongs to
/// `DiagnosticV1`. With `X8664V1Stage4A` (the declared subset: TSC, FPU, FXSR) the boundary is off even
/// without `diagnostic_past_c7`: leaf 1 and 0x8000_0001 are answered for what the subset declares
/// and the member is named `cpuid:X8664V1Stage4A`.
pub fn run_linux_direct_board_set(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with(boot, input, instruction_budget, members, false, None, false)
}

/// [`run_linux_direct_board_set`] with the user mode of the board (S4-39c): the set is built
/// `with_user_mode` (named `user_mode` in `set_members`), so an `IRETQ` to CPL3 that passes every
/// check commits, and the board then follows the CPL of the state: CS RPL 0 runs the CPL0 entry,
/// RPL 3 the CPL3 entry (`run_with_cpl3_profile_set`, its allow-list and Ring3 context). The run
/// goes on from CPL0 into CPL3 on the same state; the record carries `user_mode` and
/// `cpl3_retired`, and a stop in CPL3 is `user_mode_fault` / `user_mode_unsupported`. Supported
/// CPL3 `#PF`/`#GP(0)` exceptions and external IRQs switch through `TSS.RSP0` to their CPL0
/// handlers (S4-39d/S4-39h). A refused or aborted external IRQ ends as `run_failed`; failed
/// `#PF`/`#GP` delivery remains a guest fault with its delivery detail.
/// Without the user mode the `IRETQ` stops at the `user_entry` boundary as before. S4-39e: a user-mode set that carries
/// the EFER (`msr`), STAR, LSTAR and SYSCALL_MASK members also gets `with_syscall` (named
/// `syscall` in `set_members`), so `SYSCALL` from CPL3 enters the kernel at LSTAR.
pub fn run_linux_direct_board_set_user_mode(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with(boot, input, instruction_budget, members, true, None, false)
}

/// [`run_linux_direct_board_set`] (`user_mode` false) or [`run_linux_direct_board_set_user_mode`]
/// (`user_mode` true) with the opt-in checkpoint recorder of S4-38L4i. With `Some(table)` the
/// board records C0..C17 on the instruction boundaries of the loop (see `linux_checkpoints`) and
/// the record carries them in `checkpoints`; with `None` this is exactly the function it wraps.
/// The recorder only observes: the run retires the same instructions in the same quanta and the
/// record is equal to the one without it except for the `checkpoints` field.
pub fn run_linux_direct_board_set_observed(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    user_mode: bool,
    checkpoints: Option<LinuxCheckpointTable>,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with(
        boot,
        input,
        instruction_budget,
        members,
        user_mode,
        checkpoints,
        false,
    )
}

/// One-shot user-mode CPL0-set run with bounded RDTSC-site and delivered-#PF observations.
/// The diagnostic fields are absent from ordinary runs and do not alter guest-visible state.
pub fn run_linux_direct_board_set_user_mode_with_execution_observations(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_user_mode_with_execution_observations_options(
        boot,
        input,
        instruction_budget,
        members,
        None,
        None,
    )
}

/// One-shot user-mode run with execution observations and optional focused RDTSC and post-IRET
/// captures. A selected RDTSC RIP records bounded post-instruction register samples only.
pub fn run_linux_direct_board_set_user_mode_with_execution_observations_options(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    selected_post_iret_ordinal: Option<u64>,
    selected_rdtsc_sample_rip: Option<u64>,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_user_mode_with_execution_observations_sampling_options(
        boot,
        input,
        instruction_budget,
        members,
        selected_post_iret_ordinal,
        selected_rdtsc_sample_rip,
        1,
        None,
        None,
        None,
        false,
    )
}

/// One-shot user-mode run with explicit deterministic stride for selected-RIP samples.
pub fn run_linux_direct_board_set_user_mode_with_execution_observations_sampling_options(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    selected_post_iret_ordinal: Option<u64>,
    selected_rdtsc_sample_rip: Option<u64>,
    selected_rdtsc_sample_stride: u64,
    selected_rdtsc_entry_sample_rip: Option<u64>,
    selected_rdtsc_sample_window: Option<crate::RdtscSampleWindow>,
    selected_rdtsc_prefix_sentinel_rip: Option<u64>,
    selected_instruction_path_trace: bool,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_user_mode_with_elf_stack_copy(
        boot,
        input,
        instruction_budget,
        members,
        selected_post_iret_ordinal,
        selected_rdtsc_sample_rip,
        selected_rdtsc_sample_stride,
        selected_rdtsc_entry_sample_rip,
        selected_rdtsc_sample_window,
        selected_rdtsc_prefix_sentinel_rip,
        selected_instruction_path_trace,
        false,
    )
}

/// One-shot user-mode run with the exact S5-07 ELF stack copy observation enabled.
#[allow(clippy::too_many_arguments)]
pub fn run_linux_direct_board_set_user_mode_with_elf_stack_copy(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    selected_post_iret_ordinal: Option<u64>,
    selected_rdtsc_sample_rip: Option<u64>,
    selected_rdtsc_sample_stride: u64,
    selected_rdtsc_entry_sample_rip: Option<u64>,
    selected_rdtsc_sample_window: Option<crate::RdtscSampleWindow>,
    selected_rdtsc_prefix_sentinel_rip: Option<u64>,
    selected_instruction_path_trace: bool,
    selected_elf_stack_copy: bool,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_user_mode_with_delay_tsc_summary(
        boot,
        input,
        instruction_budget,
        members,
        selected_post_iret_ordinal,
        selected_rdtsc_sample_rip,
        selected_rdtsc_sample_stride,
        selected_rdtsc_entry_sample_rip,
        selected_rdtsc_sample_window,
        selected_rdtsc_prefix_sentinel_rip,
        selected_instruction_path_trace,
        selected_elf_stack_copy,
        false,
    )
}

/// One-shot user-mode run with the opt-in bounded delay_tsc call summary.
#[allow(clippy::too_many_arguments)]
pub fn run_linux_direct_board_set_user_mode_with_delay_tsc_summary(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    selected_post_iret_ordinal: Option<u64>,
    selected_rdtsc_sample_rip: Option<u64>,
    selected_rdtsc_sample_stride: u64,
    selected_rdtsc_entry_sample_rip: Option<u64>,
    selected_rdtsc_sample_window: Option<crate::RdtscSampleWindow>,
    selected_rdtsc_prefix_sentinel_rip: Option<u64>,
    selected_instruction_path_trace: bool,
    selected_elf_stack_copy: bool,
    selected_delay_tsc_call_summary: bool,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with_post_iret_index(
        boot,
        input,
        instruction_budget,
        members,
        true,
        None,
        true,
        selected_post_iret_ordinal,
        selected_rdtsc_sample_rip,
        selected_rdtsc_sample_stride,
        selected_rdtsc_entry_sample_rip,
        selected_rdtsc_sample_window,
        selected_rdtsc_prefix_sentinel_rip,
        selected_instruction_path_trace,
        selected_elf_stack_copy,
        selected_delay_tsc_call_summary,
    )
}

/// One-shot user-mode run with an additional read-only post-IRET walk for one delivered #PF.
pub fn run_linux_direct_board_set_user_mode_with_post_iret_index(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    delivered_page_fault_ordinal: u64,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with_post_iret_index(
        boot,
        input,
        instruction_budget,
        members,
        true,
        None,
        true,
        Some(delivered_page_fault_ordinal),
        None,
        1,
        None,
        None,
        None,
        false,
        false,
        false,
    )
}

fn run_linux_direct_board_set_with(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    user_mode: bool,
    checkpoint_table: Option<LinuxCheckpointTable>,
    capture_execution_observations: bool,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    run_linux_direct_board_set_with_post_iret_index(
        boot,
        input,
        instruction_budget,
        members,
        user_mode,
        checkpoint_table,
        capture_execution_observations,
        None,
        None,
        1,
        None,
        None,
        None,
        false,
        false,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn run_linux_direct_board_set_with_post_iret_index(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
    user_mode: bool,
    checkpoint_table: Option<LinuxCheckpointTable>,
    capture_execution_observations: bool,
    selected_post_iret_ordinal: Option<u64>,
    selected_rdtsc_sample_rip: Option<u64>,
    selected_rdtsc_sample_stride: u64,
    selected_rdtsc_entry_sample_rip: Option<u64>,
    selected_rdtsc_sample_window: Option<crate::RdtscSampleWindow>,
    selected_rdtsc_prefix_sentinel_rip: Option<u64>,
    selected_instruction_path_trace: bool,
    selected_elf_stack_copy: bool,
    selected_delay_tsc_call_summary: bool,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    check_single_vcpu(boot)?;
    check_nvme_interrupt_profile(
        boot,
        members.cpuid_profile == CpuidProfile::X8664V1Stage4AApic,
    )?;
    let pm_timer_reads_before = boot.board.acpi_pm().map(|pm| pm.timer_reads());
    // S4-39g: only the user-mode run of a board with the Linux-profile PIT and 8259 gets the
    // timer line: the PIT -> 8259 IR0 source becomes the board source, and the set carries `halt`
    // (an idle `sti; hlt` ends the quantum and the board wakes the CPU at the next timer edge)
    // and the interrupt hook. Every other run (also `--diagnostic-past-c7` without the flag,
    // `DiagnosticV1`, the base probe) keeps the board as it was.
    let mut members = members;
    members.clock_owner |= boot.board.linux_interleaved();
    // S4-38L8d: the APIC profile (`X8664V1Stage4AApic`) gets the timer line through the IOAPIC and the
    // Local APIC instead (PIT OUT0 -> INTIN2 -> LAPIC IRR), the clock owner (the LAPIC timer and the
    // idle wake need it: without it the tick hangs after `check_timer`), `halt` and the interrupt
    // hook. The 8259 source is not installed. S4-38L9-um: the user mode is allowed here too; the
    // CPL3 interrupt goes through the same boundary hook (the Local APIC IRR vector, the guest IDT,
    // `TSS.RSP0`) as at CPL0, so the source of the vector (IOAPIC or the LAPIC timer) is not
    // looked at by the privilege entry.
    if members.cpuid_profile == CpuidProfile::X8664V1Stage4AApic {
        // Only a check here: the board is changed after the last refusal below (the clock owner,
        // the checkpoint table), see `install_linux_ioapic_sources` further down.
        prepare_apic_profile(&boot.board, &mut members)?;
    } else if user_mode && boot.board.enable_pit_pic_irq() {
        // S4-39i: the UART THRE line goes to master IR4, so the kernel's 8250 driver can send
        // `tty` output (it writes THR from the THRE interrupt).
        boot.board.enable_uart_irq4();
        members.halt = true;
        members.irq = true;
    }
    let cpuid = members.cpuid_profile;
    let profile = LinuxCpl0Profile::new(members, user_mode, boot.board.efer());
    let cpu_cells = LinuxVcpuCells::from_vcpu(
        boot.board
            .vcpu(BOOT_VCPU_INDEX)
            .expect("LinuxDirect board has a boot vCPU"),
    );
    let pci_cell = Cell::new(boot.board.pci_config_address());
    let mut names = Vec::new();
    let set = profile.bind(&cpu_cells, &pci_cell, |member| {
        names.push(member.to_string())
    });
    if members.clock_owner {
        // S4-23j: before the first instruction, so a refusal leaves the guest untouched.
        boot.board
            .enable_clock_owner()
            .map_err(|error| LinuxDirectError::new("run_failed", "run", error))?;
    }
    // S4-38L4i: after every refusal that leaves the guest untouched, before the first
    // instruction; the recorder only observes.
    if let Some(table) = checkpoint_table {
        // A bad table is refused before the first instruction (`run_failed`, nothing retired).
        boot.board.enable_checkpoints(table).map_err(|error| {
            let mut error = LinuxDirectError::new("run_failed", "run", error);
            error.retired = Some(0);
            error
        })?;
    }
    // Install the APIC sources only after clock and checkpoint validation.
    if members.cpuid_profile == CpuidProfile::X8664V1Stage4AApic {
        install_linux_ioapic_sources(&mut boot.board)?;
    }
    // The C7 boundary exists for the made-up DiagnosticV1 identity only (S4-38L9a): a subset
    // profile answers leaves 1 and 0x8000_0001 for what it declares.
    let cpuid_boundary =
        members.cpuid && !members.diagnostic_past_c7 && cpuid == CpuidProfile::DiagnosticV1;
    let schedule_standalone = user_mode
        && cpuid == CpuidProfile::X8664V1Stage4A
        && members.irq
        && !boot.board.clock_owner_enabled()
        && boot.board.vcpu_count() == 1
        && boot.board.pit_pic_source().is_some();
    let result = if capture_execution_observations || schedule_standalone {
        boot.board.run_linux_boot_cpl0_set_with_post_iret_index(
            instruction_budget,
            &set,
            cpuid_boundary,
            members.fetch_cache,
            members.irq,
            capture_execution_observations,
            selected_post_iret_ordinal,
            selected_rdtsc_sample_rip,
            selected_rdtsc_sample_stride,
            selected_rdtsc_entry_sample_rip,
            selected_rdtsc_sample_window,
            selected_rdtsc_prefix_sentinel_rip,
            selected_instruction_path_trace,
            selected_elf_stack_copy,
            selected_delay_tsc_call_summary,
            schedule_standalone,
        )
    } else {
        boot.board.run_linux_boot_cpl0_set(
            instruction_budget,
            &set,
            cpuid_boundary,
            members.fetch_cache,
            members.irq,
        )
    };
    cpu_cells.commit(
        boot.board
            .vcpu_mut(BOOT_VCPU_INDEX)
            .expect("LinuxDirect board has a boot vCPU"),
    );
    boot.board.set_pci_config_address(pci_cell.get());
    let checkpoints = boot.board.take_checkpoints();
    let report = result.map_err(|failure| {
        let mut error = LinuxDirectError::new("run_failed", "run", failure.error);
        error.retired = Some(failure.retired);
        error
    })?;
    Ok(stop_record(
        boot,
        input,
        report.run,
        pm_timer_reads_before,
        SetExtras {
            checkpoints,
            path: "cpl0_set",
            members: names,
            exit_detail: Some(report.set_exit_detail),
            changes: &report.translation_context_changes,
            cpuid_divergence_leaf: report.cpuid_divergence_leaf,
            user_entry: report.user_entry,
            user_mode,
            cpl3_retired: report.cpl3_retired,
            syscall_journal: report.syscall_journal,
            exceptions_delivered: report.exceptions_delivered,
            page_faults_delivered: report.page_faults_delivered,
            general_protection_faults_delivered: report.general_protection_faults_delivered,
            execution_observations: report.execution_observations,
            irq: report.irq,
            diagnostic_past_c7: members.diagnostic_past_c7,
        },
    ))
}

/// The checkpoint slot of the marked RIP (an address slot of the Linux table, 1..=17).
const MARKER_CHECKPOINT_ID: u8 = 1;

/// What [`run_firmware_unified`] reports besides the usual run report.
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedRun {
    /// The run report; `exit` is the set's exit in `PagedProbeExit` terms.
    pub run: FirmwareRunReport,
    /// The `Debug` text of the set's own final exit.
    pub set_exit_detail: String,
    /// The members of the instruction set the run used, in binding order.
    pub members: Vec<String>,
    /// `#PF`/`#GP(0)` exceptions delivered to the guest through its IDT (not retired instructions).
    pub exceptions_delivered: u64,
    /// Successfully delivered guest `#PF` exceptions.
    pub page_faults_delivered: u64,
    /// Retired `MOV CR3` instructions.
    pub cr3_changes: u64,
    /// Instructions retired at CPL3 (part of `run.retired`).
    pub cpl3_retired: u64,
    /// The leaf of the `CPUID` the run stopped before, if it did.
    pub cpuid_divergence_leaf: Option<u32>,
    /// The guest-physical address of the final RIP, when it translates (read-only walk with the
    /// board CR3; no accessed or dirty bit is set).
    pub rip_physical: Option<u64>,
    /// Up to 32 bytes at the final RIP as a fetch would see them (they stop at the first byte
    /// that does not translate or is not RAM).
    pub rip_bytes: Vec<u8>,
    /// Why the bytes at the final RIP could not be read in full (the text of the first failed
    /// translation of `fetch_view_2m`), or `None` when all 32 were read (S7-31e).
    pub rip_fetch_error: Option<String>,
    /// The raw paging-structure entries of the final RIP from the live CR3 (S7-31e).
    pub rip_walk: Vec<FirmwareUnifiedWalkEntry>,
    /// CR2 and the error code of the first `#PF` delivered to the guest, recorded only with a
    /// marker (S7-31e).
    pub first_page_fault: Option<FirmwareUnifiedPageFault>,
    /// The CPU state the run stopped in (S7-31g, diagnostics).
    pub final_state: FirmwareUnifiedState,
    /// The interrupt state the run stopped in (S7-31m).
    pub interrupts: FirmwareUnifiedInterrupts,
    /// The RIP that was asked to be marked (S7-31d), if any.
    pub marker_rip: Option<u64>,
    /// The first boundary at which RIP was `marker_rip`: `None` when none was asked for or the
    /// run never got there.
    pub marker: Option<FirmwareUnifiedMarker>,
    /// Every event the checkpoint recorder noted (S7-31d, only with a marker): its own loop
    /// events (entry, `LIDT`, `MOV CR3`, a delivered `#PF`, `CPUID` leaf 0) and the marker.
    pub checkpoints: Vec<FirmwareUnifiedCheckpoint>,
}

/// The CPU state a unified run stopped in (S7-31g, diagnostics): what decides whether the set
/// admits the next instruction (`MOV SS` needs a clear interrupt shadow, TF = 0 and
/// modeled SS state; since S7-31g IF may be 0 or 1).
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedState {
    pub rflags: u64,
    pub rax: u64,
    pub rsp: u64,
    /// GDTR as (base, limit).
    pub gdtr: Option<(u64, u16)>,
    pub cs_selector: Option<u16>,
    pub ss_selector: Option<u16>,
    /// `Debug` text of the SS hidden cache.
    pub ss_cache: Option<String>,
    /// `Debug` text of the interrupt shadow.
    pub sti_shadow: Option<String>,
}

/// The interrupt state a unified run stopped in (S7-31m, ISSUE-640): whether the guest can still
/// be woken at the stop, and what the board did with interrupts during the run. Read after the
/// run; it only observes.
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedInterrupts {
    /// RFLAGS.IF at the stop.
    pub interrupt_flag: bool,
    /// `Debug` text of the vCPU activity state (`Running`, `Halted`, ...).
    pub activity: Option<String>,
    /// The board interrupt source of the run: `none` (no source, the run delivers nothing) or
    /// `pit_pic` (PIT channel 0 -> 8259 master IR0, UART THRE -> IR4).
    pub source: &'static str,
    /// 8259 master: interrupt mask, request and in-service registers; `None` without the Linux
    /// profile. The vector base is the accepted ICW2.
    pub pic_imr: Option<u8>,
    pub pic_irr: Option<u8>,
    pub pic_isr: Option<u8>,
    pub pic_icw2: Option<u8>,
    /// `Debug` text of the whole 8259 master snapshot.
    pub pic: Option<String>,
    /// `Debug` text of the PIT channel 0 snapshot (mode, access, reload, loaded-by-reset).
    pub pit_channel0: Option<String>,
    /// Whether a control word programmed PIT channel 0 since reset (the unified run starts from reset).
    pub pit_channel0_programmed: Option<bool>,
    /// `CPUID.1:EDX.APIC` as the run's CPUID profile declares it: without it the guest has no
    /// Local APIC timer to use.
    pub apic_declared: bool,
    /// Why a halted CPU was not resumed (SDM: `HLT` waits for an interrupt, NMI, SMI or reset), or
    /// `None` when the CPU is not halted: `if_clear` (IF = 0: no maskable interrupt can wake it;
    /// the wait is forever), `no_wake_time` (IF = 1, but the source reports no edge to wait for:
    /// its line is masked or in service, or the timer has no further edge), or
    /// `run_ended_with_a_wake_pending` (IF = 1 and the source has an edge to wait for: the run
    /// ended for another reason, the budget or a stop request).
    pub halt_stop: Option<&'static str>,
    /// External interrupts delivered through the guest IDT.
    pub irqs_delivered: u64,
    /// IRQ 0 edges the source reported.
    pub irq0_edges: u64,
    /// Boundaries at which a pending vector was held back by IF = 0.
    pub irq_refused_masked: u64,
    /// The same because of the one-instruction STI shadow.
    pub irq_refused_shadow: u64,
    /// Times a halted CPU was woken from the source's wake time.
    pub halt_wakes: u64,
    /// Virtual nanoseconds the clock skipped while the CPU was halted.
    pub halt_idle_ns: u64,
}

/// One raw paging-structure entry of the walk of the final RIP (S7-31e).
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedWalkEntry {
    pub level: &'static str,
    /// Guest-physical address of the entry.
    pub entry_gpa: u64,
    pub raw: u64,
}

/// CR2 and the error code of a delivered `#PF` (S7-31e).
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedPageFault {
    pub cr2: u64,
    pub error_code: u32,
    /// The raw paging-structure entries of `cr2` from the live CR3 at the end of the run.
    pub walk: Vec<FirmwareUnifiedWalkEntry>,
    /// The text of the failed translation of the 2 MiB read-only walk at `cr2`, or `None` if it
    /// translates to RAM.
    pub translation_error: Option<String>,
}

/// One checkpoint record of a unified run (S7-31d).
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedCheckpoint {
    pub id: u8,
    pub name: String,
    pub kind: &'static str,
    pub rip: u64,
    pub retired: u64,
    pub quanta: u64,
    pub virtual_ns: u64,
    pub cpl: u8,
}

/// The first instruction boundary of a run at the marked RIP (S7-31d).
#[derive(Debug, Eq, PartialEq)]
pub struct FirmwareUnifiedMarker {
    /// Instructions retired in the run before the instruction at the marked RIP.
    pub retired: u64,
    /// Quanta of the run up to and including the one the boundary is in.
    pub quanta: u64,
    /// Virtual time at the boundary, ns.
    pub virtual_ns: u64,
}

/// S7-31c (ADR-0013 variant (b)): run a firmware board with the Linux profile through the CPL0
/// profile set, one loop for the ROM and for the guest it starts (the EFI stub, then the kernel).
///
/// The board is a firmware board (loaded from a ROM image) built with `linux_profile`. The
/// firmware device apertures (NVRAM, display registers, the firmware NVMe window, TPM CRB) are
/// bound to the machine bus for the set loop, so the ROM's Runtime Services and boot services
/// reach the same devices as in `run_boot`. The instruction set is the Linux set of the stage 4
/// runs on the `X8664V1Stage4AExt2` CPUID subset (Stage4A plus SYSCALL, NX and LM in leaf
/// 0x80000001, plus SSE and SSE2; FPU and FXSR members included: the ROM uses
/// `FXSAVE`/`FXRSTOR`) with the CPL3 entry, so the kernel can reach user space. Nothing is
/// executed that the set does not admit: the first inadmissible instruction is the stop.
///
/// Not a replacement of `run_boot` (the firmware profile loop, which stays as it was) nor of
/// `run_linux_direct_board_set` (Linux without firmware).
pub fn run_firmware_unified(
    board: &mut LoadedFirmwareBoard,
    instruction_budget: u64,
) -> Result<FirmwareUnifiedRun, crate::FirmwareBoardError> {
    run_firmware_unified_marked(board, instruction_budget, None)
}

/// [`run_firmware_unified`] with one marked RIP (S7-31d): the run records the first boundary at
/// which RIP equals `marker_rip` (an address in the ROM, for example the entry of
/// `exit_boot_services`), through the existing checkpoint recorder of the set loop. It only
/// observes: the quanta, the clock and the result are those of a run without a marker. The
/// recorder also notes its own loop events (entry, `LIDT`, `MOV CR3`, a delivered `#PF`, `CPUID`
/// leaf 0); only the marked RIP is reported.
pub fn run_firmware_unified_marked(
    board: &mut LoadedFirmwareBoard,
    instruction_budget: u64,
    marker_rip: Option<u64>,
) -> Result<FirmwareUnifiedRun, crate::FirmwareBoardError> {
    if !board.has_firmware_entry() || !board.has_linux_profile() {
        return Err(crate::FirmwareBoardError::Config(
            "run_firmware_unified needs a firmware board with the Linux profile",
        ));
    }
    if board.vcpu_count() != 1 {
        return Err(crate::FirmwareBoardError::Config(
            "run_firmware_unified runs one vCPU",
        ));
    }
    if !board.pit_pic_irq_available() {
        return Err(crate::FirmwareBoardError::Config(
            "run_firmware_unified needs the Linux-profile PIT and 8259",
        ));
    }
    // The entry state of a ROM models no descriptors; see `model_unified_entry_descriptors`.
    crate::firmware_board::model_unified_entry_descriptors(board.state_mut());

    // `X8664V1Stage4AExt2` (S7-31i): Stage4A plus SYSCALL, NX and LM in CPUID.80000001H:EDX (S7-31h:
    // without NX the kernel strips `_PAGE_NX` and its EFI runtime mapping fails, ISSUE-634) plus SSE
    // and SSE2 in leaf 1 (the kernel's `LDMXCSR` in `kernel_fpu_begin_mask`, ISSUE-636).
    let members = LinuxCpl0SetMembers {
        cpuid_profile: CpuidProfile::X8664V1Stage4AExt2,
        msr_tdx_keyid_partitioning_read: true,
        fp_state: true,
        int3: true,
        halt: true,
        ..LinuxCpl0SetMembers::linux_default()
    };
    let profile = LinuxCpl0Profile::new(members, true, board.efer());
    let cpu_cells = LinuxVcpuCells::from_vcpu(
        board
            .vcpu(BOOT_VCPU_INDEX)
            .expect("a firmware board has a boot vCPU"),
    );
    let pci_cell = Cell::new(board.pci_config_address());
    let mut names = Vec::new();
    let set = profile.bind(&cpu_cells, &pci_cell, |member| {
        names.push(member.to_string())
    });
    board.serve_firmware_set_windows();
    if let Some(rip) = marker_rip {
        board
            .enable_checkpoints(crate::linux_checkpoints::LinuxCheckpointTable {
                targets: vec![crate::linux_checkpoints::LinuxCheckpointTarget {
                    id: MARKER_CHECKPOINT_ID,
                    name: "marker".to_owned(),
                    va: Some(rip),
                    pa: None,
                    cpl3: false,
                    after: None,
                }],
            })
            .map_err(|_| crate::FirmwareBoardError::Config("run_firmware_unified: bad marker"))?;
    }
    // S7-31m (ISSUE-640): the kernel programs the PIT (channel 0, mode 2) and unmasks IR0 of the
    // 8259, then idles in `sti; hlt` until the timer interrupt (the S7-31m measurement). The
    // board source is the one of the user-mode Linux run (S4-39g, S4-39i): PIT channel 0 -> IR0
    // and the UART THRE line -> IR4 through the 8259. With `halt` and the interrupt hook a
    // `HLT` at IF = 1 is woken at the next timer edge; `HLT` at IF = 0 ends the run as halted.
    // `pit_pic_irq_available` was checked with the refusals at the top, so both installs succeed.
    assert!(
        board.enable_pit_pic_irq() && board.enable_uart_irq4(),
        "the PIT -> 8259 source was checked available"
    );
    let result =
        board.run_linux_boot_cpl0_set(instruction_budget, &set, false, members.fetch_cache, true);
    cpu_cells.commit(
        board
            .vcpu_mut(BOOT_VCPU_INDEX)
            .expect("a firmware board has a boot vCPU"),
    );
    board.set_pci_config_address(pci_cell.get());
    let first_page_fault = board.first_delivered_page_fault().map(|(cr2, error_code)| {
        let walk = board
            .raw_page_walk(cr2)
            .into_iter()
            .map(|(level, entry_gpa, raw)| FirmwareUnifiedWalkEntry {
                level,
                entry_gpa,
                raw,
            })
            .collect();
        let (_, _, translation_error) = board.fetch_view_2m(cr2, 1);
        FirmwareUnifiedPageFault {
            cr2,
            error_code,
            walk,
            translation_error,
        }
    });
    let records = board.take_checkpoints().unwrap_or_default();
    let marker = records
        .iter()
        .find(|record| record.id == MARKER_CHECKPOINT_ID)
        .map(|record| FirmwareUnifiedMarker {
            retired: record.retired,
            quanta: record.quanta,
            virtual_ns: record.virtual_ns,
        });
    let checkpoints = records
        .into_iter()
        .map(|record| FirmwareUnifiedCheckpoint {
            id: record.id,
            name: record.name,
            kind: record.kind,
            rip: record.rip,
            retired: record.retired,
            quanta: record.quanta,
            virtual_ns: record.virtual_ns,
            cpl: record.cpl,
        })
        .collect();
    let report = result.map_err(|failure| failure.error)?;
    let rip = board.state().rip;
    let final_state = {
        let state = board.state();
        FirmwareUnifiedState {
            rflags: state.rflags,
            rax: state.gpr[Register::Rax as usize],
            rsp: state.gpr[Register::Rsp as usize],
            gdtr: state.gdtr.map(|gdtr| (gdtr.base, gdtr.limit)),
            cs_selector: state.cs_selector,
            ss_selector: state.ss_selector,
            ss_cache: state.ss_cache.map(|cache| format!("{cache:?}")),
            sti_shadow: state.sti_shadow.map(|shadow| format!("{shadow:?}")),
        }
    };
    let interrupts = {
        // Asked first: `irq_wake_ns` brings the sources up to the clock, so the snapshots below
        // are taken after it.
        let halt_stop = (board.state().activity == Some(od_isa::CpuActivity::Halted)).then(|| {
            if final_state.rflags & (1 << 9) == 0 {
                "if_clear"
            } else if board.irq_wake_ns().is_none() {
                "no_wake_time"
            } else {
                "run_ended_with_a_wake_pending"
            }
        });
        let pic = board.pic_master().map(|pic| pic.snapshot());
        let pit_channel0 = board.pit_channel0_snapshot();
        FirmwareUnifiedInterrupts {
            interrupt_flag: final_state.rflags & (1 << 9) != 0,
            activity: board
                .state()
                .activity
                .map(|activity| format!("{activity:?}")),
            source: if board.pit_pic_source().is_some() {
                "pit_pic"
            } else {
                "none"
            },
            pic_imr: pic.map(|pic| pic.imr),
            pic_irr: pic.map(|pic| pic.irr),
            pic_isr: pic.map(|pic| pic.isr),
            pic_icw2: pic.and_then(|pic| pic.icw2),
            pic: pic.map(|pic| format!("{pic:?}")),
            pit_channel0: pit_channel0.map(|channel| format!("{channel:?}")),
            pit_channel0_programmed: pit_channel0.map(|channel| !channel.loaded_by_reset),
            halt_stop,
            apic_declared: members.cpuid_profile.query(1, 0).edx & (1 << 9) != 0,
            irqs_delivered: report.irq.irqs_delivered,
            irq0_edges: report.irq.irq0_edges,
            irq_refused_masked: report.irq.irq_refused_masked,
            irq_refused_shadow: report.irq.irq_refused_shadow,
            halt_wakes: report.irq.halt_wakes,
            halt_idle_ns: report.irq.halt_idle_ns,
        }
    };
    let (rip_physical, rip_bytes, rip_fetch_error) = board.fetch_view_2m(rip, 32);
    let rip_walk = board
        .raw_page_walk(rip)
        .into_iter()
        .map(|(level, entry_gpa, raw)| FirmwareUnifiedWalkEntry {
            level,
            entry_gpa,
            raw,
        })
        .collect();
    Ok(FirmwareUnifiedRun {
        run: report.run,
        set_exit_detail: report.set_exit_detail,
        members: names,
        exceptions_delivered: report.exceptions_delivered,
        page_faults_delivered: report.page_faults_delivered,
        cr3_changes: report.translation_context_changes.len() as u64,
        cpl3_retired: report.cpl3_retired,
        cpuid_divergence_leaf: report.cpuid_divergence_leaf,
        rip_physical,
        rip_bytes,
        rip_fetch_error,
        rip_walk,
        first_page_fault,
        final_state,
        interrupts,
        marker_rip,
        marker,
        checkpoints,
    })
}

/// Reads the E820 table out of a `boot_params` page. The count byte comes from guest memory
/// (u8, up to 255) and the table holds at most 128 entries, so a larger count is a corrupted
/// page, not something to clamp: an honest loader never writes more than
/// `E820_MAX_ENTRIES_ZEROPAGE` (or none).
fn read_e820(page: &[u8]) -> Result<Vec<LinuxDirectE820>, LinuxDirectError> {
    let count = usize::from(page[OFF_E820_ENTRIES]);
    if count == 0 || count > LINUX_E820_MAX_ENTRIES {
        return Err(LinuxDirectError::text(
            "e820_invalid",
            "e820",
            format!("boot_params.e820_entries is {count}, expected 1..={LINUX_E820_MAX_ENTRIES}"),
        ));
    }
    Ok((0..count)
        .map(|index| {
            let at = OFF_E820_TABLE + index * E820_ENTRY_BYTES;
            LinuxDirectE820 {
                addr: le64(page, at),
                size: le64(page, at + 8),
                kind: le32(page, at + 16),
            }
        })
        .collect())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn zone_name(zone: crate::linux_placement::LinuxZone) -> &'static str {
    use crate::linux_placement::LinuxZone as Z;
    match zone {
        Z::Kernel => "kernel",
        Z::Initrd => "initrd",
        Z::BootParams => "boot_params",
        Z::Cmdline => "cmdline",
        Z::Gdt => "gdt",
        Z::PageTables => "page_tables",
        Z::Stack => "stack",
        Z::AcpiTables => "acpi_tables",
        Z::AcpiFacs => "acpi_facs",
    }
}

fn read(
    board: &mut LoadedFirmwareBoard,
    address: u64,
    output: &mut [u8],
) -> Result<(), LinuxDirectError> {
    board
        .bus_mut()
        .read_flat_bytes(GuestPhysicalAddress::new(address), output)
        .map_err(|error| LinuxDirectError::new("guest_memory_access", "report", (address, error)))
}

fn le32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn le64(bytes: &[u8], at: usize) -> u64 {
    let mut raw = [0u8; 8];
    raw.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_placement::{LinuxRange, LinuxZone};
    use crate::validate_acpi_blob;

    fn code(error: LinuxLoadError) -> (&'static str, &'static str) {
        let error = load_error(error);
        (error.code, error.stage)
    }

    #[test]
    fn load_errors_have_stable_codes() {
        let range = LinuxRange { start: 0, end: 1 };
        assert_eq!(
            code(LinuxLoadError::Header(LinuxHeaderError::SetupOutOfFile {
                protected_mode_offset: 0x3C00,
                file_len: 0x300,
            })),
            ("bzimage_truncated", "bzimage_header")
        );
        assert_eq!(
            code(LinuxLoadError::Header(LinuxHeaderError::PayloadOutOfFile {
                payload_offset: 0,
                payload_length: 1,
                limit: 0,
            })),
            ("bzimage_payload_out_of_file", "bzimage_header")
        );
        assert_eq!(
            code(LinuxLoadError::Header(LinuxHeaderError::BadBootFlag {
                found: 0
            })),
            ("bzimage_header_invalid", "bzimage_header")
        );
        assert_eq!(
            code(LinuxLoadError::Placement(
                LinuxPlacementError::InitrdTooLarge { len: 2, limit: 1 }
            )),
            ("initrd_too_large", "placement")
        );
        assert_eq!(
            code(LinuxLoadError::Placement(
                LinuxPlacementError::NoRoomForInitrd {
                    len: 2,
                    kernel_end: 1,
                    limit: 1,
                }
            )),
            ("initrd_too_large", "placement")
        );
        assert_eq!(
            code(LinuxLoadError::Placement(
                LinuxPlacementError::CmdlineTooLong { len: 2, max: 1 }
            )),
            ("cmdline_too_long", "cmdline")
        );
        assert_eq!(
            code(LinuxLoadError::CmdlineContainsNul { at: 0 }),
            ("cmdline_invalid", "cmdline")
        );
        // An E820 map without the ECAM window (the L3 validator) is an e820 error ...
        assert_eq!(
            code(LinuxLoadError::BootParams(
                LinuxBootParamsError::EcamNotReserved { ecam: range }
            )),
            ("e820_invalid", "e820")
        );
        // ... and a disagreement with the board handoff has its own code.
        assert_eq!(
            code(LinuxLoadError::Handoff(LinuxBootParamsError::HandoffNoEcam)),
            ("e820_handoff_mismatch", "handoff")
        );
        assert_eq!(
            code(LinuxLoadError::Write {
                zone: LinuxZone::Kernel,
                error: crate::BusAccessError::AddressOverflow,
            }),
            ("guest_memory_access", "load")
        );
        assert_eq!(
            code(LinuxLoadError::AcpiBlobMismatch),
            ("acpi_blob_invalid", "acpi_blob")
        );
        assert_eq!(
            code(LinuxLoadError::EntryEfer { found: 0 }),
            ("entry_state_invalid", "entry_state")
        );
        assert_eq!(
            code(LinuxLoadError::EntryRsi {
                expected: 1,
                found: 2,
            }),
            ("entry_state_invalid", "entry_state")
        );
    }

    const BLOB: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
    const SMP_BLOB: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage6/smp-init/acpi-2-vcpus.bin");
    const FOUR_CPU_BLOB: &[u8] =
        include_bytes!("../../../tests/fixtures/guest/stage6/smp-init/acpi-4-vcpus.bin");
    const NVME_SMP_BLOBS: [(usize, &[u8]); 2] = [
        (
            2,
            include_bytes!("../../../tests/fixtures/guest/stage6/smp-init/acpi-nvme-2-vcpus.bin"),
        ),
        (
            4,
            include_bytes!("../../../tests/fixtures/guest/stage6/smp-init/acpi-nvme-4-vcpus.bin"),
        ),
    ];
    /// `tables_config_with_ecam().acpi_manifest_hash()`, the hash of the fixture blob (S4-31d/g).
    const FIXTURE_HASH: &str = "2bd690ae0817705b1b128675f9628dd7212fee74ab97c88da3599a1ae8e398f2";
    const MIB: u64 = 1 << 20;
    const PM_OFFSET: usize = 3 * 512;
    const PM_CODE: usize = 0x3000;

    fn put16(b: &mut [u8], at: usize, v: u16) {
        b[at..at + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], at: usize, v: u64) {
        b[at..at + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// A small valid bzImage (pref 16 MiB, 2 MiB alignment, `init_size` 1 MiB).
    fn bzimage() -> Vec<u8> {
        let mut b = vec![0u8; PM_OFFSET + PM_CODE];
        for (i, byte) in b[PM_OFFSET..].iter_mut().enumerate() {
            *byte = (i % 251) as u8 ^ 0x5A;
        }
        b[0x1F1] = 2;
        b[0x201] = 0x6A;
        put32(&mut b, 0x1F4, (PM_CODE / 16) as u32);
        put16(&mut b, 0x1FE, 0xAA55);
        b[0x202..0x206].copy_from_slice(b"HdrS");
        put16(&mut b, 0x206, 0x020F);
        b[0x211] = 1;
        put32(&mut b, 0x22C, 0x7FFF_FFFF);
        put32(&mut b, 0x230, 0x0020_0000);
        b[0x234] = 1;
        put16(&mut b, 0x236, 0x7F);
        put32(&mut b, 0x238, 0x7FF);
        put32(&mut b, 0x248, 0x1000);
        put32(&mut b, 0x24C, 0x2000);
        put64(&mut b, 0x258, 0x0100_0000);
        put32(&mut b, 0x260, 0x0010_0000);
        b
    }

    fn board_hash() -> AcpiManifestHash {
        FirmwareBoardConfig {
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            ..FirmwareBoardConfig::default()
        }
        .acpi_manifest_hash()
        .unwrap()
    }

    fn input<'a>(
        file: &'a [u8],
        initrd: &'a [u8],
        blob: &'a [u8],
        hash: AcpiManifestHash,
    ) -> LinuxDirectInput<'a> {
        LinuxDirectInput {
            bzimage: file,
            initrd,
            cmdline: b"console=ttyS0 nokaslr",
            acpi_blob: blob,
            expected_acpi_manifest_hash: hash,
            ram_bytes: 128 * MIB,
        }
    }

    fn refusal(result: Result<LinuxDirectBoot, LinuxDirectError>) -> (&'static str, &'static str) {
        match result {
            Err(error) => (error.code, error.stage),
            Ok(_) => panic!("expected a refusal"),
        }
    }

    #[test]
    fn the_fixture_blob_belongs_to_the_board_the_facade_assembles() {
        // The fixture hash is the manifest hash of {pci_ecam, acpi_pm, acpi_tables}; the facade
        // builds that board (plus `linux_profile`, which does not change the hash).
        assert_eq!(hex(&board_hash()), FIXTURE_HASH);
        assert!(validate_acpi_blob(BLOB, &board_hash()).is_ok());
    }

    #[test]
    fn linux_direct_loads_the_board_topology_only_with_matching_madt() {
        use crate::{AccessWidth, IO_APIC_MMIO_BASE};

        let file = bzimage();
        let manifest = board_hash();
        for (count, blob) in [(1, BLOB), (2, SMP_BLOB), (4, FOUR_CPU_BLOB)] {
            let input = input(&file, &[], blob, manifest);
            let mut boot = load_linux_direct_board_with_vcpus(&input, count).unwrap();
            assert_eq!(boot.board.vcpu_count(), count);
            assert_eq!(boot.retired, 0);

            let acpi = validate_acpi_blob(blob, &manifest).unwrap();
            let descriptor = acpi
                .tables
                .iter()
                .find(|table| table.signature == *b"APIC")
                .unwrap();
            let mut madt = vec![0; descriptor.length as usize];
            read(
                &mut boot.board,
                acpi.table_address(b"APIC").unwrap(),
                &mut madt,
            )
            .unwrap();
            assert_eq!(madt, acpi.table_bytes(blob, descriptor));
            let mut entries = &madt[44..];
            let mut lapic_ids = Vec::new();
            let mut ioapic_id = None;
            while !entries.is_empty() {
                let (entry, rest) = entries.split_at(usize::from(entries[1]));
                match entry[0] {
                    0 => lapic_ids.push(entry[3]),
                    1 => ioapic_id = Some(entry[2]),
                    _ => {}
                }
                entries = rest;
            }
            assert_eq!(lapic_ids, (0..count as u8).collect::<Vec<_>>());
            let bus = boot.board.bus_mut();
            bus.write_physical(
                GuestPhysicalAddress::new(IO_APIC_MMIO_BASE),
                AccessWidth::Dword,
                0,
                |_| {},
            )
            .unwrap();
            let id = bus
                .read_physical(
                    GuestPhysicalAddress::new(IO_APIC_MMIO_BASE + 0x10),
                    AccessWidth::Dword,
                )
                .unwrap()
                >> 24;
            assert_eq!(u64::from(ioapic_id.unwrap()), id);
            assert_eq!(id, count as u64);
        }
        let up = input(&file, &[], BLOB, manifest);
        let smp = input(&file, &[], SMP_BLOB, manifest);
        assert_eq!(
            refusal(load_linux_direct_board_with_vcpus(&up, 2)),
            ("acpi_blob_invalid", "acpi_blob")
        );
        assert_eq!(
            refusal(load_linux_direct_board(&smp)),
            ("acpi_blob_invalid", "acpi_blob")
        );
    }

    #[test]
    fn a_hash_of_another_configuration_is_refused_before_the_blob_is_looked_at() {
        let file = bzimage();
        // The hash of a board with an NVMe controller: other devices, other manifest.
        let foreign = FirmwareBoardConfig {
            nvme: Some(crate::NvmeConfig::default()),
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            ..FirmwareBoardConfig::default()
        }
        .acpi_manifest_hash()
        .unwrap();
        assert_ne!(foreign, board_hash());
        // The (correct) fixture blob does not rescue a foreign hash ...
        assert_eq!(
            refusal(load_linux_direct_board(&input(&file, &[], BLOB, foreign))),
            ("acpi_manifest_mismatch", "acpi_blob")
        );
        // ... and a blob that is not a blob at all is refused on the hash first, not as
        // `acpi_blob_invalid`: the hash check runs before `validate_acpi_blob`.
        let junk = vec![0u8; 0x400];
        assert_eq!(
            refusal(load_linux_direct_board(&input(&file, &[], &junk, foreign))),
            ("acpi_manifest_mismatch", "acpi_blob")
        );
        // With the board's own hash the same junk is an invalid blob.
        assert_eq!(
            refusal(load_linux_direct_board(&input(
                &file,
                &[],
                &junk,
                board_hash()
            ))),
            ("acpi_blob_invalid", "acpi_blob")
        );
        // A blob carrying the board's hash in a file whose operator hash differs by one bit.
        let mut off = board_hash();
        off[0] ^= 1;
        assert_eq!(
            refusal(load_linux_direct_board(&input(&file, &[], BLOB, off))),
            ("acpi_manifest_mismatch", "acpi_blob")
        );
    }

    #[test]
    fn the_report_reads_back_the_kernel_initrd_cmdline_and_rsdp_from_memory() {
        let file = bzimage();
        let initrd: Vec<u8> = (0..5000u32).map(|i| (i % 253) as u8).collect();
        let boot =
            load_linux_direct_board(&input(&file, &initrd, BLOB, board_hash())).expect("loads");
        assert_eq!(boot.retired, 0);
        assert_eq!(&boot.rsdp_signature_in_memory, b"RSD PTR ");
        assert_eq!(boot.acpi_rsdp_addr, 0xE_0000);
        assert_eq!(boot.cmdline_in_memory, b"console=ttyS0 nokaslr");
        assert_eq!(
            boot.kernel_sha256_in_memory,
            sha256(&file[PM_OFFSET..PM_OFFSET + PM_CODE])
        );
        assert_eq!(boot.initrd_sha256_in_memory, Some(sha256(&initrd)));
        // Without an initrd there is nothing to hash.
        let boot = load_linux_direct_board(&input(&file, &[], BLOB, board_hash())).expect("loads");
        assert_eq!(boot.initrd_sha256_in_memory, None);
    }

    #[test]
    fn a_corrupted_e820_count_is_refused_not_indexed() {
        let mut page = vec![0u8; BOOT_PARAMS_BYTES];
        // An honest page: 1 and 128 entries read fine.
        for count in [1u8, 9, 128] {
            page[OFF_E820_ENTRIES] = count;
            assert_eq!(read_e820(&page).unwrap().len(), usize::from(count));
        }
        // 0, 129 and 255 (169 and up would index past the 4096-byte page) are corrupted.
        for count in [0u8, 129, 169, 255] {
            page[OFF_E820_ENTRIES] = count;
            let error = read_e820(&page).unwrap_err();
            assert_eq!(
                (error.code, error.stage),
                ("e820_invalid", "e820"),
                "{count}"
            );
        }
    }

    #[test]
    fn acpi_errors_have_stable_codes() {
        let blob =
            include_bytes!("../../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
        let hash = FirmwareBoardConfig {
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            ..FirmwareBoardConfig::default()
        }
        .acpi_manifest_hash()
        .unwrap();
        assert!(validate_acpi_blob(blob, &hash).is_ok());
        let mut foreign = hash;
        foreign[31] ^= 0x80;
        let error = acpi_error(validate_acpi_blob(blob, &foreign).unwrap_err());
        assert_eq!(
            (error.code, error.stage),
            ("acpi_manifest_mismatch", "acpi_blob")
        );
        let error = acpi_error(validate_acpi_blob(&blob[..0x40], &hash).unwrap_err());
        assert_eq!(
            (error.code, error.stage),
            ("acpi_blob_invalid", "acpi_blob")
        );
    }

    // ---- S4-38L1: the bounded run and its stop record ----

    /// `bzimage()` with `code` at the 64-bit entry (`startup_64`, offset 0x200 of the kernel).
    fn bzimage_with_entry(code: &[u8]) -> Vec<u8> {
        let mut file = bzimage();
        let at = PM_OFFSET + 0x200;
        file[at..at + code.len()].copy_from_slice(code);
        file
    }

    /// `MOV AL, imm8; MOV DX, 0x03F8; OUT DX, AL; HLT`.
    fn com1_program(byte: u8) -> Vec<u8> {
        vec![0xb0, byte, 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4]
    }

    fn boot_with_entry(code: &[u8]) -> (LinuxDirectBoot, Vec<u8>) {
        let file = bzimage_with_entry(code);
        let boot = load_linux_direct_board(&input(&file, &[], BLOB, board_hash())).expect("loads");
        (boot, file)
    }

    #[test]
    fn interleaved_linux_uses_the_ap_profile_and_counts_an_ap_budget_stop() {
        // CPUID.1, RDMSR/WRMSR of IA32_APIC_BASE, CLD, then legacy PUSH ES.
        // Isolate the long-mode leg after INIT/SIPI.
        let file = bzimage_with_entry(&[
            0xb8, 1, 0, 0, 0, 0x0f, 0xa2, 0xb9, 0x1b, 0, 0, 0, 0x0f, 0x32, 0x0f, 0x30, 0xfc, 0x06,
        ]);
        let input = input(&file, &[], SMP_BLOB, board_hash());
        let mut boot = load_linux_direct_board_with_vcpus(&input, 2).unwrap();
        start_test_ap(&mut boot);
        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let report =
            run_linux_direct_board_set_user_mode_interleaved(&mut boot, &input, 6, members, 24301)
                .unwrap();
        assert_eq!(report.stop.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(report.stop.retired, 6);
        assert_eq!(report.terminal_vcpu, None);
        assert_eq!(report.vcpus[0].retired, 0);
        assert_eq!(report.vcpus[1].retired, 6);
        assert_eq!(report.vcpus[1].gpr[Register::Rbx as usize] >> 16, 0x0102);
        assert_eq!(report.vcpus[1].gpr[Register::Rax as usize], 0xfee0_0800);
        let stopped =
            run_linux_direct_board_set_user_mode_interleaved(&mut boot, &input, 1, members, 24301)
                .unwrap();
        assert_eq!(stopped.terminal_vcpu, Some(1));
        assert_eq!(
            stopped.stop.exit,
            PagedProbeExit::Unsupported { opcode: 0x06 }
        );
        assert_eq!(stopped.stop.rip, report.vcpus[1].rip);
        assert_eq!(stopped.stop.rip_bytes.first(), Some(&0x06));
    }

    // Prepare just the long-mode leg. The independent SMP-probe suite runs the real trampoline.
    fn start_test_ap(boot: &mut LinuxDirectBoot) {
        let mut state = boot.board.state().clone();
        state.cr3 = Some(boot.board.cr3());
        state.activity = Some(od_isa::CpuActivity::Running);
        let efer = boot.board.guest_efer();
        let ap = boot.board.vcpu_mut(1).unwrap();
        ap.state = state;
        ap.guest_efer = efer;
        ap.legacy = None;
        boot.board.state_mut().activity = Some(od_isa::CpuActivity::Halted);
    }

    #[test]
    fn com1_observer_receives_guest_bytes_in_order_at_the_budget_boundary() {
        use std::sync::{Arc, Mutex};

        // MOV EDX,3f8h; MOV AL,'A'; OUT DX,AL; MOV AL,'B'; OUT DX,AL.
        let file = bzimage_with_entry(&[0xba, 0xf8, 3, 0, 0, 0xb0, b'A', 0xee, 0xb0, b'B', 0xee]);
        let input = input(&file, &[], SMP_BLOB, board_hash());
        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
            ..LinuxCpl0SetMembers::linux_default()
        };
        for parallel in [false, true] {
            let mut boot = load_linux_direct_board_with_vcpus(&input, 2).unwrap();
            start_test_ap(&mut boot);
            let observed = Arc::new(Mutex::new(Vec::new()));
            let capture = Arc::clone(&observed);
            boot.board.set_com1_observer(move |byte| {
                capture.lock().unwrap().push(byte);
            });
            let stop = if parallel {
                run_linux_direct_board_set_user_mode_parallel(
                    &mut boot,
                    &input,
                    5,
                    members,
                    LinuxThreadedOptions {
                        quantum: 16,
                        timeout: std::time::Duration::from_secs(5),
                    },
                )
                .unwrap()
                .stop
            } else {
                run_linux_direct_board_set_user_mode_interleaved(
                    &mut boot, &input, 5, members, 24301,
                )
                .unwrap()
                .stop
            };
            assert_eq!(stop.exit, PagedProbeExit::BudgetExhausted);
            assert_eq!(stop.retired, 5);
            assert_eq!(stop.com1, b"AB");
            assert_eq!(observed.lock().unwrap().as_slice(), b"AB");
        }
    }

    #[test]
    fn disk_backed_linux_direct_checks_nvme_manifest_and_madt_then_runs_both_schedulers() {
        let file = bzimage_with_entry(&[0xf4]); // HLT
        let hash = FirmwareBoardConfig {
            nvme_pci: true,
            nvme: Some(NvmeConfig::default()),
            acpi_pm: true,
            acpi_tables: true,
            pci_ecam: true,
            linux_profile: true,
            ..FirmwareBoardConfig::default()
        }
        .acpi_manifest_hash()
        .unwrap();
        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
            ..LinuxCpl0SetMembers::linux_default()
        };
        for (count, blob) in NVME_SMP_BLOBS {
            let input = input(&file, &[], blob, hash);
            let mut boot =
                load_linux_direct_board_with_nvme_and_vcpus(&input, NvmeConfig::default(), count)
                    .unwrap();
            assert_eq!(boot.board.vcpu_count(), count);
            assert!(boot.board.nvme().unwrap().has_pci_transport());
            let incompatible_count = if count == 2 { 4 } else { 2 };
            assert_eq!(
                refusal(load_linux_direct_board_with_nvme_and_vcpus(
                    &input,
                    NvmeConfig::default(),
                    incompatible_count
                )),
                ("acpi_blob_invalid", "acpi_blob")
            );
            assert_eq!(
                refusal(load_linux_direct_board_with_vcpus(&input, count)),
                ("acpi_manifest_mismatch", "acpi_blob")
            );
            let report = run_linux_direct_board_set_user_mode_interleaved(
                &mut boot, &input, 1, members, 24301,
            )
            .unwrap();
            assert_eq!(report.stop.retired, 1);
            assert_eq!(report.vcpus.len(), count);

            let mut boot =
                load_linux_direct_board_with_nvme_and_vcpus(&input, NvmeConfig::default(), count)
                    .unwrap();
            let report = run_linux_direct_board_set_user_mode_parallel(
                &mut boot,
                &input,
                1,
                members,
                LinuxThreadedOptions {
                    quantum: 16,
                    timeout: std::time::Duration::from_secs(5),
                },
            )
            .unwrap();
            assert_eq!(report.stop.retired, 1);
            assert_eq!(
                (report.threads_started, report.threads_exited),
                (count, count)
            );
        }
    }

    #[test]
    fn parallel_linux_joins_all_workers_and_reports_the_actual_ap_terminal_cpu() {
        let file = bzimage_with_entry(&[0xfc, 0x06]); // CLD; legacy PUSH ES
        for (vcpus, blob) in [(2, SMP_BLOB), (4, FOUR_CPU_BLOB)] {
            let input = input(&file, &[], blob, board_hash());
            let mut boot = load_linux_direct_board_with_vcpus(&input, vcpus).unwrap();
            start_test_ap(&mut boot);
            let report = run_linux_direct_board_set_user_mode_parallel(
                &mut boot,
                &input,
                8,
                LinuxCpl0SetMembers {
                    cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
                    ..LinuxCpl0SetMembers::linux_default()
                },
                LinuxThreadedOptions {
                    quantum: 8,
                    timeout: std::time::Duration::from_secs(5),
                },
            )
            .unwrap();
            assert_eq!(
                (report.threads_started, report.threads_exited),
                (vcpus, vcpus)
            );
            assert_eq!(report.terminal_vcpu, Some(1));
            assert_eq!(report.stop.retired, 1);
            assert_eq!(report.stop.rip_bytes.first(), Some(&0x06));
            assert_eq!(report.stop.rip, report.vcpus[1].rip);
            assert_eq!(report.vcpus[0].retired, 0);
            assert_eq!(report.vcpus[1].retired, 1);
            for cpu in &report.vcpus[2..] {
                assert_eq!(cpu.mode, LinuxVcpuRunMode::Real);
                assert_eq!(cpu.activity, od_isa::CpuActivity::WaitForSipi);
                assert_eq!(cpu.rip, u64::from(crate::vcpu::init_cpu(0).mode.eip));
                assert_eq!(cpu.cr3, None);
            }
        }
    }

    fn run_entry(code: &[u8], budget: u64) -> LinuxStopRecord {
        let (mut boot, file) = boot_with_entry(code);
        run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), budget)
            .expect("runs")
    }

    fn page_tables(boot: &mut LinuxDirectBoot) -> Vec<u8> {
        let zone = boot
            .zones
            .iter()
            .find(|zone| zone.name == "page_tables")
            .expect("page table zone");
        let mut bytes = vec![0u8; (zone.end - zone.start) as usize];
        read(&mut boot.board, zone.start, &mut bytes).expect("read");
        bytes
    }

    #[test]
    fn budget_zero_stops_before_any_fetch_and_leaves_the_page_tables_alone() {
        let (mut boot, file) = boot_with_entry(&com1_program(b'X'));
        let tables_before = page_tables(&mut boot);
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 0)
            .expect("runs");
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(record.exit_kind, "budget_exhausted");
        assert_eq!((record.retired, record.quanta), (0, 1));
        assert_eq!(record.rip, 0x0100_0200);
        // The instruction window is shown, but through a read-only walk: no accessed bit moved.
        assert_eq!(record.rip_gpa, Some(0x0100_0200));
        assert_eq!(
            record.rip_bytes,
            file[PM_OFFSET + 0x200..PM_OFFSET + 0x200 + LINUX_STOP_RIP_BYTES]
        );
        assert_eq!(record.rip_bytes_error, None);
        assert_eq!(page_tables(&mut boot), tables_before);
        assert!(record.com1.is_empty());
        assert_eq!((record.mode, record.cpl), ("long64", 0));
        assert!(record.linux_profile);
        assert_eq!(record.cr3, boot.entry.cr3);
        assert_eq!(record.cr0, boot.entry.cr0);
        assert_eq!(record.efer, 0xd00);
    }

    #[test]
    fn the_synthetic_kernel_halts_and_writes_com1_through_the_new_facade() {
        let record = run_entry(&com1_program(b'X'), 64);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.com1, b"X");
        assert_eq!(record.retired, 4);
        assert_eq!(record.rip, 0x0100_0200 + 8);
        // Other bytes give other COM1 bytes: the run is not vacuous.
        assert_eq!(run_entry(&com1_program(0xA5), 64).com1, [0xA5]);
        // RSI is the boot_params pointer, untouched.
        assert_eq!(record.gpr[od_isa::Register::Rsi as usize], 0x10_0000);
    }

    #[test]
    fn hlt_as_the_first_kernel_byte_halts_after_one_instruction() {
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        let tables_before = page_tables(&mut boot);
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 64)
            .expect("runs");
        // The fetch of a real run does set accessed bits: the budget-0 comparison is not vacuous.
        assert_ne!(page_tables(&mut boot), tables_before);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 1);
        assert_eq!(record.rip, 0x0100_0201);
        assert!(record.com1.is_empty());
    }

    #[test]
    fn cld_retires_before_unsupported_cli() {
        // The first instruction of the real kernel (`startup_64`): CLD; CLI; XOR EAX,EAX.
        let record = run_entry(&[0xFC, 0xFA, 0x31, 0xC0], 64);
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xFA });
        assert_eq!(record.exit_kind, "unsupported");
        assert_eq!(record.unsupported_opcode, Some(0xFA));
        assert_eq!((record.retired, record.rip), (1, 0x0100_0201));
        assert_eq!(&record.rip_bytes[..3], &[0xFA, 0x31, 0xC0]);
        assert_eq!(record.rip_bytes.len(), LINUX_STOP_RIP_BYTES);
    }

    #[test]
    fn the_stop_record_hashes_its_inputs() {
        let initrd = [0xAB; 100];
        let file = bzimage_with_entry(&[0xF4]);
        let input = input(&file, &initrd, BLOB, board_hash());
        let mut boot = load_linux_direct_board(&input).expect("loads");
        let record = run_linux_direct_board(&mut boot, &input, 8).expect("runs");
        assert_eq!(record.inputs.bzimage_sha256, sha256(&file));
        assert_eq!(record.inputs.initrd_sha256, Some(sha256(&initrd)));
        assert_eq!(record.inputs.initrd_sha256_in_memory, Some(sha256(&initrd)));
        assert_eq!(record.inputs.acpi_blob_sha256, sha256(BLOB));
        assert_eq!(record.inputs.acpi_manifest_hash, board_hash());
        assert_eq!(
            record.inputs.kernel_pm_code_sha256_in_memory,
            sha256(&file[PM_OFFSET..PM_OFFSET + PM_CODE])
        );
    }

    #[test]
    fn stop_record_counts_pm_timer_reads_and_keeps_their_virtual_time_tick() {
        // MOV DX, 0x0608; IN AL, DX. Each successful PM_TMR read queues one microsecond
        // after the instruction quantum, so two retired instructions advance time by 1,002 ns.
        let code = [0x66, 0xba, 0x08, 0x06, 0xec];
        let (mut boot, file) = boot_with_entry(&code);
        let first = run_set_facade(&mut boot, &file, 2, halting_default());
        assert_eq!(first.exit, PagedProbeExit::BudgetExhausted, "{first:?}");
        assert_eq!(first.retired, 2);
        assert_eq!(first.pm_timer_reads, Some(1));
        assert_eq!(first.virtual_ns, 1_002);

        // The same board can continue across calls; this field is the per-run delta, not the
        // handle's cumulative count since board construction.
        boot.board.state_mut().rip = ENTRY;
        let second = run_set_facade(&mut boot, &file, 2, halting_default());
        assert_eq!(second.pm_timer_reads, Some(1));
        assert_eq!(second.virtual_ns, 2_004);
    }

    #[test]
    fn stop_record_counts_rdtsc_and_pit_reads_per_invocation() {
        // RDTSC; MOV DX,0x61; IN; MOV DX,0x42; IN; MOV DX,0x608; IN; HLT.
        let code = [
            0x0f, 0x31, 0x66, 0xba, 0x61, 0x00, 0xec, 0x66, 0xba, 0x42, 0x00, 0xec, 0x66, 0xba,
            0x08, 0x06, 0xec, 0xf4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        program_pit_channel_2(&mut boot);
        let check = |record: &LinuxStopRecord| {
            assert_eq!(record.rdtsc_reads, Some(1));
            assert_eq!(record.rdtsc_reads_by_cpl, Some([1, 0, 0, 0]));
            assert_eq!(record.pit_port_b_reads, Some(1));
            assert_eq!(record.pit_counter_port_reads, Some(1));
            assert_eq!(record.pm_timer_reads, Some(1));
        };

        let first = run_set_facade(&mut boot, &file, 32, halting_default());
        assert_eq!(first.exit, PagedProbeExit::Halted, "{first:?}");
        check(&first);

        // Resuming the same board reports the invocation delta, not cumulative device counts.
        boot.board.state_mut().rip = ENTRY;
        boot.board.state_mut().activity = Some(CpuActivity::Running);
        let second = run_set_facade(&mut boot, &file, 32, halting_default());
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        check(&second);
    }

    #[test]
    fn the_4k_walker_on_the_linux_board_is_a_walk_error_not_unsupported() {
        // The Linux tables map low RAM with 2 MiB leaves; the 4 KiB walker (`run_boot`) refuses
        // the leaf while it walks, before any opcode is decoded.
        let (mut boot, _) = boot_with_entry(&com1_program(b'X'));
        let report = boot.board.run_boot(16).expect("runs");
        assert_eq!(report.retired, 0);
        assert!(
            matches!(report.exit, PagedProbeExit::Backend(_)),
            "{:?}",
            report.exit
        );
        assert!(
            matches!(
                report.memory_error,
                Some(crate::TrackedAccessError::UnsupportedPageSize { .. })
            ),
            "{:?}",
            report.memory_error
        );
    }

    #[test]
    fn rdtsc_through_the_facade_ticks_the_virtual_clock_by_one_microsecond() {
        // RDTSC; HLT. RDTSC is the first quantum (it ends the quantum after one instruction),
        // HLT the second: quanta == 2, retired == 2. The clock moves by 1 ns per retired
        // instruction plus the 1000 ns RDTSC tick: 2 + 1000.
        let record = run_entry(&[0x0F, 0x31, 0xF4], 64);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!((record.retired, record.quanta), (2, 2));
        assert_eq!(record.virtual_ns, 2 + 1_000);
        assert_eq!(record.rip, 0x0100_0200 + 3);
        // RDTSC read the clock at the start of the run (0): EDX:EAX = 0.
        assert_eq!(record.gpr[Register::Rax as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert!(record.linux_profile);
        // The same code without RDTSC (MOV AL, 1; HLT) moves the clock only by the retired instructions.
        let plain = run_entry(&[0xB0, 0x01, 0xF4], 64);
        assert_eq!((plain.retired, plain.quanta, plain.virtual_ns), (2, 1, 2));
    }

    #[test]
    fn a_rip_that_translates_only_in_part_gives_a_short_window_and_its_error() {
        // The last 4 bytes of the 128 MiB RAM are mapped, the next 2 MiB page is not: the walk
        // gives the GPA of RIP and 4 bytes, then the not-present PD entry of the fifth byte.
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.state_mut().rip = 128 * MIB - 4;
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 0)
            .expect("runs");
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(record.rip_gpa, Some(128 * MIB - 4));
        assert_eq!(record.rip_bytes, [0, 0, 0, 0]);
        let error = record.rip_bytes_error.expect("the window is short");
        assert!(error.contains("NotPresent") && error.contains("linear_address: Some(134217728)"));
    }

    #[test]
    fn a_rip_that_does_not_translate_has_no_gpa_and_no_bytes() {
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.state_mut().rip = 256 * MIB;
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 0)
            .expect("runs");
        assert_eq!(record.rip, 256 * MIB);
        assert_eq!(record.rip_gpa, None);
        assert!(record.rip_bytes.is_empty());
        let error = record.rip_bytes_error.expect("the first byte fails");
        assert!(error.contains("NotPresent"), "{error}");
    }

    #[test]
    fn a_fetch_from_an_unmapped_page_is_the_fault_exit_with_the_page_fault_record() {
        // Fetching from an unmapped RIP is the architectural #PF (vector 14, error code 16 =
        // instruction fetch). A store or an OUT does not reach `Fault` in this profile: MOV
        // [mem], r8 is `Unsupported` (0x88) and OUT to a missing port is `PortIo`.
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.state_mut().rip = 256 * MIB;
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 8)
            .expect("runs");
        assert_eq!(record.exit_kind, "fault");
        let PagedProbeExit::Fault(fault) = &record.exit else {
            panic!("{record:?}");
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(fault.error_code, Some(16));
        assert_eq!(fault.linear_address, Some(256 * MIB));
        assert_eq!((record.retired, record.rip), (0, 256 * MIB));
        let page_fault = record.page_fault.expect("the walker's record");
        assert!(page_fault.contains("access: Fetch"), "{page_fault}");
        assert_eq!(record.rip_gpa, None);
        assert!(record.rip_bytes.is_empty());
    }

    #[test]
    fn the_cpl_is_the_cs_selector_rpl_while_the_interpreter_runs_ring0() {
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.state_mut().cs_selector = Some(0x13);
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 0)
            .expect("runs");
        assert_eq!(record.cpl, 3);
    }

    #[test]
    fn a_clock_failure_is_run_failed_with_the_instructions_retired_before_it() {
        // The clock is at its maximum, so the first quantum (HLT, one instruction) executes and
        // then its 1 ns cannot be added: the error must not claim that nothing was retired.
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board
            .advance_time_ns(u64::MAX)
            .expect("the clock fits");
        let error = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 8)
            .expect_err("the clock overflows");
        assert_eq!((error.code, error.stage), ("run_failed", "run"));
        assert!(error.detail.contains("TimeOverflow"), "{}", error.detail);
        assert_eq!(error.retired, Some(1));
        // A loader error never ran the guest.
        let file = bzimage();
        let refused = load_linux_direct_board(&input(&file[..10], &[], BLOB, board_hash()))
            .err()
            .expect("truncated");
        assert_eq!(refused.retired, None);
    }
    // ---- S2-45f: the CPL0 profile set through the board walker ----

    use super::run_linux_direct_board_set_observed;
    use crate::execution_observations::LinuxExecutionObservationCollector;
    use crate::paged_memory_adapter::{
        PagedCpl0SetExit, PagedCpl0SetReport, run_cpl0_set_paged_2m_ram,
        run_cpl0_set_paged_2m_ram_observed,
    };
    use od_isa::execute::{
        ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1, RunExit, TranslationContextChange,
    };
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::msr::MsrProfile;
    use od_isa::{CliProfile, CpuActivity, HaltProfile, INTEGER_FLAGS, LongModeState, StiProfile};

    const ENTRY: u64 = 0x0100_0200;
    /// `cld; cli; xor eax,eax; mov ds,ax; mov es,ax; mov ss,ax`: the first bytes of `startup_64`
    /// up to the first `MOV Sreg`.
    const PROLOGUE: [u8; 10] = [0xfc, 0xfa, 0x31, 0xc0, 0x8e, 0xd8, 0x8e, 0xc0, 0x8e, 0xd0];
    /// Second page-table tree: PML4, PDPT, PD and PT, then the code page it maps at GVA 0x0100_0000.
    const TREE_PML4: u64 = 0x0400_0000;
    const TREE_PDPT: u64 = TREE_PML4 + 0x1000;
    const TREE_PD: u64 = TREE_PML4 + 0x2000;
    const TREE_PT: u64 = TREE_PML4 + 0x3000;
    const TREE_CODE: u64 = 0x0500_0000;

    fn long64() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    /// Runs the set against the board walker from the board's state (CR3 stays the board's unless
    /// `owned_cr3`, which hands it to the CPU).
    fn run_set(
        boot: &mut LinuxDirectBoot,
        owned_cr3: bool,
        budget: u64,
        set: &Cpl0ProfileSet<'_>,
    ) -> (LongModeState, PagedCpl0SetReport) {
        run_set_with_observations(boot, owned_cr3, budget, set, None)
    }

    fn run_set_with_observations(
        boot: &mut LinuxDirectBoot,
        owned_cr3: bool,
        budget: u64,
        set: &Cpl0ProfileSet<'_>,
        observations: Option<&mut LinuxExecutionObservationCollector>,
    ) -> (LongModeState, PagedCpl0SetReport) {
        let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
        let mut state = boot.board.state().clone();
        state.cr3 = owned_cr3.then_some(cr3);
        let report = run_cpl0_set_paged_2m_ram_observed(
            &mut state,
            boot.board.bus_mut(),
            cr3,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |_| {},
            false,
            None,
            None,
            None,
            observations,
        );
        (state, report)
    }

    #[test]
    fn instruction_path_trace_records_retired_conditional_branch_and_following_order() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let mut boot = boot_with_entry(&[0x74, 0x02, 0x90, 0x90, 0x90, 0xF4]).0;
        boot.board.state_mut().rflags |= 1 << 6;
        let mut observations =
            LinuxExecutionObservationCollector::default().with_instruction_path_trace(true);
        for _ in 0..crate::LINUX_INSTRUCTION_PATH_TRACE_START_HITS {
            observations.record_cpl0_rdtsc(Some(crate::LINUX_RDTSC_PREFIX_SENTINEL_RIP));
        }
        observations.begin_instruction_path_quantum(
            crate::LINUX_INSTRUCTION_PATH_TRACE_QUANTUM,
            crate::LINUX_INSTRUCTION_PATH_TRACE_RETIRED_GATE,
        );
        let (state, report) = run_set_with_observations(
            &mut boot,
            true,
            8,
            &switch_set(&cli, &halt),
            Some(&mut observations),
        );
        assert_eq!(report.instructions_executed, 3, "{report:?}");
        assert_eq!(state.rip, ENTRY + 6);
        let trace = observations.finish().instruction_path_trace.unwrap();
        assert_eq!(trace.records.len(), 3);
        assert_eq!(
            (trace.records[0].rip, trace.records[0].next_rip),
            (ENTRY, ENTRY + 4)
        );
        let branch = trace.records[0].control_flow.unwrap();
        assert_eq!(branch.kind, "branch");
        assert_eq!(branch.condition, Some(4));
        assert!(branch.taken);
        assert_ne!(branch.rflags_before & (1 << 6), 0);
        assert_eq!(branch.destination, ENTRY + 4);
        assert_eq!(
            (trace.records[1].rip, trace.records[1].next_rip),
            (ENTRY + 4, ENTRY + 5)
        );
        assert_eq!(
            (trace.records[2].rip, trace.records[2].next_rip),
            (ENTRY + 5, ENTRY + 6)
        );
    }

    fn write_phys(boot: &mut LinuxDirectBoot, address: u64, bytes: &[u8]) {
        boot.board
            .bus_mut()
            .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
            .expect("write");
    }

    #[test]
    fn cpl0_set_board_prologue_stops_at_the_first_segment_load_the_set_does_not_admit() {
        let cli = CliProfile::NonPviLong64V1;
        for owned_cr3 in [false, true] {
            let mut set =
                Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
            if owned_cr3 {
                set = set.with_cpu_owned_cr3(MovCr3Long64V1);
            }
            // The state the loader hands over has loaded DS/ES/SS descriptor caches. Since S2-46
            // the set admits the null `MOV DS/ES/SS, AX` over them: CLD, CLI, XOR and the three
            // loads retire (six), and the stop is the filler byte after the prologue (0x7A).
            let (mut boot, _) = boot_with_entry(&PROLOGUE);
            let (state, report) = run_set(&mut boot, owned_cr3, 64, &set);
            assert_eq!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0x7A }),
                "{report:?}"
            );
            assert_eq!(report.instructions_executed, 6);
            assert_eq!((state.ds_selector, state.es_selector), (0, 0));
            assert_eq!(state.ss_selector, Some(0));
            assert!(state.ds_unusable && state.es_unusable && state.ss_unusable);
            // MOV SS opened the one-instruction shadow; the filler stop did not retire.
            assert_eq!(state.sti_shadow, Some(od_isa::StiShadow::Blocking));
            assert_eq!(state.rip, ENTRY + 10);
            assert_eq!(state.rflags & (1 << 9), 0, "CLI cleared IF");
            assert_eq!(report.page_fault, None);
            assert_eq!(report.memory_error, None);
            assert!(report.translation_context_changes.is_empty());

            // The S2-45a invariant on the same bytes and the same walker: with legacy (cache-less)
            // DS/ES/SS the null loads retire and the stop is `MOV SS` after five instructions.
            let (mut boot, _) = boot_with_entry(&PROLOGUE);
            let state = boot.board.state_mut();
            (state.ds_cache, state.es_cache, state.ss_cache) = (None, None, None);
            let (state, report) = run_set(&mut boot, owned_cr3, 64, &set);
            assert_eq!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0x8E }),
                "{report:?}"
            );
            assert_eq!(report.instructions_executed, 5);
            assert_eq!(state.rip, ENTRY + 8);
            assert_eq!(state.rflags & (1 << 9), 0, "CLI cleared IF");
            assert_eq!(state.gpr[Register::Rax as usize], 0);
        }
        // Without CLI the first stop is FA, after CLD only.
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1);
        let (mut boot, _) = boot_with_entry(&PROLOGUE);
        let (state, report) = run_set(&mut boot, false, 64, &set);
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0xFA })
        );
        assert_eq!((report.instructions_executed, state.rip), (1, ENTRY + 1));
    }

    #[test]
    fn cpl0_set_board_fetch_from_an_unmapped_page_is_the_exact_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        boot.board.state_mut().rip = 256 * MIB;
        let (state, report) = run_set(&mut boot, false, 8, &set);
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(16), Some(256 * MIB))
        );
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(state.rip, 256 * MIB);
        let record = report.page_fault.expect("the walker's record");
        assert_eq!(record.saved_rip(), 256 * MIB);
        assert_eq!(record.fault(), fault);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Fetch);
    }

    #[test]
    fn cpl0_set_board_lock_mov_cr3_is_ud_with_nothing_retired() {
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cpu_owned_cr3(MovCr3Long64V1);
        let (mut boot, _) = boot_with_entry(&[0xF0, 0x0F, 0x22, 0xD8]);
        let (state, report) = run_set(&mut boot, true, 8, &set);
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!((fault.vector, fault.error_code), (6, None));
        assert_eq!((report.instructions_executed, state.rip), (0, ENTRY));
        assert_eq!(state.cr3, Some(boot.board.cr3()));
        assert_eq!(report.page_fault, None);
    }

    /// `CLI; MOV RAX, imm32; MOV CR3, RAX; MOV RBX, marker; HLT`. The bytes after MOV CR3 sit at
    /// GVA `0x0100_020B`; `marker` there is what the old tree executes.
    fn cr3_switch_program(new_cr3: u64, marker: u32) -> Vec<u8> {
        let mut code = vec![0xFA, 0x48, 0xC7, 0xC0];
        code.extend_from_slice(&(new_cr3 as u32).to_le_bytes());
        code.extend_from_slice(&[0x0F, 0x22, 0xD8]);
        code.extend_from_slice(&[0x48, 0xC7, 0xC3]);
        code.extend_from_slice(&marker.to_le_bytes());
        code.push(0xF4);
        code
    }

    /// Maps GVA `0x0100_0000..+4 KiB` to `TREE_CODE` through a 4 KiB leaf and puts the code of
    /// `cr3_switch_program` (with `marker`) on that page. Everything else is unmapped.
    fn install_second_tree(boot: &mut LinuxDirectBoot, marker: u32) {
        write_phys(boot, TREE_PML4, &(TREE_PDPT | 3).to_le_bytes());
        write_phys(boot, TREE_PDPT, &(TREE_PD | 3).to_le_bytes());
        // GVA 0x0100_0000 is PD index 8 (2 MiB units), PT index 0.
        write_phys(boot, TREE_PD + 8 * 8, &(TREE_PT | 3).to_le_bytes());
        write_phys(boot, TREE_PT, &(TREE_CODE | 3).to_le_bytes());
        write_phys(
            boot,
            TREE_CODE + 0x200,
            &cr3_switch_program(TREE_PML4, marker),
        );
    }

    fn switch_set<'a>(cli: &'a CliProfile, halt: &'a HaltProfile) -> Cpl0ProfileSet<'a> {
        Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(cli)
            .with_halt(halt)
            .with_cpu_owned_cr3(MovCr3Long64V1)
    }

    #[test]
    fn cpl0_set_board_fetch_after_mov_cr3_walks_the_new_cr3() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let board_cr3 = {
            let (boot, _) = boot_with_entry(&[0xF4]);
            boot.board.cr3()
        };
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let (state, report) = run_set(&mut boot, true, 64, &switch_set(&cli, &halt));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{report:?}"
        );
        // CLI, MOV RAX, MOV CR3, then the instruction at the old RIP + 11 fetched through the
        // new tree (marker 0x2222, not the 0x1111 the old tree holds), then HLT.
        assert_eq!(report.instructions_executed, 5);
        assert_eq!(state.gpr[Register::Rbx as usize], 0x2222);
        assert_eq!(state.rip, ENTRY + 0x13);
        assert_eq!(state.cr3, Some(TREE_PML4));
        assert_eq!(report.cr3, TREE_PML4);
        assert_eq!(
            report.translation_context_changes,
            [TranslationContextChange::Cr3Write {
                old_cr3: board_cr3,
                new_cr3: TREE_PML4
            }]
        );
        assert_eq!(state.activity, Some(CpuActivity::Halted));
    }

    #[test]
    fn cpl0_set_board_rewriting_the_same_cr3_keeps_the_old_translation() {
        // Control for the test above: the same program with the board's own CR3 executes the
        // old bytes (marker 0x1111), so the new tree is what changed the result.
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let board_cr3 = {
            let (boot, _) = boot_with_entry(&[0xF4]);
            boot.board.cr3()
        };
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(board_cr3, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let (state, report) = run_set(&mut boot, true, 64, &switch_set(&cli, &halt));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 5);
        assert_eq!(state.gpr[Register::Rbx as usize], 0x1111);
        assert_eq!(
            report.translation_context_changes,
            [TranslationContextChange::Cr3Write {
                old_cr3: board_cr3,
                new_cr3: board_cr3
            }]
        );
    }

    #[test]
    fn cpl0_set_board_budget_after_mov_cr3_stops_before_the_next_fetch() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let (state, report) = run_set(&mut boot, true, 3, &switch_set(&cli, &halt));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted)
        );
        assert_eq!(report.instructions_executed, 3);
        assert_eq!(state.rip, ENTRY + 0xB);
        assert_eq!(state.gpr[Register::Rbx as usize], 0);
        assert_eq!(report.cr3, TREE_PML4);
        assert_eq!(report.translation_context_changes.len(), 1);
    }

    #[test]
    fn cpl0_set_board_an_unmapped_rip_in_the_new_tree_is_a_page_fault_with_the_new_cr3() {
        // The new tree maps only the code page: after the switch, jump-free execution continues
        // on the same page, so unmap it by pointing CR3 at an empty PML4 instead.
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        // TREE_PML4 stays zeroed: nothing is mapped.
        let (state, report) = run_set(&mut boot, true, 64, &switch_set(&cli, &halt));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(16), Some(ENTRY + 0xB))
        );
        assert_eq!(report.instructions_executed, 3);
        assert_eq!(state.cr3, Some(TREE_PML4));
        assert_eq!(report.page_fault.expect("record").saved_rip(), ENTRY + 0xB);
    }

    #[test]
    fn cpl0_set_board_a_cr3_that_disagrees_with_the_cpu_is_a_configuration_error() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let efer = boot.board.efer();
        let mut state = boot.board.state().clone();
        state.cr3 = Some(boot.board.cr3());
        let report = run_cpl0_set_paged_2m_ram(
            &mut state,
            boot.board.bus_mut(),
            TREE_PML4,
            efer,
            long64(),
            8,
            INTEGER_FLAGS,
            &switch_set(&cli, &halt),
            None,
            || false,
            |_| {},
        );
        assert!(
            matches!(
                report.exit,
                PagedCpl0SetExit::Configuration(crate::ProbeConfigError::Cr3Mismatch { .. })
            ),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 0);
    }

    // ---- S4-38L3: dword reads through the set's board adapter ----

    /// Mapped, otherwise unused RAM of the board (the identity-mapped 128 MiB).
    const DATA: u64 = 0x0600_0000;

    fn minimal_set(cli: &CliProfile) -> Cpl0ProfileSet<'_> {
        Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(cli)
            .with_cpu_owned_cr3(MovCr3Long64V1)
    }

    #[test]
    fn cpl0_set_board_prefetcht0_is_a_nop_that_does_not_touch_memory() {
        // S4-38L6a: PREFETCHT0 [RDX] with RDX at an unmapped address: no #PF, RIP + 3.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0x0F, 0x18, 0x0A]);
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x1122_3344_5566_7788;
        state.gpr[Register::Rdx as usize] = 0x0000_7fff_0000_0000;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_7788);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
    }

    #[test]
    fn cpl0_set_board_mov_eax_from_rbx_reads_a_dword_and_zero_extends() {
        let cli = CliProfile::NonPviLong64V1;
        // MOV EAX, [RBX]; the bytes after it are the filler, not executed (budget 1).
        let (mut boot, _) = boot_with_entry(&[0x8B, 0x03]);
        // The qword around the dword has a different upper half: only 4 bytes are read.
        write_phys(&mut boot, DATA, &0xAABB_CCDD_1234_5678u64.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = u64::MAX;
        state.gpr[Register::Rbx as usize] = DATA;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state.rip, ENTRY + 2);
        assert_eq!(state.gpr[Register::Rax as usize], 0x1234_5678);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
    }

    #[test]
    fn cpl0_set_board_mov_eax_rsi_disp32_reads_the_dword_at_the_displaced_address() {
        let cli = CliProfile::NonPviLong64V1;
        // MOV EAX, [RSI + 0x230] (8B 86 30 02 00 00), the form of the real kernel at 0x0100_0215.
        let (mut boot, _) = boot_with_entry(&[0x8B, 0x86, 0x30, 0x02, 0x00, 0x00]);
        write_phys(&mut boot, DATA + 0x230, &0x8000_0001u32.to_le_bytes());
        write_phys(&mut boot, DATA + 0x234, &0xFFFF_FFFFu32.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0xDEAD_BEEF_0000_0000;
        state.gpr[Register::Rsi as usize] = DATA;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(state.rip, ENTRY + 6);
        // Bit 31 set: MOV r32 zero-extends, it does not sign-extend.
        assert_eq!(state.gpr[Register::Rax as usize], 0x8000_0001);
        assert_eq!(state.gpr[Register::Rsi as usize], DATA);
    }

    #[test]
    fn cpl0_set_board_dword_read_updates_the_page_tables_like_the_qword_read() {
        let cli = CliProfile::NonPviLong64V1;
        let run = |code: &[u8]| {
            let (mut boot, _) = boot_with_entry(code);
            boot.board.state_mut().gpr[Register::Rbx as usize] = DATA;
            let before = page_tables(&mut boot);
            let (_, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
            assert_eq!(report.instructions_executed, 1, "{report:?}");
            (before, page_tables(&mut boot))
        };
        let (before, dword) = run(&[0x8B, 0x03]);
        let (_, qword) = run(&[0x48, 0x8B, 0x03]);
        // The walk of the 2 MiB leaf at DATA sets its accessed bit; the width changes nothing.
        assert_ne!(before, dword, "the walker updates the accessed bit");
        assert_eq!(dword, qword);
    }

    #[test]
    fn cpl0_set_board_dword_inside_the_last_bytes_of_a_4k_page_is_one_access() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0x8B, 0x03]);
        // The dword ends exactly on the page end: bytes 0xFFC..0x1000.
        write_phys(&mut boot, DATA + 0xFFC, &0x0A0B_0C0Du32.to_le_bytes());
        boot.board.state_mut().gpr[Register::Rbx as usize] = DATA + 0xFFC;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rax as usize], 0x0A0B_0C0D);
    }

    #[test]
    fn cpl0_set_board_dword_crossing_a_4k_boundary_is_read_byte_by_byte() {
        let cli = CliProfile::NonPviLong64V1;
        // S4-38L5e: it was the typed `CrossesPage` stop (S4-11). The set's adapter now reads each
        // byte through its own translation. The pages here lie in one identity-mapped 2 MiB
        // leaf: the bytes are contiguous, RAX gets the dword and the data is not touched.
        for offset in [0xFFD, 0xFFE, 0xFFF] {
            let (mut boot, _) = boot_with_entry(&[0x8B, 0x03]);
            write_phys(&mut boot, DATA + 0xFFC, &[0xA1, 0xA2, 0xA3, 0xA4]);
            write_phys(&mut boot, DATA + 0x1000, &[0xB1, 0xB2, 0xB3, 0xB4]);
            let state = boot.board.state_mut();
            state.gpr[Register::Rax as usize] = 0x5555_5555_5555_5555;
            state.gpr[Register::Rbx as usize] = DATA + offset;
            let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
            assert_eq!(report.instructions_executed, 1, "{offset:#x}: {report:?}");
            assert_eq!((report.page_fault, report.memory_error), (None, None));
            let mut window = [0u8; 8];
            read(&mut boot.board, DATA + 0xFFC, &mut window).expect("read");
            assert_eq!(window, [0xA1, 0xA2, 0xA3, 0xA4, 0xB1, 0xB2, 0xB3, 0xB4]);
            let skip = (offset - 0xFFC) as usize;
            let expected = u32::from_le_bytes(window[skip..skip + 4].try_into().unwrap());
            // MOV r32 zero-extends into the whole register.
            assert_eq!(state.gpr[Register::Rax as usize], u64::from(expected));
            assert_eq!(state.rip, ENTRY + 2);
        }
    }

    #[test]
    fn cpl0_set_board_dword_read_from_an_unmapped_page_is_the_exact_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        // CLD retires, then MOV EAX, [RBX] faults: the record names the second instruction.
        let (mut boot, _) = boot_with_entry(&[0xFC, 0x8B, 0x03]);
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x77;
        state.gpr[Register::Rbx as usize] = 256 * MIB;
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        // Not-present data read at CPL0: error code 0.
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0), Some(256 * MIB))
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            (state.rip, state.gpr[Register::Rax as usize]),
            (ENTRY + 1, 0x77)
        );
        let record = report.page_fault.expect("the walker's record");
        assert_eq!(record.saved_rip(), ENTRY + 1);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Read);
        // The qword form fails the same way, with the same record shape.
        let (mut boot, _) = boot_with_entry(&[0xFC, 0x48, 0x8B, 0x03]);
        boot.board.state_mut().gpr[Register::Rbx as usize] = 256 * MIB;
        let (_, qword) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        assert_eq!(qword.exit, report.exit);
        assert_eq!(qword.instructions_executed, report.instructions_executed);
    }

    #[test]
    fn cpl0_set_board_qword_read_still_works_next_to_the_dword_read() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0x48, 0x8B, 0x03]);
        write_phys(&mut boot, DATA, &0xAABB_CCDD_1234_5678u64.to_le_bytes());
        boot.board.state_mut().gpr[Register::Rbx as usize] = DATA;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rax as usize], 0xAABB_CCDD_1234_5678);
    }

    // ---- S4-38L3: byte, word and dword stores through the set's board adapter ----

    const QWORD_SEED: u64 = 0xAABB_CCDD_1122_3344;

    fn read_qword(boot: &mut LinuxDirectBoot, address: u64) -> u64 {
        let mut bytes = [0u8; 8];
        read(&mut boot.board, address, &mut bytes).expect("read");
        u64::from_le_bytes(bytes)
    }

    /// Runs one store at `ENTRY` with RBX = `DATA + offset`, RAX = `0x1111_2222_3333_445A`.
    fn run_store(
        code: &[u8],
        offset: u64,
    ) -> (LinuxDirectBoot, LongModeState, PagedCpl0SetReport, Vec<u8>) {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(code);
        write_phys(&mut boot, DATA, &QWORD_SEED.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x1111_2222_3333_445A;
        state.gpr[Register::Rbx as usize] = DATA + offset;
        let before = page_tables(&mut boot);
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        (boot, state, report, before)
    }

    /// The page tables after one `CLD` fetched at `ENTRY`: the fetch walk sets its own accessed
    /// bits, so a store that must not touch the tables is compared against this, not against the
    /// tables before the fetch.
    fn tables_after_the_fetch_alone() -> Vec<u8> {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xFC]);
        let (_, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        page_tables(&mut boot)
    }

    #[test]
    fn cpl0_set_board_byte_store_writes_one_byte_and_keeps_the_rest_of_the_qword() {
        // MOV [RBX], AL
        let (mut boot, state, report, _) = run_store(&[0x88, 0x03], 0);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.rip, ENTRY + 2);
        assert_eq!(read_qword(&mut boot, DATA), 0xAABB_CCDD_1122_335A);
    }

    #[test]
    fn cpl0_set_board_word_store_writes_two_bytes_and_keeps_the_rest_of_the_qword() {
        // MOV [RBX], AX (66 89 03)
        let (mut boot, state, report, _) = run_store(&[0x66, 0x89, 0x03], 0);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(read_qword(&mut boot, DATA), 0xAABB_CCDD_1122_445A);
    }

    #[test]
    fn cpl0_set_board_dword_store_writes_four_bytes_and_keeps_the_rest_of_the_qword() {
        // MOV [RBX], EAX
        let (mut boot, state, report, _) = run_store(&[0x89, 0x03], 0);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, ENTRY + 2);
        assert_eq!(read_qword(&mut boot, DATA), 0xAABB_CCDD_3333_445A);
    }

    #[test]
    fn cpl0_set_board_qword_store_still_writes_all_eight_bytes() {
        // MOV [RBX], RAX
        let (mut boot, state, report, _) = run_store(&[0x48, 0x89, 0x03], 0);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(read_qword(&mut boot, DATA), 0x1111_2222_3333_445A);
    }

    #[test]
    fn cpl0_set_board_mov_rax_sil_with_rex_stores_the_low_byte_of_rsi() {
        // MOV [RAX], SIL (40 88 30): the form of the real kernel at 0x0206_3a80.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0x40, 0x88, 0x30]);
        write_phys(&mut boot, DATA + 8, &QWORD_SEED.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = DATA + 8;
        // Without REX the register would be DH: DH is 0xA5 here, so the stored byte 0x5A can
        // only be SIL.
        state.gpr[Register::Rsi as usize] = 0xFFFF_FFFF_FFFF_A55A;
        state.gpr[Register::Rdx as usize] = 0x0000_A500;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(read_qword(&mut boot, DATA + 8), 0xAABB_CCDD_1122_335A);
    }

    /// Offset in the page-table zone of the PDE of the 2 MiB leaf at `DATA`: the loader puts the
    /// PML4 on page 0, the PDPT on page 1 and the PD of the first GiB on page 2, and
    /// `DATA >> 21 = 48` (0x0600_0000 = 96 MiB).
    const DATA_PDE_OFFSET: usize = 0x2000 + ((DATA >> 21) as usize) * 8;

    fn qword_at(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    #[test]
    fn cpl0_set_board_narrow_store_updates_the_page_tables_like_the_qword_store() {
        let fetch_only = tables_after_the_fetch_alone();
        // The loader writes the leaf as present, writable and PS (0x83): A and D are clear.
        assert_eq!(qword_at(&fetch_only, DATA_PDE_OFFSET), DATA | 0x83);
        // The store sets A and D on that one PDE and nowhere else.
        let mut expected = fetch_only.clone();
        expected[DATA_PDE_OFFSET..DATA_PDE_OFFSET + 8]
            .copy_from_slice(&(DATA | 0x83 | 0x60).to_le_bytes());
        let tables = |code: &[u8]| {
            let (mut boot, _, report, _) = run_store(code, 0);
            assert_eq!(report.instructions_executed, 1, "{report:?}");
            page_tables(&mut boot)
        };
        for code in [
            &[0x48, 0x89, 0x03][..],
            &[0x88, 0x03],
            &[0x66, 0x89, 0x03],
            &[0x89, 0x03],
        ] {
            let after = tables(code);
            let changed: Vec<usize> = (0..after.len())
                .filter(|&at| after[at] != fetch_only[at])
                .collect();
            assert!(
                changed
                    .iter()
                    .all(|at| (DATA_PDE_OFFSET..DATA_PDE_OFFSET + 8).contains(at)),
                "{code:02x?}: bytes changed outside the data PDE: {changed:x?}"
            );
            assert_eq!(after, expected, "{code:02x?}");
        }
    }

    #[test]
    fn cpl0_set_board_dword_read_modify_write_stores_exactly_one_dword() {
        // ADD [RBX], EAX (01 03): the dword read-modify-write of the set. The write is the
        // 4 source bytes alone, the qword around them is not rewritten.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0x01, 0x03]);
        write_phys(&mut boot, DATA, &QWORD_SEED.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x1111_2222_3333_445A;
        state.gpr[Register::Rbx as usize] = DATA;
        let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
        let mut state = boot.board.state().clone();
        state.cr3 = Some(cr3);
        let mut writes = Vec::new();
        let report = run_cpl0_set_paged_2m_ram(
            &mut state,
            boot.board.bus_mut(),
            cr3,
            efer,
            long64(),
            1,
            INTEGER_FLAGS,
            &minimal_set(&cli),
            None,
            || false,
            |range| writes.push(range),
        );
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.rip, ENTRY + 2);
        // 0x1122_3344 + 0x3333_445A = 0x4455_779E; the upper half of the qword is untouched.
        assert_eq!(read_qword(&mut boot, DATA), 0xAABB_CCDD_4455_779E);
        let data_writes: Vec<_> = writes
            .iter()
            .filter(|range| (DATA..DATA + 0x1000).contains(&range.start().get()))
            .map(|range| (range.start().get(), range.len()))
            .collect();
        assert_eq!(data_writes, [(DATA, 4)]);
    }

    #[test]
    fn cpl0_set_board_byte_store_on_the_last_byte_of_a_4k_page_is_one_access() {
        let (mut boot, _, report, _) = run_store(&[0x88, 0x03], 0xFFF);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(report.memory_error, None);
        let mut byte = [0u8];
        read(&mut boot.board, DATA + 0xFFF, &mut byte).expect("read");
        assert_eq!(byte, [0x5A]);
        // The dword store ending exactly on the page end is one access too.
        let (mut boot, _, report, _) = run_store(&[0x89, 0x03], 0xFFC);
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        let mut dword = [0u8; 4];
        read(&mut boot.board, DATA + 0xFFC, &mut dword).expect("read");
        assert_eq!(u32::from_le_bytes(dword), 0x3333_445A);
    }

    #[test]
    fn cpl0_set_board_word_and_dword_stores_crossing_a_4k_boundary_store_both_pieces() {
        // S4-38L4h: the set's adapter performs a store across a 4 KiB boundary (it was the typed
        // `CrossesPage` stop). The pages here lie in one identity-mapped 2 MiB leaf, so the
        // physical bytes are contiguous: the value is stored whole, nothing around it moves.
        let cases: [(&[u8], &[u8], &[u64]); 2] = [
            (&[0x66, 0x89, 0x03], &[0x5A, 0x44], &[0xFFF]),
            (
                &[0x89, 0x03],
                &[0x5A, 0x44, 0x33, 0x33],
                &[0xFFD, 0xFFE, 0xFFF],
            ),
        ];
        let fetch_only = tables_after_the_fetch_alone();
        let mut expected_tables = fetch_only.clone();
        expected_tables[DATA_PDE_OFFSET..DATA_PDE_OFFSET + 8]
            .copy_from_slice(&(DATA | 0x83 | 0x60).to_le_bytes());
        for (code, stored, offsets) in cases {
            for &offset in offsets {
                let (mut boot, state, report, _) = run_store(code, offset);
                assert_eq!(report.instructions_executed, 1, "{report:?}");
                assert_eq!((report.page_fault, report.memory_error), (None, None));
                assert_eq!(state.rip, ENTRY + code.len() as u64);
                let mut window = [0u8; 0x20];
                read(&mut boot.board, DATA + 0xFF0, &mut window).expect("read");
                let mut expected = [0u8; 0x20];
                let at = (offset - 0xFF0) as usize;
                expected[at..at + stored.len()].copy_from_slice(stored);
                assert_eq!(window, expected, "{code:02x?} at {offset:#x}");
                assert_eq!(read_qword(&mut boot, DATA), QWORD_SEED);
                assert_eq!(page_tables(&mut boot), expected_tables, "{code:02x?}");
            }
        }
    }

    #[test]
    fn cpl0_set_board_byte_store_to_an_unmapped_page_is_the_exact_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        // CLD retires, then MOV [RBX], AL faults: the record names the second instruction.
        let (mut boot, _) = boot_with_entry(&[0xFC, 0x88, 0x03]);
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x5A;
        state.gpr[Register::Rbx as usize] = 256 * MIB;
        let fetch_only = tables_after_the_fetch_alone();
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        // Not-present data write at CPL0: error code 2 (W/R = 1, P = 0).
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(2), Some(256 * MIB))
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(state.rip, ENTRY + 1);
        let record = report.page_fault.expect("the walker's record");
        assert_eq!(record.saved_rip(), ENTRY + 1);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Write);
        assert_eq!(
            page_tables(&mut boot),
            fetch_only,
            "no accessed or dirty bit moved"
        );
    }

    #[test]
    fn paged_mov_2m_ram_profile_outside_the_set_still_refuses_the_byte_store() {
        // The qword-only shared profile keeps its behavior: the narrow stores belong to the
        // set's adapter alone. `88 03` is refused by the decoder allowlist (a decoder refusal,
        // not the flag guard: the flag guard is `narrow_stores_belong_to_the_cpl0_set_adapter_alone`
        // in the adapter module); nothing retires and the memory stays as it was.
        let (mut boot, _) = boot_with_entry(&[0x88, 0x03]);
        write_phys(&mut boot, DATA, &QWORD_SEED.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x5A;
        state.gpr[Register::Rbx as usize] = DATA;
        let mut state = state.clone();
        let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
        let report = crate::paged_memory_adapter::step_paged_mov_2m_ram(
            &mut state,
            boot.board.bus_mut(),
            cr3,
            efer,
            long64(),
            INTEGER_FLAGS,
            |_| {},
        )
        .expect("the step runs");
        assert_eq!(
            report.cpu.exit,
            RunExit::Unsupported { opcode: 0x88 },
            "{report:?}"
        );
        assert_eq!(report.cpu.instructions_executed, 0, "{report:?}");
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.rip, ENTRY);
        assert_eq!(read_qword(&mut boot, DATA), QWORD_SEED);
    }

    // ---- S4-38L3: the Linux board through the CPL0 set, the stop record ----

    fn run_set_facade(
        boot: &mut LinuxDirectBoot,
        file: &[u8],
        budget: u64,
        members: LinuxCpl0SetMembers,
    ) -> LinuxStopRecord {
        run_linux_direct_board_set(boot, &input(file, &[], BLOB, board_hash()), budget, members)
            .expect("runs")
    }

    #[test]
    fn set_facade_prologue_runs_the_null_segment_loads_over_the_loader_caches() {
        let (mut boot, file) = boot_with_entry(&PROLOGUE);
        let board_cr3 = boot.board.cr3();
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        // S2-46: the three null loads retire (six instructions); the stop is the filler byte
        // after the prologue.
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x7A },
            "{record:?}"
        );
        assert_eq!(record.unsupported_opcode, Some(0x7A));
        assert_eq!((record.retired, record.rip), (6, ENTRY + 10));
        assert_eq!(record.rflags & (1 << 9), 0, "CLI cleared IF");
        assert_eq!(record.path, "cpl0_set");
        assert_eq!(record.set_members, ["cli", "cpu_owned_cr3"]);
        assert_eq!(
            record.set_exit_detail.as_deref(),
            Some("Run(Unsupported { opcode: 122 })")
        );
        assert_eq!((record.cr3_changes, record.last_cr3_change), (0, None));
        assert_eq!(
            record.cr3, board_cr3,
            "no MOV CR3 retired: the board CR3 before the run"
        );
        assert_eq!(boot.board.state().cr3, None, "the CPU state keeps no CR3");
        // The base profile retires CLD but stops at CLI, which requires the set.
        let (mut boot, file) = boot_with_entry(&PROLOGUE);
        let base = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 64)
            .expect("runs");
        assert_eq!(base.exit, PagedProbeExit::Unsupported { opcode: 0xFA });
        assert_eq!(
            (base.retired, base.path, base.cr3_changes),
            (1, "paged_probe", 0)
        );
        assert!(base.set_members.is_empty() && base.set_exit_detail.is_none());
    }

    /// S4-38L5a: `MOV RAX, CR2` retires in the board's CPL0 set and reads the architectural
    /// reset value 0 (the board has no delivery yet, so nothing else writes CR2); the CPU state
    /// keeps no CR2 outside the run, like CR3.
    #[test]
    fn set_facade_mov_rax_cr2_reads_the_reset_value_zero() {
        for (code, length) in [
            (&[0x0F, 0x20, 0xD0][..], 3u64),
            (&[0x48, 0x0F, 0x20, 0xD0][..], 4),
        ] {
            let (mut boot, file) = boot_with_entry(code);
            boot.board.state_mut().gpr[Register::Rax as usize] = 0x1111_2222_3333_4444;
            assert_eq!(boot.board.state().cr2, None);
            let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
            // The read retires and exhausts the one-instruction budget.
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!((record.retired, record.rip), (1, ENTRY + length));
            assert_eq!(record.gpr[Register::Rax as usize], 0, "{code:02x?}");
            assert_eq!(boot.board.state().cr2, None, "the CPU state keeps no CR2");
        }
    }

    /// S4-38L7b: `MOV RDX, CR4` (`0F 20 E2`, the real kernel's CR4.LA57 probe) and `MOV R8, CR4`
    /// retire in the board's CPL0 set and read the board's CR4 (0x20, LA57 = 0).
    #[test]
    fn set_facade_mov_r64_cr4_reads_the_board_cr4() {
        for (code, length, dest) in [
            (&[0x0F, 0x20, 0xE2][..], 3u64, Register::Rdx),
            (&[0x41, 0x0F, 0x20, 0xE0][..], 4, Register::R8),
        ] {
            let (mut boot, file) = boot_with_entry(code);
            boot.board.state_mut().gpr[dest as usize] = 0x1111_2222_3333_4444;
            let board_cr4 = boot.board.state().cr4;
            assert_eq!(board_cr4, Some(0x20));
            let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!((record.retired, record.rip), (1, ENTRY + length));
            assert_eq!(record.gpr[dest as usize], 0x20, "{code:02x?}");
            assert_eq!(boot.board.state().cr4, board_cr4);
        }
    }

    /// S4-38L7c: `MOV CR4, RCX` (`0F 22 E1`) with RCX = 0x30 (PSE|PAE, the real kernel's value)
    /// retires in the board's CPL0 set; the board then accepts the new CR4 on re-entry (the
    /// walker snapshot admits PSE) and `MOV RDX, CR4` reads it back.
    #[test]
    fn set_facade_mov_cr4_rcx_with_pse_retires_and_the_board_reenters() {
        let code = [
            0xB9, 0x30, 0x00, 0x00, 0x00, // mov ecx, 0x30
            0x0F, 0x22, 0xE1, // mov cr4, rcx
            0x0F, 0x20, 0xE2, // mov rdx, cr4
        ];
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.state().cr4, Some(0x20));
        let record = run_set_facade(&mut boot, &file, 2, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (2, ENTRY + 8));
        assert_eq!(record.gpr[Register::Rcx as usize], 0x30);
        assert_eq!(boot.board.state().cr4, Some(0x30));
        assert_eq!(record.cr4, Some(0x30));
        let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (1, ENTRY + 11));
        assert_eq!(record.gpr[Register::Rdx as usize], 0x30);
    }

    /// S4-38L7d: `MOV CR4, RCX` with RCX = 0xb0 (PSE|PAE|PGE, the real kernel's second CR4
    /// write) retires in the board's CPL0 set; the board re-enters with the new CR4 and
    /// `MOV RDX, CR4` reads it back.
    #[test]
    fn set_facade_mov_cr4_rcx_with_pge_retires_and_the_board_reenters() {
        let code = [
            0xB9, 0xB0, 0x00, 0x00, 0x00, // mov ecx, 0xb0
            0x0F, 0x22, 0xE1, // mov cr4, rcx
            0x0F, 0x20, 0xE2, // mov rdx, cr4
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 2, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (2, ENTRY + 8));
        assert_eq!(boot.board.state().cr4, Some(0xB0));
        assert_eq!(record.cr4, Some(0xB0));
        let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (1, ENTRY + 11));
        assert_eq!(record.gpr[Register::Rdx as usize], 0xB0);
    }

    #[test]
    fn set_facade_mov_cr3_is_a_continuation_that_the_record_counts() {
        let board_cr3 = boot_with_entry(&[0xF4]).0.board.cr3();
        let (mut boot, file) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5);
        assert_eq!(
            record.gpr[Register::Rbx as usize],
            0x2222,
            "new tree fetched"
        );
        assert_eq!(record.cr3_changes, 1);
        assert_eq!(record.last_cr3_change, Some((board_cr3, TREE_PML4)));
        assert_eq!(record.cr3, TREE_PML4);
        assert_eq!(boot.board.cr3(), TREE_PML4);
        assert_eq!(boot.board.state().cr3, None);
        assert_eq!(record.set_members, ["cli", "cpu_owned_cr3", "halt"]);
        // Budget exactly at the MOV CR3: the continuation still stops before the next fetch.
        let (mut boot, file) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let record = run_set_facade(&mut boot, &file, 3, members);
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (3, ENTRY + 0xB));
        assert_eq!(record.last_cr3_change, Some((board_cr3, TREE_PML4)));
    }

    #[test]
    fn set_facade_mov_cr3_rdi_is_a_continuation_like_the_rax_form() {
        // S4-38L7h: the same layout as `cr3_switch_program`, with `mov rdi, imm32` (48 C7 C7)
        // and `mov cr3, rdi` (0F 22 DF) instead of the RAX bytes. The new tree then runs the same
        // continuation as the RAX form: five retired instructions, one CR3 change.
        let board_cr3 = boot_with_entry(&[0xF4]).0.board.cr3();
        let mut program = cr3_switch_program(TREE_PML4, 0x1111);
        program[3] = 0xC7;
        program[10] = 0xDF;
        assert_eq!(&program[8..11], &[0x0F, 0x22, 0xDF]);
        let (mut boot, file) = boot_with_entry(&program);
        install_second_tree(&mut boot, 0x2222);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "{record:?}");
        assert_eq!(record.gpr[Register::Rdi as usize], TREE_PML4);
        assert_eq!(
            record.gpr[Register::Rbx as usize],
            0x2222,
            "new tree fetched"
        );
        assert_eq!(record.cr3_changes, 1);
        assert_eq!(record.last_cr3_change, Some((board_cr3, TREE_PML4)));
        assert_eq!(record.cr3, TREE_PML4);
        assert_eq!(boot.board.cr3(), TREE_PML4);
        assert_eq!(boot.board.state().cr3, None);
        assert_eq!(record.set_members, ["cli", "cpu_owned_cr3", "halt"]);
    }

    #[test]
    fn set_facade_without_the_cr3_member_refuses_the_board_cr3_before_any_fetch() {
        let (mut boot, file) = boot_with_entry(&PROLOGUE);
        let members = LinuxCpl0SetMembers {
            cpu_owned_cr3: false,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit_kind, "rejected", "{record:?}");
        assert_eq!(
            record.exit,
            PagedProbeExit::Rejected(crate::PagedProbeRejection::Cpl0Set(
                od_isa::Cpl0ProfileSetInputError::Cr3NotAdmitted
            ))
        );
        assert_eq!((record.retired, record.rip), (0, ENTRY));
        assert_eq!(record.set_members, ["cli"]);
        assert_eq!(boot.board.state().cr3, None);
    }

    #[test]
    fn set_facade_budget_zero_stops_before_any_fetch() {
        let (mut boot, file) = boot_with_entry(&PROLOGUE);
        let record = run_set_facade(&mut boot, &file, 0, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!((record.retired, record.rip), (0, ENTRY));
        assert_eq!(record.page_fault, None);
        assert_eq!(
            record.set_exit_detail.as_deref(),
            Some("Run(InstructionBudgetExhausted)")
        );
    }

    // ---- S4-38L4b: a partial REP exit of the set is resumed by the board ----

    /// Qwords copied by the REP tests: three REP calls (1024 + 1024 + 452 iterations).
    const REP_QWORDS: u64 = 2500;
    const REP_SRC: u64 = DATA;
    const REP_DST: u64 = DATA + 0x10_0000;
    const REP_LIMIT: u64 = od_isa::execute::REP_ITERATION_WORK_LIMIT;

    fn rep_pattern() -> Vec<u8> {
        (0..REP_QWORDS * 8)
            .map(|index| (index.wrapping_mul(31) ^ (index >> 8)) as u8)
            .collect()
    }

    /// The board with `code` at the entry, the source filled with [`rep_pattern`], and
    /// RSI/RDI/RCX set for a forward copy (or, with `backward`, for a copy from the last qword
    /// down, the shape of the kernel's relocation).
    fn rep_boot(code: &[u8], backward: bool) -> LinuxDirectBoot {
        let (mut boot, _) = boot_with_entry(code);
        write_phys(&mut boot, REP_SRC, &rep_pattern());
        let state = boot.board.state_mut();
        let last = (REP_QWORDS - 1) * 8;
        let (rsi, rdi) = if backward {
            (REP_SRC + last, REP_DST + last)
        } else {
            (REP_SRC, REP_DST)
        };
        state.gpr[Register::Rsi as usize] = rsi;
        state.gpr[Register::Rdi as usize] = rdi;
        state.gpr[Register::Rcx as usize] = REP_QWORDS;
        boot
    }

    fn read_bytes(boot: &mut LinuxDirectBoot, address: u64, len: u64) -> Vec<u8> {
        let mut bytes = vec![0u8; len as usize];
        read(&mut boot.board, address, &mut bytes).expect("read");
        bytes
    }

    fn halt_members() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        }
    }

    #[test]
    fn set_facade_partial_rep_resumes_and_matches_the_expected_copy() {
        // REP MOVSQ; HLT, forward.
        let code = [0xF3, 0x48, 0xA5, 0xF4];
        let mut boot = rep_boot(&code, false);
        let rflags_before = boot.board.state().rflags;
        let file = bzimage_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halt_members());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 2, "the REP counts once, with the HLT");
        assert_eq!(record.quanta, 3, "1024 + 1024 + 452 iterations");
        assert_eq!(record.virtual_ns, 2);
        assert_eq!(record.rip, ENTRY + 4);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rsi as usize], REP_SRC + REP_QWORDS * 8);
        assert_eq!(record.gpr[Register::Rdi as usize], REP_DST + REP_QWORDS * 8);
        assert_eq!(record.rflags, rflags_before, "REP MOVS touches no flag");
        assert_eq!(
            read_bytes(&mut boot, REP_DST, REP_QWORDS * 8),
            rep_pattern()
        );
        assert_eq!(read_bytes(&mut boot, REP_DST + REP_QWORDS * 8, 8), [0; 8]);
        assert_eq!(record.page_fault, None);
        assert_eq!(record.memory_error, None);
    }

    #[test]
    fn set_facade_partial_backward_rep_between_std_and_cld_resumes() {
        // STD; REP MOVSQ; CLD; HLT, the shape of the kernel's backward relocation.
        let code = [0xFD, 0xF3, 0x48, 0xA5, 0xFC, 0xF4];
        let mut boot = rep_boot(&code, true);
        let file = bzimage_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halt_members());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4);
        assert_eq!(record.quanta, 3);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rsi as usize], REP_SRC.wrapping_sub(8));
        assert_eq!(record.gpr[Register::Rdi as usize], REP_DST.wrapping_sub(8));
        assert_eq!(record.rflags & (1 << 10), 0, "CLD ran after the REP");
        assert_eq!(
            read_bytes(&mut boot, REP_DST, REP_QWORDS * 8),
            rep_pattern()
        );
        assert_eq!(read_bytes(&mut boot, REP_DST - 8, 8), [0; 8]);
    }

    #[test]
    fn set_partial_rep_exits_keep_the_registers_and_the_memory_consistent() {
        // The set call itself: each partial exit leaves RIP on the REP, nothing retired, and
        // RCX/RSI/RDI/memory exactly at the iterations done; the calls chained by hand end in
        // the state of the whole copy.
        let cli = CliProfile::NonPviLong64V1;
        let halt = HaltProfile::Long64V1;
        let set = switch_set(&cli, &halt);
        let mut boot = rep_boot(&[0xF3, 0x48, 0xA5, 0xF4], false);
        let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
        let mut state = boot.board.state().clone();
        state.cr3 = Some(cr3);
        let rflags_before = state.rflags;
        let mut done = 0;
        let mut calls = 0;
        loop {
            let report = run_cpl0_set_paged_2m_ram(
                &mut state,
                boot.board.bus_mut(),
                cr3,
                efer,
                long64(),
                64,
                INTEGER_FLAGS,
                &set,
                None,
                || false,
                |_| {},
            );
            calls += 1;
            if report.exit != PagedCpl0SetExit::Run(RunExit::RepIterationBudgetExhausted) {
                assert_eq!(report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
                assert_eq!(report.instructions_executed, 2, "REP completes, then HLT");
                break;
            }
            done += REP_LIMIT;
            assert_eq!(
                report.instructions_executed, 0,
                "a partial REP retires nothing"
            );
            assert_eq!(state.rip, ENTRY);
            assert_eq!(state.gpr[Register::Rcx as usize], REP_QWORDS - done);
            assert_eq!(state.gpr[Register::Rsi as usize], REP_SRC + done * 8);
            assert_eq!(state.gpr[Register::Rdi as usize], REP_DST + done * 8);
            assert_eq!(state.rflags, rflags_before);
            assert_eq!(
                read_bytes(&mut boot, REP_DST, done * 8),
                rep_pattern()[..(done * 8) as usize]
            );
            assert_eq!(
                read_bytes(&mut boot, REP_DST + done * 8, 8),
                [0; 8],
                "no write beyond the iterations done"
            );
            assert_eq!((report.page_fault, report.memory_error), (None, None));
        }
        assert_eq!(calls, 3);
        assert_eq!(state.gpr[Register::Rcx as usize], 0);
        assert_eq!(state.gpr[Register::Rsi as usize], REP_SRC + REP_QWORDS * 8);
        assert_eq!(state.gpr[Register::Rdi as usize], REP_DST + REP_QWORDS * 8);
        assert_eq!(state.rip, ENTRY + 4);
        assert_eq!(state.rflags, rflags_before);
        assert_eq!(
            read_bytes(&mut boot, REP_DST, REP_QWORDS * 8),
            rep_pattern()
        );
    }

    #[test]
    fn set_facade_budget_spent_after_a_partial_rep_is_budget_exhausted_not_an_error() {
        // Budget 1: the REP is that one instruction. It is resumed to its end, then the budget
        // is spent: `budget_exhausted`, RIP on the HLT, the copy complete.
        let code = [0xF3, 0x48, 0xA5, 0xF4];
        let mut boot = rep_boot(&code, false);
        let file = bzimage_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 1, halt_members());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!(record.exit_kind, "budget_exhausted");
        assert_eq!((record.retired, record.rip), (1, ENTRY + 3));
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(
            read_bytes(&mut boot, REP_DST, REP_QWORDS * 8),
            rep_pattern()
        );
        // Budget 0: the REP does not start.
        let mut boot = rep_boot(&code, false);
        let record = run_set_facade(&mut boot, &file, 0, halt_members());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!((record.retired, record.rip), (0, ENTRY));
        assert_eq!(record.gpr[Register::Rcx as usize], REP_QWORDS);
        assert_eq!(read_bytes(&mut boot, REP_DST, 8), [0; 8]);
    }

    #[test]
    fn s6_21_interleaved_keeps_the_retirement_budget_after_partial_rep() {
        // REP MOVSQ; JMP $, with continuation needed after the first two REP calls.
        let code = [0xF3, 0x48, 0xA5, 0xEB, 0xFE];
        let file = bzimage_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            clock_owner: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        for budget in [1, 64] {
            let mut direct_boot = rep_boot(&code, false);
            let direct = run_set_facade(&mut direct_boot, &file, budget, members);
            assert_eq!(direct.exit, PagedProbeExit::BudgetExhausted);
            assert_eq!(direct.retired, budget);
            assert_eq!(direct.quanta, 3);

            let mut scheduled_boot = rep_boot(&code, false);
            scheduled_boot.board.enable_linux_interleaved().unwrap();
            let scheduled = run_set_facade(&mut scheduled_boot, &file, budget, members);
            assert_eq!(scheduled, direct);
            assert_eq!(
                read_bytes(&mut scheduled_boot, REP_DST, REP_QWORDS * 8),
                rep_pattern()
            );

            let mut seeded_boot = rep_boot(&code, false);
            // Seed 6 cuts the first turn after one step, before the REP can retire.
            seeded_boot
                .board
                .enable_seeded_linux_interleaved(6)
                .unwrap();
            let mut seeded = run_set_facade(&mut seeded_boot, &file, budget, members);
            seeded.quanta = direct.quanta;
            assert_eq!(seeded, direct);
            assert_eq!(seeded_boot.board.state(), direct_boot.board.state());
            assert_eq!(
                read_bytes(&mut seeded_boot, REP_DST, REP_QWORDS * 8),
                rep_pattern()
            );
        }
    }

    // S4-41c-1: the host stop request.
    fn host_stop_flag() -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false))
    }

    #[test]
    fn s4_41c_an_unset_host_stop_request_changes_nothing_the_guest_or_the_record_can_see() {
        // MOV ECX,1_500_000; L: DEC ECX; JNZ L; RDTSC; JMP $ . The clock of the board moves after a
        // quantum, so a quantum that was cut in advance would show a different RDTSC. The request
        // is only a predicate on the instruction boundary: the whole record, `quanta` included,
        // is bit for bit that of a run without a request.
        let code = [
            0xB9, 0x60, 0xE3, 0x16, 0x00, 0xFF, 0xC9, 0x75, 0xFC, 0x0F, 0x31, 0xEB, 0xFE,
        ];
        let members = LinuxCpl0SetMembers::linux_default();
        let budget = 3_100_000;
        let (mut plain_boot, file) = boot_with_entry(&code);
        let plain = run_set_facade(&mut plain_boot, &file, budget, members);
        let (mut flagged_boot, _) = boot_with_entry(&code);
        flagged_boot.board.set_host_stop(host_stop_flag());
        let flagged = run_set_facade(&mut flagged_boot, &file, budget, members);
        assert_eq!(plain.exit, PagedProbeExit::BudgetExhausted, "{plain:?}");
        assert_eq!(flagged, plain);
        assert_eq!(flagged_boot.board.state(), plain_boot.board.state());
        assert!(!flagged_boot.board.take_host_stop_honoured());
    }

    #[test]
    fn s4_41c_a_host_stop_request_set_before_the_run_ends_it_at_once() {
        let (mut boot, file) = boot_with_entry(&[0xEB, 0xFE]);
        let flag = host_stop_flag();
        boot.board.set_host_stop(flag.clone());
        flag.store(true, std::sync::atomic::Ordering::Release);
        let record = run_set_facade(&mut boot, &file, 10_000, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!(record.exit_kind, "budget_exhausted");
        assert_eq!((record.retired, record.rip), (0, ENTRY));
        assert!(boot.board.take_host_stop_honoured());
        assert!(
            !boot.board.take_host_stop_honoured(),
            "the record is cleared by the take"
        );
        // The same guest state as an explicit budget of 0 (the diagnostic text of the last quantum
        // and the quantum count may differ).
        let (mut zero_boot, _) = boot_with_entry(&[0xEB, 0xFE]);
        let zero = run_set_facade(&mut zero_boot, &file, 0, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            (
                record.exit,
                record.retired,
                record.rip,
                record.gpr,
                record.rflags
            ),
            (zero.exit, zero.retired, zero.rip, zero.gpr, zero.rflags)
        );
        assert_eq!(record.com1, zero.com1);
        assert_eq!(boot.board.state(), zero_boot.board.state());
    }

    #[test]
    fn s4_41c_a_host_stop_request_mid_run_equals_a_run_with_that_budget() {
        // MOV DX,0x3F8; MOV AL,'A'; OUT DX,AL; JMP $ : the first COM1 byte sets the request.
        let code = [0x66, 0xBA, 0xF8, 0x03, 0xB0, 0x41, 0xEE, 0xEB, 0xFE];
        let members = LinuxCpl0SetMembers {
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let (mut boot, file) = boot_with_entry(&code);
        let flag = host_stop_flag();
        boot.board.set_host_stop(flag.clone());
        let observed = flag.clone();
        boot.board.set_com1_observer(move |_| {
            observed.store(true, std::sync::atomic::Ordering::Release);
        });
        let stopped = run_set_facade(&mut boot, &file, 100_000, members);
        assert_eq!(stopped.exit, PagedProbeExit::BudgetExhausted, "{stopped:?}");
        assert!(boot.board.take_host_stop_honoured(), "{stopped:?}");
        assert!(
            stopped.retired > 0 && stopped.retired < 100_000,
            "stopped at {} of 100000",
            stopped.retired
        );
        assert_eq!(stopped.com1, b"A");

        // A fresh run with the retired count as its budget, no request and no observer: the
        // guest state, the COM1 bytes and the exit are the same.
        let (mut reference_boot, _) = boot_with_entry(&code);
        let mut reference = run_set_facade(&mut reference_boot, &file, stopped.retired, members);
        assert_eq!(
            reference.exit,
            PagedProbeExit::BudgetExhausted,
            "{reference:?}"
        );
        assert!(!reference_boot.board.take_host_stop_honoured());
        // Only the quantum count may differ: the stopped run starts one more, empty quantum after
        // the one the request ended (it executes nothing and ends on the same predicate).
        assert!(
            stopped.quanta >= reference.quanta,
            "{stopped:?} {reference:?}"
        );
        reference.quanta = stopped.quanta;
        assert_eq!(reference, stopped);
        assert_eq!(reference_boot.board.state(), boot.board.state());
    }

    #[test]
    fn s4_41c_the_request_is_honoured_under_the_seeded_one_vcpu_scheduler_too() {
        // Preset: nothing runs.
        let (mut boot, file) = boot_with_entry(&[0xEB, 0xFE]);
        boot.board.enable_seeded_linux_interleaved(6).unwrap();
        let flag = host_stop_flag();
        boot.board.set_host_stop(flag.clone());
        flag.store(true, std::sync::atomic::Ordering::Release);
        let record = run_set_facade(&mut boot, &file, 10_000, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!(record.retired, 0);
        assert!(boot.board.take_host_stop_honoured());

        // Mid-run, set by the first COM1 byte: the same state as a run with that budget.
        let code = [0x66, 0xBA, 0xF8, 0x03, 0xB0, 0x41, 0xEE, 0xEB, 0xFE];
        let members = LinuxCpl0SetMembers {
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let (mut boot, file) = boot_with_entry(&code);
        boot.board.enable_seeded_linux_interleaved(6).unwrap();
        let flag = host_stop_flag();
        boot.board.set_host_stop(flag.clone());
        let observed = flag.clone();
        boot.board.set_com1_observer(move |_| {
            observed.store(true, std::sync::atomic::Ordering::Release);
        });
        let stopped = run_set_facade(&mut boot, &file, 100_000, members);
        assert_eq!(stopped.exit, PagedProbeExit::BudgetExhausted, "{stopped:?}");
        assert!(boot.board.take_host_stop_honoured());
        assert!(
            stopped.retired > 0 && stopped.retired < 100_000,
            "{stopped:?}"
        );
        let (mut reference_boot, _) = boot_with_entry(&code);
        reference_boot
            .board
            .enable_seeded_linux_interleaved(6)
            .unwrap();
        let mut reference = run_set_facade(&mut reference_boot, &file, stopped.retired, members);
        reference.quanta = stopped.quanta;
        reference.set_exit_detail = stopped.set_exit_detail.clone();
        assert_eq!(reference, stopped);
    }

    #[test]
    fn s4_41c_a_request_set_by_another_thread_ends_a_long_run_within_one_instruction_boundary() {
        // JMP $ with a budget that would take far longer than the test: only the flag ends it, and
        // it does so inside the quantum (the whole budget is one quantum without a device).
        let (mut boot, file) = boot_with_entry(&[0xEB, 0xFE]);
        let flag = host_stop_flag();
        boot.board.set_host_stop(flag.clone());
        let setter = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            flag.store(true, std::sync::atomic::Ordering::Release);
        });
        let started = std::time::Instant::now();
        let budget = 100_000_000_000;
        let record = run_set_facade(&mut boot, &file, budget, LinuxCpl0SetMembers::minimal());
        let elapsed = started.elapsed();
        setter.join().unwrap();
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert!(boot.board.take_host_stop_honoured());
        assert!(record.retired > 0 && record.retired < budget, "{record:?}");
        assert!(elapsed < std::time::Duration::from_secs(30), "{elapsed:?}");
    }

    /// Measurement for AGENTS.md ("decisions on hot paths need numbers"): the cost of the host stop
    /// predicate on the instruction boundary. `cargo test -p od-machine --offline --release --lib
    /// s4_41c_measure_the_stop_predicate_cost -- --ignored --nocapture`. Alternates the two variants
    /// on the same scenario (JMP $, one long quantum) and prints ns per instruction for each.
    #[test]
    #[ignore = "measurement, not a check; run with --release --ignored --nocapture"]
    fn s4_41c_measure_the_stop_predicate_cost() {
        let budget = 20_000_000u64;
        let run = |with_request: bool| {
            let (mut boot, file) = boot_with_entry(&[0xEB, 0xFE]);
            if with_request {
                boot.board.set_host_stop(host_stop_flag());
            }
            let started = std::time::Instant::now();
            let record = run_set_facade(&mut boot, &file, budget, LinuxCpl0SetMembers::minimal());
            assert_eq!(record.retired, budget);
            started.elapsed().as_secs_f64() * 1e9 / budget as f64
        };
        run(false); // warm up
        let (mut without, mut with) = (Vec::new(), Vec::new());
        for _ in 0..7 {
            without.push(run(false));
            with.push(run(true));
        }
        let median = |v: &mut Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let (w0, w1) = (median(&mut without), median(&mut with));
        eprintln!(
            "ns/instruction without request {w0:.3} (min {:.3} max {:.3}); with request {w1:.3} (min {:.3} max {:.3}); ratio {:.4}",
            without[0],
            without[6],
            with[0],
            with[6],
            w1 / w0
        );
    }

    #[test]
    fn s6_21_seed_controls_the_linux_quantum_cuts() {
        let run = |seed| {
            let (mut boot, file) = boot_with_entry(&[0xEB, 0xFE]); // JMP $
            boot.board.enable_seeded_linux_interleaved(seed).unwrap();
            run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal())
        };
        // The first draws are 176 for seed 0, and 1 then 154 for seed 6.
        let one_quantum = run(0);
        let mut two_quanta = run(6);
        assert_eq!(one_quantum.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(one_quantum.retired, 64);
        assert_eq!((one_quantum.quanta, two_quanta.quanta), (1, 2));
        assert_eq!(run(6), two_quanta);
        two_quanta.quanta = one_quantum.quanta;
        assert_eq!(one_quantum, two_quanta);
    }

    #[test]
    fn set_facade_guest_stop_after_a_resumed_rep_sees_the_finished_copy() {
        // MOV DX, ACPI_RESET_REG; MOV AL, ACPI_RESET_VALUE; REP MOVSQ; OUT DX, AL; HLT.
        let code = [
            0x66, 0xBA, 0x0C, 0x06, 0xB0, 0x06, 0xF3, 0x48, 0xA5, 0xEE, 0xF4,
        ];
        let mut boot = rep_boot(&code, false);
        let file = bzimage_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            halt: true,
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Stopped, "{record:?}");
        assert_eq!(record.exit_kind, "stopped");
        assert_eq!(
            record.retired, 4,
            "the stop is after the OUT; the HLT never runs"
        );
        assert_eq!(record.rip, ENTRY + 10);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(
            read_bytes(&mut boot, REP_DST, REP_QWORDS * 8),
            rep_pattern()
        );
    }

    #[test]
    fn paged_probe_path_still_refuses_the_rep_form() {
        // Outside the set nothing changed: the base profile does not admit REP MOVSQ.
        let code = [0xF3, 0x48, 0xA5, 0xF4];
        let mut boot = rep_boot(&code, false);
        let file = bzimage_with_entry(&code);
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 64)
            .expect("runs");
        assert_eq!(record.path, "paged_probe");
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xA5 });
        assert_eq!((record.retired, record.rip), (0, ENTRY));
        assert_eq!(record.gpr[Register::Rcx as usize], REP_QWORDS);
    }

    #[test]
    fn set_facade_com1_through_the_bus_needs_the_pio_member() {
        // MOV AL,'X'; MOV DX,0x3F8; OUT DX,AL; HLT behind CLI.
        let mut code = vec![0xFA];
        code.extend_from_slice(&com1_program(b'X'));
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            halt: true,
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.com1, b"X");
        assert_eq!(record.retired, 5);
        assert!(record.quanta >= 2, "the THR write ends the quantum");
        // Without the member the OUT is the stop and no byte reaches the bus.
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0xEE },
            "{record:?}"
        );
        assert_eq!((record.retired, record.rip), (3, ENTRY + 1 + 6));
        assert!(record.com1.is_empty());
    }

    /// S4-38L7x: `MOV EAX, 0x123456AB; MOV EDX, 0x3F9; IN AL, DX; HLT` (the S4-38L7w boundary:
    /// a byte read of COM1 IER). AL starts non-zero (0xAB), so a missing write of AL fails the test;
    /// `IN AL, DX` leaves EAX bits 8..32 alone.
    fn in_al_com1_ier_code() -> Vec<u8> {
        let mut code = mov_eax(0x1234_56AB);
        code.extend(mov_edx(0x3f9));
        code.extend([0xEC, 0xF4]);
        code
    }

    #[test]
    fn set_facade_in_al_dx_on_com1_ier_runs_through_the_bus_in_the_linux_default() {
        let code = in_al_com1_ier_code();
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "MOV, MOV, IN, HLT: {record:?}");
        assert!(record.set_members.contains(&"pio_u8".to_string()));
        // The byte is whatever the board's COM1 gives for IER (a fresh UART: zero).
        assert_eq!(
            record.gpr[Register::Rax as usize],
            0x1234_5600,
            "{record:?}"
        );
        assert_eq!(record.gpr[Register::Rdx as usize], 0x3f9);
        assert!(record.com1.is_empty());
    }

    #[test]
    fn set_facade_in_al_dx_is_unsupported_without_the_pio_member() {
        let code = in_al_com1_ier_code();
        let (mut boot, file) = boot_with_entry(&code);
        let rip_in = boot.board.state().rip + 10;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                pio_u8: false,
                ..halting_default()
            },
        );
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0xEC },
            "{record:?}"
        );
        assert_eq!((record.retired, record.rip), (2, rip_in));
        assert_eq!(record.gpr[Register::Rax as usize], 0x1234_56AB);
        assert!(!record.set_members.contains(&"pio_u8".to_string()));
        // The minimal set has no PIO either.
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xEC });
    }

    #[test]
    fn set_facade_a_port_error_is_the_port_io_exit_with_rip_on_the_instruction() {
        // CLI; MOV AL, 1; MOV DX, 0x1234; OUT DX, AL; HLT. Nothing is mapped at 0x1234, so the
        // bus refuses the port before the OUT commits: three instructions retired, RIP on the OUT.
        let (mut boot, file) =
            boot_with_entry(&[0xFA, 0xB0, 0x01, 0x66, 0xBA, 0x34, 0x12, 0xEE, 0xF4]);
        let board_cr3 = boot.board.cr3();
        let members = LinuxCpl0SetMembers {
            halt: true,
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        let error = crate::BusAccessError::UnmappedPort(crate::PortAddress::new(0x1234));
        assert_eq!(record.exit, PagedProbeExit::PortIo(error), "{record:?}");
        assert_eq!(record.exit_kind, "port_io");
        assert_eq!(
            record.exit_detail,
            "PortIo(UnmappedPort(PortAddress(4660)))"
        );
        assert_eq!(
            record.set_exit_detail.as_deref(),
            Some("PortIo(UnmappedPort(PortAddress(4660)))")
        );
        assert_eq!((record.retired, record.rip), (3, ENTRY + 7));
        assert_eq!(record.virtual_ns, 3, "only the retired instructions");
        assert!(record.com1.is_empty());
        assert_eq!((record.cr3_changes, record.cr3), (0, board_cr3));
        assert_eq!(boot.board.state().cr3, None);
    }

    /// Host programming of PIT channel 2 (gate high, MSB-only mode 0, reload 0x1000): the
    /// counter port 0x42 answers only once it is programmed. Host writes never tick.
    fn program_pit_channel_2(boot: &mut LinuxDirectBoot) {
        for (port, value) in [(0x61u16, 0x01u32), (0x43, 0xA0), (0x42, 0x10)] {
            boot.board
                .bus_mut()
                .write_port(
                    crate::PortAddress::new(port),
                    crate::AccessWidth::Byte,
                    value,
                )
                .expect("host port write");
        }
    }

    #[test]
    fn set_facade_timer_port_reads_tick_the_clock_once_each_like_run_boot() {
        // MOV DX,0x61; 3 x IN AL,DX (PIT port B); MOV DX,0x42; 2 x IN (PIT counter);
        // MOV DX,0x608; 2 x IN (PM timer); HLT. Seven timer reads, 1 us each, and no more: the
        // pio member reaches the devices through the bus and the board applies the queued ticks
        // once per quantum, as `run_boot` does for the same bytes. Every read ends the quantum.
        let code = [
            0x66, 0xBA, 0x61, 0x00, 0xEC, 0xEC, 0xEC, 0x66, 0xBA, 0x42, 0x00, 0xEC, 0xEC, 0x66,
            0xBA, 0x08, 0x06, 0xEC, 0xEC, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        program_pit_channel_2(&mut boot);
        let base = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 64)
            .expect("runs");
        assert_eq!(base.exit, PagedProbeExit::Halted, "{base:?}");
        assert_eq!((base.retired, base.quanta), (11, 8));
        assert_eq!(base.virtual_ns, 11 + 7 * 1_000);
        // The same bytes behind CLI through the set: one more instruction, the same seven ticks.
        let mut set_code = vec![0xFA];
        set_code.extend_from_slice(&code);
        let (mut boot, file) = boot_with_entry(&set_code);
        program_pit_channel_2(&mut boot);
        let members = LinuxCpl0SetMembers {
            halt: true,
            pio_u8: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!((record.retired, record.quanta), (12, 8));
        assert_eq!(record.virtual_ns, 12 + 7 * 1_000);
        assert_eq!(boot.board.clock().now_ns(), record.virtual_ns);
        let (mut scheduled, file) = boot_with_entry(&set_code);
        program_pit_channel_2(&mut scheduled);
        scheduled.board.enable_linux_interleaved().unwrap();
        let mut scheduled_record = run_set_facade(&mut scheduled, &file, 64, members);
        scheduled_record
            .set_members
            .retain(|name| name != "clock_owner");
        assert_eq!(scheduled_record, record);
        assert_eq!(
            record.virtual_ns - base.virtual_ns,
            1,
            "the CLI is the only difference"
        );
    }

    #[test]
    fn s6_21_standalone_linux_keeps_the_direct_clock_overflow_prefix() {
        for (initial_ns, expected_pending_tick) in [(u64::MAX, 1_000), (u64::MAX - 1, 0)] {
            for profile in [CpuidProfile::DiagnosticV1, CpuidProfile::X8664V1Stage4A] {
                let (mut boot, file) = boot_with_entry(&[0xEC, 0xF4]); // IN AL,DX; HLT
                boot.board.state_mut().gpr[Register::Rdx as usize] = 0x608;
                boot.board.advance_time_ns(initial_ns).unwrap();
                let error = run_linux_direct_board_set_user_mode(
                    &mut boot,
                    &input(&file, &[], BLOB, board_hash()),
                    64,
                    LinuxCpl0SetMembers {
                        cpuid_profile: profile,
                        pio_u8: true,
                        ..LinuxCpl0SetMembers::minimal()
                    },
                )
                .unwrap_err();
                assert_eq!(error.code, "run_failed");
                assert_eq!(error.retired, Some(1));
                assert!(error.detail.contains("TimeOverflow"), "{error:?}");
                assert_eq!(boot.board.clock().now_ns(), u64::MAX);
                assert_eq!(
                    boot.board.acpi_pm().unwrap().take_timer_tick_ns(),
                    expected_pending_tick
                );
            }
        }
    }

    fn host_read_port(boot: &mut LinuxDirectBoot, port: u16) -> u32 {
        boot.board
            .bus_mut()
            .read_port(crate::PortAddress::new(port), crate::AccessWidth::Byte)
            .expect("host port read")
    }

    #[test]
    fn set_facade_out_imm8_al_programs_pit_channel_2_through_the_bus() {
        // S4-38L7bk. CLI; MOV AL,0x01; OUT 0x61,AL (gate high); MOV AL,0xA0; OUT 0x43,AL
        // (channel 2, MSB only, mode 0); MOV AL,0x10; OUT 0x42,AL (reload 0x1000); HLT. The same
        // programming by the host gives the reference: the guest reaches the PIT only through
        // the three `E6 ib` writes (RDX stays 0, so no DX form is involved).
        let code = [
            0xFA, 0xB0, 0x01, 0xE6, 0x61, 0xB0, 0xA0, 0xE6, 0x43, 0xB0, 0x10, 0xE6, 0x42, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 8, "{record:?}");
        assert_eq!(record.rip, ENTRY + 14, "after the HLT");
        assert_eq!(record.gpr[Register::Rax as usize] & 0xFF, 0x10);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        // Port B bit 0 (the gate) was written through port 0x61.
        assert_eq!(host_read_port(&mut boot, 0x61) & 1, 1);
        // The channel 2 counter answers now (MSB-only mode 0, reload 0x1000): its MSB is the
        // value the host programming gives on a twin board.
        let guest_counter = host_read_port(&mut boot, 0x42);
        let (mut twin, _) = boot_with_entry(&code);
        program_pit_channel_2(&mut twin);
        assert_eq!(guest_counter, host_read_port(&mut twin, 0x42));
        assert_eq!(guest_counter, 0x10, "reload 0x1000, MSB only");
    }

    #[test]
    fn set_facade_in_al_imm8_reads_the_pit_through_the_bus_and_ticks_once() {
        // S4-38L7bk. MOV EAX,0x123456AB; IN AL,0x42; HLT with channel 2 programmed by the host:
        // AL is the byte the board gives (the same as `IN AL,DX` with DX = 0x42), RAX[63:8] is
        // kept and RDX is not touched. The PIT read ticks the board clock once, like the DX form.
        let mut code = vec![0xFA];
        code.extend(mov_eax(0x1234_56AB));
        code.extend([0xE4, 0x42, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        program_pit_channel_2(&mut boot);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.rip, ENTRY + 1 + 5 + 2 + 1);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.gpr[Register::Rax as usize] & !0xFF, 0x1234_5600);
        assert_eq!(record.virtual_ns, 4 + 1_000, "one timer read, 1 us");
        // The DX form on a twin board reads the same byte.
        let mut dx_code = vec![0xFA];
        dx_code.extend(mov_eax(0x1234_56AB));
        dx_code.extend(mov_edx(0x42));
        dx_code.extend([0xEC, 0xF4]);
        let (mut twin, twin_file) = boot_with_entry(&dx_code);
        program_pit_channel_2(&mut twin);
        let twin_record = run_set_facade(&mut twin, &twin_file, 64, halting_default());
        assert_eq!(twin_record.exit, PagedProbeExit::Halted, "{twin_record:?}");
        assert_eq!(
            record.gpr[Register::Rax as usize],
            twin_record.gpr[Register::Rax as usize]
        );
    }

    #[test]
    fn set_facade_out_in_imm8_are_unsupported_without_the_pio_member() {
        for (code, opcode) in [
            (vec![0xFA, 0xB0, 0x01, 0xE6, 0x61, 0xF4], 0xE6u8),
            (vec![0xFA, 0xB0, 0x01, 0xE4, 0x61, 0xF4], 0xE4),
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(
                &mut boot,
                &file,
                64,
                LinuxCpl0SetMembers {
                    pio_u8: false,
                    ..halting_default()
                },
            );
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!((record.retired, record.rip), (2, ENTRY + 3));
            assert_eq!(record.gpr[Register::Rax as usize] & 0xFF, 0x01);
            assert_eq!(record.virtual_ns, 2, "no port access, no tick");
            assert_eq!(host_read_port(&mut boot, 0x61) & 1, 0, "port B untouched");
        }
    }

    #[test]
    fn set_facade_out_imm8_to_an_unmapped_port_is_the_port_io_exit_with_rip_on_the_instruction() {
        // CLI; MOV AL,1; OUT 0x44,AL; HLT. Nothing is bound at 0x44 (the PIT owns 0x40..=0x43).
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xB0, 0x01, 0xE6, 0x44, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        let error = crate::BusAccessError::UnmappedPort(crate::PortAddress::new(0x44));
        assert_eq!(record.exit, PagedProbeExit::PortIo(error), "{record:?}");
        assert_eq!((record.retired, record.rip), (2, ENTRY + 3));
    }

    #[test]
    fn set_facade_mov_rax_cr3_returns_the_board_cr3_under_the_owned_cr3_member() {
        // S4-38L4c: CLI; MOV RAX, CR3; HLT (and the REX.W spelling). `cpu_owned_cr3` is part of
        // every facade set (also `minimal()`), so no extra member is needed.
        for read in [&[0x0F, 0x20, 0xD8][..], &[0x48, 0x0F, 0x20, 0xD8][..]] {
            let mut code = vec![0xFA];
            code.extend_from_slice(read);
            code.push(0xF4);
            let (mut boot, file) = boot_with_entry(&code);
            let board_cr3 = boot.board.cr3();
            let members = LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            };
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
            assert_eq!(record.retired, 3, "{record:?}");
            assert_eq!(record.gpr[Register::Rax as usize], board_cr3);
            assert_eq!(record.cr3, board_cr3);
            assert_eq!(record.cr3_changes, 0);
            assert!(record.set_members.contains(&"cpu_owned_cr3".to_string()));
        }
        // The default Linux set keeps the owning member, so the read is available there too.
        assert!(LinuxCpl0SetMembers::linux_default().cpu_owned_cr3);
        // Without CR3 ownership the set is rejected at admission (the board CR3 is modeled), so
        // the read cannot execute.
        let (mut boot, file) = boot_with_entry(&[0xFA, 0x0F, 0x20, 0xD8, 0xF4]);
        let members = LinuxCpl0SetMembers {
            cpu_owned_cr3: false,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit_kind, "rejected", "{record:?}");
        assert_eq!(record.retired, 0);
    }

    #[test]
    fn set_facade_cr4_read_and_the_cpuid_msr_rdtsc_members() {
        // CLI; MOV RAX, CR4; XOR EAX, EAX; CPUID(0); HLT.
        let (mut boot, file) =
            boot_with_entry(&[0xFA, 0x0F, 0x20, 0xE0, 0x31, 0xC0, 0x0F, 0xA2, 0xF4]);
        let members = LinuxCpl0SetMembers {
            cpuid: true,
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.gpr[Register::Rax as usize],
            1,
            "DiagnosticV1 leaf 0 maximum"
        );
        assert_eq!(
            record.gpr[Register::Rbx as usize],
            u64::from(u32::from_le_bytes(*b"Open"))
        );
        assert_eq!(
            record.set_members,
            ["cli", "cpu_owned_cr3", "cpuid:DiagnosticV1", "halt"]
        );
        // The same bytes without the CPUID member stop at it.
        let (mut boot, file) =
            boot_with_entry(&[0xFA, 0x0F, 0x20, 0xE0, 0x31, 0xC0, 0x0F, 0xA2, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert!(
            matches!(record.exit, PagedProbeExit::Unsupported { .. }),
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        // CLI; MOV ECX, 0xC0000080; RDMSR; HLT: the EFER member reads the board EFER (0xD00)
        // into EDX:EAX.
        let msr_code = [0xFA, 0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32, 0xF4];
        let (mut boot, file) = boot_with_entry(&msr_code);
        let members = LinuxCpl0SetMembers {
            msr: true,
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4);
        assert_eq!(record.gpr[Register::Rax as usize], 0xD00);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0080);
        assert_eq!(
            record.set_members,
            [
                "cli".to_string(),
                "cpu_owned_cr3".to_string(),
                format!("msr:{:?}", MsrProfile::EferReadOnlyV1(0xD00)),
                "halt".to_string()
            ]
        );
        // Without the member RDMSR is the stop (after CLI and the MOV).
        let (mut boot, file) = boot_with_entry(&msr_code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x32 },
            "{record:?}"
        );
        assert_eq!(record.retired, 2);
        // CLI; RDTSC; HLT: the tick of the board clock is applied like in `run_boot`.
        let (mut boot, file) = boot_with_entry(&[0xFA, 0x0F, 0x31, 0xF4]);
        let members = LinuxCpl0SetMembers {
            rdtsc: true,
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!((record.retired, record.quanta), (3, 2));
        assert_eq!(record.virtual_ns, 3 + 1_000);
        // Without the member RDTSC is the stop.
        let (mut boot, file) = boot_with_entry(&[0xFA, 0x0F, 0x31, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x31 },
            "{record:?}"
        );
    }

    // ---- S4-38L7bh: RDTSC in the Linux board set ----

    #[test]
    fn rdtsc_runs_through_the_linux_board_set_and_ticks_the_virtual_clock() {
        // RDTSC; MOV RBX, RAX; RDTSC; MOV RCX, RAX; RDTSC; HLT, through the board set of the
        // runner (`linux_default`, plus `halt` for the stop). Each RDTSC ends its quantum after
        // one instruction; the clock moves 1 ns per retired instruction and 1000 ns per RDTSC
        // (the tick is applied after the quantum, the read is at its start). tsc_hz is 1 GHz,
        // so the TSC value is the clock in ns:
        //   q1: RDTSC reads 0                         -> clock 1 + 1000 = 1001
        //   q2: MOV, RDTSC reads 1001                 -> clock 1001 + 2 + 1000 = 2003
        //   q3: MOV, RDTSC reads 2003                 -> clock 2003 + 2 + 1000 = 3005
        //   q4: HLT                                   -> clock 3006
        let code = [
            0x0F, 0x31, 0x48, 0x89, 0xC3, 0x0F, 0x31, 0x48, 0x89, 0xC1, 0x0F, 0x31, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert!(record.set_members.contains(&"rdtsc".to_string()));
        assert_eq!((record.retired, record.quanta), (6, 4));
        assert_eq!(record.virtual_ns, 3_006);
        assert_eq!(record.gpr[Register::Rbx as usize], 0);
        assert_eq!(record.gpr[Register::Rcx as usize], 1_001);
        assert_eq!(record.gpr[Register::Rax as usize], 2_003);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.tsc_hz, 1_000_000_000);
        let (mut scheduled, file) = boot_with_entry(&code);
        scheduled.board.enable_linux_interleaved().unwrap();
        let mut scheduled_record = run_set_facade(&mut scheduled, &file, 64, members);
        scheduled_record
            .set_members
            .retain(|name| name != "clock_owner");
        assert_eq!(scheduled_record, record);
    }

    #[test]
    fn rdtsc_stays_refused_with_0x31_when_the_member_is_off() {
        // The board set without the member: the bare RDTSC is the stop, nothing retired, the
        // clock does not tick for it.
        let (mut boot, file) = boot_with_entry(&[0x0F, 0x31, 0xF4]);
        let members = LinuxCpl0SetMembers {
            rdtsc: false,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x31 },
            "{record:?}"
        );
        assert_eq!((record.retired, record.virtual_ns), (0, 0));
        assert!(!record.set_members.contains(&"rdtsc".to_string()));
        // The minimal set refuses it as well.
        let (mut boot, file) = boot_with_entry(&[0x0F, 0x31, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x31 },
            "{record:?}"
        );
        assert_eq!(record.retired, 0);
    }

    // ---- S4-38L6b: WRMSR/RDMSR IA32_GS_BASE through the board facade ----

    #[test]
    fn wrmsr_gs_base_runs_through_the_board_set_and_is_gated_by_its_member() {
        // CLI; MOV ECX,0xC0000101; XOR EAX,EAX; XOR EDX,EDX; WRMSR; RDMSR; HLT (the kernel's
        // GS_BASE clear, then a read back).
        let code = [
            0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0, 0x31, 0xC0, 0x31, 0xD2, 0x0F, 0x30, 0x0F, 0x32,
            0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "CLI, MOV, XOR, XOR, WRMSR, RDMSR, HLT");
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0101);
        assert_eq!(record.gpr[Register::Rax as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(boot.board.state().gs_base, 0);
        assert!(record.set_members.contains(&"msr:fs_gs_base".to_string()));

        // Without the member (`minimal`) WRMSR is the stop, after CLI, MOV, XOR and XOR.
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 4);
        assert!(
            !record
                .set_members
                .iter()
                .any(|name| name.starts_with("msr"))
        );
        assert!(!LinuxCpl0SetMembers::minimal().msr_fs_gs_base);
        assert!(LinuxCpl0SetMembers::linux_default().msr_fs_gs_base);
    }

    // ---- S4-38L7g: FS/GS-based qword store through the board facade ----

    /// `MOV RAX, imm64`.
    fn mov_rax_imm64(value: u64) -> Vec<u8> {
        let mut code = vec![0x48, 0xB8];
        code.extend(value.to_le_bytes());
        code
    }

    fn guest_qword(boot: &mut LinuxDirectBoot, address: u64) -> u64 {
        let mut bytes = [0u8; 8];
        read(&mut boot.board, address, &mut bytes).expect("read");
        u64::from_le_bytes(bytes)
    }

    #[test]
    fn gs_store_after_wrmsr_gs_base_writes_at_base_plus_effective_address() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV %rax,%gs:0x10 (absolute); MOV %rax,%gs:disp(%rip) landing on effective address
        // 0x40; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x65, 0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x40u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x48, 0x89, 0x05]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 9,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, 2 stores, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().gs_base, BASE);
        assert_eq!(guest_qword(&mut boot, BASE + 0x10), VALUE);
        assert_eq!(guest_qword(&mut boot, BASE + 0x40), VALUE);
    }

    #[test]
    fn fs_and_gs_stores_with_a_zero_base_write_at_the_effective_address() {
        const VALUE: u64 = 0x0FED_CBA9_8765_4321;
        // MOV RAX,VALUE; MOV %rax,%gs:0x0200_0020; MOV %rax,%fs:0x0200_0028; HLT (no CLI).
        let mut code = mov_rax_imm64(VALUE);
        code.extend([0x65, 0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x64, 0x48, 0x89, 0x04, 0x25, 0x28, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4);
        assert_eq!(guest_qword(&mut boot, 0x0200_0020), VALUE);
        assert_eq!(guest_qword(&mut boot, 0x0200_0028), VALUE);
    }

    // ---- S4-38L7i: FS/GS-based qword load through the board facade ----

    #[test]
    fn gs_load_after_wrmsr_gs_base_reads_guest_memory_at_base_plus_effective_address() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); MOV %gs:disp(%rip),%r12 landing on effective
        // address 0x10 (the r12 destination is the Linux boundary `65 4c 8b 25`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x4C, 0x8B, 0x25]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 9,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().gs_base, BASE);
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::R12 as usize],
            VALUE
        );
    }

    #[test]
    fn fs_and_gs_loads_with_a_zero_base_read_at_the_effective_address() {
        const VALUE: u64 = 0x0FED_CBA9_8765_4321;
        // MOV RAX,VALUE; MOV [0x0200_0020],RAX; MOV %gs:0x0200_0020,%r13;
        // MOV %fs:0x0200_0020,%r14; HLT (no CLI).
        let mut code = mov_rax_imm64(VALUE);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x65, 0x4C, 0x8B, 0x2C, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x64, 0x4C, 0x8B, 0x34, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5);
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::R13 as usize],
            VALUE
        );
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::R14 as usize],
            VALUE
        );
    }

    // ---- S4-38L7m: FS/GS-based dword load through the board facade ----

    #[test]
    fn gs_dword_load_after_wrmsr_gs_base_reads_four_bytes_and_zero_extends() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); MOV R12,-1; MOV %gs:disp(%rip),%eax landing
        // on effective address 0x10 (the Linux boundary form `65 8b 05`); MOV %gs:0x10,%r12d
        // (`65 44 8b 24 25`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x49, 0xBC]);
        code.extend(u64::MAX.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 7;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x8B, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0x65, 0x44, 0x8B, 0x24, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 11,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, MOV r12, load, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the low dword of the qword at the effective address is read; the upper 32 bits
        // of both destinations are zero (RAX held VALUE, R12 held all ones).
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x89AB_CDEF);
        assert_eq!(state.gpr[od_isa::Register::R12 as usize], 0x89AB_CDEF);
    }

    #[test]
    fn fs_and_gs_dword_loads_with_a_zero_base_read_at_the_effective_address() {
        const VALUE: u64 = 0x0FED_CBA9_8765_4321;
        // MOV RAX,VALUE; MOV [0x0200_0020],RAX; MOV R13,-1; MOV R14,-1;
        // MOV %gs:0x0200_0020,%r13d; MOV %fs:0x0200_0020,%r14d; HLT (no CLI).
        let mut code = mov_rax_imm64(VALUE);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x49, 0xBD]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0x49, 0xBE]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0x65, 0x44, 0x8B, 0x2C, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x64, 0x44, 0x8B, 0x34, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7);
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::R13 as usize],
            0x8765_4321
        );
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::R14 as usize],
            0x8765_4321
        );
    }

    // ---- S4-38L7j: FS/GS-based qword SUB through the board facade ----

    #[test]
    fn gs_sub_after_wrmsr_gs_base_subtracts_guest_memory_at_base_plus_effective_address() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,58;
        // MOV [BASE+0x10],RAX (plain, no override); MOV R12,100; SUB %gs:disp(%rip),%r12 landing
        // on effective address 0x10 (the Linux boundary form `65 4c 2b 25`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(58));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x49, 0xBC]);
        code.extend(100u64.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x4C, 0x2B, 0x25]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 10,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, MOV r12, SUB, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().gs_base, BASE);
        assert_eq!(boot.board.state().gpr[od_isa::Register::R12 as usize], 42);
    }

    #[test]
    fn fs_and_gs_sub_with_a_zero_base_subtract_at_the_effective_address() {
        // MOV RAX,5; MOV [0x0200_0020],RAX; MOV RAX,100; SUB %gs:0x0200_0020,%rax;
        // SUB %fs:0x0200_0020,%rax; HLT (no CLI).
        let mut code = mov_rax_imm64(5);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend(mov_rax_imm64(100));
        code.extend([0x65, 0x48, 0x2B, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x64, 0x48, 0x2B, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6);
        assert_eq!(boot.board.state().gpr[od_isa::Register::Rax as usize], 90);
    }

    // ---- S4-38L7aa: FS/GS-based CMP m64, r64 through the board facade ----

    #[test]
    fn gs_cmp_after_wrmsr_gs_base_sets_flags_and_does_not_write_guest_memory() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,INT64_MIN;
        // MOV [BASE+0x10],RAX (plain); MOV RDX,1; CMP %rdx,%gs:disp(%rip) landing on effective
        // address 0x10 (`65 48 39 15`, the Linux boundary form); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(0x8000_0000_0000_0000));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBA]);
        code.extend(1u64.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x48, 0x39, 0x15]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 10,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, MOV rdx, CMP, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().gs_base, BASE);
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 1);
        // INT64_MIN - 1: OF, PF, AF; no CF, ZF or SF.
        assert_eq!(
            state.rflags & 0x8d5,
            (1 << 11) | (1 << 2) | (1 << 4),
            "{:#x}",
            state.rflags
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x8000_0000_0000_0000,
            "CMP does not write"
        );
    }

    // ---- S4-38L7az: FS/GS-based OR m32, r32 and m64, r64 through the board facade ----

    #[test]
    fn gs_or_dword_after_wrmsr_gs_base_ors_edx_into_guest_memory_at_the_boundary_form() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,cell;
        // MOV [BASE+0x10],RAX (plain); MOV EDX,0xf; OR %edx,%gs:disp(%rip) landing on effective
        // address 0x10 (`65 09 15`, the S4-38L7ay Linux boundary form); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(0xdead_beef_0000_00f0));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0xBA, 0x0F, 0x00, 0x00, 0x00]);
        let next_rip = ENTRY + code.len() as u64 + 7;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x09, 0x15]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 10,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, MOV edx, OR, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let cell = guest_qword(&mut boot, BASE + 0x10);
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0x0f,
            "source kept"
        );
        // 0xf0 | 0xf = 0xff in the low dword; the upper dword is not touched.
        assert_eq!(
            cell, 0xdead_beef_0000_00ff,
            "only the low dword is rewritten"
        );
        // 0xff: PF only (CF, OF, SF, ZF cleared).
        assert_eq!(state.rflags & 0x8c5, 1 << 2, "{:#x}", state.rflags);
    }

    #[test]
    fn fs_or_qword_after_wrmsr_fs_base_ors_rdx_into_guest_memory() {
        const BASE: u64 = 0x0200_0000;
        // As above with the FS base (MSR 0xC0000100) and `64 48 09 15` (OR %rdx,%fs:disp(%rip)).
        let mut code = vec![0xFA, 0xB9, 0x00, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(0x8000_0000_0000_0001));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBA]);
        code.extend(2u64.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x64, 0x48, 0x09, 0x15]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 10, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let cell = guest_qword(&mut boot, BASE + 0x10);
        let state = boot.board.state();
        assert_eq!(state.fs_base, BASE);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 2, "source kept");
        assert_eq!(cell, 0x8000_0000_0000_0003);
        // 0x8000_0000_0000_0003: SF and PF (two ones in the low byte).
        assert_eq!(
            state.rflags & 0x8c5,
            (1 << 7) | (1 << 2),
            "{:#x}",
            state.rflags
        );
    }

    // ---- S4-38L7bc: FS/GS-based ADD m32, r32 and m64, r64 through the board facade ----

    /// CLI; WRMSR `msr` = `BASE` (the used base) and the other base MSR = `DISTRACTOR`; guest
    /// qwords `before`/`cell`/`after` at `BASE + 0x8`/`0x10`/`0x18`; MOV RAX,`addend`; `opcode`
    /// (`65 48 01 05`, ...) with a disp32 landing on `BASE + 0x10`; HLT. Returns the board, the run
    /// record and the number of instructions up to and including the HLT.
    fn run_segment_add(
        used_msr: u32,
        other_msr: u32,
        opcode: &[u8],
        guest: [u64; 3],
        addend: u64,
    ) -> (LinuxDirectBoot, LinuxStopRecord, u64) {
        const BASE: u64 = 0x0200_0000;
        const DISTRACTOR: u32 = 0x0300_0000;
        let mut code = vec![0xFA, 0xB9];
        code.extend(used_msr.to_le_bytes());
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9]);
        code.extend(other_msr.to_le_bytes());
        code.extend(mov_eax(DISTRACTOR));
        code.extend([0x0F, 0x30]);
        for (offset, value) in [(0x8u8, guest[0]), (0x10, guest[1]), (0x18, guest[2])] {
            code.extend(mov_rax_imm64(value));
            code.extend([0x48, 0x89, 0x04, 0x25, offset, 0x00, 0x00, 0x02]);
        }
        code.extend(mov_rax_imm64(addend));
        // ADD: the prefix bytes of `opcode` (all but the trailing ModRM 0x05) + ModRM + disp32.
        let next_rip = ENTRY + code.len() as u64 + opcode.len() as u64 + 4;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend(opcode);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        // CLI, MOV ECX, MOV EAX, XOR, WRMSR, MOV ECX, MOV EAX, WRMSR, 3 x (MOV RAX, store),
        // MOV RAX, ADD, HLT.
        (boot, record, 8 + 6 + 1 + 1 + 1)
    }

    #[test]
    fn gs_add_qword_after_wrmsr_gs_base_adds_rax_into_guest_memory_at_the_boundary_form() {
        const BASE: u64 = 0x0200_0000;
        // `65 48 01 05 disp32` = `addq %rax, %gs:disp(%rip)` (the S4-38L7bb Linux boundary form),
        // GS base = BASE and a distractor FS base. 0x7fff_ffff_ffff_fff0 + 0x10 overflows to the
        // sign bit.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0101,
            0xC000_0100,
            &[0x65, 0x48, 0x01, 0x05],
            [
                0x1111_1111_1111_1111,
                0x7fff_ffff_ffff_fff0,
                0x2222_2222_2222_2222,
            ],
            0x10,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x10), 0x8000_0000_0000_0000);
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.fs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x10,
            "source kept"
        );
        // 0x8000_0000_0000_0000: OF, SF and PF (zero low byte); no CF, AF, ZF.
        assert_eq!(
            state.rflags & 0x8d5,
            (1 << 11) | (1 << 7) | (1 << 2),
            "{:#x}",
            state.rflags
        );
    }

    #[test]
    fn fs_add_dword_after_wrmsr_fs_base_wraps_the_low_dword_and_keeps_the_upper_half() {
        const BASE: u64 = 0x0200_0000;
        // `64 01 05 disp32` = `addl %eax, %fs:disp(%rip)`, FS base = BASE and a distractor GS
        // base. The low dword 0xffff_fff0 + 0x20 wraps to 0x10 with CF; the upper dword stays.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0100,
            0xC000_0101,
            &[0x64, 0x01, 0x05],
            [
                0x1111_1111_1111_1111,
                0xdead_beef_ffff_fff0,
                0x2222_2222_2222_2222,
            ],
            0xaaaa_bbbb_0000_0020,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x10), 0xdead_beef_0000_0010);
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.fs_base, BASE);
        assert_eq!(
            state.gs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0xaaaa_bbbb_0000_0020,
            "source kept"
        );
        // 0x10: CF only (one bit set, so no PF; no AF, ZF, SF, OF).
        assert_eq!(state.rflags & 0x8d5, 1, "{:#x}", state.rflags);
    }

    // ---- S4-38L7bd: FS/GS-based ADD r32, m32 and r64, m64 through the board facade ----
    // The destination is RAX (`run_segment_add` loads RAX last); the memory qword is only read.

    #[test]
    fn gs_add_load_qword_after_wrmsr_gs_base_adds_guest_memory_into_rax_and_leaves_it_unchanged() {
        const BASE: u64 = 0x0200_0000;
        // `65 48 03 05 disp32` = `add %gs:disp(%rip), %rax` (the S4-38L7bc Linux boundary form
        // with RAX as the destination), GS base = BASE and a distractor FS base.
        // RAX 0x10 + 0x7fff_ffff_ffff_fff0 overflows to the sign bit.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0101,
            0xC000_0100,
            &[0x65, 0x48, 0x03, 0x05],
            [
                0x1111_1111_1111_1111,
                0x7fff_ffff_ffff_fff0,
                0x2222_2222_2222_2222,
            ],
            0x10,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x7fff_ffff_ffff_fff0,
            "memory unchanged"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.fs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x8000_0000_0000_0000
        );
        // 0x8000_0000_0000_0000: OF, SF and PF (zero low byte); no CF, AF, ZF.
        assert_eq!(
            state.rflags & 0x8d5,
            (1 << 11) | (1 << 7) | (1 << 2),
            "{:#x}",
            state.rflags
        );
    }

    #[test]
    fn fs_add_load_dword_after_wrmsr_fs_base_zero_extends_into_rax_and_leaves_memory_unchanged() {
        const BASE: u64 = 0x0200_0000;
        // `64 03 05 disp32` = `add %fs:disp(%rip), %eax`, FS base = BASE and a distractor GS
        // base. EAX 0xffff_fff0 + the low dword 0x20 of the memory qword wraps to 0x10 with CF;
        // the upper half of RAX is dropped (zero-extension).
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0100,
            0xC000_0101,
            &[0x64, 0x03, 0x05],
            [
                0x1111_1111_1111_1111,
                0xaaaa_bbbb_0000_0020,
                0x2222_2222_2222_2222,
            ],
            0xdead_beef_ffff_fff0,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0xaaaa_bbbb_0000_0020,
            "memory unchanged"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.fs_base, BASE);
        assert_eq!(
            state.gs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x10);
        // 0x10: CF only (one bit set, so no PF; no AF, ZF, SF, OF).
        assert_eq!(state.rflags & 0x8d5, 1, "{:#x}", state.rflags);
    }

    // ---- S4-38L7be: FS/GS-based CMP r32, m32 and r64, m64 through the board facade ----
    // Reuses `run_segment_add` (the same code layout, `3B` instead of `03`): RAX is the register
    // operand, the memory qword is only read, nothing is written and RAX does not change.

    #[test]
    fn gs_cmp_load_qword_after_wrmsr_gs_base_sets_flags_and_changes_nothing_else() {
        const BASE: u64 = 0x0200_0000;
        // `65 48 3b 05 disp32` = `cmp %gs:disp(%rip), %rax`, GS base = BASE and a distractor FS
        // base. RAX 0x7fff_ffff_ffff_ffff - 0xffff_ffff_ffff_fff0 (a borrow, and a signed
        // overflow: positive minus negative gives a negative) = 0x8000_0000_0000_000f: CF, OF,
        // SF and PF (four bits in the low byte); no AF, ZF.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0101,
            0xC000_0100,
            &[0x65, 0x48, 0x3b, 0x05],
            [
                0x1111_1111_1111_1111,
                0xffff_ffff_ffff_fff0,
                0x2222_2222_2222_2222,
            ],
            0x7fff_ffff_ffff_ffff,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0xffff_ffff_ffff_fff0,
            "memory unchanged"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.fs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x7fff_ffff_ffff_ffff,
            "CMP does not write the register"
        );
        assert_eq!(
            state.rflags & 0x8d5,
            1 | (1 << 11) | (1 << 7) | (1 << 2),
            "{:#x}",
            state.rflags
        );
    }

    #[test]
    fn fs_cmp_load_dword_after_wrmsr_fs_base_compares_the_low_dwords_only() {
        const BASE: u64 = 0x0200_0000;
        // `64 3b 05 disp32` = `cmp %fs:disp(%rip), %eax`, FS base = BASE and a distractor GS
        // base. The low dwords are 5 and 0x20 (the upper halves differ and are ignored):
        // 5 - 0x20 = 0xffff_ffe5 with a borrow: CF and SF only (five bits in the low byte, so no
        // PF; no AF, ZF, OF). RAX is not modified, upper half included.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0100,
            0xC000_0101,
            &[0x64, 0x3b, 0x05],
            [
                0x1111_1111_1111_1111,
                0xaaaa_bbbb_0000_0020,
                0x2222_2222_2222_2222,
            ],
            0xdead_beef_0000_0005,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0xaaaa_bbbb_0000_0020,
            "memory unchanged"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.fs_base, BASE);
        assert_eq!(
            state.gs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0xdead_beef_0000_0005
        );
        assert_eq!(state.rflags & 0x8d5, 1 | (1 << 7), "{:#x}", state.rflags);
    }

    // ---- S4-38L7bj: FS/GS-based CMOVcc r32, m32 and r64, m64 through the board facade ----
    // Reuses `run_segment_add`: RAX is the destination, the memory qword is only read. The helper's
    // `XOR EDX,EDX` leaves ZF = 1 (and PF = 1) for the CMOVE; the not-taken cases put a
    // `TEST RAX,RAX` (`48 85 c0`, one more retired instruction) in front, which clears ZF.

    #[test]
    fn gs_cmove_qword_with_zf_set_after_wrmsr_gs_base_loads_rax_and_leaves_memory_unchanged() {
        const BASE: u64 = 0x0200_0000;
        // `65 48 0f 44 05 disp32` = `cmove %gs:disp(%rip), %rax`, GS base = BASE and a distractor
        // FS base; ZF = 1 from the helper's XOR, so the qword is moved.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0101,
            0xC000_0100,
            &[0x65, 0x48, 0x0f, 0x44, 0x05],
            [
                0x1111_1111_1111_1111,
                0x8765_4321_0fed_cba9,
                0x2222_2222_2222_2222,
            ],
            0x10,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x8765_4321_0fed_cba9,
            "memory unchanged"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x8), 0x1111_1111_1111_1111);
        assert_eq!(guest_qword(&mut boot, BASE + 0x18), 0x2222_2222_2222_2222);
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.fs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x8765_4321_0fed_cba9
        );
        // CMOVE keeps the flags of the XOR: ZF and PF.
        assert_eq!(
            state.rflags & 0x8d5,
            (1 << 6) | (1 << 2),
            "{:#x}",
            state.rflags
        );
    }

    #[test]
    fn gs_cmove_dword_with_zf_set_zero_extends_the_loaded_dword_into_rax() {
        const BASE: u64 = 0x0200_0000;
        // `65 0f 44 05 disp32` = `cmove %gs:disp(%rip), %eax` (the boundary form with EAX).
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0101,
            0xC000_0100,
            &[0x65, 0x0f, 0x44, 0x05],
            [
                0x1111_1111_1111_1111,
                0xaaaa_bbbb_1122_3344,
                0x2222_2222_2222_2222,
            ],
            0xdead_beef_0000_0010,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0xaaaa_bbbb_1122_3344,
            "memory unchanged"
        );
        let state = boot.board.state();
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x0000_0000_1122_3344
        );
        assert_eq!(state.rflags & 0x8d5, (1 << 6) | (1 << 2));
    }

    #[test]
    fn fs_cmove_dword_with_zf_clear_still_zeroes_the_upper_half_of_rax() {
        const BASE: u64 = 0x0200_0000;
        // `48 85 c0` = `test %rax,%rax` (RAX non-zero: ZF = 0, PF = 0 for the low byte 0x10,
        // SF = 1 from bit 63 of 0xdead_beef_0000_0010, CF = OF = 0), then `64 0f 44 05 disp32` = `cmove %fs:disp(%rip), %eax` with FS
        // base = BASE and a distractor GS base. The move is not taken: EAX keeps 0x10, bits 63:32
        // are zeroed; the memory is read (not written) and nothing else changes.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0100,
            0xC000_0101,
            &[0x48, 0x85, 0xc0, 0x64, 0x0f, 0x44, 0x05],
            [
                0x1111_1111_1111_1111,
                0xaaaa_bbbb_1122_3344,
                0x2222_2222_2222_2222,
            ],
            0xdead_beef_0000_0010,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired + 1, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0xaaaa_bbbb_1122_3344,
            "memory unchanged"
        );
        let state = boot.board.state();
        assert_eq!(state.fs_base, BASE);
        assert_eq!(
            state.gs_base, 0x0300_0000,
            "the distractor base is not used"
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x10);
        assert_eq!(state.rflags & 0x8d5, 1 << 7, "{:#x}", state.rflags);
    }

    #[test]
    fn fs_cmovne_qword_with_zf_clear_after_wrmsr_fs_base_loads_rax() {
        const BASE: u64 = 0x0200_0000;
        // `48 85 c0` clears ZF; `64 48 0f 45 05 disp32` = `cmovne %fs:disp(%rip), %rax` moves.
        let (mut boot, record, retired) = run_segment_add(
            0xC000_0100,
            0xC000_0101,
            &[0x48, 0x85, 0xc0, 0x64, 0x48, 0x0f, 0x45, 0x05],
            [
                0x1111_1111_1111_1111,
                0x8765_4321_0fed_cba9,
                0x2222_2222_2222_2222,
            ],
            0x10,
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, retired + 1, "{record:?}");
        let state = boot.board.state();
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0x8765_4321_0fed_cba9
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x8765_4321_0fed_cba9,
            "memory unchanged"
        );
    }

    // ---- S4-38L7ah: FS/GS-based CMP m32/m64, imm8 through the board facade ----

    /// CLI; WRMSR `msr` (base chosen so that `base + next_rip + disp` lands on `BASE + 0x10`, the
    /// disp32 being the one of the Linux boundary bytes, 0x02e97995); MOV RAX,`cell`; store it at
    /// `BASE + 0x10`; `prefix` `83 3d disp32 imm8`; HLT. Returns the board, the run record and the
    /// base that was written.
    fn run_segment_cmp_imm8(
        msr: u32,
        prefix: &[u8],
        cell: u64,
        imm: u8,
    ) -> (LinuxDirectBoot, LinuxStopRecord, u64) {
        const BASE: u64 = 0x0200_0000;
        const DISP: u64 = 0x02e9_7995;
        // CLI(1) + MOV ECX(5) + MOV EAX(5) + MOV EDX(5) + WRMSR(2) + MOV RAX imm64(10) + store(8).
        let before_cmp = 1 + 5 + 5 + 5 + 2 + 10 + 8;
        let next_rip = ENTRY + before_cmp + prefix.len() as u64 + 6 + 1;
        let segment_base = (BASE + 0x10).wrapping_sub(next_rip).wrapping_sub(DISP);
        let mut code = vec![0xFA, 0xB9];
        code.extend(msr.to_le_bytes());
        code.extend(mov_eax(segment_base as u32));
        code.extend([0xBA]);
        code.extend(((segment_base >> 32) as u32).to_le_bytes());
        code.extend([0x0F, 0x30]);
        code.extend(mov_rax_imm64(cell));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        assert_eq!(code.len() as u64, before_cmp);
        code.extend(prefix);
        code.extend([0x83, 0x3d]);
        code.extend((DISP as u32).to_le_bytes());
        code.push(imm);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        (boot, record, segment_base)
    }

    #[test]
    fn gs_cmpl_imm8_boundary_bytes_set_flags_and_do_not_write_guest_memory() {
        // The Linux boundary form `65 83 3d 95 79 e9 02 01` (cmpl $1, %gs:0x2e97995(%rip)).
        // The cell's low dword is 1 (the upper dword is ignored): ZF and PF.
        let cell = 0xdead_beef_0000_0001;
        let (mut boot, record, gs_base) = run_segment_cmp_imm8(0xC000_0101, &[0x65], cell, 0x01);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 9,
            "CLI, MOV, MOV, MOV, WRMSR, MOV, store, CMP, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, gs_base);
        assert_eq!(state.fs_base, 0);
        assert_eq!(
            state.rflags & 0x8d5,
            (1 << 6) | (1 << 2),
            "{:#x}",
            state.rflags
        );
        assert_eq!(
            guest_qword(&mut boot, 0x0200_0010),
            cell,
            "CMP does not write"
        );

        // Other cell: 0 - 1 sets CF, SF, PF, AF (the upper dword is ignored again).
        let (mut boot, record, _) =
            run_segment_cmp_imm8(0xC000_0101, &[0x65], 0x1234_5678_0000_0000, 0x01);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            boot.board.state().rflags & 0x8d5,
            (1 << 0) | (1 << 7) | (1 << 2) | (1 << 4)
        );
        assert_eq!(
            guest_qword(&mut boot, 0x0200_0010),
            0x1234_5678_0000_0000,
            "CMP does not write"
        );
    }

    #[test]
    fn fs_cmpq_negative_imm8_through_the_set_loop_compares_sign_extended() {
        // `cmpq $-1, %fs:disp(%rip)` (`64 48 83 3d ... ff`) with an all-ones cell: ZF and PF.
        let (mut boot, record, fs_base) =
            run_segment_cmp_imm8(0xC000_0100, &[0x64, 0x48], u64::MAX, 0xff);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 9);
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().fs_base, fs_base);
        assert_eq!(boot.board.state().gs_base, 0);
        assert_eq!(boot.board.state().rflags & 0x8d5, (1 << 6) | (1 << 2));
        assert_eq!(guest_qword(&mut boot, 0x0200_0010), u64::MAX);

        // INT64_MIN - (-1): 0x8000_0000_0000_0000 - 0xffff_ffff_ffff_ffff = 0x8000_0000_0000_0001
        // modulo 2^64: CF (borrow), SF, no OF; the low byte 0x01 has odd parity (no PF); AF.
        let (mut boot, record, _) =
            run_segment_cmp_imm8(0xC000_0100, &[0x64, 0x48], 0x8000_0000_0000_0000, 0xff);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            boot.board.state().rflags & 0x8d5,
            (1 << 0) | (1 << 7) | (1 << 4),
            "{:#x}",
            boot.board.state().rflags
        );
        assert_eq!(guest_qword(&mut boot, 0x0200_0010), 0x8000_0000_0000_0000);
    }

    // ---- S4-38L7ab: INVLPG m (no-op) through the board facade ----

    #[test]
    fn invlpg_through_the_set_loop_retires_without_touching_guest_memory() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV RAX,INT64_MIN; MOV [BASE+0x10],RAX; MOV RDI,BASE+0x10; INVLPG (%rdi);
        // MOV RDI,non-canonical; INVLPG (%rdi) (no #GP: the operand is not evaluated);
        // GS INVLPG (%rdi); HLT.
        let mut code = vec![0xFA];
        code.extend(mov_rax_imm64(0x8000_0000_0000_0000));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBF]);
        code.extend((BASE + 0x10).to_le_bytes());
        code.extend([0x0F, 0x01, 0x3F]);
        code.extend([0x48, 0xBF]);
        code.extend(0xFFFF_FFFF_FFFF_FFFFu64.to_le_bytes());
        code.extend([0x0F, 0x01, 0x3F]);
        code.extend([0x65, 0x0F, 0x01, 0x3F]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 9,
            "CLI, MOV, store, MOV, INVLPG, MOV, INVLPG, GS INVLPG, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rdi as usize], u64::MAX);
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x8000_0000_0000_0000,
            "INVLPG does not write"
        );
    }

    // ---- S4-38L7k: LOCK CMPXCHG m64, r64 through the board facade ----

    #[test]
    fn lock_cmpxchg_reads_and_writes_guest_memory_for_equal_and_unequal_compares() {
        // MOV RAX,5; MOV [0x0200_0020],RAX; MOV RDX,9;
        // LOCK CMPXCHG [0x0200_0020],RDX (RAX == [m]: [m] = 9, RAX stays 5);
        // LOCK CMPXCHG [0x0200_0020],RDX (RAX 5 != [m] 9: RAX = 9, [m] written back as 9);
        // MOV RCX,[0x0200_0020]; HLT.
        let mut code = mov_rax_imm64(5);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBA]);
        code.extend(9u64.to_le_bytes());
        code.extend([0xF0, 0x48, 0x0F, 0xB1, 0x14, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0xF0, 0x48, 0x0F, 0xB1, 0x14, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x8B, 0x0C, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 9);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 9);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 9);
        // The second compare was unequal: ZF is clear after it (HLT/MOV leave the flags alone).
        assert_eq!(state.rflags & (1 << 6), 0);
    }

    #[test]
    fn plain_cmpxchg_memory_form_without_lock_executes_as_the_lock_form_in_the_cpl0_set() {
        // S4-38L9k: S4-38L7k refused the plain `48 0F B1 /r` memory form; on the single vCPU it is
        // the LOCK form (LOCK changes only the atomicity against other processors), so it executes
        // through the facade. MOV RAX,5; MOV [0x0200_0020],RAX; MOV RDX,9; CMPXCHG [m],RDX
        // (RAX == [m]: [m] = 9, RAX stays 5, ZF set); MOV RCX,[m]; HLT.
        let mut code = mov_rax_imm64(5);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBA]);
        code.extend(9u64.to_le_bytes());
        code.extend([0x48, 0x0F, 0xB1, 0x14, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x8B, 0x0C, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 5);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 9);
        assert_eq!(state.rflags & (1 << 6), 1 << 6, "ZF");
    }

    // ---- S4-38L7y: LOCK CMPXCHG m32, r32 through the board facade ----

    /// `MOV RBX,0x0200_0000; MOV RAX,<rax>; MOV [RBX+0x58],EAX;`
    /// `MOV RAX,<eax_before_cmpxchg>; MOV RDX,0x1_0000_0009;`
    /// `LOCK CMPXCHG [RBX+0x58],EDX (f0 0f b1 53 58); MOV ECX,[RBX+0x58]; HLT`.
    fn run_lock_cmpxchg_dword(stored: u64, rax: u64) -> (LinuxDirectBoot, LinuxStopRecord) {
        let mut code = vec![0x48, 0xBB];
        code.extend(0x0200_0000u64.to_le_bytes());
        code.extend(mov_rax_imm64(stored));
        code.extend([0x89, 0x43, 0x58]);
        code.extend(mov_rax_imm64(rax));
        code.extend([0x48, 0xBA]);
        code.extend(0x1_0000_0009u64.to_le_bytes());
        code.extend([0xF0, 0x0F, 0xB1, 0x53, 0x58]);
        code.extend([0x8B, 0x4B, 0x58]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        (boot, record)
    }

    #[test]
    fn lock_cmpxchg_dword_equal_writes_the_source_and_keeps_rax_at_disp8_of_rbx() {
        let (boot, record) = run_lock_cmpxchg_dword(0x77, 0xaaaa_bbbb_0000_0077);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 8, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // EAX == [m]: the memory takes EDX (9), RAX is untouched, ZF = 1.
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 9);
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0xaaaa_bbbb_0000_0077
        );
        assert_eq!(state.rflags & (1 << 6), 1 << 6, "ZF");
    }

    #[test]
    fn lock_cmpxchg_dword_unequal_loads_eax_zero_extended_and_writes_the_old_value_back() {
        let (boot, record) = run_lock_cmpxchg_dword(0x77, 0xaaaa_bbbb_0000_0005);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 8, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // EAX != [m]: EAX = 0x77 (upper half of RAX zeroed), memory keeps 0x77, ZF = 0.
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x77);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x77);
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
    }

    // ---- S4-38L7ay: LOCK XADD m64, r64 and m32, r32 through the board facade ----

    #[test]
    fn lock_xadd_qword_adds_to_guest_memory_and_returns_the_old_value_at_the_boundary_form() {
        // MOV RDI,0x0200_0040; MOV RAX,5; MOV [RDI],RAX; MOV RAX,-1;
        // LOCK XADD [RDI],RAX (f0 48 0f c1 07: the S4-38L7ax boundary bytes; the board adapter
        // has no atomic fetch-add, so this path used to end in a backend error);
        // MOV RCX,[RDI]; HLT.
        let mut code = vec![0x48, 0xBF];
        code.extend(0x0200_0040u64.to_le_bytes());
        code.extend(mov_rax_imm64(5));
        code.extend([0x48, 0x89, 0x07]);
        code.extend(mov_rax_imm64(u64::MAX));
        code.extend([0xF0, 0x48, 0x0F, 0xC1, 0x07]);
        code.extend([0x48, 0x8B, 0x0F]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // [m] = 5 + (-1) = 4; RAX receives the old value 5; the sum 5 + (-1) carries (CF) and has
        // the low nibble carry (AF).
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 4);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 5);
        assert_eq!(state.rflags & 1, 1, "CF");
        assert_eq!(state.rflags & (1 << 4), 1 << 4, "AF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
    }

    #[test]
    fn lock_xadd_dword_adds_to_guest_memory_and_zero_extends_the_old_value() {
        // MOV RBX,0x0200_0000; MOV RAX,0x7fffffff; MOV [RBX+0x58],EAX; MOV RAX,0x1_0000_0001;
        // LOCK XADD [RBX+0x58],EAX (f0 0f c1 43 58); MOV ECX,[RBX+0x58]; HLT.
        let mut code = vec![0x48, 0xBB];
        code.extend(0x0200_0000u64.to_le_bytes());
        code.extend(mov_rax_imm64(0x7fff_ffff));
        code.extend([0x89, 0x43, 0x58]);
        code.extend(mov_rax_imm64(0x1_0000_0001));
        code.extend([0xF0, 0x0F, 0xC1, 0x43, 0x58]);
        code.extend([0x8B, 0x4B, 0x58]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // 0x7fff_ffff + 1 = 0x8000_0000 (OF, SF, AF, PF); EAX = old value, zero-extended.
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x8000_0000);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x7fff_ffff);
        assert_eq!(state.rflags & (1 << 11), 1 << 11, "OF");
        assert_eq!(state.rflags & (1 << 7), 1 << 7, "SF");
    }

    #[test]
    fn plain_xadd_memory_form_without_lock_executes_as_the_lock_form_in_the_cpl0_set() {
        // S4-38L9k: S4-38L7ay refused the plain `48 0F C1 /r` memory form; on the single vCPU it
        // is the LOCK form, so it executes through the facade. MOV RAX,5; MOV [0x0200_0020],RAX;
        // MOV RDX,3; XADD [m],RDX ([m] = 8, RDX = 5); MOV RCX,[m]; HLT.
        let mut code = mov_rax_imm64(5);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xBA]);
        code.extend(3u64.to_le_bytes());
        code.extend([0x48, 0x0F, 0xC1, 0x14, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x8B, 0x0C, 0x25, 0x20, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 8);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 5);
    }

    // ---- S4-38L7z: LOCK ADD m32, imm8 on an RSP base through the board facade ----

    #[test]
    fn lock_add_dword_at_minus_four_of_rsp_changes_only_that_dword() {
        // MOV RSP,0x0200_0010; MOV RAX,0x7fffffff_11223344; MOV [RSP-8],RAX;
        // MOV RCX,0xa5a5a5a5_5a5a5a5a; MOV [RSP],RCX;
        // LOCK ADD DWORD [RSP-4],1 (`f0 83 44 24 fc 01`: 0x7fffffff + 1 = 0x80000000);
        // MOV RDX,[RSP-8]; MOV RBX,[RSP]; HLT.
        let mut code = vec![0x48, 0xBC];
        code.extend(0x0200_0010u64.to_le_bytes());
        code.extend(mov_rax_imm64(0x7fff_ffff_1122_3344));
        code.extend([0x48, 0x89, 0x44, 0x24, 0xF8]);
        code.extend([0x48, 0xB9]);
        code.extend(0xa5a5_a5a5_5a5a_5a5au64.to_le_bytes());
        code.extend([0x48, 0x89, 0x0C, 0x24]);
        code.extend([0xF0, 0x83, 0x44, 0x24, 0xFC, 0x01]);
        code.extend([0x48, 0x8B, 0x54, 0x24, 0xF8]);
        code.extend([0x48, 0x8B, 0x1C, 0x24]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 9, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // The dword at RSP-4 became 0x80000000; the dword at RSP-8 and the qword at RSP are intact.
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0x8000_0000_1122_3344
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            0xa5a5_a5a5_5a5a_5a5a
        );
        assert_eq!(state.gpr[od_isa::Register::Rsp as usize], 0x0200_0010);
        assert_eq!(state.rflags & (1 << 11), 1 << 11, "OF");
        assert_eq!(state.rflags & (1 << 7), 1 << 7, "SF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & 1, 0, "CF");
    }

    // ---- S4-38L7l: LOCK INC m32 through the board facade ----

    #[test]
    fn lock_inc_reads_and_writes_guest_memory_at_disp8_of_rbx() {
        // MOV RBX,0x0200_0000; MOV RAX,0x7fffffff; MOV [RBX+0x58],EAX;
        // LOCK INC DWORD [RBX+0x58] (0x7fffffff -> 0x80000000: OF and SF set);
        // MOV ECX,[RBX+0x58]; HLT.
        let mut code = vec![0x48, 0xBB];
        code.extend(0x0200_0000u64.to_le_bytes());
        code.extend(mov_rax_imm64(0x7fff_ffff));
        code.extend([0x89, 0x43, 0x58]);
        code.extend([0xF0, 0xFF, 0x43, 0x58]);
        code.extend([0x8B, 0x4B, 0x58]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x8000_0000);
        assert_eq!(state.rflags & (1 << 11), 1 << 11, "OF");
        assert_eq!(state.rflags & (1 << 7), 1 << 7, "SF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
    }

    // ---- S4-38L7s: LOCK OR m8, imm8 through the board facade ----

    #[test]
    fn lock_or_byte_reads_and_writes_one_guest_byte_at_disp8_of_rbx() {
        // MOV RBX,0x0200_0000; MOV RAX,0x1122334455667741; MOV [RBX+0x38],RAX;
        // ADD AL,0xC0 (0x41 + 0xC0 carries: CF = 1);
        // LOCK ORB [RBX+0x3D],0x80 (`f0 80 4b 3d 80`: the byte 0x33 becomes 0xB3, CF is cleared);
        // MOV RDX,[RBX+0x38]; HLT.
        let mut code = vec![0x48, 0xBB];
        code.extend(0x0200_0000u64.to_le_bytes());
        code.extend(mov_rax_imm64(0x1122_3344_5566_7741));
        code.extend([0x48, 0x89, 0x43, 0x38]);
        code.extend([0x04, 0xC0]);
        code.extend([0xF0, 0x80, 0x4B, 0x3D, 0x80]);
        code.extend([0x48, 0x8B, 0x53, 0x38]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // Only the byte at +5 changed; its neighbours are intact.
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0x1122_B344_5566_7741
        );
        assert_eq!(state.rflags & (1 << 0), 0, "CF");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_eq!(state.rflags & (1 << 7), 1 << 7, "SF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & (1 << 2), 0, "PF (0xB3 has five set bits)");
    }

    // ---- S4-38L7o: FS/GS-based TEST m8, imm8 through the board facade ----

    #[test]
    fn gs_test_byte_after_wrmsr_gs_base_reads_one_byte_sets_flags_and_leaves_memory_alone() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CD81;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); ADD AL,0x80 (0x81 + 0x80 sets CF and OF);
        // TEST %gs:0x10,$0xf0 (`65 f6 04 25 10 00 00 00 f0`: 0x81 & 0xf0 = 0x80, clears CF and OF);
        // SUB AL,2 (0x01 - 2 borrows: CF = 1);
        // TEST %gs:disp(%rip),$0x0e (`65 f6 05 disp32 0e`, effective address 0x10: 0x81 & 0x0e = 0,
        // the next RIP is after the immediate); MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x04, 0x80]);
        code.extend([0x65, 0xF6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00, 0xF0]);
        code.extend([0x2C, 0x02]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xF6, 0x05]);
        code.extend(disp.to_le_bytes());
        code.push(0x0E);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 13,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, ADD, TEST, SUB, TEST, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // TEST wrote nothing: the whole qword is still there (the ADD/SUB only touched AL, now
        // 0x81 + 0x80 - 2 = 0xff), and no register other than RDX (the load) changed.
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], VALUE);
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            (VALUE & !0xFF) | 0xFF
        );
        // The last TEST gave 0x81 & 0x0e = 0: ZF = 1, PF = 1, SF = 0, and cleared the CF that the
        // SUB had set.
        assert_eq!(state.rflags & 1, 0, "CF");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_ne!(state.rflags & (1 << 6), 0, "ZF");
        assert_ne!(state.rflags & (1 << 2), 0, "PF");
        assert_eq!(state.rflags & (1 << 7), 0, "SF");
    }

    // ---- S4-38L7r: FS/GS-based TEST m16, imm16 through the board facade ----

    #[test]
    fn gs_test_word_after_wrmsr_gs_base_reads_one_word_sets_flags_and_leaves_memory_alone() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_8001;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); SUB DL,1 (0 - 1 borrows: CF = 1);
        // TEST %gs:0x10,$0xff01 (`65 66 f7 04 25 10 00 00 00 01 ff`: 0x8001 & 0xff01 = 0x8001,
        // SF = 1, PF = 0, clears CF and OF);
        // ADD DL,2 (0xff + 2 carries: CF = 1);
        // TEST %gs:disp(%rip),$0x00fe with the prefixes swapped (`66 65 f7 05 disp32 fe 00`,
        // effective address 0x10: 0x8001 & 0x00fe = 0, the next RIP is after the imm16);
        // MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([
            0x65, 0x66, 0xF7, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00, 0x01, 0xFF,
        ]);
        code.extend([0x80, 0xC2, 0x02]);
        let next_rip = ENTRY + code.len() as u64 + 10;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x66, 0x65, 0xF7, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0xFE, 0x00]);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 13,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, SUB, TEST, ADD, TEST, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // TEST wrote nothing: the whole qword is still there (loaded into RDX), RAX is untouched.
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], VALUE);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // The last TEST gave 0x8001 & 0x00fe = 0: ZF = 1, PF = 1, SF = 0, and cleared the CF that
        // the ADD had set.
        assert_eq!(state.rflags & 1, 0, "CF");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_ne!(state.rflags & (1 << 6), 0, "ZF");
        assert_ne!(state.rflags & (1 << 2), 0, "PF");
        assert_eq!(state.rflags & (1 << 7), 0, "SF");
    }

    // ---- S4-38L7ag: LOCK AND m8, imm8 through the board facade ----

    #[test]
    fn lock_and_byte_rip_relative_with_a_negative_displacement_changes_only_that_byte() {
        const DATA: u64 = 0x0010_0010;
        const VALUE: u64 = 0x0123_4567_89AB_CD37;
        // MOV RAX,VALUE; MOV [DATA],RAX (`48 89 04 25 disp32`); SUB DL,1 (0 - 1 borrows: CF = 1);
        // LOCK ANDB $0xfe,DATA(%rip) (`f0 80 25 disp32 fe`, the S4-38L7af boundary shape: the
        // displacement is negative and the next RIP is after the immediate; 0x37 & 0xfe = 0x36);
        // MOV RBX,[DATA] (the read back); HLT.
        let mut code = mov_rax_imm64(VALUE);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x10, 0x00]);
        code.extend([0x80, 0xEA, 0x01]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (DATA.wrapping_sub(next_rip)) as u32;
        assert!(disp >> 31 == 1, "the displacement is negative");
        code.extend([0xF0, 0x80, 0x25]);
        code.extend(disp.to_le_bytes());
        code.push(0xFE);
        code.extend([0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x10, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "MOV, store, SUB, LOCK AND, load, HLT");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // Only the low byte of the qword changed: 0x37 & 0xfe = 0x36.
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            0x0123_4567_89AB_CD36
        );
        assert_eq!(guest_qword(&mut boot, DATA), 0x0123_4567_89AB_CD36);
        let state = boot.board.state();
        // AND clears CF (set by SUB DL,1); 0x36 has four set bits: PF = 1, ZF = SF = 0.
        assert_eq!(state.rflags & 1, 0, "CF");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_eq!(state.rflags & (1 << 2), 1 << 2, "PF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & (1 << 7), 0, "SF");
    }

    // ---- S4-38L7ai: LOCK ADD m64, r64 through the board facade ----

    #[test]
    fn lock_add_qword_register_source_rip_relative_adds_to_guest_memory() {
        const DATA: u64 = 0x0010_0010;
        const VALUE: u64 = 0x7FFF_FFFF_FFFF_FFF0;
        const ADDEND: u64 = 0x20;
        // MOV RAX,VALUE; MOV [DATA],RAX (`48 89 04 25 disp32`); MOV RDI,ADDEND;
        // LOCK ADD %rdi,DATA(%rip) (`f0 48 01 3d disp32`, the S4-38L7ah boundary shape: the
        // displacement is negative and the next RIP is the end of these eight bytes);
        // MOV RBX,[DATA] (the read back); HLT.
        let mut code = mov_rax_imm64(VALUE);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x10, 0x00]);
        code.extend([0x48, 0xBF]);
        code.extend(ADDEND.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (DATA.wrapping_sub(next_rip)) as u32;
        assert!(disp >> 31 == 1, "the displacement is negative");
        code.extend([0xF0, 0x48, 0x01, 0x3D]);
        code.extend(disp.to_le_bytes());
        code.extend([0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x10, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "MOV, store, MOV, LOCK ADD, load, HLT");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // 0x7fff_ffff_ffff_fff0 + 0x20 = 0x8000_0000_0000_0010.
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            0x8000_0000_0000_0010
        );
        assert_eq!(state.gpr[od_isa::Register::Rdi as usize], ADDEND);
        assert_eq!(guest_qword(&mut boot, DATA), 0x8000_0000_0000_0010);
        let state = boot.board.state();
        // Signed overflow into a negative result; no carry, no zero, odd parity, no nibble carry.
        assert_eq!(state.rflags & (1 << 11), 1 << 11, "OF");
        assert_eq!(state.rflags & (1 << 7), 1 << 7, "SF");
        assert_eq!(state.rflags & 1, 0, "CF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & (1 << 2), 0, "PF");
        assert_eq!(state.rflags & (1 << 4), 0, "AF");
    }

    // ---- S4-38L7af: CALL rel32 with a CS override through the board facade ----

    #[test]
    fn cs_call_rel32_forward_pushes_the_return_address_and_pop_reads_it_back() {
        // MOV RSP,0x0200_0100; CS CALL +1 (`2e e8 01 00 00 00`: skips the HLT right after it);
        // POP RAX at the target; HLT. The popped value is the address of the instruction after
        // the CALL (6 bytes long), RSP is back, the qword stays in memory below RSP.
        let mut code = vec![0x48, 0xBC];
        code.extend(0x0200_0100u64.to_le_bytes());
        let call_at = ENTRY + code.len() as u64;
        code.extend([0x2E, 0xE8, 0x01, 0x00, 0x00, 0x00]);
        code.extend([0xF4, 0x58, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "MOV, CS CALL, POP, HLT: {record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let return_address = call_at + 6;
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::Rax as usize],
            return_address
        );
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::Rsp as usize],
            0x0200_0100
        );
        assert_eq!(guest_qword(&mut boot, 0x0200_00F8), return_address);
        // The final HLT is the one after POP (the first one was skipped).
        assert_eq!(boot.board.state().rip, return_address + 3);
    }

    #[test]
    fn cs_call_rel32_backward_branches_to_an_earlier_target_with_a_negative_displacement() {
        // MOV RSP,0x0200_0100; JMP +2 (over `POP RAX; HLT`); CS CALL -8 back to the POP RAX.
        // POP RAX takes the return address (the instruction after the 6-byte CALL) and HLT stops.
        let mut code = vec![0x48, 0xBC];
        code.extend(0x0200_0100u64.to_le_bytes());
        code.extend([0xEB, 0x02]);
        let target = ENTRY + code.len() as u64;
        code.extend([0x58, 0xF4]);
        let call_at = ENTRY + code.len() as u64;
        let next = call_at + 6;
        let rel = (target as i64 - next as i64) as i32;
        assert_eq!(rel, -8);
        code.extend([0x2E, 0xE8]);
        code.extend(rel.to_le_bytes());
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, JMP, CS CALL, POP, HLT: {record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(boot.board.state().gpr[od_isa::Register::Rax as usize], next);
        assert_eq!(
            boot.board.state().gpr[od_isa::Register::Rsp as usize],
            0x0200_0100
        );
        assert_eq!(guest_qword(&mut boot, 0x0200_00F8), next);
    }

    // ---- S4-38L7ba: FS/GS-based MOVSX r32/r64, m8 through the board facade ----

    #[test]
    fn gs_movsx_byte_after_wrmsr_gs_base_sign_extends_into_rax_r8_and_ebx_ignoring_the_fs_base() {
        const BASE: u64 = 0x0200_0000;
        const FS_DISTRACTOR: u64 = 0x0300_0000;
        // CLI; WRMSR IA32_FS_BASE = FS_DISTRACTOR; WRMSR IA32_GS_BASE = BASE;
        // plain byte stores: [BASE+0x10] = 0x80, [BASE+0x11] = 0x7f, [FS_DISTRACTOR+0x10] = 0x11;
        // SUB DL,1 (0 - 1 borrows: CF = 1); markers in RAX, R8 and RBX;
        // MOV ECX,0x10; MOVSX RAX,%gs:(%rcx) (`65 48 0f be 01`, the S4-38L7az boundary form);
        // MOV EDX,5; MOVSX R8,%gs:0xc(%rdx) (`65 4c 0f be 42 0c`, the next instruction of the
        // boundary; effective address 0x11); MOVSX EBX,%gs:0x10 (`65 0f be 1c 25 disp32`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x00, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(FS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x01, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0x80]);
        code.extend([0xC6, 0x04, 0x25, 0x11, 0x00, 0x00, 0x02, 0x7F]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x11]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend(mov_rax_imm64(0x5555_5555_5555_5555));
        code.extend([0x49, 0xB8]);
        code.extend(0x6666_6666_6666_6666u64.to_le_bytes());
        code.extend([0x48, 0xBB]);
        code.extend(0x7777_7777_7777_7777u64.to_le_bytes());
        code.extend([0xB9, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x48, 0x0F, 0xBE, 0x01]);
        code.extend([0xBA, 0x05, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x4C, 0x0F, 0xBE, 0x42, 0x0C]);
        code.extend([0x65, 0x0F, 0xBE, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 22, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x10) & 0xFFFF, 0x7F80);
        let state = boot.board.state();
        assert_eq!((state.gs_base, state.fs_base), (BASE, FS_DISTRACTOR));
        // The FS distractor holds 0x11: a result of 0x11 would mean the wrong base was used.
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0xFFFF_FFFF_FFFF_FF80
        );
        assert_eq!(state.gpr[od_isa::Register::R8 as usize], 0x7F);
        assert_eq!(state.gpr[od_isa::Register::Rbx as usize], 0xFFFF_FF80);
        // Nothing else changed: CF set by SUB DL,1 survives and the index registers keep their values.
        assert_eq!(state.rflags & 1, 1, "CF");
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x10);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 5);
    }

    #[test]
    fn fs_movsx_byte_after_wrmsr_fs_base_loads_r9_and_eax_ignoring_the_gs_base() {
        const BASE: u64 = 0x0200_0000;
        const GS_DISTRACTOR: u64 = 0x0300_0000;
        // As above with the roles swapped: IA32_FS_BASE = BASE (MSR 0xC0000100) and the GS base
        // as the distractor; `64 4d 0f be 4b 03` = MOVSX R9,%fs:3(%r11) (REX.W, REX.R, REX.B) and
        // `64 41 0f be 02` = MOVSX EAX,%fs:(%r10) (REX.B, no REX.W).
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(GS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x00, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0xFF]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x22]);
        code.extend(mov_rax_imm64(0x5555_5555_5555_5555));
        code.extend([0x49, 0xB9]);
        code.extend(0x6666_6666_6666_6666u64.to_le_bytes());
        code.extend([0x41, 0xBB, 0x0D, 0x00, 0x00, 0x00]);
        code.extend([0x41, 0xBA, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x64, 0x4D, 0x0F, 0xBE, 0x4B, 0x03]);
        code.extend([0x64, 0x41, 0x0F, 0xBE, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 18, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!((state.fs_base, state.gs_base), (BASE, GS_DISTRACTOR));
        assert_eq!(
            state.gpr[od_isa::Register::R9 as usize],
            0xFFFF_FFFF_FFFF_FFFF
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0xFFFF_FFFF);
    }

    // ---- S4-38L7bf: FS/GS-based MOVZX r32/r64, m8 through the board facade ----

    #[test]
    fn gs_movzx_byte_after_wrmsr_gs_base_zero_extends_into_eax_r8_and_ebx_ignoring_the_fs_base() {
        const BASE: u64 = 0x0200_0000;
        const FS_DISTRACTOR: u64 = 0x0300_0000;
        // CLI; WRMSR IA32_FS_BASE = FS_DISTRACTOR; WRMSR IA32_GS_BASE = BASE;
        // plain byte stores: [BASE+0x10] = 0x80, [BASE+0x11] = 0xFF, [FS_DISTRACTOR+0x10] = 0x11;
        // SUB DL,1 (0 - 1 borrows: CF = 1); markers in RAX, R8 and RBX;
        // MOV EDI,0x10; MOVZX EAX,%gs:(%rdi) (`65 0f b6 07`, the S4-38L7be boundary form);
        // MOV EDX,5; MOVZX R8,%gs:0xc(%rdx) (`65 4c 0f b6 42 0c`, effective address 0x11);
        // MOVZX EBX,%gs:0x10 (`65 0f b6 1c 25 disp32`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x00, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(FS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x01, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0x80]);
        code.extend([0xC6, 0x04, 0x25, 0x11, 0x00, 0x00, 0x02, 0xFF]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x11]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend(mov_rax_imm64(0x5555_5555_5555_5555));
        code.extend([0x49, 0xB8]);
        code.extend(0x6666_6666_6666_6666u64.to_le_bytes());
        code.extend([0x48, 0xBB]);
        code.extend(0x7777_7777_7777_7777u64.to_le_bytes());
        code.extend([0xBF, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x0F, 0xB6, 0x07]);
        code.extend([0xBA, 0x05, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x4C, 0x0F, 0xB6, 0x42, 0x0C]);
        code.extend([0x65, 0x0F, 0xB6, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 22, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        // The neighbouring guest bytes are unchanged (nothing is written by the loads).
        assert_eq!(guest_qword(&mut boot, BASE + 0x10) & 0xFFFF, 0xFF80);
        let state = boot.board.state();
        assert_eq!((state.gs_base, state.fs_base), (BASE, FS_DISTRACTOR));
        // The FS distractor holds 0x11: a result of 0x11 would mean the wrong base was used; the
        // upper halves of the RAX/RBX markers are zeroed, R8 is a full 64-bit zero-extension.
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x80);
        assert_eq!(state.gpr[od_isa::Register::R8 as usize], 0xFF);
        assert_eq!(state.gpr[od_isa::Register::Rbx as usize], 0x80);
        // Nothing else changed: CF set by SUB DL,1 survives and the index registers keep their values.
        assert_eq!(state.rflags & 1, 1, "CF");
        assert_eq!(state.gpr[od_isa::Register::Rdi as usize], 0x10);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 5);
    }

    #[test]
    fn fs_movzx_byte_after_wrmsr_fs_base_loads_r9_and_eax_ignoring_the_gs_base() {
        const BASE: u64 = 0x0200_0000;
        const GS_DISTRACTOR: u64 = 0x0300_0000;
        // As above with the roles swapped: IA32_FS_BASE = BASE (MSR 0xC0000100) and the GS base
        // as the distractor; `64 4d 0f b6 4b 03` = MOVZX R9,%fs:3(%r11) (REX.W, REX.R, REX.B) and
        // `64 41 0f b6 02` = MOVZX EAX,%fs:(%r10) (REX.B, no REX.W).
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(GS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x00, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0xFF]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x22]);
        code.extend(mov_rax_imm64(0x5555_5555_5555_5555));
        code.extend([0x49, 0xB9]);
        code.extend(0x6666_6666_6666_6666u64.to_le_bytes());
        code.extend([0x41, 0xBB, 0x0D, 0x00, 0x00, 0x00]);
        code.extend([0x41, 0xBA, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x64, 0x4D, 0x0F, 0xB6, 0x4B, 0x03]);
        code.extend([0x64, 0x41, 0x0F, 0xB6, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 18, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!((state.fs_base, state.gs_base), (BASE, GS_DISTRACTOR));
        assert_eq!(state.gpr[od_isa::Register::R9 as usize], 0xFF);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0xFF);
        assert_eq!(state.gpr[od_isa::Register::R10 as usize], 0x10);
        assert_eq!(state.gpr[od_isa::Register::R11 as usize], 0x0D);
    }

    // ---- S4-38L7bg: FS/GS-based CMPXCHG r/m8, r8 through the board facade ----

    #[test]
    fn gs_cmpxchg_byte_after_wrmsr_gs_base_stores_cl_on_equal_and_loads_al_on_not_equal() {
        const BASE: u64 = 0x0200_0000;
        const FS_DISTRACTOR: u64 = 0x0300_0000;
        // CLI; WRMSR IA32_FS_BASE = FS_DISTRACTOR; WRMSR IA32_GS_BASE = BASE; plain byte stores:
        // [BASE+0xf] = 0xAA, [BASE+0x10] = 0x42, [BASE+0x11] = 0xBB, [FS_DISTRACTOR+0x10] = 0x42;
        // MOV EAX,0x42; MOV ECX,0x99; MOV EDI,0x10; CMPXCHG %cl,%gs:(%rdi) (`65 0f b0 0f`, the
        // S4-38L7bf boundary form; equal: the byte becomes CL, ZF = 1, AL kept);
        // MOV EAX,0x55555544; MOV EDI,0x11; CMPXCHG %cl,%gs:(%rdi) (0x44 vs 0xBB: not equal, AL
        // becomes 0xBB with the upper RAX bytes kept, the byte is written back; CF/SF/AF/OF = 1,
        // ZF/PF = 0); HLT.
        let mut code = vec![0xFA, 0xB9, 0x00, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(FS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x01, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x0F, 0x00, 0x00, 0x02, 0xAA]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0x42]);
        code.extend([0xC6, 0x04, 0x25, 0x11, 0x00, 0x00, 0x02, 0xBB]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x42]);
        code.extend(mov_eax(0x42));
        code.extend([0xB9, 0x99, 0x00, 0x00, 0x00]);
        code.extend([0xBF, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x0F, 0xB0, 0x0F]);
        code.extend(mov_eax(0x5555_5544));
        code.extend([0xBF, 0x11, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x0F, 0xB0, 0x0F]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 21, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        // [BASE+0xf..=0x11]: the neighbours are unchanged, the equal compare stored CL (0x99), the
        // failed one wrote the old byte (0xBB) back. The FS distractor byte is untouched.
        assert_eq!(guest_qword(&mut boot, BASE + 0x0F) & 0xFF_FFFF, 0xBB_99AA);
        assert_eq!(guest_qword(&mut boot, FS_DISTRACTOR + 0x10) & 0xFF, 0x42);
        let state = boot.board.state();
        assert_eq!((state.gs_base, state.fs_base), (BASE, FS_DISTRACTOR));
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x5555_55BB);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x99);
        assert_eq!(state.gpr[od_isa::Register::Rdi as usize], 0x11);
        // 0x44 - 0xBB = 0x89: CF, AF, SF, OF set; ZF and PF clear.
        assert_eq!(state.rflags & 0x8D5, 0x1 | 0x10 | 0x80 | 0x800);
    }

    #[test]
    fn fs_cmpxchg_byte_after_wrmsr_fs_base_stores_r9b_ignoring_the_gs_base() {
        const BASE: u64 = 0x0200_0000;
        const GS_DISTRACTOR: u64 = 0x0300_0000;
        // As above with the roles swapped; `64 44 0f b0 0a` = CMPXCHG %r9b,%fs:(%rdx) (REX.R).
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(GS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x00, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x20, 0x00, 0x00, 0x02, 0x42]);
        code.extend([0xC6, 0x04, 0x25, 0x20, 0x00, 0x00, 0x03, 0x42]);
        code.extend(mov_eax(0x42));
        code.extend([0x41, 0xB9, 0x66, 0x77, 0x00, 0x00]);
        code.extend([0xBA, 0x20, 0x00, 0x00, 0x00]);
        code.extend([0x64, 0x44, 0x0F, 0xB0, 0x0A]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 16, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(guest_qword(&mut boot, BASE + 0x20) & 0xFF, 0x66);
        assert_eq!(guest_qword(&mut boot, GS_DISTRACTOR + 0x20) & 0xFF, 0x42);
        let state = boot.board.state();
        assert_eq!((state.fs_base, state.gs_base), (BASE, GS_DISTRACTOR));
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x42);
        assert_eq!(state.gpr[od_isa::Register::R9 as usize], 0x7766);
        // Equal: ZF and PF set, everything else clear.
        assert_eq!(state.rflags & 0x8D5, 0x40 | 0x4);
    }

    // ---- S4-38L7bb: FS/GS-based MOV r/m8, r8 through the board facade ----

    #[test]
    fn gs_movb_store_after_wrmsr_gs_base_stores_al_and_r8b_ignoring_the_fs_base() {
        const BASE: u64 = 0x0200_0000;
        const FS_DISTRACTOR: u64 = 0x0300_0000;
        // CLI; WRMSR IA32_FS_BASE = FS_DISTRACTOR; WRMSR IA32_GS_BASE = BASE;
        // plain byte stores: [BASE+0xf] = 0xAA, [BASE+0x10] = 0x00, [BASE+0x11] = 0xBB,
        // [FS_DISTRACTOR+0x10] = 0x22; SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOV ECX,0x10; MOV EAX,0x5A; MOVB %al,%gs:(%rcx) (`65 88 01`, the S4-38L7ba boundary
        // form); MOV R8D,0x7E; MOVB %r8b,%gs:2(%rcx) (`65 44 88 41 02`); HLT.
        let mut code = vec![0xFA, 0xB9, 0x00, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(FS_DISTRACTOR as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xB9, 0x01, 0x01, 0x00, 0xC0]);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0xC6, 0x04, 0x25, 0x0F, 0x00, 0x00, 0x02, 0xAA]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02, 0x00]);
        code.extend([0xC6, 0x04, 0x25, 0x11, 0x00, 0x00, 0x02, 0xBB]);
        code.extend([0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x03, 0x22]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0xB9, 0x10, 0x00, 0x00, 0x00]);
        code.extend(mov_eax(0x5A));
        code.extend([0x65, 0x88, 0x01]);
        code.extend([0x41, 0xB8, 0x7E, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x44, 0x88, 0x41, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 20, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        // Bytes BASE+0xf..: AA (kept), 5A (new), BB (kept), 7E (new), then zeros.
        assert_eq!(guest_qword(&mut boot, BASE + 0x0F), 0x7EBB_5AAA);
        // The FS distractor byte is untouched: a store through the wrong base would change it.
        assert_eq!(guest_qword(&mut boot, FS_DISTRACTOR + 0x10) & 0xFF, 0x22);
        let state = boot.board.state();
        assert_eq!((state.gs_base, state.fs_base), (BASE, FS_DISTRACTOR));
        // Nothing else changed: CF set by SUB DL,1 survives and the sources keep their values.
        assert_eq!(state.rflags & 1, 1, "CF");
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], 0x5A);
        assert_eq!(state.gpr[od_isa::Register::R8 as usize], 0x7E);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x10);
    }

    // ---- S4-38L7ak: FS/GS-based MOVSXD r64, m32 through the board facade ----

    #[test]
    fn gs_movsxd_after_wrmsr_gs_base_sign_extends_a_dword_into_r15_and_changes_nothing_else() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR;
        // MOV R15,imm64 (a marker that must be replaced);
        // MOV DWORD [BASE+0x10],0x80000000 and MOV DWORD [BASE+0x14],0x7FFFFFFF (plain stores);
        // SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOVSXD R15,%gs:disp(%rip) (`65 4c 63 3d disp32`, the S4-38L7aj boundary shape,
        // effective address 0x10: the next RIP is after the whole instruction);
        // MOVSXD RBX,%gs:0x14 (`65 48 63 1c 25 14 00 00 00`: positive dword); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0x49, 0xBF]);
        code.extend(0x5555_5555_5555_5555u64.to_le_bytes());
        code.extend([0xC7, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend(0x8000_0000u32.to_le_bytes());
        code.extend([0xC7, 0x04, 0x25, 0x14, 0x00, 0x00, 0x02]);
        code.extend(0x7FFF_FFFFu32.to_le_bytes());
        code.extend([0x80, 0xEA, 0x01]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x4C, 0x63, 0x3D]);
        code.extend(disp.to_le_bytes());
        code.extend([0x65, 0x48, 0x63, 0x1C, 0x25, 0x14, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 12,
            "CLI, MOV, MOV, XOR, WRMSR, MOV R15, store x2, SUB, MOVSXD x2, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(
            guest_qword(&mut boot, BASE + 0x10),
            0x7FFF_FFFF_8000_0000,
            "no memory write"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.gpr[od_isa::Register::R15 as usize],
            0xFFFF_FFFF_8000_0000
        );
        assert_eq!(state.gpr[od_isa::Register::Rbx as usize], 0x7FFF_FFFF);
        // Nothing else changed: the registers set by the prologue keep their values, the loaded
        // dwords are still in memory and the CF set by SUB DL,1 is still set.
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], BASE);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0xC000_0101);
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 0xFF);
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7al: SGDT m through the board facade ----

    #[test]
    fn sgdt_after_wrmsr_gs_base_stores_the_board_gdtr_through_gs_and_plain() {
        const BASE: u64 = 0x0200_0000;
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR (IA32_GS_BASE = BASE);
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        // MOV RAX,marker; six qword stores that fill [BASE+0x10, BASE+0x28) and
        // [BASE+0x40, BASE+0x58) with the marker;
        code.extend(mov_rax_imm64(0xAAAA_AAAA_AAAA_AAAA));
        for offset in [0x10u8, 0x18, 0x20, 0x40, 0x48, 0x50] {
            code.extend([0x48, 0x89, 0x04, 0x25, offset, 0x00, 0x00, 0x02]);
        }
        // SUB DL,1 (0 - 1 borrows: CF = 1);
        code.extend([0x80, 0xEA, 0x01]);
        // MOV EBP,0x12+0x1a; SGDT %gs:-0x1a(%rbp) (`65 0f 01 45 e6`, the S4-38L7ak boundary shape
        // with GS: the address is BASE+0x12);
        code.push(0xBD);
        code.extend((0x12u32 + 0x1a).to_le_bytes());
        code.extend([0x65, 0x0F, 0x01, 0x45, 0xE6]);
        // MOV EBP,BASE+0x40+0x1a; SGDT -0x1a(%rbp) (`0f 01 45 e6`, the boundary bytes: the GS base
        // is ignored, the address is BASE+0x40); HLT.
        code.push(0xBD);
        code.extend((BASE as u32 + 0x40 + 0x1a).to_le_bytes());
        code.extend([0x0F, 0x01, 0x45, 0xE6]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 18,
            "CLI, MOV, MOV, XOR, WRMSR, MOV RAX, store x6, SUB, MOV EBP, SGDT, MOV EBP, SGDT, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        // The board entry GDTR (base 0x12_0000, limit 31), not changed by the program.
        let gdtr = boot.board.state().gdtr.expect("the board models a GDTR");
        assert_eq!((gdtr.base, gdtr.limit), (0x12_0000, 31));
        let mut want = [0u8; 10];
        want[..2].copy_from_slice(&31u16.to_le_bytes());
        want[2..].copy_from_slice(&0x12_0000u64.to_le_bytes());
        for (start, stored_at) in [(0x10u64, 0x12u64), (0x40, 0x40)] {
            let mut bytes = [0u8; 24];
            read(&mut boot.board, BASE + start, &mut bytes).expect("read");
            let mut expected = [0xAAu8; 24];
            let offset = (stored_at - start) as usize;
            expected[offset..offset + 10].copy_from_slice(&want);
            assert_eq!(bytes, expected, "SGDT at BASE+{stored_at:#x}");
        }
        let state = boot.board.state();
        // Nothing else changed: the registers keep their values and the CF set by SUB stays set.
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            0xAAAA_AAAA_AAAA_AAAA
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rbp as usize],
            BASE + 0x40 + 0x1a
        );
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 0xFF);
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7am: LTR r/m16 (register form) through the board facade ----

    #[test]
    fn ltr_ax_loads_tr_from_a_tss_descriptor_in_the_board_gdt_and_marks_it_busy() {
        // The board GDT (base 0x12_0000, limit 31) has no TSS entry, so the program writes an
        // available 64-bit TSS descriptor over the entry at selector 0x10 (its 16 bytes end at
        // the limit) and loads it.
        const GDT: u32 = 0x12_0000;
        let mut code = Vec::new();
        // MOV RAX,low qword; MOV [GDT+0x10],RAX; MOV RAX,high qword; MOV [GDT+0x18],RAX;
        code.extend(mov_rax_imm64(0x1200_8934_5678_206b));
        code.extend([0x48, 0x89, 0x04, 0x25]);
        code.extend((GDT + 0x10).to_le_bytes());
        code.extend(mov_rax_imm64(0x0000_0000_ffff_fe00));
        code.extend([0x48, 0x89, 0x04, 0x25]);
        code.extend((GDT + 0x18).to_le_bytes());
        // MOV EAX,0x10; LTR AX (`0f 00 d8`, the S4-38L7al boundary bytes); HLT.
        code.extend(mov_eax(0x10));
        code.extend([0x0F, 0x00, 0xD8, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        assert!(
            boot.board.state().tr.is_none(),
            "the board entry state has no TR"
        );
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 7,
            "MOV, store, MOV, store, MOV EAX, LTR, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(
            state.tr,
            Some(od_isa::TaskRegisterCache {
                selector: 0x10,
                base: 0xffff_fe00_1234_5678,
                limit: 0x206b,
                access: 0x8b,
                flags: 0,
            })
        );
        let gdtr = state.gdtr.expect("the board models a GDTR");
        assert_eq!((gdtr.base, gdtr.limit), (0x12_0000, 31));
        // The busy bit is set in the GDT; the other descriptor bytes are untouched.
        let mut bytes = [0u8; 16];
        read(&mut boot.board, u64::from(GDT) + 0x10, &mut bytes).expect("read");
        assert_eq!(
            bytes,
            [
                0x6b, 0x20, 0x78, 0x56, 0x34, 0x8b, 0x00, 0x12, 0x00, 0xfe, 0xff, 0xff, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn ltr_ax_with_a_selector_beyond_the_board_gdt_limit_is_gp_and_loads_nothing() {
        // The board GDT limit is 31: selector 0x40 is #GP(0x40) before any descriptor access, so
        // the facade stops on the fault without a TR and without delivery (no IDT is loaded).
        let mut code = mov_eax(0x40);
        code.extend([0x0F, 0x00, 0xD8, 0xF4]);
        let ltr_offset = code.len() as u64 - 4;
        let (mut boot, file) = boot_with_entry(&code);
        let ltr_rip = boot.board.state().rip + ltr_offset;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        // The exit is the architectural #GP(0x40) of the executed LTR, not an `Unsupported`
        // refusal (opcode 0x00) and not the HLT after it.
        let PagedProbeExit::Fault(fault) = &record.exit else {
            panic!("expected the #GP fault exit: {record:?}");
        };
        assert_eq!(fault.vector, 13, "{record:?}");
        assert_eq!(fault.error_code, Some(0x40), "{record:?}");
        assert_eq!(record.exit_kind, "fault");
        assert_eq!(record.retired, 1, "only the MOV EAX retired: {record:?}");
        // RIP stays at the faulting LTR (`0f 00 d8`), not at the HLT after it.
        assert_eq!(record.rip, ltr_rip, "{record:?}");
        assert_eq!(boot.board.state().rip, ltr_rip);
        assert_eq!(record.rip_bytes[..3], [0x0F, 0x00, 0xD8], "{record:?}");
        assert!(boot.board.state().tr.is_none());
    }

    // ---- S4-38L7aw: LLDT r/m16 (register form, null selector) through the board facade ----

    #[test]
    fn lldt_si_with_a_null_selector_marks_ldtr_unusable_through_the_board() {
        // XOR ESI,ESI; LLDT SI (`0f 00 d6`, the S4-38L7av boundary bytes); INC EAX (`ff c0`) to
        // show that execution continues after the LLDT; HLT.
        let code = [0x31, 0xF6, 0x0F, 0x00, 0xD6, 0xFF, 0xC0, 0xF4];
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(
            boot.board.state().ldtr_null_selector,
            None,
            "the board entry state has no LDTR value"
        );
        let rax_before = boot.board.state().gpr[od_isa::Register::Rax as usize];
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "XOR, LLDT, INC, HLT");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.ldtr_null_selector, Some(0));
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            rax_before.wrapping_add(1) & 0xffff_ffff
        );
        // The GDTR is the board entry GDTR; TR is still not modeled. The stop record carries no
        // LDTR field: the state is read through the board.
        let gdtr = state.gdtr.expect("the board models a GDTR");
        assert_eq!((gdtr.base, gdtr.limit), (0x12_0000, 31));
        assert!(state.tr.is_none());
    }

    #[test]
    fn lldt_si_with_a_non_null_selector_is_refused_with_opcode_0x00_through_the_board() {
        // MOV ESI,8; LLDT SI; HLT: the non-null load (GDT read, LDT descriptor checks) is not in
        // the slice, so the facade stops on the LLDT with `Unsupported { opcode: 0x00 }`.
        let mut code = vec![0xBE];
        code.extend(8u32.to_le_bytes());
        code.extend([0x0F, 0x00, 0xD6, 0xF4]);
        let lldt_offset = code.len() as u64 - 4;
        let (mut boot, file) = boot_with_entry(&code);
        let lldt_rip = boot.board.state().rip + lldt_offset;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x00 },
            "{record:?}"
        );
        assert_eq!(record.exit_kind, "unsupported");
        assert_eq!(record.retired, 1, "only the MOV ESI retired: {record:?}");
        assert_eq!(record.rip, lldt_rip, "{record:?}");
        assert_eq!(record.rip_bytes[..3], [0x0F, 0x00, 0xD6], "{record:?}");
        assert_eq!(boot.board.state().ldtr_null_selector, None);
    }

    // ---- S4-38L7ax: MOV DRn, r64 (register form, write only) through the board facade ----

    #[test]
    fn mov_dr7_rsi_stores_the_value_readable_through_the_board_state() {
        // MOV ESI,0x400; MOV DR7,RSI (`0f 23 fe`, the S4-38L7aw boundary bytes); XOR ESI,ESI;
        // MOV DR1,RSI (`0f 23 ce`); INC EAX to show execution continues; HLT.
        let mut code = vec![0xBE];
        code.extend(0x400u32.to_le_bytes());
        code.extend([
            0x0F, 0x23, 0xFE, 0x31, 0xF6, 0x0F, 0x23, 0xCE, 0xFF, 0xC0, 0xF4,
        ]);
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(
            boot.board.state().debug_registers,
            [None; 8],
            "the board entry state has no debug register value"
        );
        let rax_before = boot.board.state().gpr[od_isa::Register::Rax as usize];
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "MOV, MOV DR7, XOR, MOV DR1, INC, HLT");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        let mut expected = [None; 8];
        expected[7] = Some(0x400);
        expected[1] = Some(0);
        assert_eq!(state.debug_registers, expected);
        assert_eq!(
            state.gpr[od_isa::Register::Rax as usize],
            rax_before.wrapping_add(1) & 0xffff_ffff
        );
    }

    #[test]
    fn mov_dr4_is_refused_with_opcode_0x23_through_the_board() {
        // MOV DR4,RAX (`0f 23 e0`): the CR4.DE rule is not modeled, so the facade stops on it.
        let code = [0x0F, 0x23, 0xE0, 0xF4];
        let (mut boot, file) = boot_with_entry(&code);
        let rip = boot.board.state().rip;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x23 },
            "{record:?}"
        );
        assert_eq!(record.retired, 0, "{record:?}");
        assert_eq!(record.rip, rip, "{record:?}");
        assert_eq!(record.rip_bytes[..3], [0x0F, 0x23, 0xE0], "{record:?}");
        assert_eq!(boot.board.state().debug_registers, [None; 8]);
    }

    // ---- S4-38L7aj: FS/GS-based MOV m32/m64, imm32 through the board facade ----

    #[test]
    fn gs_mov_imm32_after_wrmsr_gs_base_stores_a_dword_and_a_qword_and_reads_back_through_gs() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX; MOV [BASE+0x18],RAX (plain, no override);
        // SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOV DWORD %gs:disp(%rip),0xEC (`65 c7 05 disp32 ec 00 00 00`, effective address 0x10:
        // the S4-38L7ai boundary shape, the next RIP is after the immediate: only the low dword
        // changes);
        // MOV QWORD %gs:0x18,-2 (`65 48 c7 04 25 18 00 00 00 fe ff ff ff`: sign-extended);
        // MOV RBX,%gs:0x10 and MOV RCX,%gs:0x18 (`65 48 8b ..`: the read back through the same
        // base); MOV RDX,[BASE+0x10] (plain); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x18, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        let next_rip = ENTRY + code.len() as u64 + 11;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xC7, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0xEC, 0x00, 0x00, 0x00]);
        code.extend([
            0x65, 0x48, 0xC7, 0x04, 0x25, 0x18, 0x00, 0x00, 0x00, 0xFE, 0xFF, 0xFF, 0xFF,
        ]);
        code.extend([0x65, 0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x48, 0x8B, 0x0C, 0x25, 0x18, 0x00, 0x00, 0x00]);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 15,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store x2, SUB, MOV imm x2, load x3, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the low dword of the first qword was replaced; the upper dword stayed.
        let expected = 0x0123_4567_0000_00ECu64;
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            expected,
            "via GS"
        );
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], expected, "plain");
        // The second qword holds the sign-extended -2.
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], u64::MAX - 1);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // MOV changes no flag: the CF set by SUB DL,1 is still set.
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7an: FS/GS-based MOV m32, r32 / m64, r64 through the board facade ----

    #[test]
    fn gs_mov_store_r32_after_wrmsr_gs_base_writes_four_bytes_of_eax_and_keeps_the_neighbours() {
        const BASE: u64 = 0x0200_0000;
        const PATTERN: u64 = 0xAAAA_AAAA_AAAA_AAAA;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,PATTERN;
        // MOV [BASE+0x10],RAX; MOV [BASE+0x18],RAX (plain); MOV RAX,VALUE;
        // SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOV %eax,%gs:disp(%rip) (`65 89 05 disp32`, effective address 0x14: the S4-38L7am
        // boundary shape, the next RIP is after the 7 bytes: only the four bytes 0x14..0x18
        // change);
        // MOV RBX,[BASE+0x10] and MOV RCX,[BASE+0x18] (plain);
        // MOV %rax,%gs:0x20 (`65 48 89 04 25 20 00 00 00`) and MOV RDX,%gs:0x20 (`65 48 8b ..`:
        // the read back through the same base); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(PATTERN));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x18, 0x00, 0x00, 0x02]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x80, 0xEA, 0x01]);
        let next_rip = ENTRY + code.len() as u64 + 7;
        let disp = (0x14u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x89, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x8B, 0x0C, 0x25, 0x18, 0x00, 0x00, 0x02]);
        code.extend([0x65, 0x48, 0x89, 0x04, 0x25, 0x20, 0x00, 0x00, 0x00]);
        code.extend([0x65, 0x48, 0x8B, 0x14, 0x25, 0x20, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 16,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store x2, MOV, SUB, GS dword store, load x2, GS qword \
             store, GS load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the dword at 0x14 changed; the dword below and the qword above stayed.
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            0x89AB_CDEF_AAAA_AAAA,
            "bytes 0x10..0x18"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rcx as usize],
            PATTERN,
            "bytes 0x18..0x20"
        );
        // The qword store and its read back through the same GS base.
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], VALUE);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE, "RAX kept");
        // MOV changes no flag: the CF set by SUB DL,1 is still set.
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7av: FS/GS-based MOV m16, r16 through the board facade ----

    #[test]
    fn gs_mov_store_r16_after_wrmsr_gs_base_writes_two_bytes_and_keeps_the_neighbours() {
        const BASE: u64 = 0x0200_0000;
        const PATTERN: u64 = 0xAAAA_AAAA_AAAA_AAAA;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,PATTERN;
        // MOV [BASE+0x10],RAX; MOV [BASE+0x18],RAX (plain); MOV RAX,VALUE;
        // SUB DL,1 (0 - 1 borrows: CF = 1); MOV EDX,0x55667788;
        // MOV %ax,%gs:disp(%rip) (`65 66 89 05 disp32`, effective address 0x12: the S4-38L7au
        // boundary shape, the next RIP is after the 8 bytes: only the two bytes 0x12..0x14
        // change); MOV %dx,%gs:disp(%rip) (`65 66 89 15 disp32`, effective address 0x14);
        // MOV RBX,[BASE+0x10] and MOV RCX,[BASE+0x18] (plain); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(PATTERN));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x89, 0x04, 0x25, 0x18, 0x00, 0x00, 0x02]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0xBA, 0x88, 0x77, 0x66, 0x55]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x12u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x66, 0x89, 0x05]);
        code.extend(disp.to_le_bytes());
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x14u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x66, 0x89, 0x15]);
        code.extend(disp.to_le_bytes());
        code.extend([0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0x8B, 0x0C, 0x25, 0x18, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 16,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store x2, MOV, SUB, MOV EDX, GS word store x2, \
             load x2, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the words at 0x12 (AX = 0xCDEF) and 0x14 (DX = 0x7788) changed; the bytes around
        // them and the qword above stayed.
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            0xAAAA_7788_CDEF_AAAA,
            "bytes 0x10..0x18"
        );
        assert_eq!(
            state.gpr[od_isa::Register::Rcx as usize],
            PATTERN,
            "bytes 0x18..0x20"
        );
        // The source registers are unchanged.
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE, "RAX kept");
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0x5566_7788,
            "RDX kept"
        );
        // MOV changes no flag: the CF set by SUB DL,1 is still set.
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7ae: FS/GS-based MOV m8, imm8 through the board facade ----

    #[test]
    fn gs_mov_byte_immediate_after_wrmsr_gs_base_stores_one_byte_and_reads_back_through_gs() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_CDEF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOV BYTE %gs:0x10,0x01 (`65 c6 04 25 10 00 00 00 01`: only the low byte changes);
        // MOV BYTE %gs:disp(%rip),0xff (`65 c6 05 disp32 ff`, effective address 0x11: the
        // S4-38L7ad boundary shape, the next RIP is after the immediate);
        // MOV RBX,%gs:0x10 (`65 48 8b 1c 25 10 00 00 00`: the read back through the same base);
        // MOV RDX,[BASE+0x10] (plain); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0x65, 0xC6, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00, 0x01]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x11u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xC6, 0x05]);
        code.extend(disp.to_le_bytes());
        code.push(0xFF);
        code.extend([0x65, 0x48, 0x8B, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 13,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, SUB, MOV m8 x2, load, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Bytes 0 and 1 of the qword were replaced; the rest stayed.
        let expected = 0x0123_4567_89AB_FF01u64;
        assert_eq!(
            state.gpr[od_isa::Register::Rbx as usize],
            expected,
            "via GS"
        );
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], expected, "plain");
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // MOV changes no flag: the CF set by SUB DL,1 is still set.
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7ad: FS/GS-based MOVZX r32/r64, m16 through the board facade ----

    #[test]
    fn gs_movzx_word_after_wrmsr_gs_base_zero_extends_and_leaves_memory_and_flags_alone() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_89AB_8001;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); MOV RDI,-1; SUB DL,1 (0 - 1 borrows: CF = 1);
        // MOVZX EDI,WORD %gs:0x10 (`65 0f b7 3c 25 10 00 00 00`: the word 0x8001 is not sign-
        // extended and the upper half of RDI is zeroed);
        // MOVZX ECX,WORD %gs:disp(%rip) (`65 0f b7 0d disp32`, effective address 0x10: the
        // S4-38L7ac boundary shape, the next RIP is after the disp32);
        // MOVZX RBX,WORD %gs:0x10 (`65 48 0f b7 1c 25 10 00 00 00`, REX.W);
        // MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x48, 0xC7, 0xC7, 0xFF, 0xFF, 0xFF, 0xFF]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0x65, 0x0F, 0xB7, 0x3C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        let next_rip = ENTRY + code.len() as u64 + 8;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0x0F, 0xB7, 0x0D]);
        code.extend(disp.to_le_bytes());
        code.extend([0x65, 0x48, 0x0F, 0xB7, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 14,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, MOV RDI, SUB, MOVZX x3, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(state.gpr[od_isa::Register::Rdi as usize], 0x8001, "EDI");
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x8001, "ECX");
        assert_eq!(state.gpr[od_isa::Register::Rbx as usize], 0x8001, "RBX");
        // The loads wrote nothing: the whole qword is still there (loaded into RDX).
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], VALUE);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // MOVZX changes no flag: the CF set by SUB DL,1 is still set.
        assert_eq!(state.rflags & 1, 1, "CF");
    }

    // ---- S4-38L7ac: FS/GS-based TEST m32/m64, imm32 through the board facade ----

    #[test]
    fn gs_test_dword_after_wrmsr_gs_base_reads_one_dword_sets_flags_and_leaves_memory_alone() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0x0123_4567_8000_0001;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); SUB DL,1 (0 - 1 borrows: CF = 1);
        // TEST %gs:0x10,$0x7fffffff (`65 f7 04 25 10 00 00 00 ff ff ff 7f`: 0x80000001 &
        // 0x7fffffff = 1, SF = 0, ZF = 0, PF = 0, clears CF and OF);
        // ADD DL,2 (0xff + 2 carries: CF = 1);
        // TEST %gs:disp(%rip),$0x7fffffff (`65 f7 05 disp32 ff ff ff 7f`, effective address 0x10:
        // the S4-38L7ab boundary shape, the next RIP is after the imm32: the same result);
        // TESTQ %gs:0x10,$-1 (`65 48 f7 04 25 10 00 00 00 ff ff ff ff`: the immediate is sign-
        // extended, the qword 0x0123456780000001 & -1 has SF = 0, ZF = 0, PF = 0);
        // MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([
            0x65, 0xF7, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0x7F,
        ]);
        code.extend([0x80, 0xC2, 0x02]);
        let next_rip = ENTRY + code.len() as u64 + 11;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xF7, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0xFF, 0xFF, 0xFF, 0x7F]);
        // Sets CF again so that the qword TEST visibly clears it.
        code.extend([0x80, 0xC2, 0xFF]);
        code.extend([
            0x65, 0x48, 0xF7, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00, 0xFF, 0xFF, 0xFF, 0xFF,
        ]);
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 15,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, SUB, TEST, ADD, TEST, ADD, TESTQ, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // The TESTs wrote nothing: the whole qword is still there (loaded into RDX), RAX is
        // untouched.
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], VALUE);
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // The last TESTQ gave 0x0123456780000001: SF = 0, ZF = 0, low byte 0x01 -> PF = 0, and
        // cleared the CF that the second ADD had set.
        assert_eq!(state.rflags & 1, 0, "CF");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & (1 << 2), 0, "PF");
        assert_eq!(state.rflags & (1 << 7), 0, "SF");
    }

    // ---- S4-38L7p: FS/GS-based INC m32 through the board facade ----

    #[test]
    fn gs_inc_dword_after_wrmsr_gs_base_increments_four_bytes_and_keeps_cf_and_the_neighbour() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0xAABB_CCDD_7FFF_FFFF;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); SUB DL,1 (0 - 1 borrows: CF = 1);
        // INC %gs:0x10 (`65 ff 04 25 10 00 00 00`: 0x7fffffff + 1 = 0x80000000, OF and SF set, CF
        // kept);
        // INC %gs:disp(%rip) (`65 ff 05 disp32`, effective address 0x10, the next RIP is after the
        // instruction: 0x80000000 + 1 = 0x80000001); MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0x65, 0xFF, 0x04, 0x25, 0x10, 0x00, 0x00, 0x00]);
        let next_rip = ENTRY + code.len() as u64 + 7;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xFF, 0x05]);
        code.extend(disp.to_le_bytes());
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 12,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, SUB, INC, INC, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the low dword changed (0x7fffffff -> 0x80000001); the neighbouring dword and the
        // other registers are untouched.
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0xAABB_CCDD_8000_0001
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // The last INC gave 0x80000001: SF = 1, ZF = 0, OF = 0 (not an overflow this time), PF = 0
        // (one bit in the low byte), and the CF set by the SUB survived both INCs.
        assert_ne!(state.rflags & 1, 0, "CF kept");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_ne!(state.rflags & (1 << 7), 0, "SF");
        assert_eq!(state.rflags & (1 << 2), 0, "PF");
    }

    // ---- S4-38L7q: FS/GS-based DEC m32 through the board facade ----

    #[test]
    fn gs_dec_dword_after_wrmsr_gs_base_decrements_four_bytes_and_keeps_cf_and_the_neighbour() {
        const BASE: u64 = 0x0200_0000;
        const VALUE: u64 = 0xAABB_CCDD_8000_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR; MOV RAX,VALUE;
        // MOV [BASE+0x10],RAX (plain, no override); SUB DL,1 (0 - 1 borrows: CF = 1);
        // DEC %gs:0x10 (`65 ff 0c 25 10 00 00 00`: 0x80000000 - 1 = 0x7fffffff, OF set, SF clear,
        // CF kept);
        // DEC %gs:disp(%rip) (`65 ff 0d disp32`, effective address 0x10, the next RIP is after the
        // instruction: 0x7fffffff - 1 = 0x7ffffffe); MOV RDX,[BASE+0x10]; HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend(mov_rax_imm64(VALUE));
        code.extend([0x48, 0x89, 0x04, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.extend([0x80, 0xEA, 0x01]);
        code.extend([0x65, 0xFF, 0x0C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        let next_rip = ENTRY + code.len() as u64 + 7;
        let disp = (0x10u64.wrapping_sub(next_rip)) as u32;
        code.extend([0x65, 0xFF, 0x0D]);
        code.extend(disp.to_le_bytes());
        code.extend([0x48, 0x8B, 0x14, 0x25, 0x10, 0x00, 0x00, 0x02]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 12,
            "CLI, MOV, MOV, XOR, WRMSR, MOV, store, SUB, DEC, DEC, load, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        // Only the low dword changed (0x80000000 -> 0x7ffffffe); the neighbouring dword and the
        // other registers are untouched.
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            0xAABB_CCDD_7FFF_FFFE
        );
        assert_eq!(state.gpr[od_isa::Register::Rax as usize], VALUE);
        // The last DEC gave 0x7ffffffe: SF = 0, ZF = 0, OF = 0 (the overflow was the first DEC),
        // PF = 0 (seven bits in the low byte), and the CF set by the SUB survived both DECs.
        assert_ne!(state.rflags & 1, 0, "CF kept");
        assert_eq!(state.rflags & (1 << 11), 0, "OF");
        assert_eq!(state.rflags & (1 << 6), 0, "ZF");
        assert_eq!(state.rflags & (1 << 7), 0, "SF");
        assert_eq!(state.rflags & (1 << 2), 0, "PF");
    }

    // ---- S4-38L7n: LOCK BTS m64, r64 through the board facade ----

    #[test]
    fn lock_bts_sets_a_bit_in_a_bit_string_in_guest_memory_and_leaves_neighbours_alone() {
        // MOV RBX,0x0200_0100; MOV RDI,0x1100; MOV [RBX+8],RDI; MOV [RBX+16],RDI;
        // MOV [RBX+24],RDI; MOV RAX,64*1+5;
        // LOCK BTS [RBX],RAX (bit 69: qword RBX+8, bit 5; the bit was clear, CF = 0);
        // MOV RAX,-1; LOCK BTS [RBX],RAX (bit -1: qword RBX-8, bit 63);
        // MOV RDX,[RBX+8]; MOV RCX,[RBX+16]; MOV RSI,[RBX+24]; MOV R8,[RBX-8]; HLT.
        let mut code = vec![0x48, 0xBB];
        code.extend(0x0200_0100u64.to_le_bytes());
        code.extend([0x48, 0xBF]);
        code.extend(0x1100u64.to_le_bytes());
        code.extend([0x48, 0x89, 0x7B, 0x08]);
        code.extend([0x48, 0x89, 0x7B, 0x10]);
        code.extend([0x48, 0x89, 0x7B, 0x18]);
        code.extend(mov_rax_imm64(64 + 5));
        code.extend([0xF0, 0x48, 0x0F, 0xAB, 0x03]);
        code.extend(mov_rax_imm64(u64::MAX));
        code.extend([0xF0, 0x48, 0x0F, 0xAB, 0x03]);
        code.extend([0x48, 0x8B, 0x53, 0x08]);
        code.extend([0x48, 0x8B, 0x4B, 0x10]);
        code.extend([0x48, 0x8B, 0x73, 0x18]);
        code.extend([0x4C, 0x8B, 0x43, 0xF8]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 14, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // Bit 69 of the string set RBX+8 bit 5 only; RBX+16 and RBX+24 keep their marker.
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 0x1100 | (1 << 5));
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x1100);
        assert_eq!(state.gpr[od_isa::Register::Rsi as usize], 0x1100);
        // Bit -1 set bit 63 of the qword at RBX-8 (it was zero RAM).
        assert_eq!(state.gpr[od_isa::Register::R8 as usize], 1 << 63);
        // Both bits were clear: CF = 0.
        assert_eq!(state.rflags & 1, 0);
    }

    // ---- S4-38L7bi: LOCK BTS m32/m64, imm8 through the board facade ----

    #[test]
    fn lock_bts_imm_sets_bits_in_guest_memory_and_leaves_neighbours_alone() {
        const BASE: u64 = 0x0200_0100;
        // MOV R12,BASE; MOV RDI,0x1100; MOV [R12],RDI; MOV [R12+8],RDI; MOV [R12-8],RDI;
        // LOCK BTS $0,(%r12) (the S4-38L7bh boundary: qword bit 0 was clear, CF = 0, 0x1101);
        // LOCK BTS $0x45,(%r12) without REX.W (dword bit 0x45 % 32 = 5, 0x1121);
        // LOCK BTS $0,(%r12) again (bit already set: CF = 1, the value stays 0x1121);
        // MOV RDX,[R12]; MOV RCX,[R12+8]; MOV RSI,[R12-8]; HLT.
        let mut code = vec![0x49, 0xBC];
        code.extend(BASE.to_le_bytes());
        code.extend([0x48, 0xBF]);
        code.extend(0x1100u64.to_le_bytes());
        code.extend([0x49, 0x89, 0x3C, 0x24]);
        code.extend([0x49, 0x89, 0x7C, 0x24, 0x08]);
        code.extend([0x49, 0x89, 0x7C, 0x24, 0xF8]);
        code.extend([0xF0, 0x49, 0x0F, 0xBA, 0x2C, 0x24, 0x00]);
        code.extend([0xF0, 0x41, 0x0F, 0xBA, 0x2C, 0x24, 0x45]);
        code.extend([0xF0, 0x49, 0x0F, 0xBA, 0x2C, 0x24, 0x00]);
        code.extend([0x49, 0x8B, 0x14, 0x24]);
        code.extend([0x49, 0x8B, 0x4C, 0x24, 0x08]);
        code.extend([0x49, 0x8B, 0x74, 0x24, 0xF8]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 12,
            "2 MOV, 3 stores, 3 LOCK BTS, 3 loads, HLT"
        );
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        assert_eq!(guest_qword(&mut boot, BASE), 0x1121);
        assert_eq!(guest_qword(&mut boot, BASE + 8), 0x1100);
        assert_eq!(guest_qword(&mut boot, BASE - 8), 0x1100);
        let state = boot.board.state();
        assert_eq!(state.gpr[od_isa::Register::Rdx as usize], 0x1121);
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], 0x1100);
        assert_eq!(state.gpr[od_isa::Register::Rsi as usize], 0x1100);
        // The last LOCK BTS found its bit already set: CF = 1.
        assert_eq!(state.rflags & 1, 1);
    }

    // ---- S4-38L7t: LOCK BTR m64, r64 through the board facade ----

    #[test]
    fn lock_btr_clears_a_bit_in_a_bit_string_in_guest_memory_and_leaves_neighbours_alone() {
        // MOV R13,0x0200_0100; MOV RDI,-1; MOV [R13+0x28],RDI; MOV [R13+0x30],RDI;
        // MOV [R13+0x38],RDI; MOV [R13+0x40],RDI; MOV R12,64*1+5;
        // LOCK BTR [R13+0x30],R12 (bit 69: qword R13+0x38, bit 5; the bit was set, CF = 1);
        // MOV R12,-1; LOCK BTR [R13+0x30],R12 (bit -1: qword R13+0x28, bit 63);
        // MOV RDX,[R13+0x38]; MOV RCX,[R13+0x40]; MOV RSI,[R13+0x30]; MOV R8,[R13+0x28]; HLT.
        let mut code = vec![0x49, 0xBD];
        code.extend(0x0200_0100u64.to_le_bytes());
        code.extend([0x48, 0xBF]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0x49, 0x89, 0x7D, 0x28]);
        code.extend([0x49, 0x89, 0x7D, 0x30]);
        code.extend([0x49, 0x89, 0x7D, 0x38]);
        code.extend([0x49, 0x89, 0x7D, 0x40]);
        code.extend([0x49, 0xBC]);
        code.extend((64u64 + 5).to_le_bytes());
        code.extend([0xF0, 0x4D, 0x0F, 0xB3, 0x65, 0x30]);
        code.extend([0x49, 0xBC]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0xF0, 0x4D, 0x0F, 0xB3, 0x65, 0x30]);
        code.extend([0x49, 0x8B, 0x55, 0x38]);
        code.extend([0x49, 0x8B, 0x4D, 0x40]);
        code.extend([0x49, 0x8B, 0x75, 0x30]);
        code.extend([0x4D, 0x8B, 0x45, 0x28]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 15, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        // Bit 69 of the string cleared R13+0x38 bit 5 only; R13+0x40 and R13+0x30 stay all ones.
        assert_eq!(
            state.gpr[od_isa::Register::Rdx as usize],
            u64::MAX & !(1 << 5)
        );
        assert_eq!(state.gpr[od_isa::Register::Rcx as usize], u64::MAX);
        assert_eq!(state.gpr[od_isa::Register::Rsi as usize], u64::MAX);
        // Bit -1 cleared bit 63 of the qword at R13+0x28.
        assert_eq!(state.gpr[od_isa::Register::R8 as usize], u64::MAX >> 1);
        // Both bits were set: CF = 1.
        assert_eq!(state.rflags & 1, 1);
    }

    // ---- S4-38L7u: MOV r/m, Sreg through the board facade ----

    #[test]
    fn mov_sreg_store_reads_loaded_selectors_and_writes_two_bytes_at_the_gs_base() {
        const BASE: u64 = 0x0200_0000;
        // CLI; MOV ECX,0xC0000101; MOV EAX,BASE; XOR EDX,EDX; WRMSR (GS base = BASE);
        // MOV %ds,%esi (the boot DS); MOV EAX,0x18; MOV SS,AX; MOV DS,AX;
        // MOV RBX,-1; MOV %ss,%ebx (the upper half is cleared); MOV RDI,-1; MOV %ds,%rdi;
        // MOV %ds,%gs:0x10 (two bytes); HLT.
        let mut code = vec![0xFA, 0xB9, 0x01, 0x01, 0x00, 0xC0];
        code.extend(mov_eax(BASE as u32));
        code.extend([0x31, 0xD2, 0x0F, 0x30]);
        code.extend([0x8C, 0xDE]);
        code.extend(mov_eax(0x18));
        code.extend([0x8E, 0xD0, 0x8E, 0xD8]);
        code.extend([0x48, 0xBB]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0x8C, 0xD3]);
        code.extend([0x48, 0xBF]);
        code.extend(u64::MAX.to_le_bytes());
        code.extend([0x48, 0x8C, 0xDF]);
        code.extend([0x65, 0x8C, 0x1C, 0x25, 0x10, 0x00, 0x00, 0x00]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 15, "{record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gs_base, BASE);
        assert_eq!(
            state.gpr[Register::Rsi as usize],
            0x18,
            "DS as the boot state holds it before the load"
        );
        assert_eq!(state.gpr[Register::Rbx as usize], 0x18, "SS, zero-extended");
        assert_eq!(state.gpr[Register::Rdi as usize], 0x18, "DS through REX.W");
        // Two bytes written at GS base + 0x10; the other bytes of the qword stay zero.
        assert_eq!(guest_qword(&mut boot, BASE + 0x10), 0x18);
    }

    // ---- S4-38L7v: repeated CS/DS/ES/SS prefixes through the board facade ----

    #[test]
    fn repeated_cs_ds_prefixes_before_xor_and_mov_execute_like_the_unprefixed_forms() {
        // MOV EAX,0x12345678; `ds ds mov %rax,%rbx` (3E 3E 48 89 C3);
        // `cs cs cs xor %eax,%eax` (2E 2E 2E 31 C0, the S4-38L7u diagnostic boundary); HLT.
        let mut code = Vec::new();
        code.extend(mov_eax(0x1234_5678));
        code.extend([0x3E, 0x3E, 0x48, 0x89, 0xC3]);
        code.extend([0x2E, 0x2E, 0x2E, 0x31, 0xC0]);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let rip0 = boot.board.state().rip;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "MOV, MOV, XOR, HLT: {record:?}");
        assert!(
            record.page_fault.is_none() && record.memory_error.is_none(),
            "{record:?}"
        );
        let state = boot.board.state();
        assert_eq!(state.gpr[Register::Rax as usize], 0, "XOR cleared RAX");
        assert_eq!(
            state.gpr[Register::Rbx as usize],
            0x1234_5678,
            "the MOV copied RAX"
        );
        // ZF = 1, PF = 1, CF = 0, OF = 0, SF = 0.
        assert_eq!(state.rflags & (0x1 | 0x4 | 0x40 | 0x80 | 0x800), 0x44);
        // RIP advanced over every prefix byte (HLT retires with RIP after it).
        assert_eq!(state.rip, rip0 + code.len() as u64);
    }

    // ---- S4-38L7w: PCI configuration ports (OUT/IN dword) through the board facade ----

    /// `MOV EDX, imm32` (`BA id`).
    fn mov_edx(value: u32) -> Vec<u8> {
        let mut code = vec![0xBA];
        code.extend_from_slice(&value.to_le_bytes());
        code
    }

    fn pci_boundary_code() -> Vec<u8> {
        // The S4-38L7v boundary: `out %eax,%dx` with EAX = 0x80001000, EDX = 0xcf8; then the data
        // read `in %dx,%eax` at 0xcfc, then HLT.
        let mut code = Vec::new();
        code.extend(mov_eax(0x8000_1000));
        code.extend(mov_edx(0xcf8));
        code.push(0xEF);
        code.extend(mov_edx(0xcfc));
        code.push(0xED);
        code.push(0xF4);
        code
    }

    #[test]
    fn pci_config_out_then_data_in_runs_through_the_board_set_and_the_address_is_kept() {
        let code = pci_boundary_code();
        let (mut boot, file) = boot_with_entry(&code);
        let rip0 = boot.board.state().rip;
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "MOV, MOV, OUT, MOV, IN, HLT: {record:?}");
        assert!(record.set_members.contains(&"pci_config".to_string()));
        let state = boot.board.state();
        assert_eq!(state.gpr[Register::Rax as usize], 0xffff_ffff, "no device");
        assert_eq!(state.gpr[Register::Rdx as usize], 0xcfc);
        assert_eq!(state.rip, rip0 + code.len() as u64);
        assert_eq!(record.pci_config_address, 0x8000_1000);
        assert_eq!(boot.board.pci_config_address(), 0x8000_1000);
    }

    #[test]
    fn pci_config_ports_are_outside_a_set_without_the_member() {
        let code = pci_boundary_code();
        let (mut boot, file) = boot_with_entry(&code);
        let rip_out = boot.board.state().rip + 10;
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                pci_config: false,
                ..halting_default()
            },
        );
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0xEF },
            "{record:?}"
        );
        assert_eq!(record.retired, 2);
        assert_eq!(record.rip, rip_out);
        assert_eq!(record.pci_config_address, 0);
        assert!(!record.set_members.contains(&"pci_config".to_string()));
    }

    #[test]
    fn pci_config_address_chains_between_two_runs_on_one_board() {
        // MOV EAX,0x80001000; MOV EDX,0xcf8; OUT DX,EAX | XOR EAX,EAX; IN EAX,DX; HLT.
        let mut code = Vec::new();
        code.extend(mov_eax(0x8000_1000));
        code.extend(mov_edx(0xcf8));
        code.push(0xEF);
        code.extend([0x31, 0xC0, 0xED, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let members = halting_default();
        // Run 1 stops by budget right after the OUT.
        let first = run_set_facade(&mut boot, &file, 3, members);
        assert_eq!(first.retired, 3, "{first:?}");
        assert_eq!(first.pci_config_address, 0x8000_1000);
        assert_eq!(boot.board.pci_config_address(), 0x8000_1000);
        // Run 2 on the same board seeds the cell from the board: IN EAX returns the address.
        let second = run_set_facade(&mut boot, &file, 3, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 3, "XOR, IN, HLT");
        assert_eq!(second.gpr[Register::Rax as usize], 0x8000_1000);
        assert_eq!(second.pci_config_address, 0x8000_1000);
    }

    #[test]
    fn threaded_linux_imports_the_config_address_set_by_the_ordinary_run() {
        let mut code = mov_eax(0x8000_1000);
        code.extend(mov_edx(0xcf8));
        code.extend([0xEF, 0x31, 0xC0, 0xED, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let members = halting_default();
        let first = run_set_facade(&mut boot, &file, 3, members);
        assert_eq!(first.pci_config_address, 0x8000_1000);
        let second = run_threaded_test(&mut boot, &file, 4, 2, members)
            .unwrap()
            .stop;
        assert_eq!(second.exit, PagedProbeExit::Halted);
        assert_eq!(second.retired, 3);
        assert_eq!(second.gpr[Register::Rax as usize], 0x8000_1000);
        assert_eq!(second.pci_config_address, 0x8000_1000);
    }

    // ---- S4-38L7e: RDMSR IA32_EFER through the board facade ----

    #[test]
    fn rdmsr_efer_runs_through_the_board_set_and_is_gated_by_its_member() {
        // MOV ECX,0xC0000080; RDMSR; HLT (no CLI: the first stop is the RDMSR itself).
        let code = [0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32, 0xF4];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 3, "MOV, RDMSR, HLT");
        assert_eq!(record.gpr[Register::Rax as usize], 0xD00);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0080);
        assert!(
            record
                .set_members
                .contains(&format!("msr:{:?}", MsrProfile::EferReadOnlyV1(0xD00)))
        );
        // The EFER of the board model is unchanged by the read.
        assert_eq!(boot.board.efer(), 0xD00);
        assert!(LinuxCpl0SetMembers::linux_default().msr);
        assert!(!LinuxCpl0SetMembers::minimal().msr);

        // RIP advances by 2 over the RDMSR and RFLAGS is not touched: stop right after it.
        let rdmsr_only = [0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32];
        let (mut boot, file) = boot_with_entry(&rdmsr_only);
        let rip0 = boot.board.state().rip;
        let flags0 = boot.board.state().rflags;
        let record = run_set_facade(&mut boot, &file, 2, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 2, "{record:?}");
        assert_eq!(record.gpr[Register::Rax as usize], 0xD00);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.rip, rip0 + 7);
        assert_eq!(record.rflags, flags0);

        // WRMSR of EFER is Unsupported without mutation: MOV ECX; XOR EAX; XOR EDX; WRMSR.
        let wr = [
            0xB9, 0x80, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let (mut boot, file) = boot_with_entry(&wr);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(boot.board.efer(), 0xD00);

        // Another MSR (STAR) is Unsupported at RDMSR.
        let star = [0xB9, 0x81, 0x00, 0x00, 0xC0, 0x0F, 0x32];
        let (mut boot, file) = boot_with_entry(&star);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x32 },
            "{record:?}"
        );
        assert_eq!(record.retired, 1);

        // `minimal()` has no member: RDMSR EFER is the stop.
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x32 },
            "{record:?}"
        );
        assert_eq!(record.retired, 1);
    }

    // ---- S4-38L7f: WRMSR IA32_EFER (SCE) and CR0.AM through the board facade ----

    #[test]
    fn wrmsr_efer_sce_runs_through_the_board_set_and_rdmsr_reads_it_back() {
        // MOV ECX,0xC0000080; MOV EAX,0xD01; XOR EDX,EDX; WRMSR; XOR EAX,EAX; RDMSR; HLT.
        let code = [
            0xB9, 0x80, 0x00, 0x00, 0xC0, 0xB8, 0x01, 0x0D, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
            0x31, 0xC0, 0x0F, 0x32, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_efer(), 0xD00);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7, "MOV, MOV, XOR, WRMSR, XOR, RDMSR, HLT");
        assert_eq!(record.gpr[Register::Rax as usize], 0xD01);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0080);
        // The guest-visible EFER carries SCE; the walker EFER (LME|LMA|NXE) is unchanged.
        assert_eq!(record.guest_efer, 0xD01);
        assert_eq!(boot.board.guest_efer(), 0xD01);
        assert_eq!(record.efer, 0xD00);
        assert_eq!(boot.board.efer(), 0xD00);

        // The value survives into the next run of the same board: RDMSR starts from 0xD01.
        let rdmsr = [0xB9, 0x80, 0x00, 0x00, 0xC0, 0x0F, 0x32];
        let (mut boot, file) = boot_with_entry(&rdmsr);
        boot.board.set_guest_efer(0xD01);
        let record = run_set_facade(&mut boot, &file, 2, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 2, "{record:?}");
        assert_eq!(record.gpr[Register::Rax as usize], 0xD01);

        // Without the msr member WRMSR is the stop (after MOV, MOV, XOR) and EFER is unchanged.
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(boot.board.guest_efer(), 0xD00);
    }

    // ---- S4-38L7ao: WRMSR IA32_STAR through the board facade (write-only) ----

    #[test]
    fn wrmsr_star_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // MOV ECX,0xC0000081; XOR EAX,EAX; MOV EDX,0x230010; WRMSR; HLT: the boundary bytes
        // `0F 30` of the diagnostic run with EDX:EAX = 0x230010:0.
        let code = [
            0xB9, 0x81, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0xBA, 0x10, 0x00, 0x23, 0x00, 0x0F, 0x30,
            0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_star(), 0);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, XOR, MOV, WRMSR, HLT");
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0081);
        assert_eq!(record.gpr[Register::Rax as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0x23_0010);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board.
        assert_eq!(record.guest_star, 0x0023_0010_0000_0000);
        assert_eq!(boot.board.guest_star(), 0x0023_0010_0000_0000);
        // Nothing else of the MSR state moved.
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(record.set_members.contains(&"msr:star_write".to_string()));

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_star(0x0023_0010_0000_0000);
        let record = run_set_facade(
            &mut boot,
            &file,
            4,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_star, 0x0023_0010_0000_0000);

        // A real chain on one board: run 1 (guest WRMSR) stops by budget right after the write,
        // run 2 on the same `boot` retires the rest and still reports the stored value.
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_star, 0x0023_0010_0000_0000);
        assert_eq!(boot.board.guest_star(), 0x0023_0010_0000_0000);
        let second = run_set_facade(&mut boot, &file, 1, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 1, "HLT");
        assert_eq!(second.guest_star, 0x0023_0010_0000_0000);
        assert_eq!(boot.board.guest_star(), 0x0023_0010_0000_0000);

        // Without the member the WRMSR is the stop (after MOV, XOR, MOV) and STAR stays 0.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_star: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_star, 0);
            assert_eq!(boot.board.guest_star(), 0);
            assert!(!record.set_members.contains(&"msr:star_write".to_string()));
        }
    }

    #[test]
    fn rdmsr_star_and_other_msr_writes_stay_the_stop_through_the_board_set() {
        // MOV ECX,0xC0000081; XOR EAX,EAX; MOV EDX,0x230010; WRMSR; RDMSR: the write is retired,
        // the read of STAR is the stop (the value is not readable by the guest).
        let code = [
            0xB9, 0x81, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0xBA, 0x10, 0x00, 0x23, 0x00, 0x0F, 0x30,
            0x0F, 0x32,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x32 },
            "{record:?}"
        );
        assert_eq!(record.retired, 4);
        assert_eq!(record.guest_star, 0x0023_0010_0000_0000);

        // WRMSR of SYSCALL_MASK (0xC0000084) with the same values is the stop when the S4-38L7at
        // member is off, STAR untouched (LSTAR is a member since S4-38L7ap and CSTAR since
        // S4-38L7aq; SYSCALL_MASK since S4-38L7at).
        let mask = [
            0xB9, 0x84, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0xBA, 0x10, 0x00, 0x23, 0x00, 0x0F, 0x30,
        ];
        let (mut boot, file) = boot_with_entry(&mask);
        let members = LinuxCpl0SetMembers {
            msr_syscall_mask: false,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(record.guest_star, 0);
        assert_eq!(record.guest_lstar, 0);
        assert_eq!(record.guest_cstar, 0);
        assert_eq!(record.guest_syscall_mask, 0);
    }

    // ---- S4-38L7ap: WRMSR IA32_LSTAR through the board facade (write-only) ----

    #[test]
    fn wrmsr_lstar_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // MOV ECX,0xC0000082; MOV EAX,0x81000080; MOV EDX,0xFFFFFFFF; WRMSR; HLT: the boundary
        // bytes `0F 30` of the diagnostic run with EDX:EAX = 0xFFFFFFFF:0x81000080.
        let code = [
            0xB9, 0x82, 0x00, 0x00, 0xC0, 0xB8, 0x80, 0x00, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30, 0xF4,
        ];
        let value = 0xFFFF_FFFF_8100_0080u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_lstar(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, MOV, MOV, WRMSR, HLT");
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0082);
        assert_eq!(record.gpr[Register::Rax as usize], 0x8100_0080);
        assert_eq!(record.gpr[Register::Rdx as usize], 0xFFFF_FFFF);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board; STAR
        // and the EFER did not move.
        assert_eq!(record.guest_lstar, value);
        assert_eq!(boot.board.guest_lstar(), value);
        assert_eq!(record.guest_star, 0);
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(record.set_members.contains(&"msr:lstar_write".to_string()));

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_lstar(value);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_lstar, value);

        // A real chain on one board: run 1 (guest WRMSR) stops by budget right after the write,
        // run 2 on the same `boot` retires the rest and still reports the stored value.
        let (mut boot, file) = boot_with_entry(&code);
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_lstar, value);
        assert_eq!(boot.board.guest_lstar(), value);
        let second = run_set_facade(&mut boot, &file, 1, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 1, "HLT");
        assert_eq!(second.guest_lstar, value);
        assert_eq!(boot.board.guest_lstar(), value);

        // Without the member the WRMSR is the stop (after MOV, MOV, MOV) and LSTAR stays 0; the
        // STAR member does not admit LSTAR.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_lstar: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_lstar, 0);
            assert_eq!(boot.board.guest_lstar(), 0);
            assert!(!record.set_members.contains(&"msr:lstar_write".to_string()));
        }
    }

    #[test]
    fn rdmsr_lstar_and_syscall_stay_the_stop_through_the_board_set() {
        // MOV ECX,0xC0000082; MOV EAX,0x81000080; MOV EDX,0xFFFFFFFF; WRMSR; RDMSR: the write is
        // retired, the read of LSTAR is the stop (the value is not readable by the guest).
        let prefix = [
            0xB9, 0x82, 0x00, 0x00, 0xC0, 0xB8, 0x80, 0x00, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x05], 0x05),
            (&[0x0F, 0x07], 0x07),
        ] {
            let mut code = prefix.to_vec();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!(record.retired, 4);
            assert_eq!(record.guest_lstar, 0xFFFF_FFFF_8100_0080);
        }
    }

    // ---- S4-38L7aq: WRMSR IA32_CSTAR through the board facade (write-only) ----

    #[test]
    fn wrmsr_cstar_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // MOV ECX,0xC0000083; MOV EAX,0x81001AB0; MOV EDX,0xFFFFFFFF; WRMSR; HLT: the boundary
        // bytes `0F 30` of the diagnostic run with EDX:EAX = 0xFFFFFFFF:0x81001AB0.
        let code = [
            0xB9, 0x83, 0x00, 0x00, 0xC0, 0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30, 0xF4,
        ];
        let value = 0xFFFF_FFFF_8100_1AB0u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_cstar(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, MOV, MOV, WRMSR, HLT");
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0083);
        assert_eq!(record.gpr[Register::Rax as usize], 0x8100_1AB0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0xFFFF_FFFF);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board; STAR,
        // LSTAR and the EFER did not move.
        assert_eq!(record.guest_cstar, value);
        assert_eq!(boot.board.guest_cstar(), value);
        assert_eq!(record.guest_star, 0);
        assert_eq!(record.guest_lstar, 0);
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(record.set_members.contains(&"msr:cstar_write".to_string()));

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_cstar(value);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_cstar, value);

        // A real chain on one board: run 1 (guest WRMSR) stops by budget right after the write,
        // run 2 on the same `boot` retires the rest and still reports the stored value.
        let (mut boot, file) = boot_with_entry(&code);
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_cstar, value);
        assert_eq!(boot.board.guest_cstar(), value);
        let second = run_set_facade(&mut boot, &file, 1, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 1, "HLT");
        assert_eq!(second.guest_cstar, value);
        assert_eq!(boot.board.guest_cstar(), value);

        // Without the member the WRMSR is the stop (after MOV, MOV, MOV) and CSTAR stays 0; the
        // STAR and LSTAR members do not admit CSTAR.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_star: true,
                msr_lstar: true,
                ..LinuxCpl0SetMembers::minimal()
            },
            LinuxCpl0SetMembers {
                msr_cstar: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_cstar, 0);
            assert_eq!(boot.board.guest_cstar(), 0);
            assert!(!record.set_members.contains(&"msr:cstar_write".to_string()));
        }
    }

    #[test]
    fn rdmsr_cstar_and_syscall_mask_stay_the_stop_through_the_board_set() {
        // MOV ECX,0xC0000083; MOV EAX,0x81001AB0; MOV EDX,0xFFFFFFFF; WRMSR; then RDMSR, SYSCALL
        // or SYSRET: the write is retired, the next instruction is the stop (the value is not
        // readable by the guest and SYSCALL/SYSRET are not modelled).
        let prefix = [
            0xB9, 0x83, 0x00, 0x00, 0xC0, 0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x05], 0x05),
            (&[0x0F, 0x07], 0x07),
        ] {
            let mut code = prefix.to_vec();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!(record.retired, 4);
            assert_eq!(record.guest_cstar, 0xFFFF_FFFF_8100_1AB0);
        }

        // WRMSR of SYSCALL_MASK (0xC0000084) is the stop when the S4-38L7at member is off; nothing
        // moved.
        let mask = [
            0xB9, 0x84, 0x00, 0x00, 0xC0, 0xB8, 0x00, 0x77, 0x04, 0x00, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let (mut boot, file) = boot_with_entry(&mask);
        let members = LinuxCpl0SetMembers {
            msr_syscall_mask: false,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
        assert_eq!(record.guest_syscall_mask, 0);
    }

    // ---- S4-38L7ar: WRMSR IA32_SYSENTER_CS through the board facade (write-only) ----

    #[test]
    fn wrmsr_sysenter_cs_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // MOV ECX,0x174; MOV EAX,0x10; XOR EDX,EDX; WRMSR; HLT: the boundary bytes `0F 30` of the
        // diagnostic run (EDX:EAX = 0:0x10, the kernel CS selector).
        let code = [
            0xB9, 0x74, 0x01, 0x00, 0x00, 0xB8, 0x10, 0x00, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
            0xF4,
        ];
        let value = 0x10u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_sysenter_cs(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, MOV, XOR, WRMSR, HLT");
        assert_eq!(record.gpr[Register::Rcx as usize], 0x174);
        assert_eq!(record.gpr[Register::Rax as usize], 0x10);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board; the
        // SYSCALL cells and the EFER did not move.
        assert_eq!(record.guest_sysenter_cs, value);
        assert_eq!(boot.board.guest_sysenter_cs(), value);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(
            record
                .set_members
                .contains(&"msr:sysenter_cs_write".to_string())
        );

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_sysenter_cs(value);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_sysenter_cs, value);

        // A real chain on one board: run 1 (guest WRMSR) stops by budget right after the write,
        // run 2 on the same `boot` retires the rest and still reports the stored value.
        let (mut boot, file) = boot_with_entry(&code);
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_sysenter_cs, value);
        assert_eq!(boot.board.guest_sysenter_cs(), value);
        let second = run_set_facade(&mut boot, &file, 1, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 1, "HLT");
        assert_eq!(second.guest_sysenter_cs, value);
        assert_eq!(boot.board.guest_sysenter_cs(), value);

        // Without the member the WRMSR is the stop (after MOV, MOV, XOR) and the cell stays 0; the
        // SYSCALL members do not admit SYSENTER_CS.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_star: true,
                msr_lstar: true,
                msr_cstar: true,
                ..LinuxCpl0SetMembers::minimal()
            },
            LinuxCpl0SetMembers {
                msr_sysenter_cs: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_sysenter_cs, 0);
            assert_eq!(boot.board.guest_sysenter_cs(), 0);
            assert!(
                !record
                    .set_members
                    .contains(&"msr:sysenter_cs_write".to_string())
            );
        }
    }

    #[test]
    fn wrmsr_sysenter_cs_keeps_the_full_64_bit_value_and_never_touches_the_other_cells() {
        // MOV ECX,0x174; MOV EAX,0x8100_1AB0; MOV EDX,0xFFFF_FFFF; WRMSR: not a 16-bit selector;
        // the cell holds all 64 bits written and no canonical check is made.
        let code = [
            0xB9, 0x74, 0x01, 0x00, 0x00, 0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF,
            0xFF, 0x0F, 0x30,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.guest_sysenter_cs, 0xFFFF_FFFF_8100_1AB0);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
    }

    #[test]
    fn rdmsr_sysenter_cs_esp_eip_and_sysenter_stay_the_stop_through_the_board_set() {
        // MOV ECX,0x174; MOV EAX,0x10; XOR EDX,EDX; WRMSR; then RDMSR, SYSENTER or SYSEXIT: the
        // write is retired, the next instruction is the stop (not readable, SYSENTER/SYSEXIT are
        // not modelled).
        let prefix = [
            0xB9, 0x74, 0x01, 0x00, 0x00, 0xB8, 0x10, 0x00, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
        ];
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x34], 0x34),
            (&[0x0F, 0x35], 0x35),
        ] {
            let mut code = prefix.to_vec();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!(record.retired, 4);
            assert_eq!(record.guest_sysenter_cs, 0x10);
        }

        // WRMSR of SYSENTER_ESP (0x175) and SYSENTER_EIP (0x176) is the stop when the S4-38L7as
        // member is off; nothing moved.
        for index in [0x75u8, 0x76] {
            let code = [
                0xB9, index, 0x01, 0x00, 0x00, 0xB8, 0x00, 0x77, 0x04, 0x00, 0x31, 0xD2, 0x0F, 0x30,
            ];
            let (mut boot, file) = boot_with_entry(&code);
            let members = LinuxCpl0SetMembers {
                msr_sysenter_stack: false,
                ..LinuxCpl0SetMembers::linux_default()
            };
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_sysenter_cs, 0);
            assert_eq!(record.guest_sysenter_esp, 0);
            assert_eq!(record.guest_sysenter_eip, 0);
        }
    }

    // ---- S4-38L7as: WRMSR IA32_SYSENTER_ESP and IA32_SYSENTER_EIP through the board facade ----

    /// MOV ECX,0x175; MOV EAX,0x3000; MOV EDX,0xFFFFFE00; WRMSR (ESP); MOV ECX,0x176;
    /// MOV EAX,0x81000010; WRMSR (EIP, EDX unchanged); HLT. The boundary bytes `0F 30` of the
    /// diagnostic run (EDX:EAX = 0xFFFFFE00:0x3000 for the ESP write).
    const SYSENTER_STACK_CODE: [u8; 25] = [
        0xB9, 0x75, 0x01, 0x00, 0x00, 0xB8, 0x00, 0x30, 0x00, 0x00, 0xBA, 0x00, 0xFE, 0xFF, 0xFF,
        0x0F, 0x30, 0xB9, 0x76, 0x01, 0x00, 0x00, 0xB8, 0x10, 0x00,
    ];

    fn sysenter_stack_program() -> Vec<u8> {
        let mut code = SYSENTER_STACK_CODE.to_vec();
        code.extend_from_slice(&[0x00, 0x81, 0x0F, 0x30, 0xF4]);
        code
    }

    #[test]
    fn wrmsr_sysenter_esp_and_eip_run_through_the_board_set_and_the_board_keeps_the_values() {
        let code = sysenter_stack_program();
        let esp = 0xFFFF_FE00_0000_3000u64;
        let eip = 0xFFFF_FE00_8100_0010u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_sysenter_esp(), 0);
        assert_eq!(boot.board.guest_sysenter_eip(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 8,
            "MOV, MOV, MOV, WRMSR, MOV, MOV, WRMSR, HLT"
        );
        assert_eq!(record.gpr[Register::Rcx as usize], 0x176);
        // The values are exactly EDX << 32 | EAX, observable through the record and the board; the
        // other cells and the EFER did not move.
        assert_eq!(record.guest_sysenter_esp, esp);
        assert_eq!(record.guest_sysenter_eip, eip);
        assert_eq!(boot.board.guest_sysenter_esp(), esp);
        assert_eq!(boot.board.guest_sysenter_eip(), eip);
        assert_eq!(record.guest_sysenter_cs, 0);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(
            record
                .set_members
                .contains(&"msr:sysenter_stack_write".to_string())
        );

        // The values survive into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_sysenter_esp(esp);
        boot.board.set_guest_sysenter_eip(eip);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_sysenter_esp, esp);
        assert_eq!(record.guest_sysenter_eip, eip);

        // A real chain on one board: run 1 stops by budget right after the ESP write (EIP still
        // 0), run 2 on the same `boot` retires the rest (EIP write, HLT) and reports both.
        let (mut boot, file) = boot_with_entry(&code);
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_sysenter_esp, esp);
        assert_eq!(first.guest_sysenter_eip, 0);
        assert_eq!(boot.board.guest_sysenter_esp(), esp);
        assert_eq!(boot.board.guest_sysenter_eip(), 0);
        let second = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 4, "MOV, MOV, WRMSR, HLT");
        assert_eq!(second.guest_sysenter_esp, esp);
        assert_eq!(second.guest_sysenter_eip, eip);
        assert_eq!(boot.board.guest_sysenter_esp(), esp);
        assert_eq!(boot.board.guest_sysenter_eip(), eip);

        // Without the member the first WRMSR is the stop (after MOV, MOV, MOV) and both cells
        // stay 0; the SYSCALL members and the SYSENTER_CS member do not admit ESP/EIP.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_star: true,
                msr_lstar: true,
                msr_cstar: true,
                msr_sysenter_cs: true,
                ..LinuxCpl0SetMembers::minimal()
            },
            LinuxCpl0SetMembers {
                msr_sysenter_stack: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_sysenter_esp, 0);
            assert_eq!(record.guest_sysenter_eip, 0);
            assert_eq!(boot.board.guest_sysenter_esp(), 0);
            assert_eq!(boot.board.guest_sysenter_eip(), 0);
            assert!(
                !record
                    .set_members
                    .contains(&"msr:sysenter_stack_write".to_string())
            );
        }
    }

    #[test]
    fn wrmsr_sysenter_esp_and_eip_keep_the_full_64_bit_value_and_never_touch_the_other_cells() {
        // MOV ECX,index; MOV EAX,0x8100_1AB0; MOV EDX,0xFFFF_FFFF; WRMSR: no canonical check; only
        // the cell of the written index changes.
        for (index, own_is_esp) in [(0x75u8, true), (0x76, false)] {
            let code = [
                0xB9, index, 0x01, 0x00, 0x00, 0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF,
                0xFF, 0xFF, 0x0F, 0x30,
            ];
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::linux_default());
            assert_eq!(record.retired, 4, "{record:?}");
            let (own, other) = if own_is_esp {
                (record.guest_sysenter_esp, record.guest_sysenter_eip)
            } else {
                (record.guest_sysenter_eip, record.guest_sysenter_esp)
            };
            assert_eq!(own, 0xFFFF_FFFF_8100_1AB0);
            assert_eq!(other, 0);
            assert_eq!(record.guest_sysenter_cs, 0);
            assert_eq!(
                (record.guest_star, record.guest_lstar, record.guest_cstar),
                (0, 0, 0)
            );
        }
    }

    #[test]
    fn rdmsr_sysenter_esp_eip_and_sysenter_stay_the_stop_after_the_stack_writes_through_the_board_set()
     {
        // The two writes of the program are retired, the next instruction is the stop (RDMSR,
        // SYSENTER, SYSEXIT: not readable, not modelled); both values are in the record.
        let mut prefix = sysenter_stack_program();
        prefix.pop(); // the HLT
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x34], 0x34),
            (&[0x0F, 0x35], 0x35),
        ] {
            let mut code = prefix.clone();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!(record.retired, 7);
            assert_eq!(record.guest_sysenter_esp, 0xFFFF_FE00_0000_3000);
            assert_eq!(record.guest_sysenter_eip, 0xFFFF_FE00_8100_0010);
        }
    }

    #[test]
    fn guest_efer_chains_between_two_runs_on_one_board_and_the_member_name_is_stable() {
        // MOV ECX,0xC0000080; MOV EAX,0xD01; XOR EDX,EDX; WRMSR | XOR EAX,EAX; RDMSR; HLT.
        let code = [
            0xB9, 0x80, 0x00, 0x00, 0xC0, 0xB8, 0x01, 0x0D, 0x00, 0x00, 0x31, 0xD2, 0x0F, 0x30,
            0x31, 0xC0, 0x0F, 0x32, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let name = format!("msr:{:?}", MsrProfile::EferReadOnlyV1(0xD00));
        assert_eq!(name, "msr:EferReadOnlyV1(3328)");
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        // Run 1 stops by budget right after the WRMSR.
        let first = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(first.retired, 4, "{first:?}");
        assert_eq!(first.guest_efer, 0xD01);
        assert_eq!(boot.board.guest_efer(), 0xD01);
        assert!(first.set_members.contains(&name), "{:?}", first.set_members);
        // Run 2 on the same board seeds the cell from the board: RDMSR returns 0xD01.
        let second = run_set_facade(&mut boot, &file, 3, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!(second.retired, 3, "XOR, RDMSR, HLT");
        assert_eq!(second.gpr[Register::Rax as usize], 0xD01);
        assert_eq!(second.guest_efer, 0xD01);
        assert_eq!(second.efer, 0xD00);
        assert!(
            second.set_members.contains(&name),
            "{:?}",
            second.set_members
        );
    }

    #[test]
    fn wrmsr_efer_outside_the_sce_change_stays_the_stop_without_mutation() {
        // MOV ECX,0xC0000080; XOR EAX,EAX (EFER = 0: LME/LMA cleared); XOR EDX,EDX; WRMSR.
        let wr = [
            0xB9, 0x80, 0x00, 0x00, 0xC0, 0x31, 0xC0, 0x31, 0xD2, 0x0F, 0x30,
        ];
        let (mut boot, file) = boot_with_entry(&wr);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(boot.board.guest_efer(), 0xD00);
        assert_eq!(record.guest_efer, 0xD00);
        // STAR is admitted since S4-38L7ao and the SYSCALL family (LSTAR, ...) is next, so the
        // "other MSR" is IA32_APIC_BASE: MOV ECX,0x1B; WRMSR is the stop as well.
        let apic_base = [0xB9, 0x1B, 0x00, 0x00, 0x00, 0x0F, 0x30];
        let (mut boot, file) = boot_with_entry(&apic_base);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.retired, 1);
        assert_eq!(boot.board.guest_efer(), 0xD00);
    }

    #[test]
    fn mov_cr0_with_am_runs_through_the_board_set() {
        // MOV EAX,0x80050033; MOV CR0,RAX; XOR EAX,EAX; MOV RAX,CR0; HLT. The fetch after the
        // CR0 write goes through the walker snapshot, which must carry AM.
        let code = [
            0xB8, 0x33, 0x00, 0x05, 0x80, 0x0F, 0x22, 0xC0, 0x31, 0xC0, 0x0F, 0x20, 0xC0, 0xF4,
        ];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(
            &mut boot,
            &file,
            64,
            LinuxCpl0SetMembers {
                halt: true,
                ..LinuxCpl0SetMembers::linux_default()
            },
        );
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5);
        assert_eq!(record.cr0, Some(0x8005_0033));
        assert_eq!(record.gpr[Register::Rax as usize], 0x8005_0033);

        // A CR0 value outside the policy (CD, bit 30) stays the stop, without mutation.
        let bad = [0xB8, 0x33, 0x00, 0x01, 0xC0, 0x0F, 0x22, 0xC0];
        let (mut boot, file) = boot_with_entry(&bad);
        let cr0_before = boot.board.state().cr0;
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x22 },
            "{record:?}"
        );
        assert_eq!(record.retired, 1);
        assert_eq!(record.cr0, cr0_before);
    }

    // ---- S4-38L6c: MOV Sreg, AX with a nonzero selector through the board facade ----

    #[test]
    fn mov_ss_and_es_with_the_boot_data_selector_run_through_the_board_set() {
        // CLI; MOV EAX,0x18; MOV SS,AX; MOV ES,AX; the stop is the filler byte after the code.
        let code = [0xFA, 0xB8, 0x18, 0x00, 0x00, 0x00, 0x8E, 0xD0, 0x8E, 0xC0];
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x7A },
            "{record:?}"
        );
        assert_eq!(record.retired, 4, "CLI, MOV, MOV SS, MOV ES");
        assert_eq!(record.rip, ENTRY + 10);
        let state = boot.board.state();
        let data = od_isa::DataSegmentDescriptorCache {
            base: 0,
            limit: 0xffff_ffff,
            access: 0x93,
            flags: 0xc,
        };
        assert_eq!(state.ss_selector, Some(0x18));
        assert_eq!(state.ss_cache, Some(data));
        assert_eq!(state.es_selector, 0x18);
        assert_eq!(state.es_cache, Some(data));
        assert!(!state.ss_unusable && !state.es_unusable);
        assert_eq!(record.gpr[Register::Rax as usize], 0x18);
        assert_eq!(record.rflags & (1 << 9), 0, "IF stays clear");
        // The shadow MOV SS opened was consumed by the MOV ES that followed it.
        assert_eq!(state.sti_shadow, Some(od_isa::StiShadow::Clear));
    }

    // ---- S4-38L6d: LEAVE through the board facade ----

    #[test]
    fn leave_runs_through_the_board_set() {
        // LEAVE then nine NOPs (the filler after ten bytes is the 0x7A stop); RBP points at a
        // frame in board RAM.
        let (mut boot, file) =
            boot_with_entry(&[0xC9, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90]);
        let saved_rbp = 0x0000_7FFF_1234_5670u64;
        write_phys(&mut boot, DATA, &saved_rbp.to_le_bytes());
        let state = boot.board.state_mut();
        state.gpr[Register::Rbp as usize] = DATA;
        state.gpr[Register::Rsp as usize] = DATA + 0x100;
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x7A },
            "{record:?}"
        );
        assert_eq!(record.retired, 10, "LEAVE and nine NOPs: {record:?}");
        assert_eq!(record.rip, ENTRY + 10);
        assert_eq!(record.gpr[Register::Rbp as usize], saved_rbp);
        assert_eq!(record.gpr[Register::Rsp as usize], DATA + 8);
        assert_eq!(read_qword(&mut boot, DATA), saved_rbp, "no write");
    }

    // ---- S4-38L4: the CPUID boundary on the set path ----

    /// `MOV EAX, imm32` (`B8 id`).
    fn mov_eax(value: u32) -> Vec<u8> {
        let mut code = vec![0xB8];
        code.extend_from_slice(&value.to_le_bytes());
        code
    }

    fn halting_default() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        }
    }

    // ---- S4-38L7at: WRMSR IA32_SYSCALL_MASK through the board facade (write-only) ----

    /// MOV ECX,0xC0000084; MOV EAX,0x257FD5; XOR EDX,EDX; WRMSR: the boundary bytes `0F 30` of
    /// the diagnostic run (EDX:EAX = 0:0x257FD5).
    const SYSCALL_MASK_CODE: [u8; 14] = [
        0xB9, 0x84, 0x00, 0x00, 0xC0, 0xB8, 0xD5, 0x7F, 0x25, 0x00, 0x31, 0xD2, 0x0F, 0x30,
    ];

    #[test]
    fn wrmsr_syscall_mask_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // Two writes in one program: the first is the boundary one; the second (MOV EAX,0x81001AB0;
        // MOV EDX,0xFFFFFFFF; WRMSR) replaces it with a value that has all 64 bits set.
        let mut code = SYSCALL_MASK_CODE.to_vec();
        code.extend_from_slice(&[
            0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x30, 0xF4,
        ]);
        let first = 0x25_7FD5u64;
        let second = 0xFFFF_FFFF_8100_1AB0u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_syscall_mask(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 8,
            "MOV, MOV, XOR, WRMSR, MOV, MOV, WRMSR, HLT"
        );
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0084);
        assert_eq!(record.gpr[Register::Rax as usize], 0x8100_1AB0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0xFFFF_FFFF);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board; the
        // other cells and the EFER did not move.
        assert_eq!(record.guest_syscall_mask, second);
        assert_eq!(boot.board.guest_syscall_mask(), second);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
        assert_eq!(
            (
                record.guest_sysenter_cs,
                record.guest_sysenter_esp,
                record.guest_sysenter_eip
            ),
            (0, 0, 0)
        );
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(
            record
                .set_members
                .contains(&"msr:syscall_mask_write".to_string())
        );

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_syscall_mask(first);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_syscall_mask, first);

        // A real chain on one board: run 1 stops by budget right after the first write, run 2 on
        // the same `boot` retires the rest (MOV, MOV, WRMSR, HLT) and reports the replaced value.
        let (mut boot, file) = boot_with_entry(&code);
        let run1 = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(run1.retired, 4, "{run1:?}");
        assert_eq!(run1.guest_syscall_mask, first);
        assert_eq!(boot.board.guest_syscall_mask(), first);
        let run2 = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(run2.exit, PagedProbeExit::Halted, "{run2:?}");
        assert_eq!(run2.retired, 4, "MOV, MOV, WRMSR, HLT");
        assert_eq!(run2.guest_syscall_mask, second);
        assert_eq!(boot.board.guest_syscall_mask(), second);

        // Without the member the WRMSR is the stop (after MOV, MOV, XOR) and the cell stays 0; the
        // other MSR members do not admit SYSCALL_MASK.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_star: true,
                msr_lstar: true,
                msr_cstar: true,
                msr_sysenter_cs: true,
                msr_sysenter_stack: true,
                ..LinuxCpl0SetMembers::minimal()
            },
            LinuxCpl0SetMembers {
                msr_syscall_mask: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_syscall_mask, 0);
            assert_eq!(boot.board.guest_syscall_mask(), 0);
            assert!(
                !record
                    .set_members
                    .contains(&"msr:syscall_mask_write".to_string())
            );
        }
    }

    #[test]
    fn swapgs_exchanges_gs_base_with_the_board_kernel_gs_base_through_the_board_set() {
        // S4-38L9A: WRMSR KERNEL_GS_BASE (EDX:EAX = 0xFFFF8880:12345678), then SWAPGS, HLT. The
        // cell becomes the board's old gs_base and gs_base becomes the written value; a second
        // SWAPGS returns both.
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let mut code = KERNEL_GS_BASE_CODE.to_vec();
        code.extend_from_slice(&[0x0F, 0x01, 0xF8, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 16, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "MOV, MOV, MOV, WRMSR, SWAPGS, HLT");
        assert_ne!(record.guest_kernel_gs_base, 0xFFFF_8880_1234_5678);

        let mut code = KERNEL_GS_BASE_CODE.to_vec();
        code.extend_from_slice(&[0x0F, 0x01, 0xF8, 0x0F, 0x01, 0xF8, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 16, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 7);
        assert_eq!(record.guest_kernel_gs_base, 0xFFFF_8880_1234_5678);
        assert_eq!(boot.board.guest_kernel_gs_base(), 0xFFFF_8880_1234_5678);
    }

    #[test]
    fn swapgs_without_the_kernel_gs_base_member_is_the_stop_through_the_board_set() {
        let members = LinuxCpl0SetMembers {
            msr_kernel_gs_base: false,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&[0x0F, 0x01, 0xF8]);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x01 },
            "{record:?}"
        );
        assert_eq!(record.retired, 0);
    }

    #[test]
    fn rdmsr_syscall_and_sysenter_stay_the_stop_after_the_syscall_mask_write_through_the_board_set()
    {
        // The write is retired, the next instruction is the stop (RDMSR: not readable; SYSCALL,
        // SYSRET, SYSENTER, SYSEXIT: not modelled); the value is in the record.
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x05], 0x05),
            (&[0x0F, 0x07], 0x07),
            (&[0x0F, 0x34], 0x34),
            (&[0x0F, 0x35], 0x35),
        ] {
            let mut code = SYSCALL_MASK_CODE.to_vec();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{record:?}"
            );
            assert_eq!(record.retired, 4);
            assert_eq!(record.guest_syscall_mask, 0x25_7FD5);
        }

        // The neighbours 0xC0000083 (CSTAR) and 0xC0000085 write their own cell only; 0xC0000085
        // is no member and is the stop with SYSCALL_MASK untouched.
        let mut code = SYSCALL_MASK_CODE.to_vec();
        code[1] = 0x85;
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 3);
        assert_eq!(record.guest_syscall_mask, 0);
        let mut code = SYSCALL_MASK_CODE.to_vec();
        code[1] = 0x83;
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.guest_cstar, 0x25_7FD5);
        assert_eq!(record.guest_syscall_mask, 0);
    }

    // ---- S4-38L7au: WRMSR IA32_KERNEL_GS_BASE through the board facade (write-only) ----

    /// MOV ECX,0xC0000102; MOV EAX,0x12345678; MOV EDX,0xFFFF8880; WRMSR: a write of a
    /// kernel-pointer-like value (EDX:EAX = 0xFFFF8880:0x12345678). The boundary bytes of the
    /// diagnostic run are the same `0F 30` with EDX:EAX = 0:0 (see the zero-write test).
    const KERNEL_GS_BASE_CODE: [u8; 17] = [
        0xB9, 0x02, 0x01, 0x00, 0xC0, 0xB8, 0x78, 0x56, 0x34, 0x12, 0xBA, 0x80, 0x88, 0xFF, 0xFF,
        0x0F, 0x30,
    ];

    #[test]
    fn wrmsr_kernel_gs_base_runs_through_the_board_set_and_the_board_keeps_the_value() {
        // Two writes in one program: the first is KERNEL_GS_BASE_CODE; the second (MOV
        // EAX,0x81001AB0; MOV EDX,0xFFFFFFFF; WRMSR) replaces it with a value that has all 64 bits
        // set in the upper half.
        let mut code = KERNEL_GS_BASE_CODE.to_vec();
        code.extend_from_slice(&[
            0xB8, 0xB0, 0x1A, 0x00, 0x81, 0xBA, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F, 0x30, 0xF4,
        ]);
        let first = 0xFFFF_8880_1234_5678u64;
        let second = 0xFFFF_FFFF_8100_1AB0u64;
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        assert_eq!(boot.board.guest_kernel_gs_base(), 0);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 8,
            "MOV, MOV, MOV, WRMSR, MOV, MOV, WRMSR, HLT"
        );
        assert_eq!(record.gpr[Register::Rcx as usize], 0xC000_0102);
        assert_eq!(record.gpr[Register::Rax as usize], 0x8100_1AB0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0xFFFF_FFFF);
        // The value is exactly EDX << 32 | EAX, observable through the record and the board; the
        // other cells and the EFER did not move.
        assert_eq!(record.guest_kernel_gs_base, second);
        assert_eq!(boot.board.guest_kernel_gs_base(), second);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
        assert_eq!(
            (
                record.guest_sysenter_cs,
                record.guest_sysenter_esp,
                record.guest_sysenter_eip,
                record.guest_syscall_mask
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(record.efer, 0xD00);
        assert!(
            record
                .set_members
                .contains(&"msr:kernel_gs_base_write".to_string())
        );

        // The value survives into the next run of the same board (no write in it).
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        boot.board.set_guest_kernel_gs_base(first);
        let record = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.guest_kernel_gs_base, first);

        // A real chain on one board: run 1 stops by budget right after the first write (MOV, MOV,
        // MOV, WRMSR), run 2 on the same `boot` retires the rest (MOV, MOV, WRMSR, HLT) and
        // reports the replaced value.
        let (mut boot, file) = boot_with_entry(&code);
        let run1 = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(run1.retired, 4, "{run1:?}");
        assert_eq!(run1.guest_kernel_gs_base, first);
        assert_eq!(boot.board.guest_kernel_gs_base(), first);
        let run2 = run_set_facade(&mut boot, &file, 4, members);
        assert_eq!(run2.exit, PagedProbeExit::Halted, "{run2:?}");
        assert_eq!(run2.retired, 4, "MOV, MOV, WRMSR, HLT");
        assert_eq!(run2.guest_kernel_gs_base, second);
        assert_eq!(boot.board.guest_kernel_gs_base(), second);

        // Without the member the WRMSR is the stop (after MOV, MOV, MOV) and the cell stays 0; the
        // other MSR members do not admit KERNEL_GS_BASE.
        for members in [
            LinuxCpl0SetMembers::minimal(),
            LinuxCpl0SetMembers {
                msr_fs_gs_base: true,
                msr_star: true,
                msr_lstar: true,
                msr_cstar: true,
                msr_sysenter_cs: true,
                msr_sysenter_stack: true,
                msr_syscall_mask: true,
                ..LinuxCpl0SetMembers::minimal()
            },
            LinuxCpl0SetMembers {
                msr_kernel_gs_base: false,
                ..LinuxCpl0SetMembers::linux_default()
            },
        ] {
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_kernel_gs_base, 0);
            assert_eq!(boot.board.guest_kernel_gs_base(), 0);
            assert!(
                !record
                    .set_members
                    .contains(&"msr:kernel_gs_base_write".to_string())
            );
        }
    }

    #[test]
    fn wrmsr_kernel_gs_base_of_zero_is_a_write_through_the_board_set() {
        // The boundary of the diagnostic run: MOV ECX,0xC0000102; XOR EAX,EAX; XOR EDX,EDX; WRMSR
        // (EDX:EAX = 0:0), here after a non-zero value, so that the zero is observable as a write.
        let code = [
            0xB9, 0x02, 0x01, 0x00, 0xC0, 0x31, 0xC0, 0x31, 0xD2, 0x0F, 0x30, 0xF4,
        ];
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::linux_default()
        };
        let (mut boot, file) = boot_with_entry(&code);
        boot.board.set_guest_kernel_gs_base(0xFFFF_8880_1234_5678);
        let record = run_set_facade(&mut boot, &file, 16, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 5, "MOV, XOR, XOR, WRMSR, HLT");
        assert_eq!(record.guest_kernel_gs_base, 0);
        assert_eq!(boot.board.guest_kernel_gs_base(), 0);
    }

    #[test]
    fn rdmsr_syscall_and_sysenter_stay_the_stop_after_the_kernel_gs_base_write_through_the_board_set()
     {
        // The write is retired, the next instruction is the stop (RDMSR: not readable; SYSCALL,
        // SYSRET, SYSENTER, SYSEXIT: not modelled); the value is in the record. SWAPGS is no
        // longer here (S4-38L9A): it is executed, see the tests below.
        for (tail, opcode) in [
            (&[0x0F, 0x32][..], 0x32u8),
            (&[0x0F, 0x05], 0x05),
            (&[0x0F, 0x07], 0x07),
            (&[0x0F, 0x34], 0x34),
            (&[0x0F, 0x35], 0x35),
        ] {
            let mut code = KERNEL_GS_BASE_CODE.to_vec();
            code.extend_from_slice(tail);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode },
                "{tail:02X?} {record:?}"
            );
            assert_eq!(record.retired, 4);
            assert_eq!(record.guest_kernel_gs_base, 0xFFFF_8880_1234_5678);
        }

        // The neighbours 0xC0000103, 0xC0010102 and 0xC0000002 are no member and are the stop with
        // KERNEL_GS_BASE untouched (patching the ECX immediate: bytes 1..=4 of the first MOV).
        for ecx in [
            [0x03, 0x01, 0x00, 0xC0],
            [0x02, 0x01, 0x01, 0xC0],
            [0x02, 0x00, 0x00, 0xC0],
        ] {
            let mut code = KERNEL_GS_BASE_CODE.to_vec();
            code[1..5].copy_from_slice(&ecx);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x30 },
                "{ecx:02X?} {record:?}"
            );
            assert_eq!(record.retired, 3);
            assert_eq!(record.guest_kernel_gs_base, 0);
        }
        // The neighbours 0xC0000101 (GS base, its own member) and 0xC0000084 (SYSCALL_MASK, its
        // own member) retire their write and leave KERNEL_GS_BASE untouched.
        let mut code = KERNEL_GS_BASE_CODE.to_vec();
        code[1] = 0x01;
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.guest_kernel_gs_base, 0);
        let mut code = KERNEL_GS_BASE_CODE.to_vec();
        code[1..3].copy_from_slice(&[0x84, 0x00]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.guest_syscall_mask, 0xFFFF_8880_1234_5678);
        assert_eq!(record.guest_kernel_gs_base, 0);
    }

    #[test]
    fn linux_default_set_is_cli_cr3_cpuid_and_popf() {
        assert_eq!(
            LinuxCpl0SetMembers::linux_default(),
            LinuxCpl0SetMembers {
                cpuid: true,
                rdtsc: true,
                popf_cpl0: true,
                sti: true,
                pio_u8: true,
                pci_config: true,
                msr: true,
                msr_fs_gs_base: true,
                msr_star: true,
                msr_lstar: true,
                msr_cstar: true,
                msr_sysenter_cs: true,
                msr_sysenter_stack: true,
                msr_syscall_mask: true,
                msr_kernel_gs_base: true,
                fetch_cache: true,
                irq: true,
                ..LinuxCpl0SetMembers::minimal()
            }
        );
        assert!(!LinuxCpl0SetMembers::minimal().fetch_cache);
        assert!(!LinuxCpl0SetMembers::minimal().irq);
        assert!(LinuxCpl0SetMembers::linux_default().pio_u8);
        assert!(!LinuxCpl0SetMembers::minimal().pio_u8);
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.set_members,
            [
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ]
        );
        assert_eq!(record.cpuid_divergence_leaf, None);
    }

    #[test]
    fn linux_default_runs_sti_and_minimal_does_not() {
        // S4-38L7bp: `STI; NOP; HLT`. With the default set STI and NOP retire (IF = 1, no
        // interrupt is delivered) and the run stops at the HLT, which the default set lacks.
        assert!(LinuxCpl0SetMembers::linux_default().sti);
        assert!(!LinuxCpl0SetMembers::minimal().sti);
        let (mut boot, file) = boot_with_entry(&[0xFB, 0x90, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::linux_default());
        assert_eq!(record.exit_kind, "unsupported", "{record:?}");
        assert_eq!(record.unsupported_opcode, Some(0xF4), "{record:?}");
        assert_eq!((record.retired, record.rip), (2, ENTRY + 2), "{record:?}");
        assert_eq!(record.rflags & (1 << 9), 1 << 9, "{record:?}");
        // The minimal set stops at the STI itself.
        let (mut boot, file) = boot_with_entry(&[0xFB, 0x90, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.unsupported_opcode, Some(0xFB), "{record:?}");
        assert_eq!((record.retired, record.rip), (0, ENTRY), "{record:?}");
    }

    #[test]
    fn cpuid_boundary_answers_other_leaves_from_diagnostic_v1() {
        // CLI; XOR EAX,EAX; CPUID(0); MOV EAX,0x80000000; CPUID; HLT.
        let mut code = vec![0xFA, 0x31, 0xC0, 0x0F, 0xA2];
        code.extend(mov_eax(0x8000_0000));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.exit_kind, "halted");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert_eq!(record.retired, 6, "CLI, XOR, CPUID, MOV, CPUID, HLT");
        // The second CPUID answered leaf 0x8000_0000: max extended leaf, other words zero.
        assert_eq!(record.gpr[Register::Rax as usize], 0x8000_0008);
        for register in [Register::Rbx, Register::Rcx, Register::Rdx] {
            assert_eq!(record.gpr[register as usize], 0, "{register:?}");
        }
        // The first one, on its own, answered leaf 0: vendor "OpenDesktop".
        let (mut boot, file) = boot_with_entry(&[0xFA, 0x31, 0xC0, 0x0F, 0xA2, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4);
        assert_eq!(record.gpr[Register::Rax as usize], 1);
        let word = |bytes: &[u8; 4]| u64::from(u32::from_le_bytes(*bytes));
        assert_eq!(record.gpr[Register::Rbx as usize], word(b"Open"));
        assert_eq!(record.gpr[Register::Rdx as usize], word(b"Desk"));
        assert_eq!(record.gpr[Register::Rcx as usize], word(b"top "));
        // A leaf outside the profile's list (0x21) is answered with zeros, not stopped.
        let mut code = vec![0xFA];
        code.extend(mov_eax(0x21));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4);
        assert_eq!(record.gpr[Register::Rax as usize], 0);
    }

    /// CLI; MOV EBX, 0x1234; MOV EAX, `leaf`; CPUID; HLT.
    fn divergence_program(prefix: &[u8], leaf: u32) -> Vec<u8> {
        let mut code = vec![0xFA, 0xBB, 0x34, 0x12, 0x00, 0x00];
        code.extend(mov_eax(leaf));
        code.extend(prefix);
        code.extend([0x0F, 0xA2, 0xF4]);
        code
    }

    fn assert_stops_before_cpuid(prefix: &[u8], leaf: u32) {
        let (mut boot, file) = boot_with_entry(&divergence_program(prefix, leaf));
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        let rip = ENTRY + 1 + 5 + 5;
        assert_eq!(record.exit_kind, "cpuid_divergence", "{record:?}");
        assert_eq!(record.exit, PagedProbeExit::Stopped, "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, Some(leaf));
        assert_eq!(
            record.set_exit_detail,
            Some(format!("CpuidDivergence {{ leaf: {leaf} }}"))
        );
        // CLI, MOV EBX and MOV EAX retired; the CPUID did not.
        assert_eq!((record.retired, record.rip), (3, rip), "{record:?}");
        assert_eq!(record.rip_gpa, Some(rip), "identity mapped");
        assert_eq!(
            record.rip_bytes[..prefix.len() + 2],
            [prefix, &[0x0F, 0xA2]].concat()[..]
        );
        // Nothing answered: EAX is still the leaf, EBX the seed, ECX and EDX untouched (zero).
        assert_eq!(record.gpr[Register::Rax as usize], u64::from(leaf));
        assert_eq!(record.gpr[Register::Rbx as usize], 0x1234);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.virtual_ns, 3, "only the retired instructions");
        assert_eq!(
            record.set_members,
            [
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "halt",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ]
        );
        assert_eq!(boot.board.state().cr3, None);
        assert_eq!(boot.board.state().rip, rip);
        // The board stays on the CPUID: a second run stops at once, retiring nothing.
        let again = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(again.exit_kind, "cpuid_divergence", "{again:?}");
        assert_eq!((again.retired, again.rip), (0, rip));
        assert_eq!(again.gpr, record.gpr);
    }

    #[test]
    fn cpuid_boundary_stops_before_leaf_1() {
        assert_stops_before_cpuid(&[], 1);
    }

    #[test]
    fn cpuid_boundary_stops_before_leaf_0x80000001() {
        assert_stops_before_cpuid(&[], 0x8000_0001);
    }

    #[test]
    fn cpuid_boundary_sees_a_prefixed_cpuid() {
        // REX.W and operand-size prefixes do not slip past the boundary; RIP is on the prefix.
        assert_stops_before_cpuid(&[0x48], 1);
        assert_stops_before_cpuid(&[0x66], 0x8000_0001);
    }

    #[test]
    fn cpuid_boundary_leaves_a_cpuid_of_another_leaf_to_the_set() {
        // Only EAX decides: leaf 2 and 0x80000002 are not boundary leaves.
        for leaf in [2u32, 0x8000_0002, 0x8000_0000] {
            let (mut boot, file) = boot_with_entry(&divergence_program(&[], leaf));
            let record = run_set_facade(&mut boot, &file, 64, halting_default());
            assert_eq!(record.exit, PagedProbeExit::Halted, "{leaf:#x}: {record:?}");
            assert_eq!(record.cpuid_divergence_leaf, None);
            assert_eq!(record.retired, 5, "{leaf:#x}: CLI, MOVs, CPUID, HLT");
        }
        // The boundary reads EAX only; the high half of RAX is not part of the leaf.
        let mut code = vec![0xFA, 0x48, 0xB8];
        code.extend_from_slice(&0x1_0000_0001u64.to_le_bytes());
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert_eq!(record.exit_kind, "cpuid_divergence", "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, Some(1));
        assert_eq!(record.gpr[Register::Rax as usize], 0x1_0000_0001);
    }

    #[test]
    fn cpuid_without_the_member_is_still_unsupported_and_no_boundary() {
        // CLI; MOV EAX, leaf; CPUID: without `cpuid` the set refuses 0F A2 for every leaf, and
        // the boundary is not in play (`exit_kind` stays `unsupported`).
        for leaf in [0u32, 0x8000_0000, 1, 0x8000_0001] {
            let mut code = vec![0xFA];
            code.extend(mov_eax(leaf));
            code.extend([0x0F, 0xA2, 0xF4]);
            let (mut boot, file) = boot_with_entry(&code);
            let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::minimal());
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0xA2 },
                "{leaf:#x}: {record:?}"
            );
            assert_eq!(record.exit_kind, "unsupported");
            assert_eq!(record.cpuid_divergence_leaf, None);
            assert_eq!((record.retired, record.rip), (2, ENTRY + 6));
        }
    }

    #[test]
    fn cpuid_boundary_does_not_apply_on_the_paged_probe_path() {
        let (mut boot, file) = boot_with_entry(&[0xFA]);
        let record = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 8)
            .expect("runs");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert_eq!(record.exit_kind, "unsupported");
    }

    // ---- S4-38L7: the diagnostic mode past C7 ----

    fn diagnostic_default() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            diagnostic_past_c7: true,
            ..halting_default()
        }
    }

    #[test]
    fn diagnostic_past_c7_is_off_by_default_and_stops_as_before() {
        assert!(!LinuxCpl0SetMembers::minimal().diagnostic_past_c7);
        assert!(!LinuxCpl0SetMembers::linux_default().diagnostic_past_c7);
        // Without the flag: the C7 stop, record not marked diagnostic (regression of S4-38L4).
        assert_stops_before_cpuid(&[], 1);
        assert_stops_before_cpuid(&[], 0x8000_0001);
        let (mut boot, file) = boot_with_entry(&divergence_program(&[], 1));
        let record = run_set_facade(&mut boot, &file, 64, halting_default());
        assert!(!record.diagnostic_past_c7);
    }

    #[test]
    fn diagnostic_past_c7_executes_leaf_1_with_the_diagnostic_v1_answer() {
        // CLI; MOV EBX, 0x1234; MOV EAX, 1; CPUID; HLT. Budget 4 ends right after the CPUID.
        let (mut boot, file) = boot_with_entry(&divergence_program(&[], 1));
        let cpuid_rip = ENTRY + 1 + 5 + 5;
        let record = run_set_facade(&mut boot, &file, 4, diagnostic_default());
        assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
        assert_eq!(record.exit_kind, "budget_exhausted");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert!(record.diagnostic_past_c7);
        assert_eq!(record.retired, 4, "CLI, MOV, MOV and the CPUID itself");
        assert_eq!(record.rip, cpuid_rip + 2, "RIP past the two-byte CPUID");
        // Exactly the DiagnosticV1 tuple of leaf 1, nothing substituted.
        assert_eq!(record.gpr[Register::Rax as usize], 0x600);
        assert_eq!(record.gpr[Register::Rbx as usize], 0x100);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        // The run goes on from there: the HLT retires.
        let next = run_set_facade(&mut boot, &file, 64, diagnostic_default());
        assert_eq!(next.exit, PagedProbeExit::Halted, "{next:?}");
        assert_eq!(next.retired, 1);
        assert_eq!(next.gpr[Register::Rax as usize], 0x600);
    }

    #[test]
    fn diagnostic_past_c7_executes_leaf_0x80000001_with_zeros() {
        // The board stays on its own state; a seed in ECX and EDX shows the CPUID wrote them.
        let mut code = vec![
            0xFA, 0xB9, 0x78, 0x56, 0x34, 0x12, 0xBA, 0x21, 0x43, 0x65, 0x07,
        ];
        code.extend(mov_eax(0x8000_0001));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, diagnostic_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert_eq!(record.retired, 6, "CLI, three MOVs, CPUID, HLT");
        for register in [Register::Rax, Register::Rbx, Register::Rcx, Register::Rdx] {
            assert_eq!(record.gpr[register as usize], 0, "{register:?}");
        }
    }

    #[test]
    fn diagnostic_past_c7_does_not_change_other_leaves_or_the_set() {
        // Other leaves: the same results with and without the flag.
        for leaf in [0u32, 2, 0x8000_0000, 0x8000_0002, 0x21] {
            let (mut plain_boot, file) = boot_with_entry(&divergence_program(&[], leaf));
            let plain = run_set_facade(&mut plain_boot, &file, 64, halting_default());
            let (mut boot, file) = boot_with_entry(&divergence_program(&[], leaf));
            let diag = run_set_facade(&mut boot, &file, 64, diagnostic_default());
            assert_eq!(diag.exit, PagedProbeExit::Halted, "{leaf:#x}: {diag:?}");
            assert_eq!(diag.gpr, plain.gpr, "{leaf:#x}");
            assert_eq!((diag.retired, diag.rip), (plain.retired, plain.rip));
            assert_eq!(diag.set_members, plain.set_members, "{leaf:#x}");
            assert!(diag.diagnostic_past_c7 && !plain.diagnostic_past_c7);
        }
        // The set members are the linux set: no label, no extra member.
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 8, diagnostic_default());
        assert_eq!(
            record.set_members,
            [
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "rdtsc",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "msr:star_write",
                "msr:lstar_write",
                "msr:cstar_write",
                "msr:sysenter_cs_write",
                "msr:sysenter_stack_write",
                "msr:syscall_mask_write",
                "msr:kernel_gs_base_write",
                "halt",
                "pio_u8",
                "sti",
                "popf_cpl0",
                "pci_config",
                "irq"
            ]
        );
    }

    #[test]
    fn diagnostic_past_c7_without_cpuid_member_stays_unsupported() {
        let mut code = vec![0xFA];
        code.extend(mov_eax(1));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            diagnostic_past_c7: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xA2 });
        assert!(record.diagnostic_past_c7);
    }

    // ---- S4-38L9a: the CPUID subset profile X8664V1Stage4A ----

    /// The halting default set with the L9a subset profile, no `diagnostic_past_c7`.
    fn stage4a_default() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4A,
            ..halting_default()
        }
    }

    #[test]
    fn cpuid_profile_defaults_to_diagnostic_v1() {
        assert_eq!(
            LinuxCpl0SetMembers::minimal().cpuid_profile,
            CpuidProfile::DiagnosticV1
        );
        assert_eq!(
            LinuxCpl0SetMembers::linux_default().cpuid_profile,
            CpuidProfile::DiagnosticV1
        );
    }

    #[test]
    fn stage4a_leaf_1_runs_without_the_c7_stop_and_gives_fpu_tsc_fxsr() {
        // CLI; MOV EBX, 0x1234; MOV EAX, 1; CPUID; HLT, with no `diagnostic_past_c7`: the C7
        // boundary does not fire for the subset profile.
        let (mut boot, file) = boot_with_entry(&divergence_program(&[], 1));
        let record = run_set_facade(&mut boot, &file, 64, stage4a_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.exit_kind, "halted");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert!(!record.diagnostic_past_c7);
        assert_eq!(record.retired, 5, "CLI, MOV, MOV, CPUID, HLT");
        // Oracle from the contract table: EAX 0x600, EBX 0x100, ECX 0, EDX bits 0 (FPU), 4 (TSC)
        // and 24 (FXSR) since S4-38L9d.
        assert_eq!(record.gpr[Register::Rax as usize], 0x600);
        assert_eq!(record.gpr[Register::Rbx as usize], 0x100);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0x0100_0011);
        assert!(
            record
                .set_members
                .contains(&"cpuid:X8664V1Stage4A".to_string()),
            "{:?}",
            record.set_members
        );
        assert!(!record.set_members.iter().any(|m| m == "cpuid:DiagnosticV1"));
    }

    #[test]
    fn stage4a_leaf_0x80000001_runs_without_the_c7_stop_and_gives_zeros() {
        let mut code = vec![
            0xFA, 0xB9, 0x78, 0x56, 0x34, 0x12, 0xBA, 0x21, 0x43, 0x65, 0x07,
        ];
        code.extend(mov_eax(0x8000_0001));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, stage4a_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert_eq!(record.retired, 6, "CLI, three MOVs, CPUID, HLT");
        for register in [Register::Rax, Register::Rbx, Register::Rcx, Register::Rdx] {
            assert_eq!(record.gpr[register as usize], 0, "{register:?}");
        }
    }

    #[test]
    fn stage4a_leaf_1_ignores_ecx_on_the_board() {
        // CLI; MOV ECX, 0x12345678; MOV EAX, 1; CPUID; HLT: the same tuple as with ECX = 0.
        let mut code = vec![0xFA, 0xB9, 0x78, 0x56, 0x34, 0x12];
        code.extend(mov_eax(1));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, stage4a_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.gpr[Register::Rax as usize], 0x600);
        assert_eq!(record.gpr[Register::Rbx as usize], 0x100);
        assert_eq!(record.gpr[Register::Rcx as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0x0100_0011);
    }

    #[test]
    fn stage4a_unclaimed_leaves_give_zeros_on_the_board() {
        for leaf in [2u32, 7, 0x8000_0002, 0x8000_0007, 0x21] {
            let (mut boot, file) = boot_with_entry(&divergence_program(&[], leaf));
            let record = run_set_facade(&mut boot, &file, 64, stage4a_default());
            assert_eq!(record.exit, PagedProbeExit::Halted, "{leaf:#x}: {record:?}");
            for register in [Register::Rax, Register::Rcx, Register::Rdx] {
                assert_eq!(record.gpr[register as usize], 0, "{leaf:#x} {register:?}");
            }
            // EBX is seeded with 0x1234 by the program and the CPUID overwrote it with zero.
            assert_eq!(record.gpr[Register::Rbx as usize], 0, "{leaf:#x}");
        }
    }

    #[test]
    fn stage4a_changes_only_the_cpuid_member_name_of_the_set() {
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xF4]);
        let old = run_set_facade(&mut boot, &file, 8, halting_default());
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xF4]);
        let new = run_set_facade(&mut boot, &file, 8, stage4a_default());
        let renamed: Vec<String> = old
            .set_members
            .iter()
            .map(|name| {
                if name == "cpuid:DiagnosticV1" {
                    "cpuid:X8664V1Stage4A".to_string()
                } else {
                    name.clone()
                }
            })
            .collect();
        assert_eq!(new.set_members, renamed);
    }

    #[test]
    fn stage4a_without_cpuid_member_stays_unsupported() {
        let mut code = vec![0xFA];
        code.extend(mov_eax(1));
        code.extend([0x0F, 0xA2, 0xF4]);
        let (mut boot, file) = boot_with_entry(&code);
        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::X8664V1Stage4A,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xA2 });
    }

    #[test]
    fn diagnostic_v1_boundary_is_unchanged_with_the_explicit_profile_field() {
        // The explicit DiagnosticV1 profile behaves exactly like the default: C7 stop.
        let members = LinuxCpl0SetMembers {
            cpuid_profile: CpuidProfile::DiagnosticV1,
            ..halting_default()
        };
        let (mut boot, file) = boot_with_entry(&divergence_program(&[], 1));
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit_kind, "cpuid_divergence", "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, Some(1));
    }

    // ---- S4-38L9c: RDMSR 0x87 (IA32_MKTME_KEYID_PARTITIONING) on the board ----

    /// MOV ECX,0x87; MOV EAX,0x55; MOV EDX,0x66; RDMSR; HLT: the boundary of the stage4a run
    /// (ECX = 0x87, `0F 32`) with garbage in EAX/EDX.
    const RDMSR_0X87_CODE: [u8; 17] = [
        0xB9, 0x87, 0x00, 0x00, 0x00, 0xB8, 0x55, 0x00, 0x00, 0x00, 0xBA, 0x66, 0x00, 0x00, 0x00,
        0x0F, 0x32,
    ];

    fn tdx_read_default() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            msr_tdx_keyid_partitioning_read: true,
            ..stage4a_default()
        }
    }

    #[test]
    fn rdmsr_0x87_is_off_in_the_default_members_and_the_default_set_names_do_not_change() {
        assert!(!LinuxCpl0SetMembers::minimal().msr_tdx_keyid_partitioning_read);
        assert!(!LinuxCpl0SetMembers::linux_default().msr_tdx_keyid_partitioning_read);
        // Without the member the read is the stop it was (0x32), RAX/RDX as written by the MOVs.
        for members in [halting_default(), stage4a_default()] {
            let (mut boot, file) = boot_with_entry(&RDMSR_0X87_CODE);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0x32 },
                "{record:?}"
            );
            assert_eq!(record.retired, 3, "MOV, MOV, MOV; the RDMSR is the stop");
            assert_eq!(record.gpr[Register::Rcx as usize], 0x87);
            assert_eq!(record.gpr[Register::Rax as usize], 0x55);
            assert!(
                !record
                    .set_members
                    .contains(&"msr:tdx_keyid_partitioning_read".to_string())
            );
        }
    }

    #[test]
    fn rdmsr_0x87_returns_zero_through_the_board_set() {
        let mut code = vec![0xFA];
        code.extend(RDMSR_0X87_CODE);
        code.push(0xF4);
        let (mut boot, file) = boot_with_entry(&code);
        let record = run_set_facade(&mut boot, &file, 64, tdx_read_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 6, "CLI, MOV, MOV, MOV, RDMSR, HLT");
        // Oracle: EDX:EAX = 0:0 (kernel tdx_init treats no TDX KeyIDs as TDX off); RCX kept.
        assert_eq!(record.gpr[Register::Rax as usize], 0);
        assert_eq!(record.gpr[Register::Rdx as usize], 0);
        assert_eq!(record.gpr[Register::Rcx as usize], 0x87);
        // The member is named, last of the MSR members, and no cell or EFER moved.
        let names = &record.set_members;
        let position = names
            .iter()
            .position(|name| name == "msr:tdx_keyid_partitioning_read")
            .expect("member named");
        assert_eq!(names[position - 1], "msr:kernel_gs_base_write");
        assert_eq!(record.guest_efer, 0xD00);
        assert_eq!(
            (record.guest_star, record.guest_lstar, record.guest_cstar),
            (0, 0, 0)
        );
    }

    #[test]
    fn rdmsr_other_indexes_and_wrmsr_0x87_stay_the_stop_with_the_member() {
        // MOV ECX,0x88; RDMSR: the neighbour is no member index, so since S4-38L9v it is the
        // `#GP(0)` of an MSR the set does not model (the Linux entry state has no IDT, so the
        // fault is the stop; before S4-38L9v it was `Unsupported { opcode: 0x32 }`).
        let (mut boot, file) = boot_with_entry(&[0xB9, 0x88, 0x00, 0x00, 0x00, 0x0F, 0x32]);
        let record = run_set_facade(&mut boot, &file, 64, tdx_read_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Fault(ArchitecturalFault {
                vector: 13,
                error_code: Some(0),
                linear_address: None
            }),
            "{record:?}"
        );
        assert_eq!(record.retired, 1);
        // MOV ECX,0x87; WRMSR: the write of 0x87 is refused.
        let (mut boot, file) = boot_with_entry(&[0xB9, 0x87, 0x00, 0x00, 0x00, 0x0F, 0x30]);
        let record = run_set_facade(&mut boot, &file, 64, tdx_read_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
        assert_eq!(record.retired, 1);
    }

    // ---- S4-38L9d: FP state (FNINIT, FXSAVE, MOV CR0, RDI) on the board ----

    /// The members of the stage4a run: the CPUID subset that declares FPU and FXSR, the FP member
    /// and HLT (so that a test can end on a retired HLT).
    fn fp_default() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            fp_state: true,
            ..stage4a_default()
        }
    }

    /// GPA of the entry (`startup_64`): the kernel at 16 MiB plus 0x200.
    const FP_ENTRY_GPA: u64 = 0x0100_0200;
    /// The FXSAVE target inside the kernel zone of the board RAM, 16-byte aligned.
    const FP_AREA_GPA: u64 = 0x0100_0600;

    /// `CLI; MOV RAX,CR0; AND RAX,-13; MOV RDI,RAX; MOV CR0,RDI; MOV EDI,0x620; MOV CR4,RDI; FNINIT;
    /// FXSAVE [RIP+disp] -> FP_AREA_GPA; HLT`: the `fpu__init_system` sequence of the kernel.
    fn fp_kernel_sequence() -> Vec<u8> {
        let mut code = vec![
            0xFA, 0x0F, 0x20, 0xC0, 0x48, 0x83, 0xE0, 0xF3, 0x48, 0x89, 0xC7, 0x0F, 0x22, 0xC7,
            0xBF, 0x20, 0x06, 0x00, 0x00, 0x0F, 0x22, 0xE7, 0xDB, 0xE3,
        ];
        let next = FP_ENTRY_GPA + code.len() as u64 + 7;
        let disp = (FP_AREA_GPA as i64 - next as i64) as i32;
        code.extend([0x0F, 0xAE, 0x05]);
        code.extend(disp.to_le_bytes());
        code.push(0xF4);
        code
    }

    fn read_area(boot: &mut LinuxDirectBoot) -> [u8; 512] {
        let mut area = [0u8; 512];
        read(&mut boot.board, FP_AREA_GPA, &mut area).expect("reads");
        area
    }

    #[test]
    fn fp_state_is_off_in_the_default_members_and_the_default_runs_stay_as_they_were() {
        assert!(!LinuxCpl0SetMembers::minimal().fp_state);
        assert!(!LinuxCpl0SetMembers::linux_default().fp_state);
        // Without the member `FNINIT` is the stop it was (0xdb) and nothing is named.
        for members in [halting_default(), stage4a_default()] {
            let (mut boot, file) = boot_with_entry(&[0xFA, 0xDB, 0xE3, 0xF4]);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0xdb },
                "{record:?}"
            );
            assert_eq!(record.retired, 1, "CLI; FNINIT is the stop");
            assert!(!record.set_members.contains(&"fp_state".to_string()));
        }
    }

    #[test]
    fn the_kernel_fpu_init_sequence_runs_through_the_board_set_and_writes_the_image() {
        let (mut boot, file) = boot_with_entry(&fp_kernel_sequence());
        let before = read_area(&mut boot);
        let record = run_set_facade(&mut boot, &file, 64, fp_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(
            record.retired, 10,
            "CLI, six CR/ALU/MOV, FNINIT, FXSAVE, HLT: {record:?}"
        );
        // CR0 lost TS/EM only (the entry value has neither: it stays), CR4 holds the two FP bits.
        assert_eq!(record.cr0.expect("cr0") & 0xc, 0);
        assert_eq!(record.cr4.expect("cr4") & 0x600, 0x600);
        // Oracle (SDM FXSAVE layout): FCW 0x037F, FSW 0, abridged FTW 0, FOP/FIP/FCS/FDP/FDS 0,
        // MXCSR 0x1F80, MXCSR_MASK 0xFFBF, registers and XMM zero; the holes are untouched.
        let after = read_area(&mut boot);
        let mut expected = before;
        expected[0..2].copy_from_slice(&[0x7F, 0x03]);
        expected[2..4].copy_from_slice(&[0, 0]);
        expected[4] = 0;
        expected[6..8].copy_from_slice(&[0, 0]);
        expected[8..14].copy_from_slice(&[0; 6]);
        expected[16..22].copy_from_slice(&[0; 6]);
        expected[24..28].copy_from_slice(&[0x80, 0x1F, 0, 0]);
        expected[28..32].copy_from_slice(&[0xBF, 0xFF, 0, 0]);
        for slot in 0..8 {
            expected[32 + 16 * slot..42 + 16 * slot].copy_from_slice(&[0; 10]);
        }
        expected[160..416].copy_from_slice(&[0; 256]);
        assert_eq!(after, expected);
        // The member is named after the MSR members (the tdx read is off in this member set).
        let names = &record.set_members;
        let position = names
            .iter()
            .position(|name| name == "fp_state")
            .expect("member named");
        assert_eq!(names[position - 1], "msr:kernel_gs_base_write");
        assert_eq!(names[position + 1], "halt");
        assert_eq!(boot.board.guest_fp(), &od_isa::GuestFpState::initial());
    }

    #[test]
    fn the_board_keeps_the_guest_fp_state_between_runs_and_fninit_resets_the_environment() {
        let (mut boot, file) = boot_with_entry(&fp_kernel_sequence());
        let mut dirty = od_isa::GuestFpState::initial();
        dirty.x87.control_word = 0x027F;
        dirty.x87.status_word = 0x3805;
        dirty.x87.tag_word = 0x1BE4;
        dirty.x87.instruction_pointer = 0x1234;
        dirty.x87.registers[3] = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
        dirty.x87.instruction_selector = Some(0x2B);
        dirty.sse.mxcsr = 0x1FA0;
        dirty.sse.xmm[15] = [0xEE; 16];
        boot.board.set_guest_fp(dirty.clone());
        let record = run_set_facade(&mut boot, &file, 64, fp_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        let left = boot.board.guest_fp().clone();
        assert_eq!(left.x87.control_word, 0x037F);
        assert_eq!(left.x87.status_word, 0);
        assert_eq!(left.x87.tag_word, 0xFFFF);
        assert_eq!(left.x87.instruction_pointer, 0);
        assert_eq!(left.x87.instruction_selector, Some(0));
        // Payloads, XMM and MXCSR survive FNINIT.
        assert_eq!(left.x87.registers, dirty.x87.registers);
        assert_eq!(left.sse, dirty.sse);
        // The image carries MXCSR and XMM15 of the carried state.
        let area = read_area(&mut boot);
        assert_eq!(&area[24..28], &[0xA0, 0x1F, 0, 0]);
        assert_eq!(&area[160 + 15 * 16..160 + 16 * 16], &[0xEE; 16]);
        assert_eq!(
            &area[32 + 3 * 16..32 + 3 * 16 + 10],
            &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
        );
    }

    #[test]
    fn the_fp_member_needs_a_cpuid_profile_that_declares_fpu_and_fxsr() {
        // The DiagnosticV1 profile declares nothing: the set refuses the run before any fetch.
        let members = LinuxCpl0SetMembers {
            fp_state: true,
            ..halting_default()
        };
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xDB, 0xE3, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(
            record.exit,
            PagedProbeExit::Rejected(crate::PagedProbeRejection::Cpl0Set(
                od_isa::Cpl0ProfileSetInputError::FpFeaturesNotDeclared
            )),
            "{record:?}"
        );
        assert_eq!(record.retired, 0);
    }

    // ---- S4-38L9e: INT3 (#BP) through the guest IDT on the board ----

    /// A board that runs `code` at the entry. IDT gate 3 (CS 0x10, `attributes`) leads to a
    /// handler that is one `IRETQ`; IF = 1 in the entry flags.
    fn int3_board(attributes: u8, code: &[u8]) -> (LinuxDirectBoot, Vec<u8>) {
        let (mut boot, file) = boot_with_entry(code);
        write_phys(&mut boot, PF_HANDLER_AT, &[0x48, 0xCF]);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(PF_HANDLER_AT as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[5] = attributes;
        gate[6..8].copy_from_slice(&((PF_HANDLER_AT >> 16) as u16).to_le_bytes());
        write_phys(&mut boot, PF_IDT_AT + 3 * 16, &gate);
        let state = boot.board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0xFF,
        });
        state.rflags = 2 | (1 << 9) | 1;
        (boot, file)
    }

    #[test]
    fn int3_member_is_off_in_the_default_member_sets() {
        assert!(!LinuxCpl0SetMembers::minimal().int3);
        assert!(!LinuxCpl0SetMembers::linux_default().int3);
        // Without the member the `INT3` is the stop it was (0xcc) and nothing is named.
        for members in [halting_default(), stage4a_default()] {
            let (mut boot, file) = int3_board(0x8E, &[0xFA, 0xCC, 0xF4]);
            let record = run_set_facade(&mut boot, &file, 64, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0xcc },
                "{record:?}"
            );
            assert_eq!(record.retired, 1, "CLI; INT3 is the stop");
            assert_eq!(record.rip, ENTRY + 1);
            assert!(!record.set_members.contains(&"int3".to_string()));
        }
    }

    #[test]
    fn int3_through_the_board_set_delivers_the_frame_and_the_handler_returns_after_it() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = int3_board(0xEE, &[0xCC, 0x90]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        // One instruction: the INT3 retires into the handler with IF cleared (interrupt gate).
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli).with_int3());
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(
            report.delivered_exceptions, 0,
            "not a board-delivered fault"
        );
        assert_eq!(state.rip, PF_HANDLER_AT);
        assert_eq!(state.cs_selector, Some(0x10));
        let frame = rsp & !15;
        assert_eq!(state.gpr[Register::Rsp as usize], frame - 40);
        assert_eq!(state.rflags & (1 << 9), 0, "an interrupt gate clears IF");
        // Hand-computed frame: SS 0x18, old RSP, RFLAGS 0x203 (IF kept, RF clear), CS 0x10 and the
        // address after the INT3.
        assert_eq!(read_qword(&mut boot, frame - 8), 0x18);
        assert_eq!(read_qword(&mut boot, frame - 16), rsp);
        assert_eq!(read_qword(&mut boot, frame - 24), 0x203);
        assert_eq!(read_qword(&mut boot, frame - 32), 0x10);
        assert_eq!(read_qword(&mut boot, frame - 40), ENTRY + 1);
        // Two instructions: the handler's IRETQ returns to the NOP after the INT3.
        let (mut boot, _) = int3_board(0xEE, &[0xCC, 0x90]);
        let (state, report) = run_set(&mut boot, true, 2, &minimal_set(&cli).with_int3());
        assert_eq!(report.instructions_executed, 2, "{report:?}");
        assert_eq!(state.rip, ENTRY + 1);
        assert_eq!(state.gpr[Register::Rsp as usize], rsp);
        assert_ne!(state.rflags & (1 << 9), 0, "IF back from the frame");
        assert_eq!(state.cs_selector, Some(0x10));
    }

    #[test]
    fn int3_through_a_trap_gate_keeps_if_on_the_board() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = int3_board(0x8F, &[0xCC, 0x90]);
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli).with_int3());
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, PF_HANDLER_AT);
        assert_ne!(state.rflags & (1 << 9), 0, "a trap gate keeps IF");
    }

    #[test]
    fn int3_stays_the_stop_on_the_board_set_without_the_member_or_with_a_bad_gate() {
        let cli = CliProfile::NonPviLong64V1;
        // Without the member: no write, the stop is the INT3.
        let (mut boot, _) = int3_board(0xEE, &[0xCC, 0x90]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        let (state, report) = run_set(&mut boot, true, 2, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0xcc }),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(state.rip, ENTRY);
        assert_eq!(read_qword(&mut boot, (rsp & !15) - 40), 0);
        // With the member but a gate that is not present: the same stop, still no write.
        let (mut boot, _) = int3_board(0x0E, &[0xCC, 0x90]);
        let (state, report) = run_set(&mut boot, true, 2, &minimal_set(&cli).with_int3());
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0xcc }),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(state.rip, ENTRY);
        assert_eq!(state.gpr[Register::Rsp as usize], rsp);
        assert_eq!(read_qword(&mut boot, (rsp & !15) - 40), 0);
    }

    #[test]
    fn int3_member_runs_through_the_board_facade_and_is_named_after_fp_state() {
        let members = LinuxCpl0SetMembers {
            int3: true,
            ..fp_default()
        };
        // CLI; INT3; HLT with the IRETQ handler: CLI, INT3, IRETQ and HLT retire.
        let (mut boot, file) = int3_board(0xEE, &[0xFA, 0xCC, 0xF4]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        let record = run_set_facade(&mut boot, &file, 64, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 4, "{record:?}");
        assert_eq!(record.rip, ENTRY + 3, "RIP after the HLT");
        assert_eq!(record.gpr[Register::Rsp as usize], rsp);
        let names = &record.set_members;
        let position = names
            .iter()
            .position(|name| name == "int3")
            .expect("member named");
        assert_eq!(names[position - 1], "fp_state");
        assert_eq!(names[position + 1], "halt");
    }

    // ---- S4-38L4e: instruction fetch across a 4 KiB page boundary in the set ----

    /// GVA of the first (always mapped) code page of the split tree.
    const SPLIT_VA: u64 = 0x0100_0000;
    /// The physical pages behind the two code pages (not contiguous on purpose).
    const SPLIT_PAGE0: u64 = 0x0500_0000;
    const SPLIT_PAGE1: u64 = 0x0500_8000;

    /// A tree that maps `SPLIT_VA` and `SPLIT_VA + 0x1000` through 4 KiB leaves (the second one
    /// only when `second_pte` is given) and puts `code` so that its first `on_first` bytes end
    /// the first page and the rest starts the second. Returns the state to run from.
    fn split_code_state(
        boot: &mut LinuxDirectBoot,
        code: &[u8],
        on_first: usize,
        second_pte: Option<u64>,
    ) -> LongModeState {
        write_phys(boot, TREE_PML4, &(TREE_PDPT | 3).to_le_bytes());
        write_phys(boot, TREE_PDPT, &(TREE_PD | 3).to_le_bytes());
        write_phys(boot, TREE_PD + 8 * 8, &(TREE_PT | 3).to_le_bytes());
        write_phys(boot, TREE_PT, &(SPLIT_PAGE0 | 3).to_le_bytes());
        if let Some(pte) = second_pte {
            write_phys(boot, TREE_PT + 8, &pte.to_le_bytes());
        }
        write_phys(
            boot,
            SPLIT_PAGE0 + 0x1000 - on_first as u64,
            &code[..on_first],
        );
        if on_first < code.len() {
            write_phys(boot, SPLIT_PAGE1, &code[on_first..]);
        }
        let mut state = boot.board.state().clone();
        state.cr3 = Some(TREE_PML4);
        state.rip = SPLIT_VA + 0x1000 - on_first as u64;
        state
    }

    fn run_split(
        boot: &mut LinuxDirectBoot,
        state: &mut LongModeState,
        set: &Cpl0ProfileSet<'_>,
    ) -> PagedCpl0SetReport {
        let efer = boot.board.efer();
        run_cpl0_set_paged_2m_ram(
            state,
            boot.board.bus_mut(),
            TREE_PML4,
            efer,
            long64(),
            1,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |_| {},
        )
    }

    const JE_REL32: [u8; 6] = [0x0F, 0x84, 0x1F, 0x02, 0x00, 0x00];

    #[test]
    fn cpl0_set_board_jcc_rel32_across_a_page_boundary_executes_for_every_split() {
        let cli = CliProfile::NonPviLong64V1;
        for on_first in 1..JE_REL32.len() {
            for (zf, target) in [(true, 6 + 0x21F), (false, 6)] {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let mut state =
                    split_code_state(&mut boot, &JE_REL32, on_first, Some(SPLIT_PAGE1 | 3));
                let start = state.rip;
                if zf {
                    state.rflags |= 1 << 6;
                } else {
                    state.rflags &= !(1 << 6);
                }
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                assert_eq!(
                    report.exit,
                    PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                    "{on_first} {zf}: {report:?}"
                );
                assert_eq!(report.instructions_executed, 1, "{on_first} {zf}");
                assert_eq!((report.page_fault, report.memory_error), (None, None));
                assert_eq!(state.rip, start + target, "{on_first} {zf}");
            }
        }
    }

    #[test]
    fn cpl0_set_board_mov_imm64_across_a_page_boundary_executes_for_every_split() {
        let cli = CliProfile::NonPviLong64V1;
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        for on_first in 1..code.len() {
            let (mut boot, _) = boot_with_entry(&[0xF4]);
            let mut state = split_code_state(&mut boot, &code, on_first, Some(SPLIT_PAGE1 | 3));
            let start = state.rip;
            let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
            assert_eq!(report.instructions_executed, 1, "{on_first}: {report:?}");
            assert_eq!((report.page_fault, report.memory_error), (None, None));
            assert_eq!(state.rip, start + 10, "{on_first}");
            assert_eq!(state.gpr[Register::Rax as usize], 0x1122_3344_5566_7788);
        }
    }

    #[test]
    fn cpl0_set_board_a_fifteen_byte_instruction_across_a_page_boundary_executes() {
        // The longest standard NOP: six 0x66, CS override, 0F 1F /0 with SIB and disp32 (the
        // decoder admits repeated 0x66 for this form only): exactly 15 bytes.
        let cli = CliProfile::NonPviLong64V1;
        let mut code = vec![0x66; 6];
        code.extend([0x2E, 0x0F, 0x1F, 0x84, 0x00, 0x00, 0x00, 0x00, 0x00]);
        assert_eq!(code.len(), 15);
        for on_first in [1, 7, 8, 10, 14] {
            let (mut boot, _) = boot_with_entry(&[0xF4]);
            let mut state = split_code_state(&mut boot, &code, on_first, Some(SPLIT_PAGE1 | 3));
            let start = state.rip;
            let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
            assert_eq!(report.instructions_executed, 1, "{on_first}: {report:?}");
            assert_eq!((report.page_fault, report.memory_error), (None, None));
            assert_eq!(state.rip, start + 15, "{on_first}");
        }
    }

    #[test]
    fn cpl0_set_board_a_missing_second_page_is_the_precise_fetch_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let mut code = vec![0x48, 0xB8];
        code.extend_from_slice(&0x1122_3344_5566_7788u64.to_le_bytes());
        for (name, bytes) in [("je", &JE_REL32[..]), ("movabs", &code[..])] {
            for on_first in 1..bytes.len() {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                // The second PTE is absent.
                let mut state = split_code_state(&mut boot, bytes, on_first, None);
                let before = state.clone();
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
                    panic!("{name} {on_first}: {report:?}");
                };
                // CR2 is the first byte on the missing page; I/D = 1, P = 0, supervisor read.
                assert_eq!(
                    (fault.vector, fault.error_code, fault.linear_address),
                    (14, Some(0x10), Some(SPLIT_VA + 0x1000)),
                    "{name} {on_first}"
                );
                assert_eq!(report.instructions_executed, 0);
                assert_eq!(
                    report.page_fault.expect("record").saved_rip(),
                    before.rip,
                    "{name} {on_first}"
                );
                assert_eq!(report.memory_error, None);
                assert_eq!(state, before, "{name} {on_first}: the state is unchanged");
            }
        }
    }

    #[test]
    fn cpl0_set_board_a_non_executable_second_page_is_a_present_fetch_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        assert_ne!(boot.board.efer() & (1 << 11), 0, "the board runs with NXE");
        let mut state =
            split_code_state(&mut boot, &JE_REL32, 2, Some(SPLIT_PAGE1 | 3 | (1 << 63)));
        let before = state.clone();
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        // P = 1, I/D = 1.
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x11), Some(SPLIT_VA + 0x1000))
        );
        assert_eq!(state, before);
    }

    #[test]
    fn cpl0_set_board_an_instruction_ending_on_the_page_end_does_not_touch_the_next_page() {
        // The whole JE is on the first page: no fetch looks ahead, so an unmapped second
        // page changes nothing.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = split_code_state(&mut boot, &JE_REL32, JE_REL32.len(), None);
        let start = state.rip;
        state.rflags |= 1 << 6;
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.rip, start + 6 + 0x21F);
    }

    #[test]
    fn cpl0_set_board_cpuid_across_a_page_boundary_is_seen_by_the_boundary_and_executes() {
        // CPUID (0F A2) with 0F on the last byte of an identity-mapped page and A2 on the next.
        let at = 0x0100_0FFF;
        // Leaf 1 stops before the instruction, RIP on the 0F byte.
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        write_phys(&mut boot, at, &[0x0F, 0xA2, 0xF4]);
        let state = boot.board.state_mut();
        state.rip = at;
        state.gpr[Register::Rax as usize] = 1;
        let record = run_set_facade(&mut boot, &file, 8, halting_default());
        assert_eq!(record.exit_kind, "cpuid_divergence", "{record:?}");
        assert_eq!(record.cpuid_divergence_leaf, Some(1));
        assert_eq!((record.retired, record.rip), (0, at));
        // Leaf 0 is answered by the set: CPUID retires, then the HLT on the next page.
        let (mut boot, file) = boot_with_entry(&[0xF4]);
        write_phys(&mut boot, at, &[0x0F, 0xA2, 0xF4]);
        let state = boot.board.state_mut();
        state.rip = at;
        state.gpr[Register::Rax as usize] = 0;
        let record = run_set_facade(&mut boot, &file, 8, halting_default());
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!(record.retired, 2);
        assert_eq!(record.gpr[Register::Rax as usize], 1);
    }

    // ---- S4-38L4f: 16-bit reads (`MOVZX r32, m16`) through the set's board adapter ----

    /// The third page behind `SPLIT_VA + 0x2000` (not contiguous with the others).
    const SPLIT_PAGE2: u64 = 0x0500_4000;
    /// `MOVZX ESI, word [RCX]`.
    const MOVZX_ESI_WORD_RCX: [u8; 3] = [0x0F, 0xB7, 0x31];
    const MOVZX_SEED: u64 = 0xDEAD_BEEF_CAFE_F00D;

    /// The split tree with the code at the end of the first page, the data page `SPLIT_VA +
    /// 0x1000` and (when `third`) `SPLIT_VA + 0x2000` mapped with clear accessed/dirty bits,
    /// RSI holding `MOVZX_SEED` and RCX the linear address `at`.
    fn movzx_state(boot: &mut LinuxDirectBoot, at: u64, third: bool) -> LongModeState {
        let mut state = split_code_state(
            boot,
            &MOVZX_ESI_WORD_RCX,
            MOVZX_ESI_WORD_RCX.len(),
            Some(SPLIT_PAGE1 | 3),
        );
        if third {
            write_phys(boot, TREE_PT + 16, &(SPLIT_PAGE2 | 3).to_le_bytes());
        }
        state.gpr[Register::Rsi as usize] = MOVZX_SEED;
        state.gpr[Register::Rcx as usize] = at;
        state
    }

    #[test]
    fn cpl0_set_board_movzx_word_reads_two_bytes_and_zero_extends_into_the_whole_register() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = SPLIT_VA + 0x1100;
        let mut state = movzx_state(&mut boot, at, false);
        // The bytes around the word differ from it: only two bytes are read.
        write_phys(
            &mut boot,
            SPLIT_PAGE1 + 0x100,
            &0x8877_6655_4433_ABCDu64.to_le_bytes(),
        );
        let start = state.rip;
        let rflags = state.rflags;
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        // Bit 15 is set: MOVZX does not sign-extend, and the old upper bytes of RSI are gone.
        assert_eq!(state.gpr[Register::Rsi as usize], 0xABCD);
        assert_eq!(state.rip, start + 3);
        assert_eq!(state.rflags, rflags);
        assert_eq!(state.gpr[Register::Rcx as usize], at);
        // The leaf got its accessed bit and no dirty bit; the data page is unchanged.
        let pte = read_qword(&mut boot, TREE_PT + 8);
        assert_eq!(pte, SPLIT_PAGE1 | 3 | 0x20);
        assert_eq!(
            read_qword(&mut boot, SPLIT_PAGE1 + 0x100),
            0x8877_6655_4433_ABCD
        );
    }

    #[test]
    fn cpl0_set_board_movzx_word_on_the_last_two_bytes_of_a_page_is_one_translation() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = SPLIT_VA + 0x1FFE;
        let mut state = movzx_state(&mut boot, at, false);
        write_phys(&mut boot, SPLIT_PAGE1 + 0xFFE, &[0x34, 0x12]);
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rsi as usize], 0x1234);
        // Only the page that holds the word is touched: the next (unmapped) page is not walked.
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | 0x20);
        assert_eq!(read_qword(&mut boot, TREE_PT + 16), 0);
    }

    #[test]
    fn cpl0_set_board_movzx_word_across_a_page_boundary_translates_each_byte_on_its_page() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = SPLIT_VA + 0x1FFF;
        let mut state = movzx_state(&mut boot, at, true);
        // The low byte ends the first data page, the high byte starts the other (and distant) one.
        write_phys(&mut boot, SPLIT_PAGE1 + 0xFFF, &[0x34]);
        write_phys(&mut boot, SPLIT_PAGE2, &[0x92, 0xFF]);
        let start = state.rip;
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.gpr[Register::Rsi as usize], 0x9234);
        assert_eq!(state.rip, start + 3);
        // Both leaves are accessed, neither is dirty, and the data stays as written.
        assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | 0x20);
        assert_eq!(read_qword(&mut boot, TREE_PT + 16), SPLIT_PAGE2 | 3 | 0x20);
        assert_eq!(read_qword(&mut boot, SPLIT_PAGE2) & 0xFFFF, 0xFF92);
    }

    #[test]
    fn cpl0_set_board_movzx_word_with_a_missing_second_page_is_a_precise_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = SPLIT_VA + 0x1FFF;
        // The page of the high byte is not present.
        let mut state = movzx_state(&mut boot, at, false);
        let start = state.rip;
        let rflags = state.rflags;
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        // Not-present read at CPL0: error code 0, CR2 is the first byte on the missing page.
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0), Some(SPLIT_VA + 0x2000))
        );
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(report.memory_error, None);
        let record = report.page_fault.expect("the walker's record");
        assert_eq!(record.saved_rip(), start);
        assert_eq!(record.access(), od_isa::memory::AccessKind::Read);
        // Nothing retired: RSI, RIP and RFLAGS are as before.
        assert_eq!(state.gpr[Register::Rsi as usize], MOVZX_SEED);
        assert_eq!((state.rip, state.rflags), (start, rflags));
        // The first byte was already read and its page marked accessed (never dirty).
        assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | 0x20);
    }

    #[test]
    fn cpl0_set_board_movzx_word_from_an_unmapped_page_is_the_exact_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xFC, 0x0F, 0xB7, 0x03]);
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = 0x77;
        state.gpr[Register::Rbx as usize] = 256 * MIB;
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0), Some(256 * MIB))
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!(
            (state.rip, state.gpr[Register::Rax as usize]),
            (ENTRY + 1, 0x77)
        );
    }

    #[test]
    fn cpl0_set_board_movzx_word_on_the_identity_mapped_ram_reads_the_word() {
        let cli = CliProfile::NonPviLong64V1;
        // MOVZX EAX, word [RBX + 2] (0F B7 43 02): the word is the last two bytes of a 4 KiB
        // page of the identity-mapped 2 MiB leaf.
        let (mut boot, _) = boot_with_entry(&[0x0F, 0xB7, 0x43, 0x02]);
        write_phys(&mut boot, DATA + 0xFFE, &[0xEF, 0xBE, 0xAD, 0xDE]);
        let state = boot.board.state_mut();
        state.gpr[Register::Rax as usize] = u64::MAX;
        state.gpr[Register::Rbx as usize] = DATA + 0xFFC;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rax as usize], 0xBEEF);
        assert_eq!(state.rip, ENTRY + 4);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
    }

    // ---- S4-38L4h: stores across a 4 KiB page boundary through the set's board adapter ----

    /// `MOV [RCX+4], RDI`, the kernel's store at 0x0543_0920.
    const MOV_RCX4_RDI: [u8; 4] = [0x48, 0x89, 0x79, 0x04];
    /// `MOV [RCX+4], EDI`.
    const MOV_RCX4_EDI: [u8; 3] = [0x89, 0x79, 0x04];
    /// `MOV [RCX+4], DI` (66 89 79 04).
    const MOV_RCX4_DI: [u8; 4] = [0x66, 0x89, 0x79, 0x04];
    const STORE_VALUE: u64 = 0x8877_6655_4433_2211;
    /// The fill of both data pages around the store (it must survive the store).
    const STORE_FILL: u8 = 0xEE;
    /// The data pages: `SPLIT_VA + 0x1000` and `SPLIT_VA + 0x2000`.
    const STORE_PAGE_A: u64 = SPLIT_VA + 0x1000;
    const STORE_PAGE_B: u64 = SPLIT_VA + 0x2000;

    /// The split tree with the store at the end of the code page, the first data page mapped
    /// with `first_pte` and the second with `second_pte` (absent when `None`), both filled with
    /// `STORE_FILL`, RDI = `STORE_VALUE` and RCX such that the store lands on `at`.
    fn store_state(
        boot: &mut LinuxDirectBoot,
        code: &[u8],
        at: u64,
        first_pte: u64,
        second_pte: Option<u64>,
    ) -> LongModeState {
        let mut state = split_code_state(boot, code, code.len(), Some(first_pte));
        if let Some(pte) = second_pte {
            write_phys(boot, TREE_PT + 16, &pte.to_le_bytes());
        }
        write_phys(boot, SPLIT_PAGE1, &[STORE_FILL; 0x1000]);
        write_phys(boot, SPLIT_PAGE2, &[STORE_FILL; 0x1000]);
        state.gpr[Register::Rdi as usize] = STORE_VALUE;
        state.gpr[Register::Rcx as usize] = at - 4;
        state
    }

    fn phys_bytes(boot: &mut LinuxDirectBoot, address: u64, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        read(&mut boot.board, address, &mut bytes).expect("read");
        bytes
    }

    /// What the two data pages hold: the fill with `stored` laid from `at`.
    fn expected_store_pages(at: u64, stored: &[u8]) -> (Vec<u8>, Vec<u8>) {
        let mut a = vec![STORE_FILL; 0x1000];
        let mut b = vec![STORE_FILL; 0x1000];
        for (index, byte) in stored.iter().enumerate() {
            let linear = at + index as u64;
            if linear < STORE_PAGE_B {
                a[(linear - STORE_PAGE_A) as usize] = *byte;
            } else {
                b[(linear - STORE_PAGE_B) as usize] = *byte;
            }
        }
        (a, b)
    }

    const PTE_A: u64 = 0x20;
    const PTE_D: u64 = 0x40;

    #[test]
    fn cpl0_set_board_qword_store_across_a_page_boundary_writes_both_pages_for_every_split() {
        let cli = CliProfile::NonPviLong64V1;
        let bytes = STORE_VALUE.to_le_bytes();
        for on_first in 1..8u64 {
            let (mut boot, _) = boot_with_entry(&[0xF4]);
            let at = STORE_PAGE_B - on_first;
            let mut state = store_state(
                &mut boot,
                &MOV_RCX4_RDI,
                at,
                SPLIT_PAGE1 | 3,
                Some(SPLIT_PAGE2 | 3),
            );
            let start = state.rip;
            let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
            assert_eq!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                "{on_first}: {report:?}"
            );
            assert_eq!(report.instructions_executed, 1, "{on_first}");
            assert_eq!((report.page_fault, report.memory_error), (None, None));
            assert_eq!(state.rip, start + 4);
            // Both pages hold their part, and no byte around the store changed.
            let (page_a, page_b) = expected_store_pages(at, &bytes);
            assert_eq!(
                phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000),
                page_a,
                "{on_first}"
            );
            assert_eq!(
                phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000),
                page_b,
                "{on_first}"
            );
            // Accessed and dirty on the leaf of each page the store touches; the code page is
            // accessed (fetched) and not dirty.
            assert_eq!(read_qword(&mut boot, TREE_PT), SPLIT_PAGE0 | 3 | PTE_A);
            assert_eq!(
                read_qword(&mut boot, TREE_PT + 8),
                SPLIT_PAGE1 | 3 | PTE_A | PTE_D
            );
            assert_eq!(
                read_qword(&mut boot, TREE_PT + 16),
                SPLIT_PAGE2 | 3 | PTE_A | PTE_D
            );
        }
    }

    #[test]
    fn cpl0_set_board_word_and_dword_stores_across_a_page_boundary_on_the_split_tree() {
        let cli = CliProfile::NonPviLong64V1;
        let cases: [(&[u8], usize, &[u64]); 2] =
            [(&MOV_RCX4_DI, 2, &[1]), (&MOV_RCX4_EDI, 4, &[1, 2, 3])];
        for (code, width, cuts) in cases {
            for &on_first in cuts {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let at = STORE_PAGE_B - on_first;
                let mut state =
                    store_state(&mut boot, code, at, SPLIT_PAGE1 | 3, Some(SPLIT_PAGE2 | 3));
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                assert_eq!(
                    report.instructions_executed, 1,
                    "{code:02x?} {on_first}: {report:?}"
                );
                assert_eq!((report.page_fault, report.memory_error), (None, None));
                let (page_a, page_b) =
                    expected_store_pages(at, &STORE_VALUE.to_le_bytes()[..width]);
                assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000), page_a);
                assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000), page_b);
                assert_eq!(
                    read_qword(&mut boot, TREE_PT + 8),
                    SPLIT_PAGE1 | 3 | PTE_A | PTE_D
                );
                assert_eq!(
                    read_qword(&mut boot, TREE_PT + 16),
                    SPLIT_PAGE2 | 3 | PTE_A | PTE_D
                );
            }
        }
    }

    #[test]
    fn cpl0_set_board_store_inside_one_page_sets_dirty_only_on_that_leaf() {
        let cli = CliProfile::NonPviLong64V1;
        // A qword ending exactly on the page end and one in the middle: one page each, and
        // the next (absent) page is not walked.
        for at in [STORE_PAGE_B - 8, STORE_PAGE_A + 0x100] {
            let (mut boot, _) = boot_with_entry(&[0xF4]);
            let mut state = store_state(&mut boot, &MOV_RCX4_RDI, at, SPLIT_PAGE1 | 3, None);
            let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
            assert_eq!(report.instructions_executed, 1, "{at:#x}: {report:?}");
            let (page_a, page_b) = expected_store_pages(at, &STORE_VALUE.to_le_bytes());
            assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000), page_a);
            assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000), page_b);
            assert_eq!(
                read_qword(&mut boot, TREE_PT + 8),
                SPLIT_PAGE1 | 3 | PTE_A | PTE_D
            );
            assert_eq!(read_qword(&mut boot, TREE_PT + 16), 0);
        }
    }

    #[test]
    fn cpl0_set_board_store_with_a_missing_second_page_writes_no_byte_and_faults_precisely() {
        let cli = CliProfile::NonPviLong64V1;
        for (code, width) in [
            (&MOV_RCX4_RDI[..], 8u64),
            (&MOV_RCX4_DI[..], 2),
            (&MOV_RCX4_EDI[..], 4),
        ] {
            for on_first in 1..width {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let at = STORE_PAGE_B - on_first;
                let mut state = store_state(&mut boot, code, at, SPLIT_PAGE1 | 3, None);
                let before = state.clone();
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
                    panic!("{width} {on_first}: {report:?}");
                };
                // Write to a not-present page at CPL0: W = 1, P = 0; CR2 is the first byte on it.
                assert_eq!(
                    (fault.vector, fault.error_code, fault.linear_address),
                    (14, Some(2), Some(STORE_PAGE_B)),
                    "{width} {on_first}"
                );
                assert_eq!(report.instructions_executed, 0);
                assert_eq!(report.memory_error, None);
                let record = report.page_fault.expect("the walker's record");
                assert_eq!(record.saved_rip(), before.rip);
                assert_eq!(record.access(), od_isa::memory::AccessKind::Write);
                assert_eq!(state, before, "{width} {on_first}: the state is unchanged");
                // No partial store: not one byte reached the first page, nor the second.
                assert_eq!(
                    phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000),
                    vec![STORE_FILL; 0x1000],
                    "{width} {on_first}"
                );
                assert_eq!(
                    phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000),
                    vec![STORE_FILL; 0x1000]
                );
                // The walk of the first page set its accessed bit; no page is dirty.
                assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | PTE_A);
                assert_eq!(read_qword(&mut boot, TREE_PT + 16), 0);
            }
        }
    }

    #[test]
    fn cpl0_set_board_store_to_a_read_only_second_page_is_a_present_write_fault_and_no_byte() {
        let cli = CliProfile::NonPviLong64V1;
        // CR0.WP is set on the board, so a supervisor write to W = 0 faults (P = 1, W = 1).
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = STORE_PAGE_B - 3;
        let mut state = store_state(
            &mut boot,
            &MOV_RCX4_RDI,
            at,
            SPLIT_PAGE1 | 3,
            Some(SPLIT_PAGE2 | 1),
        );
        let before = state.clone();
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(3), Some(STORE_PAGE_B))
        );
        assert_eq!(state, before);
        assert_eq!(
            phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000),
            vec![STORE_FILL; 0x1000]
        );
        assert_eq!(
            phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000),
            vec![STORE_FILL; 0x1000]
        );
        // The walk of the first page set its accessed bit (as with a missing second page); the
        // walk of the read-only second page set its own before the write check refused it; no
        // page is dirty.
        assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | PTE_A);
        assert_eq!(read_qword(&mut boot, TREE_PT + 16), SPLIT_PAGE2 | 1 | PTE_A);
    }

    #[test]
    fn cpl0_set_board_store_with_a_faulting_first_page_reports_the_store_address() {
        let cli = CliProfile::NonPviLong64V1;
        // The first data page is read-only, the second writable: the fault is on the first byte.
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = STORE_PAGE_B - 3;
        let mut state = store_state(
            &mut boot,
            &MOV_RCX4_RDI,
            at,
            SPLIT_PAGE1 | 1,
            Some(SPLIT_PAGE2 | 3),
        );
        let before = state.clone();
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(3), Some(at))
        );
        assert_eq!(state, before);
        assert_eq!(
            phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000),
            vec![STORE_FILL; 0x1000]
        );
        assert_eq!(
            phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000),
            vec![STORE_FILL; 0x1000]
        );
        // The second page was not walked.
        assert_eq!(read_qword(&mut boot, TREE_PT + 16), SPLIT_PAGE2 | 3);
    }

    #[test]
    fn cpl0_set_board_store_to_a_non_executable_second_page_succeeds() {
        // NX does not restrict a write: the store completes and both leaves are dirty.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        assert_ne!(boot.board.efer() & (1 << 11), 0, "the board runs with NXE");
        let at = STORE_PAGE_B - 4;
        let mut state = store_state(
            &mut boot,
            &MOV_RCX4_RDI,
            at,
            SPLIT_PAGE1 | 3 | (1 << 63),
            Some(SPLIT_PAGE2 | 3 | (1 << 63)),
        );
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        let (page_a, page_b) = expected_store_pages(at, &STORE_VALUE.to_le_bytes());
        assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE1, 0x1000), page_a);
        assert_eq!(phys_bytes(&mut boot, SPLIT_PAGE2, 0x1000), page_b);
        assert_eq!(
            read_qword(&mut boot, TREE_PT + 16),
            SPLIT_PAGE2 | 3 | (1 << 63) | PTE_A | PTE_D
        );
    }

    // ---- S4-38L5e: dword and qword reads across a 4 KiB page boundary through the set's adapter ----

    /// `MOV RCX, [RAX]`, the kernel's read at 0x0543_5ca0.
    const MOV_RCX_RAX_QWORD: [u8; 3] = [0x48, 0x8B, 0x08];
    /// `MOV ECX, [RAX]`.
    const MOV_ECX_RAX_DWORD: [u8; 2] = [0x8B, 0x08];

    /// The bytes of the two data pages (A then B) a read test lays down: all different.
    fn read_pattern() -> Vec<u8> {
        (0..0x2000usize)
            .map(|index| (index * 7 + 3) as u8 ^ 0x5A)
            .collect()
    }

    /// The split tree with the read at the end of the code page, the first data page mapped with
    /// `first_pte` and the second with `second_pte` (absent when `None`), both holding
    /// `read_pattern()`, RAX = `at` and RCX = `MOVZX_SEED`.
    fn read_state(
        boot: &mut LinuxDirectBoot,
        code: &[u8],
        at: u64,
        first_pte: u64,
        second_pte: Option<u64>,
    ) -> LongModeState {
        let mut state = split_code_state(boot, code, code.len(), Some(first_pte));
        if let Some(pte) = second_pte {
            write_phys(boot, TREE_PT + 16, &pte.to_le_bytes());
        }
        let pattern = read_pattern();
        write_phys(boot, SPLIT_PAGE1, &pattern[..0x1000]);
        write_phys(boot, SPLIT_PAGE2, &pattern[0x1000..]);
        state.gpr[Register::Rax as usize] = at;
        state.gpr[Register::Rcx as usize] = MOVZX_SEED;
        state
    }

    /// The value the little-endian read of `width` bytes at `at` must give.
    fn expected_read(at: u64, width: usize) -> u64 {
        let pattern = read_pattern();
        let start = (at - STORE_PAGE_A) as usize;
        let mut bytes = [0u8; 8];
        bytes[..width].copy_from_slice(&pattern[start..start + width]);
        u64::from_le_bytes(bytes)
    }

    fn assert_read_pages_unchanged(boot: &mut LinuxDirectBoot) {
        let pattern = read_pattern();
        assert_eq!(phys_bytes(boot, SPLIT_PAGE1, 0x1000), pattern[..0x1000]);
        assert_eq!(phys_bytes(boot, SPLIT_PAGE2, 0x1000), pattern[0x1000..]);
    }

    #[test]
    fn cpl0_set_board_qword_and_dword_reads_across_a_page_boundary_for_every_split() {
        let cli = CliProfile::NonPviLong64V1;
        let cases: [(&[u8], usize); 2] = [(&MOV_RCX_RAX_QWORD, 8), (&MOV_ECX_RAX_DWORD, 4)];
        for (code, width) in cases {
            for on_first in 1..width as u64 {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let at = STORE_PAGE_B - on_first;
                let mut state =
                    read_state(&mut boot, code, at, SPLIT_PAGE1 | 3, Some(SPLIT_PAGE2 | 3));
                let start = state.rip;
                let rflags = state.rflags;
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                assert_eq!(
                    report.exit,
                    PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                    "{width} {on_first}: {report:?}"
                );
                assert_eq!(report.instructions_executed, 1, "{width} {on_first}");
                assert_eq!((report.page_fault, report.memory_error), (None, None));
                // The lowest byte is the one at the operand address; a dword zero-extends.
                assert_eq!(
                    state.gpr[Register::Rcx as usize],
                    expected_read(at, width),
                    "{width} {on_first}"
                );
                assert_eq!(state.gpr[Register::Rax as usize], at);
                assert_eq!(
                    (state.rip, state.rflags),
                    (start + code.len() as u64, rflags)
                );
                // A read: accessed on the leaf of each page it touches, never dirty, and no
                // byte of either page changed.
                assert_eq!(read_qword(&mut boot, TREE_PT), SPLIT_PAGE0 | 3 | PTE_A);
                assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | PTE_A);
                assert_eq!(read_qword(&mut boot, TREE_PT + 16), SPLIT_PAGE2 | 3 | PTE_A);
                assert_read_pages_unchanged(&mut boot);
            }
        }
    }

    #[test]
    fn cpl0_set_board_qword_and_dword_reads_inside_one_page_walk_that_page_only() {
        let cli = CliProfile::NonPviLong64V1;
        let cases: [(&[u8], usize); 2] = [(&MOV_RCX_RAX_QWORD, 8), (&MOV_ECX_RAX_DWORD, 4)];
        for (code, width) in cases {
            // One read ending exactly on the page end, one in the middle: one page each, and the
            // next (absent) page is not walked.
            for at in [STORE_PAGE_B - width as u64, STORE_PAGE_A + 0x100] {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let mut state = read_state(&mut boot, code, at, SPLIT_PAGE1 | 3, None);
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                assert_eq!(
                    report.instructions_executed, 1,
                    "{width} {at:#x}: {report:?}"
                );
                assert_eq!(state.gpr[Register::Rcx as usize], expected_read(at, width));
                assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | PTE_A);
                assert_eq!(read_qword(&mut boot, TREE_PT + 16), 0);
            }
        }
    }

    #[test]
    fn cpl0_set_board_cross_page_read_with_a_missing_second_page_is_a_precise_page_fault() {
        let cli = CliProfile::NonPviLong64V1;
        let cases: [(&[u8], u64); 2] = [(&MOV_RCX_RAX_QWORD, 8), (&MOV_ECX_RAX_DWORD, 4)];
        for (code, width) in cases {
            for on_first in 1..width {
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let at = STORE_PAGE_B - on_first;
                let mut state = read_state(&mut boot, code, at, SPLIT_PAGE1 | 3, None);
                let before = state.clone();
                let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
                let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
                    panic!("{width} {on_first}: {report:?}");
                };
                // Read of a not-present page at CPL0: P = 0, W = 0, U = 0; CR2 is the first byte
                // on the missing page.
                assert_eq!(
                    (fault.vector, fault.error_code, fault.linear_address),
                    (14, Some(0), Some(STORE_PAGE_B)),
                    "{width} {on_first}"
                );
                assert_eq!(report.instructions_executed, 0);
                assert_eq!(report.memory_error, None);
                let record = report.page_fault.expect("the walker's record");
                assert_eq!(record.saved_rip(), before.rip);
                assert_eq!(record.access(), od_isa::memory::AccessKind::Read);
                // Nothing retired: RCX (the destination), RIP and RFLAGS are as before.
                assert_eq!(state, before, "{width} {on_first}: the state is unchanged");
                assert_read_pages_unchanged(&mut boot);
                // The walk of the first page set its accessed bit; no page is dirty.
                assert_eq!(read_qword(&mut boot, TREE_PT + 8), SPLIT_PAGE1 | 3 | PTE_A);
                assert_eq!(read_qword(&mut boot, TREE_PT + 16), 0);
            }
        }
    }

    #[test]
    fn cpl0_set_board_cross_page_read_with_a_faulting_first_page_reports_the_read_address() {
        let cli = CliProfile::NonPviLong64V1;
        // The first data page is not present, the second is: CR2 is the operand address.
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let at = STORE_PAGE_B - 3;
        let mut state = read_state(
            &mut boot,
            &MOV_RCX_RAX_QWORD,
            at,
            SPLIT_PAGE1 & !1,
            Some(SPLIT_PAGE2 | 3),
        );
        let before = state.clone();
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0), Some(at))
        );
        assert_eq!(state, before);
        // The second page was not walked.
        assert_eq!(read_qword(&mut boot, TREE_PT + 16), SPLIT_PAGE2 | 3);
    }

    #[test]
    fn cpl0_set_board_cross_page_read_of_read_only_non_executable_pages_succeeds() {
        // Neither W = 0 nor NX restricts a read: it completes, both leaves get only Accessed.
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        assert_ne!(boot.board.efer() & (1 << 11), 0, "the board runs with NXE");
        let at = STORE_PAGE_B - 3;
        let nx = 1u64 << 63;
        let mut state = read_state(
            &mut boot,
            &MOV_RCX_RAX_QWORD,
            at,
            SPLIT_PAGE1 | 1 | nx,
            Some(SPLIT_PAGE2 | 1 | nx),
        );
        let report = run_split(&mut boot, &mut state, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rcx as usize], expected_read(at, 8));
        assert_eq!(
            read_qword(&mut boot, TREE_PT + 8),
            SPLIT_PAGE1 | 1 | nx | PTE_A
        );
        assert_eq!(
            read_qword(&mut boot, TREE_PT + 16),
            SPLIT_PAGE2 | 1 | nx | PTE_A
        );
        assert_read_pages_unchanged(&mut boot);
    }

    // ---- S4-38L4g: `F3 0F BC /r` (REP BSF) as BSF through the set's facade ----

    #[test]
    fn cpl0_set_board_rep_bsf_executes_as_bsf_on_the_board() {
        const ZF: u64 = 1 << 6;
        let cli = CliProfile::NonPviLong64V1;
        // The kernel's `f3 0f bc c0` (REP BSF EAX, EAX) with a non-zero source.
        let (mut boot, _) = boot_with_entry(&[0xF3, 0x0F, 0xBC, 0xC0]);
        boot.board.state_mut().gpr[Register::Rax as usize] = 0xAAAA_BBBB_0001_0000;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 1);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
        assert_eq!(state.gpr[Register::Rax as usize], 16);
        assert_eq!(state.rip, ENTRY + 4);
        assert_eq!(state.rflags & ZF, 0);
        // A zero source sets ZF and leaves the destination alone, exactly like plain BSF.
        let (mut boot, _) = boot_with_entry(&[0xF3, 0x0F, 0xBC, 0xC0]);
        boot.board.state_mut().gpr[Register::Rax as usize] = 0xAAAA_BBBB_0000_0000;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.gpr[Register::Rax as usize], 0xAAAA_BBBB_0000_0000);
        assert_eq!(state.rip, ENTRY + 4);
        assert_ne!(state.rflags & ZF, 0);
    }

    use crate::paged_memory_adapter::PagedCpl0ExceptionFailure;
    use od_isa::{ArchitecturalFault, MemoryError};

    // ---- S4-38L5c: a guest #PF is delivered through the guest IDT and the handler returns ----

    /// The page the faulting store hits: 256 MiB, past the identity map (128 MiB), so the walk
    /// finds a not-present PDE and the supervisor write is the `#PF` with error code 2.
    const PF_VA: u64 = 256 * MIB;
    const PF_IDT_AT: u64 = DATA + 0x2000;
    const PF_HANDLER_AT: u64 = ENTRY + 0x100;
    /// `MOV [RBX], CL` (the handler reads CR2 into RAX, so the stored byte is in CL).
    const PF_STORE: [u8; 2] = [0x88, 0x0B];

    /// The handler of the delivered `#PF`: maps the 2 MiB page of the fault onto `DATA`'s
    /// 2 MiB frame (a PDE store), reads CR2 into RAX, drops the error code and returns.
    fn pf_repair_handler(pde_address: u64, pde: u64) -> Vec<u8> {
        let mut code = vec![0x48, 0xC7, 0xC2]; // MOV RDX, imm32
        code.extend_from_slice(&(pde_address as u32).to_le_bytes());
        code.extend_from_slice(&[0x48, 0xC7, 0xC6]); // MOV RSI, imm32
        code.extend_from_slice(&(pde as u32).to_le_bytes());
        code.extend_from_slice(&[0x48, 0x89, 0x32]); // MOV [RDX], RSI
        code.extend_from_slice(&[0x0F, 0x20, 0xD0]); // MOV RAX, CR2
        code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x08]); // ADD RSP, 8
        code.extend_from_slice(&[0x48, 0xCF]); // IRETQ
        code
    }

    /// A board whose entry is the faulting store. The IDT (gate 14: interrupt gate, CS 0x10, the
    /// handler) is loaded; RBX points into the unmapped page and CL holds `value`.
    fn pf_board(value: u8, handler: &[u8]) -> (LinuxDirectBoot, Vec<u8>) {
        let (mut boot, file) = boot_with_entry(&PF_STORE);
        if !handler.is_empty() {
            write_phys(&mut boot, PF_HANDLER_AT, handler);
        }
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(PF_HANDLER_AT as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((PF_HANDLER_AT >> 16) as u16).to_le_bytes());
        write_phys(&mut boot, PF_IDT_AT + 14 * 16, &gate);
        let state = boot.board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0xFF,
        });
        state.gpr[Register::Rcx as usize] = u64::from(value);
        state.gpr[Register::Rbx as usize] = PF_VA + 0x20;
        state.rflags = 2 | (1 << 9) | 1;
        (boot, file)
    }

    fn pf_pde_address(boot: &LinuxDirectBoot) -> u64 {
        // PD of the first GiB: page 2 of the table zone; entry 128 covers 256 MiB.
        boot.board.cr3() + 2 * 0x1000 + (PF_VA / (2 * MIB)) * 8
    }

    #[test]
    fn cpl0_set_board_delivers_a_page_fault_and_the_handler_restarts_the_store() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = pf_board(0x5A, &[]);
        let pde_address = pf_pde_address(&boot);
        let cr3 = boot.board.cr3();
        let pml4_index = ((PF_VA >> 39) & 0x1ff) as u16;
        let pml4_address = cr3 + u64::from(pml4_index) * 8;
        let pml4_value = read_qword(&mut boot, pml4_address);
        let pdpt_address = (pml4_value & 0x000f_ffff_ffff_f000) + (((PF_VA >> 30) & 0x1ff) * 8);
        let pdpt_value = read_qword(&mut boot, pdpt_address);
        let pd_address = (pdpt_value & 0x000f_ffff_ffff_f000) + (((PF_VA >> 21) & 0x1ff) * 8);
        let pd_value_before = read_qword(&mut boot, pd_address);
        assert_eq!(pd_address, pde_address);
        assert_eq!(pd_value_before, 0, "the original PDE is not present");
        // 2 MiB leaf (P, W, PS) onto the 2 MiB frame that holds `DATA`.
        let handler = pf_repair_handler(pde_address, (DATA & !(2 * MIB - 1)) | 0x83);
        write_phys(&mut boot, PF_HANDLER_AT, &handler);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        let faulting_rbp = 0x5550;
        let faulting_rsi = 0x6666;
        let faulting_rdi = 0x7777;
        let mut fault_frames = Vec::new();
        fault_frames.extend_from_slice(&0x5560u64.to_le_bytes());
        fault_frames.extend_from_slice(&0xffff_ffff_8100_1000u64.to_le_bytes());
        fault_frames.extend_from_slice(&0u64.to_le_bytes());
        fault_frames.extend_from_slice(&0xffff_ffff_8100_2000u64.to_le_bytes());
        write_phys(&mut boot, faulting_rbp, &fault_frames);
        {
            let state = boot.board.state_mut();
            state.gpr[Register::Rbp as usize] = faulting_rbp;
            state.gpr[Register::Rsi as usize] = faulting_rsi;
            state.gpr[Register::Rdi as usize] = faulting_rdi;
        }
        // The store faults, the frame is pushed, six handler instructions retire and IRETQ is
        // the last of them: the store is back at RIP and RF is set (it came from the frame).
        let mut observations = LinuxExecutionObservationCollector::default();
        let (state, report) = run_set_with_observations(
            &mut boot,
            true,
            6,
            &minimal_set(&cli),
            Some(&mut observations),
        );
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 6);
        assert_eq!(report.delivered_exceptions, 1);
        assert_eq!(state.rip, ENTRY, "the faulting instruction is repeated");
        assert_eq!(state.gpr[Register::Rsp as usize], rsp);
        assert_eq!(state.gpr[Register::Rax as usize], PF_VA + 0x20, "CR2 read");
        assert_eq!(state.cr2, Some(PF_VA + 0x20));
        let observations = observations.finish();
        assert_eq!(observations.page_faults_delivered, 1);
        assert_eq!(observations.page_fault_events_omitted, 0);
        let event = &observations.page_fault_events[0];
        assert_eq!(
            (event.rip, event.cr2, event.error_code, event.cpl),
            (ENTRY, PF_VA + 0x20, 2, 0)
        );
        assert_eq!(event.access, "Write");
        let context = &event.faulting_context;
        assert_eq!(
            (context.cr3, context.rsp, context.rbp),
            (cr3, rsp, faulting_rbp)
        );
        assert_eq!(
            (context.rcx, context.rsi, context.rdi),
            (0x5A, faulting_rsi, faulting_rdi)
        );
        assert_eq!(
            context.frame_chain,
            [
                crate::ConsoleContextFrame {
                    frame_pointer: faulting_rbp,
                    return_address: 0xffff_ffff_8100_1000,
                },
                crate::ConsoleContextFrame {
                    frame_pointer: faulting_rbp + 16,
                    return_address: 0xffff_ffff_8100_2000,
                },
            ]
        );
        assert_eq!(
            context.frame_chain_end,
            crate::FrameChainEnd::NullFramePointer
        );
        let page_walk = context.page_walk.as_ref().unwrap();
        assert_eq!(
            page_walk.reason,
            crate::PageFaultReason::NotPresent {
                level: crate::PageTableLevel::Pd,
            }
        );
        assert_eq!(page_walk.entries.len(), 3);
        for (entry, (level, index, address, value)) in page_walk.entries.iter().zip([
            (
                crate::PageTableLevel::Pml4,
                pml4_index,
                pml4_address,
                pml4_value,
            ),
            (
                crate::PageTableLevel::Pdpt,
                ((PF_VA >> 30) & 0x1ff) as u16,
                pdpt_address,
                pdpt_value,
            ),
            (
                crate::PageTableLevel::Pd,
                ((PF_VA >> 21) & 0x1ff) as u16,
                pd_address,
                pd_value_before,
            ),
        ]) {
            assert_eq!(
                (entry.level, entry.index, entry.entry_address.get()),
                (level, index, address)
            );
            if level == crate::PageTableLevel::Pd {
                assert_eq!(entry.entry_value, value);
            } else {
                // Instruction fetch may set A on the shared upper-level path before this data
                // access. The raw trace must otherwise match the pre-run table entries.
                assert_eq!(entry.entry_value & !0x20, value & !0x20);
            }
        }
        assert_eq!(
            read_qword(&mut boot, pde_address),
            (DATA & !(2 * MIB - 1)) | 0x83,
            "the handler repairs the absent PDE after the raw walk was captured"
        );
        assert_eq!(read_qword(&mut boot, faulting_rbp), 0x5560);
        assert_eq!(
            read_qword(&mut boot, faulting_rbp + 8),
            0xffff_ffff_8100_1000
        );
        assert_ne!(
            state.rflags & (1 << 16),
            0,
            "RF from the frame, until the store retires"
        );
        assert_ne!(state.rflags & (1 << 9), 0, "IF back from the frame");
        // The frame the CPU pushed is still on the stack below RSP: error code 2 (not present,
        // write, supervisor), saved RIP, CS, RFLAGS with RF, old RSP, SS.
        let frame = rsp & !15;
        assert_eq!(read_qword(&mut boot, frame - 48), 2);
        assert_eq!(read_qword(&mut boot, frame - 40), ENTRY);
        assert_eq!(read_qword(&mut boot, frame - 32), 0x10);
        assert_eq!(read_qword(&mut boot, frame - 24) & (1 << 16), 1 << 16);
        assert_eq!(read_qword(&mut boot, frame - 16), rsp);
        assert_eq!(read_qword(&mut boot, frame - 8), 0x18);
        // One more instruction: the restarted store retires and clears RF.
        let (mut boot, _) = pf_board(0x5A, &[]);
        write_phys(&mut boot, PF_HANDLER_AT, &handler);
        let (state, report) = run_set(&mut boot, true, 7, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 7, "{report:?}");
        assert_eq!(report.delivered_exceptions, 1);
        assert_eq!(state.rip, ENTRY + 2);
        assert_eq!(
            state.rflags & (1 << 16),
            0,
            "RF cleared after the retired store"
        );
        assert_ne!(state.rflags & (1 << 9), 0);
        // The store reached the frame the handler mapped: DATA + 0x20.
        let mut byte = [0u8; 1];
        read(&mut boot.board, DATA + 0x20, &mut byte).expect("read");
        assert_eq!(byte[0], 0x5A);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
    }

    #[test]
    fn opt_in_execution_observations_attribute_rdtsc_to_exact_instruction_rip() {
        let cli = CliProfile::NonPviLong64V1;
        let code = [0x90, 0x90, 0x0F, 0x31, 0xF4]; // NOP; NOP; RDTSC; HLT
        let (mut boot, _) = boot_with_entry(&code);
        let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
            .with_cli(&cli)
            .with_cpu_owned_cr3(MovCr3Long64V1)
            .with_rdtsc();
        let report = boot
            .board
            .run_linux_boot_cpl0_set_with_post_iret_index(
                4,
                &set,
                false,
                false,
                false,
                true,
                None,
                None,
                1,
                Some(ENTRY + 2),
                None,
                None,
                false,
                false,
                false,
                false,
            )
            .expect("board run");

        assert_eq!(report.run.retired, 3, "{report:?}");
        let observations = report
            .execution_observations
            .expect("opt-in report is present");
        assert_eq!(
            observations.rdtsc_cpl0_sites,
            [crate::RdtscSiteCount {
                rip: ENTRY + 2,
                reads: 1,
            }]
        );
        assert_eq!(observations.rdtsc_cpl0_unattributed_reads, 0);
        assert_eq!(observations.rdtsc_cpl0_site_overflow_reads, 0);
        assert_eq!(observations.rdtsc_entry_sample_hits, 1);
        assert_eq!(observations.rdtsc_entry_clock_trace_missing, 0);
        assert_eq!(observations.rdtsc_entry_clock_trace_samples.len(), 1);
        let clock_read = observations.rdtsc_entry_clock_trace_samples[0];
        assert_eq!(clock_read.hit_ordinal, 1);
        assert_eq!(
            clock_read.now_ns, 0,
            "callback precedes current-quantum charge"
        );
        assert_eq!(clock_read.retired_instructions_charged, 0);
        assert_eq!(clock_read.quantum_instructions_executed, 3);
        assert_eq!(clock_read.retired_instructions_through_rdtsc, 3);
        assert_eq!(
            clock_read.rdtsc_ns_charged, 0,
            "current RDTSC tick is post-callback"
        );
        assert_eq!(clock_read.irqs_delivered, 0);
        assert_eq!(clock_read.unaccounted_elapsed_ns, Some(0));
        assert_eq!(
            clock_read.callback_tsc,
            observations.rdtsc_entry_samples[0].observed_tsc
        );
    }

    #[test]
    fn cpl0_set_board_page_fault_without_an_idt_stays_the_fault_stop() {
        let cli = CliProfile::NonPviLong64V1;
        let (mut boot, _) = pf_board(0x5A, &[0xF4]);
        boot.board.state_mut().idtr = None;
        let before = boot.board.state().clone();
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(2), Some(PF_VA + 0x20))
        );
        assert_eq!(report.delivered_exceptions, 0);
        assert!(report.page_fault.is_some());
        assert_eq!(state.rip, ENTRY);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.cr2, None);
    }

    #[test]
    fn cpl0_set_board_other_faults_are_not_delivered() {
        let cli = CliProfile::NonPviLong64V1;
        // `LOCK MOV CR3, RAX` is #UD: an IDT is loaded, vector 6 is still the stop.
        let (mut boot, _) = pf_board(0, &[0xF4]);
        write_phys(&mut boot, ENTRY, &[0xF0, 0x0F, 0x22, 0xD8]);
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!((fault.vector, fault.error_code), (6, None));
        assert_eq!(report.delivered_exceptions, 0);
        assert_eq!(state.rip, ENTRY);
    }

    // ---- S4-38L9v: a guest #GP(0) of RDMSR is delivered through the guest IDT ----

    const GP_HANDLER_AT: u64 = ENTRY + 0x200;

    /// A board whose entry is `RDMSR; NOP` with ECX = 0x201e (an MSR no member names, the uncore
    /// discovery MSR). The IDT (gate 13: interrupt gate, CS 0x10, `GP_HANDLER_AT`) is loaded.
    fn gp_board(handler: &[u8]) -> LinuxDirectBoot {
        let (mut boot, _) = boot_with_entry(&[0x0F, 0x32, 0x90]);
        write_phys(&mut boot, GP_HANDLER_AT, handler);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(GP_HANDLER_AT as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((GP_HANDLER_AT >> 16) as u16).to_le_bytes());
        write_phys(&mut boot, PF_IDT_AT + 13 * 16, &gate);
        let state = boot.board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0xFF,
        });
        state.gpr[Register::Rcx as usize] = 0x201E;
        state.gpr[Register::Rax as usize] = 0x1111;
        state.gpr[Register::Rdx as usize] = 0x2222;
        state.rflags = 2 | (1 << 9) | 1;
        boot
    }

    fn gp_set(cli: &CliProfile) -> Cpl0ProfileSet<'_> {
        minimal_set(cli).with_msr_tdx_keyid_partitioning_read()
    }

    #[test]
    fn cpl0_set_board_delivers_the_gp0_of_an_unmodelled_rdmsr_and_the_fixup_handler_resumes() {
        let cli = CliProfile::NonPviLong64V1;
        // The handler is the shape of the kernel's `rdmsr_safe` fixup: drop the error code, move
        // the saved RIP past the `RDMSR` (`ADD QWORD [RSP], 2`) and `IRETQ`.
        let handler = [
            0x48, 0x83, 0xC4, 0x08, // ADD RSP, 8
            0x48, 0x83, 0x04, 0x24, 0x02, // ADD QWORD [RSP], 2
            0x48, 0xCF, // IRETQ
        ];
        let mut boot = gp_board(&handler);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        // The fault retires nothing; the handler is three instructions; then the NOP after
        // the RDMSR retires.
        let (state, report) = run_set(&mut boot, true, 4, &gp_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 4);
        assert_eq!(report.delivered_exceptions, 1);
        assert_eq!(
            (
                report.delivered_page_faults,
                report.delivered_general_protection_faults
            ),
            (0, 1)
        );
        assert_eq!(
            state.rip,
            ENTRY + 3,
            "RDMSR skipped by the fixup, NOP retired"
        );
        assert_eq!(state.gpr[Register::Rsp as usize], rsp);
        assert_eq!(
            (
                state.gpr[Register::Rax as usize],
                state.gpr[Register::Rdx as usize]
            ),
            (0x1111, 0x2222),
            "EAX and EDX are not set by the fault"
        );
        assert_eq!(state.cr2, None, "CR2 is not touched by a #GP");
        assert_eq!(
            state.rflags & (1 << 16),
            0,
            "RF cleared after the retired NOP"
        );
        assert_ne!(state.rflags & (1 << 9), 0, "IF back from the frame");
        // The frame: error code 0, the saved RIP (the RDMSR's, `ENTRY`, until the handler's
        // `ADD QWORD [RSP], 2` stored `ENTRY + 2` into it), CS, RFLAGS with RF, old RSP, SS.
        let frame = rsp & !15;
        assert_eq!(read_qword(&mut boot, frame - 48), 0);
        assert_eq!(read_qword(&mut boot, frame - 40), ENTRY + 2);
        assert_eq!(read_qword(&mut boot, frame - 32), 0x10);
        assert_eq!(read_qword(&mut boot, frame - 24) & (1 << 16), 1 << 16);
        assert_eq!(read_qword(&mut boot, frame - 16), rsp);
        assert_eq!(read_qword(&mut boot, frame - 8), 0x18);
        assert_eq!((report.page_fault, report.memory_error), (None, None));
    }

    #[test]
    fn cpl0_set_board_gp0_without_an_idt_stays_the_fault_stop() {
        let cli = CliProfile::NonPviLong64V1;
        let mut boot = gp_board(&[0xF4]);
        boot.board.state_mut().idtr = None;
        let before = boot.board.state().clone();
        let (state, report) = run_set(&mut boot, true, 8, &gp_set(&cli));
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (13, Some(0), None)
        );
        assert_eq!(
            (report.instructions_executed, report.delivered_exceptions),
            (0, 0)
        );
        assert_eq!(state.rip, ENTRY);
        assert_eq!(state.gpr, before.gpr);
    }

    #[test]
    fn cpl0_set_board_gp0_with_a_gate_beyond_the_idt_limit_is_a_fault_stop_with_the_reason() {
        use od_isa::Cpl0GeneralProtectionRefusal as Refusal;
        let cli = CliProfile::NonPviLong64V1;
        let mut boot = gp_board(&[0xF4]);
        boot.board.state_mut().idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 13 * 16 + 14,
        });
        let before = boot.board.state().clone();
        let (state, report) = run_set(&mut boot, true, 8, &gp_set(&cli));
        let PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure } = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!((fault.vector, fault.error_code), (13, Some(0)));
        assert_eq!(
            *failure,
            PagedCpl0ExceptionFailure::GeneralProtectionRefused(Refusal::GateBeyondIdtLimit)
        );
        assert_eq!(
            (report.instructions_executed, report.delivered_exceptions),
            (0, 0)
        );
        assert_eq!(state.rip, ENTRY);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.rflags, before.rflags);
        assert_eq!(state.cr2, None);
    }

    #[test]
    fn cpl0_set_board_gp0_delivery_with_an_unmapped_stack_aborts_and_keeps_the_cpu() {
        let cli = CliProfile::NonPviLong64V1;
        let mut boot = gp_board(&[0xF4]);
        boot.board.state_mut().gpr[Register::Rsp as usize] = PF_VA + 0x1000;
        let before = boot.board.state().clone();
        let (state, report) = run_set(&mut boot, true, 8, &gp_set(&cli));
        let PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure } = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(fault.vector, 13);
        assert!(
            matches!(
                failure,
                PagedCpl0ExceptionFailure::GeneralProtectionAborted {
                    phase: od_isa::Cpl0GeneralProtectionPhase::StackWrite {
                        slot: od_isa::PageFaultFrameSlot::OldSs
                    },
                    cause: MemoryError::Architectural(ArchitecturalFault { vector: 14, .. }),
                }
            ),
            "{failure:?}"
        );
        assert_eq!(report.delivered_exceptions, 0);
        assert_eq!(state.rip, ENTRY);
        assert_eq!(state.gpr, before.gpr);
        assert_eq!(state.cr2, None);
    }

    #[test]
    fn cpl0_set_board_rdmsr_of_a_member_index_or_without_a_msr_file_is_not_delivered() {
        let cli = CliProfile::NonPviLong64V1;
        // A set without any MSR member: Unsupported as before, no delivery.
        let mut boot = gp_board(&[0xF4]);
        let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
        assert_eq!(
            report.exit,
            PagedCpl0SetExit::Run(RunExit::Unsupported { opcode: 0x32 }),
            "{report:?}"
        );
        assert_eq!(
            (report.instructions_executed, report.delivered_exceptions),
            (0, 0)
        );
        assert_eq!(state.rip, ENTRY);
        // The 0x87 member: zeros, no fault.
        let mut boot = gp_board(&[0xF4]);
        boot.board.state_mut().gpr[Register::Rcx as usize] = 0x87;
        let (state, report) = run_set(&mut boot, true, 2, &gp_set(&cli));
        assert_eq!(report.instructions_executed, 2, "{report:?}");
        assert_eq!(report.delivered_exceptions, 0);
        assert_eq!(state.rip, ENTRY + 3);
        assert_eq!(
            (
                state.gpr[Register::Rax as usize],
                state.gpr[Register::Rdx as usize]
            ),
            (0, 0)
        );
    }

    /// The delivery is refused or aborted: the stop is the original fault with the reason in the
    /// set exit, the CPU state is what the faulting instruction left (unchanged) and CR2 is not
    /// written.
    fn assert_pf_delivery_failed(
        boot: &mut LinuxDirectBoot,
        expected: impl FnOnce(&PagedCpl0ExceptionFailure) -> bool,
        label: &str,
    ) {
        let cli = CliProfile::NonPviLong64V1;
        let before = boot.board.state().clone();
        let (state, report) = run_set(boot, true, 8, &minimal_set(&cli));
        let PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure } = &report.exit else {
            panic!("{label}: {report:?}");
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(2), Some(PF_VA + 0x20)),
            "{label}"
        );
        assert!(expected(failure), "{label}: {failure:?}");
        assert_eq!(report.delivered_exceptions, 0, "{label}");
        assert_eq!(report.instructions_executed, 0, "{label}");
        let record = report.page_fault.as_ref().expect("the original record");
        assert_eq!(record.fault(), fault, "{label}");
        assert_eq!(state.rip, ENTRY, "{label}");
        assert_eq!(state.gpr, before.gpr, "{label}");
        assert_eq!(state.rflags, before.rflags, "{label}");
        assert_eq!(state.cs_selector, before.cs_selector, "{label}");
        assert_eq!(state.cr2, None, "{label}");
    }

    #[test]
    fn cpl0_set_board_page_fault_delivery_that_cannot_complete_keeps_the_fault_stop() {
        use od_isa::Cpl0PageFaultRefusal as Refusal;
        // IST 1: not modeled.
        let (mut boot, _) = pf_board(0, &[0xF4]);
        let mut gate = [0u8; 16];
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[4] = 1;
        gate[5] = 0x8E;
        write_phys(&mut boot, PF_IDT_AT + 14 * 16, &gate);
        assert_pf_delivery_failed(
            &mut boot,
            |failure| {
                matches!(
                    failure,
                    PagedCpl0ExceptionFailure::Refused(Refusal::UnsupportedIst(1))
                )
            },
            "IST",
        );
        // The gate lies beyond the IDT limit.
        let (mut boot, _) = pf_board(0, &[0xF4]);
        boot.board.state_mut().idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0x7F,
        });
        assert_pf_delivery_failed(
            &mut boot,
            |failure| {
                matches!(
                    failure,
                    PagedCpl0ExceptionFailure::Refused(Refusal::GateBeyondIdtLimit)
                )
            },
            "limit",
        );
        // The stack is not mapped: the first frame store faults, nothing is written, the CPU
        // is untouched.
        let (mut boot, _) = pf_board(0, &[0xF4]);
        boot.board.state_mut().gpr[Register::Rsp as usize] = PF_VA + 0x1000;
        assert_pf_delivery_failed(
            &mut boot,
            |failure| {
                matches!(
                    failure,
                    PagedCpl0ExceptionFailure::Aborted {
                        phase: od_isa::Cpl0PageFaultDeliveryPhase::StackWrite {
                            slot: od_isa::PageFaultFrameSlot::OldSs
                        },
                        cause: MemoryError::Architectural(ArchitecturalFault { vector: 14, .. }),
                    }
                )
            },
            "stack",
        );
    }

    #[test]
    fn set_facade_keeps_cr2_between_calls_and_reports_the_delivery() {
        // Call 1 delivers the #PF and retires the handler's first instruction (`MOV RDX, 1`);
        // call 2 reads CR2 in the handler and halts. CR2 must survive the call boundary, and
        // the CPU state keeps no CR2 outside a call.
        let mut handler = vec![0x48, 0xC7, 0xC2, 0x01, 0x00, 0x00, 0x00]; // MOV RDX, 1
        handler.extend_from_slice(&[0x48, 0x0F, 0x20, 0xD0]); // MOV RAX, CR2
        handler.push(0xF4); // HLT
        let (mut boot, file) = pf_board(0x5A, &handler);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let first = run_set_facade(&mut boot, &file, 1, members);
        assert_eq!(first.exit, PagedProbeExit::BudgetExhausted, "{first:?}");
        assert_eq!((first.retired, first.exceptions_delivered), (1, 1));
        assert_eq!(
            (
                first.page_faults_delivered,
                first.general_protection_faults_delivered
            ),
            (1, 0)
        );
        assert_eq!(
            first.rip,
            PF_HANDLER_AT + 7,
            "in the handler, before the CR2 read"
        );
        assert_eq!(first.gpr[Register::Rdx as usize], 1);
        assert_eq!(boot.board.state().cr2, None, "the CPU state keeps no CR2");
        let second = run_set_facade(&mut boot, &file, 8, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!((second.retired, second.exceptions_delivered), (2, 0));
        assert_eq!(
            (
                second.page_faults_delivered,
                second.general_protection_faults_delivered
            ),
            (0, 0)
        );
        assert_eq!(second.gpr[Register::Rax as usize], PF_VA + 0x20);
        assert_eq!(boot.board.state().cr2, None);
    }

    /// S4-38L5d: `MOV RBX, CR2` and `MOV R11, CR2` retire in the handler through the board's
    /// facade. RBX holds the fault address when the handler starts, so it is first overwritten
    /// (`MOV RBX, 1`) to prove the read, not the leftover, supplies the final value.
    #[test]
    fn set_facade_mov_rbx_cr2_reads_the_delivered_cr2() {
        let mut handler = vec![0x48, 0xC7, 0xC3, 0x01, 0x00, 0x00, 0x00]; // MOV RBX, 1
        handler.extend_from_slice(&[0x0F, 0x20, 0xD3]); // MOV RBX, CR2
        handler.extend_from_slice(&[0x41, 0x0F, 0x20, 0xD3]); // MOV R11, CR2
        handler.push(0xF4); // HLT
        let (mut boot, file) = pf_board(0x5A, &handler);
        let members = LinuxCpl0SetMembers {
            halt: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let record = run_set_facade(&mut boot, &file, 8, members);
        assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
        assert_eq!((record.retired, record.exceptions_delivered), (4, 1));
        assert_eq!(record.gpr[Register::Rbx as usize], PF_VA + 0x20);
        assert_eq!(record.gpr[Register::R11 as usize], PF_VA + 0x20);
        assert_eq!(boot.board.state().cr2, None, "the CPU state keeps no CR2");
    }

    #[test]
    fn set_facade_page_fault_without_an_idt_is_the_unchanged_fault_stop() {
        let (mut boot, file) = pf_board(0x5A, &[0xF4]);
        boot.board.state_mut().idtr = None;
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit_kind, "fault", "{record:?}");
        assert_eq!(record.exceptions_delivered, 0);
        assert_eq!(
            record.set_exit_detail.as_deref(),
            Some(
                format!(
                    "Run(Fault(ArchitecturalFault {{ vector: 14, error_code: Some(2), linear_address: Some({}) }}))",
                    PF_VA + 0x20
                )
                .as_str()
            )
        );
    }

    #[test]
    fn set_facade_a_refused_page_fault_delivery_is_a_fault_stop_with_the_reason() {
        let (mut boot, file) = pf_board(0x5A, &[0xF4]);
        boot.board.state_mut().idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0x7F,
        });
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit_kind, "fault", "{record:?}");
        assert_eq!(
            (record.retired, record.rip, record.exceptions_delivered),
            (0, ENTRY, 0)
        );
        let detail = record.set_exit_detail.expect("detail");
        assert!(
            detail.starts_with("ExceptionDeliveryFailed {")
                && detail.contains("Refused(GateBeyondIdtLimit)"),
            "{detail}"
        );
        assert!(record.page_fault.is_some());
    }

    #[test]
    fn cpl0_set_board_a_fault_chain_that_retires_nothing_ends_at_the_repeat_limit() {
        let cli = CliProfile::NonPviLong64V1;
        // The gate points into the unmapped page: each delivery lands on a fetch #PF of its
        // own, which is delivered again, and nothing ever retires.
        let (mut boot, _) = pf_board(0, &[]);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&0x0100u16.to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((PF_VA >> 16) as u16).to_le_bytes());
        write_phys(&mut boot, PF_IDT_AT + 14 * 16, &gate);
        let (state, report) = run_set(&mut boot, true, 1_000_000, &minimal_set(&cli));
        let PagedCpl0SetExit::ExceptionDeliveryFailed { fault, failure } = &report.exit else {
            panic!("{report:?}");
        };
        assert_eq!(*failure, PagedCpl0ExceptionFailure::RepeatLimit);
        assert_eq!(fault.vector, 14);
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(
            report.delivered_exceptions,
            crate::paged_memory_adapter::MAX_DELIVERIES_WITHOUT_RETIREMENT
        );
        assert_eq!(state.rip, PF_VA + 0x100);
    }

    #[test]
    fn cpl0_set_board_rf_stays_when_the_restarted_instruction_is_an_iretq() {
        let cli = CliProfile::NonPviLong64V1;
        // `IRETQ` with RF = 1 live and RF = 1 in its own frame: it loads RF itself, so the
        // board must not clear it after the instruction retires (SDM, from memory).
        let (mut boot, _) = boot_with_entry(&[0x48, 0xCF]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        let target = ENTRY + 0x400;
        for (offset, value) in [
            (0u64, target),
            (8, 0x10),
            (16, 2 | (1 << 16)),
            (24, rsp),
            (32, 0x18),
        ] {
            write_phys(&mut boot, rsp + offset, &value.to_le_bytes());
        }
        let state = boot.board.state_mut();
        state.gpr[Register::Rsp as usize] = rsp;
        state.rflags |= 1 << 16;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rip, target);
        assert_ne!(state.rflags & (1 << 16), 0, "RF loaded by IRETQ stays");
        // The same live RF with a frame that carries RF = 0: IRETQ loads 0.
        let (mut boot, _) = boot_with_entry(&[0x48, 0xCF]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        for (offset, value) in [(0u64, target), (8, 0x10), (16, 2), (24, rsp), (32, 0x18)] {
            write_phys(&mut boot, rsp + offset, &value.to_le_bytes());
        }
        let state = boot.board.state_mut();
        state.gpr[Register::Rsp as usize] = rsp;
        state.rflags |= 1 << 16;
        let (state, report) = run_set(&mut boot, true, 1, &minimal_set(&cli));
        assert_eq!(report.instructions_executed, 1, "{report:?}");
        assert_eq!(state.rflags & (1 << 16), 0);
    }

    #[test]
    fn set_facade_an_iretq_to_cpl3_is_a_user_entry_stop_with_its_frame() {
        // S4-38L9I: the board GDT (base 0x12_0000, limit 31) gets a DPL 3 64-bit code descriptor
        // at 0x10 and a DPL 3 data descriptor at 0x18 (selectors 0x13 and 0x1b); the guest
        // `IRETQ` frame returns to them.
        const GDT: u64 = 0x12_0000;
        let (mut boot, file) = boot_with_entry(&[0x48, 0xCF]);
        write_phys(
            &mut boot,
            GDT + 0x10,
            &0x00af_fb00_0000_ffff_u64.to_le_bytes(),
        );
        write_phys(
            &mut boot,
            GDT + 0x18,
            &0x00cf_f300_0000_ffff_u64.to_le_bytes(),
        );
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        let (user_rip, user_rsp, user_rflags) =
            (0x0000_7fff_0000_1000u64, 0x0000_7fff_ffff_f000u64, 0x202u64);
        for (offset, value) in [
            (0u64, user_rip),
            (8, 0x13),
            (16, user_rflags),
            (24, user_rsp),
            (32, 0x1b),
        ] {
            write_phys(&mut boot, rsp + offset, &value.to_le_bytes());
        }
        boot.board.state_mut().gpr[Register::Rsp as usize] = rsp;
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.exit, PagedProbeExit::Stopped, "{record:?}");
        assert_eq!(record.exit_kind, "user_entry", "{record:?}");
        assert_eq!(
            record.user_entry,
            Some(od_isa::IretqUserEntry {
                rip: user_rip,
                cs: 0x13,
                rflags: user_rflags,
                rsp: user_rsp,
                ss: 0x1b,
            })
        );
        // The IRETQ did not retire: RIP is on it, the stack pointer is the one before.
        assert_eq!((record.retired, record.rip), (0, ENTRY), "{record:?}");
        assert_eq!(record.gpr[Register::Rsp as usize], rsp);
        assert_eq!(record.rip_bytes.get(..2), Some(&[0x48u8, 0xCF][..]));
        assert_eq!(record.cpuid_divergence_leaf, None);
        assert!(
            record
                .set_exit_detail
                .as_deref()
                .is_some_and(|detail| detail.starts_with("UserEntry(")),
            "{record:?}"
        );
        // The same frame to CPL0 selectors is the old same-privilege return, not a stop.
        let (mut boot, file) = boot_with_entry(&[0x48, 0xCF]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        let target = ENTRY + 0x400;
        for (offset, value) in [(0u64, target), (8, 0x10), (16, 2), (24, rsp), (32, 0x18)] {
            write_phys(&mut boot, rsp + offset, &value.to_le_bytes());
        }
        boot.board.state_mut().gpr[Register::Rsp as usize] = rsp;
        let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
        assert_ne!(record.exit_kind, "user_entry", "{record:?}");
        assert_eq!(record.user_entry, None);
        assert_eq!(record.retired, 1);
    }

    // ---- S4-39c: the board follows the CPL of the state under the user mode ----

    const USER_GDT: u64 = 0x12_0000;
    const USER_CODE_AT: u64 = ENTRY + 0x400;

    /// A board whose entry is an `IRETQ` to CPL3 (CS 0x13, SS 0x1b, DPL 3 descriptors in the
    /// board GDT) onto `user_code` at `USER_CODE_AT`. With `user_page` the page-table entries
    /// that map it (PML4E, PDPTE and the 2 MiB PDE of the first GiB) get the user bit; without
    /// it the page stays supervisor-only.
    fn user_mode_board(user_code: &[u8], user_page: bool) -> (LinuxDirectBoot, Vec<u8>) {
        user_mode_board_with(&[], user_code, user_page, 0x202)
    }

    /// `user_mode_board` with `prefix` (CPL0 code that runs before the `IRETQ`, no stack effect)
    /// and the RFLAGS of the `IRETQ` frame (`0x202`: IF set; `0x2`: IF clear in CPL3).
    fn user_mode_board_with(
        prefix: &[u8],
        user_code: &[u8],
        user_page: bool,
        user_rflags: u64,
    ) -> (LinuxDirectBoot, Vec<u8>) {
        let entry: Vec<u8> = prefix.iter().copied().chain([0x48, 0xCF]).collect();
        let (mut boot, file) = boot_with_entry(&entry);
        configure_user_mode_entry(&mut boot, user_code, user_page, user_rflags);
        (boot, file)
    }

    fn configure_user_mode_entry(
        boot: &mut LinuxDirectBoot,
        user_code: &[u8],
        user_page: bool,
        user_rflags: u64,
    ) {
        write_phys(
            boot,
            USER_GDT + 0x10,
            &0x00af_fb00_0000_ffff_u64.to_le_bytes(),
        );
        write_phys(
            boot,
            USER_GDT + 0x18,
            &0x00cf_f300_0000_ffff_u64.to_le_bytes(),
        );
        write_phys(boot, USER_CODE_AT, user_code);
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        for (offset, value) in [
            (0u64, USER_CODE_AT),
            (8, 0x13),
            (16, user_rflags),
            (24, ENTRY + 0x800),
            (32, 0x1b),
        ] {
            write_phys(boot, rsp + offset, &value.to_le_bytes());
        }
        let state = boot.board.state_mut();
        state.gpr[Register::Rsp as usize] = rsp;
        // As after the kernel's `startup_64` prologue: DS/ES/FS/GS are null, so the commit (which
        // refuses a non-null selector over a cache with DPL < 3) is not stopped by them.
        (state.ds_selector, state.es_selector) = (0, 0);
        (state.fs_selector, state.gs_selector) = (0, 0);
        if user_page {
            let cr3 = boot.board.cr3();
            for entry in [
                cr3,
                cr3 + 0x1000,
                cr3 + 2 * 0x1000 + (USER_CODE_AT / (2 * MIB)) * 8,
            ] {
                let mut raw = [0u8; 8];
                read(&mut boot.board, entry, &mut raw).expect("read entry");
                let value = u64::from_le_bytes(raw) | 4;
                write_phys(boot, entry, &value.to_le_bytes());
            }
        }
    }

    fn run_user_facade(boot: &mut LinuxDirectBoot, file: &[u8], budget: u64) -> LinuxStopRecord {
        run_linux_direct_board_set_user_mode(
            boot,
            &input(file, &[], BLOB, board_hash()),
            budget,
            LinuxCpl0SetMembers::minimal(),
        )
        .expect("runs")
    }

    #[test]
    fn user_mode_facade_continues_from_the_committed_iretq_into_the_cpl3_entry() {
        // `mov eax, 0x53; syscall`: the first form is allowed in CPL3, `SYSCALL` is outside the
        // allow-list (S4-39e) and stops the run as an honest `Unsupported` in CPL3.
        let (mut boot, file) = user_mode_board(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05], true);
        let record = run_user_facade(&mut boot, &file, 8);
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x05 },
            "{record:?}"
        );
        assert_eq!(record.exit_kind, "user_mode_unsupported", "{record:?}");
        assert_eq!(record.unsupported_opcode, Some(0x05));
        assert_eq!(record.cpl, 3);
        // The `IRETQ` retired in CPL0, the `mov` in CPL3; the `syscall` did not retire.
        assert_eq!((record.retired, record.cpl3_retired), (2, 1), "{record:?}");
        assert_eq!(record.rip, USER_CODE_AT + 5);
        assert_eq!(record.gpr[Register::Rax as usize], 0x53);
        assert_eq!(record.gpr[Register::Rsp as usize], ENTRY + 0x800);
        assert!(record.user_mode);
        assert_eq!(record.user_entry, None);
        // S4-39g: a user-mode run of the Linux-profile board also carries the idle wake members.
        assert_eq!(
            record.set_members,
            ["cli", "cpu_owned_cr3", "halt", "irq", "user_mode"]
        );
        assert_eq!(record.path, "cpl0_set");
    }

    #[test]
    fn cpl3_rdtsc_counts_only_the_admitted_form() {
        for (code, reads, opcode) in [
            (&[0x0f, 0x31, 0x0f, 0x05][..], 1, 0x05),
            (&[0x48, 0x0f, 0x31, 0x0f, 0x05][..], 0, 0x31),
        ] {
            let (mut boot, file) = user_mode_board(code, true);
            let members = LinuxCpl0SetMembers {
                rdtsc: true,
                ..LinuxCpl0SetMembers::minimal()
            };
            let record = run_linux_direct_board_set_user_mode(
                &mut boot,
                &input(&file, &[], BLOB, board_hash()),
                8,
                members,
            )
            .expect("runs");
            assert_eq!(record.exit_kind, "user_mode_unsupported", "{record:?}");
            assert_eq!(record.unsupported_opcode, Some(opcode), "{record:?}");
            assert_eq!(record.cpl, 3);
            assert_eq!(record.cpl3_retired, reads);
            assert_eq!(record.rdtsc_reads, Some(reads));
            assert_eq!(record.rdtsc_reads_by_cpl, Some([0, 0, 0, reads]));
        }
    }

    const SYSCALL_KERNEL_AT: u64 = ENTRY + 0x600;

    /// S4-39e: the `msr` (EFER cell), STAR, LSTAR and SYSCALL_MASK members that give a user-mode
    /// set the `syscall` member.
    fn syscall_members() -> LinuxCpl0SetMembers {
        LinuxCpl0SetMembers {
            msr: true,
            msr_star: true,
            msr_lstar: true,
            msr_syscall_mask: true,
            ..LinuxCpl0SetMembers::minimal()
        }
    }

    fn run_syscall_facade(
        boot: &mut LinuxDirectBoot,
        file: &[u8],
        budget: u64,
        efer: u64,
    ) -> LinuxStopRecord {
        boot.board.set_guest_efer(efer);
        boot.board.set_guest_star(0x0023_0010_0000_0000);
        boot.board.set_guest_lstar(SYSCALL_KERNEL_AT);
        boot.board.set_guest_syscall_mask(0x25_7fd5);
        run_linux_direct_board_set_user_mode(
            boot,
            &input(file, &[], BLOB, board_hash()),
            budget,
            syscall_members(),
        )
        .expect("runs")
    }

    #[test]
    fn user_mode_facade_syscall_enters_the_kernel_at_lstar_and_recomputes_the_cpl() {
        // `IRETQ` (CPL0), `mov eax, 0x53; syscall` (CPL3), then two NOPs at LSTAR through the CPL0
        // entry: the budget of 5 ends in CPL0 (CS 0x10 from STAR[47:32]).
        let (mut boot, file) = user_mode_board(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05], true);
        write_phys(&mut boot, SYSCALL_KERNEL_AT, &[0x90, 0x90]);
        let record = run_syscall_facade(&mut boot, &file, 5, 0xd01);
        assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
        assert_eq!(record.cpl, 0, "{record:?}");
        assert_eq!((record.retired, record.cpl3_retired), (5, 2), "{record:?}");
        assert_eq!(record.rip, SYSCALL_KERNEL_AT + 2);
        assert_eq!(record.gpr[Register::Rax as usize], 0x53);
        assert_eq!(record.gpr[Register::Rcx as usize], USER_CODE_AT + 7);
        assert_eq!(record.gpr[Register::R11 as usize], 0x202);
        // RSP is the user one: the kernel switches stacks itself.
        assert_eq!(record.gpr[Register::Rsp as usize], ENTRY + 0x800);
        let state = boot.board.state();
        assert_eq!(
            (state.cs_selector, state.ss_selector),
            (Some(0x10), Some(0x18))
        );
        assert_eq!(state.rflags, 2, "IF is in FMASK");
        assert_eq!(
            &record.set_members[record.set_members.len() - 2..],
            ["user_mode", "syscall"]
        );
    }

    /// S4-39f: user `mov eax, 0x53; syscall; mov eax, 7; syscall`; the kernel at LSTAR is
    /// `mov eax, 0x2a; sysretq`, so every call returns 0x2a to the instruction after its `syscall`.
    fn syscall_round_trip_board() -> (LinuxDirectBoot, Vec<u8>) {
        let (mut boot, file) = user_mode_board(
            &[
                0xb8, 0x53, 0, 0, 0, 0x0f, 0x05, 0xb8, 0x07, 0, 0, 0, 0x0f, 0x05,
            ],
            true,
        );
        write_phys(
            &mut boot,
            SYSCALL_KERNEL_AT,
            &[0xb8, 0x2a, 0, 0, 0, 0x48, 0x0f, 0x07],
        );
        (boot, file)
    }

    fn run_threaded_test(
        boot: &mut LinuxDirectBoot,
        file: &[u8],
        budget: u64,
        quantum: u64,
        members: LinuxCpl0SetMembers,
    ) -> Result<LinuxThreadedRunReport, LinuxDirectError> {
        run_linux_direct_board_set_user_mode_threaded(
            boot,
            &input(file, &[], BLOB, board_hash()),
            budget,
            LinuxCpl0SetMembers {
                cpuid_profile: CpuidProfile::X8664V1Stage4A,
                ..members
            },
            LinuxThreadedOptions {
                quantum,
                timeout: std::time::Duration::from_secs(5),
            },
        )
    }

    #[test]
    fn threaded_linux_records_syscalls_and_stops_on_the_unsupported_user_instruction() {
        let (mut boot, file) = syscall_round_trip_board();
        write_phys(&mut boot, USER_CODE_AT + 14, &[0xF1, 0xB8, 1, 0, 0, 0]);
        boot.board.set_guest_efer(0xd01);
        boot.board.set_guest_star(0x0023_0010_0000_0000);
        boot.board.set_guest_lstar(SYSCALL_KERNEL_AT);
        boot.board.set_guest_syscall_mask(0x25_7fd5);
        let report = run_threaded_test(&mut boot, &file, 32, 1, syscall_members()).unwrap();
        assert_eq!((report.threads_started, report.threads_exited), (1, 1));
        let stop = report.stop;
        assert_eq!(stop.exit_kind, "user_mode_unsupported", "{stop:?}");
        assert_eq!(stop.unsupported_opcode, Some(0xf1));
        assert_eq!(
            (stop.retired, stop.cpl3_retired, stop.virtual_ns),
            (9, 4, 9)
        );
        assert_eq!(stop.rip, USER_CODE_AT + 14);
        assert_eq!(stop.rip_bytes.first(), Some(&0xF1));
        assert_eq!(stop.gpr[Register::Rax as usize], 0x2a);
        assert_eq!(
            (stop.syscall_journal.entered, stop.syscall_journal.returned),
            (2, 2)
        );
    }

    #[test]
    fn parallel_linux_observes_syscalls_once_and_correlates_the_return() {
        let file = bzimage_with_entry(&[0x48, 0xcf]); // IRETQ into the guest task
        let input = input(&file, &[], SMP_BLOB, board_hash());
        let mut boot = load_linux_direct_board_with_vcpus(&input, 2).unwrap();
        let user = [0xb8, 0x53, 0, 0, 0, 0x0f, 0x05, 0xf1];
        configure_user_mode_entry(&mut boot, &user, true, 0x202);
        write_phys(
            &mut boot,
            SYSCALL_KERNEL_AT,
            &[0xb8, 0x2a, 0, 0, 0, 0x48, 0x0f, 0x07],
        );
        boot.board.set_guest_efer(0xd01);
        boot.board.set_guest_star(0x0023_0010_0000_0000);
        boot.board.set_guest_lstar(SYSCALL_KERNEL_AT);
        boot.board.set_guest_syscall_mask(0x25_7fd5);
        let report = run_linux_direct_board_set_user_mode_parallel(
            &mut boot,
            &input,
            32,
            LinuxCpl0SetMembers {
                cpuid_profile: CpuidProfile::X8664V1Stage4AApic,
                ..syscall_members()
            },
            LinuxThreadedOptions {
                quantum: 16,
                timeout: std::time::Duration::from_secs(5),
            },
        )
        .unwrap();
        assert_eq!(report.terminal_vcpu, Some(0));
        let journal = report.stop.syscall_journal;
        assert_eq!((journal.entered, journal.returned), (1, 1));
        assert_eq!(journal.entries[0].rax_return, Some(0x2a));
    }

    #[test]
    fn threaded_linux_reports_the_new_cr3_and_reuses_it_on_the_next_run() {
        let (mut boot, file) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        let old_cr3 = boot.board.cr3();
        let members = LinuxCpl0SetMembers {
            cpu_owned_cr3: true,
            fetch_cache: true,
            ..LinuxCpl0SetMembers::minimal()
        };
        let first = run_threaded_test(&mut boot, &file, 3, 2, members)
            .unwrap()
            .stop;
        assert_eq!(first.exit, PagedProbeExit::BudgetExhausted);
        assert_eq!(first.cr3, TREE_PML4);
        assert_eq!(first.rip_gpa, Some(TREE_CODE + 0x20B));
        assert_eq!(&first.rip_bytes[..7], &[0x48, 0xC7, 0xC3, 0x22, 0x22, 0, 0]);
        assert_eq!(first.last_cr3_change, Some((old_cr3, TREE_PML4)));
        let second = run_threaded_test(&mut boot, &file, 2, 2, members)
            .unwrap()
            .stop;
        assert_eq!(second.retired, 2);
        assert_eq!(second.cr3, TREE_PML4);
        assert_eq!(second.gpr[Register::Rbx as usize], 0x2222);
    }

    #[test]
    fn threaded_linux_reports_a_port_failure_after_charging_the_completed_prefix() {
        // MOV EDX, 0xCFD; IN EAX, DX. The PCI device refuses a misaligned dword input.
        let (mut boot, file) = boot_with_entry(&[0xBA, 0xFD, 0x0C, 0, 0, 0xED]);
        let error = run_threaded_test(
            &mut boot,
            &file,
            8,
            8,
            LinuxCpl0SetMembers {
                pci_config: true,
                ..LinuxCpl0SetMembers::minimal()
            },
        )
        .unwrap_err();
        assert_eq!((error.code, error.retired), ("run_failed", Some(1)));
        assert!(error.detail.contains("Bus("), "{error:?}");
        assert_eq!(boot.board.state().rip, ENTRY + 5);
        assert_eq!(boot.board.clock().now_ns(), 1);
    }

    #[test]
    fn user_mode_facade_sysretq_returns_to_cpl3_and_the_journal_records_both_ends() {
        // `IRETQ`, then two rounds of user `mov`, `syscall`, kernel `mov`, `sysretq`: 1 + 2 * 4 = 9.
        let (mut boot, file) = syscall_round_trip_board();
        let record = run_syscall_facade(&mut boot, &file, 9, 0xd01);
        assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
        assert_eq!((record.retired, record.cpl3_retired, record.cpl), (9, 4, 3));
        // Back at the user instruction after the second `syscall`, RSP untouched, RAX the result,
        // RFLAGS the ones SYSCALL saved in R11 (IF).
        assert_eq!(record.rip, USER_CODE_AT + 14);
        assert_eq!(record.gpr[Register::Rax as usize], 0x2a);
        assert_eq!(record.gpr[Register::Rsp as usize], ENTRY + 0x800);
        assert_eq!(record.rflags, 0x202);
        let state = boot.board.state();
        assert_eq!(
            (state.cs_selector, state.ss_selector),
            (Some(0x33), Some(0x2b))
        );
        let journal = &record.syscall_journal;
        assert_eq!((journal.entered, journal.returned), (2, 2));
        assert_eq!(
            journal.entries.iter().copied().collect::<Vec<_>>(),
            [
                SyscallJournalEntry {
                    nr: 0x53,
                    rip: USER_CODE_AT + 7,
                    rax_return: Some(0x2a),
                },
                SyscallJournalEntry {
                    nr: 7,
                    rip: USER_CODE_AT + 14,
                    rax_return: Some(0x2a),
                },
            ]
        );
        // A run that stops inside the second call has the entry without a result.
        let (mut boot, file) = syscall_round_trip_board();
        let record = run_syscall_facade(&mut boot, &file, 8, 0xd01);
        assert_eq!(record.cpl, 0, "{record:?}");
        let journal = &record.syscall_journal;
        assert_eq!((journal.entered, journal.returned), (2, 1));
        assert_eq!(journal.entries.len(), 2);
        assert_eq!(journal.entries[1].nr, 7);
        assert_eq!(journal.entries[1].rax_return, None);
        // Without the user mode there is no journal.
        let (mut boot, file) = boot_with_entry(&[0x90]);
        let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
        assert_eq!(record.syscall_journal, SyscallJournal::default());
    }

    #[test]
    fn syscall_journal_is_a_ring_and_an_iretq_return_is_not_a_system_call() {
        let mut journal = SyscallJournal::default();
        for nr in 0..(SYSCALL_JOURNAL_CAPACITY as u64 + 3) {
            let caller = crate::paged_memory_adapter::SyscallUserContext {
                rip: nr + 1,
                rsp: 0,
                cr3: 0,
            };
            journal.record(crate::paged_memory_adapter::SyscallEvent::Enter { nr, caller });
            journal.record(crate::paged_memory_adapter::SyscallEvent::Exit {
                rax: nr + 2,
                caller,
            });
        }
        assert_eq!(journal.entries.len(), SYSCALL_JOURNAL_CAPACITY);
        assert_eq!(journal.entered, SYSCALL_JOURNAL_CAPACITY as u64 + 3);
        assert_eq!(journal.returned, journal.entered);
        assert_eq!(journal.entries[0].nr, 3);
        assert_eq!(journal.entries[0].rax_return, Some(5));
        // The committed `IRETQ` into CPL3 of a user-mode run (no `SYSRETQ`) leaves no entry.
        let (mut boot, file) = user_mode_board(&[0xb8, 0x53, 0, 0, 0], true);
        let record = run_syscall_facade(&mut boot, &file, 2, 0xd01);
        assert_eq!(record.cpl3_retired, 1);
        assert_eq!(record.syscall_journal, SyscallJournal::default());
    }

    #[test]
    fn user_mode_facade_syscall_with_sce_clear_is_a_ud_stop_in_cpl3() {
        let (mut boot, file) = user_mode_board(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05], true);
        let record = run_syscall_facade(&mut boot, &file, 8, 0xd00);
        assert_eq!(record.exit_kind, "user_mode_fault", "{record:?}");
        assert!(record.exit_detail.contains("vector: 6"), "{record:?}");
        assert_eq!(record.cpl, 3);
        assert_eq!((record.retired, record.cpl3_retired), (2, 1), "{record:?}");
        assert_eq!(record.rip, USER_CODE_AT + 5);
        assert_eq!(boot.board.state().cs_selector, Some(0x13));
    }

    #[test]
    fn user_mode_facade_syscall_member_needs_the_four_cells() {
        // The stage4a-less minimal members have no cells: `SYSCALL` stays the honest stop.
        let (mut boot, file) = user_mode_board(&[0x0f, 0x05], true);
        let record = run_user_facade(&mut boot, &file, 8);
        assert_eq!(record.exit_kind, "user_mode_unsupported", "{record:?}");
        assert!(!record.set_members.iter().any(|name| name == "syscall"));
    }

    #[test]
    fn user_mode_facade_budget_and_same_privilege_returns() {
        // The budget counts the CPL3 instruction too.
        let (mut boot, file) = user_mode_board(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05], true);
        let record = run_user_facade(&mut boot, &file, 2);
        assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
        assert_eq!((record.retired, record.cpl3_retired, record.cpl), (2, 1, 3));
        // A return to CPL0 selectors under the user mode never touches the CPL3 entry.
        let (mut boot, file) = boot_with_entry(&[0x48, 0xCF]);
        let rsp = boot.board.state().gpr[Register::Rsp as usize] - 0x100;
        let target = ENTRY + 0x400;
        for (offset, value) in [(0u64, target), (8, 0x10), (16, 2), (24, rsp), (32, 0x18)] {
            write_phys(&mut boot, rsp + offset, &value.to_le_bytes());
        }
        boot.board.state_mut().gpr[Register::Rsp as usize] = rsp;
        let record = run_user_facade(&mut boot, &file, 1);
        assert_eq!((record.retired, record.cpl3_retired, record.cpl), (1, 0, 0));
        assert!(record.user_mode);
        assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
    }

    const USER_IDT_AT: u64 = DATA + 0x2000;
    const USER_TSS_AT: u64 = DATA + 0x3000;
    const USER_HANDLER_AT: u64 = ENTRY + 0x100;
    /// `TSS.RSP0` of the S4-39d boards: not 16-aligned, so that the alignment shows.
    const USER_RSP0: u64 = ENTRY + 0x1008;

    /// The PDE of the 2 MiB page of `USER_CODE_AT` in the first GiB.
    fn user_code_pde(boot: &LinuxDirectBoot) -> u64 {
        boot.board.cr3() + 2 * 0x1000 + (USER_CODE_AT / (2 * MIB)) * 8
    }

    /// S4-39d: the board of `user_mode_board` with a kernel code descriptor at GDT 0x08, an IDT whose
    /// gate 14 is an interrupt gate to `handler` at `USER_HANDLER_AT` through selector 0x08 and (with
    /// `tss`) a task register over a TSS whose RSP0 is `USER_RSP0`. The user code page is
    /// supervisor-only at the PDE (PML4E and PDPTE are user), so the first user fetch is a `#PF`.
    /// `make_handler` gets the original PDE value and returns the handler bytes.
    fn user_pf_board(
        user_code: &[u8],
        make_handler: &dyn Fn(u64, u64) -> Vec<u8>,
        tss: bool,
    ) -> (LinuxDirectBoot, Vec<u8>) {
        let (mut boot, file) = user_mode_board(user_code, true);
        let pde = user_code_pde(&boot);
        let mut raw = [0u8; 8];
        read(&mut boot.board, pde, &mut raw).expect("read pde");
        let user_pde = u64::from_le_bytes(raw);
        write_phys(&mut boot, pde, &(user_pde & !4).to_le_bytes());
        write_phys(
            &mut boot,
            USER_GDT + 0x08,
            &0x00af_9b00_0000_ffff_u64.to_le_bytes(),
        );
        write_phys(&mut boot, USER_HANDLER_AT, &make_handler(pde, user_pde));
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(USER_HANDLER_AT as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((USER_HANDLER_AT >> 16) as u16).to_le_bytes());
        write_phys(&mut boot, USER_IDT_AT + 14 * 16, &gate);
        write_phys(&mut boot, USER_TSS_AT + 4, &USER_RSP0.to_le_bytes());
        let state = boot.board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: USER_IDT_AT,
            limit: 0xFF,
        });
        if tss {
            state.tr = Some(od_isa::TaskRegisterCache {
                selector: 0x20,
                base: USER_TSS_AT,
                limit: 0x67,
                access: 0x8B,
                flags: 0,
            });
        }
        (boot, file)
    }

    #[test]
    fn user_mode_facade_without_a_task_register_a_cpl3_page_fault_is_not_delivered() {
        // S4-39d: the delivery needs `TSS.RSP0`; with no modeled task register it is the CPU
        // delivery's refusal (`MissingTaskRegister`), the fault stays the stop it was.
        let (mut boot, file) =
            user_pf_board(&[0xb8, 0x53, 0, 0, 0], &|_, _| vec![0x90, 0x90], false);
        let record = run_user_facade(&mut boot, &file, 8);
        assert_eq!(record.exit_kind, "user_mode_fault", "{record:?}");
        assert_eq!((record.retired, record.cpl3_retired, record.cpl), (1, 0, 3));
        assert_eq!(record.rip, USER_CODE_AT);
        assert_eq!(record.exceptions_delivered, 0);
        let detail = record.set_exit_detail.as_deref().expect("detail");
        assert!(
            detail.starts_with("ExceptionDeliveryFailed {")
                && detail.contains("MissingTaskRegister"),
            "{detail}"
        );
        assert!(record.exit_detail.contains("vector: 14"), "{record:?}");
        assert!(record.page_fault.is_some());
    }

    #[test]
    fn user_mode_facade_delivers_a_cpl3_page_fault_to_the_cpl0_handler_and_recomputes_the_cpl() {
        // `IRETQ` to CPL3 (retired in CPL0), the user fetch faults, the frame goes to the
        // TSS.RSP0 stack and the handler's NOPs retire through the CPL0 entry: the budget of 3
        // ends in CPL0 (the CPL was computed again from the new CS).
        let (mut boot, file) =
            user_pf_board(&[0xb8, 0x53, 0, 0, 0], &|_, _| vec![0x90, 0x90, 0x90], true);
        let record = run_user_facade(&mut boot, &file, 3);
        assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
        assert_eq!(record.exceptions_delivered, 1, "{record:?}");
        assert_eq!(record.cpl, 0, "{record:?}");
        assert_eq!((record.retired, record.cpl3_retired), (3, 0), "{record:?}");
        assert_eq!(record.rip, USER_HANDLER_AT + 2);
        let frame = USER_RSP0 & !15;
        assert_eq!(record.gpr[Register::Rsp as usize], frame - 48);
        let words = [
            (frame - 8, 0x1b),           // SS
            (frame - 16, ENTRY + 0x800), // RSP
            (frame - 32, 0x13),          // CS
            (frame - 40, USER_CODE_AT),  // RIP
            (frame - 48, 21),            // error code: P | U | I/D
        ];
        for (address, expected) in words {
            assert_eq!(read_qword(&mut boot, address), expected, "{address:#x}");
        }
        let rflags = read_qword(&mut boot, frame - 24);
        assert_eq!(
            rflags & ((1 << 9) | (1 << 16) | 2),
            (1 << 9) | (1 << 16) | 2
        );
        let state = boot.board.state();
        assert_eq!(state.cs_selector, Some(0x08));
        assert_eq!((state.ss_selector, state.ss_unusable), (Some(0), true));
        assert_eq!(state.rflags & (1 << 9), 0, "interrupt gate clears IF");
    }

    #[test]
    fn execution_observation_keeps_cpl3_registers_from_before_page_fault_delivery() {
        let (mut boot, _) = user_pf_board(&[0x90], &|_, _| vec![0x90, 0x90, 0x90], true);
        let faulting_rbp = 0x5555;
        let faulting_rcx = 0x6666;
        let faulting_rsi = 0x7777;
        let faulting_rdi = 0x8888;
        {
            let state = boot.board.state_mut();
            state.gpr[Register::Rbp as usize] = faulting_rbp;
            state.gpr[Register::Rcx as usize] = faulting_rcx;
            state.gpr[Register::Rsi as usize] = faulting_rsi;
            state.gpr[Register::Rdi as usize] = faulting_rdi;
        }
        let cli = CliProfile::NonPviLong64V1;
        let set = minimal_set(&cli).with_user_mode();
        let mut observations = LinuxExecutionObservationCollector::default();
        let (state, report) =
            run_set_with_observations(&mut boot, true, 3, &set, Some(&mut observations));

        assert_eq!(report.delivered_page_faults, 1, "{report:?}");
        assert_eq!(report.cpl3_retired, 0, "{report:?}");
        assert_eq!(state.gpr[Register::Rsp as usize], (USER_RSP0 & !15) - 48);
        let observations = observations.finish();
        assert_eq!(observations.page_fault_events.len(), 1);
        let context = &observations.page_fault_events[0].faulting_context;
        assert_eq!(context.cr3, boot.board.cr3());
        assert_eq!(context.rsp, ENTRY + 0x800);
        assert_eq!(context.rbp, faulting_rbp);
        assert_eq!(context.rcx, faulting_rcx);
        assert_eq!(context.rsi, faulting_rsi);
        assert_eq!(context.rdi, faulting_rdi);
        assert!(context.frame_chain.is_empty());
        assert_eq!(
            context.frame_chain_end,
            crate::FrameChainEnd::UnalignedFramePointer
        );
        assert!(context.page_walk.is_some());
        assert_ne!(
            observations.page_fault_events[0].faulting_context.rsp,
            state.gpr[Register::Rsp as usize],
            "the event records user RSP before delivery, not handler RSP0"
        );
    }

    #[test]
    fn user_mode_facade_the_handler_repairs_the_page_and_iretq_returns_to_cpl3() {
        // The handler makes the PDE of the user page a user page, drops the error code and
        // returns: the user `mov` retires in CPL3 and the `syscall` is the next honest stop.
        let make_handler = |pde: u64, user_pde: u64| {
            let mut handler = vec![0x48, 0xC7, 0xC2]; // MOV RDX, imm32
            handler.extend_from_slice(&(pde as u32).to_le_bytes());
            handler.extend_from_slice(&[0x48, 0xC7, 0xC6]); // MOV RSI, imm32
            handler.extend_from_slice(&(user_pde as u32).to_le_bytes());
            handler.extend_from_slice(&[0x48, 0x89, 0x32]); // MOV [RDX], RSI
            handler.extend_from_slice(&[0x48, 0x83, 0xC4, 0x08]); // ADD RSP, 8
            handler.extend_from_slice(&[0x48, 0xCF]); // IRETQ
            handler
        };
        let (mut boot, file) =
            user_pf_board(&[0xb8, 0x53, 0, 0, 0, 0x0f, 0x05], &make_handler, true);
        let record = run_user_facade(&mut boot, &file, 16);
        assert_eq!(record.exit_kind, "user_mode_unsupported", "{record:?}");
        assert_eq!(record.exceptions_delivered, 1, "{record:?}");
        assert_eq!(record.cpl, 3);
        // Entry IRETQ, five handler instructions (the last is IRETQ to CPL3), the user `mov`.
        assert_eq!((record.retired, record.cpl3_retired), (7, 1), "{record:?}");
        assert_eq!(record.rip, USER_CODE_AT + 5);
        assert_eq!(record.gpr[Register::Rax as usize], 0x53);
        assert_eq!(record.gpr[Register::Rsp as usize], ENTRY + 0x800);
        assert_eq!(record.unsupported_opcode, Some(0x05));
    }

    #[test]
    fn the_board_refuses_a_cpl0_entry_with_a_cs_of_rpl_3() {
        // Review S4-39b (3): without the user mode no user code runs through the CPL0 entry. The
        // state is the one a committed return would leave; the run is a typed rejection, nothing
        // retires and nothing is fetched.
        for user_mode in [false, true] {
            let (mut boot, file) = boot_with_entry(&[0xfa, 0xf4]);
            let state = boot.board.state_mut();
            state.cs_selector = Some(0x13);
            let record = if user_mode {
                // With the member the CPL3 entry runs instead: the board page is supervisor-only,
                // so its first fetch under Ring3 is a #PF with P, U and I/D set.
                state.cs_cache = Some(od_isa::CodeSegmentDescriptorCache {
                    base: 0,
                    limit: u32::MAX,
                    access: 0xfb,
                    flags: 0xa,
                });
                state.ss_selector = Some(0x1b);
                state.ss_cache = Some(od_isa::DataSegmentDescriptorCache {
                    base: 0,
                    limit: u32::MAX,
                    access: 0xf3,
                    flags: 0xc,
                });
                run_user_facade(&mut boot, &file, 4)
            } else {
                run_set_facade(&mut boot, &file, 4, LinuxCpl0SetMembers::minimal())
            };
            assert_eq!(record.retired, 0, "{record:?}");
            assert_eq!(record.rip, ENTRY);
            if user_mode {
                assert_eq!(record.exit_kind, "user_mode_fault", "{record:?}");
                assert!(record.exit_detail.contains("error_code: Some(21)"));
                assert_eq!(record.cpl3_retired, 0);
            } else {
                assert_eq!(record.exit_kind, "rejected", "{record:?}");
                assert!(record.exit_detail.contains("WrongPrivilege"), "{record:?}");
                assert!(!record.user_mode);
            }
        }
    }

    // ---- S4-13: the fetch translation cache of the set loop ----

    use crate::fetch_translation_cache::FetchCacheStats;
    use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_with_cpuid_boundary;

    /// The page the remapped page B points to.
    const ALT_PAGE1: u64 = 0x0500_9000;
    /// A second tree (PML4, PDPT, PD, PT) for the `MOV CR3` and the PDE-write cases.
    const TREE2_PML4: u64 = TREE_PML4 + 0x10000;
    const TREE2_PT: u64 = TREE2_PML4 + 0x3000;
    /// GVA of the data window onto a table frame: PD index 9, PT index 0.
    const WINDOW_VA: u64 = SPLIT_VA + 0x20_0000;
    const A_PRESENT_WRITABLE: u64 = 0x23;
    const A_D_PRESENT_WRITABLE: u64 = 0x63;

    /// `MOV r64, imm32` (REX.W C7 /0, sign-extended): RAX=0, RCX=1, RDX=2, RBX=3.
    fn mov_imm32(register: u8, value: u32) -> Vec<u8> {
        let mut code = vec![0x48, 0xC7, 0xC0 + register];
        code.extend_from_slice(&value.to_le_bytes());
        code
    }

    /// A page-table tree with the accessed bit set on every entry it needs, so that no walk
    /// commits a write unless the guest changed an entry: PML4 -> PDPT -> PD -> PT. `pt_entries`
    /// are `(index, value)` of the PT behind PD index 8 (GVA `SPLIT_VA`); `window` maps
    /// `WINDOW_VA` (PD index 9) to the given physical frame (accessed and dirty already set).
    fn write_tree(
        boot: &mut LinuxDirectBoot,
        pml4: u64,
        pt_entries: &[(u64, u64)],
        window: Option<u64>,
    ) {
        let table = |address: u64| (address | A_PRESENT_WRITABLE).to_le_bytes();
        write_phys(boot, pml4, &table(pml4 + 0x1000));
        write_phys(boot, pml4 + 0x1000, &table(pml4 + 0x2000));
        write_phys(boot, pml4 + 0x2000 + 8 * 8, &table(pml4 + 0x3000));
        for &(index, value) in pt_entries {
            write_phys(boot, pml4 + 0x3000 + index * 8, &value.to_le_bytes());
        }
        if let Some(frame) = window {
            write_phys(boot, pml4 + 0x2000 + 9 * 8, &table(pml4 + 0x4000));
            write_phys(
                boot,
                pml4 + 0x4000,
                &(frame | A_D_PRESENT_WRITABLE).to_le_bytes(),
            );
        }
    }

    struct CacheOutcome {
        state: LongModeState,
        report: PagedCpl0SetReport,
        writes: Vec<(u64, u64)>,
        memory: Vec<u8>,
    }

    /// Builds a fresh board, lets `build` lay out a tree and code and return the state to run
    /// from, and runs the set loop (CPUID boundary on, like the Linux board) with the fetch
    /// translation cache `cache`. Returns the end state, the report, every committed write in
    /// order and the bytes of the page-table and code frames.
    fn run_cache_case(
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        cache: bool,
    ) -> CacheOutcome {
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = build(&mut boot);
        let efer = boot.board.efer();
        let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
        let mut writes = Vec::new();
        let report = run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
            &mut state,
            boot.board.bus_mut(),
            cr3,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |range| writes.push((range.start().get(), range.len())),
            true,
            cache,
            None,
        );
        let mut memory = Vec::new();
        for (start, len) in [
            (TREE_PML4, 0x5000usize),
            (TREE2_PML4, 0x4000),
            (SPLIT_PAGE0, 0x1000),
            (SPLIT_PAGE1, 0x1000),
            (ALT_PAGE1, 0x1000),
        ] {
            let mut bytes = vec![0u8; len];
            read(&mut boot.board, start, &mut bytes).expect("frames are RAM");
            memory.extend(bytes);
        }
        CacheOutcome {
            state,
            report,
            writes,
            memory,
        }
    }

    /// The differential check: the cached and the uncached run must agree on everything the
    /// guest or a device can see (CPU state, the report with its exit, fault record and context
    /// changes, every committed RAM write in order, the bytes of the frames). Returns the
    /// outcome of the cached run, with its cache counters, for the case's own assertions.
    fn assert_cache_is_invisible(
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
    ) -> (CacheOutcome, FetchCacheStats) {
        let mut plain = run_cache_case(build, set, budget, false);
        let mut cached = run_cache_case(build, set, budget, true);
        assert_eq!(plain.report.fetch_cache_stats, None);
        let stats = cached.report.fetch_cache_stats.take().expect("cache stats");
        // S4-13b: the walk counters are measurement. The cache is invisible to data accesses
        // (same data walks) and can only remove instruction-fetch walks; the rest of the report
        // is compared as before.
        let (cached_walks, plain_walks) = (cached.report.walk_counts, plain.report.walk_counts);
        assert_eq!(cached_walks.data_read, plain_walks.data_read);
        assert_eq!(cached_walks.data_write, plain_walks.data_write);
        assert!(
            cached_walks.fetch <= plain_walks.fetch,
            "{cached_walks:?} {plain_walks:?}"
        );
        cached.report.walk_counts = Default::default();
        plain.report.walk_counts = Default::default();
        assert_eq!(cached.report, plain.report);
        assert_eq!(cached.state, plain.state);
        assert_eq!(cached.writes, plain.writes);
        assert!(cached.memory == plain.memory, "frame bytes differ");
        (cached, stats)
    }

    fn tree_state(boot: &LinuxDirectBoot, pml4: u64, rip: u64) -> LongModeState {
        let mut state = boot.board.state().clone();
        state.cr3 = Some(pml4);
        state.rip = rip;
        state
    }

    /// 40 `MOV RAX, imm32` and a `HLT` on one page: one real walk, then hits only.
    #[test]
    fn cache_straight_line_code_takes_one_walk_and_changes_nothing() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            // The leaf has no accessed bit: the first (real) walk sets it, the cache is filled
            // after it and no later fetch writes anything.
            write_tree(boot, TREE_PML4, &[(0, SPLIT_PAGE0 | 3)], None);
            let mut code = Vec::new();
            for value in 0..40u32 {
                code.extend(mov_imm32(0, value));
            }
            code.push(0xF4);
            write_phys(boot, SPLIT_PAGE0 + 0x200, &code);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x200)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 100);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.report.instructions_executed, 41);
        assert_eq!(outcome.state.gpr[Register::Rax as usize], 39);
        assert_eq!(stats.inserts, 1, "{stats:?}");
        assert!(stats.hits > 41 * 3, "{stats:?}");
        assert_eq!(outcome.writes.len(), 1, "{:?}", outcome.writes);
    }

    /// The stream of the table-write cases: `MOV RBX, store_va; MOV RAX, value; MOV [RBX], RAX`
    /// is cut so that the store crosses from page A to page B (two bytes on A, its last byte on
    /// B). Returns the 17 bytes and how many of them end page A.
    fn table_write_stream(store_va: u32, value: u32) -> (Vec<u8>, usize) {
        let mut head = mov_imm32(3, store_va);
        head.extend(mov_imm32(0, value));
        head.extend([0x48, 0x89, 0x03]);
        assert_eq!(head.len(), 17);
        (head, 16)
    }

    fn tail(first: u32, second: u32) -> Vec<u8> {
        let mut tail = mov_imm32(1, first);
        tail.extend(mov_imm32(2, second));
        tail.push(0xF4);
        tail
    }

    /// Lays out the first tree (A -> `SPLIT_PAGE0`, B -> `SPLIT_PAGE1`, a window onto
    /// `window_frame`) and the stream; page B has the tail `0x1111/0x2222`, `ALT_PAGE1` holds
    /// the same bytes with the tail `0x3333/0x4444`.
    fn table_write_case(
        boot: &mut LinuxDirectBoot,
        store_va: u32,
        value: u32,
        window_frame: u64,
    ) -> LongModeState {
        write_tree(
            boot,
            TREE_PML4,
            &[
                (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                (1, SPLIT_PAGE1 | A_PRESENT_WRITABLE),
            ],
            Some(window_frame),
        );
        let (head, on_first) = table_write_stream(store_va, value);
        write_phys(
            boot,
            SPLIT_PAGE0 + 0x1000 - on_first as u64,
            &head[..on_first],
        );
        for (page, tail) in [
            (SPLIT_PAGE1, tail(0x1111, 0x2222)),
            (ALT_PAGE1, tail(0x3333, 0x4444)),
        ] {
            let mut bytes = head[on_first..].to_vec();
            bytes.extend(tail);
            write_phys(boot, page, &bytes);
        }
        tree_state(boot, TREE_PML4, SPLIT_VA + 0x1000 - on_first as u64)
    }

    /// The guest rewrites the PTE of the page it is executing on and the next instruction is
    /// fetched through the new translation: the cache must have dropped the old one.
    #[test]
    fn cache_a_pte_store_remaps_the_page_under_the_cached_translation() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        // PTE 1 of the tree's PT is at PT + 8, seen through the window at WINDOW_VA + 8.
        let build = |boot: &mut LinuxDirectBoot| {
            table_write_case(
                boot,
                (WINDOW_VA + 8) as u32,
                (ALT_PAGE1 | 3) as u32,
                TREE_PML4 + 0x3000,
            )
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        // The tail of the NEW page ran, not the cached old one.
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x3333);
        assert_eq!(outcome.state.gpr[Register::Rdx as usize], 0x4444);
        assert_eq!(
            stats.write_flushes, 1,
            "only the PTE store hits a watched table"
        );
        assert_eq!(stats.context_flushes, 0);
        assert!(stats.hits > 0, "{stats:?}");
    }

    /// Rewriting the PTE with its accessed bit clear: the next fetch walks again and sets it
    /// again, as without the cache (the committed writes are compared in order).
    #[test]
    fn cache_a_pte_store_that_clears_accessed_sets_it_again_on_the_next_fetch() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            table_write_case(
                boot,
                (WINDOW_VA + 8) as u32,
                (SPLIT_PAGE1 | 3) as u32,
                TREE_PML4 + 0x3000,
            )
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x1111);
        assert_eq!(stats.write_flushes, 1);
        // The accessed bit is on PTE 1 again after the run.
        let pte = u64::from_le_bytes(outcome.memory[0x3008..0x3010].try_into().unwrap());
        assert_eq!(pte, SPLIT_PAGE1 | A_PRESENT_WRITABLE);
        // The last committed write is that A-bit commit on the PT entry.
        assert_eq!(
            outcome.writes.last().copied(),
            Some((TREE_PML4 + 0x3000 + 8, 8))
        );
    }

    /// The guest clears the present bit of the PTE it is executing on: the next fetch is the
    /// same precise `#PF` (error code, linear address, saved RIP) as without the cache; a failure
    /// is never served from the cache.
    #[test]
    fn cache_a_pte_store_that_unmaps_the_page_is_the_same_page_fault() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            table_write_case(boot, (WINDOW_VA + 8) as u32, 0, TREE_PML4 + 0x3000)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &outcome.report.exit else {
            panic!("{:?}", outcome.report);
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x10), Some(SPLIT_VA + 0x1001))
        );
        assert_eq!(
            outcome.report.page_fault.as_ref().map(|r| r.saved_rip()),
            Some(SPLIT_VA + 0x1001)
        );
        assert_eq!(stats.write_flushes, 1);
        // The three instructions of the stream retired, the fault is at the fourth.
        assert_eq!(outcome.report.instructions_executed, 3);
    }

    /// The guest rewrites a page DIRECTORY entry of the executing path to another PT (the
    /// watch covers every level, not just the leaf table).
    #[test]
    fn cache_a_pde_store_remaps_the_page_under_the_cached_translation() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            // The window shows the first tree's PD; PD index 8 is at offset 0x40.
            let state = table_write_case(
                boot,
                (WINDOW_VA + 0x40) as u32,
                (TREE2_PT | 3) as u32,
                TREE_PML4 + 0x2000,
            );
            // The new PT keeps A on SPLIT_PAGE0 and maps B to the alternative page.
            write_phys(
                boot,
                TREE2_PT,
                &(SPLIT_PAGE0 | A_PRESENT_WRITABLE).to_le_bytes(),
            );
            write_phys(
                boot,
                TREE2_PT + 8,
                &(ALT_PAGE1 | A_PRESENT_WRITABLE).to_le_bytes(),
            );
            state
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x3333);
        assert_eq!(stats.write_flushes, 1);
    }

    /// The guest rewrites a PDPT entry (level 3) of the executing path to another PD: the next fetch of the same page walks the new frames.
    #[test]
    fn cache_a_pdpte_store_remaps_the_page_under_the_cached_translation() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            // The window shows the first tree's PDPT; entry 0 is at offset 0.
            let state = table_write_case(
                boot,
                WINDOW_VA as u32,
                (TREE2_PML4 + 0x2000 | 3) as u32,
                TREE_PML4 + 0x1000,
            );
            // The second tree keeps A on SPLIT_PAGE0 and maps B to the alternative page.
            write_tree(
                boot,
                TREE2_PML4,
                &[
                    (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                    (1, ALT_PAGE1 | A_PRESENT_WRITABLE),
                ],
                None,
            );
            state
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x3333);
        assert_eq!(outcome.state.gpr[Register::Rdx as usize], 0x4444);
        assert_eq!(stats.write_flushes, 1, "{stats:?}");
    }

    /// The guest rewrites a PML4 entry (level 4) of the executing path to another tree: the next fetch of the same page walks the new frames.
    #[test]
    fn cache_a_pml4e_store_remaps_the_page_under_the_cached_translation() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            // The window shows the first tree's PML4; entry 0 is at offset 0.
            let state = table_write_case(
                boot,
                WINDOW_VA as u32,
                (TREE2_PML4 + 0x1000 | 3) as u32,
                TREE_PML4,
            );
            // The second tree keeps A on SPLIT_PAGE0 and maps B to the alternative page.
            write_tree(
                boot,
                TREE2_PML4,
                &[
                    (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                    (1, ALT_PAGE1 | A_PRESENT_WRITABLE),
                ],
                None,
            );
            state
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x3333);
        assert_eq!(outcome.state.gpr[Register::Rdx as usize], 0x4444);
        assert_eq!(stats.write_flushes, 1, "{stats:?}");
    }

    /// `MOV CR3` between two instructions of a cached page: the instruction after it is fetched
    /// through the NEW tree (the same linear page now maps to other bytes).
    #[test]
    fn cache_mov_cr3_drops_the_translations_of_the_old_tree() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            // The first instruction (MOV RAX, TREE2_PML4) crosses A -> B, so B is cached under
            // the first CR3; MOV CR3 and the MOV RCX of the old/new page follow on B.
            let mut stream = mov_imm32(0, TREE2_PML4 as u32);
            stream.extend([0x0F, 0x22, 0xD8]);
            let on_first = 3;
            write_tree(
                boot,
                TREE_PML4,
                &[
                    (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                    (1, SPLIT_PAGE1 | A_PRESENT_WRITABLE),
                ],
                None,
            );
            write_tree(
                boot,
                TREE2_PML4,
                &[
                    (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                    (1, ALT_PAGE1 | A_PRESENT_WRITABLE),
                ],
                None,
            );
            write_phys(
                boot,
                SPLIT_PAGE0 + 0x1000 - on_first as u64,
                &stream[..on_first],
            );
            for (page, marker) in [(SPLIT_PAGE1, 0x1111), (ALT_PAGE1, 0x3333)] {
                let mut bytes = stream[on_first..].to_vec();
                bytes.extend(mov_imm32(1, marker));
                bytes.push(0xF4);
                write_phys(boot, page, &bytes);
            }
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x1000 - on_first as u64)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x3333);
        assert_eq!(outcome.report.cr3, TREE2_PML4);
        assert_eq!(outcome.report.translation_context_changes.len(), 1);
        assert_eq!(stats.context_flushes, 1, "{stats:?}");
        assert!(stats.hits > 0, "{stats:?}");
    }

    /// An instruction across a page boundary whose second page is not mapped: the first page is
    /// cached, the second byte run walks and faults exactly as without the cache.
    #[test]
    fn cache_a_crossing_fetch_onto_an_unmapped_page_is_the_same_page_fault() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            write_tree(
                boot,
                TREE_PML4,
                &[(0, SPLIT_PAGE0 | A_PRESENT_WRITABLE)],
                None,
            );
            let code = mov_imm32(0, 7);
            write_phys(boot, SPLIT_PAGE0 + 0x1000 - 3, &code[..3]);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x1000 - 3)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 4);
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &outcome.report.exit else {
            panic!("{:?}", outcome.report);
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x10), Some(SPLIT_VA + 0x1000))
        );
        assert_eq!(
            outcome.report.page_fault.as_ref().map(|r| r.saved_rip()),
            Some(SPLIT_VA + 0x1000 - 3)
        );
        assert_eq!(outcome.report.instructions_executed, 0);
        assert_eq!(
            stats.inserts, 1,
            "only page A; the failed page is not cached"
        );
        assert!(stats.hits >= 2, "{stats:?}");
    }

    /// The CPUID boundary peeks through the cache too: a `CPUID` of leaf 1 still stops the run
    /// before it executes, with the same record.
    #[test]
    fn cache_cpuid_boundary_peek_through_the_cache_stops_before_leaf_1() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let cpuid = CpuidProfile::DiagnosticV1;
        let set = switch_set(&cli, &halt).with_cpuid(&cpuid);
        let build = |boot: &mut LinuxDirectBoot| {
            write_tree(
                boot,
                TREE_PML4,
                &[(0, SPLIT_PAGE0 | A_PRESENT_WRITABLE)],
                None,
            );
            let mut code = vec![0xFA, 0x31, 0xC0, 0x0F, 0xA2];
            code.extend(mov_eax(1));
            code.extend([0x66, 0x0F, 0xA2, 0xF4]);
            write_phys(boot, SPLIT_PAGE0 + 0x200, &code);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x200)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &set, 32);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::CpuidDivergence { leaf: 1 },
            "{:?}",
            outcome.report
        );
        // CLI, XOR, CPUID(0), MOV EAX,1 retired; the prefixed CPUID did not.
        assert_eq!(outcome.report.instructions_executed, 4);
        assert!(stats.hits > 0, "{stats:?}");
    }

    /// Self-modifying code: the guest stores over the immediate of the next instruction on the
    /// cached code page. The cache holds the translation, never the byte, so the patched
    /// immediate runs, as without the cache.
    #[test]
    fn cache_self_modifying_code_runs_the_patched_bytes() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            write_tree(
                boot,
                TREE_PML4,
                &[(0, SPLIT_PAGE0 | A_PRESENT_WRITABLE)],
                None,
            );
            // MOV RBX, patch address; MOV RAX, 0x5555; MOV [RBX], EAX; MOV RCX, 0x1111; HLT.
            let mut code = mov_imm32(3, (SPLIT_VA + 0x213) as u32);
            code.extend(mov_imm32(0, 0x5555));
            code.extend([0x89, 0x03]);
            code.extend(mov_imm32(1, 0x1111));
            code.push(0xF4);
            write_phys(boot, SPLIT_PAGE0 + 0x200, &code);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x200)
        };
        let (outcome, stats) = assert_cache_is_invisible(&build, &switch_set(&cli, &halt), 16);
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rcx as usize], 0x5555);
        assert!(stats.hits > 0, "{stats:?}");
    }

    /// The flag: off in the minimal set, on in the Linux default, and the run through the facade
    /// gives the same record with it on or off.
    #[test]
    fn cache_flag_changes_no_field_of_the_stop_record() {
        let mut code = vec![0xFA];
        for value in 0..64u32 {
            code.extend(mov_imm32(0, value));
        }
        code.push(0xF4);
        let mut records = Vec::new();
        for fetch_cache in [true, false] {
            let (mut boot, file) = boot_with_entry(&code);
            let members = LinuxCpl0SetMembers {
                fetch_cache,
                ..halting_default()
            };
            records.push(run_set_facade(&mut boot, &file, 1000, members));
        }
        let (cached, plain) = (&records[0], &records[1]);
        assert_eq!(cached.exit_kind, "halted");
        assert_eq!(cached.retired, 66);
        // `LinuxStopRecord` has no `PartialEq`: its Debug text carries every field.
        assert_eq!(format!("{cached:?}"), format!("{plain:?}"));
    }

    // ---- S4-14h: the guest-driven invalidation policy of the fetch cache ----

    use crate::fetch_translation_cache::FetchCacheInvalidation;
    use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_with_policy;

    const OLD_MARKER: u64 = 0x1111;
    const NEW_MARKER: u64 = 0x3333;
    /// `INVLPG [RBX]`: the operand is never evaluated (S4-38L7ab), RBX only has to be a register.
    const INVLPG_RBX: [u8; 3] = [0x0F, 0x01, 0x3B];
    /// `NOP DWORD [RAX]`: the same length as the `INVLPG` and no effect.
    const NOP3: [u8; 3] = [0x0F, 0x1F, 0x00];
    const MOV_CR3_RAX: [u8; 3] = [0x0F, 0x22, 0xD8];
    const MOV_CR0_RAX: [u8; 3] = [0x0F, 0x22, 0xC0];
    const MOV_CR4_RAX: [u8; 3] = [0x0F, 0x22, 0xE0];
    const CR4_PAE: u64 = 1 << 5;
    const CR4_PAE_PGE: u64 = CR4_PAE | (1 << 7);

    struct PolicyOutcome {
        state: LongModeState,
        report: PagedCpl0SetReport,
        stats: FetchCacheStats,
    }

    /// Runs the set loop with the fetch cache policy `policy` over the case `build` lays out.
    fn run_policy_case(
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        policy: FetchCacheInvalidation,
    ) -> PolicyOutcome {
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = build(&mut boot);
        let efer = boot.board.efer();
        let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
        let mut report = run_cpl0_set_paged_2m_ram_with_policy(
            &mut state,
            boot.board.bus_mut(),
            cr3,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |_| {},
            true,
            Some(policy),
            None,
        );
        let stats = report.fetch_cache_stats.take().expect("the cache is on");
        PolicyOutcome {
            state,
            report,
            stats,
        }
    }

    /// The remap layout. Page A (always mapped to `SPLIT_PAGE0`) ends with
    /// `[prefix] MOV RBX, PTE1; MOV RCX, new PTE; MOV [RBX], RCX` and the last byte of that store
    /// lies on page X (`SPLIT_VA + 0x1000`), so X is fetched, and cached, before the store
    /// retires. The store rewrites the PTE of X from the old frame (`OLD_MARKER` code) to
    /// `ALT_PAGE1` (`NEW_MARKER` code). Both frames hold the same bytes up to the marker:
    /// the rest of the store, then `sync` (what the guest does to invalidate, or a no-op of the
    /// same length), then `MOV RDX, marker; HLT`. RDX tells which frame the marker was fetched
    /// from: a stale cached translation of X runs `sync` and the marker of the old frame.
    fn remap_case(
        boot: &mut LinuxDirectBoot,
        prefix: &[u8],
        sync: &[u8],
        cr4: Option<u64>,
    ) -> LongModeState {
        write_tree(
            boot,
            TREE_PML4,
            &[
                (0, SPLIT_PAGE0 | A_PRESENT_WRITABLE),
                (1, SPLIT_PAGE1 | A_PRESENT_WRITABLE),
            ],
            Some(TREE_PML4 + 0x3000),
        );
        let mut head = prefix.to_vec();
        head.extend(mov_imm32(3, (WINDOW_VA + 8) as u32));
        head.extend(mov_imm32(1, (ALT_PAGE1 | 3) as u32));
        head.extend([0x48, 0x89, 0x0B]);
        let on_first = head.len() - 1;
        write_phys(
            boot,
            SPLIT_PAGE0 + 0x1000 - on_first as u64,
            &head[..on_first],
        );
        for (page, marker) in [(SPLIT_PAGE1, OLD_MARKER), (ALT_PAGE1, NEW_MARKER)] {
            let mut bytes = head[on_first..].to_vec();
            bytes.extend(sync);
            bytes.extend(mov_imm32(2, marker as u32));
            bytes.push(0xF4);
            write_phys(boot, page, &bytes);
        }
        let mut state = tree_state(boot, TREE_PML4, SPLIT_VA + 0x1000 - on_first as u64);
        if let Some(cr4) = cr4 {
            state.cr4 = Some(cr4);
        }
        state
    }

    fn assert_halted_with_marker(outcome: &PolicyOutcome, marker: u64) {
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(
            outcome.state.gpr[Register::Rdx as usize],
            marker,
            "{:?}",
            outcome.stats
        );
    }

    /// The guest rewrites the PTE of a cached page and executes `INVLPG`: under `GuestOnly` the
    /// store alone drops nothing, the retired `INVLPG` does, and the next fetch of the page walks
    /// the new frame.
    #[test]
    fn guest_only_invlpg_after_a_pte_store_runs_the_new_code() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &INVLPG_RBX, None);
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            16,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&outcome, NEW_MARKER);
        let stats = outcome.stats;
        assert_eq!(stats.invlpg_flushes, 1, "{stats:?}");
        assert_eq!(stats.write_flushes, 0, "{stats:?}");
        assert!(stats.ignored_table_writes >= 1, "{stats:?}");
        assert_eq!((stats.control_flushes, stats.context_flushes), (0, 0));
        // MOV RBX, MOV RCX, store, INVLPG, MOV RDX, HLT.
        assert_eq!(outcome.report.instructions_executed, 6);
    }

    /// The same program without the `INVLPG`: a TLB-like cache may keep the old translation, a
    /// write-watching one may not. Both outcomes are pinned.
    #[test]
    fn guest_only_without_invlpg_keeps_the_stale_translation_and_write_watch_does_not() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &NOP3, None);
        let guest_only = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            16,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&guest_only, OLD_MARKER);
        let stats = guest_only.stats;
        assert_eq!(
            (
                stats.invlpg_flushes,
                stats.control_flushes,
                stats.context_flushes,
                stats.write_flushes
            ),
            (0, 0, 0, 0),
            "{stats:?}"
        );
        assert!(stats.ignored_table_writes >= 1, "{stats:?}");
        let write_watch = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            16,
            FetchCacheInvalidation::WriteWatch,
        );
        assert_halted_with_marker(&write_watch, NEW_MARKER);
        assert_eq!(write_watch.stats.write_flushes, 1);
        assert_eq!(
            (
                write_watch.stats.invlpg_flushes,
                write_watch.stats.control_flushes,
                write_watch.stats.ignored_table_writes
            ),
            (0, 0, 0)
        );
    }

    /// `INVLPG` is not what makes `WriteWatch` see the new page: the policy is unchanged and the
    /// `INVLPG` flushes nothing there (the table write already did).
    #[test]
    fn write_watch_ignores_the_invlpg_of_the_guest() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &INVLPG_RBX, None);
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            16,
            FetchCacheInvalidation::WriteWatch,
        );
        assert_halted_with_marker(&outcome, NEW_MARKER);
        assert_eq!(outcome.stats.write_flushes, 1);
        assert_eq!(outcome.stats.invlpg_flushes, 0);
    }

    /// S4-13c: the shadows of a `WriteWatch` run see the guest's `INVLPG` (detected by a peek that
    /// bypasses the cache) and the table write, and the run itself is the one without the sweep:
    /// same state, same report, same counters of the real cache.
    #[test]
    fn sweep_shadows_see_the_guest_invlpg_of_a_write_watch_run_without_changing_it() {
        use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_observed_with_sweep;
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let shadow = |invalidation| crate::SweepConfig {
            invalidation,
            ..crate::SweepConfig::REFERENCE
        };
        let configs = [
            shadow(crate::SweepInvalidation::WriteWatch),
            shadow(crate::SweepInvalidation::GuestOnly),
        ];
        for (sync, invlpgs) in [(&INVLPG_RBX, 1u64), (&NOP3, 0)] {
            let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], sync, None);
            let plain = run_policy_case(&build, &set, 16, FetchCacheInvalidation::WriteWatch);
            let sweep = std::rc::Rc::new(crate::fetch_cache_sweep::FetchCacheSweep::new(&configs));
            let (mut boot, _) = boot_with_entry(&[0xF4]);
            let mut state = build(&mut boot);
            let efer = boot.board.efer();
            let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
            let mut report = run_cpl0_set_paged_2m_ram_observed_with_sweep(
                &mut state,
                boot.board.bus_mut(),
                cr3,
                efer,
                long64(),
                16,
                INTEGER_FLAGS,
                &set,
                None,
                || false,
                |_| {},
                true,
                Some(FetchCacheInvalidation::WriteWatch),
                None,
                None,
                None,
                Some(&sweep),
                crate::fetch_translation_cache::FetchCacheTag::Page4K,
            );
            let stats = report.fetch_cache_stats.take().expect("the cache is on");
            assert_eq!(stats, plain.stats, "the real cache counts the same");
            assert_eq!(state.gpr, plain.state.gpr);
            assert_eq!(state.rip, plain.state.rip);
            assert_eq!(
                format!("{report:?}"),
                format!("{:?}", {
                    let mut other = plain.report;
                    other.fetch_cache_stats = None;
                    other
                })
            );
            let report = sweep.report();
            let (watch, guest) = (&report.configs[0].total, &report.configs[1].total);
            // The write-watch shadow is the real cache, whatever the guest invalidates.
            assert_eq!(*watch, stats);
            assert_eq!((watch.invlpg_flushes, watch.ignored_table_writes), (0, 0));
            assert_eq!(guest.invlpg_flushes, invlpgs, "{guest:?}");
            assert_eq!(guest.write_flushes, 0, "{guest:?}");
            assert!(guest.ignored_table_writes >= 1, "{guest:?}");
            assert_eq!(report.events.calls, 1);
        }
    }

    /// `MOV CR3` with the value it already has, after the remap: no table is written afterwards
    /// and the paging snapshot is unchanged, so only the flush of the retired `MOV CR3` makes the
    /// next fetch walk the new frame.
    #[test]
    fn guest_only_reloading_cr3_with_the_same_value_runs_the_new_code() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            remap_case(boot, &mov_imm32(0, TREE_PML4 as u32), &MOV_CR3_RAX, None)
        };
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            16,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&outcome, NEW_MARKER);
        assert_eq!(outcome.report.translation_context_changes.len(), 1);
        assert_eq!(outcome.report.cr3, TREE_PML4);
        let stats = outcome.stats;
        assert_eq!(stats.context_flushes, 1, "{stats:?}");
        assert_eq!((stats.invlpg_flushes, stats.control_flushes), (0, 0));
        assert_eq!(stats.write_flushes, 0, "{stats:?}");
    }

    /// CR4.PGE 1 -> 0 -> 1 after the remap: the CPU state returns to where it was, the paging
    /// snapshot of the cache key never changes, so only the comparison of CR4 around each
    /// instruction flushes.
    #[test]
    fn guest_only_toggling_cr4_pge_runs_the_new_code() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let mut sync = mov_eax(CR4_PAE as u32);
        sync.extend(MOV_CR4_RAX);
        sync.extend(mov_eax(CR4_PAE_PGE as u32));
        sync.extend(MOV_CR4_RAX);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &sync, Some(CR4_PAE_PGE));
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            24,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&outcome, NEW_MARKER);
        assert_eq!(outcome.state.cr4, Some(CR4_PAE_PGE));
        let stats = outcome.stats;
        assert_eq!(stats.control_flushes, 2, "{stats:?}");
        assert_eq!((stats.invlpg_flushes, stats.context_flushes), (0, 0));
        assert_eq!(stats.write_flushes, 0, "{stats:?}");
    }

    /// A rewrite of CR0 (AM toggled and restored) is a control change too: the cache is flushed.
    #[test]
    fn guest_only_rewriting_cr0_runs_the_new_code() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let cr0 = {
            let (boot, _) = boot_with_entry(&[0xF4]);
            boot.board.state().cr0.expect("the board models CR0")
        };
        let mut sync = mov_eax((cr0 | (1 << 18)) as u32);
        sync.extend(MOV_CR0_RAX);
        sync.extend(mov_eax(cr0 as u32));
        sync.extend(MOV_CR0_RAX);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &sync, None);
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            24,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&outcome, NEW_MARKER);
        assert_eq!(outcome.state.cr0, Some(cr0));
        let stats = outcome.stats;
        assert_eq!(stats.control_flushes, 2, "{stats:?}");
        assert_eq!(stats.write_flushes, 0, "{stats:?}");
    }

    /// A MOV to a control register that changes nothing (the value it already has) is not a
    /// reason to flush: the stale translation survives, as without the instruction.
    #[test]
    fn guest_only_a_cr4_write_of_the_same_value_flushes_nothing() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let mut sync = mov_eax(CR4_PAE_PGE as u32);
        sync.extend(MOV_CR4_RAX);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &sync, Some(CR4_PAE_PGE));
        let outcome = run_policy_case(
            &build,
            &switch_set(&cli, &halt),
            24,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&outcome, OLD_MARKER);
        assert_eq!(outcome.stats.control_flushes, 0, "{:?}", outcome.stats);
    }

    /// A 2 MiB leaf that the guest tightens (`NX` set, or not present) while its code runs from
    /// the cached translation. `sync` is the `INVLPG` or a no-op; the walk of the instruction after
    /// `sync` is exact when the cache was flushed and may hit the cache when it was not.
    /// Returns the outcome of the `GuestOnly` and the `WriteWatch` run.
    fn two_mib_tightening(
        offset: u64,
        value: u32,
        sync: &[u8],
        set: &Cpl0ProfileSet<'_>,
    ) -> (PolicyOutcome, PolicyOutcome) {
        let build = |boot: &mut LinuxDirectBoot| {
            // The tree of the window onto the PD, then PD index 8 becomes a 2 MiB leaf
            // (present, writable, accessed, page size) over the frame of the split pages.
            write_tree(boot, TREE_PML4, &[], Some(TREE_PML4 + 0x2000));
            write_phys(
                boot,
                TREE_PML4 + 0x2000 + 8 * 8,
                &(SPLIT_PAGE0 | 0xA3).to_le_bytes(),
            );
            let mut code = mov_imm32(3, (WINDOW_VA + 8 * 8 + offset) as u32);
            code.extend(mov_imm32(0, value));
            code.extend([0x89, 0x03]);
            code.extend(sync);
            code.extend(mov_imm32(2, OLD_MARKER as u32));
            code.push(0xF4);
            write_phys(boot, SPLIT_PAGE0 + 0x200, &code);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x200)
        };
        (
            run_policy_case(&build, set, 16, FetchCacheInvalidation::GuestOnly),
            run_policy_case(&build, set, 16, FetchCacheInvalidation::WriteWatch),
        )
    }

    /// The fetch fault of a run that retired `retired` instructions of `MOV RBX` (7 bytes),
    /// `MOV RAX` (7), store (2) and `sync` (3): the next instruction faults.
    fn assert_fetch_fault(outcome: &PolicyOutcome, error_code: u32, retired: u64) {
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &outcome.report.exit else {
            panic!("{:?}", outcome.report);
        };
        let rip = SPLIT_VA + 0x200 + 16 + 3 * (retired - 3);
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(error_code), Some(rip)),
            "{:?}",
            outcome.report
        );
        assert_eq!(
            outcome.report.page_fault.as_ref().map(|r| r.saved_rip()),
            Some(rip)
        );
        assert_eq!(outcome.report.instructions_executed, retired);
    }

    /// After `INVLPG` the tightened 2 MiB leaf is walked again: a fetch of an `NX` page is the
    /// exact `#PF` (present, instruction fetch). Without it the stale translation may run on
    /// (allowed). `WriteWatch` sees the fault right after the table store (3 retired), at the
    /// instruction that would have been the `INVLPG`.
    #[test]
    fn guest_only_two_mib_leaf_made_nx_faults_exactly_after_invlpg() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        // The high dword of the PDE gets bit 31: PDE bit 63 (NX).
        let (with_invlpg, watch) = two_mib_tightening(4, 0x8000_0000, &INVLPG_RBX, &set);
        assert_fetch_fault(&with_invlpg, 0x11, 4);
        assert_eq!(with_invlpg.stats.invlpg_flushes, 1);
        assert_fetch_fault(&watch, 0x11, 3);
        assert_eq!(watch.stats.write_flushes, 1);
        let (without, watch) = two_mib_tightening(4, 0x8000_0000, &NOP3, &set);
        assert_halted_with_marker(&without, OLD_MARKER);
        assert_eq!(without.stats.invlpg_flushes, 0);
        assert_fetch_fault(&watch, 0x11, 3);
    }

    /// The same for a leaf that becomes not present: the fetch fault has no present bit.
    #[test]
    fn guest_only_two_mib_leaf_made_not_present_faults_exactly_after_invlpg() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let (with_invlpg, watch) = two_mib_tightening(0, 0, &INVLPG_RBX, &set);
        assert_fetch_fault(&with_invlpg, 0x10, 4);
        assert_eq!(with_invlpg.stats.invlpg_flushes, 1);
        assert_fetch_fault(&watch, 0x10, 3);
        let (without, watch) = two_mib_tightening(0, 0, &NOP3, &set);
        assert_halted_with_marker(&without, OLD_MARKER);
        assert_fetch_fault(&watch, 0x10, 3);
    }

    /// `INVLPG` with a GS override and with a REX byte (no W): the peek skips the prefixes, so the
    /// retired instruction flushes the cache like the bare form.
    #[test]
    fn guest_only_prefixed_invlpg_forms_run_the_new_code() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        for sync in [
            &[0x65u8, 0x0F, 0x01, 0x38][..], // INVLPG GS:[RAX]
            &[0x41, 0x0F, 0x01, 0x3F],       // INVLPG [R15]
            &[0x64, 0x41, 0x0F, 0x01, 0x3F], // INVLPG FS:[R15]
        ] {
            let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], sync, None);
            let outcome = run_policy_case(
                &build,
                &switch_set(&cli, &halt),
                16,
                FetchCacheInvalidation::GuestOnly,
            );
            assert_halted_with_marker(&outcome, NEW_MARKER);
            assert_eq!(outcome.stats.invlpg_flushes, 1, "{sync:02x?}");
        }
    }

    /// What the peek calls an `INVLPG` is what the decoder admits as one: an optional FS/GS
    /// override, an optional REX without W directly before the opcode, `0F 01 /7` with a memory
    /// operand. `SWAPGS` (`0F 01 F8`), `RDTSCP` (`0F 01 F9`) and every other `mod = 3` form are
    /// not; neither are the other `0F 01` groups, REX.W and the prefixes the decoder refuses.
    #[test]
    fn the_invlpg_peek_recognizes_exactly_the_decoded_shapes() {
        use crate::paged_memory_adapter::invlpg_at_rip;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let cr3 = boot.board.cr3();
        let mut state = boot.board.state().clone();
        state.cr3 = Some(cr3);
        state.rip = ENTRY;
        let paging =
            crate::page_walk::Long64ProbeConfig::from_cpu_state(&state, cr3, boot.board.efer())
                .expect("the board state is a valid snapshot");
        for (bytes, expected) in [
            (&[0x0Fu8, 0x01, 0x3B][..], true),
            (&[0x0F, 0x01, 0x38], true),
            (&[0x0F, 0x01, 0x7B, 0x10], true),
            (&[0x65, 0x0F, 0x01, 0x38], true),
            (&[0x64, 0x0F, 0x01, 0x3B], true),
            (&[0x41, 0x0F, 0x01, 0x3F], true),
            (&[0x47, 0x0F, 0x01, 0x3F], true),
            (&[0x65, 0x41, 0x0F, 0x01, 0x38], true),
            // The register forms of `0F 01 /7` and the other `mod = 3` forms.
            (&[0x0F, 0x01, 0xF8], false), // SWAPGS
            (&[0x0F, 0x01, 0xF9], false), // RDTSCP
            (&[0x0F, 0x01, 0xFB], false), // reg 7, mod 3
            (&[0x0F, 0x01, 0xC1], false), // VMCALL, mod 3
            (&[0x65, 0x0F, 0x01, 0xF8], false),
            (&[0x41, 0x0F, 0x01, 0xFF], false),
            // Memory forms of the other groups.
            (&[0x0F, 0x01, 0x10], false), // LGDT
            (&[0x0F, 0x01, 0x18], false), // LIDT
            (&[0x0F, 0x01, 0x00], false), // SGDT
            (&[0x0F, 0x01, 0x20], false), // SMSW
            // Prefixes the decoder refuses or that are not directly before the opcode.
            (&[0x48, 0x0F, 0x01, 0x3B], false),
            (&[0x4F, 0x0F, 0x01, 0x3F], false),
            (&[0x41, 0x65, 0x0F, 0x01, 0x38], false),
            (&[0x66, 0x0F, 0x01, 0x3B], false),
            (&[0xF0, 0x0F, 0x01, 0x3B], false),
            (&[0x26, 0x0F, 0x01, 0x3B], false),
            (&[0x0F, 0x00, 0x3B], false),
            (&[0x90, 0x0F, 0x01, 0x3B], false),
        ] {
            let mut window = bytes.to_vec();
            window.resize(8, 0x90);
            write_phys(&mut boot, ENTRY, &window);
            assert_eq!(
                invlpg_at_rip(
                    &state,
                    boot.board.bus_mut().physical_memory(),
                    &paging,
                    long64(),
                    None
                ),
                expected,
                "{bytes:02x?}"
            );
        }
    }

    /// The tree for the delivered-`#PF` case: the board's own tree with PD index 40 (VA
    /// `0x0500_0000`) replaced by a PT that maps page A to `SPLIT_PAGE0` and page X (`+ 0x1000`) to
    /// `SPLIT_PAGE1`. The code on X (both frames) stores to an unmapped page, so it takes a `#PF`
    /// right after it has been cached; the handler (through the guest IDT) remaps PTE 1 to
    /// `ALT_PAGE1` WITHOUT an `INVLPG`, maps the faulting page and returns to the store, which is
    /// followed by `MOV RDX, marker; HLT`.
    const PF_TEST_VA: u64 = 0x0500_0000;
    const PF_TEST_PT: u64 = 0x0400_3000;

    fn page_fault_remap_case(boot: &mut LinuxDirectBoot) -> LongModeState {
        let cr3 = boot.board.cr3();
        write_phys(
            boot,
            cr3 + 0x2000 + (PF_TEST_VA / (2 * MIB)) * 8,
            &(PF_TEST_PT | A_PRESENT_WRITABLE).to_le_bytes(),
        );
        write_phys(
            boot,
            PF_TEST_PT,
            &(SPLIT_PAGE0 | A_PRESENT_WRITABLE).to_le_bytes(),
        );
        write_phys(
            boot,
            PF_TEST_PT + 8,
            &(SPLIT_PAGE1 | A_PRESENT_WRITABLE).to_le_bytes(),
        );
        for (frame, marker) in [(SPLIT_PAGE1, OLD_MARKER), (ALT_PAGE1, NEW_MARKER)] {
            let mut code = mov_imm32(3, (PF_VA + 0x20) as u32);
            code.extend(PF_STORE);
            code.extend(mov_imm32(2, marker as u32));
            code.push(0xF4);
            write_phys(boot, frame, &code);
        }
        // PTE 1 -> ALT_PAGE1 (no INVLPG), then the repair of the faulting page and IRETQ.
        let mut handler = mov_imm32(2, (PF_TEST_PT + 8) as u32);
        handler.extend(mov_imm32(6, (ALT_PAGE1 | 3) as u32));
        handler.extend([0x48, 0x89, 0x32]); // MOV [RDX], RSI
        handler.extend(pf_repair_handler(
            pf_pde_address(boot),
            (DATA & !(2 * MIB - 1)) | 0x83,
        ));
        write_phys(boot, PF_HANDLER_AT, &handler);
        let mut gate = [0u8; 16];
        gate[0..2].copy_from_slice(&(PF_HANDLER_AT as u16).to_le_bytes());
        gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
        gate[5] = 0x8E;
        gate[6..8].copy_from_slice(&((PF_HANDLER_AT >> 16) as u16).to_le_bytes());
        write_phys(boot, PF_IDT_AT + 14 * 16, &gate);
        let state = boot.board.state_mut();
        state.idtr = Some(od_isa::state::Idtr {
            base: PF_IDT_AT,
            limit: 0xFF,
        });
        state.gpr[Register::Rcx as usize] = 0x5A;
        state.rflags = 2 | (1 << 9) | 1;
        let mut state = boot.board.state().clone();
        state.cr3 = Some(cr3);
        state.rip = PF_TEST_VA + 0x1000;
        state
    }

    /// A `#PF` delivered to the guest drops the whole cache (a deliberate superset of what
    /// the architecture requires: it would only invalidate the translation of the faulting
    /// address). This test pins the stricter implementation behaviour, not an architectural
    /// requirement: the handler changes the PTE of a page that is not the faulting one, with
    /// no `INVLPG`, and the restarted instruction must be fetched through the new frame.
    #[test]
    fn guest_only_a_delivered_page_fault_drops_the_cached_translation() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let guest_only = run_policy_case(
            &page_fault_remap_case,
            &switch_set(&cli, &halt),
            24,
            FetchCacheInvalidation::GuestOnly,
        );
        assert_halted_with_marker(&guest_only, NEW_MARKER);
        assert_eq!(guest_only.report.delivered_exceptions, 1);
        let stats = guest_only.stats;
        assert_eq!(stats.fault_flushes, 1, "{stats:?}");
        assert_eq!(
            (
                stats.invlpg_flushes,
                stats.control_flushes,
                stats.context_flushes,
                stats.write_flushes
            ),
            (0, 0, 0, 0),
            "{stats:?}"
        );
        // The write-watching policy sees the handler's PTE store itself.
        let write_watch = run_policy_case(
            &page_fault_remap_case,
            &switch_set(&cli, &halt),
            24,
            FetchCacheInvalidation::WriteWatch,
        );
        assert_halted_with_marker(&write_watch, NEW_MARKER);
        assert_eq!(write_watch.stats.fault_flushes, 0);
    }

    /// The default of the board and of the runner is untouched: the same layout through the
    /// `bool` entry point (S4-13) is the write-watching cache.
    #[test]
    fn the_bool_entry_point_is_the_write_watch_cache() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &NOP3, None);
        let outcome = run_cache_case(&build, &switch_set(&cli, &halt), 16, true);
        let stats = outcome.report.fetch_cache_stats.expect("cache stats");
        assert_eq!(outcome.state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats.write_flushes, 1, "{stats:?}");
        assert_eq!(
            (stats.invlpg_flushes, stats.control_flushes),
            (0, 0),
            "{stats:?}"
        );
    }

    // ---- S4-14i: the fetch cache that outlives a call of the set loop ----

    use crate::fetch_translation_cache::FetchTranslationCache;
    use crate::paged_memory_adapter::run_cpl0_set_paged_2m_ram_with_cache;

    /// One call of the set loop over `cache` (`None`: no cache). A cache built with
    /// `with_invalidation` and dropped afterwards is the per-call cache of every earlier caller.
    fn run_call_with_cache(
        boot: &mut LinuxDirectBoot,
        state: &mut LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        cache: Option<&FetchTranslationCache>,
    ) -> PagedCpl0SetReport {
        let efer = boot.board.efer();
        let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
        run_cpl0_set_paged_2m_ram_with_cache(
            state,
            boot.board.bus_mut(),
            crate::vcpu::BOOT_VCPU_INDEX,
            cr3,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |_| {},
            true,
            cache,
            None,
            None,
            None,
        )
    }

    /// Two calls over the remap layout of S4-14h (`sync` between the store and the marker). The
    /// first call retires `first` instructions (3 stop right after the PTE store, 4 after the
    /// instruction behind it), the second runs to the `HLT`. `between` is the host's action
    /// between the calls (it gets the board and the cache). With `persistent` the same cache
    /// serves both calls, otherwise each call gets a fresh one. Returns the end state, the report
    /// of the first and of the second call (their `fetch_cache_stats` are the cache's counters
    /// at the end of that call: cumulative for a persistent cache).
    fn run_two_calls(
        sync: &[u8],
        policy: FetchCacheInvalidation,
        persistent: bool,
        first: u64,
        between: &dyn Fn(&mut LinuxDirectBoot, &FetchTranslationCache),
    ) -> (LongModeState, PagedCpl0SetReport, PagedCpl0SetReport) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = remap_case(&mut boot, &[], sync, None);
        let shared = if persistent {
            FetchTranslationCache::persistent(policy)
        } else {
            FetchTranslationCache::with_invalidation(policy)
        };
        let call = |boot: &mut LinuxDirectBoot, state: &mut LongModeState, budget: u64| {
            if persistent {
                run_call_with_cache(boot, state, &set, budget, Some(&shared))
            } else {
                let fresh = FetchTranslationCache::with_invalidation(policy);
                run_call_with_cache(boot, state, &set, budget, Some(&fresh))
            }
        };
        let one = call(&mut boot, &mut state, first);
        assert_eq!(
            one.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
            "{one:?}"
        );
        assert_eq!(one.instructions_executed, first);
        between(&mut boot, &shared);
        let two = call(&mut boot, &mut state, 16);
        assert_eq!(two.exit, PagedCpl0SetExit::Run(RunExit::Halted), "{two:?}");
        (state, one, two)
    }

    fn stats_of(report: &PagedCpl0SetReport) -> FetchCacheStats {
        report.fetch_cache_stats.expect("the cache is on")
    }

    /// The cache survives the end of a call. After the PTE store of call 1 the page the code
    /// runs on is still cached with its old frame, and call 2 (no event in between, no flush of
    /// the policy) fetches through it and runs the old marker: the hit counter grows over the
    /// boundary. The per-call cache, with the same two calls, has lost it and walks the new PTE.
    #[test]
    fn persistent_cache_keeps_its_entries_over_the_call_boundary() {
        let none = |_: &mut LinuxDirectBoot, _: &FetchTranslationCache| {};
        let (state, one, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, true, 3, &none);
        assert_eq!(state.gpr[Register::Rdx as usize], OLD_MARKER);
        let (after_one, after_two) = (stats_of(&one), stats_of(&two));
        assert!(
            after_two.hits > after_one.hits,
            "{after_one:?} {after_two:?}"
        );
        assert_eq!(
            (
                after_two.host_flushes,
                after_two.invlpg_flushes,
                after_two.control_flushes,
                after_two.context_flushes,
                after_two.write_flushes,
                after_two.fault_flushes
            ),
            (0, 0, 0, 0, 0, 0),
            "{after_two:?}"
        );
        // Call 2 inserted nothing: the entries of call 1 served it.
        assert_eq!(after_two.inserts, after_one.inserts, "{after_two:?}");

        let (state, _, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, false, 3, &none);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert!(stats_of(&two).inserts >= 1);
    }

    /// 40 `MOV RAX, imm32` and a `HLT` on one page behind a PT entry with its accessed bit
    /// already walked, run in two calls (10 instructions, then to the `HLT`). `between` is the host's
    /// action between the calls. Returns the end state and the two reports.
    fn run_straight_line_two_calls(
        policy: FetchCacheInvalidation,
        persistent: bool,
        between: &dyn Fn(&mut LinuxDirectBoot),
    ) -> (LongModeState, [PagedCpl0SetReport; 2]) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        write_tree(&mut boot, TREE_PML4, &[(0, SPLIT_PAGE0 | 3)], None);
        let mut code = Vec::new();
        for value in 0..40u32 {
            code.extend(mov_imm32(0, value));
        }
        code.push(0xF4);
        write_phys(&mut boot, SPLIT_PAGE0 + 0x200, &code);
        let mut state = tree_state(&boot, TREE_PML4, SPLIT_VA + 0x200);
        let shared = FetchTranslationCache::persistent(policy);
        let call = |boot: &mut LinuxDirectBoot, state: &mut LongModeState, budget: u64| {
            if persistent {
                run_call_with_cache(boot, state, &set, budget, Some(&shared))
            } else {
                let fresh = FetchTranslationCache::with_invalidation(policy);
                run_call_with_cache(boot, state, &set, budget, Some(&fresh))
            }
        };
        let one = call(&mut boot, &mut state, 10);
        assert_eq!(
            one.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted)
        );
        between(&mut boot);
        let two = call(&mut boot, &mut state, 100);
        assert_eq!(two.exit, PagedCpl0SetExit::Run(RunExit::Halted), "{two:?}");
        (state, [one, two])
    }

    /// Straight-line code over two calls, in both policies: the persistent cache walks the code
    /// page once for both calls (one insert), the per-call cache once per call, and the end state
    /// and the retired count are the same.
    #[test]
    fn persistent_cache_changes_no_result_and_saves_the_second_walk() {
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let none = |_: &mut LinuxDirectBoot| {};
            let (state, reports) = run_straight_line_two_calls(policy, true, &none);
            let (plain_state, plain) = run_straight_line_two_calls(policy, false, &none);
            assert_eq!(state, plain_state, "{policy:?}");
            assert_eq!(state.gpr[Register::Rax as usize], 39);
            assert_eq!(
                reports[1].instructions_executed,
                plain[1].instructions_executed
            );
            let total = stats_of(&reports[1]);
            assert_eq!(total.inserts, 1, "{policy:?} {total:?}");
            assert_eq!(total.host_flushes, 0, "{policy:?} {total:?}");
            assert!(total.hits > stats_of(&reports[0]).hits, "{policy:?}");
            // The per-call cache of call 2 had to walk the page again.
            assert_eq!(stats_of(&plain[1]).inserts, 1);
            assert!(stats_of(&plain[1]).misses >= 1);
            assert_eq!(stats_of(&plain[1]).host_flushes, 0, "{policy:?}");
        }
    }

    /// The host rewrites the PTE of the cached code page between the calls (same value, new
    /// page generation: only the generation shows it), in both policies: call 2 starts with an
    /// empty cache and walks the page again. A write to another frame keeps the cache.
    #[test]
    fn persistent_cache_drops_entries_for_a_host_write_to_a_watched_table_frame() {
        let pte = TREE_PML4 + 0x3000;
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            // The first walk set the accessed bit: the value the PTE has now.
            let rewrite = |boot: &mut LinuxDirectBoot| {
                write_phys(boot, pte, &(SPLIT_PAGE0 | 0x23).to_le_bytes());
            };
            let (state, reports) = run_straight_line_two_calls(policy, true, &rewrite);
            assert_eq!(state.gpr[Register::Rax as usize], 39);
            let total = stats_of(&reports[1]);
            assert_eq!(total.host_flushes, 1, "{policy:?} {total:?}");
            assert_eq!(total.inserts, 2, "{policy:?} {total:?}");
            // Not a table of any entry, and not code.
            let unrelated = |boot: &mut LinuxDirectBoot| {
                write_phys(boot, SPLIT_PAGE2, &[0xAA; 8]);
            };
            let (_, reports) = run_straight_line_two_calls(policy, true, &unrelated);
            let total = stats_of(&reports[1]);
            assert_eq!((total.host_flushes, total.inserts), (0, 1), "{total:?}");
        }
    }

    /// A retired `INVLPG` at the end of call 1 drops the stale entry; call 2 fetches through the
    /// new frame. The same program with a `NOP` in its place (the persistent test above) runs the
    /// old code, so this fails if the flush does not reach the cache that crosses the boundary.
    #[test]
    fn persistent_cache_after_a_guest_invlpg_runs_the_new_code_in_the_next_call() {
        let none = |_: &mut LinuxDirectBoot, _: &FetchTranslationCache| {};
        let (state, one, two) = run_two_calls(
            &INVLPG_RBX,
            FetchCacheInvalidation::GuestOnly,
            true,
            4,
            &none,
        );
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&one).invlpg_flushes, 1);
        assert_eq!(stats_of(&two).invlpg_flushes, 1);
        assert_eq!(stats_of(&two).host_flushes, 0);
    }

    /// The write-watching policy over the boundary: the guest's PTE store in call 1 dropped the
    /// entry then, so call 2 runs the new code.
    #[test]
    fn persistent_write_watch_cache_sees_the_pte_store_of_the_previous_call() {
        let none = |_: &mut LinuxDirectBoot, _: &FetchTranslationCache| {};
        let (state, one, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::WriteWatch, true, 3, &none);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&one).write_flushes, 1);
        assert_eq!(stats_of(&two).write_flushes, 1);
    }

    /// `GuestOnly` keeps the stale translation of the guest's own PTE store over the boundary
    /// (the test above); the host then rewrites that PTE between the calls (the loader or a
    /// harness through the host API, a DMA: none of them reaches `note_write`). The generation of
    /// the table frame has moved, so call 2 starts with an empty cache and runs the new code.
    /// A write to a frame that holds no table of an entry keeps the cache.
    #[test]
    fn persistent_guest_only_cache_drops_the_stale_entry_on_a_host_pte_write() {
        let rewrite = |boot: &mut LinuxDirectBoot, _: &FetchTranslationCache| {
            write_phys(boot, TREE_PML4 + 0x3000 + 8, &(ALT_PAGE1 | 3).to_le_bytes());
        };
        let (state, _, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, true, 3, &rewrite);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&two).host_flushes, 1, "{:?}", stats_of(&two));
        let unrelated = |boot: &mut LinuxDirectBoot, _: &FetchTranslationCache| {
            write_phys(boot, SPLIT_PAGE2, &[0xAA; 8]);
        };
        let (state, _, two) = run_two_calls(
            &NOP3,
            FetchCacheInvalidation::GuestOnly,
            true,
            3,
            &unrelated,
        );
        assert_eq!(state.gpr[Register::Rdx as usize], OLD_MARKER);
        assert_eq!(stats_of(&two).host_flushes, 0);
    }

    /// A host write to the code bytes of the cached frame is seen at once: the cache holds the
    /// frame, never the byte.
    #[test]
    fn persistent_cache_reads_the_code_bytes_fresh_from_the_cached_frame() {
        // The marker immediate of the old frame: the dword of `MOV RDX, imm32` is replaced.
        let patch = |boot: &mut LinuxDirectBoot, _: &FetchTranslationCache| {
            // The frame bytes of page X after the store are `NOP3; MOV RDX, imm32; HLT`: the store
            // has one byte on X, so the immediate sits at offset 1 + 3 + 3.
            write_phys(boot, SPLIT_PAGE1 + 7, &0x7777u32.to_le_bytes());
        };
        let (state, _, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, true, 3, &patch);
        assert_eq!(state.gpr[Register::Rdx as usize], 0x7777);
        assert_eq!(stats_of(&two).host_flushes, 0);
    }

    /// The explicit host events: `flush_for_host_event` and `reset` (a cache object handed to a
    /// new machine session) both make call 2 walk the new PTE, though no table generation moved.
    #[test]
    fn persistent_cache_is_dropped_by_the_explicit_host_event_and_by_reset() {
        let flush = |_: &mut LinuxDirectBoot, cache: &FetchTranslationCache| {
            cache.flush_for_host_event();
        };
        let (state, _, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, true, 3, &flush);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&two).host_flushes, 1);
        let reset = |_: &mut LinuxDirectBoot, cache: &FetchTranslationCache| cache.reset();
        let (state, _, two) =
            run_two_calls(&NOP3, FetchCacheInvalidation::GuestOnly, true, 3, &reset);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        // The counters restarted with the reset: only call 2 is in them.
        assert_eq!(stats_of(&two).host_flushes, 0);
        assert_eq!(stats_of(&two).inserts, 1, "{:?}", stats_of(&two));
    }

    /// Two calls of the remap layout over ONE cache built with `with_invalidation` (not
    /// `persistent`): the loop seals and checks every cache it is given (S4-14i, review), so the
    /// constructor does not decide whether a host event between the calls is seen.
    fn run_two_calls_over_one_plain_cache(
        policy: FetchCacheInvalidation,
        first: u64,
        between: &dyn Fn(&mut LinuxDirectBoot),
    ) -> (LongModeState, PagedCpl0SetReport, PagedCpl0SetReport) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = remap_case(&mut boot, &[], &NOP3, None);
        let cache = FetchTranslationCache::with_invalidation(policy);
        assert!(!cache.is_persistent());
        let one = run_call_with_cache(&mut boot, &mut state, &set, first, Some(&cache));
        assert_eq!(one.instructions_executed, first);
        between(&mut boot);
        let two = run_call_with_cache(&mut boot, &mut state, &set, 16, Some(&cache));
        assert_eq!(two.exit, PagedCpl0SetExit::Run(RunExit::Halted), "{two:?}");
        (state, one, two)
    }

    /// A cache that is not `persistent` and is handed to a second call is sealed and checked like
    /// any other (it was not, before the S4-14i review: a `WriteWatch` cache then lost a host write
    /// to a table frame, against the promise that such a write always flushes). `GuestOnly`: the
    /// host rewrites the PTE of the entry's table (same value, new generation) and call 2 walks it
    /// again; before, the stale translation served call 2 and ran `OLD_MARKER`. `WriteWatch`: the
    /// guest's own store dropped the old entry in call 1, call 1 cached the new frame; the host
    /// points the PTE back to the old frame and call 2 must follow it. Without a host write the
    /// entry survives the boundary in both.
    #[test]
    fn a_cache_that_is_not_persistent_is_sealed_and_checked_like_any_other() {
        let pte_x = TREE_PML4 + 0x3000 + 8;
        let none = |_: &mut LinuxDirectBoot| {};
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            let (state, _, two) = run_two_calls_over_one_plain_cache(policy, 4, &none);
            let expected = match policy {
                // The guest store already dropped the old entry (and `sync` ran on the new frame).
                FetchCacheInvalidation::WriteWatch => NEW_MARKER,
                FetchCacheInvalidation::GuestOnly => OLD_MARKER,
            };
            assert_eq!(state.gpr[Register::Rdx as usize], expected, "{policy:?}");
            assert_eq!(stats_of(&two).host_flushes, 0, "{policy:?}");
            assert!(
                stats_of(&two).hits > 0 && stats_of(&two).inserts >= 1,
                "{policy:?} {:?}",
                stats_of(&two)
            );
        }
        // GuestOnly: a host write to the watched PTE page, after the guest's ignored store.
        let rewrite = |boot: &mut LinuxDirectBoot| {
            write_phys(boot, pte_x, &(ALT_PAGE1 | 3).to_le_bytes());
        };
        let (state, one, two) =
            run_two_calls_over_one_plain_cache(FetchCacheInvalidation::GuestOnly, 3, &rewrite);
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&one).host_flushes, 0);
        assert_eq!(stats_of(&two).host_flushes, 1, "{:?}", stats_of(&two));
        // WriteWatch: the host redirects the PTE to the old frame behind the cached new one.
        let redirect = |boot: &mut LinuxDirectBoot| {
            write_phys(boot, pte_x, &(SPLIT_PAGE1 | 3).to_le_bytes());
        };
        let (state, one, two) =
            run_two_calls_over_one_plain_cache(FetchCacheInvalidation::WriteWatch, 4, &redirect);
        assert_eq!(state.gpr[Register::Rdx as usize], OLD_MARKER);
        assert_eq!(stats_of(&one).host_flushes, 0);
        assert_eq!(stats_of(&two).host_flushes, 1, "{:?}", stats_of(&two));
    }

    /// The ordinary paths of a one-shot `WriteWatch` cache (the default of every caller) are what
    /// they were: the per-call cache of the `_observed` entry point and the S4-13 counters show no
    /// host flush, and the end state is the one of the run without any cache.
    #[test]
    fn a_one_shot_write_watch_cache_never_counts_a_host_flush() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let build = |boot: &mut LinuxDirectBoot| remap_case(boot, &[], &NOP3, None);
        let cached = run_policy_case(&build, &set, 16, FetchCacheInvalidation::WriteWatch);
        assert_halted_with_marker(&cached, NEW_MARKER);
        assert_eq!(cached.stats.host_flushes, 0, "{:?}", cached.stats);
        assert_eq!(cached.stats.write_flushes, 1, "{:?}", cached.stats);
        let plain = run_cache_case(&build, &set, 16, false);
        let with = run_cache_case(&build, &set, 16, true);
        assert_eq!(with.state, plain.state);
        assert_eq!(with.writes, plain.writes);
        assert!(with.memory == plain.memory, "frame bytes differ");
        assert_eq!(
            with.report
                .fetch_cache_stats
                .expect("cache stats")
                .host_flushes,
            0
        );
    }

    /// The host changes CR3 between the calls (not a guest `MOV CR3`, no flush of the loop) and
    /// then puts it back. The entry of the code page (`GuestOnly`: stale after the guest's PTE
    /// store, which the ignored write leaves in place) is dropped by the first check that sees the
    /// other context, so the return does not find it, though no table generation moved after the
    /// call under the other CR3 sealed its own. Without the switch the same entry serves call 2
    /// (`persistent_cache_keeps_its_entries_over_the_call_boundary`), which the key check does not
    /// touch: one flush for the call under B, one for the return to A (CR3 differs from the seal).
    #[test]
    fn the_host_switching_cr3_between_calls_drops_the_cache() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let set = switch_set(&cli, &halt);
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = remap_case(&mut boot, &[], &NOP3, None);
        let root_a = state.cr3.expect("CPU-owned CR3");
        let root_b = root_a + 0x8000;
        let cache = FetchTranslationCache::persistent(FetchCacheInvalidation::GuestOnly);
        let one = run_call_with_cache(&mut boot, &mut state, &set, 3, Some(&cache));
        assert_eq!(stats_of(&one).host_flushes, 0);
        state.cr3 = Some(root_b);
        let other = run_call_with_cache(&mut boot, &mut state, &set, 0, Some(&cache));
        assert_eq!(
            other.exit,
            PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted)
        );
        assert_eq!(stats_of(&other).host_flushes, 1, "{:?}", stats_of(&other));
        state.cr3 = Some(root_a);
        let two = run_call_with_cache(&mut boot, &mut state, &set, 16, Some(&cache));
        assert_eq!(two.exit, PagedCpl0SetExit::Run(RunExit::Halted), "{two:?}");
        assert_eq!(state.gpr[Register::Rdx as usize], NEW_MARKER);
        assert_eq!(stats_of(&two).host_flushes, 2, "{:?}", stats_of(&two));
    }

    // ---- S4-13d: the 2 MiB leaf tag of the real cache and the extended differential gate ----
    //
    // Every case runs three times over the same layout: with no cache, with the 4 KiB cache and
    // with the cache that tags a 2 MiB leaf as one entry; the three must agree on the CPU state,
    // the report (exit, fault record, context changes), every committed write in order and the
    // bytes of the frames listed by the case (page tables with their accessed/dirty bits, code
    // and data). The cache is the default `WriteWatch` one.

    use crate::fetch_translation_cache::FetchCacheTag;

    const GATE_LEAF1: u64 = 0x0600_0000;
    const GATE_LEAF2: u64 = 0x0620_0000;
    const GATE_LEAF3: u64 = 0x0640_0000;
    const GATE_FRAME3: u64 = 0x0500_a000;
    const GATE_DATA_FRAME: u64 = 0x0500_b000;
    /// Where the code of every layout starts: page 1 of the leaf, offset 0x800 (so also GVA
    /// `SPLIT_VA + 0x1000 + 0x800` for the 4 KiB layout, whose PTE 1 maps that page).
    const GATE_CODE_VA: u64 = SPLIT_VA + 0x1800;
    /// The second 2 MiB leaf (PD index 10) of the data cases.
    const GATE_OTHER_LEAF_VA: u64 = SPLIT_VA + 0x40_0000;
    const MID_MARKER: u64 = 0x2222;
    const MOV_RBX_RAX_STORE: [u8; 3] = [0x48, 0x89, 0x03];
    const MOV_RBX_RCX_STORE: [u8; 3] = [0x48, 0x89, 0x0B];

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum CacheMode {
        Plain,
        Tag4k,
        Tag2m,
    }

    struct ModeOutcome {
        state: LongModeState,
        report: PagedCpl0SetReport,
        writes: Vec<(u64, u64)>,
        memory: Vec<u8>,
        stats: Option<FetchCacheStats>,
    }

    /// One run of the case `build` lays out, with the cache mode `mode`; `frames` are the
    /// `(address, length)` ranges whose bytes are returned (concatenated) after the run.
    fn run_mode_case(
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        mode: CacheMode,
        policy: FetchCacheInvalidation,
        frames: &[(u64, usize)],
    ) -> ModeOutcome {
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let mut state = build(&mut boot);
        let efer = boot.board.efer();
        let cr3 = state.cr3.expect("the case runs with a CPU-owned CR3");
        let cache = match mode {
            CacheMode::Plain => None,
            CacheMode::Tag4k => Some(FetchTranslationCache::with_invalidation(policy)),
            CacheMode::Tag2m => Some(
                FetchTranslationCache::with_invalidation(policy).with_tag(FetchCacheTag::Leaf2M),
            ),
        };
        let mut writes = Vec::new();
        let mut report = run_cpl0_set_paged_2m_ram_with_cache(
            &mut state,
            boot.board.bus_mut(),
            crate::vcpu::BOOT_VCPU_INDEX,
            cr3,
            efer,
            long64(),
            budget,
            INTEGER_FLAGS,
            set,
            None,
            || false,
            |range| writes.push((range.start().get(), range.len())),
            true,
            cache.as_ref(),
            None,
            None,
            None,
        );
        let stats = report.fetch_cache_stats.take();
        let mut memory = Vec::new();
        for &(start, len) in frames {
            let mut bytes = vec![0u8; len];
            read(&mut boot.board, start, &mut bytes).expect("frames are RAM");
            memory.extend(bytes);
        }
        ModeOutcome {
            state,
            report,
            writes,
            memory,
            stats,
        }
    }

    struct ModesAgree {
        /// The outcome of the run with the 2 MiB tag.
        tagged: ModeOutcome,
        stats_4k: FetchCacheStats,
        stats_2m: FetchCacheStats,
        /// Instruction-fetch walks of the plain, the 4 KiB and the 2 MiB run.
        fetch_walks: [u64; 3],
    }

    /// The differential gate: no cache, the 4 KiB cache and the 2 MiB-tag cache agree on
    /// everything the guest or a device can see; data walks are equal and the caches can only
    /// remove instruction-fetch walks.
    fn assert_modes_agree(
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        frames: &[(u64, usize)],
    ) -> ModesAgree {
        assert_modes_agree_with(
            FetchCacheInvalidation::WriteWatch,
            build,
            set,
            budget,
            frames,
        )
    }

    /// [`assert_modes_agree`] with the invalidation policy of both caches chosen. Only a case in
    /// which the guest itself performs every invalidation the policy relies on may use
    /// `GuestOnly` (a TLB-like cache is allowed to differ from an uncached run otherwise).
    fn assert_modes_agree_with(
        policy: FetchCacheInvalidation,
        build: &dyn Fn(&mut LinuxDirectBoot) -> LongModeState,
        set: &Cpl0ProfileSet<'_>,
        budget: u64,
        frames: &[(u64, usize)],
    ) -> ModesAgree {
        let mut plain = run_mode_case(build, set, budget, CacheMode::Plain, policy, frames);
        let mut k4 = run_mode_case(build, set, budget, CacheMode::Tag4k, policy, frames);
        let mut k2 = run_mode_case(build, set, budget, CacheMode::Tag2m, policy, frames);
        assert_eq!(plain.stats, None);
        let walks = [
            plain.report.walk_counts,
            k4.report.walk_counts,
            k2.report.walk_counts,
        ];
        for counts in &walks[1..] {
            assert_eq!(counts.data_read, walks[0].data_read);
            assert_eq!(counts.data_write, walks[0].data_write);
            assert!(counts.fetch <= walks[0].fetch, "{counts:?} {:?}", walks[0]);
        }
        for outcome in [&mut plain, &mut k4, &mut k2] {
            outcome.report.walk_counts = Default::default();
        }
        for (name, other) in [("4k", &k4), ("2m", &k2)] {
            assert_eq!(other.report, plain.report, "{name}");
            assert_eq!(other.state, plain.state, "{name}");
            assert_eq!(other.writes, plain.writes, "{name}");
            assert!(other.memory == plain.memory, "{name}: frame bytes differ");
        }
        let stats_4k = k4.stats.take().expect("4k cache stats");
        let stats_2m = k2.stats.take().expect("2m cache stats");
        ModesAgree {
            stats_4k,
            stats_2m,
            fetch_walks: [walks[0].fetch, walks[1].fetch, walks[2].fetch],
            tagged: k2,
        }
    }

    /// The two layouts of the gate: the page the code runs on is a 4 KiB page behind a PT, or
    /// a page of a 2 MiB leaf (PD entry with the page-size bit).
    #[derive(Clone, Copy, Debug)]
    enum Leaf {
        Small,
        Large,
    }

    impl Leaf {
        /// The 4 KiB frame (`Small`, `variant` 0..3) or 2 MiB leaf (`Large`) behind the code.
        fn backing(self, variant: usize) -> u64 {
            match self {
                Leaf::Small => [SPLIT_PAGE1, ALT_PAGE1, GATE_FRAME3][variant],
                Leaf::Large => [GATE_LEAF1, GATE_LEAF2, GATE_LEAF3][variant],
            }
        }

        /// Physical address of the instruction at `GATE_CODE_VA` for the variant.
        fn code_phys(self, variant: usize) -> u64 {
            match self {
                Leaf::Small => self.backing(variant) + 0x800,
                Leaf::Large => self.backing(variant) + 0x1800,
            }
        }

        /// The bytes of the page of the code, per variant, for the frame comparison.
        fn code_frames(self) -> Vec<(u64, usize)> {
            (0..3)
                .map(|variant| (self.code_phys(variant) & !0xfff, 0x1000))
                .collect()
        }

        /// Flags of the leaf entry: present, writable, and the accessed/dirty bits asked for
        /// (plus the page-size bit for a 2 MiB leaf).
        fn flags(self, accessed: bool, dirty: bool) -> u64 {
            let base = match self {
                Leaf::Small => 0x03,
                Leaf::Large => 0x83,
            };
            base | if accessed { 0x20 } else { 0 } | if dirty { 0x40 } else { 0 }
        }

        /// The tree `pml4` (PML4 -> PDPT -> PD [-> PT]) that maps `SPLIT_VA + 0x1000` (`Small`,
        /// PTE 1) or `SPLIT_VA` (`Large`, PD entry 8) to the variant; `window` maps `WINDOW_VA`
        /// to a table frame (see `write_tree`).
        fn install(
            self,
            boot: &mut LinuxDirectBoot,
            pml4: u64,
            variant: usize,
            flags: u64,
            window: Option<u64>,
        ) {
            match self {
                Leaf::Small => {
                    write_tree(boot, pml4, &[(1, self.backing(variant) | flags)], window)
                }
                Leaf::Large => {
                    write_tree(boot, pml4, &[], window);
                    write_phys(
                        boot,
                        pml4 + 0x2000 + 8 * 8,
                        &(self.backing(variant) | flags).to_le_bytes(),
                    );
                }
            }
        }

        /// The table frame and the offset in it of the leaf entry of tree A (`TREE_PML4`).
        fn leaf_entry_of_tree_a(self) -> (u64, u64) {
            match self {
                Leaf::Small => (TREE_PML4 + 0x3000, 8),
                Leaf::Large => (TREE_PML4 + 0x2000, 8 * 8),
            }
        }
    }

    /// `head`, then `MOV RDX, marker; HLT`.
    fn with_marker(head: &[u8], marker: u64) -> Vec<u8> {
        let mut code = head.to_vec();
        code.extend(mov_imm32(2, marker as u32));
        code.push(0xF4);
        code
    }

    /// Page tables of the first tree and the second tree as `[(address, len)]` frame ranges.
    const GATE_TREE_FRAMES: [(u64, usize); 2] = [(TREE_PML4, 0x5000), (TREE2_PML4, 0x5000)];

    fn frames_of(leaf: Leaf, extra: &[(u64, usize)]) -> Vec<(u64, usize)> {
        let mut frames = GATE_TREE_FRAMES.to_vec();
        frames.extend(leaf.code_frames());
        frames.extend_from_slice(extra);
        frames
    }

    fn assert_halted(outcome: &ModeOutcome, marker: u64) {
        assert_eq!(
            outcome.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            outcome.report
        );
        assert_eq!(outcome.state.gpr[Register::Rdx as usize], marker);
    }

    // -- the 2 MiB tag itself --

    /// 640 `MOV RAX, i` (7 bytes each) straight through three 4 KiB pages of ONE 2 MiB leaf
    /// (from offset 0xf00 of the leaf to 0x2080), then `HLT`. The 4 KiB cache takes a walk per
    /// page; the 2 MiB-tag cache takes one for the leaf and serves the bytes of every page of it
    /// from the entry with the offset of each request: a wrong offset or frame would decode
    /// other bytes and end elsewhere than the uncached run, whose `RAX` is the last immediate.
    #[test]
    fn cache_2m_tag_serves_every_4k_page_of_a_leaf_from_one_entry() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let start = 0xf00u64;
        let build = |boot: &mut LinuxDirectBoot| {
            Leaf::Large.install(boot, TREE_PML4, 0, Leaf::Large.flags(true, true), None);
            let mut code = Vec::new();
            for value in 0..640u32 {
                code.extend(mov_imm32(0, value));
            }
            code.push(0xF4);
            write_phys(boot, GATE_LEAF1 + start, &code);
            tree_state(boot, TREE_PML4, SPLIT_VA + start)
        };
        let frames = [(TREE_PML4, 0x5000usize), (GATE_LEAF1, 0x3000)];
        let agreed = assert_modes_agree(&build, &switch_set(&cli, &halt), 1000, &frames);
        assert_eq!(
            agreed.tagged.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            agreed.tagged.report
        );
        assert_eq!(agreed.tagged.state.gpr[Register::Rax as usize], 639);
        assert_eq!(agreed.tagged.report.instructions_executed, 641);
        // Three pages of the leaf: three entries with the 4 KiB tag, one with the 2 MiB tag.
        assert_eq!(agreed.stats_4k.inserts, 3, "{:?}", agreed.stats_4k);
        assert_eq!(agreed.stats_2m.inserts, 1, "{:?}", agreed.stats_2m);
        assert!(agreed.stats_2m.misses < agreed.stats_4k.misses);
        assert_eq!(
            agreed.stats_2m.hits + agreed.stats_2m.misses,
            agreed.stats_4k.hits + agreed.stats_4k.misses,
            "the same lookups, only answered differently"
        );
        assert!(
            agreed.fetch_walks[2] < agreed.fetch_walks[1]
                && agreed.fetch_walks[1] < agreed.fetch_walks[0],
            "{:?}",
            agreed.fetch_walks
        );
    }

    // -- a 4 KiB leaf whose frame is congruent with its address modulo 2 MiB --

    /// Lays out a PT in which page 1 (GVA `SPLIT_VA + 0x1000`) maps the frame at
    /// `GATE_LEAF1 + 0x1000`: its physical address minus the offset of the request inside the
    /// 2 MiB region is the 2 MiB-aligned `GATE_LEAF1`, plain RAM, which is exactly what a
    /// 2 MiB leaf would look like to the cache tag. Page 2 is `page2` (another PTE). The code
    /// is one 7-byte `MOV RAX` that ends page 1 and, on page 2, `MOV RDX, marker; HLT`; the
    /// bytes at `GATE_LEAF1 + 0x2000` (where a wrongly 2 MiB-tagged entry would read page 2)
    /// are a decoy that ends with another marker.
    fn congruent_4k_leaf_case(boot: &mut LinuxDirectBoot, page2: u64) -> LongModeState {
        let congruent = GATE_LEAF1 + 0x1000;
        write_tree(boot, TREE_PML4, &[(1, congruent | 0x23), (2, page2)], None);
        write_phys(boot, congruent + 0xff9, &mov_imm32(0, 7));
        write_phys(boot, GATE_LEAF1 + 0x2000, &with_marker(&[], NEW_MARKER));
        write_phys(boot, GATE_FRAME3, &with_marker(&[], OLD_MARKER));
        tree_state(boot, TREE_PML4, SPLIT_VA + 0x1ff9)
    }

    fn congruent_4k_frames() -> Vec<(u64, usize)> {
        let mut frames = GATE_TREE_FRAMES.to_vec();
        frames.extend([
            (GATE_LEAF1 + 0x1000, 0x1000),
            (GATE_LEAF1 + 0x2000, 0x1000),
            (GATE_FRAME3, 0x1000),
        ]);
        frames
    }

    /// A 4 KiB page with a frame congruent modulo 2 MiB stays a 4 KiB entry under the 2 MiB
    /// tag: the NEXT page of the same PT maps another frame, and the fetch of it must read that
    /// frame (marker `OLD_MARKER`), not the bytes that a 2 MiB entry for `GATE_LEAF1` would
    /// serve at the same offset (`NEW_MARKER`). Two 4 KiB entries are inserted, never one leaf.
    #[test]
    fn gate_2m_congruent_4k_page_stays_a_4k_entry_and_the_next_page_is_its_own() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| congruent_4k_leaf_case(boot, GATE_FRAME3 | 0x23);
        let agreed =
            assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &congruent_4k_frames());
        assert_halted(&agreed.tagged, OLD_MARKER);
        assert_eq!(agreed.tagged.state.gpr[Register::Rax as usize], 7);
        assert_eq!(agreed.stats_2m.inserts, 2, "{:?}", agreed.stats_2m);
        assert_eq!(agreed.stats_4k.inserts, 2, "{:?}", agreed.stats_4k);
    }

    /// The same layout with the next PTE not present: the fetch of page 2 is the precise `#PF`
    /// (not present, instruction fetch: 0x10) with either cache; a 2 MiB entry for the
    /// congruent page would have run the decoy instead.
    #[test]
    fn gate_2m_congruent_4k_page_next_pte_not_present_faults_like_without_the_cache() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| congruent_4k_leaf_case(boot, 0);
        let agreed =
            assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &congruent_4k_frames());
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &agreed.tagged.report.exit else {
            panic!("{:?}", agreed.tagged.report);
        };
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x10), Some(SPLIT_VA + 0x2000))
        );
        assert_eq!(agreed.stats_2m.inserts, 1, "{:?}", agreed.stats_2m);
    }

    // -- (a) remap and NX of a 2 MiB leaf under WriteWatch --

    /// The code of a 2 MiB leaf stores into the PD entry of its own leaf (through a window onto
    /// the PD frame): `MOV RBX, window + offset; MOV RAX, value; store`. Leaf 1 holds the old
    /// marker after the store, leaf 2 the new one, at the same offsets.
    fn leaf_pde_store_case(
        boot: &mut LinuxDirectBoot,
        offset: u64,
        value: u32,
        wide: bool,
    ) -> LongModeState {
        let leaf = Leaf::Large;
        leaf.install(
            boot,
            TREE_PML4,
            0,
            leaf.flags(true, true),
            Some(TREE_PML4 + 0x2000),
        );
        let mut head = mov_imm32(3, (WINDOW_VA + offset) as u32);
        head.extend(mov_imm32(0, value));
        head.extend(if wide {
            MOV_RBX_RAX_STORE.to_vec()
        } else {
            // `MOV [RBX], EAX`: a 32-bit store of the low half of RAX.
            vec![0x89, 0x03]
        });
        for (variant, marker) in [(0, OLD_MARKER), (1, NEW_MARKER)] {
            write_phys(boot, leaf.code_phys(variant), &with_marker(&head, marker));
        }
        tree_state(boot, TREE_PML4, GATE_CODE_VA)
    }

    fn leaf_store_frames() -> Vec<(u64, usize)> {
        frames_of(Leaf::Large, &[])
    }

    /// Remap of the leaf the code runs on: the guest stores a new PD entry (another 2 MiB
    /// leaf); the next instruction is fetched through it. The 2 MiB entry covers the whole leaf,
    /// so a stale one would run the old marker. Same stop record, same state, same writes.
    #[test]
    fn gate_2m_leaf_remap_under_write_watch_runs_the_new_leaf() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let new_pde = (GATE_LEAF2 | Leaf::Large.flags(true, true)) as u32;
        let build = |boot: &mut LinuxDirectBoot| leaf_pde_store_case(boot, 8 * 8, new_pde, true);
        let agreed = assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &leaf_store_frames());
        assert_halted(&agreed.tagged, NEW_MARKER);
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.write_flushes, 1, "{stats:?}");
            assert_eq!(stats.context_flushes, 0, "{stats:?}");
            assert!(stats.hits > 0, "{stats:?}");
        }
        // The PD entry is the new one in the end.
        let pde = u64::from_le_bytes(agreed.tagged.memory[0x2040..0x2048].try_into().unwrap());
        assert_eq!(pde, GATE_LEAF2 | 0xE3);
    }

    /// NX set on the PD entry of the leaf the code runs on: the next fetch is the same precise
    /// `#PF` (present, instruction fetch: error code 0x11, linear address and saved RIP of the
    /// next instruction) with the cache of either tag: a hit would have run the instruction.
    #[test]
    fn gate_2m_leaf_nx_store_faults_like_without_the_cache() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        // The high dword of the PD entry at offset 0x40 gets bit 31 (bit 63 of the entry).
        let build =
            |boot: &mut LinuxDirectBoot| leaf_pde_store_case(boot, 8 * 8 + 4, 0x8000_0000, false);
        let agreed = assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &leaf_store_frames());
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &agreed.tagged.report.exit else {
            panic!("{:?}", agreed.tagged.report);
        };
        // MOV RBX (7) + MOV RAX (7) + the 32-bit store (2) = 16 bytes retired before the fault.
        let next = GATE_CODE_VA + 16;
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x11), Some(next))
        );
        assert_eq!(
            agreed
                .tagged
                .report
                .page_fault
                .as_ref()
                .map(|r| r.saved_rip()),
            Some(next)
        );
        assert_eq!(agreed.tagged.report.instructions_executed, 3);
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.write_flushes, 1, "{stats:?}");
        }
    }

    /// The guest clears the present bit of the PD entry of its own leaf: the next fetch is the
    /// same `#PF` (not present, instruction fetch: 0x10); a failed walk is never cached.
    #[test]
    fn gate_2m_leaf_unmap_store_faults_like_without_the_cache() {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| leaf_pde_store_case(boot, 8 * 8, 0, true);
        let agreed = assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &leaf_store_frames());
        let PagedCpl0SetExit::Run(RunExit::Fault(fault)) = &agreed.tagged.report.exit else {
            panic!("{:?}", agreed.tagged.report);
        };
        let next = GATE_CODE_VA + 17;
        assert_eq!(
            (fault.vector, fault.error_code, fault.linear_address),
            (14, Some(0x10), Some(next))
        );
        assert_eq!(agreed.tagged.report.instructions_executed, 3);
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.write_flushes, 1, "{stats:?}");
        }
    }

    // -- (b) the accessed bit cleared at every level of the path --

    #[derive(Clone, Copy, Debug)]
    enum Level {
        Pml4e,
        Pdpte,
        Pde,
    }

    /// The table frame, the offset of the entry in it and the new value (accessed bit CLEAR,
    /// same target) of the entry of `level` on the path of the code.
    fn level_entry(level: Level, leaf: Leaf) -> (u64, u64, u64) {
        match (level, leaf) {
            (Level::Pml4e, _) => (TREE_PML4, 0, (TREE_PML4 + 0x1000) | 0x03),
            (Level::Pdpte, _) => (TREE_PML4 + 0x1000, 0, (TREE_PML4 + 0x2000) | 0x03),
            (Level::Pde, Leaf::Small) => (TREE_PML4 + 0x2000, 8 * 8, (TREE_PML4 + 0x3000) | 0x03),
            (Level::Pde, Leaf::Large) => (
                TREE_PML4 + 0x2000,
                8 * 8,
                GATE_LEAF1 | leaf.flags(false, true),
            ),
        }
    }

    /// The guest rewrites an entry of the path of its own code with the accessed bit clear (the
    /// same target) through a window: the next fetch must walk again and set the bit again, as
    /// without the cache. A cache that kept the old translation would leave the bit clear in RAM.
    fn assert_clear_accessed_is_set_again(level: Level, leaf: Leaf) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (frame, offset, new_value) = level_entry(level, leaf);
        let build = |boot: &mut LinuxDirectBoot| {
            leaf.install(
                boot,
                TREE_PML4,
                0,
                leaf.flags(true, matches!(leaf, Leaf::Large)),
                Some(frame),
            );
            let mut head = mov_imm32(3, (WINDOW_VA + offset) as u32);
            head.extend(mov_imm32(0, new_value as u32));
            head.extend(MOV_RBX_RAX_STORE);
            write_phys(boot, leaf.code_phys(0), &with_marker(&head, OLD_MARKER));
            tree_state(boot, TREE_PML4, GATE_CODE_VA)
        };
        let agreed =
            assert_modes_agree(&build, &switch_set(&cli, &halt), 16, &frames_of(leaf, &[]));
        let what = format!("{level:?} {leaf:?}");
        assert_halted(&agreed.tagged, OLD_MARKER);
        let at = (frame - TREE_PML4 + offset) as usize;
        let entry = u64::from_le_bytes(agreed.tagged.memory[at..at + 8].try_into().unwrap());
        assert_eq!(entry & 0x20, 0x20, "{what}: accessed bit is set again");
        assert_eq!(
            agreed.tagged.writes.last().copied(),
            Some((frame + offset, 8)),
            "{what}: the last committed write is that accessed-bit commit"
        );
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.write_flushes, 1, "{what}: {stats:?}");
            assert!(stats.hits > 0, "{what}: {stats:?}");
        }
    }

    #[test]
    fn gate_cleared_accessed_bit_of_a_pde_is_set_again_on_the_next_fetch() {
        for leaf in [Leaf::Small, Leaf::Large] {
            assert_clear_accessed_is_set_again(Level::Pde, leaf);
        }
    }

    #[test]
    fn gate_cleared_accessed_bit_of_a_pdpte_is_set_again_on_the_next_fetch() {
        for leaf in [Leaf::Small, Leaf::Large] {
            assert_clear_accessed_is_set_again(Level::Pdpte, leaf);
        }
    }

    #[test]
    fn gate_cleared_accessed_bit_of_a_pml4e_is_set_again_on_the_next_fetch() {
        for leaf in [Leaf::Small, Leaf::Large] {
            assert_clear_accessed_is_set_again(Level::Pml4e, leaf);
        }
    }

    // -- (c) the dirty bit committed in a table of the cached code --

    /// The code stores to data whose leaf entry has the dirty bit clear: the walk of the store
    /// commits the bit into the table frame the cached code translation watches. The commit is a
    /// write to a watched frame: the cache drops the entry (one write flush), the next fetch
    /// walks again, and the dirty bit, the data and the committed writes equal the uncached run.
    /// `same_page`: the data is on the page of the code (the same entry); else on another page
    /// of the same PT (`Small`) or in another 2 MiB leaf of the same PD (`Large`).
    fn assert_dirty_commit_in_the_table_of_the_code(leaf: Leaf, same_page: bool) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (data_va, data_phys, entry_at) = match (leaf, same_page) {
            (_, true) => {
                let (frame, offset) = leaf.leaf_entry_of_tree_a();
                (
                    GATE_CODE_VA + 0x100,
                    leaf.code_phys(0) + 0x100,
                    frame + offset,
                )
            }
            (Leaf::Small, false) => (
                SPLIT_VA + 0x2000 + 0x100,
                GATE_DATA_FRAME + 0x100,
                TREE_PML4 + 0x3000 + 2 * 8,
            ),
            (Leaf::Large, false) => (
                GATE_OTHER_LEAF_VA + 0x100,
                GATE_LEAF2 + 0x100,
                TREE_PML4 + 0x2000 + 10 * 8,
            ),
        };
        let build = |boot: &mut LinuxDirectBoot| {
            leaf.install(boot, TREE_PML4, 0, leaf.flags(true, false), None);
            if !same_page {
                match leaf {
                    Leaf::Small => write_phys(
                        boot,
                        TREE_PML4 + 0x3000 + 2 * 8,
                        &(GATE_DATA_FRAME | leaf.flags(true, false)).to_le_bytes(),
                    ),
                    Leaf::Large => write_phys(
                        boot,
                        TREE_PML4 + 0x2000 + 10 * 8,
                        &(GATE_LEAF2 | leaf.flags(true, false)).to_le_bytes(),
                    ),
                }
            }
            let mut head = mov_imm32(3, data_va as u32);
            head.extend(mov_imm32(0, 0x5a5a));
            head.extend(MOV_RBX_RAX_STORE);
            write_phys(boot, leaf.code_phys(0), &with_marker(&head, OLD_MARKER));
            tree_state(boot, TREE_PML4, GATE_CODE_VA)
        };
        let data_frame = (data_phys & !0xfff, 0x1000usize);
        let agreed = assert_modes_agree(
            &build,
            &switch_set(&cli, &halt),
            16,
            &frames_of(leaf, &[data_frame]),
        );
        let what = format!("{leaf:?} same_page={same_page}");
        assert_halted(&agreed.tagged, OLD_MARKER);
        let memory = &agreed.tagged.memory;
        let entry_index = (entry_at - TREE_PML4) as usize;
        let entry = u64::from_le_bytes(memory[entry_index..entry_index + 8].try_into().unwrap());
        assert_eq!(entry & 0x40, 0x40, "{what}: the dirty bit is committed");
        let data_index = memory.len() - 0x1000 + (data_phys & 0xfff) as usize;
        assert_eq!(
            &memory[data_index..data_index + 2],
            &[0x5a, 0x5a],
            "{what}: the data is stored"
        );
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.write_flushes, 1, "{what}: {stats:?}");
            assert!(
                stats.inserts >= 2,
                "{what}: refilled after the flush {stats:?}"
            );
        }
    }

    #[test]
    fn gate_dirty_commit_on_the_page_of_the_cached_code_matches_the_uncached_run() {
        for leaf in [Leaf::Small, Leaf::Large] {
            assert_dirty_commit_in_the_table_of_the_code(leaf, true);
        }
    }

    #[test]
    fn gate_dirty_commit_for_another_page_of_the_same_table_matches_the_uncached_run() {
        for leaf in [Leaf::Small, Leaf::Large] {
            assert_dirty_commit_in_the_table_of_the_code(leaf, false);
        }
    }

    // -- (d) CR3 A -> B -> A with the tables of A edited in between --

    /// One run: the code starts under tree A on the page behind variant 0, loads tree B
    /// (`MOV CR3`), where the same linear page is variant 1; in B it rewrites the entry of that
    /// page in tree A (through a window onto A's table) to variant 2 and loads A again. The
    /// instruction after the second `MOV CR3` must come from variant 2: neither the entry cached
    /// under A before the first switch (variant 0, an old translation) nor the one cached under
    /// B (variant 1) may serve it.
    fn assert_cr3_a_b_a_runs_the_edited_tables_of_a(leaf: Leaf, policy: FetchCacheInvalidation) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (frame, offset) = leaf.leaf_entry_of_tree_a();
        let new_entry = leaf.backing(2) | leaf.flags(true, matches!(leaf, Leaf::Large));
        let mut prefix = mov_imm32(0, TREE2_PML4 as u32);
        prefix.extend(MOV_CR3_RAX);
        let mut b_phase = mov_imm32(3, (WINDOW_VA + offset) as u32);
        b_phase.extend(mov_imm32(1, new_entry as u32));
        b_phase.extend(MOV_RBX_RCX_STORE);
        b_phase.extend(mov_imm32(0, TREE_PML4 as u32));
        b_phase.extend(MOV_CR3_RAX);
        let (after_first, after_second) = (prefix.len(), prefix.len() + b_phase.len());
        let build = |boot: &mut LinuxDirectBoot| {
            let dirty = matches!(leaf, Leaf::Large);
            leaf.install(boot, TREE_PML4, 0, leaf.flags(true, dirty), None);
            leaf.install(boot, TREE2_PML4, 1, leaf.flags(true, dirty), Some(frame));
            // Variant 0 (tree A, old): the prefix, and the OLD marker wherever it would be fetched
            // from after a switch that was not honoured or a stale entry.
            let old = leaf.code_phys(0);
            write_phys(boot, old, &prefix);
            write_phys(
                boot,
                old + after_first as u64,
                &with_marker(&[], OLD_MARKER),
            );
            write_phys(
                boot,
                old + after_second as u64,
                &with_marker(&[], OLD_MARKER),
            );
            // Variant 1 (tree B): the B phase, then the MID marker if A were not restored.
            let mid = leaf.code_phys(1);
            write_phys(boot, mid + after_first as u64, &b_phase);
            write_phys(
                boot,
                mid + after_second as u64,
                &with_marker(&[], MID_MARKER),
            );
            // Variant 2 (tree A, edited): the NEW marker.
            write_phys(
                boot,
                leaf.code_phys(2) + after_second as u64,
                &with_marker(&[], NEW_MARKER),
            );
            tree_state(boot, TREE_PML4, GATE_CODE_VA)
        };
        let agreed = assert_modes_agree_with(
            policy,
            &build,
            &switch_set(&cli, &halt),
            32,
            &frames_of(leaf, &[]),
        );
        let what = format!("{leaf:?} {policy:?}");
        assert_halted(&agreed.tagged, NEW_MARKER);
        assert_eq!(agreed.tagged.report.cr3, TREE_PML4, "{what}");
        assert_eq!(
            agreed.tagged.report.translation_context_changes.len(),
            2,
            "{what}"
        );
        for stats in [agreed.stats_4k, agreed.stats_2m] {
            assert_eq!(stats.context_flushes, 2, "{what}: {stats:?}");
            assert!(stats.hits > 0, "{what}: {stats:?}");
            // `GuestOnly` never drops an entry for a table write: `MOV CR3` is the only thing that
            // keeps the old translation of A from being served after A's table was edited.
            if policy == FetchCacheInvalidation::GuestOnly {
                assert_eq!(stats.write_flushes, 0, "{what}: {stats:?}");
            }
        }
    }

    /// Under `WriteWatch` the store into A's table also drops the entries that watch it; under
    /// `GuestOnly` (a TLB: the store drops nothing) only the guest's `MOV CR3` does, so the second
    /// policy is the one that fails if the flush at `MOV CR3` is lost or the key ignores CR3.
    #[test]
    fn gate_cr3_a_b_a_with_a_4k_page_runs_the_edited_tables_of_a() {
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            assert_cr3_a_b_a_runs_the_edited_tables_of_a(Leaf::Small, policy);
        }
    }

    #[test]
    fn gate_cr3_a_b_a_with_a_2m_leaf_runs_the_edited_tables_of_a() {
        for policy in [
            FetchCacheInvalidation::WriteWatch,
            FetchCacheInvalidation::GuestOnly,
        ] {
            assert_cr3_a_b_a_runs_the_edited_tables_of_a(Leaf::Large, policy);
        }
    }

    // -- the board: the option, the report and the stop record --

    /// One board run of the CR3-switch program (two 4 KiB trees) with the report on.
    fn run_board_with_tag(
        mode: CacheMode,
        enable_tag: bool,
    ) -> (String, crate::LinuxFetchCacheReport) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        boot.board.enable_fetch_cache_report();
        if enable_tag {
            boot.board.enable_fetch_cache_leaf_2m_tag();
        }
        let report = boot
            .board
            .run_linux_boot_cpl0_set(
                64,
                &switch_set(&cli, &halt),
                false,
                mode != CacheMode::Plain,
                false,
            )
            .expect("runs");
        let fetch = boot.board.take_fetch_cache_report().expect("requested");
        (format!("{report:?}"), fetch)
    }

    /// The tag is off unless asked: the report says so; with it the run report is the same text
    /// as without the cache and with the 4 KiB cache; asking for the tag with the cache off
    /// changes nothing and reports `false`. This checks only the opt-in and the equality of the
    /// `Debug` text of the report; it does not catch a wrong offset or frame of a 2 MiB entry
    /// (the synthetic `gate_2m_*` and `cache_2m_*` cases do).
    #[test]
    fn the_2m_tag_is_opt_in_on_the_board_and_leaves_the_run_report_unchanged() {
        let (plain, plain_fetch) = run_board_with_tag(CacheMode::Plain, false);
        let (plain_tagged, plain_tagged_fetch) = run_board_with_tag(CacheMode::Plain, true);
        let (default, default_fetch) = run_board_with_tag(CacheMode::Tag4k, false);
        let (tagged, tagged_fetch) = run_board_with_tag(CacheMode::Tag2m, true);
        assert!(!plain_fetch.leaf_2m_tag && !default_fetch.leaf_2m_tag);
        assert!(!plain_tagged_fetch.leaf_2m_tag, "no cache, no tag");
        assert!(tagged_fetch.leaf_2m_tag);
        for text in [&plain_tagged, &default, &tagged] {
            assert_eq!(text, &plain);
        }
        let (d, t) = (default_fetch.total.walks, tagged_fetch.total.walks);
        assert_eq!((d.data_read, d.data_write), (t.data_read, t.data_write));
        assert!(t.fetch <= d.fetch, "{t:?} {d:?}");
    }

    /// The same check on the real kernel: the 2M-instruction prefix of the Linux board run ends
    /// with the same stop record and leaves byte-identical guest RAM (hence identical page-table
    /// accessed/dirty bits) with the fetch translation cache on and off.
    /// `OD_REAL_ARTIFACTS_ROOT` names the checkout that holds `work/stage4` (default: this one).
    #[test]
    #[ignore = "needs work/stage4 artifacts (vmlinuz, initramfs, ACPI blob v2)"]
    fn cache_real_kernel_prefix_leaves_identical_ram_and_record() {
        let root = std::env::var_os("OD_REAL_ARTIFACTS_ROOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
        let load = |relative: &str| std::fs::read(root.join(relative)).expect(relative);
        let vmlinuz = load("work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic");
        let initramfs = load("work/stage4/initramfs/initramfs.cpio");
        let blob = load("tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin");
        // The runner's command line for the nonce of the evidence.
        let cmdline = "console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 \
                       od.nonce=s4-38l4a nokaslr";
        let machine_input = LinuxDirectInput {
            bzimage: &vmlinuz,
            initrd: &initramfs,
            cmdline: cmdline.as_bytes(),
            acpi_blob: &blob,
            expected_acpi_manifest_hash: board_hash(),
            ram_bytes: 128 * MIB,
        };
        let mut runs = Vec::new();
        for fetch_cache in [false, true] {
            let mut boot = load_linux_direct_board(&machine_input).expect("loads");
            let record = run_linux_direct_board_set(
                &mut boot,
                &machine_input,
                2_000_000,
                LinuxCpl0SetMembers {
                    fetch_cache,
                    ..LinuxCpl0SetMembers::linux_default()
                },
            )
            .expect("runs");
            // A hash of the hashes of the whole guest RAM, 1 MiB at a time.
            let mut chunk_hashes = Vec::new();
            let mut chunk = vec![0u8; MIB as usize];
            for start in (0..128 * MIB).step_by(MIB as usize) {
                read(&mut boot.board, start, &mut chunk).expect("RAM");
                chunk_hashes.extend(crate::acpi_blob::sha256(&chunk));
            }
            runs.push((
                format!("{record:?}"),
                crate::acpi_blob::sha256(&chunk_hashes),
            ));
        }
        assert_eq!(runs[0].0, runs[1].0, "stop records differ");
        assert_eq!(runs[0].1, runs[1].1, "guest RAM differs");
    }

    // ---- S4-38L8b: the interrupt source hook of the CPL0 set board ----

    mod irq_hook {
        use super::*;
        use crate::irq_source::{BoardIrqSource, IrqBoundaryHook, IrqCounters};
        use crate::paged_memory_adapter::IrqDeliveryFailure;
        use od_isa::{Cpl0ExternalInterruptPhase, Cpl0ExternalInterruptRefusal, StiShadow};
        use std::sync::{Arc, Mutex};

        /// What the scripted source does, shared with the test that owns it.
        #[derive(Default)]
        struct Script {
            pending: Option<u8>,
            pending_after_queries: u64,
            /// Stays pending after the acknowledge (a level line).
            keep_pending: bool,
            acked: Vec<u8>,
            deadline: Option<u64>,
            pit_ends: bool,
            polls: u64,
        }

        #[derive(Clone, Default)]
        struct ScriptedSource(Arc<Mutex<Script>>);

        impl BoardIrqSource for ScriptedSource {
            fn pending_vector(&self) -> Option<u8> {
                let mut script = self.0.lock().unwrap();
                if script.pending_after_queries > 0 {
                    script.pending_after_queries -= 1;
                    None
                } else {
                    script.pending
                }
            }
            fn acknowledge(&mut self, vector: u8) {
                let mut script = self.0.lock().unwrap();
                script.acked.push(vector);
                if !script.keep_pending {
                    script.pending = None;
                }
            }
            fn next_deadline_ns(&self) -> Option<u64> {
                let mut script = self.0.lock().unwrap();
                script.polls += 1;
                script.deadline
            }
            fn pit_write_ends_quantum(&self) -> bool {
                self.0.lock().unwrap().pit_ends
            }
        }

        pub(super) const IRQ_IDT_AT: u64 = DATA + 0x2000;
        pub(super) const IRQ_HANDLER_AT: u64 = ENTRY + 0x100;
        pub(super) const IRQ_VECTOR: u8 = 0x30;
        /// `CLD`: a one-byte instruction the minimal set retires.
        pub(super) const CLD: u8 = 0xFC;

        /// A board whose entry is `code`, with an IDT whose gate `IRQ_VECTOR` is an interrupt gate
        /// to a handler of `CLD`s, IF = 1, the loader's `Clear` shadow and `Running` activity.
        pub(super) fn irq_board(code: &[u8]) -> (LinuxDirectBoot, Vec<u8>) {
            let (mut boot, file) = boot_with_entry(code);
            write_phys(&mut boot, IRQ_HANDLER_AT, &[CLD; 8]);
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(IRQ_HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((IRQ_HANDLER_AT >> 16) as u16).to_le_bytes());
            write_phys(&mut boot, IRQ_IDT_AT + u64::from(IRQ_VECTOR) * 16, &gate);
            let state = boot.board.state_mut();
            state.idtr = Some(od_isa::state::Idtr {
                base: IRQ_IDT_AT,
                limit: 0xFFF,
            });
            state.rflags = 2 | (1 << 9);
            assert_eq!(state.sti_shadow, Some(StiShadow::Clear));
            assert_eq!(state.activity, Some(CpuActivity::Running));
            (boot, file)
        }

        /// One call of the set loop with the hook (the CPUID boundary off, no fetch cache).
        fn run_hook(
            boot: &mut LinuxDirectBoot,
            budget: u64,
            source: &mut dyn BoardIrqSource,
            counters: &mut IrqCounters,
            skip_first_boundary: bool,
        ) -> (LongModeState, PagedCpl0SetReport) {
            run_hook_with_mirror(boot, budget, source, counters, skip_first_boundary, None)
        }

        fn run_hook_with_mirror(
            boot: &mut LinuxDirectBoot,
            budget: u64,
            source: &mut dyn BoardIrqSource,
            counters: &mut IrqCounters,
            skip_first_boundary: bool,
            delivered_mirror: Option<&std::cell::Cell<u64>>,
        ) -> (LongModeState, PagedCpl0SetReport) {
            let cli = CliProfile::NonPviLong64V1;
            let sti = StiProfile::NonPviLong64V1;
            let set = minimal_set(&cli)
                .with_pio_u8()
                .with_sti(&sti)
                .with_user_mode();
            let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
            let mut state = boot.board.state().clone();
            state.cr3 = Some(cr3);
            let report = run_cpl0_set_paged_2m_ram_with_cpuid_boundary(
                &mut state,
                boot.board.bus_mut(),
                cr3,
                efer,
                long64(),
                budget,
                INTEGER_FLAGS,
                &set,
                None,
                || false,
                |_| {},
                false,
                false,
                Some(IrqBoundaryHook {
                    source,
                    counters,
                    delivered_mirror,
                    skip_first_boundary,
                }),
            );
            (state, report)
        }

        #[test]
        fn members_irq_is_on_in_linux_default_off_in_minimal_and_listed_last() {
            assert!(LinuxCpl0SetMembers::linux_default().irq);
            assert!(!LinuxCpl0SetMembers::minimal().irq);
            let (mut boot, file) = boot_with_entry(&[CLD]);
            let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::linux_default());
            assert_eq!(record.set_members.last().map(String::as_str), Some("irq"));
            let (mut boot, file) = boot_with_entry(&[CLD]);
            let record = run_set_facade(&mut boot, &file, 1, LinuxCpl0SetMembers::minimal());
            assert!(!record.set_members.iter().any(|name| name == "irq"));
        }

        #[test]
        fn the_empty_source_changes_nothing_and_the_counters_are_zero() {
            let program = com1_program(b'X');
            let mut runs = Vec::new();
            for irq in [false, true] {
                let (mut boot, file) = boot_with_entry(&program);
                let record = run_set_facade(
                    &mut boot,
                    &file,
                    64,
                    LinuxCpl0SetMembers {
                        irq,
                        ..LinuxCpl0SetMembers::linux_default()
                    },
                );
                assert_eq!(record.com1, b"X");
                assert_eq!(
                    (
                        record.irqs_delivered,
                        record.irq0_edges,
                        record.irq_refused_masked,
                        record.irq_refused_shadow,
                        record.quantum_cut_by_deadline
                    ),
                    (0, 0, 0, 0, 0)
                );
                runs.push((
                    record.exit,
                    record.exit_detail,
                    record.set_exit_detail,
                    record.retired,
                    record.quanta,
                    record.rip,
                    record.rflags,
                    record.gpr,
                    record.virtual_ns,
                    record.com1,
                    record.cr3,
                ));
            }
            assert_eq!(runs[0], runs[1]);
        }

        #[test]
        fn a_pending_vector_is_delivered_at_the_boundary_and_acknowledged_after() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            // The delivery retires nothing; the budget of 1 is the handler's first `CLD`.
            let (state, report) = run_hook(&mut boot, 1, &mut source, &mut counters, false);
            assert_eq!(report.instructions_executed, 1, "{report:?}");
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(script.0.lock().unwrap().acked, [IRQ_VECTOR]);
            assert_eq!(state.rip, IRQ_HANDLER_AT + 1);
            assert_eq!(state.rflags & (1 << 9), 0, "an interrupt gate clears IF");
            assert_eq!(state.gpr[Register::Rsp as usize], (rsp & !15) - 40);
            // The frame: RIP = the interrupted instruction, CS, RFLAGS with IF, old RSP, SS.
            let frame = state.gpr[Register::Rsp as usize];
            assert_eq!(read_qword(&mut boot, frame), ENTRY);
            assert_eq!(read_qword(&mut boot, frame + 8), 0x10);
            assert_ne!(read_qword(&mut boot, frame + 16) & (1 << 9), 0);
            assert_eq!(read_qword(&mut boot, frame + 24), rsp);
        }

        #[test]
        fn an_empty_boundary_asks_once_and_changes_nothing() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            let rip_before = boot.board.state().rip;
            let script = ScriptedSource::default();
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let (state, report) = run_hook(&mut boot, 3, &mut source, &mut counters, false);
            assert_eq!(report.instructions_executed, 3);
            assert_eq!(state.rip, rip_before + 3);
            assert_eq!(counters, IrqCounters::default());
            assert!(script.0.lock().unwrap().acked.is_empty());
        }

        #[test]
        fn interrupts_masked_refuses_without_acknowledge_and_counts_every_boundary() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            boot.board.state_mut().rflags = 2;
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let (state, report) = run_hook(&mut boot, 3, &mut source, &mut counters, false);
            // Boundaries 0, 1 and 2; the budget check precedes the interrupt check at 3.
            assert_eq!(report.instructions_executed, 3);
            assert_eq!(counters.irq_refused_masked, 3);
            assert_eq!(counters.irq_refused_shadow, 0);
            assert_eq!(counters.irqs_delivered, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
            assert_eq!(
                script.0.lock().unwrap().pending,
                Some(IRQ_VECTOR),
                "still pending"
            );
            assert_eq!(state.rip, ENTRY + 3);
        }

        #[test]
        fn the_sti_shadow_refuses_once_then_the_vector_is_delivered() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            boot.board.state_mut().sti_shadow = Some(StiShadow::Blocking);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            // Boundary 0: shadow, refused; `CLD` retires and ends the shadow; boundary 1:
            // delivered; the second retired instruction is the handler's first.
            let (state, report) = run_hook(&mut boot, 2, &mut source, &mut counters, false);
            assert_eq!(report.instructions_executed, 2, "{report:?}");
            assert_eq!(counters.irq_refused_shadow, 1);
            assert_eq!(counters.irq_refused_masked, 0);
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(script.0.lock().unwrap().acked, [IRQ_VECTOR]);
            assert_eq!(state.rip, IRQ_HANDLER_AT + 1);
        }

        #[test]
        fn an_unknown_shadow_refuses_like_a_shadow() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            boot.board.state_mut().sti_shadow = None;
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let (_, _report) = run_hook(&mut boot, 1, &mut source, &mut counters, false);
            assert_eq!(counters.irq_refused_shadow, 1);
            assert_eq!(counters.irqs_delivered, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
        }

        #[test]
        fn the_first_boundary_of_a_resumed_partial_rep_is_skipped() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            let script = ScriptedSource::default();
            {
                let mut script = script.0.lock().unwrap();
                script.pending = Some(IRQ_VECTOR);
                script.keep_pending = true;
            }
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            // Boundary 0 is skipped: `CLD` retires and the budget ends the call.
            let (state, report) = run_hook(&mut boot, 1, &mut source, &mut counters, true);
            assert_eq!(report.instructions_executed, 1);
            assert_eq!(state.rip, ENTRY + 1);
            assert_eq!(counters, IrqCounters::default());
            assert!(script.0.lock().unwrap().acked.is_empty());
            // Without the skip the same pending vector is delivered at once.
            let (mut boot, _) = irq_board(&[CLD; 8]);
            let mut counters = IrqCounters::default();
            let (state, _) = run_hook(&mut boot, 1, &mut source, &mut counters, false);
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(state.rip, IRQ_HANDLER_AT + 1);
        }

        #[test]
        fn a_gate_that_cannot_deliver_stops_with_an_error_and_acknowledges_nothing() {
            // No gate written for the vector: P = 0.
            let (mut boot, _) = irq_board(&[CLD; 8]);
            write_phys(
                &mut boot,
                IRQ_IDT_AT + u64::from(IRQ_VECTOR) * 16,
                &[0u8; 16],
            );
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let before = boot.board.state().clone();
            let (state, report) = run_hook(&mut boot, 4, &mut source, &mut counters, false);
            assert_eq!(
                report.exit,
                PagedCpl0SetExit::IrqDeliveryFailed {
                    vector: IRQ_VECTOR,
                    failure: IrqDeliveryFailure::Refused(
                        Cpl0ExternalInterruptRefusal::GateNotPresent
                    ),
                },
                "{report:?}"
            );
            assert_eq!(report.instructions_executed, 0);
            assert_eq!(counters.irqs_delivered, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
            assert_eq!(
                (state.rip, state.gpr, state.rflags),
                (before.rip, before.gpr, before.rflags)
            );
        }

        #[test]
        fn a_failed_idt_read_is_aborted_not_silent() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            // The IDT lies in an unmapped page: the gate read faults.
            boot.board.state_mut().idtr = Some(od_isa::state::Idtr {
                base: PF_VA,
                limit: 0xFFF,
            });
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let (_, report) = run_hook(&mut boot, 4, &mut source, &mut counters, false);
            let PagedCpl0SetExit::IrqDeliveryFailed { vector, failure } = &report.exit else {
                panic!("{report:?}");
            };
            assert_eq!(*vector, IRQ_VECTOR);
            assert!(
                matches!(
                    failure,
                    IrqDeliveryFailure::Aborted {
                        phase: Cpl0ExternalInterruptPhase::IdtGateRead,
                        ..
                    }
                ),
                "{failure:?}"
            );
            assert_eq!(report.instructions_executed, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
        }

        #[test]
        fn a_failed_frame_store_is_aborted_and_leaves_the_cpu_state_unchanged() {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            // The stack lies in an unmapped page: the first frame store faults.
            boot.board.state_mut().gpr[Register::Rsp as usize] = PF_VA + 0x1000;
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let (mut source, mut counters) = (script.clone(), IrqCounters::default());
            let before = boot.board.state().clone();
            let (state, report) = run_hook(&mut boot, 4, &mut source, &mut counters, false);
            let PagedCpl0SetExit::IrqDeliveryFailed { vector, failure } = &report.exit else {
                panic!("{report:?}");
            };
            assert_eq!(*vector, IRQ_VECTOR);
            assert!(
                matches!(
                    failure,
                    IrqDeliveryFailure::Aborted {
                        phase: Cpl0ExternalInterruptPhase::StackWrite { .. },
                        ..
                    }
                ),
                "{failure:?}"
            );
            assert_eq!(report.instructions_executed, 0);
            assert_eq!(counters.irqs_delivered, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
            assert_eq!(
                (state.rip, state.gpr, state.rflags),
                (before.rip, before.gpr, before.rflags)
            );
        }

        #[test]
        fn the_board_reports_a_failed_delivery_as_a_run_error() {
            let (mut boot, file) = irq_board(&[CLD; 8]);
            write_phys(
                &mut boot,
                IRQ_IDT_AT + u64::from(IRQ_VECTOR) * 16,
                &[0u8; 16],
            );
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            boot.board.set_irq_source(Box::new(script.clone()));
            let error = run_linux_direct_board_set(
                &mut boot,
                &input(&file, &[], BLOB, board_hash()),
                8,
                LinuxCpl0SetMembers::linux_default(),
            )
            .expect_err("the delivery is refused");
            assert_eq!(error.code, "run_failed");
            assert!(
                error.detail.contains("ExternalInterruptDelivery"),
                "{error:?}"
            );
            assert!(
                error.detail.contains(&format!("vector: {}", IRQ_VECTOR)),
                "{error:?}"
            );
            assert!(
                error
                    .detail
                    .contains("failure: \"Refused(GateNotPresent)\""),
                "{error:?}"
            );
            assert!(error.detail.contains("rip: "), "{error:?}");
            assert!(error.detail.contains("cs_selector: Some("), "{error:?}");
            assert!(script.0.lock().unwrap().acked.is_empty());
            // Without the `irq` member the same source is never asked.
            let (mut boot, file) = irq_board(&[CLD; 8]);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            boot.board.set_irq_source(Box::new(script.clone()));
            let record = run_set_facade(
                &mut boot,
                &file,
                2,
                LinuxCpl0SetMembers {
                    irq: false,
                    ..LinuxCpl0SetMembers::linux_default()
                },
            );
            assert_eq!(record.irqs_delivered, 0);
            assert!(script.0.lock().unwrap().acked.is_empty());
            assert_eq!(script.0.lock().unwrap().polls, 0);
        }

        #[test]
        fn the_board_delivers_and_counts_through_the_facade() {
            let (mut boot, file) = irq_board(&[CLD; 8]);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            boot.board.set_irq_source(Box::new(script.clone()));
            let record = run_set_facade(&mut boot, &file, 3, LinuxCpl0SetMembers::linux_default());
            assert_eq!(record.irqs_delivered, 1);
            assert_eq!(script.0.lock().unwrap().acked, [IRQ_VECTOR]);
            assert_eq!(record.rip, IRQ_HANDLER_AT + 3);
            assert_eq!(record.retired, 3);
        }

        #[test]
        fn delivered_irq_mirror_updates_at_successful_delivery() {
            let (mut boot, _file) = irq_board(&[CLD; 8]);
            let mut script = ScriptedSource::default();
            script.0.lock().unwrap().pending = Some(IRQ_VECTOR);
            let mut counters = IrqCounters::default();
            let delivered = std::cell::Cell::new(0);
            let (_state, _report) = run_hook_with_mirror(
                &mut boot,
                3,
                &mut script,
                &mut counters,
                false,
                Some(&delivered),
            );
            assert_eq!(counters.irqs_delivered, 1);
            assert_eq!(delivered.get(), counters.irqs_delivered);
        }

        #[test]
        fn a_pending_irq_at_cpl3_switches_to_rsp0_and_user_code_resumes_after_iretq() {
            let (mut boot, _file) = user_mode_board(
                &[
                    0xb8, 0x53, 0, 0, 0, // MOV EAX, 0x53
                    0xb8, 0x54, 0, 0, 0, // MOV EAX, 0x54
                    0xb8, 0x55, 0, 0, 0, // MOV EAX, 0x55
                ],
                true,
            );
            write_phys(
                &mut boot,
                USER_GDT + 0x08,
                &0x00af_9b00_0000_ffff_u64.to_le_bytes(),
            );
            write_phys(&mut boot, USER_HANDLER_AT, &[0x48, 0xcf]);
            write_phys(&mut boot, USER_TSS_AT + 4, &USER_RSP0.to_le_bytes());
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(USER_HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((USER_HANDLER_AT >> 16) as u16).to_le_bytes());
            write_phys(&mut boot, USER_IDT_AT + u64::from(IRQ_VECTOR) * 16, &gate);
            let state = boot.board.state_mut();
            state.idtr = Some(od_isa::state::Idtr {
                base: USER_IDT_AT,
                limit: 0xFFF,
            });
            state.tr = Some(od_isa::TaskRegisterCache {
                selector: 0x20,
                base: USER_TSS_AT,
                limit: 0x67,
                access: 0x8B,
                flags: 0,
            });

            let source = ScriptedSource::default();
            {
                let mut script = source.0.lock().unwrap();
                // First boundary is the kernel IRETQ; the next boundary is in CPL3.
                script.pending_after_queries = 1;
                script.pending = Some(IRQ_VECTOR);
            }
            let mut counters = IrqCounters::default();
            let (state, report) = run_hook(&mut boot, 5, &mut source.clone(), &mut counters, false);

            assert_eq!(
                report.exit,
                PagedCpl0SetExit::Run(RunExit::InstructionBudgetExhausted),
                "{report:?}"
            );
            assert_eq!(counters.irqs_delivered, 1, "{report:?}");
            assert_eq!(source.0.lock().unwrap().acked, [IRQ_VECTOR]);
            assert_eq!(report.instructions_executed, 5, "{report:?}");
            assert_eq!(report.cpl3_retired, 3, "{report:?}");
            assert_eq!(state.cs_selector, Some(0x13), "{report:?}");
            assert_eq!(state.rip, USER_CODE_AT + 15, "{report:?}");
            assert_eq!(state.gpr[Register::Rax as usize], 0x55);
            assert_eq!(state.gpr[Register::Rsp as usize], ENTRY + 0x800);
            let frame = USER_RSP0 & !15;
            assert_eq!(state.gpr[Register::Rsp as usize] & 15, 0);
            let mut raw = [0u8; 8];
            for (address, expected) in [
                (frame - 8, 0x1b),
                (frame - 16, ENTRY + 0x800),
                (frame - 24, 0x202),
                (frame - 32, 0x13),
                (frame - 40, USER_CODE_AT),
            ] {
                read(&mut boot.board, address, &mut raw).expect("interrupt frame");
                assert_eq!(u64::from_le_bytes(raw), expected, "{address:#x}");
            }
        }

        #[test]
        fn a_deadline_shortens_the_quantum_and_the_run_resumes_to_the_budget() {
            let program = [CLD; 12];
            // Without a deadline the whole budget is one quantum.
            let (mut boot, file) = boot_with_entry(&program);
            let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::linux_default());
            assert_eq!((record.retired, record.quanta), (8, 1));
            assert_eq!(record.quantum_cut_by_deadline, 0);
            // A deadline 5 ns ahead: 5 instructions, then the time is up and every quantum is
            // one instruction (the minimum) until the budget is spent.
            let (mut boot, file) = boot_with_entry(&program);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().deadline = Some(boot.board.clock().now_ns() + 5);
            boot.board.set_irq_source(Box::new(script.clone()));
            let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::linux_default());
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!(record.retired, 8);
            // Quanta of 5, 1, 1 (each cut) and the last 1 that is the whole rest (not a cut).
            assert_eq!((record.quanta, record.quantum_cut_by_deadline), (4, 3));
            assert_eq!(record.rip, ENTRY + 8);
            // The deadline is not consulted without the member.
            let (mut boot, file) = boot_with_entry(&program);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().deadline = Some(boot.board.clock().now_ns() + 5);
            boot.board.set_irq_source(Box::new(script.clone()));
            let record = run_set_facade(
                &mut boot,
                &file,
                8,
                LinuxCpl0SetMembers {
                    irq: false,
                    ..LinuxCpl0SetMembers::linux_default()
                },
            );
            assert_eq!((record.quanta, record.quantum_cut_by_deadline), (1, 0));
            assert_eq!(script.0.lock().unwrap().polls, 0);
        }

        /// `MOV AL, 0x34; MOV DX, 0x43; OUT DX, AL; CLD; CLD; CLD; CLD`.
        const PIT_WRITE_PROGRAM: [u8; 11] =
            [0xB0, 0x34, 0x66, 0xBA, 0x43, 0x00, 0xEE, CLD, CLD, CLD, CLD];

        #[test]
        fn a_pit_write_ends_the_quantum_only_for_a_source_that_asks() {
            let mut exits = Vec::new();
            for pit_ends in [false, true] {
                let (mut boot, _) = irq_board(&PIT_WRITE_PROGRAM);
                let script = ScriptedSource::default();
                script.0.lock().unwrap().pit_ends = pit_ends;
                let (mut source, mut counters) = (script.clone(), IrqCounters::default());
                let (state, report) = run_hook(&mut boot, 20, &mut source, &mut counters, false);
                exits.push((
                    report.exit,
                    report.instructions_executed,
                    state.rip,
                    counters,
                ));
            }
            // Without the request the loop runs on past the OUT to the filler after the code.
            assert!(
                matches!(
                    &exits[0].0,
                    PagedCpl0SetExit::Run(RunExit::Unsupported { .. })
                ),
                "{:?}",
                exits[0]
            );
            assert_eq!(exits[0].3.pit_write_quantum_ends, 0);
            // With it the quantum ends right after the OUT (three retired instructions).
            assert_eq!(
                exits[1].0,
                PagedCpl0SetExit::Run(RunExit::StopRequested),
                "{:?}",
                exits[1]
            );
            assert_eq!((exits[1].1, exits[1].2), (3, ENTRY + 7));
            assert_eq!(exits[1].3.pit_write_quantum_ends, 1);
        }

        #[test]
        fn a_pit_write_quantum_end_is_resumed_by_the_board_and_deadline_is_asked_again() {
            let (mut boot, file) = boot_with_entry(&PIT_WRITE_PROGRAM);
            let script = ScriptedSource::default();
            script.0.lock().unwrap().pit_ends = true;
            boot.board.set_irq_source(Box::new(script.clone()));
            let record = run_set_facade(&mut boot, &file, 6, LinuxCpl0SetMembers::linux_default());
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!((record.retired, record.rip), (6, ENTRY + 10));
            // The OUT ended the first quantum; the rest is the second.
            assert_eq!(record.quanta, 2);
            // One question before each quantum.
            assert_eq!(script.0.lock().unwrap().polls, 2);
            // The same program without the request is one quantum.
            let (mut boot, file) = boot_with_entry(&PIT_WRITE_PROGRAM);
            let record = run_set_facade(&mut boot, &file, 6, LinuxCpl0SetMembers::linux_default());
            assert_eq!((record.retired, record.quanta), (6, 1));
        }
    }

    // ---- S6-12b: NMI through vector 2, blocking until IRETQ, one remembered NMI ----

    mod nmi {
        use super::irq_hook::{CLD, IRQ_HANDLER_AT, IRQ_IDT_AT, irq_board};
        use super::*;
        use crate::FirmwareBoardError;
        use crate::vcpu::NmiBoundary;
        use od_isa::{StiProfile, StiShadow};

        /// ICR_LOW of an NMI to the physical APIC ID in ICR_HIGH (delivery mode 100).
        const NMI_ICR: u32 = 4 << 8;
        const IRETQ: [u8; 2] = [0x48, 0xCF];

        /// A board whose entry is eight `CLD`s, IF = 1 (the IRQ board), with the IDT gate of
        /// vector 2 an interrupt gate (CS 0x10) to a handler that is one `IRETQ`.
        fn nmi_board() -> LinuxDirectBoot {
            let (mut boot, _) = irq_board(&[CLD; 8]);
            write_phys(&mut boot, IRQ_HANDLER_AT, &IRETQ);
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(IRQ_HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((IRQ_HANDLER_AT >> 16) as u16).to_le_bytes());
            write_phys(&mut boot, IRQ_IDT_AT + 2 * 16, &gate);
            boot
        }

        /// An NMI IPI from vCPU 0 to the APIC ID 0 (itself: physical addressing names an ID).
        fn send_nmi(boot: &LinuxDirectBoot) {
            boot.board
                .local_apic()
                .write_icr(0, 0, NMI_ICR)
                .expect("NMI IPI");
        }

        /// Run `budget` instructions of the set from the board's state and keep the result.
        fn run(boot: &mut LinuxDirectBoot, budget: u64) -> PagedCpl0SetReport {
            let cli = CliProfile::NonPviLong64V1;
            let sti = StiProfile::NonPviLong64V1;
            let set = minimal_set(&cli).with_sti(&sti);
            let (state, report) = run_set(boot, true, budget, &set);
            assert_eq!(report.instructions_executed, budget, "{report:?}");
            *boot.board.state_mut() = state;
            report
        }

        #[test]
        fn an_nmi_is_delivered_through_vector_2_whatever_the_interrupt_flag() {
            for interrupt_flag in [0, 1 << 9] {
                let mut boot = nmi_board();
                boot.board.state_mut().rflags = 2 | interrupt_flag;
                let rsp = boot.board.state().gpr[Register::Rsp as usize];
                assert_eq!(
                    boot.board.deliver_pending_nmi(0),
                    Ok(NmiBoundary::NoPending)
                );
                send_nmi(&boot);
                assert!(boot.board.local_apic().pending_nmi());
                assert_eq!(
                    boot.board.deliver_pending_nmi(0),
                    Ok(NmiBoundary::Delivered)
                );
                // The handler has control: RIP is the gate offset, CS the gate selector, the
                // frame (RIP, CS, RFLAGS, old RSP, SS) is on the stack, IF is cleared and the
                // latch is free again.
                let state = boot.board.state().clone();
                assert_eq!(state.rip, IRQ_HANDLER_AT);
                assert_eq!(state.cs_selector, Some(0x10));
                assert!(state.nmi_blocked);
                assert_eq!(state.rflags & (1 << 9), 0);
                let frame = state.gpr[Register::Rsp as usize];
                assert_eq!(frame, (rsp & !15) - 40);
                assert_eq!(read_qword(&mut boot, frame), ENTRY);
                assert_eq!(read_qword(&mut boot, frame + 8), 0x10);
                assert_eq!(read_qword(&mut boot, frame + 16) & (1 << 9), interrupt_flag);
                assert_eq!(read_qword(&mut boot, frame + 24), rsp);
                assert!(!boot.board.local_apic().pending_nmi());
            }
        }

        #[test]
        fn s6_21_interleaved_runs_the_scheduler_nmi_boundary() {
            let mut boot = nmi_board();
            boot.board.enable_linux_interleaved().unwrap();
            send_nmi(&boot);
            assert!(boot.board.local_apic().pending_nmi());

            let cli = CliProfile::NonPviLong64V1;
            let set = minimal_set(&cli);
            let report = boot
                .board
                .run_linux_boot_cpl0_set(1, &set, false, false, false)
                .unwrap();
            // The one retired instruction is the NMI handler's IRETQ, which returns to CLD.
            assert_eq!(report.run.retired, 1);
            assert_eq!(boot.board.state().rip, ENTRY);
            assert!(!boot.board.local_apic().pending_nmi());
            assert!(!boot.board.state().nmi_blocked);
        }

        #[test]
        fn s6_21_interleaved_can_wake_from_halt_by_nmi_before_the_board_irq() {
            struct NmiOnIdle {
                lapic: crate::LocalApicHandle,
                fired: Cell<bool>,
            }

            impl crate::irq_source::BoardIrqSource for NmiOnIdle {
                fn pending_vector(&self) -> Option<u8> {
                    None
                }

                fn acknowledge(&mut self, _: u8) {
                    panic!("the board IRQ has not arrived");
                }

                fn next_deadline_ns(&self) -> Option<u64> {
                    Some(10_000)
                }

                fn wake_ns(&self) -> Option<u64> {
                    if !self.fired.replace(true) {
                        self.lapic.write_icr(0, 0, NMI_ICR).unwrap();
                    }
                    Some(10_000)
                }
            }

            let mut boot = nmi_board();
            write_phys(&mut boot, ENTRY, &[0xF4, CLD]);
            boot.board.state_mut().rflags |= 1 << 9;
            boot.board.set_irq_source(Box::new(NmiOnIdle {
                lapic: boot.board.local_apic(),
                fired: Cell::new(false),
            }));
            boot.board.enable_linux_interleaved().unwrap();

            let cli = CliProfile::NonPviLong64V1;
            let halt = HaltProfile::Long64V1;
            let set = minimal_set(&cli).with_halt(&halt);
            let report = boot
                .board
                .run_linux_boot_cpl0_set(2, &set, false, false, true)
                .unwrap();
            assert_eq!(report.run.exit, PagedProbeExit::BudgetExhausted);
            assert_eq!((report.run.retired, report.run.virtual_ns), (2, 2));
            assert_eq!(report.irq.halt_wakes, 0);
            assert_eq!(boot.board.state().rip, ENTRY + 1);
            assert!(!boot.board.local_apic().pending_nmi());
        }

        #[test]
        fn the_second_nmi_during_the_handler_is_delivered_after_iretq_and_the_third_is_lost() {
            let mut boot = nmi_board();
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            // First NMI: delivered, the handler (one IRETQ) has not run yet.
            send_nmi(&boot);
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Ok(NmiBoundary::Delivered)
            );
            assert_eq!(boot.board.state().rip, IRQ_HANDLER_AT);
            // Second and third NMI arrive during the handler: one is remembered, the other lost.
            // NMI blocking holds the remembered one: no second frame, RIP and RSP unchanged.
            send_nmi(&boot);
            send_nmi(&boot);
            let in_handler = boot.board.state().clone();
            assert_eq!(boot.board.deliver_pending_nmi(0), Ok(NmiBoundary::Held));
            assert_eq!(*boot.board.state(), in_handler);
            assert!(boot.board.local_apic().pending_nmi());
            // The handler's IRETQ retires: blocking ends and the interrupted code is resumed.
            run(&mut boot, 1);
            let state = boot.board.state().clone();
            assert!(!state.nmi_blocked);
            assert_eq!(state.rip, ENTRY);
            assert_eq!(state.gpr[Register::Rsp as usize], rsp);
            assert!(boot.board.local_apic().pending_nmi(), "still waiting");
            // The second NMI is delivered now: the handler has control again.
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Ok(NmiBoundary::Delivered)
            );
            assert_eq!(boot.board.state().rip, IRQ_HANDLER_AT);
            assert!(boot.board.state().nmi_blocked);
            assert!(!boot.board.local_apic().pending_nmi());
            // After the second IRETQ nothing waits: the third NMI was lost.
            run(&mut boot, 1);
            assert_eq!(boot.board.state().rip, ENTRY);
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Ok(NmiBoundary::NoPending)
            );
            assert_eq!(boot.board.state().rip, ENTRY);
            assert!(!boot.board.state().nmi_blocked);
        }

        #[test]
        fn an_nmi_under_the_sti_shadow_waits_one_instruction_and_is_never_dropped() {
            let mut boot = nmi_board();
            boot.board.state_mut().sti_shadow = Some(StiShadow::Blocking);
            send_nmi(&boot);
            let before = boot.board.state().clone();
            assert_eq!(boot.board.deliver_pending_nmi(0), Ok(NmiBoundary::Held));
            assert_eq!(*boot.board.state(), before);
            assert!(boot.board.local_apic().pending_nmi());
            // One `CLD` retires and ends the shadow; then the NMI is delivered.
            run(&mut boot, 1);
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Ok(NmiBoundary::Delivered)
            );
            assert_eq!(boot.board.state().rip, IRQ_HANDLER_AT);
        }

        #[test]
        fn a_halted_vcpu_wakes_for_an_nmi() {
            let mut boot = nmi_board();
            boot.board.state_mut().activity = Some(CpuActivity::Halted);
            // IF = 0: only an NMI can wake it.
            boot.board.state_mut().rflags = 2;
            send_nmi(&boot);
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Ok(NmiBoundary::Delivered)
            );
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Running));
            assert_eq!(boot.board.state().rip, IRQ_HANDLER_AT);
        }

        #[test]
        fn a_state_that_is_not_modelled_is_an_error_not_a_hold() {
            // Activity `None` and STI shadow `None` say nothing about the processor: the board
            // does not guess a hold, it refuses before any change and the NMI stays pending.
            let mut boot = nmi_board();
            boot.board.state_mut().activity = None;
            send_nmi(&boot);
            let before = boot.board.state().clone();
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Err(FirmwareBoardError::Config(
                    "deliver_pending_nmi: vCPU activity not modelled"
                ))
            );
            assert_eq!(*boot.board.state(), before);
            assert!(boot.board.local_apic().pending_nmi());

            let mut boot = nmi_board();
            boot.board.state_mut().sti_shadow = None;
            send_nmi(&boot);
            let before = boot.board.state().clone();
            assert_eq!(
                boot.board.deliver_pending_nmi(0),
                Err(FirmwareBoardError::Config(
                    "deliver_pending_nmi: STI shadow not modelled"
                ))
            );
            assert_eq!(*boot.board.state(), before);
            assert!(boot.board.local_apic().pending_nmi());
        }

        #[test]
        fn a_gate_that_cannot_deliver_is_an_error_and_the_nmi_stays_pending() {
            let mut boot = nmi_board();
            write_phys(&mut boot, IRQ_IDT_AT + 2 * 16, &[0u8; 16]);
            send_nmi(&boot);
            let before = boot.board.state().clone();
            let error = boot.board.deliver_pending_nmi(0).expect_err("no gate");
            assert!(
                matches!(
                    &error,
                    FirmwareBoardError::ExternalInterruptDelivery { vector: 2, failure, .. }
                        if failure == "Refused(GateNotPresent)"
                ),
                "{error:?}"
            );
            assert_eq!(*boot.board.state(), before);
            assert!(boot.board.local_apic().pending_nmi());
            assert_eq!(
                boot.board.deliver_pending_nmi(7),
                Err(FirmwareBoardError::Config(
                    "deliver_pending_nmi: no such vCPU"
                ))
            );
        }
    }

    // ---- S4-38L8c: the PIT -> IOAPIC INTIN2 source on a synthetic guest ----

    /// The source of the board (PIT OUT0 -> IOAPIC INTIN2 -> Local APIC) driven by a flat guest
    /// that programs the PIT with its own `OUT`s and idles in `JMP $`. The guest cannot touch the
    /// Local APIC or the IOAPIC (the set loop has RAM, ports and the ECAM window only, S4-38L9s), so the host writes entry 2
    /// and SVR before the run and the EOI between the chunks of the run, through the same device
    /// registers a guest store would reach.
    /// S4-39g: `STI; HLT` woken by the PIT through the 8259 master (IR0, vector `0x30`), the guest
    /// EOI (port `0x20`) and the INTA of the board loop. The guest programs the PIC and the PIT
    /// with its own `OUT`s; the handler is `[OUT 0x20, 0x20;] IRETQ`.
    mod pic_wake {
        use super::irq_hook::{IRQ_HANDLER_AT, IRQ_VECTOR, irq_board};
        use super::*;
        use crate::{AccessWidth, PortAddress};
        use od_isa::StiProfile;

        const IRETQ: [u8; 2] = [0x48, 0xCF];
        const SPIN: [u8; 2] = [0xEB, 0xFE];
        const STI_HLT: [u8; 2] = [0xFB, 0xF4];
        const CLI_HLT: [u8; 2] = [0xFA, 0xF4];
        /// `MOV AL, 0x5A`: the marker of the instruction after the first `HLT`.
        const MARK: [u8; 2] = [0xB0, 0x5A];
        /// `MOV AL, 0x77`: the marker after the second `HLT`.
        const MARK2: [u8; 2] = [0xB0, 0x77];

        /// `MOV AL, value; MOV DX, port; OUT DX, AL`.
        fn outb(port: u16, value: u8) -> Vec<u8> {
            vec![0xB0, value, 0x66, 0xBA, port as u8, (port >> 8) as u8, 0xEE]
        }

        /// `init_8259A` of the master as Linux writes it (ICW1..ICW4) and the mask `imr`.
        fn pic_init(imr: u8) -> Vec<u8> {
            [
                outb(0x20, 0x11),
                outb(0x21, 0x30),
                outb(0x21, 0x04),
                outb(0x21, 0x01),
                outb(0x21, imr),
            ]
            .concat()
        }

        /// PIT channel 0, LSB+MSB, mode 2, reload `n`.
        fn pit_program(n: u16) -> Vec<u8> {
            [
                outb(0x43, 0x34),
                outb(0x40, n as u8),
                outb(0x40, (n >> 8) as u8),
            ]
            .concat()
        }

        /// The handler with the EOI of the guest, and the handler without it.
        fn eoi_handler() -> Vec<u8> {
            [outb(0x20, 0x20), IRETQ.to_vec()].concat()
        }

        fn program(imr: u8, tail: &[&[u8]]) -> Vec<u8> {
            let mut code = pic_init(imr);
            code.extend(pit_program(10));
            for part in tail {
                code.extend_from_slice(part);
            }
            code
        }

        /// A board with the IDT of `irq_board`, `handler` at the vector, and (with `wake`) the
        /// PIT -> 8259 source installed.
        fn board(code: &[u8], handler: &[u8], wake: bool) -> (LinuxDirectBoot, Vec<u8>) {
            let (mut boot, file) = irq_board(code);
            write_phys(&mut boot, IRQ_HANDLER_AT, handler);
            if wake {
                assert!(
                    boot.board.enable_pit_pic_irq(),
                    "the Linux board has PIT and PIC"
                );
            }
            (boot, file)
        }

        fn run(
            boot: &mut LinuxDirectBoot,
            budget: u64,
        ) -> crate::firmware_board::LinuxSetRunReport {
            let cli = CliProfile::NonPviLong64V1;
            let sti = StiProfile::NonPviLong64V1;
            let halt = HaltProfile::Long64V1;
            let set = minimal_set(&cli)
                .with_pio_u8()
                .with_sti(&sti)
                .with_halt(&halt);
            boot.board
                .run_linux_boot_cpl0_set(budget, &set, false, false, true)
                .expect("runs")
        }

        fn rax_low(boot: &LinuxDirectBoot) -> u8 {
            boot.board.state().gpr[Register::Rax as usize] as u8
        }

        fn pic(boot: &LinuxDirectBoot) -> (u8, u8) {
            let snapshot = boot.board.pic_master().expect("the PIC exists").snapshot();
            (snapshot.irr, snapshot.isr)
        }

        fn out_port(boot: &mut LinuxDirectBoot, port: u16, value: u8) {
            boot.board
                .bus_mut()
                .write_port(PortAddress::new(port), AccessWidth::Byte, u32::from(value))
                .expect("the port accepts the write");
        }

        #[test]
        fn an_idle_sti_hlt_is_woken_by_ir0_and_returns_to_the_instruction_after_the_hlt() {
            let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
            let (mut boot, _) = board(&code, &eoi_handler(), true);
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            let report = run(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            // The clock skipped from the HLT (a few dozen ns) to the edge at 8381 ns.
            assert!(
                (8000..8381).contains(&report.irq.halt_idle_ns),
                "{}",
                report.irq.halt_idle_ns
            );
            assert!(report.run.virtual_ns >= 8381, "{}", report.run.virtual_ns);
            // The wake is one unit of the budget: 299 instructions retired.
            assert_eq!(report.run.retired, 299);
            // The handler ran its EOI and IRETQ; the instruction after the HLT ran next.
            assert_eq!(pic(&boot), (0, 0), "the EOI of the guest cleared ISR");
            assert_eq!(rax_low(&boot), 0x5A);
            let state = boot.board.state();
            assert_eq!(state.rip, ENTRY + code.len() as u64 - 2, "the spin loop");
            assert_ne!(state.rflags & (1 << 9), 0, "IRETQ restored IF");
            assert_eq!(state.gpr[Register::Rsp as usize], rsp);
            assert_eq!(state.activity, Some(CpuActivity::Running));
            assert_eq!(IRQ_VECTOR, 0x30);
        }

        #[test]
        fn without_the_eoi_isr_stays_set_and_the_next_edge_waits() {
            let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
            let (mut boot, _) = board(&code, &IRETQ, true);
            let first = run(&mut boot, 300);
            assert_eq!(first.irq.irqs_delivered, 1, "{first:?}");
            assert_eq!(pic(&boot), (0, 1), "INTA set ISR, the handler sent no EOI");
            // The second edge (16762 ns) comes while the spin loop runs with IF = 1.
            let second = run(&mut boot, 10_000);
            assert_eq!(
                second.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{second:?}"
            );
            assert_eq!(second.irq.irqs_delivered, 0, "{second:?}");
            assert_eq!(pic(&boot), (1, 1), "the second edge waits in IRR");
            // The EOI of the host delivers the one waiting request.
            out_port(&mut boot, 0x20, 0x20);
            assert_eq!(pic(&boot), (1, 0));
            let third = run(&mut boot, 100);
            assert_eq!(third.irq.irqs_delivered, 1, "{third:?}");
            assert_eq!(pic(&boot), (0, 1));
        }

        #[test]
        fn a_second_hlt_with_ir0_in_service_stays_halted() {
            let code = program(0x00, &[&STI_HLT, &MARK, &STI_HLT, &MARK2, &SPIN]);
            let (mut boot, _) = board(&code, &IRETQ, true);
            let report = run(&mut boot, 400);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            assert_eq!(
                rax_low(&boot),
                0x5A,
                "the instruction after the second HLT did not run"
            );
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
            assert_eq!(pic(&boot).1, 1);
        }

        #[test]
        fn hlt_without_if_is_not_woken() {
            let code = program(0x00, &[&CLI_HLT, &MARK, &SPIN]);
            let (mut boot, _) = board(&code, &eoi_handler(), true);
            let report = run(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(
                rax_low(&boot),
                0x00,
                "no instruction ran after the HLT (AL is still the last OUT value)"
            );
            let state = boot.board.state();
            assert_eq!(state.rflags & (1 << 9), 0);
            assert_eq!(state.activity, Some(CpuActivity::Halted));
            assert_eq!(
                state.rip,
                ENTRY + code.len() as u64 - 4,
                "right after the HLT"
            );
        }

        #[test]
        fn hlt_with_ir0_masked_is_not_woken() {
            let code = program(0x01, &[&STI_HLT, &MARK, &SPIN]);
            let (mut boot, _) = board(&code, &eoi_handler(), true);
            let report = run(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
        }

        #[test]
        fn without_the_source_hlt_ends_the_run_as_before() {
            let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
            let (mut boot, _) = board(&code, &eoi_handler(), false);
            let report = run(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.run.retired, 26, "15 + 9 + STI + HLT");
        }

        #[test]
        fn the_user_mode_facade_wakes_and_the_other_facade_leaves_hlt_unsupported() {
            let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
            let members = LinuxCpl0SetMembers {
                pio_u8: true,
                sti: true,
                ..LinuxCpl0SetMembers::minimal()
            };
            let (mut boot, file) = board(&code, &eoi_handler(), false);
            let record = run_linux_direct_board_set_user_mode(
                &mut boot,
                &input(&file, &[], BLOB, board_hash()),
                300,
                members,
            )
            .expect("runs");
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!(
                (record.irqs_delivered, record.halt_wakes),
                (1, 1),
                "{record:?}"
            );
            assert!(record.halt_idle_ns >= 8000, "{record:?}");
            for name in ["halt", "irq", "user_mode"] {
                assert!(
                    record.set_members.iter().any(|member| member == name),
                    "{name}: {:?}",
                    record.set_members
                );
            }
            // Without the user mode neither the source nor `halt` is switched on: the HLT
            // stays outside the set, as it was.
            let (mut boot, file) = board(&code, &eoi_handler(), false);
            let record = run_set_facade(&mut boot, &file, 300, members);
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0xF4 },
                "{record:?}"
            );
            assert_eq!((record.irqs_delivered, record.halt_wakes), (0, 0));
            assert!(!record.set_members.iter().any(|member| member == "halt"));
            assert!(boot.board.pit_pic_source().is_none());
        }

        /// S4-23j: the opt-in single clock owner in the set loop. The default board never takes
        /// the clock; with the opt-in the PIT -> IR0 runs above give the same results.
        mod clock_owner {
            use super::*;
            use crate::{FirmwareBoardError, LOCAL_APIC_MMIO_BASE, VirtualClockError};

            /// Everything observable of one run on a board with the PIT -> IR0 source.
            #[derive(Debug, Eq, PartialEq)]
            struct Outcome {
                report: crate::firmware_board::LinuxSetRunReport,
                state: LongModeState,
                com1: Vec<u8>,
                now: u64,
                pic: (u8, u8),
            }

            fn outcome(code: &[u8], handler: &[u8], owner: bool, budget: u64) -> Outcome {
                let (mut boot, _) = board(code, handler, true);
                if owner {
                    boot.board.enable_clock_owner().expect("the board can");
                }
                assert_eq!(boot.board.clock_owner_enabled(), owner);
                let report = run(&mut boot, budget);
                Outcome {
                    report,
                    state: boot.board.state().clone(),
                    com1: boot.board.com1_transcript().to_vec(),
                    now: boot.board.clock().now_ns(),
                    pic: pic(&boot),
                }
            }

            fn same_with_and_without_owner(code: &[u8], handler: &[u8], budget: u64) -> Outcome {
                let default = outcome(code, handler, false, budget);
                let owned = outcome(code, handler, true, budget);
                assert_eq!(owned, default);
                default
            }

            /// The Local APIC register of the board through the bus, as the host (a guest cannot
            /// reach it from the set loop).
            fn lapic_write(boot: &mut LinuxDirectBoot, offset: u64, value: u64) {
                boot.board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
                        AccessWidth::Dword,
                        value,
                        |_| {},
                    )
                    .expect("the register accepts the write");
            }

            /// SVR enabled; a one-shot LVT timer of `vector` that fires `ns` after now (the timer
            /// runs at 100 MHz and the divider is 1, so one count is 10 ns).
            fn arm_lapic_timer(boot: &mut LinuxDirectBoot, vector: u64, ns: u64) {
                lapic_write(boot, 0xF0, 0x1ff);
                lapic_write(boot, 0x320, vector);
                lapic_write(boot, 0x3E0, 0xB);
                lapic_write(boot, 0x380, ns / 10);
            }

            #[test]
            fn opt_in_matches_default_for_sti_hlt_woken_by_ir0() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                // The wake of the first HLT, and then 20000 instructions of the spin loop with
                // the periodic edge every 8381 ns delivered at its own time.
                let woken = same_with_and_without_owner(&code, &eoi_handler(), 300);
                assert_eq!(woken.report.irq.halt_wakes, 1, "{woken:?}");
                assert!(woken.report.irq.halt_idle_ns >= 8000, "{woken:?}");
                let spin = same_with_and_without_owner(&code, &eoi_handler(), 20_000);
                assert!(spin.report.irq.irqs_delivered >= 2, "{spin:?}");
                assert!(spin.report.irq.quantum_cut_by_deadline >= 1, "{spin:?}");
                // No EOI: the second HLT stays halted with IR0 in service.
                let code = program(0x00, &[&STI_HLT, &MARK, &STI_HLT, &MARK2, &SPIN]);
                let in_service = same_with_and_without_owner(&code, &IRETQ, 400);
                assert_eq!(in_service.report.run.exit, PagedProbeExit::Halted);
                // IF = 0: the HLT is not woken.
                let code = program(0x00, &[&CLI_HLT, &MARK, &SPIN]);
                let no_if = same_with_and_without_owner(&code, &eoi_handler(), 300);
                assert_eq!(no_if.report.irq.halt_wakes, 0, "{no_if:?}");
                assert_eq!(no_if.report.run.exit, PagedProbeExit::Halted);
                // IR0 masked: the same.
                let code = program(0x01, &[&STI_HLT, &MARK, &SPIN]);
                let masked = same_with_and_without_owner(&code, &eoi_handler(), 300);
                assert_eq!(masked.report.irq.halt_wakes, 0, "{masked:?}");
                assert_eq!(masked.report.run.exit, PagedProbeExit::Halted);
            }

            #[test]
            fn s6_21_interleaved_matches_owned_linux_halt_and_pic_delivery() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let handler = eoi_handler();
                // Eight OUT sequences retire three instructions each. STI; HLT adds two.
                let through_halt = 8 * 3 + 2;
                for budget in [0, through_halt, through_halt + 1, 20_000] {
                    let direct = outcome(&code, &handler, true, budget);
                    assert_eq!(
                        direct.report.irq.halt_wakes,
                        u64::from(budget > through_halt)
                    );
                    if budget == through_halt + 1 {
                        assert_eq!(direct.report.run.retired, through_halt);
                        assert_eq!(direct.report.run.exit, PagedProbeExit::BudgetExhausted);
                    }
                    let (mut boot, _) = board(&code, &handler, true);
                    boot.board.enable_linux_interleaved().unwrap();
                    let report = run(&mut boot, budget);
                    let scheduled = Outcome {
                        report,
                        state: boot.board.state().clone(),
                        com1: boot.board.com1_transcript().to_vec(),
                        now: boot.board.clock().now_ns(),
                        pic: pic(&boot),
                    };
                    assert_eq!(scheduled, direct, "budget {budget}");

                    let (mut boot, _) = board(&code, &handler, true);
                    boot.board.enable_seeded_linux_interleaved(6).unwrap();
                    let mut report = run(&mut boot, budget);
                    // The seed can end a turn before a device deadline. It must not
                    // count that earlier cut as a device deadline.
                    if budget == 20_000 {
                        assert_eq!(report.irq.quantum_cut_by_deadline, 2);
                        report.irq.quantum_cut_by_deadline =
                            direct.report.irq.quantum_cut_by_deadline;
                    }
                    report.run.quanta = direct.report.run.quanta;
                    let seeded = Outcome {
                        report,
                        state: boot.board.state().clone(),
                        com1: boot.board.com1_transcript().to_vec(),
                        now: boot.board.clock().now_ns(),
                        pic: pic(&boot),
                    };
                    assert_eq!(seeded, direct, "seeded budget {budget}");
                }
            }

            #[test]
            #[ignore = "S6-10 parity uses locally staged Linux and initramfs images"]
            fn s6_21_interleaved_matches_owned_linux_on_s6_10_image() {
                let root =
                    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../work/stage4");
                let kernel = std::fs::read(root.join("linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"))
                    .expect("staged S6-10 Linux kernel");
                let initramfs = std::fs::read(root.join("initramfs/initramfs.cpio"))
                    .expect("staged S6-10 initramfs");
                assert_eq!(
                    hex(&sha256(&kernel)),
                    "9f55ef7253055a9cf68f8a04323c93dead2fca617386d842fbde9ca7b178b27e",
                );
                assert_eq!(
                    hex(&sha256(&initramfs)),
                    "ee9183a4eded3c16a8c6a7a81c6d5bb79be8bb023f793f7cd61694ea2bd3e588",
                );
                let input = LinuxDirectInput {
                    cmdline: b"console=ttyS0,115200 earlycon=uart8250,io,0x3f8 8250.nr_uarts=1 od.nonce=s6-10-ctl nokaslr",
                    ..input(&kernel, &initramfs, BLOB, board_hash())
                };
                let members = LinuxCpl0SetMembers {
                    cpuid_profile: CpuidProfile::X8664V1Stage4A,
                    clock_owner: true,
                    ..LinuxCpl0SetMembers::linux_default()
                };
                let budget = 300_000_000;
                let run_record = |interleaved| {
                    let mut boot = load_linux_direct_board(&input).unwrap();
                    if interleaved {
                        boot.board.enable_linux_interleaved().unwrap();
                    }
                    run_linux_direct_board_set_user_mode(&mut boot, &input, budget, members)
                        .expect("Linux run")
                };
                let direct = run_record(false);
                assert_eq!(direct.retired, budget);
                assert_eq!(direct.exit, PagedProbeExit::BudgetExhausted);
                assert_eq!(run_record(true), direct);
            }

            #[test]
            fn default_never_claims_the_clock() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let (mut boot, _) = board(&code, &eoi_handler(), true);
                let report = run(&mut boot, 300);
                assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
                assert!(!boot.board.clock_owner_enabled());
                // The standalone advance still works: nobody claimed the clock.
                let now = boot.board.clock().now_ns();
                boot.board.advance_time_ns(5).expect("not claimed");
                assert_eq!(boot.board.clock().now_ns(), now + 5);
            }

            /// Total of `total` instructions in calls of at most `chunk`; the clock, the wakes and
            /// the deliveries at the end.
            fn trace(code: &[u8], owner: bool, chunk: u64, total: u64) -> (u64, u64, u64) {
                let (mut boot, _) = board(code, &eoi_handler(), true);
                if owner {
                    boot.board.enable_clock_owner().expect("the board can");
                }
                let (mut left, mut wakes, mut irqs) = (total, 0, 0);
                while left > 0 {
                    let budget = left.min(chunk);
                    let report = run(&mut boot, budget);
                    wakes += report.irq.halt_wakes;
                    irqs += report.irq.irqs_delivered;
                    left -= budget;
                }
                (boot.board.clock().now_ns(), wakes, irqs)
            }

            #[test]
            fn opt_in_quantum_does_not_change_wake_times() {
                const CHUNK: u64 = 311;
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let whole = trace(&code, true, 30_000, 30_000);
                assert!(whole.2 >= 3, "{whole:?}");
                assert_eq!(trace(&code, true, CHUNK, 30_000), whole);
                assert_eq!(trace(&code, true, 29, 30_000), whole);
                // The default board gives the same trace in either shape.
                assert_eq!(trace(&code, false, CHUNK, 30_000), whole);
                assert_eq!(trace(&code, false, 30_000, 30_000), whole);
            }

            #[test]
            fn opt_in_host_armed_lapic_timer_cuts_quantum_and_clock_stays_single() {
                // The PIT line is masked, so the only deadline is the host-armed Local APIC timer
                // at 9000 ns (not the 8381 ns of the PIT edge of the other tests).
                let code = program(0x01, &[&SPIN]);
                let spin = |owner: bool| {
                    let (mut boot, _) = board(&code, &eoi_handler(), true);
                    arm_lapic_timer(&mut boot, 0x40, 9000);
                    assert_eq!(boot.board.local_apic().next_timer_deadline(), Some(9000));
                    if owner {
                        boot.board.enable_clock_owner().expect("the board can");
                    }
                    let report = run(&mut boot, 12_000);
                    (boot, report)
                };
                let (mut default, default_report) = spin(false);
                let (mut owned, owned_report) = spin(true);
                // The default board never looks at the Local APIC deadline; the owner cuts the
                // quanta that end before 9000 ns at it (the masked PIT line still has its edge
                // deadline, which both boards cut at). Retired and the clock are the same.
                assert!(
                    owned_report.irq.quantum_cut_by_deadline
                        > default_report.irq.quantum_cut_by_deadline,
                    "{owned_report:?} {default_report:?}"
                );
                assert!(owned_report.run.quanta > default_report.run.quanta);
                assert_eq!(owned_report.run.retired, default_report.run.retired);
                assert_eq!(owned_report.run.virtual_ns, default_report.run.virtual_ns);
                // The timer fired once (a one-shot) and its vector waits in the IRR; the default
                // board never dispatched it.
                let (lapic, plain) = (owned.board.local_apic(), default.board.local_apic());
                assert_eq!(lapic.next_timer_deadline(), None);
                assert_eq!(lapic.vector_state(0x40), (true, false));
                assert_eq!(plain.next_timer_deadline(), Some(9000));
                // One clock: a standalone advance of the owned board fails, the default is fine.
                assert!(matches!(
                    owned.board.advance_time_ns(1),
                    Err(FirmwareBoardError::Clock(
                        VirtualClockError::ClockAlreadyOwned
                    ))
                ));
                assert!(default.board.advance_time_ns(1).is_ok());
            }

            #[test]
            fn s6_21_standalone_linux_keeps_the_board_only_irq_and_clock_policy() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let mut direct = board(&code, &eoi_handler(), true).0;
                let (mut scheduled, file) = board(&code, &eoi_handler(), true);
                for boot in [&mut direct, &mut scheduled] {
                    arm_lapic_timer(boot, 0x40, 100);
                    boot.board.local_apic().write_icr(0, 0, 4 << 8).unwrap();
                }
                let direct_report = run(&mut direct, 20_000);
                let scheduled_report = run_linux_direct_board_set_user_mode(
                    &mut scheduled,
                    &input(&file, &[], BLOB, board_hash()),
                    20_000,
                    LinuxCpl0SetMembers {
                        cpuid_profile: CpuidProfile::X8664V1Stage4A,
                        pio_u8: true,
                        sti: true,
                        ..LinuxCpl0SetMembers::minimal()
                    },
                )
                .unwrap();
                assert_eq!(scheduled_report.exit, PagedProbeExit::BudgetExhausted);
                assert_eq!(scheduled_report.retired, direct_report.run.retired);
                assert_eq!(scheduled_report.quanta, direct_report.run.quanta);
                assert_eq!(scheduled_report.virtual_ns, direct_report.run.virtual_ns);
                assert_eq!(scheduled_report.com1, direct_report.run.com1);
                assert_eq!(scheduled_report.halt_wakes, 1);
                assert_eq!(
                    scheduled_report.irqs_delivered,
                    direct_report.irq.irqs_delivered
                );
                assert_eq!(scheduled.board.state(), direct.board.state());
                let lapic = scheduled.board.local_apic();
                assert_eq!(lapic.next_timer_deadline(), Some(100));
                assert_eq!(lapic.vector_state(0x40), (false, false));
                assert!(lapic.pending_nmi());
                assert!(!scheduled.board.clock_owner_enabled());
                assert!(
                    !scheduled_report
                        .set_members
                        .iter()
                        .any(|name| name == "clock_owner")
                );
                assert!(scheduled.board.advance_time_ns(1).is_ok());
            }

            #[test]
            fn opt_in_refuses_a_lapic_vector_that_would_wake_the_cpu() {
                // The timer fires at 100 ns and leaves its vector eligible in the LAPIC. The
                // guest then halts with IF = 1 and the board source has a wake time: the arbiter
                // would see a LAPIC wake, but the loop delivers the board source only, so the
                // run is refused instead of letting the guest pass its HLT with no interrupt.
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let cli = CliProfile::NonPviLong64V1;
                let sti = StiProfile::NonPviLong64V1;
                let halt = HaltProfile::Long64V1;
                let set = minimal_set(&cli)
                    .with_pio_u8()
                    .with_sti(&sti)
                    .with_halt(&halt);
                let run_refusal = |interleaved| {
                    let (mut boot, _) = board(&code, &eoi_handler(), true);
                    arm_lapic_timer(&mut boot, 0x40, 100);
                    if interleaved {
                        boot.board.enable_linux_interleaved().unwrap();
                    } else {
                        boot.board.enable_clock_owner().unwrap();
                    }
                    let failure = boot
                        .board
                        .run_linux_boot_cpl0_set(300, &set, false, false, true)
                        .expect_err("refused");
                    assert_eq!(rax_low(&boot), 0x00, "the HLT was not passed");
                    assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
                    (failure.error, failure.retired, boot.board.clock().now_ns())
                };
                let direct = run_refusal(false);
                assert!(
                    matches!(direct.0, FirmwareBoardError::Config(text) if text.contains("Local APIC vector"))
                );
                assert_eq!(run_refusal(true), direct);
            }

            #[test]
            fn s6_21_interleaved_preserves_the_report_for_a_board_already_halted() {
                let code = program(0x01, &[&STI_HLT, &MARK, &SPIN]);
                let run_twice = |interleaved| {
                    let (mut boot, _) = board(&code, &eoi_handler(), true);
                    arm_lapic_timer(&mut boot, 0x40, 100);
                    if interleaved {
                        boot.board.enable_linux_interleaved().unwrap();
                    } else {
                        boot.board.enable_clock_owner().unwrap();
                    }
                    let first = run(&mut boot, 300);
                    assert_eq!(first.run.exit, PagedProbeExit::Halted);
                    assert_eq!(first.irq.halt_wakes, 0);
                    let second = run(&mut boot, 300);
                    (first, second, boot.board.state().clone())
                };
                assert_eq!(run_twice(true), run_twice(false));
            }

            #[test]
            fn opt_in_zero_budget_executes_nothing() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let zero = same_with_and_without_owner(&code, &eoi_handler(), 0);
                assert_eq!(zero.report.run.retired, 0, "{zero:?}");
                assert_eq!(zero.now, 0, "{zero:?}");
                assert_eq!(zero.report.run.exit, PagedProbeExit::BudgetExhausted);
            }

            #[test]
            fn the_set_loop_has_no_local_apic_mmio() {
                // The owner dispatches the Local APIC timers only; that is sound while the set
                // loop cannot let a guest write the Local APIC (S4-38L9s): this is the sentinel.
                use crate::bus::SetStepMmio;
                let (mut boot, _) = boot_with_entry(&[0xF4]);
                let (_, io) = boot.board.bus_mut().split_for_set_step();
                let read = crate::RamAccessOperation::Read {
                    width: AccessWidth::Dword,
                };
                let write = crate::RamAccessOperation::Write {
                    width: AccessWidth::Dword,
                    value: 0,
                };
                for offset in [0xF0, 0x320, 0x380, 0xB0] {
                    let address = GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset);
                    assert!(io.dispatch(address, 4, read).is_err(), "{offset:#x}");
                    assert!(io.dispatch(address, 4, write).is_err(), "{offset:#x}");
                }
            }

            #[test]
            fn the_user_mode_facade_names_the_member_only_when_it_is_on() {
                let code = program(0x00, &[&STI_HLT, &MARK, &SPIN]);
                let members = LinuxCpl0SetMembers {
                    pio_u8: true,
                    sti: true,
                    ..LinuxCpl0SetMembers::minimal()
                };
                assert!(!members.clock_owner);
                assert!(!LinuxCpl0SetMembers::linux_default().clock_owner);
                let run_record = |clock_owner: bool| {
                    let (mut boot, file) = board(&code, &eoi_handler(), false);
                    let record = run_linux_direct_board_set_user_mode(
                        &mut boot,
                        &input(&file, &[], BLOB, board_hash()),
                        20_000,
                        LinuxCpl0SetMembers {
                            clock_owner,
                            ..members
                        },
                    )
                    .expect("runs");
                    (boot, record)
                };
                let (default_boot, default) = run_record(false);
                let (owned_boot, mut owned) = run_record(true);
                assert!(!default_boot.board.clock_owner_enabled());
                assert!(owned_boot.board.clock_owner_enabled());
                assert!(!default.set_members.iter().any(|name| name == "clock_owner"));
                assert!(owned.set_members.iter().any(|name| name == "clock_owner"));
                // Apart from its name the record is the same, byte for byte.
                owned.set_members.retain(|name| name != "clock_owner");
                assert_eq!(owned, default);
                assert!(default.halt_wakes >= 1, "{default:?}");
            }
        }
    }

    /// S4-39i: the UART THRE line to master IR4 under the user-mode facade. The guest enables
    /// THRE and spins; its handler at vector `0x34` is the 8250 driver's shape: read IIR, send
    /// one byte of a table to THR (nothing at the NUL end), EOI, `IRETQ`. Every byte of COM1 is
    /// written by the handler, so without the wire to IR4 the transcript stays empty.
    mod uart_irq4 {
        use super::irq_hook::{IRQ_HANDLER_AT, IRQ_IDT_AT, irq_board};
        use super::*;

        const SPIN: [u8; 2] = [0xEB, 0xFE];
        const TABLE: u64 = DATA + 0x3000;
        const VECTOR: u8 = 0x34;
        const NONCE: &[u8] = b"OD_NONCE=s4-38l4a\n";

        /// `MOV AL, value; MOV DX, port; OUT DX, AL`.
        fn outb(port: u16, value: u8) -> Vec<u8> {
            vec![0xB0, value, 0x66, 0xBA, port as u8, (port >> 8) as u8, 0xEE]
        }

        /// `init_8259A` of the master (ICW2 `0x30`, so IR4 is vector `0x34`), IMR = 0.
        fn pic_init() -> Vec<u8> {
            [
                outb(0x20, 0x11),
                outb(0x21, 0x30),
                outb(0x21, 0x04),
                outb(0x21, 0x01),
                outb(0x21, 0x00),
            ]
            .concat()
        }

        /// The guest: PIC, MCR OUT2, IER = THRE, spin.
        fn program() -> Vec<u8> {
            [
                pic_init(),
                outb(0x3FC, 0x08),
                outb(0x3F9, 0x02),
                SPIN.to_vec(),
            ]
            .concat()
        }

        /// IIR read, `AL = [RSI]`, a NUL ends the output, else the byte goes to THR and RSI
        /// moves on; then EOI and `IRETQ`.
        fn handler() -> Vec<u8> {
            [
                vec![0x66, 0xBA, 0xFA, 0x03, 0xEC],
                vec![0x8A, 0x06, 0x84, 0xC0, 0x74, 0x08],
                vec![0x66, 0xBA, 0xF8, 0x03, 0xEE, 0x48, 0xFF, 0xC6],
                outb(0x20, 0x20),
                vec![0x48, 0xCF],
            ]
            .concat()
        }

        fn board() -> (LinuxDirectBoot, Vec<u8>) {
            let (mut boot, file) = irq_board(&program());
            write_phys(&mut boot, IRQ_HANDLER_AT, &handler());
            let mut table = NONCE.to_vec();
            table.push(0);
            write_phys(&mut boot, TABLE, &table);
            // The gate of vector 0x34 is the gate of the board's IRQ vector, at the same handler.
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(IRQ_HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x10u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((IRQ_HANDLER_AT >> 16) as u16).to_le_bytes());
            write_phys(&mut boot, IRQ_IDT_AT + u64::from(VECTOR) * 16, &gate);
            boot.board.state_mut().gpr[Register::Rsi as usize] = TABLE;
            (boot, file)
        }

        fn members() -> LinuxCpl0SetMembers {
            LinuxCpl0SetMembers {
                pio_u8: true,
                sti: true,
                ..LinuxCpl0SetMembers::minimal()
            }
        }

        #[test]
        fn the_user_mode_run_sends_the_nonce_line_to_com1_from_the_thre_interrupt() {
            let (mut boot, file) = board();
            let record = run_linux_direct_board_set_user_mode(
                &mut boot,
                &input(&file, &[], BLOB, board_hash()),
                4000,
                members(),
            )
            .expect("runs");
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert_eq!(record.com1, NONCE, "{record:?}");
            // One interrupt for the IER enable and one after each byte (the last finds the NUL).
            assert_eq!(record.irqs_delivered, NONCE.len() as u64 + 1, "{record:?}");
            let source = boot.board.uart_irq_source().expect("installed");
            assert_eq!(source.uart_edges(), NONCE.len() as u64 + 1);
            // The handler's EOI cleared ISR; the last IIR read left nothing pending.
            let pic = boot.board.pic_master().expect("the PIC exists").snapshot();
            assert_eq!((pic.irr, pic.isr), (0, 0));
            assert_eq!(boot.board.uart().peek_iir(), 0x01);
            assert_eq!(boot.board.state().gpr[Register::Rsi as usize], TABLE + 18);
        }

        #[test]
        fn opt_in_matches_default_for_the_uart_ir4_run() {
            // S4-23j: the single clock owner changes nothing of the THRE -> IR4 run.
            let run_record = |clock_owner: bool, interleaved: bool| {
                let (mut boot, file) = board();
                if interleaved {
                    boot.board.enable_linux_interleaved().unwrap();
                }
                let record = run_linux_direct_board_set_user_mode(
                    &mut boot,
                    &input(&file, &[], BLOB, board_hash()),
                    4000,
                    LinuxCpl0SetMembers {
                        clock_owner,
                        ..members()
                    },
                )
                .expect("runs");
                (boot, record)
            };
            let (default_boot, default) = run_record(false, false);
            let (owned_boot, mut owned) = run_record(true, false);
            let (scheduled_boot, scheduled) = run_record(false, true);
            assert!(!default_boot.board.clock_owner_enabled());
            assert!(owned_boot.board.clock_owner_enabled());
            assert_eq!(default.com1, NONCE, "{default:?}");
            assert!(owned.set_members.iter().any(|name| name == "clock_owner"));
            assert_eq!(scheduled, owned);
            assert_eq!(scheduled_boot.board.state(), owned_boot.board.state());
            owned.set_members.retain(|name| name != "clock_owner");
            assert_eq!(owned, default);
            assert_eq!(
                owned_boot.board.clock().now_ns(),
                default_boot.board.clock().now_ns()
            );
            assert_eq!(owned_boot.board.state(), default_boot.board.state());
        }

        #[test]
        fn without_the_user_mode_the_uart_line_reaches_nothing_and_com1_stays_empty() {
            let (mut boot, file) = board();
            let record = run_set_facade(&mut boot, &file, 4000, members());
            assert_eq!(record.exit, PagedProbeExit::BudgetExhausted, "{record:?}");
            assert!(record.com1.is_empty(), "{record:?}");
            assert_eq!(record.irqs_delivered, 0, "{record:?}");
            assert!(boot.board.uart_irq_source().is_none());
            // The UART holds the condition (IIR shows THRE), nothing took it.
            assert_eq!(boot.board.uart().peek_iir(), 0x02);
        }

        #[test]
        fn the_uart_source_needs_the_pit_source_first_and_is_installed_once() {
            let (mut boot, _) = board();
            assert!(boot.board.enable_pit_pic_irq());
            assert!(boot.board.enable_uart_irq4());
            assert!(
                boot.board.enable_uart_irq4(),
                "a second call keeps the source"
            );
            assert!(boot.board.pit_pic_source().is_some());
            // Without the PIT source first the UART source is refused.
            let (mut fresh, _) = board();
            assert!(!fresh.board.enable_uart_irq4());
            assert!(fresh.board.uart_irq_source().is_none());
        }
    }

    mod irq_pit_source {
        use super::irq_hook::{CLD, IRQ_HANDLER_AT, IRQ_VECTOR, irq_board};
        use super::*;
        use crate::irq_source::BoardIrqSource;
        use crate::pit_ioapic_irq_source::PitIoapicIrqSource;
        use crate::{AccessWidth, IO_APIC_MMIO_BASE, LOCAL_APIC_MMIO_BASE, pit_ticks};
        use od_isa::StiProfile;

        const PIT_HZ: u64 = 1_193_182;
        const MASKED: u64 = 1 << 16;
        /// `IRETQ`.
        const IRETQ: [u8; 2] = [0x48, 0xCF];
        /// `JMP $`.
        const SPIN: [u8; 2] = [0xEB, 0xFE];
        /// Instructions per call of the board loop; one instruction is one ns, so the chunk is
        /// far inside one PIT period of the tests (the EOI latency of the host).
        const CHUNK: u64 = 500;

        /// The deadline of the PIT tick, by hand: `ceil(tick * 1e9 / 1_193_182)`.
        fn deadline(tick: u64) -> u64 {
            (tick * 1_000_000_000).div_ceil(PIT_HZ)
        }

        /// `MOV AL, control; MOV DX, 0x43; OUT DX, AL; MOV AL, lo; MOV DX, 0x40; OUT DX, AL;
        /// MOV AL, hi; OUT DX, AL`.
        fn program(control: u8, n: u16) -> Vec<u8> {
            vec![
                0xB0,
                control,
                0x66,
                0xBA,
                0x43,
                0x00,
                0xEE,
                0xB0,
                n as u8,
                0x66,
                0xBA,
                0x40,
                0x00,
                0xEE,
                0xB0,
                (n >> 8) as u8,
                0xEE,
            ]
        }

        /// `MOV AL, 0x30; MOV DX, 0x43; OUT DX, AL`: channel 0 mode 0 without a count.
        const SHUTDOWN: [u8; 7] = [0xB0, 0x30, 0x66, 0xBA, 0x43, 0x00, 0xEE];

        fn mmio_write(boot: &mut LinuxDirectBoot, address: u64, value: u64) {
            boot.board
                .bus_mut()
                .write_physical(
                    GuestPhysicalAddress::new(address),
                    AccessWidth::Dword,
                    value,
                    |_| {},
                )
                .expect("the register accepts the write");
        }

        /// The board of `irq_board` with an `IRETQ` handler, the source installed, SVR enabled and
        /// entry 2 (`entry2`) in the IOAPIC.
        fn pit_board(code: &[u8], entry2: u64) -> (LinuxDirectBoot, PitIoapicIrqSource) {
            let (mut boot, _) = irq_board(code);
            write_phys(&mut boot, IRQ_HANDLER_AT, &IRETQ);
            let source = boot
                .board
                .pit_ioapic_irq_source()
                .expect("the Linux board has a PIT");
            boot.board.set_irq_source(Box::new(source.clone()));
            mmio_write(&mut boot, LOCAL_APIC_MMIO_BASE + 0xF0, 0x1ff);
            mmio_write(&mut boot, IO_APIC_MMIO_BASE, 0x14);
            mmio_write(&mut boot, IO_APIC_MMIO_BASE + 0x10, entry2);
            (boot, source)
        }

        fn vector_state(boot: &LinuxDirectBoot) -> (bool, bool) {
            boot.board.local_apic().vector_state(IRQ_VECTOR)
        }

        #[derive(Debug, Default)]
        struct Totals {
            delivered: u64,
            refused_masked: u64,
            cut_by_deadline: u64,
            quanta: u64,
        }

        /// Runs the board loop in chunks until the clock is at `until_ns`; with `eoi` the host
        /// writes the Local APIC EOI register after each chunk that left a vector in service.
        fn run_until(boot: &mut LinuxDirectBoot, until_ns: u64, eoi: bool) -> Totals {
            let cli = CliProfile::NonPviLong64V1;
            let sti = StiProfile::NonPviLong64V1;
            let set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                .with_cli(&cli)
                .with_cpu_owned_cr3(MovCr3Long64V1)
                .with_pio_u8()
                .with_sti(&sti);
            let mut totals = Totals::default();
            while boot.board.clock().now_ns() < until_ns {
                let report = boot
                    .board
                    .run_linux_boot_cpl0_set(CHUNK, &set, false, false, true)
                    .expect("runs");
                assert_eq!(
                    report.run.exit,
                    PagedProbeExit::BudgetExhausted,
                    "{report:?}"
                );
                totals.delivered += report.irq.irqs_delivered;
                totals.refused_masked += report.irq.irq_refused_masked;
                totals.cut_by_deadline += report.irq.quantum_cut_by_deadline;
                totals.quanta += report.run.quanta;
                if eoi && vector_state(boot).1 {
                    mmio_write(boot, LOCAL_APIC_MMIO_BASE + 0xB0, 0);
                }
            }
            totals
        }

        /// Edges of a mode 2 count of `n` ticks loaded at `t0` whose deadline is `<= now`.
        fn edges_due(t0: u64, n: u64, now: u64) -> u64 {
            (1..).take_while(|k| deadline(t0 + k * n) <= now).count() as u64
        }

        #[test]
        fn the_helper_formula_matches_the_model_tick_function() {
            for tick in [1, 4, 10, 1003, 1_000_000] {
                assert_eq!(pit_ticks(deadline(tick)), tick);
                assert!(pit_ticks(deadline(tick) - 1) < tick);
            }
        }

        #[test]
        fn the_board_gives_a_source_because_it_has_the_linux_profile_pit() {
            let (boot, _) = boot_with_entry(&[CLD]);
            assert!(boot.board.pit_ioapic_irq_source().is_some());
        }

        #[test]
        fn a_thousand_periods_give_a_thousand_vectors_with_an_eoi_and_an_iretq_each() {
            let n = 4u16;
            let mut code = program(0x34, n);
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, u64::from(IRQ_VECTOR));
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            // The count is loaded by the guest's own OUTs in the first microseconds.
            let mut totals = run_until(&mut boot, 1000, true);
            let run = source.channel0_run().expect("loaded by the guest");
            assert_eq!(run.reload, u32::from(n));
            let t0 = run.start_tick;
            let until = deadline(t0 + 1000 * u64::from(n)) + 1000;
            let rest = run_until(&mut boot, until, true);
            totals.delivered += rest.delivered;
            assert_eq!(totals.delivered, 1000, "{totals:?} {rest:?}");
            let stats = source.stats();
            assert_eq!(stats.edges, 1000);
            assert_eq!(stats.acknowledged, 1000);
            assert_eq!(
                (stats.edges_refused, stats.ack_refused, stats.queue_faults),
                (0, 0, 0)
            );
            // The IRETQ circle is balanced: stack, IF and the spin loop are as before.
            let state = boot.board.state();
            assert_eq!(state.gpr[Register::Rsp as usize], rsp);
            assert_ne!(state.rflags & (1 << 9), 0, "IRETQ restored IF");
            assert!(
                state.rip == IRQ_HANDLER_AT
                    || (ENTRY..ENTRY + code.len() as u64).contains(&state.rip),
                "rip {:#x}",
                state.rip
            );
            assert_eq!(vector_state(&boot), (false, false));
            assert!(rest.cut_by_deadline >= 990, "{rest:?}");
            assert_eq!(rest.refused_masked, 0);
        }

        #[test]
        fn without_an_eoi_the_second_irq_is_not_delivered() {
            let mut code = program(0x34, 10);
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, u64::from(IRQ_VECTOR));
            let mut totals = run_until(&mut boot, 1000, false);
            let t0 = source.channel0_run().unwrap().start_tick;
            let rest = run_until(&mut boot, deadline(t0 + 40) + 1000, false);
            totals.delivered += rest.delivered;
            assert_eq!(totals.delivered, 1, "{totals:?}");
            // Four edges happened; the vector is in service and one request waits in IRR.
            assert_eq!(source.stats().edges, 4);
            assert_eq!(vector_state(&boot), (true, true));
            // The EOI delivers exactly the one coalesced request.
            mmio_write(&mut boot, LOCAL_APIC_MMIO_BASE + 0xB0, 0);
            let now = boot.board.clock().now_ns();
            let rest = run_until(&mut boot, now + 100, false);
            assert_eq!(rest.delivered, 1, "{rest:?}");
        }

        #[test]
        fn a_masked_entry_delivers_nothing_and_the_edges_are_lost() {
            let mut code = program(0x34, 10);
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, MASKED | u64::from(IRQ_VECTOR));
            let mut totals = run_until(&mut boot, 1000, true);
            let t0 = source.channel0_run().unwrap().start_tick;
            let rest = run_until(&mut boot, deadline(t0 + 40) + 1000, true);
            totals.delivered += rest.delivered;
            assert_eq!(totals.delivered, 0);
            assert_eq!(totals.refused_masked + rest.refused_masked, 0);
            assert_eq!(
                source.stats().edges,
                4,
                "the edges are presented and lost at the IOAPIC"
            );
            assert_eq!(vector_state(&boot), (false, false));
        }

        #[test]
        fn a_pit_shutdown_leaves_no_event_and_no_delivery() {
            let mut code = program(0x34, 10);
            code.extend_from_slice(&SHUTDOWN);
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, u64::from(IRQ_VECTOR));
            let totals = run_until(&mut boot, 300_000, true);
            assert_eq!(totals.delivered, 0);
            assert_eq!(source.next_deadline_ns(), None);
            assert_eq!(source.queued(), 0);
            assert_eq!(source.channel0_run(), None);
            let stats = source.stats();
            // Control word (mode, no count), the loaded count, then the shutdown word.
            assert_eq!(
                (stats.edges, stats.cancelled, stats.generation_changes),
                (0, 1, 3)
            );
            assert_eq!(vector_state(&boot), (false, false));
        }

        #[test]
        fn a_reprogram_cancels_the_old_generation_and_the_new_period_gives_the_edges() {
            let mut code = program(0x34, 1003);
            code.extend_from_slice(&program(0x34, 10));
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, u64::from(IRQ_VECTOR));
            let mut totals = run_until(&mut boot, 1000, true);
            let run = source.channel0_run().expect("the new count");
            assert_eq!(run.reload, 10);
            let t0 = run.start_tick;
            // Past the first edge of the old count (about t0 + 1003 ticks, 840 us).
            let rest = run_until(&mut boot, deadline(t0 + 1010) + 1000, true);
            totals.delivered += rest.delivered;
            let now = boot.board.clock().now_ns();
            assert!(now > deadline(t0 + 1004), "the old first edge is behind");
            // The question applies the last due edge, as the loop would at its next boundary.
            let _ = source.next_deadline_ns();
            let expected = edges_due(t0, 10, now);
            assert!(expected >= 100);
            let stats = source.stats();
            assert_eq!(stats.edges, expected, "only the new period gives edges");
            assert!(stats.cancelled >= 1);
            // Every edge but the last (applied after the final chunk) was delivered once.
            assert!(totals.delivered + 1 >= expected, "{totals:?}");
            assert!(totals.delivered <= expected, "{totals:?}");
        }

        #[test]
        fn with_if_clear_the_vector_is_refused_and_the_deadline_never_falls_behind() {
            let mut code = program(0x34, 10);
            code.push(0xFA); // CLI
            code.extend_from_slice(&SPIN);
            let (mut boot, source) = pit_board(&code, u64::from(IRQ_VECTOR));
            let mut totals = run_until(&mut boot, 1000, true);
            let t0 = source.channel0_run().unwrap().start_tick;
            let rest = run_until(&mut boot, deadline(t0 + 200) + 1000, true);
            totals.delivered += rest.delivered;
            totals.quanta += rest.quanta;
            assert_eq!(totals.delivered, 0);
            assert!(rest.refused_masked > 0, "{rest:?}");
            assert_eq!(boot.board.state().rflags & (1 << 9), 0);
            assert_eq!(vector_state(&boot), (true, false), "waiting in IRR");
            // About one quantum per edge (plus the PIT writes and the chunk ends): a deadline in
            // the past would cut every quantum to one instruction.
            let chunks = boot.board.clock().now_ns() / CHUNK + 1;
            assert!(
                totals.quanta <= source.stats().edges + chunks + 8,
                "{totals:?}"
            );
            let next = source.next_deadline_ns().expect("the next edge");
            assert!(next > boot.board.clock().now_ns());
        }
    }

    // ---- S4-38L8d: the APIC profile (`X8664V1Stage4AApic`): guest access to the LAPIC and the IOAPIC ----

    mod apic_profile {
        use super::irq_hook::{CLD, IRQ_HANDLER_AT, IRQ_VECTOR, irq_board};
        use super::*;
        use crate::{IO_APIC_MMIO_BASE, LOCAL_APIC_MMIO_BASE};
        use od_isa::StiProfile;

        const PIT_HZ: u64 = 1_193_182;
        const MASKED: u32 = 1 << 16;
        const CHUNK: u64 = 500;

        /// The deadline of the PIT tick, by hand: `ceil(tick * 1e9 / 1_193_182)`.
        fn deadline(tick: u64) -> u64 {
            (tick * 1_000_000_000).div_ceil(PIT_HZ)
        }

        /// `MOV <r32>, imm32` (`B8 + r`): EAX = 0, ECX = 1, EBX = 3.
        fn mov32(register: u8, value: u32) -> Vec<u8> {
            let mut code = vec![0xB8 + register];
            code.extend_from_slice(&value.to_le_bytes());
            code
        }

        /// `MOV AL, control; MOV DX, 0x43; OUT DX, AL; MOV AL, lo; MOV DX, 0x40; OUT DX, AL;
        /// MOV AL, hi; OUT DX, AL` (`n` < 256, so AL = 0 afterwards).
        fn pit_program(control: u8, n: u8) -> Vec<u8> {
            vec![
                0xB0, control, 0x66, 0xBA, 0x43, 0x00, 0xEE, 0xB0, n, 0x66, 0xBA, 0x40, 0x00, 0xEE,
                0xB0, 0x00, 0xEE,
            ]
        }

        /// `EBX = LAPIC base`, `ECX = IOAPIC base`; SVR := 0x1FF (software enable, spurious
        /// vector 0xFF); IOREGSEL := 0x14, IOWIN := `entry2` (the low dword of redirection entry 2);
        /// then `EAX = 0`.
        fn apic_setup(entry2: u32) -> Vec<u8> {
            let mut code = mov32(3, LOCAL_APIC_MMIO_BASE as u32);
            code.extend(mov32(1, IO_APIC_MMIO_BASE as u32));
            code.extend(mov32(0, 0x1FF));
            code.extend([0x89, 0x83, 0xF0, 0x00, 0x00, 0x00]); // MOV [RBX+0xF0], EAX
            code.extend(mov32(0, 0x14));
            code.extend([0x89, 0x01]); // MOV [RCX], EAX
            code.extend(mov32(0, entry2));
            code.extend([0x89, 0x41, 0x10]); // MOV [RCX+0x10], EAX
            code.extend(mov32(0, 0));
            code
        }

        /// The guest: set the APIC up, load a PIT mode 2 count of `n`, `STI`, spin.
        fn timer_guest(entry2: u32, n: u8) -> Vec<u8> {
            let mut code = apic_setup(entry2);
            code.extend(pit_program(0x34, n));
            code.push(0xFB); // STI
            code.extend([0xEB, 0xFE]); // JMP $
            code
        }

        /// The handler at `IRQ_HANDLER_AT`: with `eoi` the guest writes the Local APIC EOI register
        /// (`MOV [RBX+0xB0], EAX`, EAX = 0, RBX = the Local APIC base), then `IRETQ`.
        fn handler(eoi: bool) -> Vec<u8> {
            let mut code = Vec::new();
            if eoi {
                code.extend([0x89, 0x83, 0xB0, 0x00, 0x00, 0x00]);
            }
            code.extend([0x48, 0xCF]);
            code
        }

        /// The loader's tables identity-map the first GiB only (2 MiB leaves in the PD at
        /// `cr3 + 0x2000`): map GVA = GPA for the 2 MiB pages of the IOAPIC (`0xFEC00000`) and of the
        /// Local APIC (`0xFEE00000`) through a PD of its own behind PDPT entry 3.
        fn map_apic_windows(boot: &mut LinuxDirectBoot) {
            const PD: u64 = TREE_PML4;
            let cr3 = boot.board.cr3();
            write_phys(boot, cr3 + 0x1000 + 3 * 8, &(PD | 3).to_le_bytes());
            for base in [IO_APIC_MMIO_BASE, LOCAL_APIC_MMIO_BASE] {
                let index = (base >> 21) & 511;
                write_phys(boot, PD + index * 8, &(base | 0x83).to_le_bytes());
            }
        }

        /// A board of `irq_board` with the APIC profile's timer line enabled.
        fn apic_board(code: &[u8], eoi: bool) -> LinuxDirectBoot {
            let (mut boot, _) = irq_board(code);
            map_apic_windows(&mut boot);
            write_phys(&mut boot, IRQ_HANDLER_AT, &handler(eoi));
            assert!(boot.board.enable_pit_ioapic_irq());
            boot
        }

        fn set<'a>(
            cli: &'a CliProfile,
            sti: &'a StiProfile,
            halt: &'a HaltProfile,
        ) -> Cpl0ProfileSet<'a> {
            Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1)
                .with_cli(cli)
                .with_cpu_owned_cr3(MovCr3Long64V1)
                .with_pio_u8()
                .with_halt(halt)
                .with_sti(sti)
        }

        /// Chunks of the board loop until the clock reaches `until_ns`; the delivered vectors.
        fn run_until(boot: &mut LinuxDirectBoot, until_ns: u64) -> u64 {
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set = set(&cli, &sti, &halt);
            let mut delivered = 0;
            while boot.board.clock().now_ns() < until_ns {
                let report = boot
                    .board
                    .run_linux_boot_cpl0_set(CHUNK, &set, false, false, true)
                    .expect("runs");
                assert_eq!(
                    report.run.exit,
                    PagedProbeExit::BudgetExhausted,
                    "{report:?}"
                );
                delivered += report.irq.irqs_delivered;
            }
            delivered
        }

        /// `MOV EBX, 0xFEE00000; MOV EAX, [RBX+0x30]; MOV ECX, 0xFEC00000; MOV EAX, 1;
        /// MOV [RCX], EAX; MOV EDX, [RCX+0x10]; MOV EAX, [RBX+0x30]; HLT`: the Local APIC version in
        /// EAX, the IOAPIC version (IOREGSEL 1, IOWIN) in EDX.
        fn register_guest() -> Vec<u8> {
            let mut code = mov32(3, LOCAL_APIC_MMIO_BASE as u32);
            code.extend([0x8B, 0x43, 0x30]); // MOV EAX, [RBX+0x30]
            code.extend(mov32(1, IO_APIC_MMIO_BASE as u32));
            code.extend(mov32(0, 1));
            code.extend([0x89, 0x01]); // MOV [RCX], EAX: IOREGSEL := 1
            code.extend([0x8B, 0x51, 0x10]); // MOV EDX, [RCX+0x10]
            code.extend([0x8B, 0x43, 0x30]); // MOV EAX, [RBX+0x30]
            code.push(0xF4);
            code
        }

        fn run_register_guest(apic: bool) -> (LinuxDirectBoot, crate::FirmwareRunReport) {
            run_guest_code(&register_guest(), apic)
        }

        fn run_guest_code(code: &[u8], apic: bool) -> (LinuxDirectBoot, crate::FirmwareRunReport) {
            let (mut boot, _) = boot_with_entry(code);
            map_apic_windows(&mut boot);
            if apic {
                assert!(boot.board.enable_pit_ioapic_irq());
            }
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set = set(&cli, &sti, &halt);
            let report = boot
                .board
                .run_linux_boot_cpl0_set(64, &set, false, false, true)
                .expect("runs");
            (boot, report.run)
        }

        #[test]
        fn the_guest_reads_the_lapic_version_and_the_ioapic_version_under_the_apic_profile() {
            let (boot, run) = run_register_guest(true);
            assert_eq!(run.exit, PagedProbeExit::Halted, "{run:?}");
            assert_eq!(run.memory_error, None);
            let state = boot.board.state();
            assert_eq!(state.gpr[Register::Rax as usize], 0x0003_0014);
            assert_eq!(state.gpr[Register::Rdx as usize], 0x0017_0020);
        }

        /// `MOV ECX, 0xFEC00000; MOV EAX, 0x22; MOV [RCX], EAX; MOV EAX, 0xA020;
        /// MOV [RCX+0x10], EAX; MOV EDX, [RCX+0x10]; HLT`: the SCI redirection entry (GSI 9) written
        /// as level, active low, vector 0x20, unmasked (the write the L8d run stopped on), and read
        /// back in EDX.
        fn sci_rte_guest() -> Vec<u8> {
            let mut code = mov32(1, IO_APIC_MMIO_BASE as u32);
            code.extend(mov32(0, 0x22));
            code.extend([0x89, 0x01]); // MOV [RCX], EAX: IOREGSEL := 0x22
            code.extend(mov32(0, 0xA020));
            code.extend([0x89, 0x41, 0x10]); // MOV [RCX+0x10], EAX
            code.extend([0x8B, 0x51, 0x10]); // MOV EDX, [RCX+0x10]
            code.push(0xF4);
            code
        }

        /// S4-38L8e: under the APIC profile the guest's level/active-low SCI entry is accepted
        /// through IOREGSEL/IOWIN and reads back; the run reaches HLT without a memory error.
        #[test]
        fn the_sci_rte_write_is_accepted_under_the_apic_profile() {
            let (boot, run) = run_guest_code(&sci_rte_guest(), true);
            assert_eq!(run.exit, PagedProbeExit::Halted, "{run:?}");
            assert_eq!(run.memory_error, None);
            assert_eq!(boot.board.state().gpr[Register::Rdx as usize], 0xA020);
        }

        /// S4-38L8e: stage4a keeps the former typed error: the IOAPIC window is not served by its
        /// set loop, so the very first register access (IOREGSEL) is a `DeviceUnavailable` error
        /// and the guest does not halt.
        #[test]
        fn stage4a_keeps_the_sci_rte_write_a_typed_error() {
            let (boot, run) = run_guest_code(&sci_rte_guest(), false);
            assert!(
                matches!(
                    run.memory_error,
                    Some(crate::TrackedAccessError::DeviceUnavailable { address })
                        if address.get() == IO_APIC_MMIO_BASE
                ),
                "{run:?}"
            );
            assert_ne!(run.exit, PagedProbeExit::Halted);
            assert_eq!(boot.board.state().gpr[Register::Rdx as usize], 0);
        }

        /// Stage4a keeps the typed error: the Local APIC window is not served by its set loop.
        #[test]
        fn stage4a_keeps_the_lapic_access_a_typed_error() {
            let (boot, run) = run_register_guest(false);
            assert!(
                matches!(
                    run.memory_error,
                    Some(crate::TrackedAccessError::OperandPhysical { address, .. })
                        if address.get() == LOCAL_APIC_MMIO_BASE + 0x30
                ),
                "{run:?}"
            );
            assert_ne!(run.exit, PagedProbeExit::Halted);
            assert_eq!(boot.board.state().gpr[Register::Rax as usize], 0);
        }

        #[test]
        fn a_thousand_periods_give_a_thousand_vectors_with_a_guest_eoi_and_an_iretq_each() {
            let n = 4u8;
            let mut boot = apic_board(&timer_guest(u32::from(IRQ_VECTOR), n), true);
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            // The count is loaded by the guest's own accesses in the first microseconds.
            run_until(&mut boot, 2000);
            let source = boot.board.pit_ioapic_source().expect("installed");
            let run = source.channel0_run().expect("loaded by the guest");
            assert_eq!(u8::try_from(run.reload).unwrap(), n);
            let t0 = run.start_tick;
            let until = deadline(t0 + 1000 * u64::from(n)) + 1000;
            let first = source.stats().acknowledged;
            let delivered = run_until(&mut boot, until);
            let stats = source.stats();
            assert!(delivered >= 1000, "delivered {delivered}, {stats:?}");
            assert_eq!(stats.acknowledged, first + delivered);
            assert_eq!(
                (stats.edges_refused, stats.ack_refused, stats.queue_faults),
                (0, 0, 0),
                "{stats:?}"
            );
            let state = boot.board.state();
            assert_eq!(
                state.gpr[Register::Rsp as usize],
                rsp,
                "the IRETQ circle is balanced"
            );
            assert_ne!(state.rflags & (1 << 9), 0, "IRETQ restored IF");
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        #[test]
        fn a_masked_entry_delivers_nothing() {
            let n = 10u8;
            let mut boot = apic_board(&timer_guest(MASKED | u32::from(IRQ_VECTOR), n), true);
            run_until(&mut boot, 2000);
            let source = boot.board.pit_ioapic_source().expect("installed");
            let t0 = source.channel0_run().expect("loaded").start_tick;
            let delivered = run_until(&mut boot, deadline(t0 + 40) + 1000);
            assert_eq!(delivered, 0);
            assert_eq!(source.stats().edges, 4, "presented and lost at the IOAPIC");
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        /// `STI; HLT`, a mark (`MOV AL, 0x5A`) and a spin: the code after the `HLT` runs only when
        /// the halted CPU was woken and the vector went through the handler.
        const STI_HLT_MARK_SPIN: [u8; 6] = [0xFB, 0xF4, 0xB0, 0x5A, 0xEB, 0xFE];

        fn halt_guest(entry2: u32, n: u8) -> Vec<u8> {
            let mut code = apic_setup(entry2);
            code.extend(pit_program(0x34, n));
            code.extend(STI_HLT_MARK_SPIN);
            code
        }

        fn run_set(
            boot: &mut LinuxDirectBoot,
            budget: u64,
        ) -> crate::firmware_board::LinuxSetRunReport {
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set = set(&cli, &sti, &halt);
            boot.board
                .run_linux_boot_cpl0_set(budget, &set, false, false, true)
                .expect("runs")
        }

        /// S4-38L8f: a halted CPU with IF = 1 under the APIC profile is woken at the PIT edge
        /// (`wake_ns` of the IOAPIC source), the vector of entry 2 goes through the guest IDT and
        /// the Local APIC (guest EOI), and the instruction after the `HLT` runs.
        #[test]
        fn an_idle_sti_hlt_is_woken_by_the_pit_through_the_ioapic() {
            let code = halt_guest(u32::from(IRQ_VECTOR), 10);
            let mut boot = apic_board(&code, true);
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            let report = run_set(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            // The clock skipped from the HLT to the first edge after the guest's PIT write.
            assert!(report.irq.halt_idle_ns > 0, "{report:?}");
            assert_eq!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
            let state = boot.board.state();
            assert_eq!(state.gpr[Register::Rsp as usize], rsp, "IRETQ balanced");
            assert_ne!(state.rflags & (1 << 9), 0, "IRETQ restored IF");
            assert_eq!(state.activity, Some(CpuActivity::Running));
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false),
                "the guest EOI cleared ISR"
            );
            let source = boot.board.pit_ioapic_source().expect("installed");
            let stats = source.stats();
            assert_eq!(
                (stats.acknowledged, stats.edges_refused, stats.ack_refused),
                (1, 0, 0),
                "{stats:?}"
            );
        }

        /// S4-38L8f: a masked entry 2 gives no wake time: the run ends as `halted`, not woken by
        /// a false deadline, and no vector reaches the guest.
        #[test]
        fn hlt_with_a_masked_entry_is_not_woken_under_the_apic_profile() {
            let code = halt_guest(MASKED | u32::from(IRQ_VECTOR), 10);
            let mut boot = apic_board(&code, true);
            let report = run_set(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
            assert_ne!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
        }

        /// S4-38L8f: stage4a (no IOAPIC source, no APIC windows) is unchanged: `STI; HLT` with the
        /// empty source ends the run as `halted`, nothing wakes it.
        #[test]
        fn stage4a_hlt_with_if_set_still_ends_the_run_as_halted() {
            let mut code = pit_program(0x34, 10);
            code.extend(STI_HLT_MARK_SPIN);
            let (mut boot, _) = irq_board(&code);
            assert!(boot.board.pit_ioapic_source().is_none());
            let report = run_set(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
        }

        /// LVT Timer bits: periodic mode, mask.
        const LVT_PERIODIC: u32 = 1 << 17;
        const LVT_MASKED: u32 = 1 << 16;

        /// Instructions of `apic_setup` (9) and of `lapic_timer_setup` (7) before `STI; HLT`, and
        /// the `STI` and `HLT` themselves: the clock reads 18 ns at the `HLT` (one ns each).
        const RETIRED_AT_HLT: u64 = 18;

        /// After `apic_setup` (the PIT entry masked, so the PIT is no source): divide by 1 (DCR
        /// `0xB`), the LVT Timer `lvt`, the initial count `count` (100 MHz: one count is 10 ns);
        /// then `EAX = 0` (the handler's EOI writes it).
        fn lapic_timer_setup(lvt: u32, count: u32) -> Vec<u8> {
            let mut code = apic_setup(MASKED | u32::from(IRQ_VECTOR));
            code.extend(lapic_timer_program(lvt, count));
            code
        }

        /// The timer registers only (`RBX` = the Local APIC base, as `apic_setup` leaves it);
        /// 7 instructions, `EAX = 0` afterwards.
        fn lapic_timer_program(lvt: u32, count: u32) -> Vec<u8> {
            let mut code = Vec::new();
            code.extend(mov32(0, 0xB));
            code.extend([0x89, 0x83, 0xE0, 0x03, 0x00, 0x00]); // MOV [RBX+0x3E0], EAX
            code.extend(mov32(0, lvt));
            code.extend([0x89, 0x83, 0x20, 0x03, 0x00, 0x00]); // MOV [RBX+0x320], EAX
            code.extend(mov32(0, count));
            code.extend([0x89, 0x83, 0x80, 0x03, 0x00, 0x00]); // MOV [RBX+0x380], EAX
            code.extend(mov32(0, 0));
            code
        }

        fn lapic_timer_guest(lvt: u32, count: u32) -> Vec<u8> {
            let mut code = lapic_timer_setup(lvt, count);
            code.extend(STI_HLT_MARK_SPIN);
            code
        }

        /// A board with the APIC profile's timer line and the clock owner (the owner dispatches
        /// the Local APIC timers; the APIC-profile run enables it, S4-23j).
        fn owned_apic_board(code: &[u8]) -> LinuxDirectBoot {
            let mut boot = apic_board(code, true);
            boot.board.enable_clock_owner().expect("the board can");
            boot
        }

        /// S4-38L8i: a one-shot Local APIC timer wakes a halted CPU with IF = 1 at its deadline
        /// (the clock owner and the arbiter, no PIT, no 8259), the vector goes through the Local
        /// APIC IRR and the guest IDT, the guest EOIs, and the code after the `HLT` runs.
        #[test]
        fn an_idle_sti_hlt_is_woken_by_a_one_shot_lapic_timer() {
            // 100 counts of 10 ns: the timer starts at 0 ns, its deadline is 1000 ns.
            let mut boot = owned_apic_board(&lapic_timer_guest(u32::from(IRQ_VECTOR), 100));
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            let report = run_set(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            // The clock jumped from the `HLT` to the deadline, not past it.
            assert_eq!(report.irq.halt_idle_ns, 1000 - RETIRED_AT_HLT, "{report:?}");
            // Only retired instructions and the idle jump moved it: it stood on the deadline when
            // the vector was delivered.
            assert_eq!(
                report.run.virtual_ns,
                report.run.retired + report.irq.halt_idle_ns,
                "{report:?}"
            );
            assert_eq!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
            let state = boot.board.state();
            assert_eq!(state.gpr[Register::Rsp as usize], rsp, "IRETQ balanced");
            assert_ne!(state.rflags & (1 << 9), 0, "IRETQ restored IF");
            assert_eq!(state.activity, Some(CpuActivity::Running));
            let lapic = boot.board.local_apic();
            assert_eq!(lapic.vector_state(IRQ_VECTOR), (false, false), "EOI");
            assert_eq!(lapic.next_timer_deadline(), None, "one-shot: fired once");
        }

        /// S4-38L8i: a periodic Local APIC timer wakes the halted CPU on every period: the guest
        /// loops `HLT; INC EDX; JMP HLT`, every period is one wake and one vector.
        #[test]
        fn an_idle_hlt_loop_is_woken_on_every_period_of_a_periodic_lapic_timer() {
            let mut code = lapic_timer_setup(LVT_PERIODIC | u32::from(IRQ_VECTOR), 100);
            // STI; HLT; INC EDX; JMP -5 (back to the HLT).
            code.extend([0xFB, 0xF4, 0xFF, 0xC2, 0xEB, 0xFB]);
            let mut boot = owned_apic_board(&code);
            let report = run_set(&mut boot, 300);
            // Every wake costs one unit of the budget: the run ends in a `HLT` with nothing left.
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(
                report.run.retired + report.irq.halt_wakes,
                300,
                "{report:?}"
            );
            let wakes = report.irq.halt_wakes;
            assert!(wakes >= 3, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, wakes, "{report:?}");
            // The setup and the first `STI; HLT` retire 18 instructions; every wake then runs the
            // handler (EOI, `IRETQ`), `INC EDX`, `JMP` and the next `HLT`: 5 more.
            assert_eq!(report.run.retired, RETIRED_AT_HLT + 5 * wakes, "{report:?}");
            // Each period is 1000 ns and the clock stands on every deadline: after the last wake
            // only its 5 instructions have run. The idle skips are the rest.
            assert_eq!(report.run.virtual_ns, 1000 * wakes + 5, "{report:?}");
            assert_eq!(
                report.irq.halt_idle_ns,
                report.run.virtual_ns - report.run.retired,
                "{report:?}"
            );
            assert_eq!(
                boot.board.state().gpr[Register::Rdx as usize] & 0xFFFF_FFFF,
                report.irq.halt_wakes,
                "{report:?}"
            );
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        /// S4-38L8i: a masked LVT Timer gives no wake time (a masked expiry raises nothing): the
        /// run ends as `halted`, no false deadline, no vector.
        #[test]
        fn hlt_with_a_masked_lapic_timer_is_not_woken() {
            let mut boot =
                owned_apic_board(&lapic_timer_guest(LVT_MASKED | u32::from(IRQ_VECTOR), 100));
            let report = run_set(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
            assert_ne!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
        }

        /// S4-38L8i: stage4a is unchanged: the Local APIC window is not served by its set loop, so
        /// the guest's first Local APIC register WRITE (SVR; the read path is the test above) is
        /// the typed `DeviceUnavailable` error, the guest never reaches the timer registers or
        /// its `HLT`, and no timer is armed.
        #[test]
        fn stage4a_lapic_timer_programming_is_a_typed_write_error_and_arms_nothing() {
            let (boot, run) = run_guest_code(&lapic_timer_guest(u32::from(IRQ_VECTOR), 100), false);
            assert!(
                matches!(
                    run.memory_error,
                    Some(crate::TrackedAccessError::DeviceUnavailable { address })
                        if address.get() == LOCAL_APIC_MMIO_BASE + 0xF0
                ),
                "{run:?}"
            );
            assert_ne!(run.exit, PagedProbeExit::Halted);
            assert!(!boot.board.clock_owner_enabled());
            assert_eq!(boot.board.local_apic().next_timer_deadline(), None);
        }

        /// S4-38L8i: a vector the Local APIC would not accept (TPR class not below the vector's
        /// class) gives no wake time, although the LVT Timer is unmasked and armed: the run ends
        /// as `halted`, the clock does not skip (the vector would sit in IRR and the CPU would
        /// pass its `HLT` with no interrupt).
        #[test]
        fn hlt_with_a_timer_vector_blocked_by_the_tpr_is_not_woken() {
            let mut code = lapic_timer_setup(u32::from(IRQ_VECTOR), 100);
            code.extend(mov32(0, u32::from(IRQ_VECTOR & 0xF0)));
            code.extend([0x89, 0x83, 0x80, 0x00, 0x00, 0x00]); // MOV [RBX+0x80], EAX: TPR
            code.extend(mov32(0, 0));
            code.extend(STI_HLT_MARK_SPIN);
            let mut boot = owned_apic_board(&code);
            let report = run_set(&mut boot, 300);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.halt_idle_ns, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
            assert_ne!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
            let lapic = boot.board.local_apic();
            assert_eq!(lapic.vector_state(IRQ_VECTOR), (false, false));
            assert!(lapic.next_timer_deadline().is_some(), "armed, never fired");
        }

        /// S4-38L8i: the PIT line is open (entry 2 unmasked on another vector, a count of 255
        /// ticks: the first edge is about 213 us away) and the Local APIC timer is due at 1000
        /// ns: the halted CPU is woken by the timer, not by the PIT.
        #[test]
        fn an_earlier_lapic_timer_wakes_the_cpu_before_the_pit_edge() {
            let mut code = apic_setup(u32::from(IRQ_VECTOR + 0x10));
            code.extend(pit_program(0x34, 255));
            code.extend(lapic_timer_program(u32::from(IRQ_VECTOR), 100));
            code.extend(STI_HLT_MARK_SPIN);
            let mut boot = owned_apic_board(&code);
            let report = run_set(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(
                report.run.virtual_ns,
                report.run.retired + report.irq.halt_idle_ns,
                "{report:?}"
            );
            assert!(
                (900..1000).contains(&report.irq.halt_idle_ns),
                "idle up to the LAPIC deadline, not the PIT edge: {report:?}"
            );
            assert_eq!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
            let source = boot.board.pit_ioapic_source().expect("installed");
            assert_eq!(source.stats().edges, 0, "the PIT edge has not come");
            let lapic = boot.board.local_apic();
            assert_eq!(lapic.vector_state(IRQ_VECTOR), (false, false));
            assert_eq!(lapic.vector_state(IRQ_VECTOR + 0x10), (false, false));
        }

        #[test]
        fn without_a_guest_eoi_the_second_irq_is_not_delivered() {
            let n = 10u8;
            let mut boot = apic_board(&timer_guest(u32::from(IRQ_VECTOR), n), false);
            run_until(&mut boot, 2000);
            let source = boot.board.pit_ioapic_source().expect("installed");
            let t0 = source.channel0_run().expect("loaded").start_tick;
            run_until(&mut boot, deadline(t0 + 40) + 1000);
            let stats = source.stats();
            assert_eq!(stats.acknowledged, 1, "{stats:?}");
            assert_eq!(stats.edges, 4);
            // In service, and one coalesced request waits in IRR.
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (true, true)
            );
        }

        /// S4-38L8d measurement (AGENTS.md: performance decisions rest on measurements): the wall
        /// time per retired instruction of a `JMP $` spin without the interrupt hook, with the hook and
        /// the empty source, and with the hook and the PIT -> IOAPIC -> Local APIC source (the PIT
        /// count of 65536 ticks gives no edge inside the run, so every boundary asks
        /// `pending_vector` and gets `None`). `cargo test -p od-machine --release --lib
        /// the_pending_vector_cost -- --ignored --nocapture`.
        #[test]
        #[ignore = "measurement, not an assertion of speed: run with --release -- --ignored --nocapture"]
        fn the_pending_vector_cost_per_boundary_is_measured() {
            const N: u64 = 20_000_000;
            const ROUNDS: usize = 7;
            let (cli, sti, halt) = (
                CliProfile::NonPviLong64V1,
                StiProfile::NonPviLong64V1,
                HaltProfile::Long64V1,
            );
            let set = set(&cli, &sti, &halt);
            let run = |mode: usize| -> f64 {
                let mut boot = match mode {
                    0 | 1 => irq_board(&[0xEB, 0xFE]).0,
                    _ => apic_board(&timer_guest(u32::from(IRQ_VECTOR), 0), true),
                };
                if mode == 2 {
                    // Let the guest program the APIC and the PIT first (a few dozen instructions).
                    boot.board
                        .run_linux_boot_cpl0_set(200, &set, false, false, true)
                        .expect("runs");
                }
                let started = std::time::Instant::now();
                let report = boot
                    .board
                    .run_linux_boot_cpl0_set(N, &set, false, false, mode != 0)
                    .expect("runs");
                let elapsed = started.elapsed();
                assert_eq!(report.run.exit, PagedProbeExit::BudgetExhausted);
                assert_eq!(report.irq.irqs_delivered, 0);
                elapsed.as_nanos() as f64 / report.run.retired as f64
            };
            let mut samples = [const { Vec::new() }; 3];
            for _ in 0..ROUNDS {
                for (mode, bucket) in samples.iter_mut().enumerate() {
                    bucket.push(run(mode));
                }
            }
            let names = [
                "no hook",
                "hook, empty source",
                "hook, PIT->IOAPIC->LAPIC source",
            ];
            for (name, bucket) in names.iter().zip(samples.iter_mut()) {
                bucket.sort_by(f64::total_cmp);
                println!(
                    "{name:34} median {:7.2} ns/instr  min {:7.2}  max {:7.2}",
                    bucket[ROUNDS / 2],
                    bucket[0],
                    bucket[ROUNDS - 1]
                );
            }
        }

        #[test]
        fn the_two_irq_sources_are_exclusive() {
            let (mut boot, _) = boot_with_entry(&[CLD]);
            assert!(boot.board.enable_pit_pic_irq());
            assert!(
                !boot.board.enable_pit_ioapic_irq(),
                "the 8259 source is installed"
            );
            assert!(boot.board.pit_ioapic_source().is_none());
            let (mut boot, _) = boot_with_entry(&[CLD]);
            assert!(boot.board.enable_pit_ioapic_irq());
            assert!(
                boot.board.enable_pit_ioapic_irq(),
                "a second call keeps the source"
            );
            assert!(
                !boot.board.enable_pit_pic_irq(),
                "the IOAPIC source is installed"
            );
            assert!(!boot.board.enable_uart_irq4(), "no 8259 source to wrap");
            // S4-38 (UART -> INTIN4): the IOAPIC UART source needs the IOAPIC PIT source and
            // excludes the 8259 variant in both directions.
            assert!(boot.board.enable_uart_ioapic_irq4());
            assert!(
                boot.board.enable_uart_ioapic_irq4(),
                "a second call keeps the source"
            );
            assert!(boot.board.uart_ioapic_source().is_some());
            assert!(boot.board.uart_irq_source().is_none());
            let (mut boot, _) = boot_with_entry(&[CLD]);
            assert!(
                !boot.board.enable_uart_ioapic_irq4(),
                "no IOAPIC PIT source to wrap"
            );
            assert!(boot.board.enable_pit_pic_irq());
            assert!(
                !boot.board.enable_uart_ioapic_irq4(),
                "the 8259 source is installed"
            );
            assert!(boot.board.enable_uart_irq4());
            assert!(boot.board.uart_ioapic_source().is_none());
        }

        fn members(profile: CpuidProfile) -> LinuxCpl0SetMembers {
            LinuxCpl0SetMembers {
                cpuid_profile: profile,
                ..LinuxCpl0SetMembers::linux_default()
            }
        }

        #[test]
        fn the_apic_profile_turns_on_irq_halt_and_the_clock_owner_and_stage4a_does_not() {
            let (mut boot, file) = boot_with_entry(&com1_program(b'X'));
            let record = run_set_facade(
                &mut boot,
                &file,
                64,
                members(CpuidProfile::X8664V1Stage4AApic),
            );
            assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
            assert_eq!(record.com1, b"X");
            for name in ["cpuid:X8664V1Stage4AApic", "halt", "irq", "clock_owner"] {
                assert!(record.set_members.iter().any(|m| m == name), "{name}");
            }
            assert!(!record.set_members.iter().any(|m| m == "user_mode"));
            assert!(boot.board.clock_owner_enabled());
            assert!(boot.board.pit_ioapic_source().is_some());
            assert!(boot.board.uart_ioapic_source().is_some());

            let (mut boot, file) = boot_with_entry(&com1_program(b'X'));
            let record =
                run_set_facade(&mut boot, &file, 64, members(CpuidProfile::X8664V1Stage4A));
            assert_eq!(
                record.exit,
                PagedProbeExit::Unsupported { opcode: 0xF4 },
                "{record:?}"
            );
            assert!(
                !record
                    .set_members
                    .iter()
                    .any(|m| m == "clock_owner" || m == "halt")
            );
            assert!(!boot.board.clock_owner_enabled());
            assert!(boot.board.pit_ioapic_source().is_none());
            assert!(boot.board.uart_ioapic_source().is_none());
        }

        // S4-38 (UART -> INTIN4). The guest programs the Local APIC (SVR) and redirection entry 4
        // (IOREGSEL 0x18), enables OUT2 and the RDA interrupt of the UART, and sets IF. The
        // receive byte is injected before the run (the S4-23c RX event of the host). The handler
        // at `IRQ_HANDLER_AT` (vector `IRQ_VECTOR`) counts in RSI, optionally reads RBR (which
        // clears the RDA condition), writes the Local APIC EOI and returns.

        const ENTRY4_EDGE: u32 = IRQ_VECTOR as u32;
        const ENTRY4_LEVEL: u32 = (1 << 15) | IRQ_VECTOR as u32;
        const ENTRY4_MASKED: u32 = (1 << 16) | IRQ_VECTOR as u32;

        /// `MOV AL, value; MOV DX, port; OUT DX, AL`.
        fn outb(port: u16, value: u8) -> Vec<u8> {
            vec![0xB0, value, 0x66, 0xBA, port as u8, (port >> 8) as u8, 0xEE]
        }

        /// SVR := 0x1FF, IOREGSEL := 0x18, IOWIN := `entry4`, MCR := OUT2, IER := RDA.
        fn uart_setup(entry4: u32) -> Vec<u8> {
            uart_setup_with(entry4, 0x1FF, 0x01)
        }

        /// The same with the SVR value `svr` and the IER value `ier`.
        fn uart_setup_with(entry4: u32, svr: u32, ier: u8) -> Vec<u8> {
            let mut code = mov32(3, LOCAL_APIC_MMIO_BASE as u32);
            code.extend(mov32(1, IO_APIC_MMIO_BASE as u32));
            code.extend(mov32(0, svr));
            code.extend([0x89, 0x83, 0xF0, 0x00, 0x00, 0x00]); // MOV [RBX+0xF0], EAX
            code.extend(mov32(0, 0x18));
            code.extend([0x89, 0x01]); // MOV [RCX], EAX
            code.extend(mov32(0, entry4));
            code.extend([0x89, 0x41, 0x10]); // MOV [RCX+0x10], EAX
            code.extend(outb(0x3FC, 0x08));
            code.extend(outb(0x3F9, ier));
            code
        }

        /// `INC RSI`, `[MOV DX, 0x3F8; IN AL, DX]` with `read_rbr`, `XOR EAX, EAX`, the EOI, `IRETQ`.
        fn uart_handler(read_rbr: bool) -> Vec<u8> {
            let mut code = vec![0x48, 0xFF, 0xC6];
            if read_rbr {
                code.extend([0x66, 0xBA, 0xF8, 0x03, 0xEC]);
            }
            code.extend([0x31, 0xC0]); // XOR EAX, EAX (the EOI register takes 0)
            code.extend([0x89, 0x83, 0xB0, 0x00, 0x00, 0x00]); // MOV [RBX+0xB0], EAX
            code.extend([0x48, 0xCF]);
            code
        }

        /// An APIC-profile board (IOAPIC PIT source and UART source installed) running `code`
        /// with a received byte waiting in the UART.
        fn uart_board(code: &[u8], read_rbr: bool) -> LinuxDirectBoot {
            let boot = uart_board_empty(code, read_rbr);
            boot.board.uart().inject_received_byte(b'A').unwrap();
            boot
        }

        /// The same board with nothing received yet.
        fn uart_board_empty(code: &[u8], read_rbr: bool) -> LinuxDirectBoot {
            let mut boot = apic_board(code, true);
            write_phys(&mut boot, IRQ_HANDLER_AT, &uart_handler(read_rbr));
            assert!(boot.board.enable_uart_ioapic_irq4());
            boot.board.state_mut().gpr[Register::Rsi as usize] = 0;
            boot
        }

        /// `uart_setup`, `STI`, spin.
        fn uart_guest(entry4: u32) -> Vec<u8> {
            let mut code = uart_setup(entry4);
            code.push(0xFB);
            code.extend([0xEB, 0xFE]);
            code
        }

        /// UART RX event -> INTIN4 (edge form) -> Local APIC IRR -> guest IDT -> handler -> EOI.
        #[test]
        fn a_uart_rx_event_reaches_the_guest_through_intin4_and_the_local_apic() {
            let mut boot = uart_board(&uart_guest(ENTRY4_EDGE), true);
            let rsp = boot.board.state().gpr[Register::Rsp as usize];
            let report = run_set(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            let state = boot.board.state();
            assert_eq!(state.gpr[Register::Rsi as usize], 1, "the handler ran once");
            assert_eq!(state.gpr[Register::Rsp as usize], rsp, "IRETQ balanced");
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false),
                "the guest EOI cleared ISR"
            );
            // The handler's RBR read took the byte and the condition.
            assert_eq!(boot.board.uart().received_byte(), None);
            assert_eq!(boot.board.uart().peek_iir(), 0x01);
            let source = boot.board.uart_ioapic_source().expect("installed");
            assert_eq!((source.uart_edges(), source.uart_refused()), (1, 0));
            assert_eq!(source.pit_source().stats().acknowledged, 1);
        }

        /// The same through the board's installed source, with the PIC untouched.
        #[test]
        fn the_uart_intin4_run_leaves_the_8259_alone() {
            let mut boot = uart_board(&uart_guest(ENTRY4_EDGE), true);
            let report = run_set(&mut boot, 300);
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            let pic = boot.board.pic_master().expect("the PIC exists").snapshot();
            assert_eq!((pic.irr, pic.isr), (0, 0));
            assert!(boot.board.uart_irq_source().is_none());
        }

        /// A masked entry 4: the line rises, nothing reaches the Local APIC.
        #[test]
        fn a_masked_intin4_gives_no_delivery() {
            let mut boot = uart_board(&uart_guest(ENTRY4_MASKED), true);
            let report = run_set(&mut boot, 300);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().gpr[Register::Rsi as usize], 0);
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
            // The byte is still waiting: nothing took it.
            assert_eq!(boot.board.uart().received_byte(), Some(b'A'));
            let source = boot.board.uart_ioapic_source().expect("installed");
            assert_eq!(source.uart_edges(), 1, "the line rose");
        }

        /// Level form, the handler does not read RBR: the line stays high, so the guest EOI
        /// redelivers the vector (Remote IRR semantics of INTIN4), again and again.
        #[test]
        fn a_level_intin4_with_the_line_still_active_is_redelivered_after_the_eoi() {
            let mut boot = uart_board(&uart_guest(ENTRY4_LEVEL), false);
            let report = run_set(&mut boot, 400);
            assert!(report.irq.irqs_delivered >= 2, "{report:?}");
            assert_eq!(
                boot.board.state().gpr[Register::Rsi as usize],
                report.irq.irqs_delivered
            );
            assert_eq!(boot.board.uart().received_byte(), Some(b'A'));
        }

        /// Level form, the handler reads RBR: the line falls inside the handler, so exactly one
        /// delivery happens and the EOI redelivers nothing.
        #[test]
        fn a_level_intin4_with_the_line_cleared_is_delivered_once() {
            let mut boot = uart_board(&uart_guest(ENTRY4_LEVEL), true);
            let report = run_set(&mut boot, 400);
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(boot.board.state().gpr[Register::Rsi as usize], 1);
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        /// Edge form, the handler does not read RBR: the EOI redelivers nothing (no new edge),
        /// so the vector comes once however long the line stays high.
        #[test]
        fn an_edge_intin4_with_the_line_still_active_is_delivered_once() {
            let mut boot = uart_board(&uart_guest(ENTRY4_EDGE), false);
            let report = run_set(&mut boot, 400);
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(boot.board.uart().received_byte(), Some(b'A'));
        }

        /// `uart_setup`, `STI`, `HLT`, a mark (`MOV AL, 0x5A`) and a spin.
        fn uart_halt_guest(entry4: u32) -> Vec<u8> {
            let mut code = uart_setup(entry4);
            code.extend(STI_HLT_MARK_SPIN);
            code
        }

        /// A halted CPU (`STI; HLT`) with a UART byte already waiting is woken (the sample before
        /// `wake_ns` puts the request in IRR) and the vector goes through the guest IDT.
        #[test]
        fn a_halted_cpu_is_woken_by_a_waiting_uart_rx_byte() {
            let mut boot = uart_board(&uart_halt_guest(ENTRY4_EDGE), true);
            // IF = 0 until the guest's `STI`, so the vector waits for the `HLT`.
            boot.board.state_mut().rflags &= !(1 << 9);
            let report = run_set(&mut boot, 400);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.halt_wakes, 1, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(boot.board.state().gpr[Register::Rax as usize] as u8, 0x5A);
            assert_eq!(boot.board.state().gpr[Register::Rsi as usize], 1);
        }

        /// With nothing received the line stays low: `STI; HLT` ends the run as `halted`, no
        /// wake and no delivery (the UART source gives a wake only for a live request).
        #[test]
        fn a_halted_cpu_with_a_quiet_uart_is_not_woken() {
            let mut boot = uart_board_empty(&uart_halt_guest(ENTRY4_EDGE), true);
            boot.board.state_mut().rflags &= !(1 << 9);
            let report = run_set(&mut boot, 400);
            assert_eq!(report.run.exit, PagedProbeExit::Halted, "{report:?}");
            assert_eq!(report.irq.halt_wakes, 0, "{report:?}");
            assert_eq!(report.irq.irqs_delivered, 0, "{report:?}");
            assert_eq!(boot.board.state().activity, Some(CpuActivity::Halted));
        }

        /// A level request refused by a software-disabled Local APIC (SVR bit 8 clear) is not
        /// lost: the source keeps trying, and once the guest enables the SVR the vector is
        /// delivered, exactly once (one accepted rise, one refused rise).
        #[test]
        fn a_refused_intin4_rise_is_retried_after_the_local_apic_is_enabled() {
            let mut code = uart_setup_with(ENTRY4_LEVEL, 0x0FF, 0x01);
            code.extend(mov32(0, 0x1FF));
            code.extend([0x89, 0x83, 0xF0, 0x00, 0x00, 0x00]); // MOV [RBX+0xF0], EAX
            code.push(0xFB); // STI
            code.extend([0xEB, 0xFE]);
            let mut boot = uart_board(&code, true);
            // IF = 0 until the guest's `STI`, after the SVR enable.
            boot.board.state_mut().rflags &= !(1 << 9);
            let report = run_set(&mut boot, 400);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(report.irq.irqs_delivered, 1, "{report:?}");
            assert_eq!(boot.board.state().gpr[Register::Rsi as usize], 1);
            let source = boot.board.uart_ioapic_source().expect("installed");
            assert_eq!((source.uart_refused(), source.uart_edges()), (1, 1));
            assert_eq!(boot.board.uart().received_byte(), None);
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        /// The THRE interrupt through INTIN4 (edge form): the handler writes the next byte of a
        /// table to THR (the THRE condition drops) and the loop's pump raises it again, one edge
        /// per byte; the NUL at the end writes nothing. N bytes give N + 1 deliveries (the IER
        /// enable is the first) and the N bytes reach COM1.
        #[test]
        fn the_thre_interrupt_through_intin4_sends_one_byte_per_delivery() {
            const TABLE: u64 = DATA + 0x3000;
            const MESSAGE: &[u8] = b"OD_NONCE=s4-38\n";
            let mut code = uart_setup_with(ENTRY4_EDGE, 0x1FF, 0x02);
            code.push(0xFB); // STI
            code.extend([0xEB, 0xFE]);
            let mut boot = uart_board_empty(&code, true);
            // `MOV AL, [RSI]; TEST AL, AL; JZ +8; MOV DX, 0x3F8; OUT DX, AL; INC RSI;
            // XOR EAX, EAX; MOV [RBX+0xB0], EAX; IRETQ`.
            let handler = [
                vec![0x8A, 0x06, 0x84, 0xC0, 0x74, 0x08],
                vec![0x66, 0xBA, 0xF8, 0x03, 0xEE, 0x48, 0xFF, 0xC6],
                vec![0x31, 0xC0, 0x89, 0x83, 0xB0, 0x00, 0x00, 0x00, 0x48, 0xCF],
            ]
            .concat();
            write_phys(&mut boot, IRQ_HANDLER_AT, &handler);
            let mut table = MESSAGE.to_vec();
            table.push(0);
            write_phys(&mut boot, TABLE, &table);
            boot.board.state_mut().gpr[Register::Rsi as usize] = TABLE;
            let report = run_set(&mut boot, 1500);
            assert_eq!(
                report.run.exit,
                PagedProbeExit::BudgetExhausted,
                "{report:?}"
            );
            assert_eq!(boot.board.com1_transcript(), MESSAGE, "{report:?}");
            assert_eq!(
                report.irq.irqs_delivered,
                MESSAGE.len() as u64 + 1,
                "{report:?}"
            );
            let source = boot.board.uart_ioapic_source().expect("installed");
            assert_eq!(source.uart_edges(), MESSAGE.len() as u64 + 1);
            assert_eq!(source.uart_refused(), 0);
        }

        // ---- S4-38L9-um: the user mode under the APIC profile (ISSUE-090 for the APIC board) ----

        /// A user-mode board for the APIC profile: `user_mode_board_with` plus the APIC windows,
        /// the kernel code descriptor at GDT 0x08, an interrupt gate `IRQ_VECTOR` to the handler
        /// at `USER_HANDLER_AT` (guest EOI, `IRETQ`; `RBX` = the Local APIC base from the prefix and
        /// `EAX` = 0 from the user spin) and a task register over a TSS whose RSP0 is `USER_RSP0`.
        /// `prefix` is the CPL0 setup that runs before the `IRETQ` to CPL3.
        fn user_apic_board(
            prefix: &[u8],
            user_code: &[u8],
            user_rflags: u64,
        ) -> (LinuxDirectBoot, Vec<u8>) {
            let (mut boot, file) = user_mode_board_with(prefix, user_code, true, user_rflags);
            map_apic_windows(&mut boot);
            write_phys(
                &mut boot,
                USER_GDT + 0x08,
                &0x00af_9b00_0000_ffff_u64.to_le_bytes(),
            );
            write_phys(&mut boot, USER_HANDLER_AT, &handler(true));
            let mut gate = [0u8; 16];
            gate[0..2].copy_from_slice(&(USER_HANDLER_AT as u16).to_le_bytes());
            gate[2..4].copy_from_slice(&0x08u16.to_le_bytes());
            gate[5] = 0x8E;
            gate[6..8].copy_from_slice(&((USER_HANDLER_AT >> 16) as u16).to_le_bytes());
            write_phys(&mut boot, USER_IDT_AT + u64::from(IRQ_VECTOR) * 16, &gate);
            write_phys(&mut boot, USER_TSS_AT + 4, &USER_RSP0.to_le_bytes());
            let state = boot.board.state_mut();
            state.idtr = Some(od_isa::state::Idtr {
                base: USER_IDT_AT,
                limit: 0xFFF,
            });
            state.tr = Some(od_isa::TaskRegisterCache {
                selector: 0x20,
                base: USER_TSS_AT,
                limit: 0x67,
                access: 0x8B,
                flags: 0,
            });
            (boot, file)
        }

        /// CPL3 code: `MOV EAX, 0; JMP -7` (a spin that retires two instructions per turn and
        /// keeps `EAX` = 0 for the handler's EOI write).
        const USER_SPIN: [u8; 7] = [0xB8, 0, 0, 0, 0, 0xEB, 0xF9];

        fn run_user_apic(boot: &mut LinuxDirectBoot, file: &[u8], budget: u64) -> LinuxStopRecord {
            run_linux_direct_board_set_user_mode(
                boot,
                &input(file, &[], BLOB, board_hash()),
                budget,
                members(CpuidProfile::X8664V1Stage4AApic),
            )
            .expect("the APIC profile runs in user mode")
        }

        /// The Local APIC timer armed by the host before the run (the clock owner of the facade
        /// then cuts the first quantum at its deadline): SVR enabled, a one-shot LVT timer of
        /// `IRQ_VECTOR`, divider 1, `count` counts of 10 ns. A timer armed by the guest inside a
        /// quantum is dispatched at the next quantum boundary (a Local APIC write does not end
        /// the quantum), so these tests do not arm it from the guest.
        fn arm_lapic_timer(boot: &mut LinuxDirectBoot, count: u64) {
            for (offset, value) in [
                (0xF0, 0x1ff),
                (0x320, u64::from(IRQ_VECTOR)),
                (0x3E0, 0xB),
                (0x380, count),
            ] {
                boot.board
                    .bus_mut()
                    .write_physical(
                        GuestPhysicalAddress::new(LOCAL_APIC_MMIO_BASE + offset),
                        crate::AccessWidth::Dword,
                        value,
                        |_| {},
                    )
                    .expect("the register accepts the write");
            }
        }

        #[test]
        fn a_lapic_timer_irq_in_cpl3_goes_through_the_idt_and_iretq_returns_to_cpl3() {
            // The timer, armed by the host, fires at 200 ns, while the user spin runs (one
            // instruction is one ns; the `IRETQ` to CPL3 retires at 1 ns). The handler runs at
            // CPL0 on `TSS.RSP0`, writes the EOI and `IRETQ`s to the spin, which goes on to the
            // end of the budget.
            let (mut boot, file) =
                user_apic_board(&mov32(3, LOCAL_APIC_MMIO_BASE as u32), &USER_SPIN, 0x202);
            arm_lapic_timer(&mut boot, 20);
            let record = run_user_apic(&mut boot, &file, 1000);
            assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
            assert_eq!(record.irqs_delivered, 1, "{record:?}");
            assert_eq!(record.irq_refused_masked, 0, "{record:?}");
            assert_eq!(record.cpl, 3, "{record:?}");
            assert_eq!(record.retired, 1000, "{record:?}");
            // The user code kept retiring after the return: the IRQ came after about 200 retired
            // user instructions, and everything but the `IRETQ` and the two handler
            // instructions is user code.
            assert!(record.cpl3_retired > 900, "{record:?}");
            assert!(
                (USER_CODE_AT..USER_CODE_AT + 7).contains(&record.rip),
                "{record:?}"
            );
            assert_eq!(
                record.gpr[Register::Rsp as usize],
                ENTRY + 0x800,
                "the frame on TSS.RSP0 was popped by the IRETQ"
            );
            for name in ["user_mode", "irq", "clock_owner", "halt"] {
                assert!(
                    record.set_members.iter().any(|member| member == name),
                    "{name}: {:?}",
                    record.set_members
                );
            }
            // The handler's EOI (a CPL0 access to the Local APIC) cleared the vector.
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
            let frame = USER_RSP0 & !15;
            // The frame the delivery pushed (no error code for an external vector) is still in
            // memory: the CPL3 SS, RSP, RFLAGS (IF set), CS and the interrupted user RIP.
            assert_eq!(read_qword(&mut boot, frame - 8), 0x1b, "SS");
            assert_eq!(read_qword(&mut boot, frame - 16), ENTRY + 0x800, "RSP");
            assert_ne!(read_qword(&mut boot, frame - 24) & (1 << 9), 0, "RFLAGS.IF");
            assert_eq!(read_qword(&mut boot, frame - 32), 0x13, "CS");
            let rip = read_qword(&mut boot, frame - 40);
            assert!((USER_CODE_AT..USER_CODE_AT + 7).contains(&rip), "{rip:#x}");
        }

        #[test]
        fn a_pit_ioapic_irq_in_cpl3_is_delivered_each_period_and_the_user_code_goes_on() {
            // PIT mode 2, count 10 (one edge every 8381 ns), entry 2 -> `IRQ_VECTOR`, all set up
            // by the guest at CPL0 before the `IRETQ`. Two edges fall in 20000 ns.
            let mut prefix = apic_setup(u32::from(IRQ_VECTOR));
            prefix.extend(pit_program(0x34, 10));
            for seed in [None, Some(24_301)] {
                let (mut boot, file) = user_apic_board(&prefix, &USER_SPIN, 0x202);
                if let Some(seed) = seed {
                    boot.board.enable_seeded_linux_interleaved(seed).unwrap();
                }
                let record = run_user_apic(&mut boot, &file, 20_000);
                assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
                assert!(record.irqs_delivered >= 2, "{record:?}");
                assert_eq!(record.irq_refused_masked, 0, "{record:?}");
                assert_eq!(record.cpl, 3, "{record:?}");
                assert!(record.cpl3_retired > 19_000, "{record:?}");
                assert_eq!(record.gpr[Register::Rsp as usize], ENTRY + 0x800);
                let source = boot.board.pit_ioapic_source().expect("installed");
                let stats = source.stats();
                assert_eq!(stats.acknowledged, record.irqs_delivered, "{stats:?}");
                assert_eq!(
                    (stats.edges_refused, stats.ack_refused, stats.queue_faults),
                    (0, 0, 0),
                    "{stats:?}"
                );
                assert_eq!(
                    boot.board.local_apic().vector_state(IRQ_VECTOR),
                    (false, false)
                );
            }
        }

        #[test]
        fn seeded_linux_ioapic_one_shot_lapic_timer_wakes_hlt() {
            let code = lapic_timer_guest(u32::from(IRQ_VECTOR), 100);
            let mut boot = apic_board(&code, true);
            let file = bzimage_with_entry(&code);
            boot.board.enable_seeded_linux_interleaved(24_301).unwrap();
            let stop = run_user_apic(&mut boot, &file, 300);
            assert_eq!(stop.exit_kind, "budget_exhausted", "{stop:?}");
            assert_eq!(
                (stop.retired, stop.halt_wakes, stop.irqs_delivered),
                (299, 1, 1)
            );
            assert!(stop.halt_idle_ns > 0);
            assert_eq!(stop.virtual_ns, stop.retired + stop.halt_idle_ns);
            assert_eq!(stop.gpr[Register::Rax as usize] as u8, 0x5a);
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
            assert_eq!(boot.board.local_apic().next_timer_deadline(), None);
        }

        #[test]
        fn threaded_linux_ioapic_pit_interrupts_return_to_cpl3() {
            let mut prefix = apic_setup(u32::from(IRQ_VECTOR));
            prefix.extend(pit_program(0x34, 10));
            for quantum in [1, 1024] {
                let (mut boot, file) = user_apic_board(&prefix, &USER_SPIN, 0x202);
                let report = run_threaded_apic(&mut boot, &file, 20_000, quantum)
                    .expect("the APIC worker runs in user mode");
                assert_eq!((report.threads_started, report.threads_exited), (1, 1));
                let stop = report.stop;
                assert_eq!(stop.exit, PagedProbeExit::BudgetExhausted, "{stop:?}");
                assert_eq!(stop.retired, 20_000);
                assert_eq!(stop.cpl, 3);
                assert!(stop.cpl3_retired > 19_000);
                assert!(stop.irqs_delivered >= 2);
                assert!(stop.set_members.iter().any(|name| name == "msr:apic_base"));
                assert_eq!(stop.gpr[Register::Rsp as usize], ENTRY + 0x800);
                assert!(boot.board.pit_pic_source().is_none());
                assert!(boot.board.uart_ioapic_source().is_some());
                let stats = boot.board.pit_ioapic_source().expect("installed").stats();
                assert_eq!(stats.acknowledged, stop.irqs_delivered);
                assert_eq!(stats.edges, stop.irq0_edges);
                assert_eq!(
                    (stats.edges_refused, stats.ack_refused, stats.queue_faults),
                    (0, 0, 0)
                );
            }
        }

        #[test]
        fn threaded_linux_ioapic_refuses_an_already_configured_pic_board() {
            let (mut boot, file) = boot_with_entry(&[0x90]);
            assert!(boot.board.enable_pit_pic_irq());
            let original_rip = boot.board.state().rip;
            let error = run_threaded_apic(&mut boot, &file, 1, 1).unwrap_err();
            assert_eq!((error.code, error.retired), ("run_failed", Some(0)));
            assert_eq!(boot.board.state().rip, original_rip);
            assert!(!boot.board.clock_owner_enabled());
            assert!(boot.board.pit_ioapic_source().is_none());
        }

        #[test]
        fn threaded_linux_ioapic_one_shot_lapic_timer_wakes_hlt() {
            let code = lapic_timer_guest(u32::from(IRQ_VECTOR), 100);
            let mut boot = apic_board(&code, true);
            let file = bzimage_with_entry(&code);
            let stop = run_threaded_apic(&mut boot, &file, 300, 1024).unwrap().stop;
            assert_eq!(stop.exit, PagedProbeExit::BudgetExhausted, "{stop:?}");
            assert_eq!(
                (stop.retired, stop.halt_wakes, stop.irqs_delivered),
                (300, 1, 1)
            );
            assert!(stop.halt_idle_ns > 0);
            assert_eq!(stop.virtual_ns, stop.retired + stop.halt_idle_ns);
            assert_eq!(stop.gpr[Register::Rax as usize] as u8, 0x5a);
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
            assert_eq!(boot.board.local_apic().next_timer_deadline(), None);
        }

        #[test]
        fn threaded_linux_ioapic_routes_uart_rx_through_intin4() {
            let code = uart_guest(ENTRY4_EDGE);
            let mut boot = uart_board(&code, true);
            let file = bzimage_with_entry(&code);
            let stop = run_threaded_apic(&mut boot, &file, 300, 1024).unwrap().stop;
            assert_eq!(stop.exit, PagedProbeExit::BudgetExhausted, "{stop:?}");
            assert_eq!(stop.irqs_delivered, 1);
            assert_eq!(stop.gpr[Register::Rsi as usize], 1);
            assert_eq!(boot.board.uart().received_byte(), None);
            let source = boot.board.uart_ioapic_source().expect("installed");
            assert_eq!((source.uart_edges(), source.uart_refused()), (1, 0));
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (false, false)
            );
        }

        fn run_threaded_apic(
            boot: &mut LinuxDirectBoot,
            file: &[u8],
            budget: u64,
            quantum: u64,
        ) -> Result<LinuxThreadedRunReport, LinuxDirectError> {
            run_linux_direct_board_set_user_mode_threaded(
                boot,
                &input(file, &[], BLOB, board_hash()),
                budget,
                members(CpuidProfile::X8664V1Stage4AApic),
                LinuxThreadedOptions {
                    quantum,
                    timeout: std::time::Duration::from_secs(5),
                },
            )
        }

        #[test]
        fn a_cpl3_irq_with_if_clear_is_refused_as_masked_and_stays_pending() {
            // The `IRETQ` frame carries IF = 0: the timer vector reaches the Local APIC IRR and
            // each user instruction boundary counts a masked refusal; nothing is delivered and
            // the run does not fail.
            let (mut boot, file) =
                user_apic_board(&mov32(3, LOCAL_APIC_MMIO_BASE as u32), &USER_SPIN, 0x2);
            arm_lapic_timer(&mut boot, 20);
            let record = run_user_apic(&mut boot, &file, 1000);
            assert_eq!(record.exit_kind, "budget_exhausted", "{record:?}");
            assert_eq!(record.irqs_delivered, 0, "{record:?}");
            assert!(record.irq_refused_masked > 100, "{record:?}");
            assert_eq!(record.cpl, 3, "{record:?}");
            assert_eq!(record.exceptions_delivered, 0, "{record:?}");
            assert!(record.cpl3_retired > 900, "{record:?}");
            assert_eq!(
                boot.board.local_apic().vector_state(IRQ_VECTOR),
                (true, false),
                "the vector waits in the IRR"
            );
            assert_eq!(boot.board.state().rflags & (1 << 9), 0);
        }

        /// S4-38L8g: `MOV ECX, 0x1B; MOV EDX, 0x66; <body>; HLT`.
        fn apic_base_guest(body: &[u8]) -> Vec<u8> {
            let mut code = mov32(1, 0x1B);
            code.extend(mov32(2, 0x66));
            code.extend_from_slice(body);
            code.push(0xF4);
            code
        }

        /// `MOV EAX, eax; MOV EDX, edx; WRMSR` (the guest's ECX is 0x1B from the prologue).
        fn wrmsr_apic_base(eax: u32, edx: u32) -> Vec<u8> {
            let mut code = mov32(0, eax);
            code.extend(mov32(2, edx));
            code.extend([0x0F, 0x30]);
            code
        }

        #[test]
        fn rdmsr_0x1b_returns_the_apic_base_with_bsp_and_en_under_the_apic_profile() {
            let (mut boot, file) = boot_with_entry(&apic_base_guest(&[0x0F, 0x32]));
            let record = run_set_facade(
                &mut boot,
                &file,
                64,
                members(CpuidProfile::X8664V1Stage4AApic),
            );
            assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
            // Oracle (SDM, by hand): base 0xFEE00000 | BSP (bit 8) | EN (bit 11) = 0xFEE00900.
            assert_eq!(record.gpr[Register::Rax as usize], 0xFEE0_0900);
            assert_eq!(record.gpr[Register::Rdx as usize], 0);
            assert_eq!(record.gpr[Register::Rcx as usize], 0x1B);
            assert!(record.set_members.iter().any(|m| m == "msr:apic_base"));
        }

        #[test]
        fn wrmsr_0x1b_accepts_only_the_matching_fields_and_refuses_the_rest_unchanged() {
            let apic = || members(CpuidProfile::X8664V1Stage4AApic);
            // The kernel's own value is accepted, and the read after it is the same constant.
            let mut body = wrmsr_apic_base(0xFEE0_0900, 0);
            body.extend([0x0F, 0x32]);
            let (mut boot, file) = boot_with_entry(&apic_base_guest(&body));
            let record = run_set_facade(&mut boot, &file, 64, apic());
            assert_eq!(record.exit, PagedProbeExit::Halted, "{record:?}");
            assert_eq!(record.gpr[Register::Rax as usize], 0xFEE0_0900);
            assert_eq!(record.gpr[Register::Rdx as usize], 0);
            // Another base, EN or BSP clear, an upper half: a typed refusal before the write.
            for (eax, edx) in [
                (0xFEE0_1900u32, 0u32),
                (0xFEE0_0800, 0),
                (0xFEE0_0100, 0),
                (0xFEE0_0900, 1),
            ] {
                let (mut boot, file) =
                    boot_with_entry(&apic_base_guest(&wrmsr_apic_base(eax, edx)));
                let record = run_set_facade(&mut boot, &file, 64, apic());
                assert_eq!(
                    record.exit,
                    PagedProbeExit::Unsupported { opcode: 0x30 },
                    "{eax:#x}:{edx:#x} {record:?}"
                );
                assert_eq!(record.retired, 4, "MOV ECX, MOV EDX, MOV EAX, MOV EDX");
                assert_eq!(record.gpr[Register::Rax as usize], u64::from(eax));
                assert_eq!(record.gpr[Register::Rdx as usize], u64::from(edx));
            }
        }

        #[test]
        fn stage4a_and_the_default_keep_the_0x1b_msr_access_refused() {
            for profile in [CpuidProfile::X8664V1Stage4A, CpuidProfile::DiagnosticV1] {
                let (mut boot, file) = boot_with_entry(&apic_base_guest(&[0x0F, 0x32]));
                let record = run_set_facade(&mut boot, &file, 64, members(profile));
                // Unchanged since S4-38L9v: the read of an unmodelled index is `#GP(0)` (no IDT
                // here, so the run ends with the fault), RAX and RDX as the MOVs left them.
                assert_eq!(
                    record.exit,
                    PagedProbeExit::Fault(od_isa::ArchitecturalFault {
                        vector: 13,
                        error_code: Some(0),
                        linear_address: None,
                    }),
                    "{record:?}"
                );
                assert_eq!(record.retired, 2, "MOV ECX, MOV EDX; the RDMSR faults");
                assert_eq!(record.gpr[Register::Rdx as usize], 0x66);
                assert!(!record.set_members.iter().any(|m| m == "msr:apic_base"));
                let (mut boot, file) =
                    boot_with_entry(&apic_base_guest(&wrmsr_apic_base(0xFEE0_0900, 0)));
                let record = run_set_facade(&mut boot, &file, 64, members(profile));
                assert_eq!(
                    record.exit,
                    PagedProbeExit::Unsupported { opcode: 0x30 },
                    "{record:?}"
                );
            }
            assert!(!LinuxCpl0SetMembers::minimal().msr_apic_base);
            assert!(!LinuxCpl0SetMembers::linux_default().msr_apic_base);
        }
    }

    // ---- S4-38L9s: ECAM (PCI MMCONFIG) data accesses of the set loop ----

    /// GPA of the ECAM byte for bus 0, `device`, `function`, `register`.
    fn ecam_gpa(device: u64, function: u64, register: u64) -> u64 {
        crate::PCI_ECAM_DESCRIPTOR.base + (device << 15) + (function << 12) + register
    }

    /// Maps GVA `SPLIT_VA + 0x1000 * (i + 1)` to the 4 KiB frame of `frames[i]` (accessed and
    /// dirty set, so a store to the frame writes no table entry), runs `code` (at
    /// `SPLIT_VA + 0x200`) and returns the outcome of the run without the fetch cache.
    fn run_ecam_program(frames: &[u64], code: &[u8]) -> CacheOutcome {
        run_mmio_program(frames, code, A_D_PRESENT_WRITABLE)
    }

    /// [`run_ecam_program`] with the leaf flags of the mapped frames chosen by the caller.
    fn run_mmio_program(frames: &[u64], code: &[u8], leaf_flags: u64) -> CacheOutcome {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let build = |boot: &mut LinuxDirectBoot| {
            let mut entries = vec![(0, SPLIT_PAGE0 | 3)];
            for (i, frame) in frames.iter().enumerate() {
                entries.push((i as u64 + 1, (frame & !0xfff) | leaf_flags));
            }
            write_tree(boot, TREE_PML4, &entries, None);
            write_phys(boot, SPLIT_PAGE0 + 0x200, code);
            tree_state(boot, TREE_PML4, SPLIT_VA + 0x200)
        };
        run_cache_case(&build, &switch_set(&cli, &halt), 32, false)
    }

    /// `MOV RBX, imm32; <body>; HLT`, RBX = the GVA of frame `index` plus `offset`.
    fn ecam_program(index: u64, offset: u32, body: &[u8]) -> Vec<u8> {
        let mut code = mov_imm32(3, (SPLIT_VA + 0x1000 * (index + 1)) as u32 + offset);
        code.extend_from_slice(body);
        code.push(0xF4);
        code
    }

    #[test]
    fn the_mcfg_base_of_the_stage4_blob_is_the_boards_ecam_region() {
        // The kernel takes the ECAM base from the MCFG of the blob; it is the one the board
        // registers as an MMIO region.
        let blob = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/guest/stage4/acpi_blob/stage4_acpi_blob.bin"
        ))
        .expect("the stage 4 blob fixture");
        // The table header: "MCFG", then its length (44-byte header + one 16-byte allocation).
        let at = blob
            .windows(8)
            .position(|w| &w[..4] == b"MCFG" && w[4..8] == 60u32.to_le_bytes())
            .expect("an MCFG table");
        let table = &blob[at..at + 60];
        let base = u64::from_le_bytes(table[44..52].try_into().unwrap());
        let (segment, start_bus, end_bus) = (
            u16::from_le_bytes(table[52..54].try_into().unwrap()),
            table[54],
            table[55],
        );
        assert_eq!(
            (base, segment, start_bus, end_bus),
            (0xFC10_0000, 0, 0, 0),
            "MCFG allocation"
        );
        let ecam = crate::PCI_ECAM_DESCRIPTOR;
        assert_eq!(base, ecam.base);
        assert_eq!(ecam.size, 0x10_0000);
        assert_eq!(
            (segment, start_bus, end_bus),
            (ecam.segment, ecam.start_bus, ecam.end_bus)
        );
    }

    #[test]
    fn an_ecam_dword_read_of_an_absent_function_is_all_ones() {
        // The access of `pci_mmcfg_read`: `MOV EAX, [RBX]` at bus 0, device 0, function 0,
        // register 0 (the boundary of S4-38L9r) and at other devices and functions.
        for (device, function, register) in [(0, 0, 0), (3, 2, 0x10), (31, 7, 0xFFC)] {
            let gpa = ecam_gpa(device, function, register);
            let code = ecam_program(0, (gpa & 0xfff) as u32, &[0x8B, 0x03]);
            let out = run_ecam_program(&[gpa], &code);
            assert_eq!(
                out.report.exit,
                PagedCpl0SetExit::Run(RunExit::Halted),
                "{:?}",
                out.report
            );
            assert_eq!(out.report.memory_error, None);
            assert_eq!(out.report.instructions_executed, 3);
            assert_eq!(out.state.gpr[Register::Rax as usize], 0xFFFF_FFFF);
            // Only the code page's accessed bit is written; the read writes nothing.
            assert_eq!(out.writes.len(), 1, "{:?}", out.writes);
        }
    }

    #[test]
    fn an_ecam_byte_and_word_read_of_an_absent_function_is_all_ones() {
        let gpa = ecam_gpa(0, 0, 2);
        let off = (gpa & 0xfff) as u32;
        let word = run_ecam_program(&[gpa], &ecam_program(0, off, &[0x66, 0x8B, 0x03]));
        assert_eq!(word.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
        assert_eq!(word.state.gpr[Register::Rax as usize] & 0xFFFF, 0xFFFF);
        let byte = run_ecam_program(&[gpa], &ecam_program(0, off, &[0x8A, 0x03]));
        assert_eq!(byte.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
        assert_eq!(byte.state.gpr[Register::Rax as usize] & 0xFF, 0xFF);
    }

    #[test]
    fn an_ecam_dword_write_to_an_absent_function_is_dropped() {
        // MOV EAX, 0x12345678 (B8 imm32); MOV [RBX], EAX; MOV ECX, [RBX].
        let gpa = ecam_gpa(0, 0, 0x10);
        let mut body = vec![0xB8, 0x78, 0x56, 0x34, 0x12, 0x89, 0x03];
        body.extend([0x8B, 0x0B]);
        let out = run_ecam_program(&[gpa], &ecam_program(0, (gpa & 0xfff) as u32, &body));
        assert_eq!(
            out.report.exit,
            PagedCpl0SetExit::Run(RunExit::Halted),
            "{:?}",
            out.report
        );
        assert_eq!(out.report.memory_error, None);
        assert_eq!(out.state.gpr[Register::Rax as usize], 0x1234_5678);
        assert_eq!(out.state.gpr[Register::Rcx as usize], 0xFFFF_FFFF);
        // The store reached no RAM and no table entry (the leaf has accessed and dirty set).
        assert_eq!(out.writes.len(), 1, "{:?}", out.writes);
    }

    #[test]
    fn a_read_just_past_the_ecam_region_keeps_the_old_error() {
        // GPA ECAM_BASE + 1 MiB is not in the board's region: the walker keeps the typed
        // physical-address error it gave before the ECAM was served by the set loop.
        let gpa = crate::PCI_ECAM_DESCRIPTOR.base + crate::PCI_ECAM_DESCRIPTOR.size;
        let code = ecam_program(0, 0, &[0x8B, 0x03]);
        let out = run_ecam_program(&[gpa], &code);
        assert!(
            matches!(
                out.report.memory_error,
                Some(crate::TrackedAccessError::OperandPhysical { address, .. })
                    if address.get() == gpa
            ),
            "{:?}",
            out.report
        );
        assert_ne!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
    }

    /// The page-table entry of mapped frame `index` (`run_mmio_program` numbering), read from the
    /// frames the outcome holds.
    fn ecam_leaf(out: &CacheOutcome, index: u64) -> u64 {
        let at = (0x3000 + 8 * (index + 1)) as usize;
        u64::from_le_bytes(out.memory[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn an_ecam_store_sets_the_dirty_bit_first_and_commits_one_table_write() {
        // The leaf has accessed set and dirty clear. The store sets dirty (one committed write of
        // the entry, before the device is reached) and the device drops the data.
        let gpa = ecam_gpa(0, 0, 0x10);
        let off = (gpa & 0xfff) as u32;
        let leaf_a = A_PRESENT_WRITABLE | (1 << 5);
        let store = ecam_program(0, off, &[0xB8, 0x78, 0x56, 0x34, 0x12, 0x89, 0x03]);
        let out = run_mmio_program(&[gpa], &store, leaf_a);
        assert_eq!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
        assert_eq!(out.report.memory_error, None);
        let entry = TREE_PML4 + 0x3000 + 8;
        assert_eq!(ecam_leaf(&out, 0) & (1 << 6), 1 << 6, "dirty set");
        assert_eq!(
            out.writes.iter().filter(|w| **w == (entry, 8)).count(),
            1,
            "{:?}",
            out.writes
        );
        // Code page accessed bit plus the one dirty write.
        assert_eq!(out.writes.len(), 2, "{:?}", out.writes);
        // A read does not set dirty.
        let load = ecam_program(0, off, &[0x8B, 0x03]);
        let out = run_mmio_program(&[gpa], &load, leaf_a);
        assert_eq!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
        assert_eq!(ecam_leaf(&out, 0) & (1 << 6), 0);
        assert_eq!(out.writes.len(), 1, "{:?}", out.writes);
    }

    #[test]
    fn the_set_loop_serves_ecam_and_no_other_mmio() {
        // The board has the Local APIC and the I/O APIC as MMIO regions; the set loop does not
        // reach them: the same typed error as before S4-38L9s, nothing read, nothing written, and
        // a store leaves the leaf's dirty bit alone.
        use crate::{IO_APIC_MMIO_BASE, LOCAL_APIC_MMIO_BASE};
        for base in [LOCAL_APIC_MMIO_BASE, IO_APIC_MMIO_BASE] {
            let leaf_a = A_PRESENT_WRITABLE | (1 << 5);
            let out = run_mmio_program(&[base], &ecam_program(0, 0x20, &[0x8B, 0x03]), leaf_a);
            assert!(
                matches!(
                    out.report.memory_error,
                    Some(crate::TrackedAccessError::OperandPhysical {
                        address,
                        source: crate::MemoryAccessError::UnreadableRegion(
                            crate::MemoryRegionKind::Mmio
                        ),
                    }) if address.get() == base + 0x20
                ),
                "{base:#x}: {:?}",
                out.report
            );
            assert_ne!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
            assert_eq!(out.state.gpr[Register::Rax as usize], 0);
            let store = ecam_program(0, 0x20, &[0x89, 0x03]);
            let out = run_mmio_program(&[base], &store, leaf_a);
            assert!(
                matches!(
                    out.report.memory_error,
                    Some(crate::TrackedAccessError::DeviceUnavailable { address })
                        if address.get() == base + 0x20
                ),
                "{base:#x}: {:?}",
                out.report
            );
            assert_eq!(
                ecam_leaf(&out, 0) & (1 << 6),
                0,
                "no dirty before the error"
            );
            assert_eq!(out.writes.len(), 1, "{:?}", out.writes);
        }
        // The dispatch itself refuses them, and serves the ECAM window.
        use crate::bus::SetStepMmio;
        let (mut boot, _) = boot_with_entry(&[0xF4]);
        let (_, io) = boot.board.bus_mut().split_for_set_step();
        let read = crate::RamAccessOperation::Read {
            width: crate::AccessWidth::Dword,
        };
        assert_eq!(
            io.windows(),
            [(
                crate::PCI_ECAM_DESCRIPTOR.base,
                crate::PCI_ECAM_DESCRIPTOR.size
            )]
        );
        for base in [LOCAL_APIC_MMIO_BASE, IO_APIC_MMIO_BASE] {
            assert!(
                io.dispatch(GuestPhysicalAddress::new(base), 4, read)
                    .is_err()
            );
        }
        let ecam = GuestPhysicalAddress::new(crate::PCI_ECAM_DESCRIPTOR.base);
        assert_eq!(io.dispatch(ecam, 4, read), Ok(0xFFFF_FFFF));
        let last = GuestPhysicalAddress::new(crate::PCI_ECAM_DESCRIPTOR.base + 0xF_FFFC);
        assert_eq!(io.dispatch(last, 4, read), Ok(0xFFFF_FFFF));
        let past = GuestPhysicalAddress::new(crate::PCI_ECAM_DESCRIPTOR.base + 0xF_FFFD);
        assert!(
            io.dispatch(past, 4, read).is_err(),
            "crosses the window end"
        );
    }

    #[test]
    fn a_split_access_never_reaches_the_ecam_device() {
        // Two adjacent ECAM pages (device 0, functions 0 and 1). A dword or word read, or a
        // dword store, across their boundary is refused as before S4-38L9s (the typed error of an
        // MMIO operand), not split into byte accesses to the device.
        let (page0, page1) = (ecam_gpa(0, 0, 0), ecam_gpa(0, 1, 0));
        let leaf_ad = A_D_PRESENT_WRITABLE;
        for (body, last_byte) in [
            (&[0x8Bu8, 0x03][..], 0xFFEu32),
            (&[0x66, 0x8B, 0x03][..], 0xFFF),
        ] {
            let out = run_mmio_program(&[page0, page1], &ecam_program(0, last_byte, body), leaf_ad);
            assert!(
                matches!(
                    out.report.memory_error,
                    Some(crate::TrackedAccessError::OperandPhysical {
                        source: crate::MemoryAccessError::UnreadableRegion(
                            crate::MemoryRegionKind::Mmio
                        ),
                        ..
                    })
                ),
                "{body:x?}: {:?}",
                out.report
            );
            assert_ne!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
            assert_eq!(out.state.gpr[Register::Rax as usize], 0, "{body:x?}");
        }
        let store = ecam_program(0, 0xFFE, &[0x89, 0x03]);
        let out = run_mmio_program(&[page0, page1], &store, leaf_ad);
        assert!(
            matches!(
                out.report.memory_error,
                Some(crate::TrackedAccessError::DeviceUnavailable { .. })
            ),
            "{:?}",
            out.report
        );
        assert_ne!(out.report.exit, PagedCpl0SetExit::Run(RunExit::Halted));
    }

    // ---- S4-38L4i: the opt-in checkpoint recorder on the board ----

    fn target(id: u8, name: &str, pa: u64) -> crate::LinuxCheckpointTarget {
        target_at(id, name, None, pa)
    }

    fn target_at(id: u8, name: &str, va: Option<u64>, pa: u64) -> crate::LinuxCheckpointTarget {
        crate::LinuxCheckpointTarget {
            id,
            name: name.to_string(),
            va,
            pa: Some(pa),
            cpl3: false,
            after: None,
        }
    }

    /// Runs `cr3_switch_program` through the board loop with the recorder and returns its records
    /// and the run report (`Debug` text, for equality) of a fresh board each time.
    fn run_switch_with_recorder(
        targets: Option<Vec<crate::LinuxCheckpointTarget>>,
        fetch_cache: bool,
        budget: u64,
    ) -> (Option<Vec<crate::LinuxCheckpointRecord>>, String) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        if let Some(targets) = targets {
            boot.board
                .enable_checkpoints(crate::LinuxCheckpointTable { targets })
                .expect("valid table");
        }
        let report = boot
            .board
            .run_linux_boot_cpl0_set(budget, &switch_set(&cli, &halt), false, fetch_cache, false)
            .expect("runs");
        (boot.board.take_checkpoints(), format!("{report:?}"))
    }

    #[test]
    fn checkpoints_follow_the_boundaries_of_the_board_loop_in_order() {
        // CLI, MOV RAX, MOV CR3 (retired 3), then the old RIP + 0xB is fetched through the new
        // tree: its physical address is in the code page of the second tree, not at ENTRY + 0xB.
        let new_gpa = TREE_CODE + 0x20B;
        for fetch_cache in [false, true] {
            let (records, _) = run_switch_with_recorder(
                Some(vec![
                    target_at(8, "after_cr3", Some(ENTRY + 0xB), new_gpa),
                    target_at(9, "old_mapping", Some(ENTRY + 0xB), ENTRY + 0xB),
                    target_at(10, "halt", Some(ENTRY + 0x12), TREE_CODE + 0x212),
                ]),
                fetch_cache,
                64,
            );
            let records = records.expect("recorder was enabled");
            let ids: Vec<u8> = records.iter().map(|record| record.id).collect();
            assert_eq!(ids, [0, 3, 8, 10], "{records:?}");
            let by = |id: u8| records.iter().find(|record| record.id == id).unwrap();
            let c0 = by(0);
            assert_eq!(
                (c0.rip, c0.gpa, c0.retired, c0.quanta, c0.cr3),
                (ENTRY, Some(ENTRY), 0, 1, c0.cr3)
            );
            let board_cr3 = c0.cr3;
            assert_ne!(board_cr3, TREE_PML4);
            let c3 = by(3);
            assert_eq!(
                (c3.kind, c3.rip, c3.retired),
                ("after_event", ENTRY + 0xB, 3)
            );
            assert_eq!((c3.cr3, c3.gpa), (TREE_PML4, Some(new_gpa)));
            let c8 = by(8);
            assert_eq!(
                (c8.rip, c8.gpa, c8.retired, c8.virtual_ns, c8.cpl),
                (ENTRY + 0xB, Some(new_gpa), 3, 3, 0)
            );
            assert_eq!(c8.bytes.len(), 16);
            // The HLT, the fourth instruction: the table row of the old mapping never matched.
            let c10 = by(10);
            assert_eq!(
                (c10.rip, c10.retired, c10.name.as_str()),
                (ENTRY + 0x12, 4, "halt")
            );
        }
    }

    #[test]
    fn the_recorder_does_not_change_the_run() {
        for fetch_cache in [false, true] {
            let (none, without) = run_switch_with_recorder(None, fetch_cache, 64);
            assert_eq!(none, None);
            let (some, with) = run_switch_with_recorder(Some(Vec::new()), fetch_cache, 64);
            assert!(some.is_some_and(|records| !records.is_empty()));
            assert_eq!(with, without, "fetch_cache {fetch_cache}");
        }
        // A budget that ends the run right after the MOV CR3 (before the next fetch).
        let (_, without) = run_switch_with_recorder(None, false, 3);
        let (records, with) = run_switch_with_recorder(Some(Vec::new()), false, 3);
        assert_eq!(with, without);
        let records = records.unwrap();
        // The budget ended the run on the boundary after the MOV CR3: C3 is not seen (the boundary
        // hook runs after the budget check), and nothing past C0 is invented.
        assert_eq!(records.iter().map(|r| r.id).collect::<Vec<_>>(), [0]);
    }

    // ---- S4-13b: the opt-in fetch cache report of the board loop ----

    /// `run_switch_with_recorder` with the report requested or not; returns the `Debug` text of
    /// the run report and the fetch cache report.
    fn run_switch_with_fetch_report(
        targets: Option<Vec<crate::LinuxCheckpointTarget>>,
        fetch_cache: bool,
        report_on: bool,
    ) -> (String, Option<crate::LinuxFetchCacheReport>) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        if let Some(targets) = targets {
            boot.board
                .enable_checkpoints(crate::LinuxCheckpointTable { targets })
                .expect("valid table");
        }
        if report_on {
            boot.board.enable_fetch_cache_report();
        }
        let report = boot
            .board
            .run_linux_boot_cpl0_set(64, &switch_set(&cli, &halt), false, fetch_cache, false)
            .expect("runs");
        let fetch_report = boot.board.take_fetch_cache_report();
        // Taking the report ends the request: a second take has nothing.
        assert_eq!(boot.board.take_fetch_cache_report(), None);
        (format!("{report:?}"), fetch_report)
    }

    /// The report changes nothing in the run, is split at the boundary of C5 (the HLT here, the
    /// fifth instruction) and its phases add up to the total.
    #[test]
    fn fetch_cache_report_splits_at_c5_and_leaves_the_run_unchanged() {
        let c5 = || {
            Some(vec![target_at(
                5,
                "e_entry",
                Some(ENTRY + 0x12),
                TREE_CODE + 0x212,
            )])
        };
        let mut walks_by_cache = Vec::new();
        for fetch_cache in [false, true] {
            let (plain, none) = run_switch_with_fetch_report(c5(), fetch_cache, false);
            assert_eq!(none, None, "no opt-in, no report");
            let (observed, report) = run_switch_with_fetch_report(c5(), fetch_cache, true);
            assert_eq!(observed, plain, "fetch_cache {fetch_cache}");
            let report = report.expect("requested");
            assert_eq!(report.cache_enabled, fetch_cache);
            assert_eq!(report.phase_rule, crate::FETCH_CACHE_PHASE_RULE_C5);
            assert!(report.c5_reached);
            // Four instructions retire before the boundary of C5 (the HLT, as in the checkpoint
            // test above); the HLT itself is after it.
            assert_eq!(report.total.retired, 5);
            assert_eq!(report.before_c5.retired, 4);
            assert_eq!(report.after_c5.retired, 1);
            assert_eq!((report.total.calls, report.before_c5.calls), (1, 1));
            assert_eq!(report.after_c5.calls, 0);
            // The HLT after the boundary is fetched by a walk or served by the cache.
            assert!(report.after_c5.walks.fetch + report.after_c5.cache.hits > 0);
            let mut both = report.before_c5;
            both += report.after_c5;
            assert_eq!(both, report.total);
            assert!(report.total.walks.fetch > 0, "{report:?}");
            if fetch_cache {
                assert!(report.total.cache.hits > 0, "{report:?}");
                assert!(report.total.cache.misses > 0, "{report:?}");
                assert!(report.total.cache.context_flushes > 0, "{report:?}");
            } else {
                assert_eq!(report.total.cache, crate::FetchCacheStats::default());
            }
            walks_by_cache.push(report.total.walks);
        }
        let (plain, cached) = (walks_by_cache[0], walks_by_cache[1]);
        assert_eq!(plain.data_read, cached.data_read);
        assert_eq!(plain.data_write, cached.data_write);
        assert!(cached.fetch < plain.fetch, "{cached:?} {plain:?}");
    }

    /// Without the checkpoint recorder nothing is split: everything is phase 1 and the report
    /// says which rule applied.
    #[test]
    fn fetch_cache_report_without_checkpoints_has_no_split() {
        let (_, report) = run_switch_with_fetch_report(None, true, true);
        let report = report.expect("requested");
        assert_eq!(report.phase_rule, crate::FETCH_CACHE_PHASE_RULE_NONE);
        assert!(!report.c5_reached);
        assert_eq!(report.before_c5, report.total);
        assert_eq!(report.after_c5, crate::LinuxFetchCachePhase::default());
        assert_eq!(report.total.retired, 5);
    }

    // ---- S4-13c: the opt-in shadow caches of the board loop ----

    /// `run_switch_with_fetch_report` with the sweep asked for or not (the fetch report is on in
    /// both runs); returns the `Debug` text of the run report and both reports.
    fn run_switch_with_sweep(
        sweep_on: bool,
        fetch_cache: bool,
    ) -> (
        String,
        Option<crate::LinuxFetchCacheReport>,
        Option<crate::FetchSweepReport>,
    ) {
        let (cli, halt) = (CliProfile::NonPviLong64V1, HaltProfile::Long64V1);
        let (mut boot, _) = boot_with_entry(&cr3_switch_program(TREE_PML4, 0x1111));
        install_second_tree(&mut boot, 0x2222);
        boot.board
            .enable_checkpoints(crate::LinuxCheckpointTable {
                targets: vec![target_at(
                    5,
                    "e_entry",
                    Some(ENTRY + 0x12),
                    TREE_CODE + 0x212,
                )],
            })
            .expect("valid table");
        boot.board.enable_fetch_cache_report();
        if sweep_on {
            boot.board
                .enable_fetch_cache_sweep(crate::SweepConfig::full_grid());
        }
        let report = boot
            .board
            .run_linux_boot_cpl0_set(64, &switch_set(&cli, &halt), false, fetch_cache, false)
            .expect("runs");
        let fetch_report = boot.board.take_fetch_cache_report();
        let sweep = boot.board.take_fetch_cache_sweep();
        // Taking the result ends the request.
        assert_eq!(boot.board.take_fetch_cache_sweep(), None);
        (format!("{report:?}"), fetch_report, sweep)
    }

    /// The sweep leaves the run and the real cache's counters as they were, the reference shadow
    /// counts exactly like the real cache in both phases and in total, and no sweep is built
    /// without the cache or without the request.
    #[test]
    fn the_sweep_leaves_the_board_run_unchanged_and_the_reference_shadow_equals_the_real_cache() {
        let (plain, plain_fetch, none) = run_switch_with_sweep(false, true);
        assert_eq!(none, None, "no request, no sweep");
        let (observed, fetch, sweep) = run_switch_with_sweep(true, true);
        assert_eq!(observed, plain, "the run report is the same byte for byte");
        assert_eq!(fetch, plain_fetch, "the real cache counts the same");
        let sweep = sweep.expect("requested");
        assert!(sweep.c5_reached);
        assert_eq!(sweep.configs.len(), 112);
        let reference = sweep
            .find(crate::SweepConfig::REFERENCE)
            .expect("the grid has the reference");
        let fetch = fetch.expect("report on");
        assert_eq!(reference.total, fetch.total.cache);
        assert_eq!(reference.before_c5, fetch.before_c5.cache);
        assert_eq!(reference.after_c5, fetch.after_c5.cache);
        assert!(reference.total.hits > 0 && reference.total.context_flushes > 0);
        // One call, hence one start of the shadows.
        assert_eq!(sweep.events.calls, fetch.total.calls);
        // Without the cache there are no events and no sweep.
        let (_, _, without_cache) = run_switch_with_sweep(true, false);
        assert_eq!(without_cache, None);
    }

    #[test]
    fn checkpoints_through_the_facade_cpuid_leaves_and_the_c7_stop() {
        // CLI; MOV EAX, 0; CPUID; MOV EAX, 1; CPUID. DiagnosticV1 answers leaf 0 and stops before
        // leaf 1 (cpuid_divergence), at the address the table calls C7.
        let program = [
            0xFA, 0xB8, 0, 0, 0, 0, 0x0F, 0xA2, 0xB8, 1, 0, 0, 0, 0x0F, 0xA2,
        ];
        let c7_gpa = ENTRY + 13;
        let table = crate::LinuxCheckpointTable {
            targets: vec![
                target(7, "cpuid1", c7_gpa),
                // One byte off: the same instruction, but not this address.
                target(8, "shifted", c7_gpa + 1),
            ],
        };
        let members = LinuxCpl0SetMembers::linux_default();
        let (mut boot, file) = boot_with_entry(&program);
        let with = run_linux_direct_board_set_observed(
            &mut boot,
            &input(&file, &[], BLOB, board_hash()),
            64,
            members,
            false,
            Some(table),
        )
        .expect("runs");
        assert_eq!(with.exit_kind, "cpuid_divergence");
        assert_eq!(with.cpuid_divergence_leaf, Some(1));
        let records = with.checkpoints.clone().expect("enabled");
        assert_eq!(
            records.iter().map(|record| record.id).collect::<Vec<_>>(),
            [0, 6, 7]
        );
        let c6 = &records[1];
        assert_eq!((c6.rip, c6.retired, c6.kind), (ENTRY + 6, 2, "boundary"));
        let c7 = &records[2];
        // The boundary of the stop: the same retired and quanta count as the stop record.
        assert_eq!((c7.rip, c7.gpa), (c7_gpa, Some(c7_gpa)));
        assert_eq!((c7.retired, c7.quanta), (with.retired, with.quanta));
        assert_eq!(c7.virtual_ns, with.virtual_ns);
        assert_eq!(with.retired, 4);

        // The same run without the table: the record is equal except for the field.
        let (mut boot, file) = boot_with_entry(&program);
        let without = run_linux_direct_board_set(
            &mut boot,
            &input(&file, &[], BLOB, board_hash()),
            64,
            members,
        )
        .expect("runs");
        assert_eq!(without.checkpoints, None);
        let mut with = with;
        with.checkpoints = None;
        assert_eq!(with, without);
    }

    #[test]
    fn a_checkpoint_table_with_an_id_out_of_range_is_refused_before_the_run() {
        let (mut boot, file) = boot_with_entry(&CHECKPOINT_PROGRAM_HLT);
        let error = run_linux_direct_board_set_observed(
            &mut boot,
            &input(&file, &[], BLOB, board_hash()),
            64,
            LinuxCpl0SetMembers::linux_default(),
            false,
            Some(crate::LinuxCheckpointTable {
                targets: vec![target(18, "beyond", ENTRY)],
            }),
        )
        .expect_err("refused");
        assert_eq!((error.code, error.stage), ("run_failed", "run"));
        assert_eq!(error.retired, Some(0));
        assert!(error.detail.contains("IdOutOfRange"), "{error:?}");
        // Nothing ran and no recorder was left on the board.
        assert_eq!(boot.board.state().rip, ENTRY);
        assert_eq!(boot.board.take_checkpoints(), None);
    }

    const CHECKPOINT_PROGRAM_HLT: [u8; 1] = [0xF4];

    // ---- S4-09f: system acceptance of the A/D bits through the set loop ----

    mod stage4_ad_acceptance;
}
