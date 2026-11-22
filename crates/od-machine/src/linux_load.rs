//! Linux direct-boot loader (S4-37L5): writes the kernel, initrd, `boot_params`, command line, GDT
//! and page tables to a board and prepares the 64-bit entry state of the Linux boot protocol,
//! then validates that state.
//!
//! Nothing here starts the guest (that is S4-38) and nothing is exported outside the crate.
//!
//! Order of guest memory writes (all through the machine side of the bus, never a guest store;
//! every range comes from [`plan_linux_placement`] and is disjoint from the others):
//!
//! ```text
//! 1. ACPI payload      [0xE_0000, +payload_len)      RSDP at its first byte
//! 2. ACPI FACS         [0xE_F000, +64)
//! 3. kernel            [pref_address, +pm_code_size) protected-mode code of the file (the rest of
//!                                                    the init_size window stays zero)
//! 4. initrd            [plan.initrd.start, +len)     only with an initrd
//! 5. command line      [0x10_1000, +len + 1)         NUL terminated
//! 6. boot_params       [0x10_0000, +0x1000)          serialized by L3
//! 7. GDT               [0x12_0000, +32)
//! 8. page tables       [0x13_0000, +3..6 pages)
//! ```
//!
//! Entry state (`Documentation/arch/x86/boot.rst`, 64-bit boot protocol; STAGE_4.md, S4-37L):
//!
//! | register       | value                                                              |
//! |----------------|--------------------------------------------------------------------|
//! | `RIP`          | `pref_address + 0x200` (`startup_64`; `pref_address` is not an entry) |
//! | `RSI`          | `0x10_0000`, the `boot_params` page                                 |
//! | `RSP`          | `0x15_0000`, the top of the stack zone of the plan (the kernel sets its own) |
//! | other GPRs     | 0 (a deterministic choice, see the note below)                     |
//! | `CS`           | `0x10` (`__BOOT_CS`, 64-bit flat code `0x00af_9b00_0000_ffff`)       |
//! | `SS/DS/ES`     | `0x18` (`__BOOT_DS`, flat data `0x00cf_9300_0000_ffff`)              |
//! | `GDTR`         | base `0x12_0000`, limit 31 (null, 32-bit code, `__BOOT_CS`, `__BOOT_DS`) |
//! | `RFLAGS`       | 2 (IF = 0)                                                         |
//! | `CR0`          | `0x8001_0033` (PG, PE, WP, NE, ET, MP)                              |
//! | `CR3`          | `0x13_0000`; the board's `page_table_root` (`cr3()`), `state.cr3` is `None` |
//! | `CR4`          | `0x20` (PAE only, LA57 = 0)                                        |
//! | `EFER`         | `0xD00` (LME, LMA, NXE); not a field of the CPU state, the board's  |
//!
//! Zero `RBP`, `RBX` and `RDI` is a requirement of the 32-bit boot protocol only. The 64-bit
//! entry does not need it; zero GPRs are harmless and keep the entry state deterministic. Only
//! `RSI` = `boot_params` is required by the 64-bit protocol.
//!
//! `CS = 0x10` (`__BOOT_CS`) and `SS/DS/ES = 0x18` (`__BOOT_DS`) are required by the boot
//! protocol and STAGE_4.md (S4-37L, "Состояние на входе"); the GDT therefore keeps a filler
//! descriptor at `0x08`. `CS = 0x08` is not a valid entry: it is only the injection that the test
//! `injection_wrong_cs` uses to prove the validator rejects a wrong code selector.
//!
//! The page tables are a 4-level identity map of low RAM `[0, min(R, 0xF400_0000))` with 2 MiB
//! leaves (`P|RW|PS`, executable); nothing else is mapped (no MMIO, no RAM above 4 GiB).
//!
//! `CR3` follows the board convention of S4-37L4: it is the board field `page_table_root`, and
//! `state.cr3` stays `None` (the base paged profile rejects a CPU-owned CR3). The board's own
//! `run` still walks 4 KiB leaves only, so a Linux `run` path is S4-38; the smoke test uses
//! `paged_probe::run_paged_probe_2m_ram` (base CPU profile + 2 MiB walk, added in L5). The
//! CPU-owned CR3 quantum `run_paged_cr3_probe_2m_ram` cannot run the smoke: it admits only
//! `MOV CR3` and `MOV RAX,[RBX]` (no `MOV imm`, `OUT`, `HLT`).

// Used by L6 / S4-38; tests exercise the module now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::acpi_blob::{ACPI_FACS_BYTES, ValidatedAcpiBlob, sha256};
use crate::firmware_board::{
    FIRMWARE_CR0, FIRMWARE_CR4, FIRMWARE_EFER, FIRMWARE_LOW_RAM_LIMIT, FirmwareBoardConfig,
    FirmwareBoardError, LoadedFirmwareBoard, assemble_board,
};
use crate::linux_boot::{LinuxBzImageHeader, LinuxHeaderError, parse_linux_bzimage_header};
use crate::linux_boot_params::{
    LinuxBootParamsError, LinuxBootParamsInput, LinuxE820Entry, build_linux_e820,
    check_e820_against_handoff, parse_handoff_regions, serialize_linux_boot_params,
};
use crate::linux_placement::{
    LinuxPlacement, LinuxPlacementError, LinuxPlacementInput, LinuxRange, LinuxZone,
    plan_linux_placement,
};
use crate::{BusAccessError, GuestPhysicalAddress, PciEcam};
use od_isa::execute::{Cr0Profile, Cr4Profile};
use od_isa::state::{CodeSegmentDescriptorCache, DataSegmentDescriptorCache, Gdtr};
use od_isa::{CpuActivity, LongModeState, Register, StiShadow};

/// `startup_64` is at offset 0x200 of the protected-mode code (boot.rst, 64-bit boot protocol).
pub(crate) const LINUX_ENTRY_OFFSET: u64 = 0x200;
/// `__BOOT_CS` and `__BOOT_DS` (boot.rst: "a GDT must be loaded with the descriptors for
/// selectors __BOOT_CS(0x10) and __BOOT_DS(0x18)").
pub(crate) const LINUX_BOOT_CS: u16 = 0x10;
pub(crate) const LINUX_BOOT_DS: u16 = 0x18;
/// GDT contents: null, 32-bit flat code (filler at 0x08), 64-bit flat code (`__BOOT_CS`), flat
/// data (`__BOOT_DS`). Access bytes have the accessed bit set, so no load writes the GDT.
pub(crate) const LINUX_GDT: [u64; 4] = [
    0,
    0x00cf_9b00_0000_ffff,
    0x00af_9b00_0000_ffff,
    0x00cf_9300_0000_ffff,
];
pub(crate) const LINUX_GDT_BYTES_WRITTEN: usize = LINUX_GDT.len() * 8;
pub(crate) const LINUX_GDT_LIMIT: u16 = (LINUX_GDT_BYTES_WRITTEN - 1) as u16;
/// `CR0` at entry: PG|WP|NE|ET|MP|PE, the only value the base paged CPU profile admits
/// (`Cr0Profile::BASE`). One definition, `FIRMWARE_CR0` of `firmware_board.rs` (S4-37L6).
pub(crate) const LINUX_ENTRY_CR0: u64 = FIRMWARE_CR0;
const _: () = assert!(FIRMWARE_CR0 == Cr0Profile::BASE);
/// `CR4` at entry: PAE only (`Cr4Profile::BASE`, one definition `FIRMWARE_CR4`). LA57 (bit 12)
/// must be clear (S4-37L, "Состояние на входе").
pub(crate) const LINUX_ENTRY_CR4: u64 = FIRMWARE_CR4;
const _: () = assert!(FIRMWARE_CR4 == Cr4Profile::BASE);
pub(crate) const LINUX_CR4_LA57: u64 = 1 << 12;
const RFLAGS_IF: u64 = 1 << 9;
const RFLAGS_ENTRY: u64 = 2;

const PAGE_4K: u64 = 0x1000;
const PAGE_2M: u64 = 0x20_0000;
const GIB: u64 = 1 << 30;
const ENTRY_PRESENT: u64 = 1;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_PS: u64 = 1 << 7;
const ENTRY_NX: u64 = 1 << 63;
/// Physical address bits of a table entry (bits 12..=51).
const ENTRY_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
/// Physical address bits of a 2 MiB leaf (bits 21..=51).
const ENTRY_2M_ADDRESS_MASK: u64 = 0x000f_ffff_ffe0_0000;
/// Reserved bits 13..=20 of a 2 MiB leaf (bit 12 is the PAT bit).
const ENTRY_2M_RESERVED: u64 = 0x1f_e000;
const TABLE_FLAGS: u64 = ENTRY_PRESENT | ENTRY_WRITABLE;
const LEAF_FLAGS: u64 = ENTRY_PRESENT | ENTRY_WRITABLE | ENTRY_PS;

/// Everything [`load_linux_direct`] needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinuxLoadInput<'a> {
    /// Board configuration; must have the Linux profile (`acpi_pm`, `acpi_tables`, `pci_ecam`).
    pub config: &'a FirmwareBoardConfig,
    /// The bzImage file.
    pub bzimage: &'a [u8],
    /// The initrd; `None` or empty means no initrd.
    pub initrd: Option<&'a [u8]>,
    /// The command line without the terminating NUL.
    pub cmdline: &'a [u8],
    /// The validated ACPI blob and its raw bytes (`validate_acpi_blob` input).
    pub acpi: &'a ValidatedAcpiBlob,
    pub acpi_blob: &'a [u8],
}

/// A board with the Linux image written and the entry state set (and validated).
///
/// `board` is a [`LoadedFirmwareBoard`] only in type: its `handoff_block()` is empty and
/// `boot_page_table_bytes()` is the size of the Linux identity tables, not the firmware's;
/// `cr3()` is the Linux table root. `run_boot` (4 KiB walker) must not be used on it: the Linux
/// table uses 2 MiB leaves; `run_linux_boot` (S4-38L1) is the 2 MiB walk.
pub(crate) struct LoadedLinuxBoard {
    pub board: LoadedFirmwareBoard,
    pub header: LinuxBzImageHeader,
    pub plan: LinuxPlacement,
    pub ram_size: u64,
}

impl LoadedLinuxBoard {
    /// Re-validates the entry state and the memory it depends on (see [`validate_linux_entry`]).
    pub(crate) fn validate_entry(&mut self) -> Result<(), LinuxLoadError> {
        check_entry_in_kernel(self.header.pm_code_size)?;
        validate_linux_entry(&mut self.board, &self.plan, self.ram_size)
    }
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum LinuxLoadError {
    /// The board configuration lacks `linux_profile`.
    NotLinuxProfile,
    Board(FirmwareBoardError),
    Header(LinuxHeaderError),
    Placement(LinuxPlacementError),
    BootParams(LinuxBootParamsError),
    /// The E820 map disagrees with the handoff block of the assembled board (S4-37L6).
    Handoff(LinuxBootParamsError),
    /// The command line contains a NUL byte at this index.
    CmdlineContainsNul {
        at: usize,
    },
    /// An input does not fit in the 32-bit length the plan takes.
    InputTooLarge {
        zone: LinuxZone,
    },
    /// The protected-mode code range is not inside the file.
    KernelOutOfFile,
    /// The payload or FACS range of the blob is not inside the blob bytes.
    AcpiBlobTooShort {
        blob_len: usize,
    },
    /// The length of `acpi_blob` differs from the length `acpi` was validated from: the two
    /// inputs of [`LinuxLoadInput`] do not describe the same bytes.
    AcpiBlobMismatch,
    /// The identity map needs more bytes than the page table zone holds.
    PageTablesTooLarge {
        needed: u64,
        available: u64,
    },
    /// Low RAM is not a multiple of 2 MiB (the board validates RAM, so never for a real board).
    LowRamNotAligned {
        low_end: u64,
    },
    /// The page table root is not a 4 KiB aligned physical address (bits 12..=51 only).
    PageTableRootInvalid {
        root: u64,
    },
    /// Low RAM is above 512 GiB, the reach of one PDPT (the identity map has one PD per GiB).
    LowRamTooLarge {
        low_end: u64,
    },
    /// The entry offset `0x200` is not inside the protected-mode code that is written.
    EntryOutsideKernel {
        entry_offset: u64,
        pm_code_size: u64,
    },
    /// A machine-side write failed.
    Write {
        zone: LinuxZone,
        error: BusAccessError,
    },
    /// A machine-side read failed (validation).
    Read {
        address: u64,
        error: BusAccessError,
    },

    // ---- entry state validator ----
    /// `RIP` is not `pref_address + 0x200` (for example the start of the image).
    EntryRip {
        expected: u64,
        found: u64,
    },
    /// `RSI` is not the `boot_params` page.
    EntryRsi {
        expected: u64,
        found: u64,
    },
    /// A general register other than `RSI` and `RSP` is not zero.
    EntryGpr {
        register: usize,
        value: u64,
    },
    /// `RSP` is not the top of the stack zone.
    EntryRsp {
        expected: u64,
        found: u64,
    },
    /// `CS` is not `__BOOT_CS` (or its hidden descriptor differs).
    EntryCs {
        found: Option<u16>,
    },
    EntryCsCache,
    /// `SS`, `DS` or `ES` is not `__BOOT_DS` (or the hidden descriptor differs).
    EntryDataSegment {
        register: &'static str,
        found: Option<u16>,
    },
    /// `RFLAGS.IF` is set.
    EntryInterruptsEnabled {
        rflags: u64,
    },
    /// `RFLAGS` differs from 2 with IF clear.
    EntryRflags {
        found: u64,
    },
    EntryCr0 {
        found: Option<u64>,
    },
    /// `CR4.LA57` is set (5-level paging is not what the tables describe).
    EntryCr4La57 {
        cr4: u64,
    },
    EntryCr4 {
        found: Option<u64>,
    },
    EntryEfer {
        found: u64,
    },
    /// The board `CR3` is not the page table zone start, or the state carries a CPU-owned CR3
    /// (`state` is `Some`; the entry state keeps it `None`).
    EntryCr3 {
        expected: u64,
        state: Option<u64>,
        board: u64,
    },
    /// `STI` shadow or CPU activity is not the running state with no inhibition.
    EntryActivity,
    /// `GDTR` is not (GDT zone start, limit 31).
    EntryGdtr {
        found: Option<Gdtr>,
    },
    /// The GDT bytes in memory differ from [`LINUX_GDT`].
    GdtMismatch,

    // ---- page table walk ----
    /// The identity map has no entry for this address; `level` is 4 (PML4) .. 2 (PD).
    PageNotMapped {
        zone: LinuxZone,
        address: u64,
        level: u8,
    },
    /// An entry is not present+writable+executable (or is a leaf where a table is expected,
    /// or has reserved bits).
    PageBadEntry {
        zone: LinuxZone,
        address: u64,
        level: u8,
        entry: u64,
    },
    /// The 2 MiB leaf of `address` maps another physical address.
    PageNotIdentity {
        zone: LinuxZone,
        address: u64,
        mapped: u64,
    },
    /// The first 2 MiB page above low RAM is mapped (MMIO and the holes must stay unmapped).
    HighWindowMapped {
        address: u64,
    },
    /// A PML4 (`level` 4) or PDPT (`level` 3) entry points to a table that is not entirely
    /// inside the page table zone.
    PageTablePointerOutsideZone {
        level: u8,
        target: u64,
    },
    /// The page tables in memory differ from [`build_linux_page_tables`] at this byte offset
    /// (from the start of the zone), though every structural rule above holds.
    PageTablesMismatch {
        first_diff: usize,
    },
}

impl From<LinuxPlacementError> for LinuxLoadError {
    fn from(error: LinuxPlacementError) -> Self {
        Self::Placement(error)
    }
}

impl From<LinuxBootParamsError> for LinuxLoadError {
    fn from(error: LinuxBootParamsError) -> Self {
        Self::BootParams(error)
    }
}

impl From<FirmwareBoardError> for LinuxLoadError {
    fn from(error: FirmwareBoardError) -> Self {
        Self::Board(error)
    }
}

/// Builds the 4-level identity map of `[0, low_end)` with 2 MiB leaves, to be written at
/// `root`: page 0 is the PML4, page 1 the PDPT, then one PD per GiB (`3..=6` pages for the
/// Linux profile, below the 16 pages of the zone). `root` must be 4 KiB aligned and `low_end` a
/// non-zero multiple of 2 MiB not above 512 GiB (the reach of the single PDPT).
pub(crate) fn build_linux_page_tables(root: u64, low_end: u64) -> Result<Vec<u8>, LinuxLoadError> {
    if root & !ENTRY_ADDRESS_MASK != 0 {
        return Err(LinuxLoadError::PageTableRootInvalid { root });
    }
    if low_end == 0 || !low_end.is_multiple_of(PAGE_2M) {
        return Err(LinuxLoadError::LowRamNotAligned { low_end });
    }
    if low_end > 512 * GIB {
        return Err(LinuxLoadError::LowRamTooLarge { low_end });
    }
    let pd_count = low_end.div_ceil(GIB) as usize;
    // Only PDPT entries 0..pd_count are used; `low_end <= 512 GiB` keeps pd_count <= 512.
    let mut entries = vec![0u64; 512 * (2 + pd_count)];
    entries[0] = (root + PAGE_4K) | TABLE_FLAGS;
    for pd in 0..pd_count {
        entries[512 + pd] = (root + PAGE_4K * (2 + pd as u64)) | TABLE_FLAGS;
        for index in 0..512usize {
            let address = (pd as u64) * GIB + (index as u64) * PAGE_2M;
            if address < low_end {
                entries[512 * (2 + pd) + index] = address | LEAF_FLAGS;
            }
        }
    }
    Ok(entries.iter().flat_map(|e| e.to_le_bytes()).collect())
}

fn gdt_bytes() -> Vec<u8> {
    LINUX_GDT.iter().flat_map(|d| d.to_le_bytes()).collect()
}

/// The entry state of the module documentation for `plan`.
pub(crate) fn linux_entry_state(plan: &LinuxPlacement) -> LongModeState {
    let code = CodeSegmentDescriptorCache {
        base: 0,
        limit: 0xffff_ffff,
        access: 0x9b,
        // L and G: 64-bit code, 4 KiB granularity.
        flags: 0xa,
    };
    let data = DataSegmentDescriptorCache {
        base: 0,
        limit: 0xffff_ffff,
        access: 0x93,
        // D/B and G.
        flags: 0xc,
    };
    let mut state = LongModeState {
        rip: plan.kernel.start + LINUX_ENTRY_OFFSET,
        rflags: RFLAGS_ENTRY,
        cr0: Some(LINUX_ENTRY_CR0),
        cr4: Some(LINUX_ENTRY_CR4),
        cs_selector: Some(LINUX_BOOT_CS),
        cs_cache: Some(code),
        ss_selector: Some(LINUX_BOOT_DS),
        ss_cache: Some(data),
        ds_selector: LINUX_BOOT_DS,
        es_selector: LINUX_BOOT_DS,
        ds_cache: Some(data),
        es_cache: Some(data),
        gdtr: Some(Gdtr {
            base: plan.gdt.start,
            limit: LINUX_GDT_LIMIT,
        }),
        sti_shadow: Some(StiShadow::Clear),
        activity: Some(CpuActivity::Running),
        ..LongModeState::default()
    };
    state.gpr[Register::Rsp as usize] = plan.stack.end;
    state.gpr[Register::Rsi as usize] = plan.boot_params.start;
    state
}

fn write(
    board: &mut LoadedFirmwareBoard,
    zone: LinuxZone,
    address: u64,
    bytes: &[u8],
) -> Result<(), LinuxLoadError> {
    board
        .bus_mut()
        .write_flat_bytes(GuestPhysicalAddress::new(address), bytes, |_| {})
        .map_err(|error| LinuxLoadError::Write { zone, error })
}

/// The entry `kernel.start + 0x200` must be inside the protected-mode code that is written.
fn check_entry_in_kernel(pm_code_size: u64) -> Result<(), LinuxLoadError> {
    if LINUX_ENTRY_OFFSET >= pm_code_size {
        return Err(LinuxLoadError::EntryOutsideKernel {
            entry_offset: LINUX_ENTRY_OFFSET,
            pm_code_size,
        });
    }
    Ok(())
}

/// The E820 map must agree with the handoff block the board builds for the same configuration:
/// the ACPI and NVS entries equal handoff regions 11 and 12, and the ECAM, framebuffer and ROM
/// regions of the handoff are reserved in E820 (S4-37L6). The block is the one of the real
/// assembled board, so the check follows the board map and not a copy of it.
pub(crate) fn check_e820_against_handoff_block(
    entries: &[LinuxE820Entry],
    handoff: &[u8],
) -> Result<(), LinuxLoadError> {
    let regions = parse_handoff_regions(handoff).map_err(LinuxLoadError::Handoff)?;
    check_e820_against_handoff(entries, &regions).map_err(LinuxLoadError::Handoff)
}

/// Checked sub-slice `[offset, offset + len)`.
fn slice(bytes: &[u8], offset: usize, len: usize) -> Option<&[u8]> {
    bytes.get(offset..offset.checked_add(len)?)
}

/// Loads a bzImage (and optionally an initrd) into a fresh board and sets the 64-bit entry
/// state. Every cheap input error (header, placement, E820, `boot_params`, page table size, blob
/// ranges) is reported before the board is assembled; the entry state is validated before the
/// board is returned, so an `Ok` board always passes [`validate_linux_entry`].
pub(crate) fn load_linux_direct(
    input: &LinuxLoadInput<'_>,
) -> Result<LoadedLinuxBoard, LinuxLoadError> {
    let config = input.config;
    if !config.linux_profile {
        return Err(LinuxLoadError::NotLinuxProfile);
    }
    let header = parse_linux_bzimage_header(input.bzimage).map_err(LinuxLoadError::Header)?;
    if let Some(at) = input.cmdline.iter().position(|&b| b == 0) {
        return Err(LinuxLoadError::CmdlineContainsNul { at });
    }
    let cmdline_len =
        u32::try_from(input.cmdline.len()).map_err(|_| LinuxLoadError::InputTooLarge {
            zone: LinuxZone::Cmdline,
        })?;
    let initrd = input.initrd.filter(|bytes| !bytes.is_empty());
    let ram_size = config.ram_bytes;
    let acpi = input.acpi;
    let plan = plan_linux_placement(
        &header,
        &LinuxPlacementInput {
            ram_size,
            initrd_len: initrd.map_or(0, |bytes| bytes.len() as u64),
            cmdline_len,
            acpi,
        },
    )?;
    let kernel = usize::try_from(header.protected_mode_offset)
        .ok()
        .zip(usize::try_from(header.pm_code_size).ok())
        .and_then(|(offset, len)| slice(input.bzimage, offset, len))
        .ok_or(LinuxLoadError::KernelOutOfFile)?;
    check_entry_in_kernel(header.pm_code_size)?;
    let blob_short = || LinuxLoadError::AcpiBlobTooShort {
        blob_len: input.acpi_blob.len(),
    };
    let acpi_payload =
        slice(input.acpi_blob, acpi.payload_offset, acpi.payload_len).ok_or_else(blob_short)?;
    let acpi_facs = slice(input.acpi_blob, acpi.facs_offset, ACPI_FACS_BYTES as usize)
        .ok_or_else(blob_short)?;
    // `acpi` and `acpi_blob` arrive separately: the description must come from these very
    // bytes (`validate_acpi_blob` records the blob SHA-256), so a caller cannot pair a
    // description with other bytes.
    if acpi.blob_sha256 != sha256(input.acpi_blob) {
        return Err(LinuxLoadError::AcpiBlobMismatch);
    }

    let e820 = build_linux_e820(&plan, ram_size)?;
    let boot_params = serialize_linux_boot_params(&LinuxBootParamsInput::new(
        input.bzimage,
        &plan,
        ram_size,
        &e820,
        acpi,
    ))?;
    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);
    let tables = build_linux_page_tables(plan.page_tables.start, low_end)?;
    if tables.len() as u64 > plan.page_tables.len() {
        return Err(LinuxLoadError::PageTablesTooLarge {
            needed: tables.len() as u64,
            available: plan.page_tables.len(),
        });
    }

    let assembled = assemble_board(config, PciEcam::new(), None, None)?;
    check_e820_against_handoff_block(&e820, assembled.handoff_block())?;
    let mut board = assembled.into_linux_board(plan.page_tables.start, tables.len() as u64)?;
    write(
        &mut board,
        LinuxZone::AcpiTables,
        acpi.load_base,
        acpi_payload,
    )?;
    write(
        &mut board,
        LinuxZone::AcpiFacs,
        acpi.facs_address,
        acpi_facs,
    )?;
    write(&mut board, LinuxZone::Kernel, plan.kernel.start, kernel)?;
    if let (Some(range), Some(bytes)) = (plan.initrd, initrd) {
        write(&mut board, LinuxZone::Initrd, range.start, bytes)?;
    }
    let mut cmdline = input.cmdline.to_vec();
    cmdline.push(0);
    write(&mut board, LinuxZone::Cmdline, plan.cmdline.start, &cmdline)?;
    write(
        &mut board,
        LinuxZone::BootParams,
        plan.boot_params.start,
        &boot_params,
    )?;
    write(&mut board, LinuxZone::Gdt, plan.gdt.start, &gdt_bytes())?;
    write(
        &mut board,
        LinuxZone::PageTables,
        plan.page_tables.start,
        &tables,
    )?;
    *board.state_mut() = linux_entry_state(&plan);

    let mut loaded = LoadedLinuxBoard {
        board,
        header,
        plan,
        ram_size,
    };
    loaded.validate_entry()?;
    Ok(loaded)
}

fn read_bytes(
    board: &mut LoadedFirmwareBoard,
    address: u64,
    output: &mut [u8],
) -> Result<(), LinuxLoadError> {
    board
        .bus_mut()
        .read_flat_bytes(GuestPhysicalAddress::new(address), output)
        .map_err(|error| LinuxLoadError::Read { address, error })
}

fn read_u64(board: &mut LoadedFirmwareBoard, address: u64) -> Result<u64, LinuxLoadError> {
    let mut raw = [0u8; 8];
    read_bytes(board, address, &mut raw)?;
    Ok(u64::from_le_bytes(raw))
}

enum Walk {
    /// A valid 2 MiB leaf.
    Leaf(u64),
    /// A PML4/PDPT entry points to a table outside the page table zone.
    OutsideZone {
        level: u8,
        target: u64,
    },
    NotPresent {
        level: u8,
    },
    Bad {
        level: u8,
        entry: u64,
    },
}

/// Walks the 4-level tables at `root` for `linear`. Every level must be present, writable and
/// executable; PML4 and PDPT entries must point to tables inside `tables` (the page table zone,
/// checked before the next read), the PD entry must be a 2 MiB leaf.
fn walk(
    board: &mut LoadedFirmwareBoard,
    root: u64,
    tables: LinuxRange,
    linear: u64,
) -> Result<Walk, LinuxLoadError> {
    let mut table = root & ENTRY_ADDRESS_MASK;
    for level in [4u8, 3, 2] {
        let shift = 12 + 9 * (u32::from(level) - 1);
        let index = (linear >> shift) & 511;
        let entry = read_u64(board, table + index * 8)?;
        if entry & ENTRY_PRESENT == 0 {
            return Ok(Walk::NotPresent { level });
        }
        if entry & ENTRY_WRITABLE == 0 || entry & ENTRY_NX != 0 {
            return Ok(Walk::Bad { level, entry });
        }
        if level == 2 {
            if entry & ENTRY_PS == 0 || entry & ENTRY_2M_RESERVED != 0 {
                return Ok(Walk::Bad { level, entry });
            }
            return Ok(Walk::Leaf(entry & ENTRY_2M_ADDRESS_MASK));
        }
        if entry & ENTRY_PS != 0 {
            return Ok(Walk::Bad { level, entry });
        }
        let target = entry & ENTRY_ADDRESS_MASK;
        if target < tables.start || target > tables.end.saturating_sub(PAGE_4K) {
            return Ok(Walk::OutsideZone { level, target });
        }
        table = target;
    }
    unreachable!("the walk returns at level 2")
}

/// Walks one address of `zone`; the leaf must map the 2 MiB page of `linear` onto itself.
fn check_identity(
    board: &mut LoadedFirmwareBoard,
    root: u64,
    tables: LinuxRange,
    zone: LinuxZone,
    linear: u64,
) -> Result<(), LinuxLoadError> {
    match walk(board, root, tables, linear)? {
        Walk::Leaf(mapped) if mapped == linear & ENTRY_2M_ADDRESS_MASK => Ok(()),
        Walk::Leaf(mapped) => Err(LinuxLoadError::PageNotIdentity {
            zone,
            address: linear,
            mapped,
        }),
        Walk::OutsideZone { level, target } => {
            Err(LinuxLoadError::PageTablePointerOutsideZone { level, target })
        }
        Walk::NotPresent { level } => Err(LinuxLoadError::PageNotMapped {
            zone,
            address: linear,
            level,
        }),
        Walk::Bad { level, entry } => Err(LinuxLoadError::PageBadEntry {
            zone,
            address: linear,
            level,
            entry,
        }),
    }
}

/// Every 2 MiB page that `range` touches, as the first linear address inside the range.
fn pages_of(range: LinuxRange) -> impl Iterator<Item = u64> {
    let mut next = Some(range.start);
    std::iter::from_fn(move || {
        let address = next?;
        if address >= range.end {
            return None;
        }
        next = (address & !(PAGE_2M - 1)).checked_add(PAGE_2M);
        Some(address)
    })
}

/// Checks the entry state of a loaded Linux board against the contract of the module
/// documentation, in this order: `RIP`, `RSI`, other GPRs, `RSP`, `CS`, `SS/DS/ES`, `RFLAGS`,
/// `CR0`, `CR4`, `EFER`, `CR3`, activity, `GDTR`; then the GDT bytes in memory and the page
/// tables: every zone of the plan is identity-mapped by 2 MiB leaves, and the first 2 MiB page
/// above low RAM is not mapped; last, the page table bytes must equal [`build_linux_page_tables`]
/// byte for byte. Each rule has its own error variant.
pub(crate) fn validate_linux_entry(
    board: &mut LoadedFirmwareBoard,
    plan: &LinuxPlacement,
    ram_size: u64,
) -> Result<(), LinuxLoadError> {
    let state = board.state().clone();
    let efer = board.efer();
    let board_cr3 = board.cr3();

    let rip = plan.kernel.start + LINUX_ENTRY_OFFSET;
    if state.rip != rip {
        return Err(LinuxLoadError::EntryRip {
            expected: rip,
            found: state.rip,
        });
    }
    let rsi = state.gpr[Register::Rsi as usize];
    if rsi != plan.boot_params.start {
        return Err(LinuxLoadError::EntryRsi {
            expected: plan.boot_params.start,
            found: rsi,
        });
    }
    for (register, &value) in state.gpr.iter().enumerate() {
        if register != Register::Rsi as usize && register != Register::Rsp as usize && value != 0 {
            return Err(LinuxLoadError::EntryGpr { register, value });
        }
    }
    let rsp = state.gpr[Register::Rsp as usize];
    if rsp != plan.stack.end || !rsp.is_multiple_of(16) {
        return Err(LinuxLoadError::EntryRsp {
            expected: plan.stack.end,
            found: rsp,
        });
    }
    if state.cs_selector != Some(LINUX_BOOT_CS) {
        return Err(LinuxLoadError::EntryCs {
            found: state.cs_selector,
        });
    }
    let reference = linux_entry_state(plan);
    if state.cs_cache != reference.cs_cache {
        return Err(LinuxLoadError::EntryCsCache);
    }
    for (register, found, cache_matches) in [
        (
            "SS",
            state.ss_selector,
            state.ss_cache == reference.ss_cache,
        ),
        (
            "DS",
            Some(state.ds_selector),
            state.ds_cache == reference.ds_cache,
        ),
        (
            "ES",
            Some(state.es_selector),
            state.es_cache == reference.es_cache,
        ),
    ] {
        if found != Some(LINUX_BOOT_DS) || !cache_matches {
            return Err(LinuxLoadError::EntryDataSegment { register, found });
        }
    }
    if state.rflags & RFLAGS_IF != 0 {
        return Err(LinuxLoadError::EntryInterruptsEnabled {
            rflags: state.rflags,
        });
    }
    if state.rflags != RFLAGS_ENTRY {
        return Err(LinuxLoadError::EntryRflags {
            found: state.rflags,
        });
    }
    if state.cr0 != Some(LINUX_ENTRY_CR0) {
        return Err(LinuxLoadError::EntryCr0 { found: state.cr0 });
    }
    if let Some(cr4) = state.cr4
        && cr4 & LINUX_CR4_LA57 != 0
    {
        return Err(LinuxLoadError::EntryCr4La57 { cr4 });
    }
    if state.cr4 != Some(LINUX_ENTRY_CR4) {
        return Err(LinuxLoadError::EntryCr4 { found: state.cr4 });
    }
    // EFER is not a CPU-state field: `LoadedFirmwareBoard::efer()` is `FIRMWARE_EFER` (LME|LMA, 0xD00)
    // with the NXE bit of the boot vCPU's guest EFER (S2-25f). The entry state is the loader's, so
    // NXE is set and the walker EFER is 0xD00; this check guards the entry handoff against a
    // board whose guest EFER was changed before the load.
    if efer != FIRMWARE_EFER {
        return Err(LinuxLoadError::EntryEfer { found: efer });
    }
    let root = plan.page_tables.start;
    if state.cr3.is_some() || board_cr3 != root {
        return Err(LinuxLoadError::EntryCr3 {
            expected: root,
            state: state.cr3,
            board: board_cr3,
        });
    }
    if state.sti_shadow != Some(StiShadow::Clear) || state.activity != Some(CpuActivity::Running) {
        return Err(LinuxLoadError::EntryActivity);
    }
    let gdtr = Gdtr {
        base: plan.gdt.start,
        limit: LINUX_GDT_LIMIT,
    };
    if state.gdtr != Some(gdtr) {
        return Err(LinuxLoadError::EntryGdtr { found: state.gdtr });
    }

    let mut gdt = [0u8; LINUX_GDT_BYTES_WRITTEN];
    read_bytes(board, plan.gdt.start, &mut gdt)?;
    if gdt.as_slice() != gdt_bytes().as_slice() {
        return Err(LinuxLoadError::GdtMismatch);
    }

    for (zone, range) in plan.zones() {
        for address in pages_of(range) {
            check_identity(board, root, plan.page_tables, zone, address)?;
        }
    }
    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);
    match walk(board, root, plan.page_tables, low_end)? {
        Walk::NotPresent { .. } => {}
        Walk::OutsideZone { level, target } => {
            return Err(LinuxLoadError::PageTablePointerOutsideZone { level, target });
        }
        _ => return Err(LinuxLoadError::HighWindowMapped { address: low_end }),
    }

    // The structural rules hold; the bytes must also be exactly what the builder produces
    // (this catches, for example, a stray accessed bit or a leaf of an unused page).
    let expected = build_linux_page_tables(root, low_end)?;
    let mut found = vec![0u8; expected.len()];
    read_bytes(board, root, &mut found)?;
    if let Some(first_diff) = expected.iter().zip(&found).position(|(a, b)| a != b) {
        return Err(LinuxLoadError::PageTablesMismatch { first_diff });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PagedProbeExit;
    use crate::linux_boot_params::LINUX_BOOT_PARAMS_LEN;
    use crate::linux_placement::{
        LINUX_BOOT_PARAMS_BASE, LINUX_CMDLINE_BASE, LINUX_GDT_BASE, LINUX_PAGE_TABLES_BASE,
        LINUX_STACK_BASE,
    };
    use crate::paged_probe::run_paged_probe_2m_ram;
    use od_isa::INTEGER_FLAGS;
    use od_isa::memory::{AccessContext, AccessOrigin, InstructionMode, PrivilegeLevel};

    const MIB: u64 = 1 << 20;
    const PREF: u64 = 0x0100_0000;
    const PM_OFFSET: usize = 3 * 512;
    const PM_CODE: usize = 0x3000;

    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Synthetic bzImage (same shape as the L2/L3 tests: pref 16 MiB, 2 MiB alignment,
    /// `init_size` 1 MiB, `cmdline_size` 0x7FF). The protected-mode code is `PM_CODE` bytes of a
    /// recognizable pattern with `code` at offset 0x200 (`startup_64`).
    fn synthetic_bzimage(code: &[u8]) -> Vec<u8> {
        let payload_offset = 0x1000u32;
        let payload_length = 0x2000u32;
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
        put32(&mut b, 0x248, payload_offset);
        put32(&mut b, 0x24C, payload_length);
        put64(&mut b, 0x258, PREF);
        put32(&mut b, 0x260, 0x0010_0000);
        let at = PM_OFFSET + LINUX_ENTRY_OFFSET as usize;
        b[at..at + code.len()].copy_from_slice(code);
        b
    }

    /// `MOV AL, imm8; MOV DX, 0x03F8; OUT DX, AL; HLT` (the allowlist of the base paged profile,
    /// FIRMWARE_BOOT.md section on executable instructions).
    fn com1_program(byte: u8) -> Vec<u8> {
        vec![0xb0, byte, 0x66, 0xba, 0xf8, 0x03, 0xee, 0xf4]
    }

    fn config(ram_bytes: u64) -> FirmwareBoardConfig {
        FirmwareBoardConfig {
            ram_bytes,
            acpi_pm: true,
            acpi_tables: true,
            pci_ecam: true,
            linux_profile: true,
            ..FirmwareBoardConfig::default()
        }
    }

    fn acpi() -> (ValidatedAcpiBlob, Vec<u8>) {
        let mut blob = vec![0u8; 0x460 + 0x40];
        for (i, byte) in blob.iter_mut().enumerate() {
            *byte = (i % 241) as u8;
        }
        blob[0x60..0x68].copy_from_slice(b"RSD PTR ");
        let acpi = ValidatedAcpiBlob {
            version: 2,
            blob_sha256: sha256(&blob),
            manifest_hash: [0; 32],
            load_base: 0xE_0000,
            payload_offset: 0x60,
            payload_len: 0x400,
            tables: Vec::new(),
            rsdp_address: 0xE_0000,
            facs_offset: 0x460,
            facs_address: 0xE_F000,
        };
        (acpi, blob)
    }

    fn load_with(
        file: &[u8],
        ram: u64,
        initrd: Option<&[u8]>,
        cmdline: &[u8],
    ) -> Result<LoadedLinuxBoard, LinuxLoadError> {
        let (acpi, blob) = acpi();
        load_linux_direct(&LinuxLoadInput {
            config: &config(ram),
            bzimage: file,
            initrd,
            cmdline,
            acpi: &acpi,
            acpi_blob: &blob,
        })
    }

    fn load_smoke() -> LoadedLinuxBoard {
        load_with(
            &synthetic_bzimage(&com1_program(b'X')),
            128 * MIB,
            None,
            b"console=ttyS0 nokaslr",
        )
        .expect("load")
    }

    fn read(board: &mut LoadedFirmwareBoard, address: u64, len: usize) -> Vec<u8> {
        let mut out = vec![0u8; len];
        read_bytes(board, address, &mut out).expect("read");
        out
    }

    fn write_u64(board: &mut LoadedFirmwareBoard, address: u64, value: u64) {
        board
            .bus_mut()
            .write_flat_bytes(
                GuestPhysicalAddress::new(address),
                &value.to_le_bytes(),
                |_| {},
            )
            .expect("write");
    }

    fn validate(loaded: &mut LoadedLinuxBoard) -> Result<(), LinuxLoadError> {
        loaded.validate_entry()
    }

    fn context() -> AccessContext {
        AccessContext {
            mode: InstructionMode::Long64,
            privilege: PrivilegeLevel::Ring0,
            origin: AccessOrigin::Explicit,
        }
    }

    /// Runs the loaded board for at most `budget` instructions through the base CPU profile
    /// with the 2 MiB walk and the board's CR3, from `state`.
    fn run_2m(
        board: &mut LoadedFirmwareBoard,
        state: &mut LongModeState,
        budget: u64,
    ) -> crate::PagedProbeQuantumReport {
        let (cr3, efer) = (board.cr3(), board.efer());
        run_paged_probe_2m_ram(
            state,
            board.bus_mut(),
            cr3,
            efer,
            context(),
            budget,
            INTEGER_FLAGS,
            || false,
            |_| {},
            None,
        )
    }

    /// Moves the byte of the UART holding register out, like `run_boot` does.
    fn drain_com1(board: &LoadedFirmwareBoard) -> Vec<u8> {
        let uart = board.uart();
        let mut com1 = Vec::new();
        while uart.transfer_holding_to_shift() {
            if let Some(byte) = uart.complete_transmission() {
                com1.push(byte);
            }
        }
        com1
    }

    /// Loads the synthetic kernel (the `MOV/OUT/HLT` program at `+0x200`), starts at
    /// `RIP + rip_offset` and runs it to the end; returns COM1, the final state and the retired
    /// count.
    fn smoke(byte: u8, rip_offset: u64) -> (Vec<u8>, LongModeState, u64) {
        let mut loaded = load_with(
            &synthetic_bzimage(&com1_program(byte)),
            128 * MIB,
            None,
            b"",
        )
        .expect("load");
        let board = &mut loaded.board;
        let mut state = board.state().clone();
        state.rip += rip_offset;
        let report = run_2m(board, &mut state, 16);
        assert_eq!(report.exit, PagedProbeExit::Halted, "{report:?}");
        (drain_com1(board), state, report.instructions_executed)
    }

    #[test]
    fn smoke_runs_the_synthetic_kernel_through_the_board_cr3_and_2m_leaves() {
        let (com1, state, retired) = smoke(b'X', 0);
        assert_eq!(com1, b"X");
        assert_eq!(retired, 4);
        // `HLT` retires and leaves RIP after itself; RSI is untouched.
        assert_eq!(state.rip, PREF + 0x200 + 8);
        assert_eq!(state.gpr[Register::Rsi as usize], 0x10_0000);
        assert_eq!(state.cr3, None);
        assert_eq!(state.activity, Some(CpuActivity::Halted));
        // A different byte gives a different COM1 byte: the smoke is not vacuous.
        assert_eq!(smoke(0xA5, 0).0, [0xA5]);
    }

    #[test]
    fn e820_agrees_with_the_handoff_of_the_real_board_for_every_ram_size() {
        // `load_linux_direct` runs the check against the handoff of the assembled board, so a
        // successful load is the comparison. Sizes: the minimum, a mid size, one above the
        // 256 MiB the handoff tables were first tested with, 4 GiB and 8 GiB (RAM above 4 GiB).
        // The maximum (0xF_F400_0000) is not tried: the firmware boot tables of the handoff use
        // 4 KiB leaves and take minutes and gigabytes at that size (a limit of the board
        // assembly, not of this check).
        for ram in [128 * MIB, 256 * MIB, 512 * MIB, 4096 * MIB, 8192 * MIB] {
            let loaded = load_with(
                &synthetic_bzimage(&com1_program(b'X')),
                ram,
                None,
                b"console=ttyS0",
            );
            assert!(loaded.is_ok(), "ram {ram:#x}: {:?}", loaded.err());
        }
    }

    #[test]
    fn a_disagreeing_e820_is_refused_by_the_handoff_check() {
        let loaded = load_smoke();
        let e820 = build_linux_e820(&loaded.plan, 128 * MIB).expect("e820");
        let assembled =
            assemble_board(&config(128 * MIB), PciEcam::new(), None, None).expect("board");
        let handoff = assembled.handoff_block().to_vec();
        assert_eq!(check_e820_against_handoff_block(&e820, &handoff), Ok(()));

        let handoff_error =
            |entries: &[LinuxE820Entry], block: &[u8]| match check_e820_against_handoff_block(
                entries, block,
            ) {
                Err(LinuxLoadError::Handoff(error)) => error,
                other => panic!("expected a handoff error, got {other:?}"),
            };
        // No ECAM entry: the window of the handoff is not reserved in E820.
        let no_ecam: Vec<_> = e820
            .iter()
            .copied()
            .filter(|e| e.addr != 0xFC10_0000)
            .collect();
        assert!(matches!(
            handoff_error(&no_ecam, &handoff),
            LinuxBootParamsError::HandoffWindowNotReserved { kind: 3, .. }
        ));
        // No ROM entry, no framebuffer entry.
        for (addr, kind) in [(0xFFC0_0000, 4), (0xF800_0000, 6)] {
            let without: Vec<_> = e820.iter().copied().filter(|e| e.addr != addr).collect();
            assert!(matches!(
                handoff_error(&without, &handoff),
                LinuxBootParamsError::HandoffWindowNotReserved { kind: k, .. } if k == kind
            ));
        }
        // The ACPI entry shifted by a page.
        let mut shifted = e820.clone();
        shifted.iter_mut().find(|e| e.kind == 3).unwrap().size -= 0x1000;
        assert!(matches!(
            handoff_error(&shifted, &handoff),
            LinuxBootParamsError::HandoffAcpiMismatch { .. }
        ));
        // A corrupted handoff block.
        let mut bad_magic = handoff.clone();
        bad_magic[0] ^= 0xFF;
        assert_eq!(
            handoff_error(&e820, &bad_magic),
            LinuxBootParamsError::HandoffMalformed
        );
        assert_eq!(
            handoff_error(&e820, &handoff[..0x20]),
            LinuxBootParamsError::HandoffMalformed
        );
    }

    #[test]
    fn cr3_owned_quantum_cannot_run_the_program() {
        // The CPU-owned CR3 quantum admits only MOV CR3 and MOV RAX,[RBX]: it fetches the first
        // byte of the program through the 2 MiB leaf (so the entry and the tables work) and
        // refuses `MOV AL, imm8`. It also rejects a state without a CPU-owned CR3. Debt for
        // S4-38 (STAGE_4.md open item 5).
        let mut loaded = load_smoke();
        let board = &mut loaded.board;
        let mut state = board.state().clone();
        state.cr3 = Some(board.cr3());
        let efer = board.efer();
        let report = crate::run_paged_cr3_probe_2m_ram(
            &mut state,
            board.bus_mut(),
            efer,
            context(),
            16,
            INTEGER_FLAGS,
            || false,
            |_| {},
        );
        assert_eq!(
            report.exit,
            PagedProbeExit::Unsupported { opcode: 0xb0 },
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 0);
        assert_eq!(state.rip, PREF + 0x200);
    }

    #[test]
    fn running_from_the_start_of_the_image_is_not_the_entry() {
        // RIP = load runs the filler pattern, not the program: it never reaches COM1.
        let mut loaded = load_smoke();
        let board = &mut loaded.board;
        let mut state = board.state().clone();
        state.rip = PREF;
        let report = run_2m(board, &mut state, 16);
        assert_ne!(report.exit, PagedProbeExit::Halted, "{report:?}");
        assert!(drain_com1(board).is_empty());
    }

    #[test]
    fn every_piece_is_written_where_the_plan_says() {
        let file = synthetic_bzimage(&com1_program(b'X'));
        let initrd: Vec<u8> = (0..0x2345u32).map(|i| (i % 253) as u8).collect();
        let cmdline = b"console=ttyS0 nokaslr";
        let mut loaded =
            load_with(&file, 128 * MIB, Some(&initrd), cmdline).expect("load with initrd");
        let plan = loaded.plan;
        let (acpi, blob) = acpi();
        let board = &mut loaded.board;

        assert_eq!(plan.boot_params.start, LINUX_BOOT_PARAMS_BASE);
        assert_eq!(plan.cmdline.start, LINUX_CMDLINE_BASE);
        assert_eq!(plan.gdt.start, LINUX_GDT_BASE);
        assert_eq!(plan.page_tables.start, LINUX_PAGE_TABLES_BASE);
        assert_eq!(plan.stack.start, LINUX_STACK_BASE);

        // Kernel: exactly the protected-mode code, the rest of the window is zero.
        assert_eq!(
            read(board, PREF, PM_CODE),
            file[PM_OFFSET..PM_OFFSET + PM_CODE]
        );
        assert_eq!(read(board, PREF + PM_CODE as u64, 0x100), vec![0u8; 0x100]);
        // Initrd.
        let initrd_at = plan.initrd.expect("initrd").start;
        assert_eq!(read(board, initrd_at, initrd.len()), initrd);
        // Command line with its NUL.
        let mut expected = cmdline.to_vec();
        expected.push(0);
        assert_eq!(read(board, LINUX_CMDLINE_BASE, expected.len()), expected);
        // boot_params: the serializer output of the same plan.
        let page = read(board, LINUX_BOOT_PARAMS_BASE, LINUX_BOOT_PARAMS_LEN);
        assert_eq!(
            u32::from_le_bytes(page[0x228..0x22C].try_into().unwrap()) as u64,
            LINUX_CMDLINE_BASE
        );
        assert_eq!(
            u32::from_le_bytes(page[0x218..0x21C].try_into().unwrap()) as u64,
            initrd_at
        );
        assert_eq!(
            u32::from_le_bytes(page[0x21C..0x220].try_into().unwrap()) as usize,
            initrd.len()
        );
        assert_eq!(page[0x210], 0xFF);
        assert_eq!(
            u64::from_le_bytes(page[0x70..0x78].try_into().unwrap()),
            0xE_0000
        );
        // ACPI blob and FACS.
        assert_eq!(read(board, 0xE_0000, 0x400), blob[0x60..0x460]);
        assert_eq!(read(board, acpi.facs_address, 0x40), blob[0x460..0x4A0]);
        // GDT.
        assert_eq!(read(board, LINUX_GDT_BASE, 32), gdt_bytes());
        // Page tables: PML4 -> PDPT -> PD, first leaf and the last leaf of low RAM.
        assert_eq!(
            read(board, LINUX_PAGE_TABLES_BASE, 8),
            ((LINUX_PAGE_TABLES_BASE + 0x1000) | 3).to_le_bytes()
        );
        assert_eq!(
            read(board, LINUX_PAGE_TABLES_BASE + 0x2000, 8),
            0x83u64.to_le_bytes()
        );
        assert_eq!(
            read(board, LINUX_PAGE_TABLES_BASE + 0x2000 + 63 * 8, 8),
            (0x7E0_0000u64 | 0x83).to_le_bytes()
        );
        assert_eq!(
            read(board, LINUX_PAGE_TABLES_BASE + 0x2000 + 64 * 8, 8),
            0u64.to_le_bytes()
        );
        assert_eq!(board.cr3(), LINUX_PAGE_TABLES_BASE);
        assert_eq!(board.boot_page_table_bytes(), 3 * 0x1000);
        assert!(board.handoff_block().is_empty());
        validate(&mut loaded).expect("valid");
    }

    #[test]
    fn entry_state_matches_the_contract() {
        let loaded = load_smoke();
        let state = loaded.board.state();
        assert_eq!(state.rip, PREF + 0x200);
        assert_eq!(state.gpr[Register::Rsi as usize], 0x10_0000);
        assert_eq!(state.gpr[Register::Rsp as usize], 0x15_0000);
        for (i, v) in state.gpr.iter().enumerate() {
            if i != Register::Rsi as usize && i != Register::Rsp as usize {
                assert_eq!(*v, 0, "gpr {i}");
            }
        }
        assert_eq!(state.cs_selector, Some(0x10));
        assert_eq!(state.ss_selector, Some(0x18));
        assert_eq!((state.ds_selector, state.es_selector), (0x18, 0x18));
        assert_eq!(state.rflags, 2);
        assert_eq!(state.cr0, Some(0x8001_0033));
        assert_eq!(state.cr3, None);
        assert_eq!(loaded.board.cr3(), 0x13_0000);
        assert_eq!(state.cr4, Some(0x20));
        assert_eq!(loaded.board.efer(), 0xd00);
        assert_eq!(
            state.gdtr,
            Some(Gdtr {
                base: 0x12_0000,
                limit: 31
            })
        );
    }

    #[test]
    fn page_table_builder_sizes() {
        // 128 MiB: PML4 + PDPT + one PD; 4 GiB and the maximum: low RAM is 0xF400_0000, 4 PDs.
        assert_eq!(
            build_linux_page_tables(0x13_0000, 128 * MIB).unwrap().len(),
            3 * 0x1000
        );
        assert_eq!(
            build_linux_page_tables(0x13_0000, 1 << 30).unwrap().len(),
            3 * 0x1000
        );
        let max = build_linux_page_tables(0x13_0000, FIRMWARE_LOW_RAM_LIMIT).unwrap();
        assert_eq!(max.len(), 6 * 0x1000);
        assert!(max.len() as u64 <= 0x1_0000);
        // The last leaf is 0xF3E0_0000 (PD 3, index 415) and the one after it is empty.
        let entry =
            |index: usize| u64::from_le_bytes(max[index * 8..index * 8 + 8].try_into().unwrap());
        let last_pd = 5 * 512;
        assert_eq!(entry(last_pd + 415), 0xF3E0_0000 | 0x83);
        assert_eq!(entry(last_pd + 416), 0);
        assert_eq!(
            build_linux_page_tables(0x13_0000, MIB),
            Err(LinuxLoadError::LowRamNotAligned { low_end: MIB })
        );
    }

    #[test]
    fn a_fresh_load_passes_for_a_large_board() {
        // 4 GiB: low RAM is capped at 0xF400_0000, the rest is above 4 GiB and unmapped.
        let mut loaded = load_with(
            &synthetic_bzimage(&com1_program(b'X')),
            4 * 1024 * MIB,
            None,
            b"",
        )
        .expect("load 4 GiB");
        validate(&mut loaded).expect("valid");
        assert_eq!(loaded.board.boot_page_table_bytes(), 6 * 0x1000);
    }

    // ---- injections: each one has its own, exact error ----

    #[test]
    fn injection_rip_at_the_load_address() {
        let mut loaded = load_smoke();
        loaded.board.state_mut().rip = PREF;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryRip {
                expected: PREF + 0x200,
                found: PREF
            })
        );
    }

    #[test]
    fn injection_wrong_cs() {
        for cs in [0x08u16, 0x18, 0] {
            let mut loaded = load_smoke();
            loaded.board.state_mut().cs_selector = Some(cs);
            assert_eq!(
                validate(&mut loaded),
                Err(LinuxLoadError::EntryCs { found: Some(cs) }),
                "cs {cs:#x}"
            );
        }
        let mut loaded = load_smoke();
        loaded.board.state_mut().cs_selector = None;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryCs { found: None })
        );
    }

    #[test]
    fn injection_interrupt_flag() {
        let mut loaded = load_smoke();
        loaded.board.state_mut().rflags |= 1 << 9;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryInterruptsEnabled { rflags: 0x202 })
        );
        // A different RFLAGS error is a different variant.
        let mut loaded = load_smoke();
        loaded.board.state_mut().rflags = 0x2 | (1 << 10);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryRflags { found: 0x402 })
        );
    }

    #[test]
    fn injection_cr4_la57() {
        let mut loaded = load_smoke();
        loaded.board.state_mut().cr4 = Some(0x20 | (1 << 12));
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryCr4La57 { cr4: 0x1020 })
        );
        // The interpreter refuses the same state, too (its CR4 mask has no LA57).
        let mut state = loaded.board.state().clone();
        let report = run_2m(&mut loaded.board, &mut state, 16);
        assert!(
            matches!(report.exit, PagedProbeExit::Rejected(_)),
            "{report:?}"
        );
        assert_eq!(report.instructions_executed, 0);
        // Without PAE it is a plain CR4 mismatch, not an LA57 one.
        let mut loaded = load_smoke();
        loaded.board.state_mut().cr4 = Some(0);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryCr4 { found: Some(0) })
        );
    }

    #[test]
    fn injection_page_tables_without_the_kernel_range() {
        // The PD entry of the first kernel page (16 MiB = PD index 8) is gone.
        let mut loaded = load_smoke();
        write_u64(
            &mut loaded.board,
            LINUX_PAGE_TABLES_BASE + 0x2000 + 8 * 8,
            0,
        );
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageNotMapped {
                zone: LinuxZone::Kernel,
                address: PREF,
                level: 2
            })
        );
        // The guest itself faults on the missing kernel page: no COM1 byte.
        let mut state = loaded.board.state().clone();
        let report = run_2m(&mut loaded.board, &mut state, 16);
        let PagedProbeExit::Fault(fault) = &report.exit else {
            panic!("{report:?}")
        };
        assert_eq!(fault.vector, 14);
        assert_eq!(report.instructions_executed, 0);
        assert!(drain_com1(&loaded.board).is_empty());
        // The whole PDPT entry is gone: the loader zones are not mapped either (level 3).
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, LINUX_PAGE_TABLES_BASE + 0x1000, 0);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageNotMapped {
                zone: LinuxZone::Kernel,
                address: PREF,
                level: 3
            })
        );
    }

    #[test]
    fn injection_page_table_entries_with_wrong_content() {
        let pd = LINUX_PAGE_TABLES_BASE + 0x2000;
        // NX on the kernel leaf.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 8 * 8, PREF | 0x83 | (1 << 63));
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageBadEntry {
                zone: LinuxZone::Kernel,
                address: PREF,
                level: 2,
                entry: PREF | 0x83 | (1 << 63)
            })
        );
        // Not writable.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 8 * 8, PREF | 0x81);
        assert!(matches!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageBadEntry { level: 2, .. })
        ));
        // Maps another physical page.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 8 * 8, 0x120_0000 | 0x83);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageNotIdentity {
                zone: LinuxZone::Kernel,
                address: PREF,
                mapped: 0x120_0000
            })
        );
        // A 4 KiB table where a 2 MiB leaf is expected.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 8 * 8, 0x13_0000 | 0x3);
        assert!(matches!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageBadEntry { level: 2, .. })
        ));
        // MMIO / the window above low RAM is mapped (128 MiB: PD index 64).
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 64 * 8, 0x800_0000 | 0x83);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::HighWindowMapped {
                address: 0x800_0000
            })
        );
    }

    #[test]
    fn injection_other_registers_and_memory() {
        let mut loaded = load_smoke();
        loaded.board.state_mut().gpr[Register::Rsi as usize] = 0;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryRsi {
                expected: 0x10_0000,
                found: 0
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().gpr[Register::Rdi as usize] = 1;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryGpr {
                register: Register::Rdi as usize,
                value: 1
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().gpr[Register::Rsp as usize] = 0x14_0008;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryRsp {
                expected: 0x15_0000,
                found: 0x14_0008
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().ss_selector = Some(0x10);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryDataSegment {
                register: "SS",
                found: Some(0x10)
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().ds_selector = 0;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryDataSegment {
                register: "DS",
                found: Some(0)
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().cr0 = Some(0x8001_0033 & !(1 << 31));
        assert!(matches!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryCr0 { .. })
        ));
        let mut loaded = load_smoke();
        loaded.board.state_mut().cr3 = Some(0x60_0000);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryCr3 {
                expected: 0x13_0000,
                state: Some(0x60_0000),
                board: 0x13_0000
            })
        );
        let mut loaded = load_smoke();
        loaded.board.state_mut().activity = Some(CpuActivity::Halted);
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::EntryActivity));
        let mut loaded = load_smoke();
        loaded.board.state_mut().gdtr = None;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryGdtr { found: None })
        );
        // A corrupted `__BOOT_CS` descriptor in memory.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, LINUX_GDT_BASE + 0x10, 0);
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::GdtMismatch));
    }

    #[test]
    fn injection_segment_caches_and_sti_shadow() {
        // The hidden code descriptor differs (access byte of a non-accessed 64-bit code segment).
        let mut loaded = load_smoke();
        let mut cache = loaded.board.state().cs_cache.expect("cs cache");
        cache.access = 0x9a;
        loaded.board.state_mut().cs_cache = Some(cache);
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::EntryCsCache));
        // A missing hidden descriptor is the same error.
        let mut loaded = load_smoke();
        loaded.board.state_mut().cs_cache = None;
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::EntryCsCache));
        // Hidden data descriptors: the selector is right, the cache is not.
        for register in ["SS", "DS", "ES"] {
            let mut loaded = load_smoke();
            let state = loaded.board.state_mut();
            let mut cache = state.ds_cache.expect("data cache");
            cache.limit = 0xffff;
            match register {
                "SS" => state.ss_cache = Some(cache),
                "DS" => state.ds_cache = Some(cache),
                _ => state.es_cache = Some(cache),
            }
            assert_eq!(
                validate(&mut loaded),
                Err(LinuxLoadError::EntryDataSegment {
                    register,
                    found: Some(0x18)
                }),
                "{register} cache"
            );
        }
        // ES selector.
        let mut loaded = load_smoke();
        loaded.board.state_mut().es_selector = 0x10;
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryDataSegment {
                register: "ES",
                found: Some(0x10)
            })
        );
        // STI shadow (the activity part of `EntryActivity` has its own test above).
        let mut loaded = load_smoke();
        loaded.board.state_mut().sti_shadow = Some(StiShadow::Blocking);
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::EntryActivity));
        let mut loaded = load_smoke();
        loaded.board.state_mut().sti_shadow = None;
        assert_eq!(validate(&mut loaded), Err(LinuxLoadError::EntryActivity));
    }

    #[test]
    fn injection_gdtr_base_and_limit() {
        let wrong_base = Gdtr {
            base: LINUX_GDT_BASE + 8,
            limit: LINUX_GDT_LIMIT,
        };
        let mut loaded = load_smoke();
        loaded.board.state_mut().gdtr = Some(wrong_base);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryGdtr {
                found: Some(wrong_base)
            })
        );
        let wrong_limit = Gdtr {
            base: LINUX_GDT_BASE,
            limit: LINUX_GDT_LIMIT - 8,
        };
        let mut loaded = load_smoke();
        loaded.board.state_mut().gdtr = Some(wrong_limit);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::EntryGdtr {
                found: Some(wrong_limit)
            })
        );
    }

    #[test]
    fn injection_page_table_pointers_outside_the_zone() {
        // PML4[0] -> a plausible, page-aligned address just above the zone (stack zone).
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, LINUX_PAGE_TABLES_BASE, 0x14_1000 | 3);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablePointerOutsideZone {
                level: 4,
                target: 0x14_1000
            })
        );
        // The first byte past the zone end is already outside.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, LINUX_PAGE_TABLES_BASE, 0x14_0000 | 3);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablePointerOutsideZone {
                level: 4,
                target: 0x14_0000
            })
        );
        // PDPT[0] -> the LAPIC/IOAPIC MMIO window.
        let mut loaded = load_smoke();
        write_u64(
            &mut loaded.board,
            LINUX_PAGE_TABLES_BASE + 0x1000,
            0xFEC0_0000 | 3,
        );
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablePointerOutsideZone {
                level: 3,
                target: 0xFEC0_0000
            })
        );
        // Below the zone start (loader GDT page).
        let mut loaded = load_smoke();
        write_u64(
            &mut loaded.board,
            LINUX_PAGE_TABLES_BASE + 0x1000,
            LINUX_GDT_BASE | 3,
        );
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablePointerOutsideZone {
                level: 3,
                target: LINUX_GDT_BASE
            })
        );
    }

    #[test]
    fn injection_page_table_bytes_differ_from_the_builder() {
        let pd = LINUX_PAGE_TABLES_BASE + 0x2000;
        // An accessed bit on the leaf of an unused page (6 MiB, PD index 3): every walk of the
        // plan zones still succeeds, only the byte comparison sees it.
        let mut loaded = load_smoke();
        write_u64(&mut loaded.board, pd + 3 * 8, 0x60_0000 | 0x83 | 0x20);
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablesMismatch {
                first_diff: 0x2000 + 3 * 8
            })
        );
        // A stray PML4 entry (slot 1).
        let mut loaded = load_smoke();
        write_u64(
            &mut loaded.board,
            LINUX_PAGE_TABLES_BASE + 8,
            (LINUX_PAGE_TABLES_BASE + 0x1000) | 3,
        );
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablesMismatch { first_diff: 8 })
        );
        // An accessed bit on PML4[0] keeps the walk valid too.
        let mut loaded = load_smoke();
        write_u64(
            &mut loaded.board,
            LINUX_PAGE_TABLES_BASE,
            (LINUX_PAGE_TABLES_BASE + 0x1000) | 3 | 0x20,
        );
        assert_eq!(
            validate(&mut loaded),
            Err(LinuxLoadError::PageTablesMismatch { first_diff: 0 })
        );
    }

    #[test]
    fn page_table_builder_rejects_bad_inputs() {
        // Root: not 4 KiB aligned, or beyond the 52 physical address bits.
        assert_eq!(
            build_linux_page_tables(0x13_0800, 128 * MIB),
            Err(LinuxLoadError::PageTableRootInvalid { root: 0x13_0800 })
        );
        assert_eq!(
            build_linux_page_tables(1 << 52, 128 * MIB),
            Err(LinuxLoadError::PageTableRootInvalid { root: 1 << 52 })
        );
        // Low RAM: 512 GiB is the largest map one PDPT reaches.
        let max = 512 * GIB;
        assert_eq!(
            build_linux_page_tables(0x13_0000, max + PAGE_2M),
            Err(LinuxLoadError::LowRamTooLarge {
                low_end: max + PAGE_2M
            })
        );
        let tables = build_linux_page_tables(0x13_0000, max).expect("512 GiB");
        assert_eq!(tables.len(), 514 * 0x1000);
        assert_eq!(
            build_linux_page_tables(0x13_0000, 0),
            Err(LinuxLoadError::LowRamNotAligned { low_end: 0 })
        );
    }

    #[test]
    fn the_entry_must_be_inside_the_kernel_code() {
        // Injection into a loaded board: the recorded code size no longer covers `+0x200`.
        for size in [0x200u64, 0x100, 0] {
            let mut loaded = load_smoke();
            loaded.header.pm_code_size = size;
            assert_eq!(
                validate(&mut loaded),
                Err(LinuxLoadError::EntryOutsideKernel {
                    entry_offset: 0x200,
                    pm_code_size: size
                }),
                "size {size:#x}"
            );
        }
        // One byte of code at the entry is enough.
        let mut loaded = load_smoke();
        loaded.header.pm_code_size = 0x201;
        validate(&mut loaded).expect("entry inside the code");
        // A file whose protected-mode code ends at the entry is refused before the board exists.
        let mut file = synthetic_bzimage(&com1_program(b'X'));
        put32(&mut file, 0x1F4, 0x20);
        put32(&mut file, 0x248, 0);
        put32(&mut file, 0x24C, 0x100);
        assert_eq!(
            load_with(&file, 128 * MIB, None, b"").err(),
            Some(LinuxLoadError::EntryOutsideKernel {
                entry_offset: 0x200,
                pm_code_size: 0x200
            })
        );
    }

    // ---- input errors, reported before the board exists ----

    #[test]
    fn load_input_errors() {
        let file = synthetic_bzimage(&com1_program(b'X'));
        let (acpi, blob) = acpi();
        let mut input = LinuxLoadInput {
            config: &config(128 * MIB),
            bzimage: &file,
            initrd: None,
            cmdline: b"",
            acpi: &acpi,
            acpi_blob: &blob,
        };
        // No Linux profile.
        let plain = FirmwareBoardConfig {
            ram_bytes: 128 * MIB,
            ..FirmwareBoardConfig::default()
        };
        input.config = &plain;
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::NotLinuxProfile)
        ));
        let profile_only = FirmwareBoardConfig {
            linux_profile: true,
            ..FirmwareBoardConfig::default()
        };
        input.config = &profile_only;
        // Linux profile without its prerequisites: the board refuses, not the loader.
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::Board(FirmwareBoardError::Config(_)))
        ));
        let config_ok = config(128 * MIB);
        input.config = &config_ok;
        // NUL in the command line.
        input.cmdline = b"a\0b";
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::CmdlineContainsNul { at: 1 })
        ));
        // Command line longer than `cmdline_size`.
        let long = vec![b'x'; 0x800];
        input.cmdline = &long;
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::Placement(
                LinuxPlacementError::CmdlineTooLong { .. }
            ))
        ));
        input.cmdline = b"";
        // Initrd that cannot fit below the end of RAM.
        let huge = vec![0u8; 120 * MIB as usize];
        input.initrd = Some(&huge);
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::Placement(_))
        ));
        input.initrd = None;
        // Truncated ACPI blob.
        input.acpi_blob = &blob[..0x100];
        assert_eq!(
            load_linux_direct(&input).err(),
            Some(LinuxLoadError::AcpiBlobTooShort { blob_len: 0x100 })
        );
        // Bytes that are not the ones `acpi` was validated from (other length).
        let mut longer = blob.clone();
        longer.push(0);
        input.acpi_blob = &longer;
        assert_eq!(
            load_linux_direct(&input).err(),
            Some(LinuxLoadError::AcpiBlobMismatch)
        );
        // Same length, one byte changed (inside the FACS, which no other check covers).
        let mut flipped = blob.clone();
        flipped[0x460 + 1] ^= 0xff;
        input.acpi_blob = &flipped;
        assert_eq!(
            load_linux_direct(&input).err(),
            Some(LinuxLoadError::AcpiBlobMismatch)
        );
        input.acpi_blob = &blob;
        // Not a bzImage.
        let junk = vec![0u8; 0x1000];
        input.bzimage = &junk;
        assert!(matches!(
            load_linux_direct(&input),
            Err(LinuxLoadError::Header(_))
        ));
        // An empty initrd is no initrd.
        input.bzimage = &file;
        input.initrd = Some(&[]);
        assert!(
            load_linux_direct(&input)
                .expect("load")
                .plan
                .initrd
                .is_none()
        );
    }

    #[test]
    fn code_larger_than_the_window_is_refused_by_the_header_parser() {
        // L1 already rejects `init_size < pm_code_size`; the loader relies on it.
        let mut file = synthetic_bzimage(&com1_program(b'X'));
        put32(&mut file, 0x260, 0x1000);
        let got = load_with(&file, 128 * MIB, None, b"").err();
        assert!(
            matches!(
                got,
                Some(LinuxLoadError::Header(LinuxHeaderError::InitSizeTooSmall {
                    init_size: 0x1000,
                    pm_code_size: 0x3000
                }))
            ),
            "{got:?}"
        );
    }

    // ---- real vmlinuz (ignored: `work/` is not part of the repository) ----

    const REAL_VMLINUZ: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"
    );

    /// Ignored by default; must be run explicitly (`--ignored`) and fails, instead of passing
    /// vacuously, if the file is absent. The guest is not started.
    #[test]
    #[ignore = "needs work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic; run with --ignored"]
    fn real_vmlinuz_loads_at_128_mib_and_1_gib() {
        let file = std::fs::read(REAL_VMLINUZ)
            .unwrap_or_else(|e| panic!("vmlinuz absent, test NOT passed: {REAL_VMLINUZ}: {e}"));
        let initrd: Vec<u8> = (0..0x0200_0001u32).map(|i| (i % 251) as u8).collect();
        let cmdline = b"console=ttyS0 nokaslr root=/dev/ram0";
        for (ram, initrd_at) in [(128 * MIB, 0x5FF_F000u64), (1024 * MIB, 0x3DFF_F000)] {
            let mut loaded = load_with(&file, ram, Some(&initrd), cmdline)
                .unwrap_or_else(|e| panic!("ram {ram:#x}: {e:?}"));
            let header = loaded.header;
            let plan = loaded.plan;
            let pm = header.protected_mode_offset as usize;
            let len = header.pm_code_size as usize;
            // Golden values read straight from the file bytes, not through the parser:
            // setup_sects = 39 -> protected-mode code at (39 + 1) * 512 = 0x5000; syssize * 16 bytes long.
            assert_eq!(file[0x1F1], 39, "setup_sects");
            assert_eq!(pm, 0x5000);
            assert_eq!(pm, (usize::from(file[0x1F1]) + 1) * 512);
            let syssize = u32::from_le_bytes(file[0x1F4..0x1F8].try_into().unwrap());
            assert_eq!(len, syssize as usize * 16);
            assert_eq!(plan.kernel.start, 0x100_0000);
            assert_eq!(
                plan.initrd.map(|r| r.start),
                Some(initrd_at),
                "ram {ram:#x}"
            );
            let board = &mut loaded.board;
            // Kernel bytes equal the file over the whole protocol range and the tail is zero.
            let written = read(board, plan.kernel.start, len);
            assert!(written == file[pm..pm + len], "kernel bytes, ram {ram:#x}");
            assert_eq!(
                read(board, plan.kernel.start + len as u64, 0x1000),
                vec![0u8; 0x1000]
            );
            // The entry point `startup_64` is in the loaded bytes.
            assert_eq!(
                read(board, plan.kernel.start + 0x200, 16),
                file[pm + 0x200..pm + 0x210]
            );
            assert!(read(board, initrd_at, initrd.len()) == initrd, "initrd");
            let mut expected = cmdline.to_vec();
            expected.push(0);
            assert_eq!(read(board, 0x10_1000, expected.len()), expected);
            let page = read(board, 0x10_0000, LINUX_BOOT_PARAMS_LEN);
            assert_eq!(
                u32::from_le_bytes(page[0x228..0x22C].try_into().unwrap()),
                0x10_1000
            );
            assert_eq!(
                u32::from_le_bytes(page[0x218..0x21C].try_into().unwrap()) as u64,
                initrd_at
            );
            assert_eq!(page[0x210], 0xFF);
            assert_eq!(
                u64::from_le_bytes(page[0x70..0x78].try_into().unwrap()),
                0xE_0000
            );
            assert!(page[0x1E8] > 0);
            assert_eq!(&page[0x202..0x206], b"HdrS");
            validate(&mut loaded).unwrap_or_else(|e| panic!("ram {ram:#x}: {e:?}"));
            assert_eq!(loaded.board.state().rip, 0x100_0200);
        }
    }
}
