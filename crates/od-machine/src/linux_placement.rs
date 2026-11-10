//! Pure placement plan of a Linux direct boot (S4-37L2).
//!
//! Given a validated bzImage header ([`LinuxBzImageHeader`]), the guest RAM size, the lengths of
//! the initrd and the command line and the validated ACPI blob, [`plan_linux_placement`] decides
//! where every piece lives in guest physical memory. It never reads or writes a guest and never
//! panics: every arithmetic step is checked and every violated rule has its own
//! [`LinuxPlacementError`] variant.
//!
//! Layout (guest physical, `[start, end)`):
//!
//! ```text
//! [0x00_0000, 0x09_F000)  forbidden (reserved for the Linux real-mode trampoline; the loader
//!                         places nothing here)
//! [0x0E_0000, 0x10_0000)  forbidden (ACPI region and BIOS ROM area; ACPI blob lives here)
//! [0x10_0000, 0x10_1000)  boot_params (zero page)
//! [0x10_1000, +cmdline_size+1)  command line buffer, inside the slot ending at 0x12_0000
//! [0x12_0000, 0x12_1000)  GDT
//! [0x13_0000, 0x14_0000)  boot page tables
//! [0x14_0000, 0x15_0000)  boot stack
//! [pref, pref + init_size)  kernel (loaded at its preferred address, no relocation search)
//! [..., min(low RAM end, initrd_addr_max + 1))  initrd, top-down, start page-aligned
//! ```
//!
//! The loader area `[0x10_0000, 0x15_0000)` must lie below the kernel (`[0x10_0000, pref)` in the
//! spec), so a kernel that starts below [`LINUX_LOADER_END`] is rejected. A kernel that
//! intersects a loader structure is reported as an [`LinuxPlacementError::Overlap`] when it
//! does; a kernel that sits in a gap below the loader area is reported as
//! [`LinuxPlacementError::KernelBelowLoaderArea`]. The kernel is never silently moved.
//!
//! RAM is not contiguous on the board: low RAM is `[0, min(R, 0xF400_0000))` and the rest of
//! `R` lives above 4 GiB. Everything the planner places must be inside low RAM.

// Used by L3-L6; tests exercise the planner now.
#![cfg_attr(not(test), allow(dead_code))]

use crate::acpi_blob::ValidatedAcpiBlob;
use crate::firmware_board::FIRMWARE_LOW_RAM_LIMIT;
use crate::linux_boot::LinuxBzImageHeader;

// The low RAM window is `[0, min(R, FIRMWARE_LOW_RAM_LIMIT))` (the board maps RAM there and the
// rest of RAM above 4 GiB, so nothing in `[FIRMWARE_LOW_RAM_LIMIT, 4 GiB)` is RAM). The constant
// has one definition, in `firmware_board.rs` (S4-37L6; before it was a copy here).

/// Page size used for initrd alignment.
pub(crate) const LINUX_PAGE_BYTES: u64 = 0x1000;

/// Everything below this address is reserved for the Linux real-mode trampoline; never used.
pub(crate) const LINUX_LOW_FORBIDDEN_END: u64 = 0x9_F000;
/// Start of the legacy ACPI / BIOS ROM area.
pub(crate) const LINUX_HIGH_FORBIDDEN_START: u64 = 0xE_0000;
/// End of the legacy ACPI / BIOS ROM area (1 MiB).
pub(crate) const LINUX_HIGH_FORBIDDEN_END: u64 = 0x10_0000;
/// Zones kernel, initrd and loader structures must not intersect.
pub(crate) const LINUX_FORBIDDEN_ZONES: [LinuxRange; 2] = [
    LinuxRange {
        start: 0,
        end: LINUX_LOW_FORBIDDEN_END,
    },
    LinuxRange {
        start: LINUX_HIGH_FORBIDDEN_START,
        end: LINUX_HIGH_FORBIDDEN_END,
    },
];

pub(crate) const LINUX_BOOT_PARAMS_BASE: u64 = 0x10_0000;
pub(crate) const LINUX_BOOT_PARAMS_BYTES: u64 = 0x1000;
pub(crate) const LINUX_CMDLINE_BASE: u64 = 0x10_1000;
/// End of the command line slot; the buffer (`cmdline_size + 1` bytes) must fit before it.
pub(crate) const LINUX_CMDLINE_SLOT_END: u64 = 0x12_0000;
pub(crate) const LINUX_GDT_BASE: u64 = 0x12_0000;
pub(crate) const LINUX_GDT_BYTES: u64 = 0x1000;
pub(crate) const LINUX_PAGE_TABLES_BASE: u64 = 0x13_0000;
pub(crate) const LINUX_PAGE_TABLES_BYTES: u64 = 0x1_0000;
pub(crate) const LINUX_STACK_BASE: u64 = 0x14_0000;
pub(crate) const LINUX_STACK_BYTES: u64 = 0x1_0000;
/// End of the whole loader area.
pub(crate) const LINUX_LOADER_END: u64 = LINUX_STACK_BASE + LINUX_STACK_BYTES;

/// Half-open guest physical range `[start, end)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinuxRange {
    pub start: u64,
    pub end: u64,
}

impl LinuxRange {
    pub(crate) fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.end <= self.start
    }

    pub(crate) fn overlaps(&self, other: &LinuxRange) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// A piece of guest memory the plan places or knows about.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxZone {
    Kernel,
    Initrd,
    BootParams,
    Cmdline,
    Gdt,
    PageTables,
    Stack,
    AcpiTables,
    AcpiFacs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LinuxPlacementError {
    /// `pref_address + init_size` (or another range end) does not fit in `u64`.
    RangeOverflow { zone: LinuxZone },
    /// A zone has zero or negative length (`init_size == 0`, inverted ACPI range, ...).
    EmptyRange { zone: LinuxZone },
    /// Low RAM (`min(ram_size, 0xF400_0000)`) cannot hold the kernel window or the loader area.
    RamTooSmall { low_ram_end: u64, required: u64 },
    /// A zone ends beyond the end of low RAM.
    OutsideRam {
        zone: LinuxZone,
        end: u64,
        low_ram_end: u64,
    },
    /// The command line is longer than `cmdline_size` of the header.
    CmdlineTooLong { len: u32, max: u32 },
    /// `cmdline_size + 1` does not fit in the command line slot.
    CmdlineBufferTooLarge { needed: u64, slot: u64 },
    /// The initrd is longer than the memory below its upper limit.
    InitrdTooLarge { len: u64, limit: u64 },
    /// The initrd would start below the end of the kernel window.
    NoRoomForInitrd {
        len: u64,
        kernel_end: u64,
        limit: u64,
    },
    /// A zone intersects `[0, 0x9_F000)` or `[0xE_0000, 0x10_0000)`.
    ForbiddenZone {
        zone: LinuxZone,
        range: LinuxRange,
        forbidden: LinuxRange,
    },
    /// Two zones intersect; `a` precedes `b` in [`LinuxPlacement::zones`] order.
    Overlap { a: LinuxZone, b: LinuxZone },
    /// The kernel starts below the end of the loader area (`0x15_0000`).
    KernelBelowLoaderArea { start: u64, loader_end: u64 },
    /// The kernel start is not a multiple of the header `kernel_alignment`.
    KernelMisaligned { start: u64, alignment: u64 },
    /// The initrd ends above `min(low RAM end, initrd_addr_max + 1)`.
    InitrdAboveLimit { end: u64, limit: u64 },
    /// The initrd start is not page-aligned.
    InitrdNotPageAligned { start: u64 },
    /// The initrd starts below the page-rounded end of the kernel.
    InitrdBelowKernel { start: u64, kernel_end: u64 },
}

/// Inputs of the planner that do not come from the bzImage header.
#[derive(Clone, Copy, Debug)]
pub(crate) struct LinuxPlacementInput<'a> {
    /// Full guest RAM size in bytes. Only low RAM `[0, min(ram_size, FIRMWARE_LOW_RAM_LIMIT))`
    /// is used for placement.
    pub ram_size: u64,
    /// Initrd length in bytes; 0 means no initrd.
    pub initrd_len: u64,
    /// Command line length in bytes, without the terminating NUL.
    pub cmdline_len: u32,
    /// Validated ACPI blob; its payload and FACS ranges are reserved.
    pub acpi: &'a ValidatedAcpiBlob,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinuxPlacement {
    /// `[pref_address, pref_address + init_size)`.
    pub kernel: LinuxRange,
    /// Page-aligned start, `end = start + initrd_len` (exact length). The page-rounded end is
    /// also below the limit. `None` when there is no initrd.
    pub initrd: Option<LinuxRange>,
    pub boot_params: LinuxRange,
    /// `cmdline_size + 1` bytes (the NUL included).
    pub cmdline: LinuxRange,
    pub gdt: LinuxRange,
    pub page_tables: LinuxRange,
    pub stack: LinuxRange,
    pub acpi_tables: LinuxRange,
    pub acpi_facs: LinuxRange,
}

impl LinuxPlacement {
    /// Every zone in the order used for overlap reporting; an absent initrd is skipped.
    pub(crate) fn zones(&self) -> Vec<(LinuxZone, LinuxRange)> {
        let mut zones = vec![(LinuxZone::Kernel, self.kernel)];
        if let Some(initrd) = self.initrd {
            zones.push((LinuxZone::Initrd, initrd));
        }
        zones.extend([
            (LinuxZone::BootParams, self.boot_params),
            (LinuxZone::Cmdline, self.cmdline),
            (LinuxZone::Gdt, self.gdt),
            (LinuxZone::PageTables, self.page_tables),
            (LinuxZone::Stack, self.stack),
            (LinuxZone::AcpiTables, self.acpi_tables),
            (LinuxZone::AcpiFacs, self.acpi_facs),
        ]);
        zones
    }
}

fn checked_range(zone: LinuxZone, start: u64, len: u64) -> Result<LinuxRange, LinuxPlacementError> {
    let end = start
        .checked_add(len)
        .ok_or(LinuxPlacementError::RangeOverflow { zone })?;
    Ok(LinuxRange { start, end })
}

/// Zones that are not allowed to touch the forbidden areas. ACPI zones live in the high
/// forbidden area by design and are exempt.
fn must_avoid_forbidden(zone: LinuxZone) -> bool {
    !matches!(zone, LinuxZone::AcpiTables | LinuxZone::AcpiFacs)
}

/// Re-checks the invariants of a finished plan: non-empty ranges, inside low RAM, outside the
/// forbidden areas (except ACPI), pairwise disjoint, kernel at or above the end of the loader
/// area and aligned to `header.kernel_alignment`, initrd page-aligned and ending at or below
/// `min(low RAM end, initrd_addr_max + 1)`. [`plan_linux_placement`] runs it on its own result;
/// it is also usable on a hand-modified plan (fault injection in tests).
pub(crate) fn validate_linux_placement(
    plan: &LinuxPlacement,
    header: &LinuxBzImageHeader,
    ram_size: u64,
) -> Result<(), LinuxPlacementError> {
    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);
    let zones = plan.zones();
    for &(zone, range) in &zones {
        if range.is_empty() {
            return Err(LinuxPlacementError::EmptyRange { zone });
        }
        if range.end > low_end {
            return Err(LinuxPlacementError::OutsideRam {
                zone,
                end: range.end,
                low_ram_end: low_end,
            });
        }
        if must_avoid_forbidden(zone) {
            for forbidden in LINUX_FORBIDDEN_ZONES {
                if range.overlaps(&forbidden) {
                    return Err(LinuxPlacementError::ForbiddenZone {
                        zone,
                        range,
                        forbidden,
                    });
                }
            }
        }
    }
    for (i, &(a, ra)) in zones.iter().enumerate() {
        for &(b, rb) in &zones[i + 1..] {
            if ra.overlaps(&rb) {
                return Err(LinuxPlacementError::Overlap { a, b });
            }
        }
    }
    if plan.kernel.start < LINUX_LOADER_END {
        return Err(LinuxPlacementError::KernelBelowLoaderArea {
            start: plan.kernel.start,
            loader_end: LINUX_LOADER_END,
        });
    }
    let alignment = u64::from(header.kernel_alignment);
    if plan.kernel.start.checked_rem(alignment) != Some(0) {
        return Err(LinuxPlacementError::KernelMisaligned {
            start: plan.kernel.start,
            alignment,
        });
    }
    if let Some(initrd) = plan.initrd {
        let limit = align_down(
            low_end.min(u64::from(header.initrd_addr_max) + 1),
            LINUX_PAGE_BYTES,
        );
        // Page-rounded end: the planner keeps the rounded end below the limit too.
        let rounded_end = page_round_up(initrd.end).unwrap_or(u64::MAX);
        if rounded_end > limit {
            return Err(LinuxPlacementError::InitrdAboveLimit {
                end: initrd.end,
                limit,
            });
        }
        if initrd.start % LINUX_PAGE_BYTES != 0 {
            return Err(LinuxPlacementError::InitrdNotPageAligned {
                start: initrd.start,
            });
        }
        let kernel_end = page_round_up(plan.kernel.end).unwrap_or(u64::MAX);
        if initrd.start < kernel_end {
            return Err(LinuxPlacementError::InitrdBelowKernel {
                start: initrd.start,
                kernel_end,
            });
        }
    }
    Ok(())
}

/// Rounds `value` up to a page boundary; `None` on overflow.
fn page_round_up(value: u64) -> Option<u64> {
    value
        .checked_add(LINUX_PAGE_BYTES - 1)
        .map(|v| align_down(v, LINUX_PAGE_BYTES))
}

fn align_down(value: u64, align: u64) -> u64 {
    value - value % align
}

/// Computes the placement of kernel, initrd, loader structures and ACPI blob.
///
/// Decisions:
/// - the kernel is placed exactly at `pref_address` (the parser already requires it to be a
///   multiple of `kernel_alignment`);
/// - everything must fit in low RAM `[0, min(ram_size, 0xF400_0000))`; RAM above 4 GiB is unused;
/// - the initrd goes as high as possible below `min(low RAM end, initrd_addr_max + 1)`, its
///   start rounded down to a page; it must start at or above the end of the kernel window;
/// - the loader area is a set of constants and is never moved;
/// - `cmdline_len == cmdline_size` is accepted (the header field excludes the NUL).
pub(crate) fn plan_linux_placement(
    header: &LinuxBzImageHeader,
    input: &LinuxPlacementInput<'_>,
) -> Result<LinuxPlacement, LinuxPlacementError> {
    let ram_size = input.ram_size;
    let low_end = ram_size.min(FIRMWARE_LOW_RAM_LIMIT);

    if input.cmdline_len > header.cmdline_size {
        return Err(LinuxPlacementError::CmdlineTooLong {
            len: input.cmdline_len,
            max: header.cmdline_size,
        });
    }
    let cmdline_bytes = u64::from(header.cmdline_size) + 1;
    let cmdline_slot = LINUX_CMDLINE_SLOT_END - LINUX_CMDLINE_BASE;
    if cmdline_bytes > cmdline_slot {
        return Err(LinuxPlacementError::CmdlineBufferTooLarge {
            needed: cmdline_bytes,
            slot: cmdline_slot,
        });
    }

    let kernel = checked_range(
        LinuxZone::Kernel,
        header.pref_address,
        u64::from(header.init_size),
    )?;
    if kernel.is_empty() {
        return Err(LinuxPlacementError::EmptyRange {
            zone: LinuxZone::Kernel,
        });
    }
    let required = kernel.end.max(LINUX_LOADER_END);
    if low_end < required {
        return Err(LinuxPlacementError::RamTooSmall {
            low_ram_end: low_end,
            required,
        });
    }

    let initrd = place_initrd(header, input, low_end, kernel)?;

    let acpi = input.acpi;
    let acpi_tables = checked_range(
        LinuxZone::AcpiTables,
        acpi.load_base,
        u64::try_from(acpi.payload_len).map_err(|_| LinuxPlacementError::RangeOverflow {
            zone: LinuxZone::AcpiTables,
        })?,
    )?;
    let acpi_facs = checked_range(
        LinuxZone::AcpiFacs,
        acpi.facs_address,
        u64::from(crate::acpi_blob::ACPI_FACS_BYTES),
    )?;

    let plan = LinuxPlacement {
        kernel,
        initrd,
        boot_params: checked_range(
            LinuxZone::BootParams,
            LINUX_BOOT_PARAMS_BASE,
            LINUX_BOOT_PARAMS_BYTES,
        )?,
        cmdline: checked_range(LinuxZone::Cmdline, LINUX_CMDLINE_BASE, cmdline_bytes)?,
        gdt: checked_range(LinuxZone::Gdt, LINUX_GDT_BASE, LINUX_GDT_BYTES)?,
        page_tables: checked_range(
            LinuxZone::PageTables,
            LINUX_PAGE_TABLES_BASE,
            LINUX_PAGE_TABLES_BYTES,
        )?,
        stack: checked_range(LinuxZone::Stack, LINUX_STACK_BASE, LINUX_STACK_BYTES)?,
        acpi_tables,
        acpi_facs,
    };
    validate_linux_placement(&plan, header, ram_size)?;
    Ok(plan)
}

fn place_initrd(
    header: &LinuxBzImageHeader,
    input: &LinuxPlacementInput<'_>,
    low_end: u64,
    kernel: LinuxRange,
) -> Result<Option<LinuxRange>, LinuxPlacementError> {
    let len = input.initrd_len;
    if len == 0 {
        return Ok(None);
    }
    // `initrd_addr_max` is the last usable byte, so the exclusive limit is one more.
    let limit = align_down(
        low_end.min(u64::from(header.initrd_addr_max) + 1),
        LINUX_PAGE_BYTES,
    );
    let start = match limit.checked_sub(len) {
        Some(raw) => align_down(raw, LINUX_PAGE_BYTES),
        None => {
            return Err(LinuxPlacementError::InitrdTooLarge { len, limit });
        }
    };
    // Round the kernel end up to a page so the initrd never shares a page with the kernel.
    let kernel_end_page = kernel
        .end
        .checked_add(LINUX_PAGE_BYTES - 1)
        .map(|v| align_down(v, LINUX_PAGE_BYTES))
        .ok_or(LinuxPlacementError::RangeOverflow {
            zone: LinuxZone::Kernel,
        })?;
    if start < kernel_end_page {
        return Err(LinuxPlacementError::NoRoomForInitrd {
            len,
            kernel_end: kernel.end,
            limit,
        });
    }
    checked_range(LinuxZone::Initrd, start, len).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::linux_boot::parse_linux_bzimage_header;

    const MIB: u64 = 1 << 20;

    fn put16(b: &mut [u8], off: usize, v: u16) {
        b[off..off + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn put32(b: &mut [u8], off: usize, v: u32) {
        b[off..off + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn put64(b: &mut [u8], off: usize, v: u64) {
        b[off..off + 8].copy_from_slice(&v.to_le_bytes());
    }

    /// Header of a synthetic bzImage shaped like the real one: pref 16 MiB, 2 MiB alignment,
    /// `init_size` 0x100000, `initrd_addr_max` 0x7FFF_FFFF, `cmdline_size` 0x7FF.
    fn synthetic_header() -> LinuxBzImageHeader {
        let payload_offset = 0x1000u32;
        let payload_length = 0x2000u32;
        let pm_code = payload_offset + payload_length;
        let pm = 3 * 512;
        let mut b = vec![0u8; pm + pm_code as usize];
        b[0x1F1] = 2;
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
        parse_linux_bzimage_header(&b).expect("synthetic header")
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

    fn input<'a>(
        acpi: &'a ValidatedAcpiBlob,
        ram_size: u64,
        initrd_len: u64,
    ) -> LinuxPlacementInput<'a> {
        LinuxPlacementInput {
            ram_size,
            initrd_len,
            cmdline_len: 64,
            acpi,
        }
    }

    #[test]
    fn synthetic_plan_is_valid_and_ordered() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 4 * MIB)).expect("plan");
        assert_eq!(
            plan.kernel,
            LinuxRange {
                start: 0x100_0000,
                end: 0x110_0000
            }
        );
        let initrd = plan.initrd.expect("initrd");
        assert_eq!(initrd.start % LINUX_PAGE_BYTES, 0);
        assert_eq!(initrd.len(), 4 * MIB);
        assert_eq!(initrd.end, 128 * MIB);
        assert!(initrd.start >= plan.kernel.end);
        assert_eq!(plan.cmdline.len(), 0x7FF + 1);
        assert_eq!(plan.boot_params.start, 0x10_0000);
        assert!(plan.stack.end <= plan.kernel.start);
        assert_eq!(
            plan.acpi_tables,
            LinuxRange {
                start: 0xE_0000,
                end: 0xE_0400
            }
        );
        assert_eq!(
            plan.acpi_facs,
            LinuxRange {
                start: 0xE_F000,
                end: 0xE_F040
            }
        );
        validate_linux_placement(&plan, &h, 128 * MIB).expect("valid");
    }

    #[test]
    fn no_initrd_gives_none() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).expect("plan");
        assert_eq!(plan.initrd, None);
    }

    #[test]
    fn initrd_start_is_page_aligned_down() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0x0123_4567)).expect("plan");
        let initrd = plan.initrd.expect("initrd");
        assert_eq!(initrd.start % LINUX_PAGE_BYTES, 0);
        assert_eq!(initrd.len(), 0x0123_4567);
        // Even the page-rounded end stays below the limit.
        assert!(initrd.end.div_ceil(LINUX_PAGE_BYTES) * LINUX_PAGE_BYTES <= 128 * MIB);
    }

    #[test]
    fn initrd_stays_below_initrd_addr_max() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        // 4 GiB of RAM: the limit is initrd_addr_max + 1 = 2 GiB, not the end of RAM.
        let plan = plan_linux_placement(&h, &input(&acpi, 4096 * MIB, 8 * MIB)).expect("plan");
        let initrd = plan.initrd.expect("initrd");
        assert_eq!(initrd.end, 0x8000_0000);
        assert!(initrd.end - 1 <= u64::from(h.initrd_addr_max));
        // Lower initrd_addr_max moves the initrd down, never above it.
        let mut low = h;
        low.initrd_addr_max = 0x3FFF_FFFF;
        let plan = plan_linux_placement(&low, &input(&acpi, 4096 * MIB, 8 * MIB)).expect("plan");
        assert_eq!(plan.initrd.expect("initrd").end, 0x4000_0000);
    }

    #[test]
    fn initrd_stays_below_the_ram_hole() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        // 8 GiB of RAM and no initrd_addr_max restriction: RAM above 4 GiB is not contiguous
        // with low RAM, so the initrd tops out at the start of the hole.
        h.initrd_addr_max = 0xFFFF_FFFF;
        let plan = plan_linux_placement(&h, &input(&acpi, 8192 * MIB, 8 * MIB)).expect("plan");
        let initrd = plan.initrd.expect("initrd");
        assert_eq!(initrd.end, 0xF400_0000);
        validate_linux_placement(&plan, &h, 8192 * MIB).expect("valid");
        // The same holds for exactly 4 GiB.
        let plan = plan_linux_placement(&h, &input(&acpi, 4096 * MIB, 8 * MIB)).expect("plan");
        assert_eq!(plan.initrd.expect("initrd").end, 0xF400_0000);
    }

    #[test]
    fn kernel_crossing_the_ram_hole_does_not_fit() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        // Kernel [0xF300_0000, 0xF500_0000) crosses the start of the hole; it is not covered by
        // low RAM, so the planner reports that low RAM is too small (not OutsideRam: no plan
        // exists yet).
        h.pref_address = 0xF300_0000;
        h.init_size = 0x200_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 8192 * MIB, 0)),
            Err(LinuxPlacementError::RamTooSmall {
                low_ram_end: 0xF400_0000,
                required: 0xF500_0000,
            })
        );
        // Kernel entirely in the hole.
        h.pref_address = 0xF400_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 8192 * MIB, 0)),
            Err(LinuxPlacementError::RamTooSmall {
                low_ram_end: 0xF400_0000,
                required: 0xF600_0000,
            })
        );
        // Kernel at the start of high RAM: also unusable.
        h.pref_address = 0x1_0000_0000;
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 8192 * MIB, 0)),
            Err(LinuxPlacementError::RamTooSmall { .. })
        ));
    }

    #[test]
    fn injected_zone_in_the_ram_hole_is_outside_ram() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 8192 * MIB, 0)).expect("plan");
        validate_linux_placement(&plan, &h, 8192 * MIB).expect("valid before injection");
        plan.kernel = LinuxRange {
            start: 0xF300_0000,
            end: 0xF500_0000,
        };
        assert_eq!(
            validate_linux_placement(&plan, &h, 8192 * MIB),
            Err(LinuxPlacementError::OutsideRam {
                zone: LinuxZone::Kernel,
                end: 0xF500_0000,
                low_ram_end: 0xF400_0000,
            })
        );
    }

    #[test]
    fn initrd_cannot_fit_below_initrd_addr_max_above_kernel() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        // Limit 32 MiB, kernel ends at 17 MiB: a 20 MiB initrd would start at 12 MiB.
        h.initrd_addr_max = 0x01FF_FFFF;
        match plan_linux_placement(&h, &input(&acpi, 4096 * MIB, 20 * MIB)) {
            Err(LinuxPlacementError::NoRoomForInitrd {
                len,
                kernel_end,
                limit,
            }) => {
                assert_eq!(len, 20 * MIB);
                assert_eq!(kernel_end, 0x110_0000);
                assert_eq!(limit, 0x200_0000);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn initrd_larger_than_limit_is_too_large() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        match plan_linux_placement(&h, &input(&acpi, 128 * MIB, 129 * MIB)) {
            Err(LinuxPlacementError::InitrdTooLarge { len, limit }) => {
                assert_eq!(len, 129 * MIB);
                assert_eq!(limit, 128 * MIB);
            }
            other => panic!("{other:?}"),
        }
        // Absurd lengths must not wrap.
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, u64::MAX)),
            Err(LinuxPlacementError::InitrdTooLarge { .. })
        ));
    }

    #[test]
    fn small_ram_for_big_init_size() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.init_size = 0x700_0000;
        // Kernel [16 MiB, 128 MiB) fits exactly, so only the initrd has no room.
        assert!(plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).is_ok());
        match plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0x1000)) {
            Err(LinuxPlacementError::NoRoomForInitrd {
                kernel_end, limit, ..
            }) => {
                assert_eq!(kernel_end, 128 * MIB);
                assert_eq!(limit, 128 * MIB);
            }
            other => panic!("{other:?}"),
        }
        // One more page of init_size no longer fits in RAM at all.
        h.init_size = 0x700_1000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::RamTooSmall {
                low_ram_end: 128 * MIB,
                required: 0x800_1000,
            })
        );
    }

    #[test]
    fn ram_smaller_than_loader_area() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.pref_address = 0x20_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 0x14_0000, 0)),
            Err(LinuxPlacementError::RamTooSmall {
                low_ram_end: 0x14_0000,
                required: 0x30_0000,
            })
        );
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 0, 0)),
            Err(LinuxPlacementError::RamTooSmall {
                low_ram_end: 0,
                required: 0x30_0000,
            })
        );
    }

    #[test]
    fn kernel_range_overflow() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.pref_address = u64::MAX - 0xFFF;
        h.init_size = 0x2000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::RangeOverflow {
                zone: LinuxZone::Kernel
            })
        );
        // Huge but non-overflowing kernel: not in RAM.
        h.pref_address = u64::MAX - 0x1_0000;
        h.init_size = 0x1000;
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::RamTooSmall { .. })
        ));
    }

    #[test]
    fn acpi_range_overflow() {
        let h = synthetic_header();
        let mut acpi = acpi_blob();
        acpi.load_base = u64::MAX - 1;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::RangeOverflow {
                zone: LinuxZone::AcpiTables
            })
        );
    }

    #[test]
    fn empty_init_size_is_rejected() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.init_size = 0;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::EmptyRange {
                zone: LinuxZone::Kernel
            })
        );
    }

    #[test]
    fn cmdline_length_limit() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut inp = input(&acpi, 128 * MIB, 0);
        // Exactly cmdline_size characters fit (the NUL is the extra byte).
        inp.cmdline_len = h.cmdline_size;
        let plan = plan_linux_placement(&h, &inp).expect("max cmdline");
        assert_eq!(plan.cmdline.len(), u64::from(h.cmdline_size) + 1);
        inp.cmdline_len = h.cmdline_size + 1;
        assert_eq!(
            plan_linux_placement(&h, &inp),
            Err(LinuxPlacementError::CmdlineTooLong {
                len: 0x800,
                max: 0x7FF
            })
        );
        inp.cmdline_len = u32::MAX;
        assert!(matches!(
            plan_linux_placement(&h, &inp),
            Err(LinuxPlacementError::CmdlineTooLong { .. })
        ));
    }

    #[test]
    fn cmdline_buffer_must_fit_its_slot() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.cmdline_size = u32::MAX;
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::CmdlineBufferTooLarge { .. })
        ));
        h.cmdline_size = 0x1_F000;
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::CmdlineBufferTooLarge { .. })
        ));
        h.cmdline_size = 0x1_EFFF;
        assert!(plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).is_ok());
    }

    #[test]
    fn kernel_overlapping_loader_area() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.pref_address = 0x10_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::Overlap {
                a: LinuxZone::Kernel,
                b: LinuxZone::BootParams
            })
        );
        h.pref_address = 0x13_8000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::Overlap {
                a: LinuxZone::Kernel,
                b: LinuxZone::PageTables
            })
        );
        // Right after the loader area is fine (with an alignment that allows that address).
        h.kernel_alignment = 0x1000;
        h.pref_address = LINUX_LOADER_END;
        assert!(plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).is_ok());
    }

    #[test]
    fn kernel_in_forbidden_zones() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        h.init_size = 0x1000;
        for pref in [0x1000u64, 0x9_E000, 0xE_0000, 0xF_F000] {
            h.pref_address = pref;
            match plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)) {
                Err(LinuxPlacementError::ForbiddenZone { zone, range, .. }) => {
                    assert_eq!(zone, LinuxZone::Kernel);
                    assert_eq!(range.start, pref);
                }
                other => panic!("pref {pref:#x}: {other:?}"),
            }
        }
        // Straddling the start of the high zone.
        h.pref_address = 0xD_F000;
        h.init_size = 0x2000;
        assert!(matches!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::ForbiddenZone { .. })
        ));
        // Between the two zones the zone rules allow it, but it is below the loader area.
        h.pref_address = 0x9_F000;
        h.init_size = 0x1000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::KernelBelowLoaderArea {
                start: 0x9_F000,
                loader_end: LINUX_LOADER_END,
            })
        );
    }

    #[test]
    fn kernel_below_loader_area_is_rejected() {
        let mut h = synthetic_header();
        let acpi = acpi_blob();
        // A gap kernel [0xA_0000, 0xB_0000) touches no zone but sits below the loader area.
        h.pref_address = 0xA_0000;
        h.init_size = 0x1_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)),
            Err(LinuxPlacementError::KernelBelowLoaderArea {
                start: 0xA_0000,
                loader_end: 0x15_0000,
            })
        );
        // Just at the end of the loader area it is accepted (with a 4 KiB kernel alignment).
        h.kernel_alignment = 0x1000;
        h.pref_address = 0x15_0000;
        h.init_size = 0x1000;
        assert!(plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).is_ok());
    }

    // The planner never puts the initrd below the kernel (which is itself at or above the end of
    // the loader area), so the next two cases are reachable only through a modified plan.
    #[test]
    fn injected_initrd_in_forbidden_zone() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0x1000)).expect("plan");
        plan.initrd = Some(LinuxRange {
            start: 0xF_F000,
            end: 0x10_0000,
        });
        assert_eq!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::ForbiddenZone {
                zone: LinuxZone::Initrd,
                range: LinuxRange {
                    start: 0xF_F000,
                    end: 0x10_0000
                },
                forbidden: LinuxRange {
                    start: 0xE_0000,
                    end: 0x10_0000
                },
            })
        );
    }

    #[test]
    fn injected_initrd_overlapping_loader_area() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0x1000)).expect("plan");
        plan.initrd = Some(LinuxRange {
            start: 0x14_F000,
            end: 0x15_0000,
        });
        assert_eq!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::Overlap {
                a: LinuxZone::Initrd,
                b: LinuxZone::Stack
            })
        );
    }

    #[test]
    fn acpi_overlapping_loader_area_is_reported() {
        let h = synthetic_header();
        // ACPI blob moved on top of the loader area (the FACS lands in the stack).
        let mut moved = acpi_blob();
        moved.facs_address = 0x14_0000;
        assert_eq!(
            plan_linux_placement(&h, &input(&moved, 128 * MIB, 0)),
            Err(LinuxPlacementError::Overlap {
                a: LinuxZone::Stack,
                b: LinuxZone::AcpiFacs
            })
        );
    }

    #[test]
    fn injected_initrd_on_kernel_is_overlap() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 4 * MIB)).expect("plan");
        validate_linux_placement(&plan, &h, 128 * MIB).expect("valid before injection");
        let len = plan.initrd.expect("initrd").len();
        plan.initrd = Some(LinuxRange {
            start: plan.kernel.start,
            end: plan.kernel.start + len,
        });
        assert_eq!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::Overlap {
                a: LinuxZone::Kernel,
                b: LinuxZone::Initrd
            })
        );
        // One byte of overlap at the kernel's last byte is enough.
        plan.initrd = Some(LinuxRange {
            start: plan.kernel.end - 1,
            end: plan.kernel.end - 1 + len,
        });
        assert!(matches!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::Overlap { .. })
        ));
        // Adjacent ranges do not overlap.
        plan.initrd = Some(LinuxRange {
            start: plan.kernel.end,
            end: plan.kernel.end + len,
        });
        assert_eq!(validate_linux_placement(&plan, &h, 128 * MIB), Ok(()));
    }

    #[test]
    fn injected_initrd_above_initrd_addr_max_is_rejected() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 4096 * MIB, 8 * MIB)).expect("plan");
        validate_linux_placement(&plan, &h, 4096 * MIB).expect("valid before injection");
        // Inside RAM and disjoint from everything, but above initrd_addr_max + 1 = 2 GiB.
        plan.initrd = Some(LinuxRange {
            start: 0x9000_0000,
            end: 0x9000_0000 + 8 * MIB,
        });
        assert_eq!(
            validate_linux_placement(&plan, &h, 4096 * MIB),
            Err(LinuxPlacementError::InitrdAboveLimit {
                end: 0x9000_0000 + 8 * MIB,
                limit: 0x8000_0000,
            })
        );
        // One byte above the limit is enough.
        plan.initrd = Some(LinuxRange {
            start: 0x8000_0000 - 8 * MIB + 1,
            end: 0x8000_0001,
        });
        assert!(matches!(
            validate_linux_placement(&plan, &h, 4096 * MIB),
            Err(LinuxPlacementError::InitrdAboveLimit { .. })
        ));
    }

    #[test]
    fn injected_initrd_below_kernel_is_rejected() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 4 * MIB)).expect("plan");
        // Above the loader area and disjoint from everything, but below the kernel.
        plan.initrd = Some(LinuxRange {
            start: 0x0018_0000,
            end: 0x0019_0000,
        });
        assert!(matches!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::InitrdBelowKernel {
                start: 0x0018_0000,
                ..
            })
        ));
    }

    #[test]
    fn injected_misaligned_initrd_and_kernel_are_rejected() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 4 * MIB)).expect("plan");
        let initrd = plan.initrd.expect("initrd");
        plan.initrd = Some(LinuxRange {
            start: initrd.start - 1,
            end: initrd.end - 1,
        });
        assert_eq!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::InitrdNotPageAligned {
                start: initrd.start - 1
            })
        );
        plan.initrd = Some(initrd);
        plan.kernel = LinuxRange {
            start: 0x100_1000,
            end: 0x110_1000,
        };
        assert_eq!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::KernelMisaligned {
                start: 0x100_1000,
                alignment: 0x20_0000
            })
        );
    }

    #[test]
    fn injected_zone_beyond_ram_is_outside_ram() {
        let h = synthetic_header();
        let acpi = acpi_blob();
        let mut plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, 0)).expect("plan");
        plan.stack.end = 129 * MIB;
        assert!(matches!(
            validate_linux_placement(&plan, &h, 128 * MIB),
            Err(LinuxPlacementError::OutsideRam {
                zone: LinuxZone::Stack,
                ..
            })
        ));
    }

    const REAL_VMLINUZ: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic"
    );

    /// Ignored by default because `work/` is not part of the repository. It must be run
    /// explicitly (`--ignored`) and fails, instead of passing vacuously, if the file is absent.
    #[test]
    #[ignore = "needs work/stage4/linux-7.0.0-38.38/vmlinuz-7.0.0-38-generic; run with --ignored"]
    fn real_vmlinuz_golden_placement() {
        let file = std::fs::read(REAL_VMLINUZ)
            .unwrap_or_else(|e| panic!("vmlinuz absent, test NOT passed: {REAL_VMLINUZ}: {e}"));
        let h = parse_linux_bzimage_header(&file).expect("real vmlinuz header");
        let acpi = acpi_blob();
        let initrd_len = 0x0200_0001u64; // ~32 MiB, deliberately not page-sized

        // 128 MiB: kernel [0x100_0000, 0x54C_1000), initrd at the very top.
        let plan = plan_linux_placement(&h, &input(&acpi, 128 * MIB, initrd_len)).expect("128 MiB");
        assert_eq!(
            plan.kernel,
            LinuxRange {
                start: 0x100_0000,
                end: 0x54C_1000
            }
        );
        assert_eq!(
            plan.initrd,
            Some(LinuxRange {
                start: 0x5FF_F000,
                end: 0x5FF_F000 + initrd_len
            })
        );
        assert_eq!(
            plan.cmdline,
            LinuxRange {
                start: 0x10_1000,
                end: 0x10_1000 + 0x800
            }
        );
        assert_eq!(
            plan.boot_params,
            LinuxRange {
                start: 0x10_0000,
                end: 0x10_1000
            }
        );
        validate_linux_placement(&plan, &h, 128 * MIB).expect("valid");

        // 1 GiB: same kernel, initrd under the top of RAM.
        let plan = plan_linux_placement(&h, &input(&acpi, 1024 * MIB, initrd_len)).expect("1 GiB");
        assert_eq!(
            plan.kernel,
            LinuxRange {
                start: 0x100_0000,
                end: 0x54C_1000
            }
        );
        assert_eq!(
            plan.initrd,
            Some(LinuxRange {
                start: 0x3DFF_F000,
                end: 0x3FFF_F001
            })
        );
        validate_linux_placement(&plan, &h, 1024 * MIB).expect("valid");

        // 4 GiB: the initrd is capped by initrd_addr_max + 1 = 2 GiB.
        let plan = plan_linux_placement(&h, &input(&acpi, 4096 * MIB, initrd_len)).expect("4 GiB");
        assert_eq!(
            plan.initrd,
            Some(LinuxRange {
                start: 0x7DFF_F000,
                end: 0x7FFF_F001
            })
        );
        validate_linux_placement(&plan, &h, 4096 * MIB).expect("valid");
    }
}
