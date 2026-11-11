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
use std::cell::Cell;

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
    /// The guest-visible IA32_EFER (`efer` plus the `SCE` a guest `WRMSR` may have set,
    /// S4-38L7f); `efer` stays the walker's EFER.
    pub guest_efer: u64,
    /// The guest PCI CONFIG_ADDRESS (port `0xCF8`, S4-38L7w) after the run; `0` when never written.
    pub pci_config_address: u32,
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
    /// S4-38L5c: how many guest `#PF` exceptions the CPL0 set board delivered through the guest
    /// IDT in this run (0 on the `paged_probe` path). A delivery is not a retired instruction.
    pub exceptions_delivered: u64,
    /// S4-38L7: the run was made with the diagnostic mode past C7
    /// ([`LinuxCpl0SetMembers::diagnostic_past_c7`]): the CPUID boundary was off, so the result is
    /// diagnostic only and is NOT counted. `false` on every other run.
    pub diagnostic_past_c7: bool,
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
            exceptions_delivered: 0,
            diagnostic_past_c7: false,
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
    exceptions_delivered: u64,
    diagnostic_past_c7: bool,
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
        guest_efer: boot.board.guest_efer(),
        pci_config_address: boot.board.pci_config_address(),
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
        exceptions_delivered: extras.exceptions_delivered,
        diagnostic_past_c7: extras.diagnostic_past_c7,
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
    /// `with_msr(EferReadOnlyV1(board EFER))` (S4-38L7e: in [`Self::linux_default`]): `RDMSR` of
    /// IA32_EFER at CPL0 gives the board EFER (`EDX:EAX`); `WRMSR` of it and every other MSR
    /// stay `Unsupported` (the EFER value is not changed by the run).
    pub msr: bool,
    /// `with_msr_fs_gs_base` (S4-38L6b): `WRMSR`/`RDMSR` of IA32_FS_BASE and IA32_GS_BASE at
    /// CPL0 (`EDX:EAX`, canonical 48-bit, stored in the CPU state); no other MSR. Independent of
    /// [`Self::msr`] (the read-only EFER profile), which it does not replace.
    pub msr_fs_gs_base: bool,
    /// `with_halt`.
    pub halt: bool,
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
}

impl LinuxCpl0SetMembers {
    /// CLI and the CPU-owned CR3 only.
    pub const fn minimal() -> Self {
        Self {
            cpu_owned_cr3: true,
            cpuid: false,
            rdtsc: false,
            msr: false,
            msr_fs_gs_base: false,
            halt: false,
            pio_u8: false,
            popf_cpl0: false,
            pci_config: false,
            fetch_cache: false,
            diagnostic_past_c7: false,
        }
    }

    /// The set of the Linux board run (`--set-cpl0`): CLI, the CPU-owned CR3, CPUID
    /// (DiagnosticV1 with the divergence boundary, S4-38L4), the qword `POPFQ` (S2-50), the FS/GS
    /// base MSRs (S4-38L6b), the read-only EFER (S4-38L7e), the PCI configuration ports
    /// (S4-38L7w) and the byte PIO (S4-38L7x: `pio_u8`, all ports bound on the board bus; a default
    /// decision that may be revisited: without COM1 and the timer the kernel does not go further).
    /// Known limit: byte accesses to `0xCF8..` use the bus model's own conf1 latch, dword accesses
    /// the `pci_config` cell; they agree while the bus has no PCI function.
    /// [`Self::minimal`] stays without it.
    pub const fn linux_default() -> Self {
        Self {
            cpuid: true,
            popf_cpl0: true,
            pio_u8: true,
            pci_config: true,
            msr: true,
            msr_fs_gs_base: true,
            fetch_cache: true,
            diagnostic_past_c7: false,
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
///
/// Diagnostic mode (S4-38L7, `members.diagnostic_past_c7`): the boundary is off. Leaf 1 and
/// 0x8000_0001 are answered by DiagnosticV1 as every other leaf and the run goes on; the record
/// carries `diagnostic_past_c7: true` and the result is NOT counted.
pub fn run_linux_direct_board_set(
    boot: &mut LinuxDirectBoot,
    input: &LinuxDirectInput<'_>,
    instruction_budget: u64,
    members: LinuxCpl0SetMembers,
) -> Result<LinuxStopRecord, LinuxDirectError> {
    let cli = CliProfile::NonPviLong64V1;
    let cpuid = CpuidProfile::DiagnosticV1;
    // The profile only names the member and seeds nothing under the cell, so it carries the
    // constant board EFER: `set_members` keeps `msr:EferReadOnlyV1(3328)` on every run.
    let msr = MsrProfile::EferReadOnlyV1(boot.board.efer());
    // S4-38L7f: the guest-visible EFER; the set writes it, the board keeps it between runs.
    let efer_cell = Cell::new(boot.board.guest_efer());
    // S4-38L7w: the guest CONFIG_ADDRESS, kept by the board between runs like the EFER.
    let pci_cell = Cell::new(boot.board.pci_config_address());
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
        set = set.with_msr(&msr).with_msr_efer_write(&efer_cell);
        names.push(format!("msr:{msr:?}"));
    }
    if members.msr_fs_gs_base {
        set = set.with_msr_fs_gs_base();
        names.push("msr:fs_gs_base".into());
    }
    if members.halt {
        set = set.with_halt(&halt);
        names.push("halt".into());
    }
    if members.pio_u8 {
        set = set.with_pio_u8();
        names.push("pio_u8".into());
    }
    if members.popf_cpl0 {
        set = set.with_popf_cpl0();
        names.push("popf_cpl0".into());
    }
    if members.pci_config {
        set = set.with_pci_config(&pci_cell);
        names.push("pci_config".into());
    }
    let result = boot.board.run_linux_boot_cpl0_set(
        instruction_budget,
        &set,
        members.cpuid && !members.diagnostic_past_c7,
        members.fetch_cache,
    );
    // The EFER the guest left stays whatever way the run ended.
    boot.board.set_guest_efer(efer_cell.get());
    boot.board.set_pci_config_address(pci_cell.get());
    let report = result.map_err(|failure| {
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
            exceptions_delivered: report.exceptions_delivered,
            diagnostic_past_c7: members.diagnostic_past_c7,
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

    /// S4-38L7x: `MOV EAX, 0x12345600; MOV EDX, 0x3F9; IN AL, DX; HLT` (the S4-38L7w boundary:
    /// a byte read of COM1 IER). `IN AL, DX` leaves EAX bits 8..32 alone.
    fn in_al_com1_ier_code() -> Vec<u8> {
        let mut code = mov_eax(0x1234_5600);
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
        assert_eq!(record.gpr[Register::Rax as usize], 0x1234_5600);
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
        assert_eq!(
            record.virtual_ns - base.virtual_ns,
            1,
            "the CLI is the only difference"
        );
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
    fn plain_cmpxchg_memory_form_without_lock_is_refused_in_the_cpl0_set() {
        // The set models only the LOCK form; the plain `48 0F B1 /r` memory form is refused
        // through the facade with opcode 0xB1 after the one preceding MOV retired.
        let mut code = mov_rax_imm64(5);
        code.extend([0x48, 0x0F, 0xB1, 0x14, 0x25, 0x20, 0x00, 0x00, 0x02]);
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
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0xB1 },
            "{record:?}"
        );
        assert_eq!(record.retired, 1);
        assert_eq!(boot.board.state().gpr[od_isa::Register::Rax as usize], 5);
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
        // STAR is not EFER: MOV ECX,0xC0000081; WRMSR is the stop as well.
        let star = [0xB9, 0x81, 0x00, 0x00, 0xC0, 0x0F, 0x30];
        let (mut boot, file) = boot_with_entry(&star);
        let record = run_set_facade(&mut boot, &file, 64, LinuxCpl0SetMembers::linux_default());
        assert_eq!(
            record.exit,
            PagedProbeExit::Unsupported { opcode: 0x30 },
            "{record:?}"
        );
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

    #[test]
    fn linux_default_set_is_cli_cr3_cpuid_and_popf() {
        assert_eq!(
            LinuxCpl0SetMembers::linux_default(),
            LinuxCpl0SetMembers {
                cpuid: true,
                popf_cpl0: true,
                pio_u8: true,
                pci_config: true,
                msr: true,
                msr_fs_gs_base: true,
                fetch_cache: true,
                ..LinuxCpl0SetMembers::minimal()
            }
        );
        assert!(!LinuxCpl0SetMembers::minimal().fetch_cache);
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
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
            ]
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
            [
                "cli",
                "cpu_owned_cr3",
                "cpuid:DiagnosticV1",
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "halt",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
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
                "msr:EferReadOnlyV1(3328)",
                "msr:fs_gs_base",
                "halt",
                "pio_u8",
                "popf_cpl0",
                "pci_config"
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
        // 2 MiB leaf (P, W, PS) onto the 2 MiB frame that holds `DATA`.
        let handler = pf_repair_handler(pde_address, (DATA & !(2 * MIB - 1)) | 0x83);
        write_phys(&mut boot, PF_HANDLER_AT, &handler);
        let rsp = boot.board.state().gpr[Register::Rsp as usize];
        // The store faults, the frame is pushed, six handler instructions retire and IRETQ is
        // the last of them: the store is back at RIP and RF is set (it came from the frame).
        let (state, report) = run_set(&mut boot, true, 6, &minimal_set(&cli));
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
            first.rip,
            PF_HANDLER_AT + 7,
            "in the handler, before the CR2 read"
        );
        assert_eq!(first.gpr[Register::Rdx as usize], 1);
        assert_eq!(boot.board.state().cr2, None, "the CPU state keeps no CR2");
        let second = run_set_facade(&mut boot, &file, 8, members);
        assert_eq!(second.exit, PagedProbeExit::Halted, "{second:?}");
        assert_eq!((second.retired, second.exceptions_delivered), (2, 0));
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
        let plain = run_cache_case(build, set, budget, false);
        let mut cached = run_cache_case(build, set, budget, true);
        assert_eq!(plain.report.fetch_cache_stats, None);
        let stats = cached.report.fetch_cache_stats.take().expect("cache stats");
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
}
