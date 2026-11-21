//! Read-only diagnostic access to a stopped vCPU and to its guest-virtual memory (S8-10).
//!
//! The runner takes a stop snapshot between quanta, after the machine owner has stopped
//! executing. Everything here only reads: the translation walks the guest page tables without
//! setting accessed or dirty bits, and the bytes are read with [`PhysicalMemory::load_bytes`]
//! (RAM, ROM and framebuffer only: MMIO is never touched, so no device register changes).
//! The walk understands 4 KiB, 2 MiB and 1 GiB pages of the 4-level format. Reserved bits are not
//! validated (a diagnostic reports what the tables say), and 5-level paging (CR4.LA57) is refused.

use crate::MemoryAccessError;
use od_isa::CpuActivity;

const PRESENT: u64 = 1;
const WRITABLE: u64 = 1 << 1;
const USER: u64 = 1 << 2;
const PAGE_SIZE_BIT: u64 = 1 << 7;
const NO_EXECUTE: u64 = 1 << 63;
/// Physical address bits 51:12 of a paging-structure entry.
const ENTRY_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const CR0_PG: u64 = 1 << 31;
const CR4_LA57: u64 = 1 << 12;
pub(crate) const DIAGNOSTIC_PAGE_BYTES: u64 = 0x1000;

/// Registers of one vCPU at the moment of a stop. A copy: reading it changes nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VcpuDiagnosticState {
    pub vcpu: usize,
    pub rip: u64,
    pub rflags: u64,
    /// RAX, RCX, RDX, RBX, RSP, RBP, RSI, RDI, R8..R15 (hardware encoding order).
    pub gpr: [u64; 16],
    pub cr0: Option<u64>,
    pub cr2: Option<u64>,
    /// The page-table root the board walks from (the root register of this vCPU).
    pub cr3: Option<u64>,
    pub cr4: Option<u64>,
    /// The guest-visible `IA32_EFER`.
    pub efer: u64,
    pub cs_selector: Option<u16>,
    /// `CS.selector & 3`; `None` when the state carries no CS selector.
    pub cpl: Option<u8>,
    /// The vCPU executes in the 64-bit state model (an application processor still in the
    /// real-mode or protected-mode model has its registers elsewhere and is not described here).
    pub long_mode_model: bool,
    pub activity: Option<CpuActivity>,
}

/// Why a guest-virtual address could not be translated.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticTranslateError {
    NoSuchVcpu,
    /// The vCPU is not in the 64-bit state model, has no CR3 or CR0.PG is clear.
    NoLongModePaging,
    /// CR4.LA57 is set: 5-level paging is not walked.
    UnsupportedLa57,
    NonCanonical,
    /// A paging-structure entry is not present. `level`: 4 PML4E, 3 PDPTE, 2 PDE, 1 PTE.
    NotPresent {
        level: u8,
        entry: u64,
    },
    /// Bit 7 of a PML4E is reserved and must be clear.
    ReservedPageSize {
        level: u8,
        entry: u64,
    },
    /// The paging structure at `table` is not readable RAM or ROM.
    TableUnreadable {
        level: u8,
        table: u64,
        error: MemoryAccessError,
    },
}

/// A successful translation. The flags are the combined effect of every level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticTranslation {
    pub physical: u64,
    /// 4 KiB, 2 MiB or 1 GiB.
    pub page_bytes: u64,
    pub writable: bool,
    pub user: bool,
    pub no_execute: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DiagnosticReadError {
    Translate(DiagnosticTranslateError),
    /// The translated physical address is not readable RAM, ROM or framebuffer.
    Physical {
        physical: u64,
        error: MemoryAccessError,
    },
}

/// A read that stopped early: `read` leading bytes of the output are valid.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiagnosticReadFailure {
    pub read: usize,
    /// The guest-virtual address at which the read stopped.
    pub linear: u64,
    pub error: DiagnosticReadError,
}

/// Walks the 4-level tables from `cr3` for `linear`. `read_entry` loads the 8 bytes at a physical
/// address. Nothing is written, so accessed and dirty bits stay as the guest left them.
pub(crate) fn translate_long64(
    mut read_entry: impl FnMut(u64) -> Result<u64, MemoryAccessError>,
    cr3: u64,
    linear: u64,
) -> Result<DiagnosticTranslation, DiagnosticTranslateError> {
    let sign = (linear as i64) >> 47;
    if sign != 0 && sign != -1 {
        return Err(DiagnosticTranslateError::NonCanonical);
    }
    let mut table = cr3 & ENTRY_ADDRESS_MASK;
    let (mut writable, mut user, mut no_execute) = (true, true, false);
    for level in (1..=4u8).rev() {
        let index = (linear >> (12 + 9 * (u32::from(level) - 1))) & 0x1ff;
        let entry = read_entry(table + index * 8).map_err(|error| {
            DiagnosticTranslateError::TableUnreadable {
                level,
                table,
                error,
            }
        })?;
        if entry & PRESENT == 0 {
            return Err(DiagnosticTranslateError::NotPresent { level, entry });
        }
        writable &= entry & WRITABLE != 0;
        user &= entry & USER != 0;
        no_execute |= entry & NO_EXECUTE != 0;
        let page_bytes = match (level, entry & PAGE_SIZE_BIT != 0) {
            (4, true) => {
                return Err(DiagnosticTranslateError::ReservedPageSize { level, entry });
            }
            (3, true) => 1u64 << 30,
            (2, true) => 1u64 << 21,
            (1, _) => DIAGNOSTIC_PAGE_BYTES,
            _ => {
                table = entry & ENTRY_ADDRESS_MASK;
                continue;
            }
        };
        let offset_mask = page_bytes - 1;
        return Ok(DiagnosticTranslation {
            physical: (entry & ENTRY_ADDRESS_MASK & !offset_mask) | (linear & offset_mask),
            page_bytes,
            writable,
            user,
            no_execute,
        });
    }
    unreachable!("level 1 always returns")
}

/// Checks that the control registers allow the 4-level walk of this vCPU.
pub(crate) fn require_long_mode_paging(
    cr0: Option<u64>,
    cr3: Option<u64>,
    cr4: Option<u64>,
    long_mode_model: bool,
) -> Result<u64, DiagnosticTranslateError> {
    let (Some(cr3), true) = (cr3, long_mode_model) else {
        return Err(DiagnosticTranslateError::NoLongModePaging);
    };
    if cr0.is_some_and(|cr0| cr0 & CR0_PG == 0) {
        return Err(DiagnosticTranslateError::NoLongModePaging);
    }
    if cr4.is_some_and(|cr4| cr4 & CR4_LA57 != 0) {
        return Err(DiagnosticTranslateError::UnsupportedLa57);
    }
    Ok(cr3)
}

/// Reads `output.len()` bytes at the guest-virtual address `linear`, page by page. `read_physical`
/// loads bytes at a physical address; `translate` is [`translate_long64`] bound to a root.
pub(crate) fn read_virtual(
    mut translate: impl FnMut(u64) -> Result<DiagnosticTranslation, DiagnosticTranslateError>,
    mut read_physical: impl FnMut(u64, &mut [u8]) -> Result<(), MemoryAccessError>,
    linear: u64,
    output: &mut [u8],
) -> Result<(), DiagnosticReadFailure> {
    let mut done = 0usize;
    while done < output.len() {
        let address = linear.wrapping_add(done as u64);
        let fail = |error| DiagnosticReadFailure {
            read: done,
            linear: address,
            error,
        };
        let translation =
            translate(address).map_err(|error| fail(DiagnosticReadError::Translate(error)))?;
        // Chunks never cross a 4 KiB boundary, so one translation covers each chunk.
        let to_boundary = DIAGNOSTIC_PAGE_BYTES - (address & (DIAGNOSTIC_PAGE_BYTES - 1));
        let chunk = (output.len() - done).min(to_boundary as usize);
        read_physical(translation.physical, &mut output[done..done + chunk]).map_err(|error| {
            fail(DiagnosticReadError::Physical {
                physical: translation.physical,
                error,
            })
        })?;
        done += chunk;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Fake physical memory of 8-byte entries; an absent address is unreadable.
    struct Tables(HashMap<u64, u64>);

    impl Tables {
        fn reader(&self) -> impl FnMut(u64) -> Result<u64, MemoryAccessError> + '_ {
            |address| {
                self.0
                    .get(&address)
                    .copied()
                    .ok_or(MemoryAccessError::AddressOverflow)
            }
        }
    }

    const CR3: u64 = 0x1000;
    const RW_US: u64 = PRESENT | WRITABLE | USER;

    /// PML4 -> 0x2000 (PDPT) -> 0x3000 (PD) -> 0x4000 (PT) for linear 0x0000_0080_4020_1000.
    fn four_level(pte: u64) -> (Tables, u64) {
        let linear = (1u64 << 39) | (1 << 30) | (1 << 21) | (1 << 12);
        let mut tables = HashMap::new();
        tables.insert(CR3 + 8, 0x2000 | RW_US);
        tables.insert(0x2000 + 8, 0x3000 | RW_US);
        tables.insert(0x3000 + 8, 0x4000 | RW_US);
        tables.insert(0x4000 + 8, pte);
        (Tables(tables), linear)
    }

    #[test]
    fn walks_4k_2m_and_1g_pages_with_the_offset_inside_the_page() {
        let (tables, linear) = four_level(0x7000_0000 | RW_US);
        let found = translate_long64(tables.reader(), CR3, linear | 0x123).unwrap();
        assert_eq!(found.physical, 0x7000_0123);
        assert_eq!(found.page_bytes, 0x1000);
        assert!(found.writable && found.user && !found.no_execute);

        // 2 MiB: the PDE at level 2 has PS; offset bits 20:0 come from the linear address.
        let mut huge = four_level(0).0;
        huge.0
            .insert(0x3000 + 8, 0x4000_0000 | RW_US | PAGE_SIZE_BIT);
        let found = translate_long64(huge.reader(), CR3, linear | 0x1f_ffff).unwrap();
        assert_eq!(found.physical, 0x4000_0000 + 0x1f_ffff);
        assert_eq!(found.page_bytes, 1 << 21);

        // 1 GiB: the PDPTE has PS; offset bits 29:0 come from the linear address.
        let mut giant = four_level(0).0;
        giant.0.insert(
            0x2000 + 8,
            0x8000_0000 | PRESENT | PAGE_SIZE_BIT | NO_EXECUTE,
        );
        let found = translate_long64(giant.reader(), CR3, linear | 0x3fff_ffff).unwrap();
        assert_eq!(found.physical, 0x8000_0000 + 0x3fff_ffff);
        assert_eq!(found.page_bytes, 1 << 30);
        assert!(!found.writable && !found.user && found.no_execute);
    }

    #[test]
    fn reports_the_failing_level_and_never_writes() {
        let (tables, linear) = four_level(0);
        assert_eq!(
            translate_long64(tables.reader(), CR3, linear),
            Err(DiagnosticTranslateError::NotPresent { level: 1, entry: 0 })
        );
        let (mut tables, linear) = four_level(0);
        tables.0.remove(&0x3008);
        assert!(matches!(
            translate_long64(tables.reader(), CR3, linear),
            Err(DiagnosticTranslateError::TableUnreadable {
                level: 2,
                table: 0x3000,
                ..
            })
        ));
        let (mut tables, linear) = four_level(0);
        tables.0.insert(CR3 + 8, 0x2000 | RW_US | PAGE_SIZE_BIT);
        assert!(matches!(
            translate_long64(tables.reader(), CR3, linear),
            Err(DiagnosticTranslateError::ReservedPageSize { level: 4, .. })
        ));
        // A non-canonical address is refused before any table is read.
        assert_eq!(
            translate_long64(|_| unreachable!(), CR3, 0x0001_0000_0000_0000),
            Err(DiagnosticTranslateError::NonCanonical)
        );
        // The upper canonical half walks from PML4 index 511.
        assert_eq!(
            translate_long64(
                |address| {
                    assert_eq!(address, CR3 + 511 * 8);
                    Ok(0)
                },
                CR3,
                0xffff_ff80_0000_0000
            ),
            Err(DiagnosticTranslateError::NotPresent { level: 4, entry: 0 })
        );
    }

    #[test]
    fn control_registers_gate_the_walk() {
        assert_eq!(
            require_long_mode_paging(Some(0x8001_0033), Some(0x3000), Some(0x20), true),
            Ok(0x3000)
        );
        for (cr0, cr3, cr4, long, error) in [
            (
                Some(0x1),
                Some(0x3000),
                Some(0x20),
                true,
                DiagnosticTranslateError::NoLongModePaging,
            ),
            (
                Some(0x8001_0033),
                None,
                Some(0x20),
                true,
                DiagnosticTranslateError::NoLongModePaging,
            ),
            (
                Some(0x8001_0033),
                Some(0x3000),
                Some(0x20),
                false,
                DiagnosticTranslateError::NoLongModePaging,
            ),
            (
                Some(0x8001_0033),
                Some(0x3000),
                Some(0x1020),
                true,
                DiagnosticTranslateError::UnsupportedLa57,
            ),
        ] {
            assert_eq!(require_long_mode_paging(cr0, cr3, cr4, long), Err(error));
        }
    }

    #[test]
    fn a_read_crosses_pages_and_reports_how_much_was_read_before_the_failure() {
        // Two adjacent 4 KiB pages of the linear space map to unrelated physical pages; the
        // third is not present.
        let translate = |linear: u64| match linear >> 12 {
            0x10 => Ok(DiagnosticTranslation {
                physical: 0x9000 + (linear & 0xfff),
                page_bytes: 0x1000,
                writable: true,
                user: true,
                no_execute: false,
            }),
            0x11 => Ok(DiagnosticTranslation {
                physical: 0x2000 + (linear & 0xfff),
                page_bytes: 0x1000,
                writable: true,
                user: true,
                no_execute: false,
            }),
            _ => Err(DiagnosticTranslateError::NotPresent { level: 1, entry: 0 }),
        };
        let read = |physical: u64, out: &mut [u8]| {
            for (index, byte) in out.iter_mut().enumerate() {
                *byte = (physical as usize + index) as u8;
            }
            Ok(())
        };
        let mut output = [0u8; 8];
        read_virtual(translate, read, 0x10_ffc, &mut output).unwrap();
        assert_eq!(output, [0xfc, 0xfd, 0xfe, 0xff, 0x00, 0x01, 0x02, 0x03]);
        let mut long = [0u8; 0x2010];
        let failure = read_virtual(translate, read, 0x10_ff0, &mut long).unwrap_err();
        assert_eq!(failure.read, 0x10 + 0x1000);
        assert_eq!(failure.linear, 0x12_000);
        assert!(matches!(
            failure.error,
            DiagnosticReadError::Translate(DiagnosticTranslateError::NotPresent { level: 1, .. })
        ));
    }
}
