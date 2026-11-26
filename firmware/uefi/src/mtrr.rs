//! S7-32 slice I: the MTRR layout the firmware programs at boot (Intel SDM Vol. 3A, chapter
//! "Memory Cache Control", section "Memory Type Range Registers (MTRRs)").
//!
//! After a reset the MTRRs are disabled (IA32_MTRR_DEF_TYPE.E = 0, every PHYSMASKn.V = 0): all of
//! physical memory is UC until the platform firmware enables them. On a CPU that declares MTRR
//! (CPUID.01H:EDX[12]) the firmware programs:
//!
//! - IA32_MTRR_DEF_TYPE = E | WB (`0x806`): the default type is WB, the fixed-range MTRRs stay
//!   disabled (FE = 0). Below 1 MiB this board has only RAM (no legacy VGA window, no option-ROM
//!   shadows, the ACPI tables live in RAM), so fixed ranges would all be WB, the default.
//! - UC for the device window below 4 GiB: from the lowest handoff region that is not RAM-backed
//!   (MMIO, ROM, NVRAM, framebuffer, reserved) up to 4 GiB, as naturally aligned power-of-two
//!   blocks, one variable pair each. The board puts every device into
//!   `[0xF400_0000, 0x1_0000_0000)` and RAM below 4 GiB never above `0xF400_0000`
//!   (FIRMWARE_BOOT.md section 2); the plan takes the bounds from the handoff and refuses a map
//!   in which RAM lies inside the window or a device lies above 4 GiB.
//!
//! The pure planning lives here; the MSR, CR and cache instructions that apply it are in the
//! binary (`main.rs`).

use crate::handoff::{Handoff, region_kind::*};

/// IA32_MTRRCAP (SDM Vol. 4 Table 2-2, FEH): VCNT in bits 7:0.
pub const IA32_MTRRCAP: u32 = 0xfe;
/// IA32_MTRR_DEF_TYPE (2FFH).
pub const IA32_MTRR_DEF_TYPE: u32 = 0x2ff;
/// IA32_MTRR_PHYSBASE0 (200H): pair `n` is PHYSBASEn `0x200 + 2n`, PHYSMASKn `0x201 + 2n`.
pub const IA32_MTRR_PHYSBASE0: u32 = 0x200;
/// CPUID.01H:EDX bit 12 (MTRR).
pub const CPUID_LEAF1_EDX_MTRR: u32 = 1 << 12;
/// IA32_MTRR_DEF_TYPE.E (bit 11): MTRRs enabled.
pub const DEF_TYPE_ENABLE: u64 = 1 << 11;

const FOUR_GIB: u64 = 1 << 32;
const PAGE: u64 = 0x1000;
const TYPE_UC: u64 = 0;
const TYPE_WB: u64 = 6;
const PHYSMASK_VALID: u64 = 1 << 11;
/// The most blocks a page-aligned window ending at 4 GiB can need: one per address bit 12..=31.
const MAX_RANGES: usize = 20;

/// Why the firmware cannot give the board a correct MTRR layout; the boot stops with it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MtrrPlanError {
    /// MAXPHYADDR from CPUID is outside 36..=52.
    PhysicalAddressWidth(u8),
    /// A RAM-backed region at `base` lies inside the UC window below 4 GiB.
    RamInUncachedWindow { base: u64 },
    /// A device region at `base` ends above 4 GiB, outside the UC window.
    DeviceAboveFourGiB { base: u64 },
    /// The window needs `needed` variable pairs and the CPU has `available`.
    TooManyRanges { needed: u8, available: u8 },
}

impl MtrrPlanError {
    /// The `detail` of the stop line (`OD-FW STOP code=0x36`).
    pub fn stop_detail(self) -> u64 {
        match self {
            Self::PhysicalAddressWidth(bits) => 1 | (u64::from(bits) << 32),
            Self::RamInUncachedWindow { base } => 2 | (base << 8),
            Self::DeviceAboveFourGiB { base } => 3 | (base << 8),
            Self::TooManyRanges { needed, available } => {
                4 | (u64::from(needed) << 32) | (u64::from(available) << 40)
            }
        }
    }
}

/// The values the firmware writes: IA32_MTRR_DEF_TYPE and the variable pairs in use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MtrrPlan {
    def_type: u64,
    /// `(PHYSBASEn, PHYSMASKn)`, in address order.
    pairs: [(u64, u64); MAX_RANGES],
    count: usize,
}

impl MtrrPlan {
    /// IA32_MTRR_DEF_TYPE: E | WB, FE = 0.
    pub const fn def_type(&self) -> u64 {
        self.def_type
    }

    /// The `(PHYSBASEn, PHYSMASKn)` values of pairs `0..count`; the other pairs are written with
    /// PHYSMASK = 0 (V = 0).
    pub fn pairs(&self) -> &[(u64, u64)] {
        &self.pairs[..self.count]
    }
}

/// The plan for the handoff's region table, a CPU with `variable_count` variable pairs
/// (IA32_MTRRCAP.VCNT) and a MAXPHYADDR of `physical_bits` (CPUID.80000008H:EAX[7:0]).
pub fn plan(
    handoff: &Handoff<'_>,
    variable_count: u8,
    physical_bits: u8,
) -> Result<MtrrPlan, MtrrPlanError> {
    plan_regions(
        || {
            handoff
                .regions()
                .map(|region| (region.kind, region.base, region.length))
        },
        variable_count,
        physical_bits,
    )
}

/// Region kinds backed by RAM (FIRMWARE_BOOT.md 6.2: `RAM` and its sub-ranges 7..=12). Every other
/// kind (2 `RESERVED`, 3 `MMIO`, 4 `ROM`, 5 `NVRAM`, 6 `FRAMEBUFFER`) is a device or an unbacked
/// range and must be UC.
const fn ram_backed(kind: u32) -> bool {
    matches!(
        kind,
        RAM | HANDOFF | BOOT_STACK | FIRMWARE_SHADOW | BOOT_PAGE_TABLES | ACPI_TABLES | ACPI_NVS
    )
}

/// [`plan`] over the `(kind, base, length)` records `regions()` yields (the handoff guarantees
/// 4 KiB alignment, non-zero lengths and no overlap).
fn plan_regions<I: Iterator<Item = (u32, u64, u64)>>(
    regions: impl Fn() -> I,
    variable_count: u8,
    physical_bits: u8,
) -> Result<MtrrPlan, MtrrPlanError> {
    if !(36..=52).contains(&physical_bits) {
        return Err(MtrrPlanError::PhysicalAddressWidth(physical_bits));
    }
    let mut window = FOUR_GIB;
    for (kind, base, length) in regions() {
        if ram_backed(kind) {
            continue;
        }
        if base + length > FOUR_GIB {
            return Err(MtrrPlanError::DeviceAboveFourGiB { base });
        }
        window = window.min(base);
    }
    for (kind, base, length) in regions() {
        if ram_backed(kind) && base < FOUR_GIB && base + length > window {
            return Err(MtrrPlanError::RamInUncachedWindow { base });
        }
    }
    let address_mask = ((1u64 << physical_bits) - 1) & !(PAGE - 1);
    let mut plan = MtrrPlan {
        def_type: DEF_TYPE_ENABLE | TYPE_WB,
        pairs: [(0, 0); MAX_RANGES],
        count: 0,
    };
    let mut base = window;
    while base < FOUR_GIB {
        // The largest naturally aligned block at `base` that does not pass 4 GiB.
        let mut size = if base == 0 {
            FOUR_GIB
        } else {
            base & base.wrapping_neg()
        };
        while base + size > FOUR_GIB {
            size >>= 1;
        }
        plan.pairs[plan.count] = (
            base | TYPE_UC,
            (!(size - 1) & address_mask) | PHYSMASK_VALID,
        );
        plan.count += 1;
        base += size;
    }
    if plan.count > usize::from(variable_count) {
        return Err(MtrrPlanError::TooManyRanges {
            needed: plan.count as u8,
            available: variable_count,
        });
    }
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1 << 20;

    /// The region table of the Windows run's board (4096 MiB of RAM, one screen, NVMe and xHCI on
    /// PCI, TPM, NVRAM; FIRMWARE_BOOT.md section 2): low RAM up to `0xF400_0000`, the devices from
    /// `0xF800_0000`, high RAM from 4 GiB.
    fn board_4096_mib() -> Vec<(u32, u64, u64)> {
        vec![
            (RAM, 0, 0x10_0000),
            (HANDOFF, 0x10_0000, 0x1_0000),
            (BOOT_STACK, 0x11_0000, 0x4_0000),
            (RAM, 0x15_0000, 0xb_0000),
            (FIRMWARE_SHADOW, 0x20_0000, 0x40_0000),
            (BOOT_PAGE_TABLES, 0x60_0000, 0x81_f000),
            (RAM, 0xe1_f000, 0xF400_0000 - 0xe1_f000),
            (FRAMEBUFFER, 0xF800_0000, 36 * MIB),
            (MMIO, 0xFC00_0000, 0x1000),
            (MMIO, 0xFC00_4000, 0x4000),
            (MMIO, 0xFC10_0000, 0x10_0000),
            (MMIO, 0xFEC0_0000, 0x1000),
            (MMIO, 0xFED4_0000, 0x5000),
            (MMIO, 0xFEE0_0000, 0x1000),
            (NVRAM, 0xFF00_0000, 0x4_1000),
            (ROM, 0xFFC0_0000, 0x40_0000),
            (RAM, FOUR_GIB, 192 * MIB),
        ]
    }

    /// Expected values by hand from the SDM layout of the MSRs: PHYSBASE = base | type (UC = 0),
    /// PHYSMASK = the address bits MAXPHYADDR-1:12 that must match | V (bit 11); a block of size S
    /// at base B matches every address A with (A & mask) == (B & mask), i.e. [B, B + S).
    #[test]
    fn the_windows_board_gets_wb_by_default_and_one_uc_pair_for_the_device_window() {
        let plan = plan_regions(|| board_4096_mib().into_iter(), 8, 36).unwrap();
        assert_eq!(plan.def_type(), 0x806, "E | WB, FE = 0");
        // [0xF800_0000, 0x1_0000_0000): 128 MiB at a 128 MiB-aligned base.
        assert_eq!(plan.pairs(), &[(0xF800_0000, 0xF_F800_0800)]);
    }

    #[test]
    fn two_screens_need_two_pairs() {
        let mut regions = board_4096_mib();
        regions.insert(7, (FRAMEBUFFER, 0xF400_0000, 36 * MIB));
        let plan = plan_regions(|| regions.iter().copied(), 8, 36).unwrap();
        // [0xF400_0000, 0xF800_0000) is 64 MiB, then the 128 MiB block up to 4 GiB.
        assert_eq!(
            plan.pairs(),
            &[(0xF400_0000, 0xF_FC00_0800), (0xF800_0000, 0xF_F800_0800)]
        );
    }

    #[test]
    fn a_small_ram_leaves_the_gap_below_the_devices_write_back() {
        // 128 MiB of RAM: [0x0800_0000, 0xF800_0000) is backed by nothing and stays WB (no access
        // reaches it); only the device window is UC.
        let regions = vec![
            (RAM, 0, 0x0800_0000),
            (FRAMEBUFFER, 0xF800_0000, 36 * MIB),
            (ROM, 0xFFC0_0000, 0x40_0000),
        ];
        let plan = plan_regions(|| regions.iter().copied(), 8, 36).unwrap();
        assert_eq!(plan.pairs(), &[(0xF800_0000, 0xF_F800_0800)]);
    }

    #[test]
    fn the_mask_follows_maxphyaddr() {
        let plan = plan_regions(|| board_4096_mib().into_iter(), 8, 39).unwrap();
        assert_eq!(plan.pairs(), &[(0xF800_0000, 0x7F_F800_0800)]);
        for bits in [0, 32, 35, 53, 64] {
            assert_eq!(
                plan_regions(|| board_4096_mib().into_iter(), 8, bits),
                Err(MtrrPlanError::PhysicalAddressWidth(bits))
            );
        }
    }

    #[test]
    fn an_unaligned_window_is_split_into_aligned_blocks() {
        // A device at 0xFE00_1000: blocks of 4 KiB, 8 KiB, ... up to 16 MiB at 0xFF00_0000.
        let regions = vec![(RAM, 0, 0x8000_0000), (MMIO, 0xFE00_1000, 0x1000)];
        let plan = plan_regions(|| regions.iter().copied(), 20, 36).unwrap();
        let mut covered = 0xFE00_1000u64;
        for &(physbase, physmask) in plan.pairs() {
            assert_eq!(physbase & 0xfff, 0, "type UC, reserved bits clear");
            assert_eq!(physbase, covered);
            assert_ne!(physmask & (1 << 11), 0, "valid");
            let size = (!(physmask & 0xF_FFFF_F000) & 0xF_FFFF_FFFF) + 1;
            assert_eq!(physbase % size, 0, "naturally aligned");
            covered += size;
        }
        assert_eq!(covered, FOUR_GIB);
        assert_eq!(plan.pairs().len(), 13);
        assert_eq!(
            plan_regions(|| regions.iter().copied(), 8, 36),
            Err(MtrrPlanError::TooManyRanges {
                needed: 13,
                available: 8
            })
        );
    }

    #[test]
    fn ram_inside_the_window_and_devices_above_four_gib_are_refused() {
        let mut regions = board_4096_mib();
        regions.push((RAM, 0xFA40_0000, 0x1000));
        regions.sort_by_key(|region| region.1);
        assert_eq!(
            plan_regions(|| regions.iter().copied(), 8, 36),
            Err(MtrrPlanError::RamInUncachedWindow { base: 0xFA40_0000 })
        );
        let mut regions = board_4096_mib();
        regions.push((MMIO, 0x2_0000_0000, 0x1000));
        assert_eq!(
            plan_regions(|| regions.iter().copied(), 8, 36),
            Err(MtrrPlanError::DeviceAboveFourGiB {
                base: 0x2_0000_0000
            })
        );
    }

    #[test]
    fn without_devices_below_four_gib_every_pair_stays_unused() {
        let plan = plan_regions(|| [(RAM, 0, 0x8000_0000)].into_iter(), 8, 36).unwrap();
        assert_eq!(plan.def_type(), 0x806);
        assert!(plan.pairs().is_empty());
        // No variable pair at all is enough then.
        assert!(plan_regions(|| [(RAM, 0, 0x8000_0000)].into_iter(), 0, 36).is_ok());
    }

    #[test]
    fn stop_details_name_the_cause() {
        assert_eq!(
            MtrrPlanError::PhysicalAddressWidth(32).stop_detail(),
            0x20_0000_0001
        );
        assert_eq!(
            MtrrPlanError::RamInUncachedWindow { base: 0xFA40_0000 }.stop_detail(),
            0xFA_4000_0002
        );
        assert_eq!(
            MtrrPlanError::TooManyRanges {
                needed: 13,
                available: 8
            }
            .stop_detail(),
            0x080D_0000_0004
        );
    }
}
