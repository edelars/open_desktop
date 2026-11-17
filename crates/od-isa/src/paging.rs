//! IA-32e (4-level) page-table walk for the AP trampoline path (S6-14m).
//!
//! Scope: linear addresses below 4 GiB (compatibility mode and the identity subset of the
//! trampoline), `CR3` -> PML4 -> PDPT -> PD -> PT (4 KiB pages) or PD (2 MiB pages, `PS = 1`).
//! 1 GiB pages are not declared by the `OdX8664V1Neutral` CPUID profile (`0x80000001:EDX` bit 26
//! is clear) and refused ([`PageRefusal::GigabytePage`]). Memory is the [`RealModeMemory`] of the
//! other slices, addressed with a 32-bit physical address: an entry or a page above 4 GiB is
//! refused ([`PageRefusal::PhysicalBeyond4GiB`]). There is no TLB and no `INVLPG`: every access
//! walks the tables, so a `MOV CR3` and any table write take effect at once.
//!
//! A `#PF` is not delivered: a walk that would fault is an explicit refusal with the reason and
//! the linear address (the hardware loads `CR2` with it; the KVM oracle of S6-14m reports the same
//! address). The privilege level is always 0 (supervisor): `U/S` is not looked at, and there is no
//! `SMEP`/`SMAP` (`CR4` bits above 10 are never set by the model).
//!
//! Rules, from the Intel SDM Vol. 3A chapter 4 (by memory; every behaviour the model relies on is
//! also an oracle image in `docs/reports/data/stage6/s6-14m/`):
//! - an entry with `P = 0` is not present (any level);
//! - reserved bits (`#PF` with `RSVD`): in a PML4E, bit 7 (`PS`); in a PDE with `PS = 1`, bits 20:13;
//!   in every entry, bits 51:`MAXPHYADDR` (36 for the profile, `0x80000008:EAX` = `0x3024`) and bit
//!   63 (`XD`) when `EFER.NXE = 0`; bits 62:52 are ignored (no protection keys);
//! - supervisor write: a clear `R/W` at any level refuses the access when `CR0.WP = 1`, and is
//!   ignored when `WP = 0`; an instruction fetch is refused when `EFER.NXE = 1` and `XD = 1` at any
//!   level;
//! - **accessed / dirty (decision of S6-14m):** a walk that succeeds sets `A` (bit 5) in every
//!   entry it used (PML4E, PDPTE, PDE and, for a 4 KiB page, PTE) and, for a write, `D` (bit 6) in
//!   the final entry (PDE for a 2 MiB page, PTE for a 4 KiB page). The bits are returned as
//!   *touches* (entry address, OR mask) and written by the caller only after the whole instruction
//!   is known to succeed. A walk that ends in a `#PF` sets `A` in none of its entries: the
//!   model, like the oracle (images `k`, `m`..`q`, `s`), leaves the entries of a refused walk
//!   untouched (contract S6-14m, decision 4; walks that completed earlier in the same instruction
//!   keep their bits, see `ProtectedCpu::step`). A
//!   touch needs the low byte of the entry writable in memory, otherwise the walk is refused with
//!   [`PageRefusal::EntryUnwritable`]. Entries are 8 bytes, little-endian; `A` and `D` live in the
//!   low byte, so one byte is OR-ed (S6-25: `RealModeMemory::or_u8`).

use crate::real_mode_exec::RealModeMemory;

/// `MAXPHYADDR` of the profile (`CPUID.80000008H:EAX[7:0]`).
pub const MAXPHYADDR_BITS: u32 = 36;

const P: u64 = 1;
const RW: u64 = 1 << 1;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;
const XD: u64 = 1 << 63;
/// Physical-address bits 51:12 of a table pointer or a 4 KiB page entry.
const ADDR_4K: u64 = 0x000F_FFFF_FFFF_F000;
/// Physical-address bits 51:21 of a 2 MiB page entry.
const ADDR_2M: u64 = 0x000F_FFFF_FFE0_0000;
/// Bits 51:`MAXPHYADDR` (reserved in every entry).
const BEYOND_MAXPHYADDR: u64 = 0x000F_FFFF_FFFF_FFFF & !((1u64 << MAXPHYADDR_BITS) - 1);
/// Bits 20:13 of a 2 MiB PDE (must be zero: the page is 2 MiB aligned; bit 12 is PAT).
const PDE_2M_RESERVED: u64 = 0x1F_E000;

/// Kind of memory access that walks the tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessKind {
    Read,
    Write,
    Fetch,
}

/// Paging control state of the walk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagingParams {
    /// `CR3`: the PML4 is at `cr3 & 0xFFFFF000`.
    pub cr3: u32,
    /// `CR0.WP`.
    pub wp: bool,
    /// `EFER.NXE`.
    pub nxe: bool,
}

/// Why a walk was refused (the `#PF` or a model limit; the state and memory are unchanged).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageRefusal {
    /// The entry of `level` (4 = PML4E, 3 = PDPTE, 2 = PDE, 1 = PTE) has `P = 0`.
    NotPresent { level: u8, entry: u64 },
    /// A reserved bit of the entry of `level` is set (`#PF` with `RSVD`).
    ReservedBits { level: u8, entry: u64 },
    /// PDPTE with `PS = 1` (1 GiB page): not declared by the CPUID profile (in hardware without it
    /// the bit is reserved, also a `#PF` with `RSVD`).
    GigabytePage { entry: u64 },
    /// A write with `CR0.WP = 1` through an entry with `R/W = 0`.
    WriteProtected { entry: u64 },
    /// An instruction fetch through an entry with `XD = 1` and `EFER.NXE = 1`.
    NoExecute { entry: u64 },
    /// The 8 bytes of the entry at `phys` are not all readable in memory.
    EntryUnreadable { phys: u32 },
    /// The accessed/dirty update needs the entry byte at `phys` writable, and it is not.
    EntryUnwritable { phys: u32 },
    /// The entry or the page lies at `phys` >= 4 GiB (memory is addressed with 32 bits).
    PhysicalBeyond4GiB { phys: u64 },
}

/// Page size of a successful translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageSize {
    Size4K,
    Size2M,
}

/// A successful walk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Translation {
    /// Physical address of the byte.
    pub phys: u32,
    pub size: PageSize,
    /// Accessed/dirty updates still to be written: (address of the low byte of the entry, bits to
    /// OR into it). Empty when every `A` (and, for a write, `D`) bit is already set. Every address
    /// has been checked writable.
    pub touches: Vec<(u32, u8)>,
}

// S6-25 deviation (AP path): the entry is read byte by byte (eight `read_u8`), not as one 8-byte
// access; correct in interleaving mode (the instruction is indivisible). S6-31 left this as it is:
// the CAS on the value the walk read needs a trait change, recorded for the architecture review
// (see `RealModeMemory::or_u8` and `MACHINE_CONTRACT.md`, S6-31, "Путь AP").
fn read_entry<M: RealModeMemory>(mem: &M, phys: u32) -> Result<u64, PageRefusal> {
    let mut bytes = [0u8; 8];
    for (i, slot) in bytes.iter_mut().enumerate() {
        *slot = u32::checked_add(phys, i as u32)
            .and_then(|a| mem.read_u8(a))
            .ok_or(PageRefusal::EntryUnreadable { phys })?;
    }
    Ok(u64::from_le_bytes(bytes))
}

/// Walks the 4-level tables for the 32-bit `linear` address.
pub fn translate<M: RealModeMemory>(
    mem: &M,
    params: PagingParams,
    linear: u32,
    kind: AccessKind,
) -> Result<Translation, PageRefusal> {
    // (level, address of the entry, entry)
    let mut used: Vec<(u8, u32, u64)> = Vec::with_capacity(4);
    let mut table = u64::from(params.cr3 & 0xFFFF_F000);
    let mut rw_all = true;
    let mut xd_any = false;
    let l = u64::from(linear);
    // A linear address below 4 GiB: the PML4 index (bits 47:39) is 0.
    let indexes = [
        (4u8, 0u64),
        (3, (l >> 30) & 0x1FF),
        (2, (l >> 21) & 0x1FF),
        (1, (l >> 12) & 0x1FF),
    ];
    let mut leaf: Option<(PageSize, u64)> = None;
    for (level, index) in indexes {
        let entry_at = table + index * 8;
        let Ok(entry_phys) = u32::try_from(entry_at) else {
            return Err(PageRefusal::PhysicalBeyond4GiB { phys: entry_at });
        };
        let entry = read_entry(mem, entry_phys)?;
        if entry & P == 0 {
            return Err(PageRefusal::NotPresent { level, entry });
        }
        if entry & BEYOND_MAXPHYADDR != 0 || (entry & XD != 0 && !params.nxe) {
            return Err(PageRefusal::ReservedBits { level, entry });
        }
        used.push((level, entry_phys, entry));
        rw_all &= entry & RW != 0;
        xd_any |= entry & XD != 0;
        match level {
            4 if entry & PS != 0 => return Err(PageRefusal::ReservedBits { level, entry }),
            3 if entry & PS != 0 => return Err(PageRefusal::GigabytePage { entry }),
            2 if entry & PS != 0 => {
                if entry & PDE_2M_RESERVED != 0 {
                    return Err(PageRefusal::ReservedBits { level, entry });
                }
                leaf = Some((PageSize::Size2M, entry));
                break;
            }
            1 => {
                leaf = Some((PageSize::Size4K, entry));
                break;
            }
            _ => table = entry & ADDR_4K,
        }
    }
    let (size, entry) = leaf.expect("the walk ends at a PDE with PS or at a PTE");
    if kind == AccessKind::Write && params.wp && !rw_all {
        return Err(PageRefusal::WriteProtected { entry });
    }
    if kind == AccessKind::Fetch && params.nxe && xd_any {
        return Err(PageRefusal::NoExecute { entry });
    }
    let phys64 = match size {
        PageSize::Size2M => (entry & ADDR_2M) | (l & 0x1F_FFFF),
        PageSize::Size4K => (entry & ADDR_4K) | (l & 0xFFF),
    };
    let Ok(phys) = u32::try_from(phys64) else {
        return Err(PageRefusal::PhysicalBeyond4GiB { phys: phys64 });
    };
    let mut touches = Vec::new();
    for (i, &(_, entry_phys, e)) in used.iter().enumerate() {
        let last = i + 1 == used.len();
        let mut want = if e & ACCESSED == 0 { ACCESSED } else { 0 };
        if last && kind == AccessKind::Write && e & DIRTY == 0 {
            want |= DIRTY;
        }
        if want != 0 {
            if !mem.can_write(entry_phys) {
                return Err(PageRefusal::EntryUnwritable { phys: entry_phys });
            }
            touches.push((entry_phys, want as u8));
        }
    }
    Ok(Translation {
        phys,
        size,
        touches,
    })
}

/// Writes the accessed/dirty updates of [`Translation::touches`]: ORs each mask into its byte
/// through [`RealModeMemory::or_u8`] (S6-25: the single place of this path, no ordinary
/// read-then-write here). Every address was checked writable by [`translate`], so this does not
/// fail. S6-25 deviation (AP path): the OR is deferred and applied without comparing the entry
/// with the value read by the walk; correct while whole instructions are interleaved. S6-31 did
/// not replace it: a CAS on the walk's value (an atomic OR alone could turn a zeroed PTE into a
/// nonzero one) needs the walked value in [`Translation::touches`] and a retry in the caller, a
/// decision recorded for the architecture review; replacement points are this function and
/// `RealModeMemory::or_u8`.
pub fn commit_touches<M: RealModeMemory>(mem: &mut M, touches: &[(u32, u8)]) {
    for &(at, mask) in touches {
        assert!(
            mem.or_u8(at, mask),
            "the entry was read and can_write was true"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::real_mode_exec::{ImageMemory, ImageMemoryMut};

    const PML4: usize = 0x1000;
    const PDPT: usize = 0x2000;
    const PD: usize = 0x3000;
    const PT: usize = 0x4000;
    const RAM: usize = 0x5000;
    const PARAMS: PagingParams = PagingParams {
        cr3: PML4 as u32,
        wp: true,
        nxe: true,
    };

    fn set(ram: &mut [u8], at: usize, entry: u64) {
        ram[at..at + 8].copy_from_slice(&entry.to_le_bytes());
    }

    fn entry(ram: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(ram[at..at + 8].try_into().unwrap())
    }

    /// PML4[0] -> PDPT, PDPT[0] -> PD, PD[0] -> PT, PT[i] = identity of page i (i < 5).
    fn tables_4k() -> Vec<u8> {
        let mut ram = vec![0u8; RAM];
        set(&mut ram, PML4, PDPT as u64 | 3);
        set(&mut ram, PDPT, PD as u64 | 3);
        set(&mut ram, PD, PT as u64 | 3);
        for page in 0..5usize {
            set(&mut ram, PT + 8 * page, (page as u64) << 12 | 3);
        }
        ram
    }

    /// Walk over a writable copy (the touches are not committed).
    fn walk(ram: &[u8], linear: u32, kind: AccessKind) -> Result<Translation, PageRefusal> {
        let mut copy = ram.to_vec();
        translate(
            &ImageMemoryMut {
                base: 0,
                bytes: &mut copy,
            },
            PARAMS,
            linear,
            kind,
        )
    }

    /// Walk over read-only memory.
    fn walk_ro(ram: &[u8], linear: u32, kind: AccessKind) -> Result<Translation, PageRefusal> {
        translate(
            &ImageMemory {
                base: 0,
                bytes: ram,
            },
            PARAMS,
            linear,
            kind,
        )
    }

    #[test]
    fn four_kib_translation_adds_the_page_offset_and_wants_the_accessed_bits() {
        let mut ram = tables_4k();
        // PT[2] maps page 2 to physical 0x3000 (not the identity), the offset comes from the address.
        set(&mut ram, PT + 16, 0x3000 | 3);
        let t = walk(&ram, 0x2065, AccessKind::Read).unwrap();
        assert_eq!((t.phys, t.size), (0x3065, PageSize::Size4K));
        assert_eq!(
            t.touches,
            vec![
                (PML4 as u32, 0x20),
                (PDPT as u32, 0x20),
                (PD as u32, 0x20),
                ((PT + 16) as u32, 0x20),
            ]
        );
    }

    #[test]
    fn two_mib_translation_uses_bits_20_0_of_the_address() {
        let mut ram = tables_4k();
        // PD[0] = 2 MiB page at physical 2 MiB (the tables are not read for the data page).
        set(&mut ram, PD, 0x20_0000 | PS | 3);
        let t = walk(&ram, 0x1F_FFFF, AccessKind::Read).unwrap();
        assert_eq!((t.phys, t.size), (0x3F_FFFF, PageSize::Size2M));
        // Three levels were used: PML4E, PDPTE and the PDE (no PTE).
        assert_eq!(t.touches.len(), 3);
        assert_eq!(t.touches[2], (PD as u32, 0x20));
    }

    #[test]
    fn pdpt_index_comes_from_bits_31_30_and_pd_index_from_bits_29_21() {
        let mut ram = tables_4k();
        // PDPT[1] -> PD (same PD page), PD[3] = 2 MiB page at 0x40_0000.
        set(&mut ram, PDPT + 8, PD as u64 | 3);
        set(&mut ram, PD + 24, 0x40_0000 | PS | 3);
        let t = walk(&ram, 0x4000_0000 + 0x6_0000 + 0x123, AccessKind::Read);
        // Linear 0x4006_0123: PDPT idx 1, PD idx 0 -> PT page 0x60 is not present (PT idx 0x60).
        assert!(matches!(t, Err(PageRefusal::NotPresent { level: 1, .. })));
        let t = walk(&ram, 0x4000_0000 + 3 * 0x20_0000 + 0x123, AccessKind::Read).unwrap();
        assert_eq!(t.phys, 0x40_0123);
    }

    #[test]
    fn a_write_sets_dirty_in_the_final_entry_only() {
        let ram = tables_4k();
        let t = walk(&ram, 0x2000, AccessKind::Write).unwrap();
        assert_eq!(t.touches.last(), Some(&((PT + 16) as u32, 0x60)));
        assert!(t.touches[..3].iter().all(|&(_, m)| m == 0x20));
        // Fetch and read never set D.
        let t = walk(&ram, 0x2000, AccessKind::Fetch).unwrap();
        assert_eq!(t.touches.last(), Some(&((PT + 16) as u32, 0x20)));
    }

    #[test]
    fn set_accessed_and_dirty_bits_need_no_touch() {
        let mut ram = tables_4k();
        for at in [PML4, PDPT, PD, PT + 16] {
            let e = entry(&ram, at);
            set(&mut ram, at, e | 0x60);
        }
        let t = walk(&ram, 0x2000, AccessKind::Write).unwrap();
        assert!(t.touches.is_empty());
    }

    #[test]
    fn commit_ors_the_bits_into_the_entries() {
        let mut ram = tables_4k();
        let t = walk(&ram, 0x2000, AccessKind::Write).unwrap();
        commit_touches(
            &mut ImageMemoryMut {
                base: 0,
                bytes: &mut ram,
            },
            &t.touches,
        );
        assert_eq!(entry(&ram, PML4), PDPT as u64 | 0x23);
        assert_eq!(entry(&ram, PT + 16), 0x2000 | 0x63);
        // The same walk now has nothing to do.
        assert!(
            walk(&ram, 0x2000, AccessKind::Write)
                .unwrap()
                .touches
                .is_empty()
        );
    }

    #[test]
    fn a_missing_page_is_refused_at_every_level() {
        for (level, at) in [(4u8, PML4), (3, PDPT), (2, PD), (1, PT + 16)] {
            let mut ram = tables_4k();
            set(&mut ram, at, 0);
            assert_eq!(
                walk(&ram, 0x2000, AccessKind::Read),
                Err(PageRefusal::NotPresent { level, entry: 0 }),
                "level {level}"
            );
        }
        // P = 0 with other bits set is still not present.
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, 0x2000 | 2);
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::NotPresent { level: 1, .. })
        ));
    }

    #[test]
    fn a_misaligned_two_mib_page_is_refused_but_pat_bit_12_is_not() {
        for bit in 13..=20 {
            let mut ram = tables_4k();
            set(&mut ram, PD, (1u64 << bit) | PS | 3);
            assert!(
                matches!(
                    walk(&ram, 0x2000, AccessKind::Read),
                    Err(PageRefusal::ReservedBits { level: 2, .. })
                ),
                "bit {bit}"
            );
        }
        let mut ram = tables_4k();
        set(&mut ram, PD, (1u64 << 12) | PS | 3);
        assert_eq!(walk(&ram, 0x2000, AccessKind::Read).unwrap().phys, 0x2000);
    }

    #[test]
    fn reserved_bits_of_the_other_levels() {
        // PML4E with PS.
        let mut ram = tables_4k();
        set(&mut ram, PML4, PDPT as u64 | PS | 3);
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::ReservedBits { level: 4, .. })
        ));
        // Bit 36 (>= MAXPHYADDR) of a PTE; bit 35 is a valid address bit (but beyond 4 GiB here).
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, (1u64 << 36) | 3);
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::ReservedBits { level: 1, .. })
        ));
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, (1u64 << 35) | 3);
        assert_eq!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::PhysicalBeyond4GiB { phys: 1 << 35 })
        );
        // XD with NXE = 0 is reserved; with NXE = 1 it only refuses a fetch.
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, XD | 0x2000 | 3);
        let no_nx = PagingParams {
            nxe: false,
            ..PARAMS
        };
        assert!(matches!(
            translate(
                &ImageMemory {
                    base: 0,
                    bytes: &ram
                },
                no_nx,
                0x2000,
                AccessKind::Read
            ),
            Err(PageRefusal::ReservedBits { level: 1, .. })
        ));
        assert!(walk(&ram, 0x2000, AccessKind::Read).is_ok());
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Fetch),
            Err(PageRefusal::NoExecute { .. })
        ));
        // Bits 62:52 are ignored.
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, (0x7FFu64 << 52) | 0x2000 | 3);
        assert_eq!(walk(&ram, 0x2000, AccessKind::Read).unwrap().phys, 0x2000);
    }

    #[test]
    fn a_gigabyte_page_is_refused() {
        let mut ram = tables_4k();
        set(&mut ram, PDPT, PS | 3);
        assert_eq!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::GigabytePage { entry: PS | 3 })
        );
    }

    #[test]
    fn xd_at_an_upper_level_refuses_a_fetch_and_rw_at_any_level_refuses_a_write() {
        let mut ram = tables_4k();
        set(&mut ram, PD, PT as u64 | XD | 3);
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Fetch),
            Err(PageRefusal::NoExecute { .. })
        ));
        let mut ram = tables_4k();
        set(&mut ram, PDPT, PD as u64 | 1);
        assert!(matches!(
            walk(&ram, 0x2000, AccessKind::Write),
            Err(PageRefusal::WriteProtected { .. })
        ));
        // Reads and fetches are fine, and so is a write with CR0.WP = 0.
        assert!(walk(&ram, 0x2000, AccessKind::Read).is_ok());
        assert!(walk(&ram, 0x2000, AccessKind::Fetch).is_ok());
        let no_wp = PagingParams {
            wp: false,
            ..PARAMS
        };
        assert!(
            translate(
                &ImageMemoryMut {
                    base: 0,
                    bytes: &mut ram.clone()
                },
                no_wp,
                0x2000,
                AccessKind::Write
            )
            .is_ok()
        );
    }

    #[test]
    fn unreadable_entries_and_unwritable_touches_are_refused() {
        let ram = tables_4k();
        // CR3 outside the image.
        let far = PagingParams {
            cr3: 0x10_0000,
            ..PARAMS
        };
        assert_eq!(
            translate(
                &ImageMemory {
                    base: 0,
                    bytes: &ram
                },
                far,
                0x2000,
                AccessKind::Read
            ),
            Err(PageRefusal::EntryUnreadable { phys: 0x10_0000 })
        );
        // ImageMemory is read-only: a walk that has to set an A bit is refused.
        let mut ram2 = tables_4k();
        for at in [PML4, PDPT, PD, PT + 16] {
            let e = entry(&ram2, at);
            set(&mut ram2, at, e | 0x20);
        }
        assert!(walk_ro(&ram2, 0x2000, AccessKind::Read).is_ok());
        assert_eq!(
            walk_ro(&ram2, 0x2000, AccessKind::Write),
            Err(PageRefusal::EntryUnwritable {
                phys: (PT + 16) as u32
            })
        );
        assert_eq!(
            walk_ro(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::EntryUnwritable { phys: PML4 as u32 })
        );
    }

    #[test]
    fn cr3_low_bits_are_ignored_and_the_page_may_not_be_below_4_gib() {
        let ram = tables_4k();
        let odd = PagingParams {
            cr3: PML4 as u32 | 0x18,
            ..PARAMS
        };
        assert!(
            translate(
                &ImageMemoryMut {
                    base: 0,
                    bytes: &mut ram.clone()
                },
                odd,
                0x2000,
                AccessKind::Read
            )
            .is_ok()
        );
        let mut ram = tables_4k();
        set(&mut ram, PT + 16, (1u64 << 32) | 3);
        assert_eq!(
            walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::PhysicalBeyond4GiB { phys: 1 << 32 })
        );
    }

    /// S6-25: the accessed/dirty bits of this path are written only through
    /// `RealModeMemory::or_u8`: an ordinary `write_u8` is never called by `commit_touches`.
    #[test]
    fn commit_touches_goes_through_the_or_operation_only() {
        struct OrOnly {
            bytes: Vec<u8>,
            ors: Vec<(u32, u8)>,
        }
        impl RealModeMemory for OrOnly {
            fn read_u8(&self, linear: u32) -> Option<u8> {
                self.bytes.get(linear as usize).copied()
            }
            fn can_write(&self, _linear: u32) -> bool {
                true
            }
            fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
                panic!("an ordinary byte write on the accessed/dirty path");
            }
            fn or_u8(&mut self, linear: u32, mask: u8) -> bool {
                self.ors.push((linear, mask));
                self.bytes[linear as usize] |= mask;
                true
            }
        }
        let mut ram = tables_4k();
        let mut mem = OrOnly {
            bytes: ram.split_off(0),
            ors: Vec::new(),
        };
        let walked = translate(&mem, PARAMS, 0x2abc, AccessKind::Write).unwrap();
        // Another writer sets a bit of the same low byte after the walk: the OR keeps it.
        mem.bytes[PT + 16] |= 1 << 4;
        commit_touches(&mut mem, &walked.touches);
        assert_eq!(mem.ors, walked.touches);
        assert_eq!(mem.bytes[PT + 16] & 0x70, 0x70, "A, D and the other bit");
    }
}
