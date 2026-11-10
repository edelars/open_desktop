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
//!    (the SAME validate_acpi_blob call gives the bytes loaded)
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

use crate::GuestPhysicalAddress;
use crate::PagedProbeExit;
use crate::acpi_blob::{
    AcpiBlobError, AcpiManifestHash, ValidatedAcpiBlob, sha256, validate_acpi_blob,
};
use crate::firmware_board::{FirmwareBoardConfig, FirmwareRunReport, LoadedFirmwareBoard};
use crate::linux_boot::{LinuxBzImageHeader, LinuxHeaderError, parse_linux_bzimage_header};
use crate::linux_boot_params::{LINUX_E820_MAX_ENTRIES, LinuxBootParamsError};
use crate::linux_load::{LinuxLoadError, LinuxLoadInput, load_linux_direct};
use crate::linux_placement::LinuxPlacementError;
use od_isa::cpuid::CpuidProfile;
use od_isa::execute::{
    ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1, TranslationContextChange,
};
use od_isa::msr::MsrProfile;
use od_isa::{CliProfile, HaltProfile, Register};

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
    // 1. Header first: a truncated or foreign kernel is refused before anything else is looked at.
    parse_linux_bzimage_header(input.bzimage).map_err(header_error)?;
    // 2. The board this module assembles, and its own manifest hash.
    let config = FirmwareBoardConfig {
        ram_bytes: input.ram_bytes,
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
    let acpi = validate_acpi_blob(input.acpi_blob, &board_hash).map_err(acpi_error)?;
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
    /// `Stopped` then, see `cpuid_divergence_leaf`).
    pub exit_kind: &'static str,
    /// The `Debug` text of `exit` (carries the fault, rejection or backend details).
    pub exit_detail: String,
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
    pub rflags: u64,
    /// RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15 (the order of `od_isa::Register`).
    pub gpr: [u64; 16],
    pub retired: u64,
    pub quanta: u64,
    pub virtual_ns: u64,
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
        SetExtras {
            path: "paged_probe",
            members: Vec::new(),
            exit_detail: None,
            changes: &[],
            cpuid_divergence_leaf: None,
        },
    ))
}

/// What the set path adds to a stop record.
struct SetExtras<'a> {
    path: &'static str,
    members: Vec<String>,
    exit_detail: Option<String>,
    changes: &'a [TranslationContextChange],
    cpuid_divergence_leaf: Option<u32>,
}

fn stop_record(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    report: FirmwareRunReport,
    extras: SetExtras<'_>,
) -> LinuxStopRecord {
    let state = boot.board.state().clone();
    let (rip_gpa, rip_bytes, rip_bytes_error) =
        boot.board.fetch_view_2m(state.rip, LINUX_STOP_RIP_BYTES);
    let unsupported_opcode = match report.exit {
        PagedProbeExit::Unsupported { opcode } => Some(opcode),
        _ => None,
    };
    LinuxStopRecord {
        exit_kind: if extras.cpuid_divergence_leaf.is_some() {
            "cpuid_divergence"
        } else {
            exit_kind(&report.exit)
        },
        exit_detail: format!("{:?}", report.exit),
        exit: report.exit,
        unsupported_opcode,
        rip: state.rip,
        rip_gpa,
        rip_bytes,
        rip_bytes_error,
        mode: "long64",
        cpl: state.cs_selector.map_or(0, |cs| (cs & 3) as u8),
        cr0: state.cr0,
        cr3: boot.board.cr3(),
        cr4: state.cr4,
        efer: boot.board.efer(),
        rflags: state.rflags,
        gpr: state.gpr,
        retired: report.retired,
        quanta: report.quanta,
        virtual_ns: report.virtual_ns,
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
    /// `with_cpuid(DiagnosticV1)`: only what that profile declares, nothing more. With this
    /// member the run also stops before a `CPUID` of leaf 1 or 0x8000_0001 (`cpuid_divergence`,
    /// S4-38L4): the profile does not answer the feature leaves of the target.
    pub cpuid: bool,
    /// `with_rdtsc`: the source is the board clock, which also queues the 1 us RDTSC tick like
    /// `run_boot`; admitted only under the board's Linux profile.
    pub rdtsc: bool,
    /// `with_msr(EferReadOnlyV1(board EFER))`.
    pub msr: bool,
    /// `with_halt`.
    pub halt: bool,
    /// `with_pio_u8`: `IN AL, DX` / `OUT DX, AL` through the machine bus (COM1, PM timer, PIT).
    pub pio_u8: bool,
}

impl LinuxCpl0SetMembers {
    /// CLI and the CPU-owned CR3 only.
    pub const fn minimal() -> Self {
        Self {
            cpu_owned_cr3: true,
            cpuid: false,
            rdtsc: false,
            msr: false,
            halt: false,
            pio_u8: false,
        }
    }

    /// The set of the Linux board run (`--set-cpl0`, S4-38L4): CLI, the CPU-owned CR3 and
    /// CPUID (DiagnosticV1 with the divergence boundary). The first stop on the real kernel is
    /// a `CPUID` (leaf 0x8000_0000), which the profile answers; nothing else is added.
    pub const fn linux_default() -> Self {
        Self {
            cpuid: true,
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
/// CPUID boundary (S4-38L4, checkpoint C7): with `members.cpuid` the run stops BEFORE retiring a
/// `CPUID` whose EAX is 1 or 0x8000_0001 (`exit_kind` `cpuid_divergence`, `cpuid_divergence_leaf`
/// set). The board does not answer those leaves with the DiagnosticV1 values (a made-up feature
/// word would steer the kernel) and does not patch the kernel: the run ends at the honest edge.
/// Other leaves (0, 0x8000_0000, ...) are answered by DiagnosticV1 as the set does. Results past
/// the edge are diagnostic only. Without the member `CPUID` stays `Unsupported` (0xa2).
pub fn run_linux_direct_board_set(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    let cli = CliProfile::NonPviLong64V1;
    let cpuid = CpuidProfile::DiagnosticV1;
    let msr = MsrProfile::EferReadOnlyV1(boot.board.efer());
    let halt = HaltProfile::Long64V1;
    let mut names = vec!["cli".to_string()];
    let mut set = Cpl0ProfileSet::new(ControlRegisterPolicy::Cpl0BaseLong64V1).with_cli(&cli);
    if members.cpu_owned_cr3 {
        set = set.with_cpu_owned_cr3(MovCr3Long64V1);
        names.push("cpu_owned_cr3".into());
    }
    if members.cpuid {
        set = set.with_cpuid(&cpuid);
        names.push("cpuid:DiagnosticV1".into());
    }
    if members.rdtsc {
        set = set.with_rdtsc();
        names.push("rdtsc".into());
    }
    if members.msr {
        set = set.with_msr(&msr);
        names.push(format!("msr:{msr:?}"));
    }
    if members.halt {
        set = set.with_halt(&halt);
        names.push("halt".into());
    }
    if members.pio_u8 {
        set = set.with_pio_u8();
        names.push("pio_u8".into());
    }
    let report = boot
        .board
        .run_linux_boot_cpl0_set(instruction_budget, &set, members.cpuid)
        .map_err(|failure| {
            let mut error = LinuxDirectError::new("run_failed", "run", failure.error);
            error.retired = Some(failure.retired);
            error
        })?;
    Ok(stop_record(
        boot,
        input,
        report.run,
        SetExtras {
            path: "cpl0_set",
            members: names,
            exit_detail: Some(report.set_exit_detail),
            changes: &report.translation_context_changes,
            cpuid_divergence_leaf: report.cpuid_divergence_leaf,
        },
    ))
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
    fn cld_outside_the_allowlist_is_the_unsupported_stop_before_retirement() {
        // The first instruction of the real kernel (`startup_64`): CLD; CLI; XOR EAX,EAX.
        let record = run_entry(&[0xFC, 0xFA, 0x31, 0xC0], 64);
        assert_eq!(record.exit, PagedProbeExit::Unsupported { opcode: 0xFC });
        assert_eq!(record.exit_kind, "unsupported");
        assert_eq!(record.unsupported_opcode, Some(0xFC));
        assert_eq!((record.retired, record.rip), (0, 0x0100_0200));
        assert_eq!(&record.rip_bytes[..4], &[0xFC, 0xFA, 0x31, 0xC0]);
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

    use crate::paged_memory_adapter::{
        PagedCpl0SetExit, PagedCpl0SetReport, run_cpl0_set_paged_2m_ram,
    };
    use od_isa::execute::{
        ControlRegisterPolicy, Cpl0ProfileSet, MovCr3Long64V1, RunExit, TranslationContextChange,
    };
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};
    use od_isa::{CliProfile, CpuActivity, HaltProfile, INTEGER_FLAGS, LongModeState};

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
        let (cr3, efer) = (boot.board.cr3(), boot.board.efer());
        let mut state = boot.board.state().clone();
        state.cr3 = owned_cr3.then_some(cr3);
        let report = run_cpl0_set_paged_2m_ram(
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
        );
        (state, report)
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
    fn cpl0_set_board_dword_crossing_a_4k_boundary_is_the_typed_cross_page_stop() {
        let cli = CliProfile::NonPviLong64V1;
        // S4-11: the walker admits no split access; a crossing dword is `CrossesPage` before any
        // table or operand access, a backend error of the adapter (nothing retires, RAX stays).
        for offset in [0xFFD, 0xFFE, 0xFFF] {
            let (mut boot, _) = boot_with_entry(&[0x8B, 0x03]);
            let state = boot.board.state_mut();
            state.gpr[Register::Rax as usize] = 0x55;
            state.gpr[Register::Rbx as usize] = DATA + offset;
            let (state, report) = run_set(&mut boot, true, 8, &minimal_set(&cli));
            assert!(
                matches!(report.exit, PagedCpl0SetExit::Run(RunExit::MemoryError(_))),
                "{report:?}"
            );
            assert_eq!(report.instructions_executed, 0);
            assert_eq!(
                report.memory_error,
                Some(crate::TrackedAccessError::CrossesPage {
                    address: crate::GuestLinearAddress::new(DATA + offset),
                    width: 4
                })
            );
            assert_eq!(
                (state.rip, state.gpr[Register::Rax as usize]),
                (ENTRY, 0x55)
            );
            assert_eq!(report.page_fault, None);
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
    fn cpl0_set_board_word_and_dword_stores_crossing_a_4k_boundary_are_the_typed_cross_page_stop() {
        // (code, width, offsets that cross): the walker admits no split access and mutates
        // nothing (no byte, no accessed/dirty bit) before reporting `CrossesPage`.
        let cases: [(&[u8], u8, &[u64]); 2] = [
            (&[0x66, 0x89, 0x03], 2, &[0xFFF]),
            (&[0x89, 0x03], 4, &[0xFFD, 0xFFE, 0xFFF]),
        ];
        for (code, width, offsets) in cases {
            for &offset in offsets {
                let (mut boot, state, report, _) = run_store(code, offset);
                let fetch_only = tables_after_the_fetch_alone();
                assert!(
                    matches!(report.exit, PagedCpl0SetExit::Run(RunExit::MemoryError(_))),
                    "{report:?}"
                );
                assert_eq!(report.instructions_executed, 0);
                assert_eq!(
                    report.memory_error,
                    Some(crate::TrackedAccessError::CrossesPage {
                        address: crate::GuestLinearAddress::new(DATA + offset),
                        width
                    })
                );
                assert_eq!(report.page_fault, None);
                assert_eq!(state.rip, ENTRY);
                assert_eq!(
                    page_tables(&mut boot),
                    fetch_only,
                    "{code:02x?} at {offset:#x}"
                );
                assert_eq!(read_qword(&mut boot, DATA), QWORD_SEED);
                // No partial write reached the end of the page either.
                assert_eq!(
                    read_qword(&mut boot, DATA + 0xFF8),
                    0,
                    "{code:02x?} at {offset:#x}"
                );
                let mut next_page = [0u8; 8];
                read(&mut boot.board, DATA + 0x1000, &mut next_page).expect("read");
                assert_eq!(next_page, [0u8; 8], "the next page is untouched");
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
        // The same board run through the base profile is the L1 control: CLD is the stop.
        let (mut boot, file) = boot_with_entry(&PROLOGUE);
        let base = run_linux_direct_board(&mut boot, &input(&file, &[], BLOB, board_hash()), 64)
            .expect("runs");
        assert_eq!(base.exit, PagedProbeExit::Unsupported { opcode: 0xFC });
        assert_eq!(
            (base.retired, base.path, base.cr3_changes),
            (0, "paged_probe", 0)
        );
        assert!(base.set_members.is_empty() && base.set_exit_detail.is_none());
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
        assert_eq!(
            record.virtual_ns - base.virtual_ns,
            1,
            "the CLI is the only difference"
        );
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

    #[test]
    fn linux_default_set_is_cli_cr3_and_cpuid() {
        assert_eq!(
            LinuxCpl0SetMembers::linux_default(),
            LinuxCpl0SetMembers {
                cpuid: true,
                ..LinuxCpl0SetMembers::minimal()
            }
        );
        let (mut boot, file) = boot_with_entry(&[0xFA, 0xF4]);
        let record = run_set_facade(&mut boot, &file, 8, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.set_members,
            ["cli", "cpu_owned_cr3", "cpuid:DiagnosticV1"]
        );
        assert_eq!(record.cpuid_divergence_leaf, None);
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
            ["cli", "cpu_owned_cr3", "cpuid:DiagnosticV1", "halt"]
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
}
