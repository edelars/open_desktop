//! Public entry of the Linux direct-boot loader (S4-37L6).
//!
//! The loader pieces of L1-L5 are `pub(crate)`; this module is the one door the runner uses. It
//! takes the raw bzImage, initrd, command line and ACPI blob, validates every input in a fixed
//! order, loads a board and hands back a plain-data description of what was placed. It never
//! runs the guest: the board comes back at the entry state, with no instruction retired
//! (S4-38 starts it).
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
use crate::acpi_blob::{
    AcpiBlobError, AcpiManifestHash, ValidatedAcpiBlob, sha256, validate_acpi_blob,
};
use crate::firmware_board::{FirmwareBoardConfig, LoadedFirmwareBoard};
use crate::linux_boot::{LinuxBzImageHeader, LinuxHeaderError, parse_linux_bzimage_header};
use crate::linux_boot_params::{LINUX_E820_MAX_ENTRIES, LinuxBootParamsError};
use crate::linux_load::{LinuxLoadError, LinuxLoadInput, load_linux_direct};
use crate::linux_placement::LinuxPlacementError;
use od_isa::Register;

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
    /// Instructions the guest retired: always 0, the loader never runs it.
    pub retired: u64,
}

/// A refused input. `code` is stable (see the module documentation), `stage` names the step,
/// `detail` is the `Debug` text of the underlying error.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LinuxDirectError {
    pub code: &'static str,
    pub stage: &'static str,
    pub detail: String,
}

impl LinuxDirectError {
    fn new(code: &'static str, stage: &'static str, detail: impl std::fmt::Debug) -> Self {
        Self {
            code,
            stage,
            detail: format!("{detail:?}"),
        }
    }

    /// Like [`Self::new`] with a detail that is already text (no `Debug` quoting).
    fn text(code: &'static str, stage: &'static str, detail: String) -> Self {
        Self {
            code,
            stage,
            detail,
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
}
