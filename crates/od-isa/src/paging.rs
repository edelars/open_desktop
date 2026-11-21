//! IA-32e (4-level) page-table walk for the AP trampoline path (S6-14m).
//!
//! S2-25 adds PAE paging (`CR0.PG = 1`, `CR4.PAE = 1`, `EFER.LME = 0`, SDM Vol. 3A §5.4 in the Rev. 093
//! numbering, chapter 4 in older ones): [`load_pdptes`] and [`translate_pae`], below. Model only: there
//! is no hardware oracle for it in this repository.
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
//!   *touches* (entry address, the value the walk read, OR mask) and written by the caller only
//!   after the whole instruction is known to succeed. A walk that ends in a `#PF` sets `A` in none of its entries: the
//!   model, like the oracle (images `k`, `m`..`q`, `s`), leaves the entries of a refused walk
//!   untouched (contract S6-14m, decision 4; walks that completed earlier in the same instruction
//!   keep their bits, see `ProtectedCpu::step`). A
//!   touch needs the low byte of the entry writable in memory, otherwise the walk is refused with
//!   [`PageRefusal::EntryUnwritable`]. Entries are 8 bytes, little-endian, read with one 8-byte
//!   load; the update is a compare-and-replace of the whole entry on the value the walk read
//!   (S6-31b: [`commit_touches`], `RealModeMemory::compare_replace_entry`).

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
    /// Accessed/dirty updates still to be written (see [`Touch`]). Empty when every `A` (and, for
    /// a write, `D`) bit is already set. Every address has been checked writable.
    pub touches: Vec<Touch>,
}

/// One accessed/dirty update of a walk (S6-31b): the entry, the value of it the walk read and the
/// bits to set. [`commit_touches`] replaces `read` with `read | mask` only if the entry still holds
/// `read` (compare-and-replace), so a concurrent change of the entry is never overwritten.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Touch {
    /// Address of the entry (8-aligned).
    pub at: u32,
    /// The 8 bytes of the entry as the walk read them.
    pub read: u64,
    /// Bits to OR into the low byte (`A` and, for the final entry of a write, `D`).
    pub mask: u8,
}

/// S6-31b: one 8-byte read of the entry (it was eight `read_u8` before).
fn read_entry<M: RealModeMemory>(mem: &M, phys: u32) -> Result<u64, PageRefusal> {
    mem.read_u64(phys)
        .ok_or(PageRefusal::EntryUnreadable { phys })
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
    finish_walk(
        mem,
        WalkEnd {
            wp: params.wp,
            nxe: params.nxe,
            kind,
            linear: l,
            used: &used,
            rw_all,
            xd_any,
            size,
            entry,
        },
    )
}

/// What a walk of either mode knows once it has reached its leaf entry (shared by [`translate`]
/// and [`translate_pae`]).
struct WalkEnd<'a> {
    wp: bool,
    nxe: bool,
    kind: AccessKind,
    linear: u64,
    /// (level, address of the entry, entry) of every entry that takes an accessed bit.
    used: &'a [(u8, u32, u64)],
    rw_all: bool,
    xd_any: bool,
    size: PageSize,
    entry: u64,
}

/// Access-rights checks, physical address and accessed/dirty touches of a finished walk.
fn finish_walk<M: RealModeMemory>(mem: &M, end: WalkEnd<'_>) -> Result<Translation, PageRefusal> {
    let WalkEnd {
        wp,
        nxe,
        kind,
        linear: l,
        used,
        rw_all,
        xd_any,
        size,
        entry,
    } = end;
    if kind == AccessKind::Write && wp && !rw_all {
        return Err(PageRefusal::WriteProtected { entry });
    }
    if kind == AccessKind::Fetch && nxe && xd_any {
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
            touches.push(Touch {
                at: entry_phys,
                read: e,
                mask: want as u8,
            });
        }
    }
    Ok(Translation {
        phys,
        size,
        touches,
    })
}

/// PDPTE bits that must be 0 when `P = 1`: 2:1, 8:5 and 63:`MAXPHYADDR` (SDM Vol. 3A Table 5-8).
const PDPTE_RESERVED: u64 = 0xFFFF_FFF0_0000_0000 | 0x1E6;

/// PDE/PTE bits 62:`MAXPHYADDR` with PAE paging, reserved when `P = 1` (SDM Vol. 3A §5.4.2; the
/// 4-level walk ignores bits 62:52, PAE paging does not).
const PAE_ENTRY_RESERVED: u64 = 0x7FFF_FFF0_0000_0000;

/// Paging control state of a PAE walk (S2-25): `CR0.PG = 1`, `CR4.PAE = 1`, `EFER.LME = 0`. The walk
/// does not read `CR3`; it uses the four PDPTE registers loaded by [`load_pdptes`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaeParams {
    /// The PDPTE registers, selected by bits 31:30 of the linear address.
    pub pdptes: [u64; 4],
    /// `CR0.WP`.
    pub wp: bool,
    /// `EFER.NXE`.
    pub nxe: bool,
}

/// Why the PDPTEs could not be loaded by [`load_pdptes`] (`MOV` to `CR0`/`CR3`/`CR4`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PdpteRefusal {
    /// A PDPTE has `P = 1` and a reserved bit set: `#GP(0)` at the `MOV` to control register, the
    /// PDPTEs are not loaded (SDM Vol. 3A §5.4.1). `index` is the PDPTE number.
    Reserved { index: u8, pdpte: u64 },
    /// The 8 bytes of the PDPTE at `phys` are not readable in memory (a model limit).
    Unreadable { phys: u32 },
}

/// Loads the four PDPTEs from the 32-byte-aligned table at `cr3 & 0xFFFF_FFE0` (SDM Vol. 3A Table
/// 5-7: bits 4:0 of `CR3` are ignored). Reserved bits are checked only in PDPTEs with `P = 1`
/// (the SDM notes that some processors check them also with `P = 0`; not modelled).
pub fn load_pdptes<M: RealModeMemory>(mem: &M, cr3: u32) -> Result<[u64; 4], PdpteRefusal> {
    let base = cr3 & 0xFFFF_FFE0;
    let mut pdptes = [0u64; 4];
    for (index, slot) in pdptes.iter_mut().enumerate() {
        let phys = base + 8 * index as u32;
        let pdpte = mem
            .read_u64(phys)
            .ok_or(PdpteRefusal::Unreadable { phys })?;
        if pdpte & P != 0 && pdpte & PDPTE_RESERVED != 0 {
            return Err(PdpteRefusal::Reserved {
                index: index as u8,
                pdpte,
            });
        }
        *slot = pdpte;
    }
    Ok(pdptes)
}

/// Walks the PAE tables (PDPTE register, PD, PT; SDM Vol. 3A §5.4.2) for the 32-bit `linear`
/// address. 4 KiB and 2 MiB pages. A PDPTE has no `R/W`, `U/S`, `XD` or accessed bit: it takes no
/// part in the access rights and is never touched. Everything else is the walk of [`translate`]
/// (levels 2 and 1 only, `PageRefusal::NotPresent { level: 3 }` is a PDPTE with `P = 0`).
pub fn translate_pae<M: RealModeMemory>(
    mem: &M,
    params: PaeParams,
    linear: u32,
    kind: AccessKind,
) -> Result<Translation, PageRefusal> {
    let l = u64::from(linear);
    let pdpte = params.pdptes[(linear >> 30) as usize];
    if pdpte & P == 0 {
        return Err(PageRefusal::NotPresent {
            level: 3,
            entry: pdpte,
        });
    }
    let mut table = pdpte & ADDR_4K;
    let mut used: Vec<(u8, u32, u64)> = Vec::with_capacity(2);
    let mut rw_all = true;
    let mut xd_any = false;
    let mut leaf: Option<(PageSize, u64)> = None;
    for (level, index) in [(2u8, (l >> 21) & 0x1FF), (1, (l >> 12) & 0x1FF)] {
        let entry_at = table + index * 8;
        let Ok(entry_phys) = u32::try_from(entry_at) else {
            return Err(PageRefusal::PhysicalBeyond4GiB { phys: entry_at });
        };
        let entry = read_entry(mem, entry_phys)?;
        if entry & P == 0 {
            return Err(PageRefusal::NotPresent { level, entry });
        }
        if entry & PAE_ENTRY_RESERVED != 0 || (entry & XD != 0 && !params.nxe) {
            return Err(PageRefusal::ReservedBits { level, entry });
        }
        used.push((level, entry_phys, entry));
        rw_all &= entry & RW != 0;
        xd_any |= entry & XD != 0;
        if level == 2 && entry & PS != 0 {
            if entry & PDE_2M_RESERVED != 0 {
                return Err(PageRefusal::ReservedBits { level, entry });
            }
            leaf = Some((PageSize::Size2M, entry));
            break;
        }
        if level == 1 {
            leaf = Some((PageSize::Size4K, entry));
            break;
        }
        table = entry & ADDR_4K;
    }
    let (size, entry) = leaf.expect("the walk ends at a PDE with PS or at a PTE");
    finish_walk(
        mem,
        WalkEnd {
            wp: params.wp,
            nxe: params.nxe,
            kind,
            linear: l,
            used: &used,
            rw_all,
            xd_any,
            size,
            entry,
        },
    )
}

/// Bits that take no part in the translation: `A`, `D`, 9..=11 and 52..=62 (the profile has no
/// protection keys). Same set as `IGNORED` and `AD` in `commit_leaf_dirty` of `od-machine`.
const NOT_TRANSLATION: u64 = ACCESSED | DIRTY | (0b111 << 9) | (0x7FF << 52);

// Bits 59..=62 of an entry are protection-key bits when CR4.PKE (bit 22) is set; `NOT_TRANSLATION`
// ignores them, which is right only while `MOV CR4` cannot set PKE.
const _: () = assert!(
    crate::protected_exec::CR4_MODELLED_MASK & (1 << 22) == 0,
    "NOT_TRANSLATION ignores bits 59..=62: revisit it before CR4.PKE becomes settable"
);

/// How many failed replaces one touch takes before it is discarded (bounds the retry; the same
/// number as `TABLE_REPLACE_RETRY_LIMIT` of the walk of `od-machine`).
const TOUCH_RETRY_LIMIT: u32 = 16;

/// Writes the accessed/dirty updates of [`Translation::touches`] (S6-31b) and returns how many
/// touches were discarded. Each touch is a compare-and-replace
/// ([`RealModeMemory::compare_replace_entry`]) of the entry from the value the walk read to that
/// value with the mask ORed in: the single place of this path, no ordinary read-then-write here.
/// Every address was checked writable by [`translate`], so a missing entry is a bug (panic).
///
/// When the entry no longer holds the value the walk read, the observed value is compared with it
/// on the translation bits (all bits except `A`, `D`, 9..=11 and 52..=62): if they are equal the
/// mask is ORed into the OBSERVED value and the replace is repeated (the other writer's bits are
/// kept; the same protocol as `commit_leaf_dirty`); if a translation bit changed (or the retry
/// limit is reached) the touch is DISCARDED: neither `A` nor `D` is set. The instruction is not
/// repeated: the caller runs after the instruction retired (`ProtectedCpu::step`), and a repeat
/// would duplicate its effects. Using the stale translation is allowed (like a stale TLB entry).
/// A zeroed entry never becomes nonzero: the replace needs the entry to equal a value read by
/// the walk or observed with equal translation bits (a not present entry would have a different
/// `P`).
pub fn commit_touches<M: RealModeMemory>(mem: &mut M, touches: &[Touch]) -> usize {
    let mut discarded = 0;
    for touch in touches {
        let mut expected = touch.read;
        let mut failed = 0;
        loop {
            let replacement = expected | u64::from(touch.mask);
            if replacement == expected {
                // The observed value already has the bits (another writer set them).
                break;
            }
            let outcome = mem
                .compare_replace_entry(touch.at, expected, replacement)
                .expect("the entry was read and can_write was true");
            match outcome {
                Ok(()) => break,
                Err(observed) => {
                    failed += 1;
                    if observed & !NOT_TRANSLATION != touch.read & !NOT_TRANSLATION
                        || failed > TOUCH_RETRY_LIMIT
                    {
                        discarded += 1;
                        break;
                    }
                    expected = observed;
                }
            }
        }
    }
    discarded
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

    /// The touch of the entry at `at` as the walk reports it (the value read from `ram`).
    fn touch(ram: &[u8], at: usize, mask: u8) -> Touch {
        Touch {
            at: at as u32,
            read: entry(ram, at),
            mask,
        }
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
                touch(&ram, PML4, 0x20),
                touch(&ram, PDPT, 0x20),
                touch(&ram, PD, 0x20),
                touch(&ram, PT + 16, 0x20),
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
        assert_eq!(t.touches[2], touch(&ram, PD, 0x20));
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
        assert_eq!(t.touches.last(), Some(&touch(&ram, PT + 16, 0x60)));
        assert!(t.touches[..3].iter().all(|t| t.mask == 0x20));
        // Fetch and read never set D.
        let t = walk(&ram, 0x2000, AccessKind::Fetch).unwrap();
        assert_eq!(t.touches.last(), Some(&touch(&ram, PT + 16, 0x20)));
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

    // ---- S2-25: PAE paging (model only: there is no hardware oracle for it) ----------------

    /// PAE tables: the PDPT (32 bytes) at `PDPT`, PDPTE0 -> PD (`P` only: bits 2:1 and 8:5 of a PDPTE
    /// are reserved), PD[0] -> PT, PT[i] = identity of page i (i < 5).
    fn pae_tables_4k() -> Vec<u8> {
        let mut ram = vec![0u8; RAM];
        set(&mut ram, PDPT, PD as u64 | 1);
        set(&mut ram, PD, PT as u64 | 3);
        for page in 0..5usize {
            set(&mut ram, PT + 8 * page, (page as u64) << 12 | 3);
        }
        ram
    }

    fn pae_params(ram: &[u8]) -> PaeParams {
        PaeParams {
            pdptes: load_pdptes(
                &ImageMemory {
                    base: 0,
                    bytes: ram,
                },
                PDPT as u32,
            )
            .unwrap(),
            wp: true,
            nxe: true,
        }
    }

    fn pae_walk(ram: &[u8], linear: u32, kind: AccessKind) -> Result<Translation, PageRefusal> {
        let mut copy = ram.to_vec();
        translate_pae(
            &ImageMemoryMut {
                base: 0,
                bytes: &mut copy,
            },
            pae_params(ram),
            linear,
            kind,
        )
    }

    #[test]
    fn pae_four_kib_translation_touches_the_pde_and_pte_but_never_the_pdpte() {
        let ram = pae_tables_4k();
        let read = pae_walk(&ram, 0x2123, AccessKind::Read).unwrap();
        assert_eq!((read.phys, read.size), (0x2123, PageSize::Size4K));
        // A on the PDE and the PTE; the PDPTE is a register: no accessed bit, no touch.
        assert_eq!(
            read.touches,
            vec![touch(&ram, PD, 0x20), touch(&ram, PT + 16, 0x20)]
        );
        let write = pae_walk(&ram, 0x2123, AccessKind::Write).unwrap();
        assert_eq!(
            write.touches,
            vec![touch(&ram, PD, 0x20), touch(&ram, PT + 16, 0x60)]
        );
    }

    #[test]
    fn pae_two_mib_page_and_the_pdpte_selected_by_bits_31_30() {
        let mut ram = pae_tables_4k();
        set(&mut ram, PD, 0x40_0000 | 0x83);
        let t = pae_walk(&ram, 0x0012_3456, AccessKind::Write).unwrap();
        assert_eq!((t.phys, t.size), (0x52_3456, PageSize::Size2M));
        assert_eq!(t.touches, vec![touch(&ram, PD, 0x60)]);
        // PDPTE3 (linear 0xC000_0000..) -> a page directory at 0x1000 with one 2 MiB page at 0.
        set(&mut ram, PDPT + 24, 0x1000 | 1);
        set(&mut ram, 0x1000, 0x83);
        let t = pae_walk(&ram, 0xC000_1234, AccessKind::Read).unwrap();
        assert_eq!((t.phys, t.size), (0x1234, PageSize::Size2M));
        // PDPTE1 is not present: a #PF at level 3 (the entry is the register value).
        assert_eq!(
            pae_walk(&ram, 0x4000_0000, AccessKind::Read),
            Err(PageRefusal::NotPresent { level: 3, entry: 0 })
        );
    }

    #[test]
    fn pae_reserved_bits_differ_from_the_four_level_walk() {
        // Bits 62:36 are reserved in a PAE PDE/PTE (the 4-level walk ignores 62:52).
        for bit in [36u32, 51, 52, 62] {
            let mut ram = pae_tables_4k();
            set(&mut ram, PT + 16, 0x2000 | 3 | 1 << bit);
            assert_eq!(
                pae_walk(&ram, 0x2000, AccessKind::Read),
                Err(PageRefusal::ReservedBits {
                    level: 1,
                    entry: 0x2000 | 3 | 1 << bit
                }),
                "PTE bit {bit}"
            );
            let mut ram = pae_tables_4k();
            set(&mut ram, PD, PT as u64 | 3 | 1 << bit);
            assert!(matches!(
                pae_walk(&ram, 0x2000, AccessKind::Read),
                Err(PageRefusal::ReservedBits { level: 2, .. })
            ));
        }
        // PDE with PS: bits 20:13 are reserved, bit 12 (PAT) is not.
        let mut ram = pae_tables_4k();
        set(&mut ram, PD, 0x2000 | 0x83);
        assert!(matches!(
            pae_walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::ReservedBits { level: 2, .. })
        ));
        set(&mut ram, PD, 0x1000 | 0x83);
        assert!(pae_walk(&ram, 0x2000, AccessKind::Read).is_ok());
        // XD is reserved while EFER.NXE = 0.
        let mut ram = pae_tables_4k();
        set(&mut ram, PT + 16, 0x2000 | 3 | 1 << 63);
        let mut copy = ram.clone();
        let no_nxe = PaeParams {
            nxe: false,
            ..pae_params(&ram)
        };
        let mem = ImageMemoryMut {
            base: 0,
            bytes: &mut copy,
        };
        assert!(matches!(
            translate_pae(&mem, no_nxe, 0x2000, AccessKind::Read),
            Err(PageRefusal::ReservedBits { level: 1, .. })
        ));
        // Not present PDE and PTE.
        let mut ram = pae_tables_4k();
        set(&mut ram, PT + 16, 0x2000 | 2);
        assert!(matches!(
            pae_walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::NotPresent { level: 1, .. })
        ));
        set(&mut ram, PD, PT as u64 | 2);
        assert!(matches!(
            pae_walk(&ram, 0x2000, AccessKind::Read),
            Err(PageRefusal::NotPresent { level: 2, .. })
        ));
    }

    #[test]
    fn pae_write_protect_and_no_execute_use_the_pde_and_pte_only() {
        let mut ram = pae_tables_4k();
        set(&mut ram, PD, PT as u64 | 1); // R/W clear in the PDE
        assert!(matches!(
            pae_walk(&ram, 0x2000, AccessKind::Write),
            Err(PageRefusal::WriteProtected { .. })
        ));
        assert!(pae_walk(&ram, 0x2000, AccessKind::Read).is_ok());
        let mut ram = pae_tables_4k();
        set(&mut ram, PT + 16, 0x2000 | 3 | 1 << 63);
        assert!(matches!(
            pae_walk(&ram, 0x2000, AccessKind::Fetch),
            Err(PageRefusal::NoExecute { .. })
        ));
        assert!(pae_walk(&ram, 0x2000, AccessKind::Read).is_ok());
    }

    #[test]
    fn pdpte_loading_checks_reserved_bits_only_with_p_and_ignores_cr3_low_bits() {
        let ok = pae_tables_4k();
        let m = ImageMemory {
            base: 0,
            bytes: &ok,
        };
        assert_eq!(
            load_pdptes(&m, PDPT as u32 | 0x1F),
            Ok([PD as u64 | 1, 0, 0, 0]),
            "bits 4:0 of CR3 are ignored"
        );
        // Bits 2:1, 8:5 and 63:36 are reserved in a PDPTE with P = 1.
        for bit in [1u32, 2, 5, 6, 7, 8, 36, 50, 63] {
            for index in 0..4usize {
                let mut ram = pae_tables_4k();
                let bad = PD as u64 | 1 | 1 << bit;
                set(&mut ram, PDPT + 8 * index, bad);
                let m = ImageMemory {
                    base: 0,
                    bytes: &ram,
                };
                assert_eq!(
                    load_pdptes(&m, PDPT as u32),
                    Err(PdpteRefusal::Reserved {
                        index: index as u8,
                        pdpte: bad
                    }),
                    "bit {bit} of PDPTE{index}"
                );
                // The same bits with P = 0 are not checked (model decision, SDM footnote).
                let mut ram = pae_tables_4k();
                set(&mut ram, PDPT + 8 * index, bad & !1);
                let m = ImageMemory {
                    base: 0,
                    bytes: &ram,
                };
                assert!(load_pdptes(&m, PDPT as u32).is_ok());
            }
        }
        // Bits 3 (PWT), 4 (PCD) and 11:9 (ignored) are allowed.
        let mut ram = pae_tables_4k();
        set(&mut ram, PDPT, PD as u64 | 1 | 0x18 | 0xE00);
        let m = ImageMemory {
            base: 0,
            bytes: &ram,
        };
        assert!(load_pdptes(&m, PDPT as u32).is_ok());
        // The table must be readable: 0x5000 is the end of the image.
        assert_eq!(
            load_pdptes(&m, 0x5000),
            Err(PdpteRefusal::Unreadable { phys: 0x5000 })
        );
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

    /// An image memory that records every call of the write paths and refuses the ordinary
    /// byte write on the accessed/dirty path (S6-31b: only `compare_replace_entry` writes it).
    struct CasOnly {
        bytes: Vec<u8>,
        cas: Vec<(u32, u64, u64)>,
        /// Runs after the walk read the entry and before the compare of the next replace: lets a
        /// test change the entry the way another vCPU would between the read and the CAS.
        before_cas: Vec<(usize, u64)>,
    }

    impl CasOnly {
        fn new(bytes: Vec<u8>) -> Self {
            Self {
                bytes,
                cas: Vec::new(),
                before_cas: Vec::new(),
            }
        }
    }

    impl RealModeMemory for CasOnly {
        fn read_u8(&self, linear: u32) -> Option<u8> {
            self.bytes.get(linear as usize).copied()
        }
        fn can_write(&self, _linear: u32) -> bool {
            true
        }
        fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
            panic!("an ordinary byte write on the accessed/dirty path");
        }
        fn compare_replace_entry(
            &mut self,
            linear: u32,
            expected: u64,
            replacement: u64,
        ) -> Option<Result<(), u64>> {
            // The interference of another writer scheduled for this replace, if any.
            if !self.before_cas.is_empty() {
                let (at, value) = self.before_cas.remove(0);
                set(&mut self.bytes, at, value);
            }
            self.cas.push((linear, expected, replacement));
            let observed = entry(&self.bytes, linear as usize);
            if observed != expected {
                return Some(Err(observed));
            }
            set(&mut self.bytes, linear as usize, replacement);
            Some(Ok(()))
        }
    }

    /// S6-31b: `commit_touches` writes a table entry only by the compare-and-replace of the value
    /// the walk read; the same walk then leaves the same state as the former byte OR.
    #[test]
    fn commit_touches_goes_through_the_compare_and_replace_only() {
        let mut mem = CasOnly::new(tables_4k());
        let walked = translate(&mem, PARAMS, 0x2abc, AccessKind::Write).unwrap();
        assert_eq!(commit_touches(&mut mem, &walked.touches), 0);
        let expected: Vec<(u32, u64, u64)> = walked
            .touches
            .iter()
            .map(|t| (t.at, t.read, t.read | u64::from(t.mask)))
            .collect();
        assert_eq!(mem.cas, expected);
        assert_eq!(entry(&mem.bytes, PT + 16), 0x2000 | 0x63);
    }

    /// S6-31b (a): the state of the entries after walk and commit equals the former OR of the
    /// mask into the low byte, for the two A/D states and every leaf kind.
    #[test]
    fn commit_equals_the_former_byte_or_for_two_ad_states_and_every_leaf() {
        let leaves: [(&str, u32, AccessKind); 6] = [
            ("4k read", 0x2000, AccessKind::Read),
            ("4k write", 0x2000, AccessKind::Write),
            ("4k fetch", 0x2000, AccessKind::Fetch),
            ("2m read", 0x20_0123, AccessKind::Read),
            ("2m write", 0x20_0123, AccessKind::Write),
            ("2m fetch", 0x20_0123, AccessKind::Fetch),
        ];
        for ad in [0u64, 0x60] {
            for (name, linear, kind) in leaves {
                let mut ram = tables_4k();
                // PD[1] = 2 MiB page at 2 MiB (linear 0x20_0000 is PD index 1).
                set(&mut ram, PD + 8, 0x20_0000 | PS | 3);
                for at in [PML4, PDPT, PD, PD + 8, PT + 16] {
                    let e = entry(&ram, at);
                    set(&mut ram, at, e | ad);
                }
                let t = walk(&ram, linear, kind).unwrap();
                let mut by_or = ram.clone();
                for tt in &t.touches {
                    by_or[tt.at as usize] |= tt.mask;
                }
                let mut by_cas = ram.clone();
                let discarded = commit_touches(
                    &mut ImageMemoryMut {
                        base: 0,
                        bytes: &mut by_cas,
                    },
                    &t.touches,
                );
                assert_eq!(discarded, 0, "{name}, ad {ad:#x}");
                assert_eq!(by_cas, by_or, "{name}, ad {ad:#x}");
                assert_eq!(t.touches.is_empty(), ad == 0x60, "{name}, ad {ad:#x}");
            }
        }
    }

    /// S6-31b (b): a bit of the translation changed between the walk and the replace: the touch
    /// is discarded (neither A nor D), the changed entry is left as the other writer made it,
    /// and `commit_touches` neither repeats nor writes anything else.
    #[test]
    fn a_translation_change_between_the_walk_and_the_replace_discards_the_touch() {
        // Each change: another vCPU re-points the page, drops RW, clears P or sets XD.
        type Change = fn(u64) -> u64;
        let changes: [(&str, Change); 5] = [
            ("new frame", |e| (e & !ADDR_4K) | 0x3000),
            ("RW cleared", |e| e & !RW),
            ("P cleared", |e| e & !P),
            ("zeroed", |_| 0),
            ("XD set", |e| e | XD),
        ];
        for (name, change) in changes {
            let mut mem = CasOnly::new(tables_4k());
            let walked = translate(&mem, PARAMS, 0x2abc, AccessKind::Write).unwrap();
            let read = entry(&mem.bytes, PT + 16);
            let changed = change(read);
            mem.before_cas = vec![(PT + 16, changed)];
            let before_other = mem.bytes.clone();
            assert_eq!(commit_touches(&mut mem, &walked.touches), 1, "{name}");
            assert_eq!(entry(&mem.bytes, PT + 16), changed, "{name}: not touched");
            // The other three entries got their A bits.
            for at in [PML4, PDPT, PD] {
                assert_eq!(
                    entry(&mem.bytes, at),
                    entry(&before_other, at) | 0x20,
                    "{name}"
                );
            }
            // One replace per entry, the failed one not repeated: the instruction (the caller)
            // is not repeated either, the touch list is consumed once.
            assert_eq!(mem.cas.len(), 4, "{name}");
        }
    }

    /// S6-31b (c): only A/D or ignored bits (9..=11, 52..=62) changed: the replace is repeated
    /// over the observed value, the other writer's bits are kept and A/D are set.
    #[test]
    fn a_change_of_ignored_or_ad_bits_is_absorbed_and_the_replace_repeated() {
        let soft: u64 = (0b101 << 9) | (0x3A5 << 52);
        for other in [soft, 0x40, 0x20, 0x60 | soft] {
            let mut mem = CasOnly::new(tables_4k());
            let walked = translate(&mem, PARAMS, 0x2abc, AccessKind::Write).unwrap();
            let read = entry(&mem.bytes, PT + 16);
            // Interference only before the replace of the leaf (the last touch).
            let leaf = walked.touches.len() - 1;
            let (head, tail) = walked.touches.split_at(leaf);
            commit_touches(&mut mem, head);
            let cas_before = mem.cas.len();
            mem.before_cas = vec![(PT + 16, read | other)];
            assert_eq!(commit_touches(&mut mem, tail), 0, "other {other:#x}");
            assert_eq!(
                entry(&mem.bytes, PT + 16),
                read | other | 0x60,
                "other {other:#x}: the observed bits are kept, A and D set"
            );
            // First replace on the stale value fails, the second (over the observed one) wins,
            // unless the observed value already holds A and D (no second replace needed).
            let made = mem.cas.len() - cas_before;
            assert_eq!(
                made,
                if other & 0x60 == 0x60 { 1 } else { 2 },
                "other {other:#x}"
            );
        }
    }

    /// S6-31b: a stale touch of an entry that was zeroed does not make it nonzero (the reason an
    /// atomic OR without comparison is not enough), and the retry is bounded.
    #[test]
    fn a_zeroed_entry_stays_zero_and_an_endless_interference_ends() {
        let mut mem = CasOnly::new(tables_4k());
        let walked = translate(&mem, PARAMS, 0x2abc, AccessKind::Read).unwrap();
        for at in [PML4, PDPT, PD, PT + 16] {
            set(&mut mem.bytes, at, 0);
        }
        assert_eq!(commit_touches(&mut mem, &walked.touches), 4);
        for at in [PML4, PDPT, PD, PT + 16] {
            assert_eq!(entry(&mem.bytes, at), 0, "still zero");
        }

        // Another writer flips an ignored bit on every attempt: the touch gives up after the limit.
        struct Churn {
            bytes: Vec<u8>,
            calls: u32,
        }
        impl RealModeMemory for Churn {
            fn read_u8(&self, linear: u32) -> Option<u8> {
                self.bytes.get(linear as usize).copied()
            }
            fn can_write(&self, _linear: u32) -> bool {
                true
            }
            fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
                false
            }
            fn compare_replace_entry(
                &mut self,
                linear: u32,
                _expected: u64,
                _replacement: u64,
            ) -> Option<Result<(), u64>> {
                self.calls += 1;
                let e = entry(&self.bytes, linear as usize) ^ (1 << 9) ^ (1 << 10);
                set(&mut self.bytes, linear as usize, e);
                Some(Err(e))
            }
        }
        let mut churn = Churn {
            bytes: tables_4k(),
            calls: 0,
        };
        let touch = Touch {
            at: PT as u32,
            read: entry(&churn.bytes, PT),
            mask: 0x20,
        };
        assert_eq!(commit_touches(&mut churn, &[touch]), 1);
        assert_eq!(churn.calls, TOUCH_RETRY_LIMIT + 1);
    }

    /// S6-32 step 4: the walk reads each table entry with ONE `read_u64` and never with `read_u8`
    /// (the memory counts both). Every level of a 4 KiB and a 2 MiB walk, for every access kind.
    #[test]
    fn the_walk_reads_each_entry_with_one_read_u64_and_no_read_u8() {
        use std::cell::RefCell;
        struct Counting {
            bytes: Vec<u8>,
            u8_reads: RefCell<Vec<u32>>,
            u64_reads: RefCell<Vec<u32>>,
        }
        impl RealModeMemory for Counting {
            fn read_u8(&self, linear: u32) -> Option<u8> {
                self.u8_reads.borrow_mut().push(linear);
                self.bytes.get(linear as usize).copied()
            }
            fn read_u64(&self, linear: u32) -> Option<u64> {
                self.u64_reads.borrow_mut().push(linear);
                let at = linear as usize;
                let bytes: [u8; 8] = self.bytes.get(at..at + 8)?.try_into().ok()?;
                Some(u64::from_le_bytes(bytes))
            }
            fn can_write(&self, _linear: u32) -> bool {
                true
            }
            fn write_u8(&mut self, _linear: u32, _value: u8) -> bool {
                false
            }
        }
        for (linear, entries) in [
            (0x2abcu32, vec![PML4, PDPT, PD, PT + 16]),
            (0x20_0123, vec![PML4, PDPT, PD + 8]),
        ] {
            for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                let mut bytes = tables_4k();
                // PD[1] = 2 MiB page at 2 MiB.
                set(&mut bytes, PD + 8, 0x20_0000 | PS | 3);
                let mem = Counting {
                    bytes,
                    u8_reads: RefCell::new(Vec::new()),
                    u64_reads: RefCell::new(Vec::new()),
                };
                translate(&mem, PARAMS, linear, access).unwrap();
                assert_eq!(
                    *mem.u64_reads.borrow(),
                    entries.iter().map(|at| *at as u32).collect::<Vec<_>>(),
                    "{linear:#x} {access:?}: one read_u64 per level, in walk order"
                );
                assert!(
                    mem.u8_reads.borrow().is_empty(),
                    "{linear:#x} {access:?}: no read_u8 in the walk: {:?}",
                    mem.u8_reads.borrow()
                );
            }
        }
    }

    /// The default 8-byte read and compare-and-replace on an image: unreadable and unwritable
    /// entries are `None`, a mismatch writes nothing.
    #[test]
    fn the_default_entry_operations_on_an_image() {
        let mut ram = tables_4k();
        let mut mem = ImageMemoryMut {
            base: 0,
            bytes: &mut ram,
        };
        assert_eq!(mem.read_u64(PML4 as u32), Some(PDPT as u64 | 3));
        assert_eq!(
            mem.read_u64((RAM - 4) as u32),
            None,
            "crosses the image end"
        );
        assert_eq!(
            mem.compare_replace_entry(PML4 as u32, 1, 2),
            Some(Err(PDPT as u64 | 3))
        );
        assert_eq!(
            mem.compare_replace_entry(PML4 as u32, PDPT as u64 | 3, PDPT as u64 | 0x23),
            Some(Ok(()))
        );
        assert_eq!(mem.read_u64(PML4 as u32), Some(PDPT as u64 | 0x23));
        let mut ro = ImageMemory {
            base: 0,
            bytes: &ram,
        };
        assert_eq!(
            ro.compare_replace_entry(PML4 as u32, PDPT as u64 | 0x23, 0),
            None
        );
    }
}
