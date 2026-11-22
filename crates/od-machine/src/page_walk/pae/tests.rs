//! Tests of the PAE walk (S2-25h). There is no hardware oracle for PAE paging: the walk is checked
//! against the `od_isa` model (`translate_pae`, S2-25) on the same tables, and by hand against the SDM
//! (Vol. 3A section 5.4, Rev. 093 numbering).

use super::*;
use crate::ap_legacy_exec::BoardRam;
use crate::{MachineMemoryMap, MemoryRegionSpec, RamLimits};
use od_isa::legacy_mode::ProtectedSegment;
use od_isa::memory::{AccessOrigin, InstructionMode};
use od_isa::paging::{self as isa, PaeParams, PageRefusal, PageSize};
use od_isa::real_mode_exec::TableRegister;

const P: u64 = 1;
const RW: u64 = 1 << 1;
const US: u64 = 1 << 2;
const ACCESSED: u64 = 1 << 5;
const DIRTY: u64 = 1 << 6;
const PS: u64 = 1 << 7;
const XD: u64 = 1 << 63;

const PDPT: u64 = 0x1000;
const PD: u64 = 0x2000;
const PT: u64 = 0x3000;
/// The page directory of PDPTE3 (linear 0xC000_0000 and up).
const PD3: u64 = 0x4000;
const IMAGE: usize = 0x5000;
const RAM_END: u64 = 0x2_0000;
/// An address no region of the test memory covers.
const HOLE: u64 = 0x4_0000;

fn at(address: u64) -> GuestPhysicalAddress {
    GuestPhysicalAddress::new(address)
}

/// RAM below 0x2_0000 and a ROM page above it.
fn memory() -> PhysicalMemory {
    let range = |start: u64, len: u64| GuestPhysicalRange::new(at(start), len).unwrap();
    let regions = vec![
        MemoryRegionSpec::new(range(0, RAM_END), MemoryRegionKind::Ram),
        MemoryRegionSpec::new(range(RAM_END, 0x1000), MemoryRegionKind::Rom),
    ];
    PhysicalMemory::new(
        &MachineMemoryMap::new(36, regions).unwrap(),
        RamLimits::new(1 << 20),
    )
    .unwrap()
}

fn put(image: &mut [u8], address: u64, value: u64) {
    image[address as usize..address as usize + 8].copy_from_slice(&value.to_le_bytes());
}

/// PDPTE0 -> PD, PDPTE3 -> PD3; PD[0] -> PT; PD[1] = 2 MiB page at 0x20_0000; PD3[0] = 2 MiB page at
/// 0x40_0000; PT[i] = page 0x10 + i (physical 0x1_0000 + 0x1000 * i), every entry P, R/W and U/S.
fn tables() -> Vec<u8> {
    let mut image = vec![0u8; IMAGE];
    put(&mut image, PDPT, PD | P);
    put(&mut image, PDPT + 24, PD3 | P);
    put(&mut image, PD, PT | P | RW | US);
    put(&mut image, PD + 8, 0x20_0000 | PS | P | RW | US);
    put(&mut image, PD3, 0x40_0000 | PS | P | RW | US);
    for page in 0..8u64 {
        put(&mut image, PT + 8 * page, (0x10 + page) << 12 | P | RW | US);
    }
    image
}

fn load(memory: &PhysicalMemory, image: &[u8]) {
    memory.store_bytes(at(0), image, |_| {}).unwrap();
}

fn dump(memory: &PhysicalMemory) -> Vec<u8> {
    let mut bytes = vec![0u8; IMAGE];
    memory.load_bytes(at(0), &mut bytes).unwrap();
    bytes
}

fn pdptes(image: &[u8]) -> [u64; 4] {
    std::array::from_fn(|i| {
        let at = PDPT as usize + 8 * i;
        u64::from_le_bytes(image[at..at + 8].try_into().unwrap())
    })
}

fn config(image: &[u8], wp: bool, nxe: bool) -> PaeProbeConfig {
    let cr0 = 0x8000_0011 | if wp { CR0_WP } else { 0 };
    let efer = if nxe { EFER_NXE } else { 0 };
    PaeProbeConfig::new(cr0, PDPT, 0x20, efer, pdptes(image)).unwrap()
}

fn ctx(privilege: PrivilegeLevel) -> AccessContext {
    AccessContext {
        mode: InstructionMode::Long64,
        privilege,
        origin: AccessOrigin::Explicit,
    }
}

fn walk(
    memory: &PhysicalMemory,
    config: &PaeProbeConfig,
    linear: u32,
    privilege: PrivilegeLevel,
    access: AccessKind,
) -> Result<PaeTranslation, TrackedAccessError> {
    translate_pae_ram(
        memory,
        config,
        0x1234,
        linear,
        ctx(privilege),
        access,
        |_| {},
    )
}

fn fault(error: TrackedAccessError) -> (PageFaultReason, u32, u64) {
    match error {
        TrackedAccessError::PageFault { record, reason } => (
            reason,
            record.fault().error_code.unwrap(),
            record.cr2_value(),
        ),
        other => panic!("expected a page fault, got {other:?}"),
    }
}

fn flat_segment() -> ProtectedSegment {
    ProtectedSegment {
        selector: 0x10,
        base: 0,
        limit: 0xFFFF_FFFF,
        access: 0x93,
        flags: 0x0C,
    }
}

// ---- configuration and PDPTE loading --------------------------------------------------------

#[test]
fn the_configuration_checks_the_pae_register_state() {
    let image = tables();
    let ok = |cr0, cr3, cr4, efer| PaeProbeConfig::new(cr0, cr3, cr4, efer, pdptes(&image));
    assert!(ok(0x8000_0011, PDPT, 0x20, 0).is_ok());
    assert!(ok(0x8001_0011, PDPT, 0x20, EFER_NXE | EFER_SCE).is_ok());
    // PG, PE, ET and PAE are required; the allowed-bit masks are those of the IA-32e snapshot.
    assert!(matches!(
        ok(0x0000_0011, PDPT, 0x20, 0),
        Err(PaeConfigError::Config(ProbeConfigError::RequiredBits {
            register: "CR0",
            ..
        }))
    ));
    assert!(matches!(
        ok(0x8000_0001, PDPT, 0x20, 0),
        Err(PaeConfigError::Config(
            ProbeConfigError::Cr0EtRequired { .. }
        ))
    ));
    assert!(matches!(
        ok(0x8000_0011, PDPT, 0x00, 0),
        Err(PaeConfigError::Config(ProbeConfigError::RequiredBits {
            register: "CR4",
            ..
        }))
    ));
    assert!(matches!(
        ok(0x8000_0011, PDPT, 0x20 | 1 << 12, 0),
        Err(PaeConfigError::Config(ProbeConfigError::UnsupportedBits {
            register: "CR4",
            ..
        }))
    ));
    // IA-32e (LME, LMA) is not PAE paging; CR3 is a 32-bit value.
    for efer in [EFER_LME, EFER_LMA, EFER_LME | EFER_LMA] {
        assert!(matches!(
            ok(0x8000_0011, PDPT, 0x20, efer),
            Err(PaeConfigError::Config(ProbeConfigError::UnsupportedBits {
                register: "EFER",
                ..
            }))
        ));
    }
    assert!(matches!(
        ok(0x8000_0011, 1 << 32, 0x20, 0),
        Err(PaeConfigError::Config(ProbeConfigError::InvalidCr3 { .. }))
    ));
}

#[test]
fn pdptes_are_loaded_from_cr3_without_its_low_five_bits_and_checked_for_reserved_bits() {
    let memory = memory();
    let mut image = tables();
    load(&memory, &image);
    assert_eq!(
        load_pae_pdptes(&memory, PDPT | 0x1f),
        Ok([PD | P, 0, 0, PD3 | P])
    );
    let config = PaeProbeConfig::load(&memory, 0x8000_0011, PDPT | 0x1f, 0x20, 0).unwrap();
    assert_eq!(config.pdptes(), [PD | P, 0, 0, PD3 | P]);
    // Reserved in a PDPTE with P = 1: bits 2:1, 8:5 and 63:36 (SDM Vol. 3A Table 5-8).
    for bit in [1, 2, 5, 6, 7, 8, 36, 51, 52, 62, 63] {
        put(&mut image, PDPT + 8, PD | P | 1 << bit);
        load(&memory, &image);
        assert_eq!(
            load_pae_pdptes(&memory, PDPT),
            Err(PaeConfigError::PdpteReserved {
                index: 1,
                pdpte: PD | P | 1 << bit
            }),
            "bit {bit}"
        );
    }
    // Bits 4:3 (PWT, PCD) and 11:9 are not reserved; a reserved bit in a PDPTE with P = 0 is not
    // checked (an implementation choice shared with od_isa).
    put(&mut image, PDPT + 8, PD | P | 0b11 << 3 | 0b111 << 9);
    put(&mut image, PDPT + 16, 1 << 40);
    load(&memory, &image);
    assert!(load_pae_pdptes(&memory, PDPT).is_ok());
    // The PDPT must be backed memory: here it is a hole of the memory map.
    assert!(matches!(
        load_pae_pdptes(&memory, HOLE),
        Err(PaeConfigError::PdpteUnreadable { .. })
    ));
}

#[test]
fn a_protected_mode_model_on_pae_paging_is_a_pae_snapshot() {
    let image = tables();
    let mut cpu = od_isa::protected_exec::ProtectedCpu {
        gpr: [0; 8],
        eip: 0,
        rip_high: 0,
        target_entered: false,
        eflags: 2,
        cr0: 0x8001_0011,
        cr3: PDPT as u32,
        cr4: 0x20,
        efer: EFER_NXE,
        pdptes: pdptes(&image),
        pse36: false,
        cs: flat_segment(),
        ds: flat_segment(),
        es: flat_segment(),
        ss: flat_segment(),
        fs: flat_segment(),
        gs: flat_segment(),
        gdtr: TableRegister {
            base: 0,
            limit: 0xFFFF,
        },
        idtr: TableRegister {
            base: 0,
            limit: 0xFFFF,
        },
        mov_ss_shadow: false,
    };
    let config = PaeProbeConfig::from_protected_cpu(&cpu).unwrap();
    assert_eq!(config.pdptes(), pdptes(&image));
    assert_eq!(
        (config.cr0(), config.cr3(), config.cr4()),
        (0x8001_0011, PDPT, 0x20)
    );
    assert_eq!(config.efer(), EFER_NXE);
    // Without paging, or in IA-32e mode, it is not.
    cpu.cr0 = 0x0000_0011;
    assert!(PaeProbeConfig::from_protected_cpu(&cpu).is_err());
    cpu.cr0 = 0x8000_0011;
    cpu.efer = EFER_LME | EFER_LMA;
    assert!(PaeProbeConfig::from_protected_cpu(&cpu).is_err());
}

// ---- 4 KiB and 2 MiB translations --------------------------------------------------------------

#[test]
fn a_4k_translation_sets_accessed_in_the_pde_and_pte_and_never_touches_the_pdpt() {
    let memory = memory();
    let image = tables();
    load(&memory, &image);
    let config = config(&image, true, true);
    let mut written = Vec::new();
    let read = translate_pae_ram(
        &memory,
        &config,
        0x1234,
        0x2123,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Read,
        |range| written.push(range),
    )
    .unwrap();
    assert_eq!(read.physical, at(0x1_2123));
    assert_eq!(read.page_size, PaePageSize::Size4K);
    assert_eq!(read.leaf_entry_address, at(PT + 16));
    assert_eq!(read.leaf_entry_value, (0x12 << 12) | P | RW | US | ACCESSED);
    assert_eq!(read.table_pages, [None, None, Some(at(PD)), Some(at(PT))]);
    assert_eq!(read.ad_updates.len(), 2);
    assert_eq!(written.len(), 2);
    let after = dump(&memory);
    assert_eq!(
        &after[..PD as usize],
        &image[..PD as usize],
        "PDPT untouched"
    );
    assert_eq!(after[PD as usize] & ACCESSED as u8, ACCESSED as u8);
    assert_eq!(after[PT as usize + 16] & ACCESSED as u8, ACCESSED as u8);
    // A write sets D in the PTE as well (and nothing else changes in the PD).
    let write = walk(
        &memory,
        &config,
        0x2123,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(write.ad_updates.len(), 1);
    assert_eq!(write.ad_updates[0].bit, AdBit::Dirty);
    assert_eq!(write.ad_updates[0].level, PageTableLevel::Pt);
    let after = dump(&memory);
    assert_eq!(
        after[PT as usize + 16] & (ACCESSED | DIRTY) as u8,
        (ACCESSED | DIRTY) as u8
    );
    // Nothing is left to set: a second write commits nothing.
    let again = walk(
        &memory,
        &config,
        0x2123,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap();
    assert!(again.ad_updates.is_empty());
}

#[test]
fn a_2m_translation_ends_at_the_pde_with_the_21_bit_offset() {
    let memory = memory();
    let image = tables();
    load(&memory, &image);
    let config = config(&image, true, true);
    let write = walk(
        &memory,
        &config,
        0x2f_edcb,
        PrivilegeLevel::Ring0,
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(write.physical, at(0x2f_edcb));
    assert_eq!(write.page_size, PaePageSize::Size2M);
    assert_eq!(write.leaf_entry_address, at(PD + 8));
    assert_eq!(write.table_pages, [None, None, Some(at(PD)), None]);
    let kinds: Vec<_> = write.ad_updates.iter().map(|u| (u.level, u.bit)).collect();
    assert_eq!(
        kinds,
        [
            (PageTableLevel::Pd, AdBit::Accessed),
            (PageTableLevel::Pd, AdBit::Dirty)
        ]
    );
    let after = dump(&memory);
    assert_eq!(
        after[PD as usize + 8] & (ACCESSED | DIRTY) as u8,
        (ACCESSED | DIRTY) as u8
    );
    // The PDPTE selected by bits 31:30: PDPTE3 -> PD3, a 2 MiB page at 0x40_0000.
    let high = walk(
        &memory,
        &config,
        0xC012_3456,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(high.physical, at(0x52_3456));
    assert_eq!(high.table_pages[2], Some(at(PD3)));
    // The trace holds the PD entry only; the PDPTE is a register.
    let (result, trace) = translate_pae_ram_read_only_with_trace(
        &memory,
        &config,
        0,
        0x20_0000,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Read,
    );
    assert!(result.is_ok());
    assert_eq!(trace.len(), 1);
    assert_eq!(trace.entries()[0].unwrap().level, PageTableLevel::Pd);
    assert_eq!(trace.entries()[0].unwrap().index, 1);
}

#[test]
fn a_physical_address_above_4_gib_is_translated_unlike_in_the_od_isa_model() {
    // MAXPHYADDR = 36: a PTE may map 0xF_FFFF_F000. The od_isa model refuses it as a model limit.
    let memory = memory();
    let mut image = tables();
    put(&mut image, PT + 16, 0xF_FFFF_F000 | P | RW | US);
    load(&memory, &image);
    let config = config(&image, true, true);
    let translation = walk(
        &memory,
        &config,
        0x2abc,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(translation.physical, at(0xF_FFFF_FABC));
}

#[test]
fn the_read_only_forms_commit_nothing_and_a_bad_entry_read_is_a_typed_error() {
    let memory = memory();
    let image = tables();
    load(&memory, &image);
    let config = config(&image, true, true);
    let before = dump(&memory);
    let read_only = translate_pae_ram_read_only(
        &memory,
        &config,
        0x1234,
        0x2123,
        ctx(PrivilegeLevel::Ring0),
        AccessKind::Write,
    )
    .unwrap();
    assert_eq!(read_only.physical, at(0x1_2123));
    assert!(read_only.ad_updates.is_empty());
    assert_eq!(dump(&memory), before);
    // A PDPTE that points the page directory at a hole of the memory map: the PD entry read fails.
    let mut image = tables();
    put(&mut image, PDPT, HOLE | P);
    load(&memory, &image);
    let config = PaeProbeConfig::new(0x8000_0011, PDPT, 0x20, 0, pdptes(&image)).unwrap();
    let error = walk(
        &memory,
        &config,
        0x1000,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        TrackedAccessError::TableRead {
            level: PageTableLevel::Pd,
            ..
        }
    ));
}

// ---- faults ------------------------------------------------------------------------------------

#[test]
fn a_not_present_pdpte_faults_at_the_pdpt_level_without_any_table_read() {
    let memory = memory();
    let image = tables();
    load(&memory, &image);
    let config = config(&image, true, true);
    for (linear, access, code) in [
        (0x4000_0000u32, AccessKind::Read, 0u32),
        (0x8000_0000, AccessKind::Write, 2),
    ] {
        let (result, trace) = translate_pae_ram_read_only_with_trace(
            &memory,
            &config,
            0x77,
            linear,
            ctx(PrivilegeLevel::Ring0),
            access,
        );
        let (reason, error_code, cr2) = fault(result.unwrap_err());
        assert_eq!(
            reason,
            PageFaultReason::NotPresent {
                level: PageTableLevel::Pdpt
            }
        );
        assert_eq!((error_code, cr2), (code, u64::from(linear)));
        assert!(trace.is_empty());
    }
}

#[test]
fn not_present_pde_and_pte_fault_with_the_p_bit_clear_in_the_error_code() {
    let memory = memory();
    let mut image = tables();
    put(&mut image, PT + 16, (0x12 << 12) | RW | US); // P = 0
    put(&mut image, PD + 16, PT | RW | US); // PDE2 P = 0
    load(&memory, &image);
    let config = config(&image, true, true);
    let (reason, code, _) = fault(
        walk(
            &memory,
            &config,
            0x2000,
            PrivilegeLevel::Ring3,
            AccessKind::Read,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::NotPresent {
            level: PageTableLevel::Pt
        }
    );
    assert_eq!(code, 4, "U/S only");
    let (reason, code, _) = fault(
        walk(
            &memory,
            &config,
            0x40_0000,
            PrivilegeLevel::Ring0,
            AccessKind::Write,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::NotPresent {
            level: PageTableLevel::Pd
        }
    );
    assert_eq!(code, 2, "W/R only");
}

#[test]
fn bits_62_to_36_are_reserved_in_a_pde_and_a_pte_unlike_the_four_level_walk() {
    let memory = memory();
    let config_for = |image: &[u8]| config(image, true, true);
    for bit in [36, 40, 51, 52, 58, 62] {
        // PTE
        let mut image = tables();
        put(&mut image, PT + 16, (0x12 << 12) | P | RW | US | 1 << bit);
        load(&memory, &image);
        let (reason, code, _) = fault(
            walk(
                &memory,
                &config_for(&image),
                0x2000,
                PrivilegeLevel::Ring0,
                AccessKind::Read,
            )
            .unwrap_err(),
        );
        assert_eq!(
            reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pt,
                offending_bits: 1 << bit
            },
            "PTE bit {bit}"
        );
        assert_eq!(code, 1 | 8, "P and RSVD");
        // PDE
        let mut image = tables();
        put(&mut image, PD, PT | P | RW | US | 1 << bit);
        load(&memory, &image);
        let (reason, _, _) = fault(
            walk(
                &memory,
                &config_for(&image),
                0x2000,
                PrivilegeLevel::Ring0,
                AccessKind::Read,
            )
            .unwrap_err(),
        );
        assert_eq!(
            reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pd,
                offending_bits: 1 << bit
            },
            "PDE bit {bit}"
        );
    }
    // Bits 63 (XD, with NXE) and 35:12 (address) are not reserved.
    let mut image = tables();
    put(&mut image, PT + 16, (0x12 << 12) | P | RW | US | XD);
    load(&memory, &image);
    assert!(
        walk(
            &memory,
            &config_for(&image),
            0x2000,
            PrivilegeLevel::Ring0,
            AccessKind::Read
        )
        .is_ok()
    );
}

#[test]
fn bits_20_to_13_of_a_2m_pde_are_reserved_and_bit_12_is_the_pat_bit() {
    let memory = memory();
    for bit in 13..=20 {
        let mut image = tables();
        put(&mut image, PD + 8, 0x20_0000 | PS | P | RW | US | 1 << bit);
        load(&memory, &image);
        let (reason, _, _) = fault(
            walk(
                &memory,
                &config(&image, true, true),
                0x20_0000,
                PrivilegeLevel::Ring0,
                AccessKind::Read,
            )
            .unwrap_err(),
        );
        assert_eq!(
            reason,
            PageFaultReason::ReservedBits {
                level: PageTableLevel::Pd,
                offending_bits: 1 << bit
            },
            "bit {bit}"
        );
    }
    let mut image = tables();
    put(&mut image, PD + 8, 0x20_0000 | PS | P | RW | US | 1 << 12);
    load(&memory, &image);
    let translation = walk(
        &memory,
        &config(&image, true, true),
        0x20_0abc,
        PrivilegeLevel::Ring0,
        AccessKind::Read,
    )
    .unwrap();
    assert_eq!(
        translation.physical,
        at(0x20_0abc),
        "bit 12 is not an address bit of a 2 MiB page"
    );
}

#[test]
fn xd_is_reserved_without_nxe_and_denies_only_fetches_with_it() {
    let memory = memory();
    let mut image = tables();
    put(&mut image, PT + 16, (0x12 << 12) | P | RW | US | XD);
    put(&mut image, PD + 8, 0x20_0000 | PS | P | RW | US | XD);
    load(&memory, &image);
    for linear in [0x2000u32, 0x20_0000] {
        // NXE = 0: XD is a reserved bit, for every access kind (PTE and 2 MiB PDE alike).
        let no_nx = config(&image, true, false);
        for access in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
            let (reason, code, _) =
                fault(walk(&memory, &no_nx, linear, PrivilegeLevel::Ring0, access).unwrap_err());
            assert!(
                matches!(
                    reason,
                    PageFaultReason::ReservedBits {
                        offending_bits: XD,
                        ..
                    }
                ),
                "{linear:#x} {access:?}"
            );
            assert_eq!(code & (1 | 8), 1 | 8);
            assert_eq!(code & 16, 0, "I/D is reported only with NXE");
        }
        // NXE = 1: reads and writes pass, a fetch is a protection fault with I/D.
        let nx = config(&image, true, true);
        assert!(
            walk(
                &memory,
                &nx,
                linear,
                PrivilegeLevel::Ring0,
                AccessKind::Read
            )
            .is_ok()
        );
        assert!(
            walk(
                &memory,
                &nx,
                linear,
                PrivilegeLevel::Ring0,
                AccessKind::Write
            )
            .is_ok()
        );
        let (reason, code, _) = fault(
            walk(
                &memory,
                &nx,
                linear,
                PrivilegeLevel::Ring0,
                AccessKind::Fetch,
            )
            .unwrap_err(),
        );
        assert_eq!(
            reason,
            PageFaultReason::Protection {
                user_denied: false,
                write_denied: false,
                execute_denied: true
            }
        );
        assert_eq!(code, 1 | 16);
    }
    // XD in the PDE alone also denies the fetch through the PTE (the rights are the OR of the levels).
    let mut image = tables();
    put(&mut image, PD, PT | P | RW | US | XD);
    load(&memory, &image);
    assert!(
        walk(
            &memory,
            &config(&image, true, true),
            0x2000,
            PrivilegeLevel::Ring0,
            AccessKind::Fetch
        )
        .is_err()
    );
}

#[test]
fn write_protection_and_the_user_bit_follow_cr0_wp_and_the_cpl() {
    let memory = memory();
    let mut image = tables();
    put(&mut image, PT + 16, (0x12 << 12) | P | US); // read-only page
    put(&mut image, PT + 24, (0x13 << 12) | P | RW); // supervisor page
    load(&memory, &image);
    let wp = config(&image, true, true);
    let no_wp = config(&image, false, true);
    // Supervisor write to a read-only page: faults with WP = 1, passes with WP = 0.
    let (reason, code, _) = fault(
        walk(
            &memory,
            &wp,
            0x2000,
            PrivilegeLevel::Ring0,
            AccessKind::Write,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::Protection {
            user_denied: false,
            write_denied: true,
            execute_denied: false
        }
    );
    assert_eq!(code, 1 | 2);
    assert!(
        walk(
            &memory,
            &no_wp,
            0x2000,
            PrivilegeLevel::Ring0,
            AccessKind::Write
        )
        .is_ok()
    );
    // A CPL 3 write to a read-only page faults whatever WP says, with the U/S bit in the code.
    let (_, code, _) = fault(
        walk(
            &memory,
            &no_wp,
            0x2000,
            PrivilegeLevel::Ring3,
            AccessKind::Write,
        )
        .unwrap_err(),
    );
    assert_eq!(code, 1 | 2 | 4);
    // A CPL 3 access to a supervisor page faults; the supervisor reads it.
    let (reason, _, _) = fault(
        walk(
            &memory,
            &wp,
            0x3000,
            PrivilegeLevel::Ring3,
            AccessKind::Read,
        )
        .unwrap_err(),
    );
    assert_eq!(
        reason,
        PageFaultReason::Protection {
            user_denied: true,
            write_denied: false,
            execute_denied: false
        }
    );
    assert!(
        walk(
            &memory,
            &wp,
            0x3000,
            PrivilegeLevel::Ring0,
            AccessKind::Read
        )
        .is_ok()
    );
    // A descriptor-table access is made with supervisor privilege whatever the CPL (page_privilege).
    let table = AccessContext {
        origin: AccessOrigin::DescriptorTable,
        ..ctx(PrivilegeLevel::Ring3)
    };
    assert!(translate_pae_ram(&memory, &wp, 0, 0x3000, table, AccessKind::Read, |_| {}).is_ok());
    // The rights are those of the PDE and the PTE together: a supervisor PDE hides a user PTE.
    let mut image = tables();
    put(&mut image, PD, PT | P | RW);
    load(&memory, &image);
    assert!(
        walk(
            &memory,
            &config(&image, true, true),
            0x2000,
            PrivilegeLevel::Ring3,
            AccessKind::Read
        )
        .is_err()
    );
}

// ---- differential against the od_isa model -------------------------------------------------------

#[derive(Debug, Eq, PartialEq)]
enum Outcome {
    Mapped { physical: u64, large: bool },
    NotPresent(u8),
    Reserved(u8),
    WriteProtected,
    NoExecute,
}

fn od_isa_outcome(
    ram: &BoardRam<'_>,
    params: PaeParams,
    linear: u32,
    kind: AccessKind,
) -> (Outcome, Vec<isa::Touch>) {
    let isa_kind = match kind {
        AccessKind::Read => isa::AccessKind::Read,
        AccessKind::Write => isa::AccessKind::Write,
        AccessKind::Fetch => isa::AccessKind::Fetch,
    };
    match isa::translate_pae(ram, params, linear, isa_kind) {
        Ok(t) => (
            Outcome::Mapped {
                physical: u64::from(t.phys),
                large: t.size == PageSize::Size2M,
            },
            t.touches,
        ),
        Err(PageRefusal::NotPresent { level, .. }) => (Outcome::NotPresent(level), Vec::new()),
        Err(PageRefusal::ReservedBits { level, .. }) => (Outcome::Reserved(level), Vec::new()),
        Err(PageRefusal::WriteProtected { .. }) => (Outcome::WriteProtected, Vec::new()),
        Err(PageRefusal::NoExecute { .. }) => (Outcome::NoExecute, Vec::new()),
        Err(other) => panic!("unexpected od_isa refusal {other:?}"),
    }
}

fn od_machine_outcome(
    memory: &PhysicalMemory,
    config: &PaeProbeConfig,
    linear: u32,
    kind: AccessKind,
) -> Outcome {
    match walk(memory, config, linear, PrivilegeLevel::Ring0, kind) {
        Ok(t) => Outcome::Mapped {
            physical: t.physical.get(),
            large: t.page_size == PaePageSize::Size2M,
        },
        Err(error) => match fault(error).0 {
            PageFaultReason::NotPresent { level } => Outcome::NotPresent(match level {
                PageTableLevel::Pdpt => 3,
                PageTableLevel::Pd => 2,
                PageTableLevel::Pt => 1,
                PageTableLevel::Pml4 => unreachable!("PAE paging has no PML4"),
            }),
            PageFaultReason::ReservedBits { level, .. } => Outcome::Reserved(match level {
                PageTableLevel::Pd => 2,
                PageTableLevel::Pt => 1,
                other => panic!("reserved bits at {other:?}"),
            }),
            PageFaultReason::Protection {
                write_denied: true,
                user_denied: false,
                execute_denied: false,
            } => Outcome::WriteProtected,
            PageFaultReason::Protection {
                execute_denied: true,
                user_denied: false,
                write_denied: false,
            } => Outcome::NoExecute,
            other => panic!("unexpected protection fault {other:?}"),
        },
    }
}

fn next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// A random PDE/PTE: mostly present, random R/W, U/S, A, D, XD and ignored bits, now and then a
/// reserved bit (36..=62 or, for a 2 MiB PDE, 13..=20).
fn random_entry(state: &mut u64, address: u64, large: bool) -> u64 {
    let r = next(state);
    let mut entry = address;
    if r & 7 != 0 {
        entry |= P;
    }
    for (shift, bit) in [(3, RW), (4, US), (5, ACCESSED), (6, DIRTY), (9, XD)] {
        if (r >> shift) & 1 != 0 && !(bit == XD && (r >> 10) & 1 != 0) {
            entry |= bit;
        }
    }
    entry |= ((r >> 11) & 7) << 9; // ignored bits 11:9
    let reserved = next(state);
    if reserved.is_multiple_of(9) {
        entry |= 1 << (36 + (reserved >> 4) % 27);
    }
    if large {
        entry |= PS;
        if next(state).is_multiple_of(9) {
            entry |= 1 << (13 + next(state) % 8);
        }
        if next(state).is_multiple_of(4) {
            entry |= 1 << 12; // PAT: not an address bit of a 2 MiB page
        }
    }
    entry
}

#[test]
fn the_od_machine_walk_and_the_od_isa_walk_agree_on_random_tables() {
    let machine = memory();
    let isa_memory = memory();
    let mut state = 0x5EED_2025_u64;
    let mut counts = [0usize; 5];
    let mut mapped_large = 0;
    for _ in 0..400 {
        let mut image = tables();
        let pde0_large = next(&mut state).is_multiple_of(3);
        let pde1_large = next(&mut state).is_multiple_of(2);
        let pde0_address = if pde0_large {
            (next(&mut state) & 0x7FF) << 21
        } else {
            PT
        };
        let pde0 = random_entry(&mut state, pde0_address, pde0_large);
        let pde1_address = if pde1_large {
            (next(&mut state) & 0x7FF) << 21
        } else {
            PT
        };
        let pde1 = random_entry(&mut state, pde1_address, pde1_large);
        put(&mut image, PD, pde0);
        put(&mut image, PD + 8, pde1);
        for page in 1..3u64 {
            let address = (next(&mut state) & 0xF_FFFF) << 12;
            let mut pte = random_entry(&mut state, address, false);
            if next(&mut state).is_multiple_of(3) {
                pte |= PS; // the PAT bit of a PTE: no meaning for the walk
            }
            put(&mut image, PT + 8 * page, pte);
        }
        for (wp, nxe) in [(false, false), (true, false), (false, true), (true, true)] {
            load(&machine, &image);
            load(&isa_memory, &image);
            let config = config(&image, wp, nxe);
            let params = PaeParams {
                pdptes: pdptes(&image),
                wp,
                nxe,
            };
            for linear in [
                0x1abcu32,
                0x2abc,
                0x20_1234,
                0x20_0000,
                0x4000_0000,
                0xC000_1234,
            ] {
                for kind in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
                    load(&machine, &image);
                    load(&isa_memory, &image);
                    let mut ram = BoardRam::new(&isa_memory);
                    let (expected, touches) = od_isa_outcome(&ram, params, linear, kind);
                    let got = od_machine_outcome(&machine, &config, linear, kind);
                    assert_eq!(
                        got, expected,
                        "linear {linear:#x} {kind:?} wp {wp} nxe {nxe} pde0 {pde0:#x} pde1 {pde1:#x}"
                    );
                    match &expected {
                        Outcome::Mapped { large, .. } => {
                            counts[0] += 1;
                            mapped_large += usize::from(*large);
                            // The accessed/dirty updates leave the same table bytes. (On a refused walk
                            // od_machine has already set the accessed bits it passed, od_isa none: not
                            // compared.)
                            assert_eq!(isa::commit_touches(&mut ram, &touches), 0);
                            assert_eq!(
                                dump(&machine),
                                dump(&isa_memory),
                                "tables after {linear:#x} {kind:?}"
                            );
                        }
                        Outcome::NotPresent(_) => counts[1] += 1,
                        Outcome::Reserved(_) => counts[2] += 1,
                        Outcome::WriteProtected => counts[3] += 1,
                        Outcome::NoExecute => counts[4] += 1,
                    }
                }
            }
        }
    }
    // The sweep reaches every outcome, 2 MiB pages included.
    assert!(counts.iter().all(|&n| n > 100), "{counts:?}");
    assert!(mapped_large > 100, "{mapped_large}");
}

#[test]
fn the_read_only_walk_is_the_committing_walk_without_the_writes() {
    let memory = memory();
    let mut state = 77u64;
    for _ in 0..200 {
        let mut image = tables();
        let large = next(&mut state).is_multiple_of(2);
        let pde = random_entry(&mut state, if large { 0x20_0000 } else { PT }, large);
        put(&mut image, PD, pde);
        put(
            &mut image,
            PT + 16,
            random_entry(&mut state, 0x1_2000, false),
        );
        let config = config(&image, true, true);
        for kind in [AccessKind::Read, AccessKind::Write, AccessKind::Fetch] {
            load(&memory, &image);
            let read_only = translate_pae_ram_read_only(
                &memory,
                &config,
                0x1234,
                0x2abc,
                ctx(PrivilegeLevel::Ring0),
                kind,
            );
            assert_eq!(dump(&memory), image, "no write by the read-only walk");
            let committing = walk(&memory, &config, 0x2abc, PrivilegeLevel::Ring0, kind);
            match (read_only, committing) {
                (Ok(a), Ok(b)) => {
                    assert_eq!((a.physical, a.page_size), (b.physical, b.page_size));
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                (a, b) => panic!("{a:?} vs {b:?}"),
            }
        }
    }
}
