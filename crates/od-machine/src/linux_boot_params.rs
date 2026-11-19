//! E820 map and `boot_params` (zero page) serializer of a Linux direct boot (S4-37L3).
//!
//! Pure functions: nothing here reads or writes a guest and nothing panics. Inputs are the
//! placement plan of L2 ([`LinuxPlacement`]), the guest RAM size, the validated ACPI blob and the
//! raw bytes of the bzImage. Offsets follow `arch/x86/include/uapi/asm/bootparam.h`,
//! `setup_data.h`, `e820.h` and `Documentation/arch/x86/zero-page.rst` of Linux 7.0 (the
//! `work/stage4/linux-7.0.0-38.38` source tree).
//!
//! E820 layout (guest physical, `[start, end)`; low RAM end `L = min(R, 0xF400_0000)`):
//!
//! ```text
//! [0x00_0000, 0x09_F000)  RAM       Linux real-mode trampoline lives here; the loader uses none
//! [0x09_F000, 0x0E_0000)  reserved  board RAM; reserved by PC convention, Linux reserves
//!                                   [0x9F000, 1M) itself (ebda.c)
//! [0x0E_0000, 0x0E_F000)  ACPI      handoff region 11 (tables; the RSDP is at its first byte)
//! [0x0E_F000, 0x0F_0000)  NVS       handoff region 12 (FACS page)
//! [0x0F_0000, 0x15_0000)  reserved  BIOS ROM area and the whole loader area of L2
//! [0x15_0000, L)          RAM       kernel window and initrd lie inside (Linux requires an
//!                                   initrd to be RAM and reserves the kernel itself)
//! [0xF800_0000, 0xFA40_0000) reserved  display framebuffer (board map; S4-37L6)
//! [0xFC10_0000, +1 MiB)   reserved  PCIe ECAM window; Linux only trusts MCFG when it is reserved
//! [0xFFC0_0000, 4 GiB)    reserved  firmware ROM window (board map; S4-37L6)
//! [4 GiB, 4 GiB + R - L)  RAM       only when R > L
//! ```
//!
//! The framebuffer and the ROM window are the two non-RAM windows of the board map that are
//! large enough to matter (S4-37L6): Linux must not treat them as holes it may assign to PCI
//! BARs. The small device windows (local APIC, I/O APIC, display registers, NVMe BAR) stay
//! unlisted; Linux knows the first two from MADT and none of them is RAM.
//!
//! The kernel window may be large (`init_size`), so it stays RAM instead of being wasted as
//! reserved; the loader structures in `[0x10_0000, 0x15_0000)` are reserved (a cheap 320 KiB).
//!
//! The ACPI and NVS entries are built from the board constants (`FIRMWARE_ACPI_TABLES_BASE`,
//! `FIRMWARE_ACPI_FACS_BASE`), not from the blob or the handoff; [`build_linux_e820`] and
//! [`validate_linux_e820`] reject a plan whose ACPI tables or FACS start elsewhere
//! ([`LinuxBootParamsError::AcpiBaseMismatch`]), and [`check_e820_acpi_against_handoff`] compares
//! the entries with the handoff regions 11 and 12.
//!
//! The reserved entries for `[0x9_F000, 0xE_0000)` and `[0xF_0000, 0x15_0000)` are a decision of
//! this module: the handoff declares those ranges as RAM, which is a safe divergence (reserved
//! instead of RAM).

// Used by L4-L6; tests exercise the module now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::acpi_blob::{ACPI_RECLAIM_END, ACPI_REGION_END, ValidatedAcpiBlob};
use crate::display_device::{DISPLAY_FRAMEBUFFER_BASE, DISPLAY_FRAMEBUFFER_BYTES};
use crate::firmware_board::{
    FIRMWARE_ACPI_FACS_BASE, FIRMWARE_ACPI_TABLES_BASE, FIRMWARE_HIGH_RAM_BASE,
    FIRMWARE_IMAGE_BYTES, FIRMWARE_LOW_RAM_LIMIT, FIRMWARE_ROM_BASE,
};
use crate::linux_boot::LINUX_SETUP_HEADER_MIN_BYTES;
use crate::linux_placement::{
    LINUX_BOOT_PARAMS_BYTES, LINUX_HIGH_FORBIDDEN_START, LINUX_LOADER_END, LINUX_LOW_FORBIDDEN_END,
    LinuxPlacement, LinuxRange, LinuxZone,
};
use crate::pci_ecam::PCI_ECAM_DESCRIPTOR;

/// Size of the zero page: `sizeof(struct boot_params)` (bootparam.h, `_pad9` ends at 0x1000).
pub(crate) const LINUX_BOOT_PARAMS_LEN: usize = 0x1000;
// The planner reserves exactly one page for the zero page.
const _: () = assert!(LINUX_BOOT_PARAMS_LEN as u64 == LINUX_BOOT_PARAMS_BYTES);

/// `boot_params.acpi_rsdp_addr` (u64), bootparam.h `/* 0x070 */`.
const OFF_ACPI_RSDP_ADDR: usize = 0x070;
/// `boot_params.e820_entries` (u8), bootparam.h `/* 0x1e8 */`.
const OFF_E820_ENTRIES: usize = 0x1E8;
/// `boot_params.hdr`: `struct setup_header` starts here, bootparam.h `/* 0x1f1 */`.
const OFF_SETUP_HEADER: usize = 0x1F1;
/// The header length byte (boot.rst: `0x202 + byte at 0x201` is the end of `setup_header`).
const OFF_HEADER_LENGTH: usize = 0x201;
/// The header length is counted from here (the "HdrS" magic at 0x202).
const OFF_HEADER_LENGTH_BASE: usize = 0x202;
/// Smallest accepted header end: Linux 2.14 ends at 0x268 (`handover_offset` at 0x264, 4 bytes),
/// the smallest protocol that L1 accepts (`LINUX_MIN_BOOT_PROTOCOL`).
const OFF_SETUP_HEADER_MIN_END: usize = LINUX_SETUP_HEADER_MIN_BYTES;
/// Largest accepted header end: `boot_params.edd_mbr_sig_buffer` starts at 0x290 (bootparam.h).
const OFF_SETUP_HEADER_MAX_END: usize = 0x290;
/// Length of an ACPI 2.0 RSDP (`struct acpi_table_rsdp`).
const RSDP_V2_BYTES: u64 = 36;
/// E820 ACPI and NVS entries are page aligned on the board.
const E820_ACPI_ALIGN: u64 = 0x1000;
/// `setup_header.type_of_loader` (u8), boot.rst, file offset 0x210.
const OFF_TYPE_OF_LOADER: usize = 0x210;
/// `setup_header.ramdisk_image` (u32), boot.rst, 0x218.
const OFF_RAMDISK_IMAGE: usize = 0x218;
/// `setup_header.ramdisk_size` (u32), boot.rst, 0x21C.
const OFF_RAMDISK_SIZE: usize = 0x21C;
/// `setup_header.cmd_line_ptr` (u32), boot.rst, 0x228.
const OFF_CMD_LINE_PTR: usize = 0x228;
/// `boot_params.e820_table`, bootparam.h `/* 0x2d0 */`.
const OFF_E820_TABLE: usize = 0x2D0;
/// `sizeof(struct boot_e820_entry)`: packed `addr u64, size u64, type u32` (setup_data.h).
const E820_ENTRY_BYTES: usize = 20;
/// `E820_MAX_ENTRIES_ZEROPAGE` (bootparam.h).
pub(crate) const LINUX_E820_MAX_ENTRIES: usize = 128;
/// `type_of_loader` of a loader without an assigned id (boot.rst: 0xFF = "undefined").
pub(crate) const LINUX_TYPE_OF_LOADER_UNDEFINED: u8 = 0xFF;

/// E820 types (`asm/e820/types.h`, `enum e820_type`).
pub(crate) const E820_RAM: u32 = 1;
pub(crate) const E820_RESERVED: u32 = 2;
pub(crate) const E820_ACPI: u32 = 3;
pub(crate) const E820_NVS: u32 = 4;

/// Handoff region kinds (FIRMWARE_BOOT.md section 6; `REGION_*` of `firmware_board.rs`, private).
const HANDOFF_REGION_MMIO: u32 = 3;
const HANDOFF_REGION_ROM: u32 = 4;
const HANDOFF_REGION_FRAMEBUFFER: u32 = 6;
const HANDOFF_REGION_ACPI_TABLES: u32 = 11;
const HANDOFF_REGION_ACPI_NVS: u32 = 12;
/// Handoff block header: magic, region table offset (u32 @0x30), region count (u32 @0x34), region
/// entry size (u32 @0x38).
const HANDOFF_MAGIC: &[u8; 8] = b"ODHANDOF";
const HANDOFF_OFF_REGION_TABLE: usize = 0x30;
const HANDOFF_OFF_REGION_COUNT: usize = 0x34;
/// Region entry size (u32 @0x38), 32 on the board.
const HANDOFF_OFF_REGION_ENTRY_SIZE: usize = 0x38;
/// Region entry: `kind u32 @0, attrs u32 @4, base u64 @8, length u64 @0x10, owner u32 @0x18`.
const HANDOFF_REGION_ENTRY_BYTES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinuxE820Entry {
    pub addr: u64,
    pub size: u64,
    pub kind: u32,
}

impl LinuxE820Entry {
    fn range(&self) -> Option<LinuxRange> {
        self.addr.checked_add(self.size).map(|end| LinuxRange {
            start: self.addr,
            end,
        })
    }
}

/// A region of the handoff block region table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinuxHandoffRegion {
    pub kind: u32,
    pub range: LinuxRange,
}

/// Handoff regions 11 (`ACPI_TABLES`) and 12 (`ACPI_NVS`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinuxHandoffAcpi {
    pub tables: LinuxRange,
    pub nvs: LinuxRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxBootParamsError {
    /// More entries than `E820_MAX_ENTRIES_ZEROPAGE`, or none.
    BadEntryCount { count: usize },
    /// Zero-size entry, or `addr + size` overflows.
    BadEntry { index: usize },
    /// Type outside RAM, reserved, ACPI, NVS.
    UnknownType { index: usize, kind: u32 },
    /// Entry `index` starts below the end of the previous one (unsorted or overlapping).
    NotSorted { index: usize },
    /// A piece of the fixed layout is empty or inverted (for example a hand-moved ACPI range).
    EmptyRange { kind: u32, start: u64, end: u64 },
    /// No single reserved entry covers the ECAM window.
    EcamNotReserved { ecam: LinuxRange },
    /// No single reserved entry covers this MMIO window of the board map (framebuffer or ROM).
    WindowNotReserved { window: LinuxRange },
    /// No single ACPI (tables) or NVS (FACS) entry covers the plan range.
    AcpiNotCovered { zone: LinuxZone, range: LinuxRange },
    /// The RAM entries below 4 GiB are not exactly `[0, 0x9_F000)` and `[0x15_0000, low end)`.
    LowRamMismatch {
        expected: [LinuxRange; 2],
        found_count: usize,
    },
    /// The RAM entries from 4 GiB are not exactly `[4 GiB, 4 GiB + R - low end)` (or absent).
    HighRamMismatch {
        expected: Option<LinuxRange>,
        found_count: usize,
    },
    /// Kernel or initrd is not inside one RAM entry.
    ZoneNotRam { zone: LinuxZone, range: LinuxRange },
    /// A loader structure intersects a RAM entry.
    LoaderZoneInRam { zone: LinuxZone, range: LinuxRange },
    /// The E820 ACPI (or NVS) entry does not equal handoff region 11 (or 12). `found` is the
    /// single such entry, `None` when there is none or several.
    HandoffAcpiMismatch {
        zone: LinuxZone,
        expected: LinuxRange,
        found: Option<LinuxRange>,
    },
    /// A handoff ROM, framebuffer or ECAM region is not covered by one reserved E820 entry.
    HandoffWindowNotReserved { kind: u32, range: LinuxRange },
    /// The handoff declares no ECAM MMIO region at the ECAM base.
    HandoffNoEcam,
    /// The handoff block is shorter than its header, has a bad magic or a region table outside
    /// the block.
    HandoffMalformed,
    /// The handoff has no (or several) region of this kind.
    HandoffRegionCount { kind: u32, count: usize },
    /// `range.end` overflows `u64`.
    RangeOverflow,
    /// The bzImage prefix is shorter than the end of `setup_header` (`0x202 + header length`;
    /// 0x202 when it cannot even hold the header length byte at 0x201).
    SetupHeaderTooShort { len: usize },
    /// `0x202 + byte at 0x201` is outside `[0x268, 0x290]`.
    SetupHeaderLength { end: usize },
    /// The ACPI tables or the FACS of the plan do not start at the board constant.
    AcpiBaseMismatch {
        zone: LinuxZone,
        expected: u64,
        found: u64,
    },
    /// An ACPI or NVS entry boundary is not 4 KiB aligned.
    AcpiEntryUnaligned { index: usize },
    /// The command line buffer is not entirely below 4 GiB (`cmd_line_ptr` is 32-bit).
    CmdlineAbove4G { range: LinuxRange },
    /// The initrd is not entirely below 4 GiB (`ramdisk_image`/`ramdisk_size` are 32-bit).
    InitrdAbove4G { range: LinuxRange },
    /// The 36-byte RSDP at `rsdp_addr` is not entirely inside the ACPI tables range of the plan.
    RsdpOutsideAcpiTables { rsdp: u64, tables: LinuxRange },
    /// A write would leave the 4096-byte page; never happens for the fixed offsets.
    WriteOutOfBounds { offset: usize },
}

fn ram_entry(start: u64, end: u64) -> Result<LinuxE820Entry, LinuxBootParamsError> {
    piece(E820_RAM, start, end)
}

fn piece(kind: u32, start: u64, end: u64) -> Result<LinuxE820Entry, LinuxBootParamsError> {
    match end.checked_sub(start) {
        Some(size) if size > 0 => Ok(LinuxE820Entry {
            addr: start,
            size,
            kind,
        }),
        _ => Err(LinuxBootParamsError::EmptyRange { kind, start, end }),
    }
}

/// The PCIe ECAM window of the board (`PCI_ECAM_DESCRIPTOR`: `[0xFC10_0000, +1 MiB)`).
fn ecam_range() -> Result<LinuxRange, LinuxBootParamsError> {
    let base = PCI_ECAM_DESCRIPTOR.base;
    let end = base
        .checked_add(PCI_ECAM_DESCRIPTOR.size)
        .ok_or(LinuxBootParamsError::RangeOverflow)?;
    Ok(LinuxRange { start: base, end })
}

/// The display framebuffer window of the board map.
fn framebuffer_range() -> Result<LinuxRange, LinuxBootParamsError> {
    let end = DISPLAY_FRAMEBUFFER_BASE
        .checked_add(DISPLAY_FRAMEBUFFER_BYTES)
        .ok_or(LinuxBootParamsError::RangeOverflow)?;
    Ok(LinuxRange {
        start: DISPLAY_FRAMEBUFFER_BASE,
        end,
    })
}

/// The firmware ROM window of the board map (`[0xFFC0_0000, 4 GiB)`).
fn rom_range() -> Result<LinuxRange, LinuxBootParamsError> {
    let end = FIRMWARE_ROM_BASE
        .checked_add(FIRMWARE_IMAGE_BYTES as u64)
        .ok_or(LinuxBootParamsError::RangeOverflow)?;
    Ok(LinuxRange {
        start: FIRMWARE_ROM_BASE,
        end,
    })
}

/// Builds the E820 map of the module documentation for `plan` and guest RAM `ram_size`, and
/// validates it with [`validate_linux_e820`]. The ACPI entry is `FIRMWARE_ACPI_TABLES_BASE ..
/// ACPI_RECLAIM_END` and the NVS entry `FIRMWARE_ACPI_FACS_BASE .. ACPI_REGION_END`; the plan
/// must start its ACPI tables and FACS at those bases.
pub(crate) fn build_linux_e820(
    plan: &LinuxPlacement,
    ram_size: u64,
) -> Result<Vec<LinuxE820Entry>, LinuxBootParamsError> {
    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);
    let ecam = ecam_range()?;
    let framebuffer = framebuffer_range()?;
    let rom = rom_range()?;
    let mut entries = vec![
        ram_entry(0, LINUX_LOW_FORBIDDEN_END)?,
        piece(
            E820_RESERVED,
            LINUX_LOW_FORBIDDEN_END,
            LINUX_HIGH_FORBIDDEN_START,
        )?,
        piece(E820_ACPI, FIRMWARE_ACPI_TABLES_BASE, ACPI_RECLAIM_END)?,
        piece(E820_NVS, FIRMWARE_ACPI_FACS_BASE, ACPI_REGION_END)?,
        piece(E820_RESERVED, ACPI_REGION_END, LINUX_LOADER_END)?,
        ram_entry(LINUX_LOADER_END, low_end)?,
        piece(E820_RESERVED, framebuffer.start, framebuffer.end)?,
        piece(E820_RESERVED, ecam.start, ecam.end)?,
        piece(E820_RESERVED, rom.start, rom.end)?,
    ];
    let high = ram_size - low_end;
    if high > 0 {
        let end = FIRMWARE_HIGH_RAM_BASE
            .checked_add(high)
            .ok_or(LinuxBootParamsError::RangeOverflow)?;
        entries.push(ram_entry(FIRMWARE_HIGH_RAM_BASE, end)?);
    }
    validate_linux_e820(&entries, plan, ram_size)?;
    Ok(entries)
}

fn covers(outer: LinuxRange, inner: LinuxRange) -> bool {
    outer.start <= inner.start && inner.end <= outer.end
}

/// Checks an E820 map against the plan: at most 128 well-formed, sorted, disjoint entries with
/// 4 KiB aligned ACPI and NVS boundaries; the plan's ACPI tables and FACS starting at the board
/// constants; the ECAM, framebuffer and ROM windows reserved; ACPI tables and FACS covered by
/// ACPI and NVS entries; RAM exactly
/// `[0, 0x9_F000)`, `[0x15_0000, low end)` and `[4 GiB, 4 GiB + R - low end)`; kernel and initrd
/// inside RAM; loader structures outside RAM.
pub(crate) fn validate_linux_e820(
    entries: &[LinuxE820Entry],
    plan: &LinuxPlacement,
    ram_size: u64,
) -> Result<(), LinuxBootParamsError> {
    if entries.is_empty() || entries.len() > LINUX_E820_MAX_ENTRIES {
        return Err(LinuxBootParamsError::BadEntryCount {
            count: entries.len(),
        });
    }
    let mut ranges = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let range = match entry.range() {
            Some(range) if entry.size > 0 => range,
            _ => return Err(LinuxBootParamsError::BadEntry { index }),
        };
        if !matches!(entry.kind, E820_RAM | E820_RESERVED | E820_ACPI | E820_NVS) {
            return Err(LinuxBootParamsError::UnknownType {
                index,
                kind: entry.kind,
            });
        }
        if let Some(prev) = ranges.last().map(|r: &LinuxRange| r.end)
            && range.start < prev
        {
            return Err(LinuxBootParamsError::NotSorted { index });
        }
        if matches!(entry.kind, E820_ACPI | E820_NVS)
            && (range.start % E820_ACPI_ALIGN != 0 || range.end % E820_ACPI_ALIGN != 0)
        {
            return Err(LinuxBootParamsError::AcpiEntryUnaligned { index });
        }
        ranges.push(range);
    }
    for (zone, expected, found) in [
        (
            LinuxZone::AcpiTables,
            FIRMWARE_ACPI_TABLES_BASE,
            plan.acpi_tables.start,
        ),
        (
            LinuxZone::AcpiFacs,
            FIRMWARE_ACPI_FACS_BASE,
            plan.acpi_facs.start,
        ),
    ] {
        if found != expected {
            return Err(LinuxBootParamsError::AcpiBaseMismatch {
                zone,
                expected,
                found,
            });
        }
    }
    let of_kind = |kind: u32| {
        entries
            .iter()
            .zip(&ranges)
            .filter(move |(entry, _)| entry.kind == kind)
            .map(|(_, range)| *range)
    };

    let ecam = ecam_range()?;
    if !of_kind(E820_RESERVED).any(|r| covers(r, ecam)) {
        return Err(LinuxBootParamsError::EcamNotReserved { ecam });
    }
    for window in [framebuffer_range()?, rom_range()?] {
        if !of_kind(E820_RESERVED).any(|r| covers(r, window)) {
            return Err(LinuxBootParamsError::WindowNotReserved { window });
        }
    }
    if !of_kind(E820_ACPI).any(|r| covers(r, plan.acpi_tables)) {
        return Err(LinuxBootParamsError::AcpiNotCovered {
            zone: LinuxZone::AcpiTables,
            range: plan.acpi_tables,
        });
    }
    if !of_kind(E820_NVS).any(|r| covers(r, plan.acpi_facs)) {
        return Err(LinuxBootParamsError::AcpiNotCovered {
            zone: LinuxZone::AcpiFacs,
            range: plan.acpi_facs,
        });
    }

    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);
    let expected_low = [
        LinuxRange {
            start: 0,
            end: LINUX_LOW_FORBIDDEN_END,
        },
        LinuxRange {
            start: LINUX_LOADER_END,
            end: low_end,
        },
    ];
    let low: Vec<LinuxRange> = of_kind(E820_RAM)
        .filter(|r| r.start < FIRMWARE_HIGH_RAM_BASE)
        .collect();
    if low != expected_low {
        return Err(LinuxBootParamsError::LowRamMismatch {
            expected: expected_low,
            found_count: low.len(),
        });
    }
    let high_len = ram_size - low_end;
    let expected_high = if high_len > 0 {
        Some(LinuxRange {
            start: FIRMWARE_HIGH_RAM_BASE,
            end: FIRMWARE_HIGH_RAM_BASE
                .checked_add(high_len)
                .ok_or(LinuxBootParamsError::RangeOverflow)?,
        })
    } else {
        None
    };
    let high: Vec<LinuxRange> = of_kind(E820_RAM)
        .filter(|r| r.start >= FIRMWARE_HIGH_RAM_BASE)
        .collect();
    if high != expected_high.into_iter().collect::<Vec<_>>() {
        return Err(LinuxBootParamsError::HighRamMismatch {
            expected: expected_high,
            found_count: high.len(),
        });
    }

    let ram: Vec<LinuxRange> = of_kind(E820_RAM).collect();
    for (zone, range) in [
        (LinuxZone::Kernel, Some(plan.kernel)),
        (LinuxZone::Initrd, plan.initrd),
    ] {
        if let Some(range) = range
            && !ram.iter().any(|r| covers(*r, range))
        {
            return Err(LinuxBootParamsError::ZoneNotRam { zone, range });
        }
    }
    for (zone, range) in [
        (LinuxZone::BootParams, plan.boot_params),
        (LinuxZone::Cmdline, plan.cmdline),
        (LinuxZone::Gdt, plan.gdt),
        (LinuxZone::PageTables, plan.page_tables),
        (LinuxZone::Stack, plan.stack),
    ] {
        if ram.iter().any(|r| r.overlaps(&range)) {
            return Err(LinuxBootParamsError::LoaderZoneInRam { zone, range });
        }
    }
    Ok(())
}

fn le32(block: &[u8], at: usize) -> Option<u32> {
    let bytes = block.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn le64(block: &[u8], at: usize) -> Option<u64> {
    let bytes = block.get(at..at.checked_add(8)?)?;
    Some(u64::from_le_bytes(bytes.try_into().ok()?))
}

/// Reads the region table of a handoff block (`LoadedFirmwareBoard::handoff_block`).
pub(crate) fn parse_handoff_regions(
    block: &[u8],
) -> Result<Vec<LinuxHandoffRegion>, LinuxBootParamsError> {
    let malformed = || LinuxBootParamsError::HandoffMalformed;
    if block.get(..HANDOFF_MAGIC.len()) != Some(HANDOFF_MAGIC.as_slice()) {
        return Err(malformed());
    }
    let table = le32(block, HANDOFF_OFF_REGION_TABLE).ok_or_else(malformed)? as usize;
    let count = le32(block, HANDOFF_OFF_REGION_COUNT).ok_or_else(malformed)? as usize;
    if le32(block, HANDOFF_OFF_REGION_ENTRY_SIZE) != Some(HANDOFF_REGION_ENTRY_BYTES as u32) {
        return Err(malformed());
    }
    let mut regions = Vec::with_capacity(count.min(block.len() / HANDOFF_REGION_ENTRY_BYTES));
    for index in 0..count {
        let at = index
            .checked_mul(HANDOFF_REGION_ENTRY_BYTES)
            .and_then(|off| off.checked_add(table))
            .ok_or_else(malformed)?;
        let kind = le32(block, at).ok_or_else(malformed)?;
        let base_at = at.checked_add(8).ok_or_else(malformed)?;
        let length_at = at.checked_add(0x10).ok_or_else(malformed)?;
        let base = le64(block, base_at).ok_or_else(malformed)?;
        let length = le64(block, length_at).ok_or_else(malformed)?;
        let end = base.checked_add(length).ok_or_else(malformed)?;
        regions.push(LinuxHandoffRegion {
            kind,
            range: LinuxRange { start: base, end },
        });
    }
    Ok(regions)
}

/// Picks handoff regions 11 and 12; each must occur exactly once.
pub(crate) fn handoff_acpi_regions(
    regions: &[LinuxHandoffRegion],
) -> Result<LinuxHandoffAcpi, LinuxBootParamsError> {
    let only = |kind: u32| {
        let count = regions.iter().filter(|r| r.kind == kind).count();
        match regions.iter().find(|r| r.kind == kind) {
            Some(region) if count == 1 => Ok(region.range),
            _ => Err(LinuxBootParamsError::HandoffRegionCount { kind, count }),
        }
    };
    Ok(LinuxHandoffAcpi {
        tables: only(HANDOFF_REGION_ACPI_TABLES)?,
        nvs: only(HANDOFF_REGION_ACPI_NVS)?,
    })
}

/// The ECAM window as the handoff declares it: the MMIO region at the ECAM base.
pub(crate) fn handoff_ecam_region(regions: &[LinuxHandoffRegion]) -> Option<LinuxRange> {
    regions
        .iter()
        .find(|r| r.kind == HANDOFF_REGION_MMIO && r.range.start == PCI_ECAM_DESCRIPTOR.base)
        .map(|r| r.range)
}

/// Requires exactly one E820 ACPI entry equal to handoff region 11 and exactly one E820 NVS
/// entry equal to handoff region 12.
pub(crate) fn check_e820_acpi_against_handoff(
    entries: &[LinuxE820Entry],
    handoff: &LinuxHandoffAcpi,
) -> Result<(), LinuxBootParamsError> {
    for (zone, kind, expected) in [
        (LinuxZone::AcpiTables, E820_ACPI, handoff.tables),
        (LinuxZone::AcpiFacs, E820_NVS, handoff.nvs),
    ] {
        let mut found = entries
            .iter()
            .filter(|e| e.kind == kind)
            .filter_map(LinuxE820Entry::range);
        let single = match (found.next(), found.next()) {
            (Some(range), None) => Some(range),
            _ => None,
        };
        if single != Some(expected) {
            return Err(LinuxBootParamsError::HandoffAcpiMismatch {
                zone,
                expected,
                found: single,
            });
        }
    }
    Ok(())
}

/// Compares an E820 map with the region table of the board's handoff block (S4-37L6): the single
/// ACPI and NVS entries equal handoff regions 11 and 12 ([`check_e820_acpi_against_handoff`]),
/// the handoff declares the ECAM window, and every handoff ROM, framebuffer and ECAM region is
/// covered by one reserved E820 entry.
pub(crate) fn check_e820_against_handoff(
    entries: &[LinuxE820Entry],
    regions: &[LinuxHandoffRegion],
) -> Result<(), LinuxBootParamsError> {
    check_e820_acpi_against_handoff(entries, &handoff_acpi_regions(regions)?)?;
    if handoff_ecam_region(regions).is_none() {
        return Err(LinuxBootParamsError::HandoffNoEcam);
    }
    let reserved: Vec<LinuxRange> = entries
        .iter()
        .filter(|e| e.kind == E820_RESERVED)
        .filter_map(LinuxE820Entry::range)
        .collect();
    for region in regions {
        let window = match region.kind {
            HANDOFF_REGION_ROM | HANDOFF_REGION_FRAMEBUFFER => true,
            HANDOFF_REGION_MMIO => region.range.start == PCI_ECAM_DESCRIPTOR.base,
            _ => false,
        };
        if window && !reserved.iter().any(|r| covers(*r, region.range)) {
            return Err(LinuxBootParamsError::HandoffWindowNotReserved {
                kind: region.kind,
                range: region.range,
            });
        }
    }
    Ok(())
}

/// Everything [`serialize_linux_boot_params`] needs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinuxBootParamsInput<'a> {
    /// The bzImage file (or at least its first `0x202 + header length` bytes, 0x26C for Linux
    /// 7.0): `setup_header` is copied from it.
    pub bzimage: &'a [u8],
    pub plan: &'a LinuxPlacement,
    pub ram_size: u64,
    pub e820: &'a [LinuxE820Entry],
    /// Physical address of the RSDP: `ValidatedAcpiBlob::rsdp_address`.
    pub rsdp_addr: u64,
}

impl<'a> LinuxBootParamsInput<'a> {
    pub(crate) fn new(
        bzimage: &'a [u8],
        plan: &'a LinuxPlacement,
        ram_size: u64,
        e820: &'a [LinuxE820Entry],
        acpi: &ValidatedAcpiBlob,
    ) -> Self {
        Self {
            bzimage,
            plan,
            ram_size,
            e820,
            rsdp_addr: acpi.rsdp_address,
        }
    }
}

fn put(
    page: &mut [u8; LINUX_BOOT_PARAMS_LEN],
    offset: usize,
    bytes: &[u8],
) -> Result<(), LinuxBootParamsError> {
    let slot = offset
        .checked_add(bytes.len())
        .and_then(|end| page.get_mut(offset..end))
        .ok_or(LinuxBootParamsError::WriteOutOfBounds { offset })?;
    slot.copy_from_slice(bytes);
    Ok(())
}

fn below_4g(range: LinuxRange) -> Option<(u32, u32)> {
    let start = u32::try_from(range.start).ok()?;
    let len = u32::try_from(range.len()).ok()?;
    (range.end <= FIRMWARE_HIGH_RAM_BASE).then_some((start, len))
}

/// Serializes the 4096-byte `boot_params` page:
/// - `setup_header` bytes `[0x1F1, end)` copied from the bzImage, where `end = 0x202 + byte at
///   0x201` (boot.rst; 0x26C for Linux 7.0, so `kernel_info_offset` is included) must lie in
///   `[0x268, 0x290]`; the bytes from `end` on stay 0. Then `type_of_loader` = 0xFF,
///   `cmd_line_ptr` = start of the command line buffer, `ramdisk_image` / `ramdisk_size` =
///   initrd start and length (both 0 without an initrd);
/// - `e820_entries` and `e820_table` from the (validated) map;
/// - `acpi_rsdp_addr` = RSDP address; the whole 36-byte RSDP must lie in the ACPI tables range
///   of the plan.
///
/// `loadflags` is left as the kernel built it: `CAN_USE_HEAP` only concerns the 16-bit setup code,
/// which a direct 64-bit boot never runs, and nothing in the 64-bit entry reads it. The `ext_*`
/// fields stay 0 because every address is below 4 GiB. All other bytes are 0.
pub(crate) fn serialize_linux_boot_params(
    input: &LinuxBootParamsInput<'_>,
) -> Result<[u8; LINUX_BOOT_PARAMS_LEN], LinuxBootParamsError> {
    let plan = input.plan;
    let too_short = || LinuxBootParamsError::SetupHeaderTooShort {
        len: input.bzimage.len(),
    };
    let header_length = *input.bzimage.get(OFF_HEADER_LENGTH).ok_or_else(too_short)?;
    let header_end = OFF_HEADER_LENGTH_BASE
        .checked_add(usize::from(header_length))
        .ok_or(LinuxBootParamsError::SetupHeaderLength { end: usize::MAX })?;
    if !(OFF_SETUP_HEADER_MIN_END..=OFF_SETUP_HEADER_MAX_END).contains(&header_end) {
        return Err(LinuxBootParamsError::SetupHeaderLength { end: header_end });
    }
    let header = input
        .bzimage
        .get(OFF_SETUP_HEADER..header_end)
        .ok_or_else(too_short)?;
    validate_linux_e820(input.e820, plan, input.ram_size)?;
    let (cmd_line_ptr, _) = below_4g(plan.cmdline).ok_or(LinuxBootParamsError::CmdlineAbove4G {
        range: plan.cmdline,
    })?;
    let (ramdisk_image, ramdisk_size) = match plan.initrd {
        Some(initrd) => {
            below_4g(initrd).ok_or(LinuxBootParamsError::InitrdAbove4G { range: initrd })?
        }
        None => (0, 0),
    };
    let tables = plan.acpi_tables;
    let rsdp_inside = input
        .rsdp_addr
        .checked_add(RSDP_V2_BYTES)
        .is_some_and(|end| input.rsdp_addr >= tables.start && end <= tables.end);
    if !rsdp_inside {
        return Err(LinuxBootParamsError::RsdpOutsideAcpiTables {
            rsdp: input.rsdp_addr,
            tables,
        });
    }

    let mut page = [0u8; LINUX_BOOT_PARAMS_LEN];
    put(&mut page, OFF_SETUP_HEADER, header)?;
    put(
        &mut page,
        OFF_TYPE_OF_LOADER,
        &[LINUX_TYPE_OF_LOADER_UNDEFINED],
    )?;
    put(&mut page, OFF_RAMDISK_IMAGE, &ramdisk_image.to_le_bytes())?;
    put(&mut page, OFF_RAMDISK_SIZE, &ramdisk_size.to_le_bytes())?;
    put(&mut page, OFF_CMD_LINE_PTR, &cmd_line_ptr.to_le_bytes())?;
    put(
        &mut page,
        OFF_ACPI_RSDP_ADDR,
        &input.rsdp_addr.to_le_bytes(),
    )?;
    // validate_linux_e820 bounds the count by 128, so it fits the u8 field.
    let count =
        u8::try_from(input.e820.len()).map_err(|_| LinuxBootParamsError::BadEntryCount {
            count: input.e820.len(),
        })?;
    put(&mut page, OFF_E820_ENTRIES, &[count])?;
    for (index, entry) in input.e820.iter().enumerate() {
        let at = index
            .checked_mul(E820_ENTRY_BYTES)
            .and_then(|off| off.checked_add(OFF_E820_TABLE))
            .ok_or(LinuxBootParamsError::WriteOutOfBounds {
                offset: OFF_E820_TABLE,
            })?;
        put(&mut page, at, &entry.addr.to_le_bytes())?;
        put(&mut page, at + 8, &entry.size.to_le_bytes())?;
        put(&mut page, at + 16, &entry.kind.to_le_bytes())?;
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc32::crc32_ieee;
    use crate::firmware_board::{
        FIRMWARE_ACPI_FACS_BASE, FIRMWARE_ACPI_FACS_BYTES, FIRMWARE_ACPI_TABLES_BASE,
        FIRMWARE_ACPI_TABLES_BYTES, FIRMWARE_IMAGE_BYTES, FIRMWARE_LINK_BASE,
        FIRMWARE_MAX_RAM_BYTES, FirmwareBoardConfig, load_firmware_image,
    };
    use crate::linux_boot::{LinuxBzImageHeader, parse_linux_bzimage_header};
    use crate::linux_placement::{LinuxPlacementInput, plan_linux_placement};

    const MIB: u64 = 1 << 20;
    const GIB: u64 = 1 << 30;
    const ECAM: LinuxRange = LinuxRange {
        start: 0xFC10_0000,
        end: 0xFC20_0000,
    };

    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }
    fn rd32(b: &[u8], off: usize) -> u32 {
        u32::from_le_bytes(b[off..off + 4].try_into().unwrap())
    }
    fn rd64(b: &[u8], off: usize) -> u64 {
        u64::from_le_bytes(b[off..off + 8].try_into().unwrap())
    }

    /// Synthetic bzImage shaped like the real one (pref 16 MiB, 2 MiB alignment, `init_size`
    /// 0x100000, `initrd_addr_max` 0x7FFF_FFFF, `cmdline_size` 0x7FF), same as the L2 tests.
    fn synthetic_file() -> Vec<u8> {
        let payload_offset = 0x1000u32;
        let payload_length = 0x2000u32;
        let pm_code = payload_offset + payload_length;
        let pm = 3 * 512;
        let mut b = vec![0u8; pm + pm_code as usize];
        b[0x1F1] = 2;
        b[0x201] = 0x6A; // header end 0x202 + 0x6A = 0x26C, as in Linux 7.0
        put32(&mut b, 0x1F4, pm_code / 16);
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
        put64(&mut b, 0x258, 0x0100_0000);
        put32(&mut b, 0x260, 0x0010_0000);
        b
    }

    /// The synthetic image with every `setup_header` field that the parser does not validate set
    /// to a distinct non-zero pattern, including the fields the serializer must overwrite.
    fn patterned_file() -> Vec<u8> {
        let mut b = synthetic_file();
        // (offset, length) of root_flags, ram_size, vid_mode, root_dev (the byte at 0x201 is the
        // header length and stays), jump, realmode_swtch, start_sys_seg, kernel_version,
        // type_of_loader, setup_move_size, code32_start, ramdisk_image, ramdisk_size,
        // bootsect_kludge, heap_end_ptr, ext_loader_ver/type, cmd_line_ptr, min_alignment,
        // hardware_subarch, handover_offset, kernel_info_offset.
        let fields: [(usize, usize); 20] = [
            (0x1F2, 2),
            (0x1F8, 4),
            (0x1FC, 2),
            (0x200, 1),
            (0x208, 4),
            (0x20C, 2),
            (0x20E, 2),
            (0x210, 1),
            (0x212, 2),
            (0x214, 4),
            (0x218, 4),
            (0x21C, 4),
            (0x220, 4),
            (0x224, 2),
            (0x226, 2),
            (0x228, 4),
            (0x235, 1),
            (0x23C, 4),
            (0x264, 4),
            (0x268, 4),
        ];
        let mut value = 0x41u8;
        for (off, len) in fields {
            for byte in &mut b[off..off + len] {
                *byte = value;
                value = value.wrapping_add(7) | 1;
            }
        }
        b[0x211] = 0x81; // LOADED_HIGH | CAN_USE_HEAP, must stay as is
        b
    }

    fn acpi_blob() -> ValidatedAcpiBlob {
        ValidatedAcpiBlob {
            version: 2,
            blob_sha256: [0; 32],
            manifest_hash: [0; 32],
            load_base: 0xE_0000,
            payload_offset: 0x60,
            payload_len: 0x400,
            tables: Vec::new(),
            rsdp_address: 0xE_0000,
            facs_offset: 0,
            facs_address: 0xE_F000,
        }
    }

    fn plan_for(
        header: &LinuxBzImageHeader,
        acpi: &ValidatedAcpiBlob,
        ram_size: u64,
        initrd_len: u64,
    ) -> LinuxPlacement {
        plan_linux_placement(
            header,
            &LinuxPlacementInput {
                ram_size,
                initrd_len,
                cmdline_len: 64,
                acpi,
            },
        )
        .expect("plan")
    }

    fn e(addr: u64, size: u64, kind: u32) -> LinuxE820Entry {
        LinuxE820Entry { addr, size, kind }
    }

    /// The first six entries and the framebuffer, ECAM and ROM entries are the same for every
    /// RAM size; the entry at index 5 ends at the low RAM end.
    fn expected_e820(low_end: u64, high: u64) -> Vec<LinuxE820Entry> {
        let mut v = vec![
            e(0, 0x9_F000, E820_RAM),
            e(0x9_F000, 0x4_1000, E820_RESERVED),
            e(0xE_0000, 0xF000, E820_ACPI),
            e(0xE_F000, 0x1000, E820_NVS),
            e(0xF_0000, 0x6_0000, E820_RESERVED),
            e(0x15_0000, low_end - 0x15_0000, E820_RAM),
            e(0xF800_0000, 0x0240_0000, E820_RESERVED),
            e(0xFC10_0000, 0x10_0000, E820_RESERVED),
            e(0xFFC0_0000, 0x40_0000, E820_RESERVED),
        ];
        if high > 0 {
            v.push(e(0x1_0000_0000, high, E820_RAM));
        }
        v
    }

    fn entry_bytes(entry: &LinuxE820Entry) -> [u8; 20] {
        let mut b = [0u8; 20];
        b[..8].copy_from_slice(&entry.addr.to_le_bytes());
        b[8..16].copy_from_slice(&entry.size.to_le_bytes());
        b[16..].copy_from_slice(&entry.kind.to_le_bytes());
        b
    }

    /// Serializes `file` for a fresh plan and returns everything the asserts need.
    fn build(
        file: &[u8],
        ram_size: u64,
        initrd_len: u64,
    ) -> (
        LinuxPlacement,
        Vec<LinuxE820Entry>,
        [u8; LINUX_BOOT_PARAMS_LEN],
    ) {
        let header = parse_linux_bzimage_header(file).expect("header");
        let acpi = acpi_blob();
        let plan = plan_for(&header, &acpi, ram_size, initrd_len);
        let e820 = build_linux_e820(&plan, ram_size).expect("e820");
        let page = serialize_linux_boot_params(&LinuxBootParamsInput::new(
            file, &plan, ram_size, &e820, &acpi,
        ))
        .expect("page");
        (plan, e820, page)
    }

    /// Every byte outside `written` is zero.
    fn assert_zero_outside(page: &[u8], written: &[(usize, usize)]) {
        for (at, byte) in page.iter().enumerate() {
            let inside = written.iter().any(|&(s, l)| at >= s && at < s + l);
            assert!(inside || *byte == 0, "stray byte {byte:#x} at {at:#x}");
        }
    }

    fn written_ranges(entries: usize) -> [(usize, usize); 4] {
        [
            (0x70, 8),
            (0x1E8, 1),
            (0x1F1, 0x26C - 0x1F1),
            (0x2D0, entries * 20),
        ]
    }

    fn assert_e820_in_page(page: &[u8], e820: &[LinuxE820Entry]) {
        assert_eq!(page[0x1E8] as usize, e820.len());
        for (i, entry) in e820.iter().enumerate() {
            assert_eq!(
                page[0x2D0 + i * 20..0x2D0 + (i + 1) * 20],
                entry_bytes(entry)
            );
        }
    }

    #[test]
    fn golden_128_mib() {
        let file = patterned_file();
        let (plan, e820, page) = build(&file, 128 * MIB, 4 * MIB);
        assert_eq!(
            plan.initrd,
            Some(LinuxRange {
                start: 0x7C0_0000,
                end: 0x800_0000
            })
        );
        assert_eq!(e820, expected_e820(0x800_0000, 0));
        assert_eq!(e820.len(), 9);

        assert_eq!(page.len(), 4096);
        assert_eq!(page[0x210], 0xFF);
        assert_eq!(rd32(&page, 0x228), 0x10_1000);
        assert_eq!(rd32(&page, 0x218), 0x7C0_0000);
        assert_eq!(rd32(&page, 0x21C), 0x40_0000);
        assert_eq!(rd64(&page, 0x70), 0xE_0000);
        assert_eq!(page[0x211], 0x81);
        assert_eq!(page[0x1E8], 9);
        // First entry: RAM [0, 0x9F000); entry 7: ECAM reserved; last: ROM reserved.
        let mut first = [0u8; 20];
        first[8..12].copy_from_slice(&0x9_F000u32.to_le_bytes());
        first[16] = 1;
        assert_eq!(page[0x2D0..0x2D0 + 20], first);
        let mut ecam = [0u8; 20];
        ecam[..4].copy_from_slice(&0xFC10_0000u32.to_le_bytes());
        ecam[8..12].copy_from_slice(&0x10_0000u32.to_le_bytes());
        ecam[16] = 2;
        assert_eq!(page[0x2D0 + 7 * 20..0x2D0 + 8 * 20], ecam);
        let mut rom = [0u8; 20];
        rom[..4].copy_from_slice(&0xFFC0_0000u32.to_le_bytes());
        rom[8..12].copy_from_slice(&0x40_0000u32.to_le_bytes());
        rom[16] = 2;
        assert_eq!(page[0x2D0 + 8 * 20..0x2D0 + 9 * 20], rom);
        // The entry after the last one is untouched.
        assert_eq!(page[0x2D0 + 9 * 20..0x2D0 + 10 * 20], [0u8; 20]);
        assert_e820_in_page(&page, &e820);
        assert_zero_outside(&page, &written_ranges(9));
    }

    #[test]
    fn golden_maximum_ram() {
        let file = patterned_file();
        let (plan, e820, page) = build(&file, FIRMWARE_MAX_RAM_BYTES, 4 * MIB);
        assert_eq!(FIRMWARE_MAX_RAM_BYTES, 0xF_F400_0000);
        // Low RAM stops at 0xF400_0000; the initrd is capped by initrd_addr_max + 1 = 2 GiB.
        assert_eq!(
            plan.initrd,
            Some(LinuxRange {
                start: 0x7FC0_0000,
                end: 0x8000_0000
            })
        );
        assert_eq!(e820, expected_e820(0xF400_0000, 0xF_0000_0000));
        assert_eq!(e820.len(), 10);
        assert_eq!(e820[5], e(0x15_0000, 0xF3EB_0000, E820_RAM));
        assert_eq!(e820[9], e(0x1_0000_0000, 0xF_0000_0000, E820_RAM));
        assert_eq!(page[0x1E8], 10);
        let mut high = [0u8; 20];
        high[4] = 1; // addr 0x1_0000_0000
        high[12] = 0xF; // size 0xF_0000_0000
        high[16] = 1;
        assert_eq!(page[0x2D0 + 9 * 20..0x2D0 + 10 * 20], high);
        assert_eq!(rd32(&page, 0x218), 0x7FC0_0000);
        assert_eq!(rd32(&page, 0x228), 0x10_1000);
        assert_e820_in_page(&page, &e820);
        assert_zero_outside(&page, &written_ranges(10));
    }

    #[test]
    fn four_gib_has_a_high_ram_entry_from_4_gib() {
        let file = patterned_file();
        let (_, e820, page) = build(&file, 4 * GIB, 0);
        assert_eq!(e820, expected_e820(0xF400_0000, 0x0C00_0000));
        assert_eq!(e820[9], e(0x1_0000_0000, 0x0C00_0000, E820_RAM));
        assert_e820_in_page(&page, &e820);
        // 3 GiB is below the limit 0xF400_0000 (3 GiB + 0x3400_0000): no high entry.
        let (_, e820, _) = build(&file, 3 * GIB, 0);
        assert_eq!(e820, expected_e820(0xC000_0000, 0));
    }

    #[test]
    fn no_initrd_zeroes_the_ramdisk_fields() {
        let file = patterned_file();
        let (plan, _, page) = build(&file, 128 * MIB, 0);
        assert_eq!(plan.initrd, None);
        assert_eq!(rd32(&page, 0x218), 0);
        assert_eq!(rd32(&page, 0x21C), 0);
        assert_eq!(rd32(&page, 0x228), 0x10_1000);
    }

    #[test]
    fn setup_header_is_copied_byte_for_byte_except_the_loader_fields() {
        let file = patterned_file();
        // The pattern really differs from what the serializer writes.
        assert_ne!(file[0x210], 0xFF);
        assert_ne!(rd32(&file, 0x218), 0x7C0_0000);
        assert_ne!(rd32(&file, 0x228), 0x10_1000);
        let (_, _, page) = build(&file, 128 * MIB, 4 * MIB);
        // type_of_loader, ramdisk_image, ramdisk_size, cmd_line_ptr.
        let overwritten =
            |at: usize| at == 0x210 || (0x218..0x220).contains(&at) || (0x228..0x22C).contains(&at);
        let mut compared = 0;
        for at in 0x1F1..0x26C {
            if !overwritten(at) {
                assert_eq!(page[at], file[at], "offset {at:#x}");
                compared += 1;
            }
        }
        assert_eq!(compared, 0x7B - 1 - 8 - 4);
        // kernel_info_offset (0x268, past the 2.14 minimum) is inside the copy.
        assert_ne!(rd32(&file, 0x268), 0);
        assert_eq!(page[0x268..0x26C], file[0x268..0x26C]);
        // Bytes below 0x1F1 and from the header end 0x26C on are not copied.
        assert_eq!(page[0x1F0], 0);
        assert_eq!(page[0x26C], 0);
    }

    #[test]
    fn header_offsets_agree_with_the_parser() {
        // The serializer writes these fields at the OFF_* offsets of the page; the parser must
        // read the same fields from the same offsets of the file.
        let file = patterned_file();
        let h = parse_linux_bzimage_header(&file).expect("header");
        assert_eq!(h.type_of_loader, file[OFF_TYPE_OF_LOADER]);
        assert_eq!(h.ramdisk_image, rd32(&file, OFF_RAMDISK_IMAGE));
        assert_eq!(h.ramdisk_size, rd32(&file, OFF_RAMDISK_SIZE));
        assert_eq!(h.cmd_line_ptr, rd32(&file, OFF_CMD_LINE_PTR));
        assert_eq!(h.loadflags, 0x81);
    }

    fn handoff_block(ram_bytes: u64) -> Vec<u8> {
        let mut image = vec![0u8; FIRMWARE_IMAGE_BYTES];
        image[..8].copy_from_slice(b"ODFWIMG1");
        put32(&mut image, 0x08, 1);
        put32(&mut image, 0x0C, 0x40);
        put64(&mut image, 0x10, FIRMWARE_IMAGE_BYTES as u64);
        put64(&mut image, 0x18, FIRMWARE_LINK_BASE);
        put64(&mut image, 0x20, 0x40);
        put16(&mut image, 0x28, 1);
        put16(&mut image, 0x2A, 2); // handoff minor 2: ACPI regions
        image[0x40] = 0xF4;
        let crc = crc32_ieee(&[&image]);
        put32(&mut image, 0x30, crc);
        let config = FirmwareBoardConfig {
            ram_bytes,
            pci_ecam: true,
            acpi_pm: true,
            acpi_tables: true,
            linux_profile: true,
            ..FirmwareBoardConfig::default()
        };
        load_firmware_image(&image, &config)
            .expect("board")
            .handoff_block()
            .to_vec()
    }

    #[test]
    fn e820_acpi_ranges_equal_handoff_regions_11_and_12() {
        // Loading a board with more RAM costs seconds and GBs (the loader materializes the
        // boot page tables); regions 11 and 12 do not depend on R, so small sizes stand in. The
        // E820 side is built for the same R.
        for ram_size in [128 * MIB, 256 * MIB] {
            let regions = parse_handoff_regions(&handoff_block(ram_size)).expect("regions");
            let acpi = handoff_acpi_regions(&regions).expect("regions 11 and 12");
            assert_eq!(
                acpi.tables,
                LinuxRange {
                    start: 0xE_0000,
                    end: 0xE_F000
                }
            );
            assert_eq!(
                acpi.nvs,
                LinuxRange {
                    start: 0xE_F000,
                    end: 0xF_0000
                }
            );
            // The board constants say the same thing.
            assert_eq!(acpi.tables.start, FIRMWARE_ACPI_TABLES_BASE);
            assert_eq!(acpi.tables.len(), FIRMWARE_ACPI_TABLES_BYTES);
            assert_eq!(acpi.nvs.start, FIRMWARE_ACPI_FACS_BASE);
            assert_eq!(acpi.nvs.len(), FIRMWARE_ACPI_FACS_BYTES);

            let file = synthetic_file();
            let (_, e820, _) = build(&file, ram_size, 4 * MIB);
            check_e820_acpi_against_handoff(&e820, &acpi).expect("matches the handoff");
            // The ECAM window reserved in E820 is the MMIO region the handoff declares.
            assert_eq!(handoff_ecam_region(&regions), Some(ECAM));
            assert!(e820.contains(&e(ECAM.start, ECAM.len(), E820_RESERVED)));
        }
    }

    #[test]
    fn shifted_facs_diverges_from_the_handoff() {
        let regions = parse_handoff_regions(&handoff_block(128 * MIB)).expect("regions");
        let acpi_h = handoff_acpi_regions(&regions).expect("11 and 12");
        let header = parse_linux_bzimage_header(&synthetic_file()).expect("header");

        // Baseline: the unshifted blob matches.
        let blob = acpi_blob();
        let plan = plan_for(&header, &blob, 128 * MIB, 0);
        let e820 = build_linux_e820(&plan, 128 * MIB).expect("e820");
        check_e820_acpi_against_handoff(&e820, &acpi_h).expect("baseline matches");

        // FACS one byte up or down: the plan no longer starts at the board constant, so the map
        // is rejected at build time (before any comparison with the handoff).
        for delta in [1i64, -1] {
            let mut shifted = acpi_blob();
            shifted.facs_address = shifted.facs_address.wrapping_add_signed(delta);
            let plan = plan_for(&header, &shifted, 128 * MIB, 0);
            assert_eq!(
                build_linux_e820(&plan, 128 * MIB),
                Err(LinuxBootParamsError::AcpiBaseMismatch {
                    zone: LinuxZone::AcpiFacs,
                    expected: FIRMWARE_ACPI_FACS_BASE,
                    found: FIRMWARE_ACPI_FACS_BASE.wrapping_add_signed(delta),
                }),
                "delta {delta}"
            );
        }

        // The tables moved: same.
        let mut moved = acpi_blob();
        moved.load_base = 0xE_0010;
        let moved_plan = plan_for(&header, &moved, 128 * MIB, 0);
        assert_eq!(
            build_linux_e820(&moved_plan, 128 * MIB),
            Err(LinuxBootParamsError::AcpiBaseMismatch {
                zone: LinuxZone::AcpiTables,
                expected: FIRMWARE_ACPI_TABLES_BASE,
                found: 0xE_0010,
            })
        );
        // validate rejects the same plan for a baseline map.
        assert_eq!(
            validate_linux_e820(&e820, &moved_plan, 128 * MIB),
            Err(LinuxBootParamsError::AcpiBaseMismatch {
                zone: LinuxZone::AcpiTables,
                expected: FIRMWARE_ACPI_TABLES_BASE,
                found: 0xE_0010,
            })
        );

        // The handoff comparison on its own: hand-edited entries (still valid maps in shape)
        // differ from regions 12 and 11.
        let mut nvs_up = e820.clone();
        nvs_up[3] = e(0xE_F001, 0xFFF, E820_NVS);
        assert_eq!(
            check_e820_acpi_against_handoff(&nvs_up, &acpi_h),
            Err(LinuxBootParamsError::HandoffAcpiMismatch {
                zone: LinuxZone::AcpiFacs,
                expected: acpi_h.nvs,
                found: Some(LinuxRange {
                    start: 0xE_F001,
                    end: 0xF_0000
                }),
            })
        );
        let mut tables_up = e820.clone();
        tables_up[2] = e(0xE_0010, 0xEFF0, E820_ACPI);
        assert_eq!(
            check_e820_acpi_against_handoff(&tables_up, &acpi_h),
            Err(LinuxBootParamsError::HandoffAcpiMismatch {
                zone: LinuxZone::AcpiTables,
                expected: acpi_h.tables,
                found: Some(LinuxRange {
                    start: 0xE_0010,
                    end: 0xE_F000
                }),
            })
        );
        // A second ACPI entry is not "exactly one".
        let mut doubled = e820.clone();
        doubled.push(e(0x3_0000_0000, 0x1000, E820_ACPI));
        assert!(matches!(
            check_e820_acpi_against_handoff(&doubled, &acpi_h),
            Err(LinuxBootParamsError::HandoffAcpiMismatch { found: None, .. })
        ));
    }

    #[test]
    fn unaligned_acpi_or_nvs_entries_are_rejected() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);
        // ACPI end one byte short, NVS start one byte up (end stays aligned).
        let mut acpi_end = e820.clone();
        acpi_end[2].size -= 1;
        assert_eq!(
            page_with(&file, &plan, &acpi_end),
            Err(LinuxBootParamsError::AcpiEntryUnaligned { index: 2 })
        );
        let mut nvs_start = e820.clone();
        nvs_start[3].addr += 1;
        nvs_start[3].size -= 1;
        assert_eq!(
            page_with(&file, &plan, &nvs_start),
            Err(LinuxBootParamsError::AcpiEntryUnaligned { index: 3 })
        );
    }

    #[test]
    fn handoff_without_region_11_or_with_garbage_is_rejected() {
        let block = handoff_block(128 * MIB);
        let regions = parse_handoff_regions(&block).expect("regions");
        let without: Vec<_> = regions.iter().copied().filter(|r| r.kind != 11).collect();
        assert_eq!(
            handoff_acpi_regions(&without),
            Err(LinuxBootParamsError::HandoffRegionCount { kind: 11, count: 0 })
        );
        let mut twice = regions.clone();
        twice.push(*regions.iter().find(|r| r.kind == 12).unwrap());
        assert_eq!(
            handoff_acpi_regions(&twice),
            Err(LinuxBootParamsError::HandoffRegionCount { kind: 12, count: 2 })
        );
        let mut bad_magic = block.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            parse_handoff_regions(&bad_magic),
            Err(LinuxBootParamsError::HandoffMalformed)
        );
        assert_eq!(
            parse_handoff_regions(&block[..0xA0 + 40]),
            Err(LinuxBootParamsError::HandoffMalformed)
        );
        let mut bad_entry_size = block.clone();
        put32(&mut bad_entry_size, 0x38, 40);
        assert_eq!(
            parse_handoff_regions(&bad_entry_size),
            Err(LinuxBootParamsError::HandoffMalformed)
        );
        assert_eq!(
            parse_handoff_regions(&[]),
            Err(LinuxBootParamsError::HandoffMalformed)
        );
    }

    fn page_with(
        file: &[u8],
        plan: &LinuxPlacement,
        e820: &[LinuxE820Entry],
    ) -> Result<[u8; LINUX_BOOT_PARAMS_LEN], LinuxBootParamsError> {
        serialize_linux_boot_params(&LinuxBootParamsInput::new(
            file,
            plan,
            128 * MIB,
            e820,
            &acpi_blob(),
        ))
    }

    #[test]
    fn missing_ecam_fails_validation_and_serialization() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);
        let ecam_error = LinuxBootParamsError::EcamNotReserved { ecam: ECAM };

        let without: Vec<_> = e820
            .iter()
            .copied()
            .filter(|x| x.addr != ECAM.start)
            .collect();
        assert_eq!(without.len(), 8);
        assert_eq!(
            validate_linux_e820(&without, &plan, 128 * MIB),
            Err(ecam_error)
        );
        assert_eq!(page_with(&file, &plan, &without), Err(ecam_error));

        // One byte short of the window is not enough.
        let mut short = e820.clone();
        short[7].size -= 1;
        assert_eq!(page_with(&file, &plan, &short), Err(ecam_error));
        // The window typed as RAM (or ACPI) is not reserved.
        for kind in [E820_RAM, E820_ACPI] {
            let mut wrong = e820.clone();
            wrong[7].kind = kind;
            assert_eq!(
                page_with(&file, &plan, &wrong),
                Err(ecam_error),
                "kind {kind}"
            );
        }
        // The unmodified map serializes.
        assert!(page_with(&file, &plan, &e820).is_ok());
    }

    #[test]
    fn missing_framebuffer_or_rom_window_fails_validation() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);
        for (addr, window) in [
            (
                0xF800_0000,
                LinuxRange {
                    start: 0xF800_0000,
                    end: 0xFA40_0000,
                },
            ),
            (
                0xFFC0_0000,
                LinuxRange {
                    start: 0xFFC0_0000,
                    end: 0x1_0000_0000,
                },
            ),
        ] {
            let without: Vec<_> = e820.iter().copied().filter(|x| x.addr != addr).collect();
            assert_eq!(
                validate_linux_e820(&without, &plan, 128 * MIB),
                Err(LinuxBootParamsError::WindowNotReserved { window })
            );
            // Typed as RAM it is not reserved either.
            let mut ram = e820.clone();
            ram.iter_mut().find(|x| x.addr == addr).unwrap().kind = E820_RAM;
            assert_eq!(
                validate_linux_e820(&ram, &plan, 128 * MIB),
                Err(LinuxBootParamsError::WindowNotReserved { window })
            );
        }
    }

    fn region(kind: u32, start: u64, end: u64) -> LinuxHandoffRegion {
        LinuxHandoffRegion {
            kind,
            range: LinuxRange { start, end },
        }
    }

    /// The handoff regions of the board that matter to E820: ACPI tables and NVS, ECAM, ROM and
    /// the framebuffer (kinds 11, 12, 3, 4, 6).
    fn board_regions() -> Vec<LinuxHandoffRegion> {
        vec![
            region(11, 0xE_0000, 0xE_F000),
            region(12, 0xE_F000, 0xF_0000),
            region(6, 0xF800_0000, 0xFA40_0000),
            region(3, 0xFC10_0000, 0xFC20_0000),
            region(3, 0xFEC0_0000, 0xFEC0_1000),
            region(4, 0xFFC0_0000, 0x1_0000_0000),
        ]
    }

    #[test]
    fn e820_is_checked_against_the_handoff_regions() {
        let file = synthetic_file();
        let (_, e820, _) = build(&file, 128 * MIB, 4 * MIB);
        assert_eq!(check_e820_against_handoff(&e820, &board_regions()), Ok(()));

        // A handoff without an ECAM window.
        let mut no_ecam = board_regions();
        no_ecam.retain(|r| r.range.start != 0xFC10_0000);
        assert_eq!(
            check_e820_against_handoff(&e820, &no_ecam),
            Err(LinuxBootParamsError::HandoffNoEcam)
        );
        // An ACPI region that moved in the handoff only.
        let mut moved = board_regions();
        moved[0].range.end = 0xE_E000;
        assert!(matches!(
            check_e820_against_handoff(&e820, &moved),
            Err(LinuxBootParamsError::HandoffAcpiMismatch { .. })
        ));
        // E820 entries missing for each window the handoff declares.
        for (addr, kind) in [(0xFC10_0000u64, 3), (0xF800_0000, 6), (0xFFC0_0000, 4)] {
            let without: Vec<_> = e820.iter().copied().filter(|x| x.addr != addr).collect();
            assert!(
                matches!(
                    check_e820_against_handoff(&without, &board_regions()),
                    Err(LinuxBootParamsError::HandoffWindowNotReserved { kind: k, .. }) if k == kind
                ),
                "window at {addr:#x}"
            );
        }
        // A window typed as RAM in E820 is not reserved.
        let mut ram = e820.clone();
        ram.iter_mut().find(|x| x.addr == 0xFFC0_0000).unwrap().kind = E820_RAM;
        assert!(matches!(
            check_e820_against_handoff(&ram, &board_regions()),
            Err(LinuxBootParamsError::HandoffWindowNotReserved { kind: 4, .. })
        ));
        // A second framebuffer in the handoff that E820 does not list is refused (the Linux
        // loader does not build second-screen boards).
        let mut two = board_regions();
        two.push(region(6, 0xFA40_0000, 0xFC80_0000));
        assert!(matches!(
            check_e820_against_handoff(&e820, &two),
            Err(LinuxBootParamsError::HandoffWindowNotReserved { kind: 6, .. })
        ));
    }

    #[test]
    fn malformed_maps_are_rejected() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);

        // 129 entries: one more than the zero page holds.
        let mut many = e820.clone();
        for i in 0..120u64 {
            many.push(e(0x2_0000_0000 + i * 0x2000, 0x1000, E820_RESERVED));
        }
        assert_eq!(many.len(), 129);
        assert_eq!(
            page_with(&file, &plan, &many),
            Err(LinuxBootParamsError::BadEntryCount { count: 129 })
        );
        many.truncate(128);
        assert!(page_with(&file, &plan, &many).is_ok());
        assert_eq!(
            validate_linux_e820(&[], &plan, 128 * MIB),
            Err(LinuxBootParamsError::BadEntryCount { count: 0 })
        );

        let mut unsorted = e820.clone();
        unsorted.swap(0, 1);
        assert_eq!(
            page_with(&file, &plan, &unsorted),
            Err(LinuxBootParamsError::NotSorted { index: 1 })
        );
        let mut empty = e820.clone();
        empty[1].size = 0;
        assert_eq!(
            page_with(&file, &plan, &empty),
            Err(LinuxBootParamsError::BadEntry { index: 1 })
        );
        let mut overflow = e820.clone();
        overflow[7].size = u64::MAX;
        assert_eq!(
            page_with(&file, &plan, &overflow),
            Err(LinuxBootParamsError::BadEntry { index: 7 })
        );
        let mut unknown = e820.clone();
        unknown[1].kind = 5;
        assert_eq!(
            page_with(&file, &plan, &unknown),
            Err(LinuxBootParamsError::UnknownType { index: 1, kind: 5 })
        );
    }

    #[test]
    fn ram_that_swallows_the_loader_area_or_acpi_is_rejected() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);

        // The loader area handed to RAM: three low RAM entries instead of two.
        let mut loader_ram = e820.clone();
        loader_ram[4].kind = E820_RAM;
        assert!(matches!(
            page_with(&file, &plan, &loader_ram),
            Err(LinuxBootParamsError::LowRamMismatch { found_count: 3, .. })
        ));
        // The ACPI entry typed RAM: the plan range is no longer covered.
        let mut acpi_ram = e820.clone();
        acpi_ram[2].kind = E820_RAM;
        assert_eq!(
            page_with(&file, &plan, &acpi_ram),
            Err(LinuxBootParamsError::AcpiNotCovered {
                zone: LinuxZone::AcpiTables,
                range: plan.acpi_tables
            })
        );
        // The FACS entry typed reserved.
        let mut nvs_reserved = e820.clone();
        nvs_reserved[3].kind = E820_RESERVED;
        assert_eq!(
            page_with(&file, &plan, &nvs_reserved),
            Err(LinuxBootParamsError::AcpiNotCovered {
                zone: LinuxZone::AcpiFacs,
                range: plan.acpi_facs
            })
        );
        // High RAM where the board has none.
        let mut extra = e820.clone();
        extra.push(e(0x1_0000_0000, 0x1000_0000, E820_RAM));
        assert_eq!(
            page_with(&file, &plan, &extra),
            Err(LinuxBootParamsError::HighRamMismatch {
                expected: None,
                found_count: 1
            })
        );
    }

    #[test]
    fn zones_must_sit_in_the_right_kind_of_entry() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);

        let mut kernel_in_hole = plan;
        kernel_in_hole.kernel = LinuxRange {
            start: 0xE_0000,
            end: 0x20_0000,
        };
        assert_eq!(
            page_with(&file, &kernel_in_hole, &e820),
            Err(LinuxBootParamsError::ZoneNotRam {
                zone: LinuxZone::Kernel,
                range: kernel_in_hole.kernel
            })
        );
        let mut initrd_high = plan;
        initrd_high.initrd = Some(LinuxRange {
            start: 0x7F0_0000,
            end: 0x810_0000,
        });
        assert!(matches!(
            page_with(&file, &initrd_high, &e820),
            Err(LinuxBootParamsError::ZoneNotRam {
                zone: LinuxZone::Initrd,
                ..
            })
        ));
        let mut gdt_in_ram = plan;
        gdt_in_ram.gdt = LinuxRange {
            start: 0x20_0000,
            end: 0x20_1000,
        };
        assert_eq!(
            page_with(&file, &gdt_in_ram, &e820),
            Err(LinuxBootParamsError::LoaderZoneInRam {
                zone: LinuxZone::Gdt,
                range: gdt_in_ram.gdt
            })
        );
    }

    #[test]
    fn serializer_rejects_bad_inputs() {
        let file = synthetic_file();
        let (plan, e820, _) = build(&file, 128 * MIB, 4 * MIB);

        // The header ends at 0x202 + 0x6A = 0x26C: that many bytes are needed and enough.
        assert_eq!(
            page_with(&file[..0x26B], &plan, &e820),
            Err(LinuxBootParamsError::SetupHeaderTooShort { len: 0x26B })
        );
        assert!(page_with(&file[..0x26C], &plan, &e820).is_ok());
        // Without the header length byte at 0x201 there is no header at all.
        assert_eq!(
            page_with(&file[..0x201], &plan, &e820),
            Err(LinuxBootParamsError::SetupHeaderTooShort { len: 0x201 })
        );
        // Header end below 0x268 (protocol 2.14 minimum) or above 0x290 (edd_mbr_sig_buffer).
        for (length, end) in [
            (0x65u8, 0x267usize),
            (0x00, 0x202),
            (0x8F, 0x291),
            (0xFF, 0x301),
        ] {
            let mut bad = file.clone();
            bad[0x201] = length;
            assert_eq!(
                page_with(&bad, &plan, &e820),
                Err(LinuxBootParamsError::SetupHeaderLength { end }),
                "length {length:#x}"
            );
        }
        // The bounds themselves are accepted: 2.14 (end 0x268) and 0x290; the copy follows.
        for (length, end) in [(0x66u8, 0x268usize), (0x8E, 0x290)] {
            let mut ok = file.clone();
            ok[0x201] = length;
            let page = page_with(&ok, &plan, &e820).expect("bound accepted");
            assert_eq!(page[end - 1], ok[end - 1]);
            assert_eq!(page[end], 0);
        }

        let mut cmdline_high = plan;
        cmdline_high.cmdline = LinuxRange {
            start: 0x1_0000_0000,
            end: 0x1_0000_0800,
        };
        assert_eq!(
            page_with(&file, &cmdline_high, &e820),
            Err(LinuxBootParamsError::CmdlineAbove4G {
                range: cmdline_high.cmdline
            })
        );
        let mut cmdline_straddle = plan;
        cmdline_straddle.cmdline = LinuxRange {
            start: 0xFFFF_FC00,
            end: 0x1_0000_0400,
        };
        assert!(matches!(
            page_with(&file, &cmdline_straddle, &e820),
            Err(LinuxBootParamsError::CmdlineAbove4G { .. })
        ));

        // The whole 36-byte RSDP must be inside the ACPI tables range of the plan
        // ([0xE_0000, 0xE_0400)): the last valid start is 0xE_0400 - 36 = 0xE_03DC.
        assert_eq!(plan.acpi_tables.end, 0xE_0400);
        for rsdp in [
            0,
            0xD_FFFF,
            0xE_03DD,
            0xE_03FF,
            0xE_0400,
            0xE_F000,
            u64::MAX,
        ] {
            let mut blob = acpi_blob();
            blob.rsdp_address = rsdp;
            let result = serialize_linux_boot_params(&LinuxBootParamsInput::new(
                &file,
                &plan,
                128 * MIB,
                &e820,
                &blob,
            ));
            assert_eq!(
                result,
                Err(LinuxBootParamsError::RsdpOutsideAcpiTables {
                    rsdp,
                    tables: plan.acpi_tables
                }),
                "rsdp {rsdp:#x}"
            );
        }
        let mut last_byte = acpi_blob();
        last_byte.rsdp_address = 0xE_03DC;
        let page = serialize_linux_boot_params(&LinuxBootParamsInput::new(
            &file,
            &plan,
            128 * MIB,
            &e820,
            &last_byte,
        ))
        .expect("RSDP ends exactly at the end of the tables");
        assert_eq!(rd64(&page, 0x70), 0xE_03DC);
    }

    const REAL_VMLINUZ: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"
    );

    /// Ignored by default because `work/` is not part of the repository. It must be run
    /// explicitly (`--ignored`) and fails, instead of passing vacuously, if the file is absent.
    #[test]
    #[ignore = "needs work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic; run with --ignored"]
    fn real_vmlinuz_golden_boot_params() {
        let file = std::fs::read(REAL_VMLINUZ)
            .unwrap_or_else(|e| panic!("vmlinuz absent, test NOT passed: {REAL_VMLINUZ}: {e}"));
        let initrd_len = 0x0200_0001u64;
        for (ram_size, low_end, high, initrd_start) in [
            (128 * MIB, 0x800_0000u64, 0u64, 0x5FF_F000u64),
            (4 * GIB, 0xF400_0000, 0x0C00_0000, 0x7DFF_F000),
            (
                FIRMWARE_MAX_RAM_BYTES,
                0xF400_0000,
                0xF_0000_0000,
                0x7DFF_F000,
            ),
        ] {
            let (plan, e820, page) = build(&file, ram_size, initrd_len);
            assert_eq!(
                plan.kernel,
                LinuxRange {
                    start: 0x100_0000,
                    end: 0x54C_1000
                }
            );
            assert_eq!(e820, expected_e820(low_end, high), "ram {ram_size:#x}");
            assert_eq!(rd32(&page, 0x228), 0x10_1000);
            assert_eq!(rd32(&page, 0x218) as u64, initrd_start);
            assert_eq!(rd32(&page, 0x21C), 0x0200_0001);
            assert_eq!(page[0x210], 0xFF);
            assert_eq!(rd64(&page, 0x70), 0xE_0000);
            assert_e820_in_page(&page, &e820);
            for at in 0x1F1..0x26C {
                let overwritten =
                    at == 0x210 || (0x218..0x220).contains(&at) || (0x228..0x22C).contains(&at);
                if !overwritten {
                    assert_eq!(page[at], file[at], "offset {at:#x}");
                }
            }
            // boot.rst: version 2.15 and LOADED_HIGH survive the copy.
            assert_eq!(u16::from_le_bytes([page[0x206], page[0x207]]), 0x020F);
            assert_eq!(page[0x211] & 1, 1);
            assert_zero_outside(&page, &written_ranges(e820.len()));
        }
    }
}
